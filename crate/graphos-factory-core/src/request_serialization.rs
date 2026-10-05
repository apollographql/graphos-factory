//! `graphos-factory-core serialization`: whether every selected operation's
//! outbound request is proven, computed from its declared GraphQL
//! arguments, the connector wiring, the source's request-body shape, the
//! authored test cases and stubs, the resolved decisions, and the executed
//! result the recorded e2e log carries per case. A fixture on disk is never
//! read as executed, and an aggregate "the layer passed" is never read as a
//! specific case passing.
//!
//! It replaced four lint rules (`loose-write-body`, `list-arg-unproven`,
//! `mutation-cases`, `unit-no-body`; retired in ADR 0079 Step 2), which could
//! only say "no finding". It counts a numerator and denominator, requires a
//! null-vs-omission case where a decision documents one, the map input's
//! five cases, a required-only and full-body pair on a read-only POST, an
//! assertion for every fixed value the connector writes into a body, and
//! requires the proving case to have run. The verb is the standalone form
//! of the `write_body_proof` evidence layer. Ported from the
//! specialist-pipeline copy without its scoring harness.
//!
//! ## Scope this module actually computes (named, not hidden)
//!
//! - **Argument inventory**: every top-level declared GraphQL argument on a
//!   selected operation's root field, plus every nested `input` type member
//!   at any depth when the argument's own type resolves to a known `input`
//!   declaration (a cycle stops expanding rather than recursing forever, and
//!   never silently drops the member that closes it). A list of input
//!   objects is proven whole, by the
//!   exact match of the array at its pointer.
//! - **Where each argument goes** (`WireMap`, ADR 0079): the connector is
//!   read structurally. A body entry `key: $args.x` at any depth, inside
//!   `$({...})` or bare, is a JSON pointer; `$args.input { Wire: field }`
//!   places each member at its wire key; a plain `queryParams` entry is a
//!   query key; a `{$args.x}` placeholder is a URL path segment or a header
//!   value. A value is proven only at its own location: a body value by
//!   equality at its pointer, a query value by the key's matcher, a path or
//!   header value in its own placeholder. Anything else (a method chain,
//!   `??`, a `$this` read, a transformed value) is an **unproven mapping**,
//!   never a pass and never a search for the value elsewhere.
//! - **Omission**: an optional body argument needs an executed case that
//!   omits it behind an exact body assertion proving its pointer absent.
//! - **Explicit null**: what the connector sends for an explicit `null` is a
//!   judgement, `omit | send_null`: `selection.yaml`'s
//!   `defaults.null_handling`, overridden per argument by a resolved
//!   `null_handling` entry in `.factory/decisions.json`; prose is never
//!   read. With neither, the argument is undecided (no implicit default).
//!   `send_null` needs an executed case passing `null` behind a stub
//!   demanding JSON `null`; `omit` an executed case proving the key absent.
//!   An executed case showing the other behavior is unproven, whichever
//!   rule applied.
//! - **Map five-case contract**: an argument is map-shaped when its GraphQL
//!   type is a list of a known `input` type whose own fields include both
//!   `key` and `value` (the request-side mirror of `->entries`'s response
//!   convention). The five cases (empty, one entry, several entries with
//!   distinct keys, a nested-typed value, a non-GraphQL-name key) are read
//!   from the literal list argument's element texts in `tests/cases/*`.
//! - **Executed-evidence tie-in**: `.factory/evidence/latest.json`'s
//!   per-layer `status` gates whether a case counts at all, and the layer's
//!   recorded log (`layers.<layer>.log`) must be readable and carry the
//!   case's own `PASS: <name>` line (the convention `crate::cmd::evidence`
//!   emits). **Known contract gap**: `evidence/latest.json` records
//!   pass/fail per *operation*, not per *case*, so without the log no case
//!   is proven. The pilots do not commit their run logs, so on a clean
//!   checkout every pilot case reads as not recorded until `evidence` runs.

use crate::graphql;
use crate::json::{get, get_arr, get_obj, get_str, truthy, Object};
use crate::lint::{
    body_field_args, body_field_text, body_mapping, wiring, Arg, BodyMapping, Wiring,
};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

/// One obligation's outcome. `Unexecuted` means the instrument could not
/// run at all (no selection, inventory or schema), never a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationStatus {
    Pass,
    Fail,
    NotApplicable,
    Unexecuted,
}

impl ObligationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ObligationStatus::Pass => "pass",
            ObligationStatus::Fail => "fail",
            ObligationStatus::NotApplicable => "not_applicable",
            ObligationStatus::Unexecuted => "unexecuted",
        }
    }
}

/// What one unmet requirement is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// An argument, or one member of an input argument, has no executed
    /// case behind a stub that demands its value where the connector puts it.
    ArgumentUnasserted,
    /// The connector places the argument (or a documented body member) in a
    /// way this instrument does not classify, so no proof is attempted.
    UnprovenMapping,
    /// The source request body documents a member no argument feeds.
    SourceMemberUnmapped,
    /// The connector writes a fixed value at a body position (a literal, not
    /// an argument), and no executed case has a stub that demands that value
    /// there: the body could carry any other value and every layer would
    /// still pass.
    FixedValueUnasserted,
    /// A list argument has no executed case sending two or more distinct
    /// values in the exact wire encoding.
    ListUnproven,
    /// A write has no executed case passing every argument.
    FullCaseMissing,
    /// A write has no executed case passing only its required arguments.
    RequiredOnlyCaseMissing,
    /// An optional body argument has no executed case proving its key is
    /// absent (not null) from the body.
    OmissionUnproven,
    /// A non-flat body: omission cannot be proven by this instrument.
    OmissionUnprovable,
    /// No resolved decision records `null_handling` for a nullable body
    /// argument: what the connector sends for an explicit `null` is
    /// undecided, so it is unproven.
    NullHandlingUndecided,
    /// A resolved `null_handling` decision exists, and no executed case
    /// passes `null` behind a stub demanding that behavior.
    NullUnproven,
    /// A map-shaped input argument lacks one of its five cases.
    MapCaseMissing,
    /// Outside the connector-unit layer's range, and no passing e2e case.
    UnitWithoutE2e,
    /// The reader could not place the argument, and the executed cases
    /// cannot say where it goes either: its value sits at more than one body
    /// pointer, is too common to tell from other values, or is the same as
    /// another argument's in the same case. The fix is in the case, not the
    /// connector: give the argument a distinctive value.
    AmbiguousValue,
    /// A resolved request-direction omit decision covers a body member the
    /// source marks required. An omit records why the connector does not
    /// send an optional member; it cannot excuse a required one, so the
    /// member stays a gap and the message names the decision.
    RequiredMemberOmitted,
}

/// One unmet requirement, on one operation.
#[derive(Debug, Clone, Serialize)]
pub struct Gap {
    pub operation: String,
    pub kind: GapKind,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Obligation {
    pub id: String,
    pub status: ObligationStatus,
    pub message: String,
    pub evidence_ref: Option<String>,
    pub denominator: Option<(usize, usize)>,
    pub gaps: Vec<Gap>,
}

/// One selected operation whose connector sends a body with POST, PUT or
/// PATCH, and every gap this instrument found on it.
#[derive(Debug, Clone, Serialize)]
pub struct WriteReport {
    pub operation: String,
    pub field: String,
    pub method: String,
    pub arguments: usize,
    /// No argument, input member or source body member is left without an
    /// executed, exact outbound-body assertion.
    pub body_proven: bool,
    pub gaps: Vec<Gap>,
}

/// One case's own verdict (ADR 0079 Step 2, `evidence.schema.json`'s
/// `case_proofs`): whether it is actually recorded as executed and passing
/// at the e2e layer -- this module has no notion of a per-case unit-layer
/// verdict (every check above reads only the `wiremock_e2e` log), so a case
/// this instrument never touches (a read-only case, or one no selected
/// write references) is simply absent here, not reported "unproven".
#[derive(Debug, Clone, Serialize)]
pub struct CaseProof {
    pub verdict: String,
    pub findings: Vec<String>,
}

/// How one write argument's place in the outbound request was established
/// (ADR 0079, executed placement): read from the connector (`static`), or
/// found by execution (`executed`), where an executed, passing case's stub
/// demands the argument's own value at exactly one body pointer. `case` names
/// the proving case for an executed placement.
#[derive(Debug, Clone, Serialize)]
pub struct Placement {
    pub operation: String,
    pub argument: String,
    pub via: String,
    pub location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub writes: Vec<WriteReport>,
    pub obligations: Vec<Obligation>,
    pub case_proofs: std::collections::BTreeMap<String, CaseProof>,
    /// Where each write argument goes and which way that was established.
    pub placements: Vec<Placement>,
    /// Gaps on a body-sending write, and gaps on every other selected
    /// operation (a read's query arguments). Counted apart so a workspace
    /// whose writes are all proven is not summarised by its reads alone;
    /// the obligations still fail on either.
    pub write_gaps: usize,
    pub read_gaps: usize,
}

impl Report {
    fn new(
        writes: Vec<WriteReport>,
        obligations: Vec<Obligation>,
        case_proofs: std::collections::BTreeMap<String, CaseProof>,
        placements: Vec<Placement>,
    ) -> Report {
        let write_gaps: usize = writes.iter().map(|w| w.gaps.len()).sum();
        let all_gaps: usize = obligations.iter().map(|o| o.gaps.len()).sum();
        Report {
            writes,
            obligations,
            case_proofs,
            placements,
            write_gaps,
            read_gaps: all_gaps.saturating_sub(write_gaps),
        }
    }
}

// ── Textual scanning: bracket matching and call-argument extraction ────────
//
// Deliberately self-contained rather than reaching into `crate::lint`'s
// private helpers of the same shape (`matching_close`, `list_elements`):
// this module needs the actual *value text* of each passed argument (to
// tell a supplied value from an explicit `null`, and to read a list
// literal's or a map entry's real content), not just `lint::passed_args`'s
// element *count*. The scanning approach (blank strings/comments first,
// scan the blanked text for structure, slice the ORIGINAL text for content
// at the same offsets) is the same one `crate::lint::passed_args` already
// uses, applied one level further.

/// Index of the bracket that closes the one at `open` (any of `([{`).
fn matching_close(code: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in code.iter().enumerate().skip(open) {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The end (exclusive) of a single argument value starting at `start`,
/// scanning up to `limit` (the enclosing call's own close paren). A
/// bracketed or braced value runs to its match in `code` (blanked). A scalar
/// is one token, read from `orig` (the unblanked text, where a string's
/// quotes are still visible): a quoted or block string to its closing
/// quote, anything else to the next whitespace, comma or closing bracket.
/// GraphQL separates arguments with whitespace as well as commas, so a
/// scalar cannot run to the next comma: `a: "x"\n  b: 1` is two arguments.
fn value_end(code: &[u8], orig: &[u8], start: usize, limit: usize) -> usize {
    if start >= limit {
        return start;
    }
    if matches!(code[start], b'[' | b'{') {
        return matching_close(code, start)
            .map(|c| c + 1)
            .unwrap_or(limit)
            .min(limit);
    }
    if orig.get(start) == Some(&b'"') {
        if orig[start..limit].starts_with(b"\"\"\"") {
            let mut i = start + 3;
            while i + 3 <= limit {
                if &orig[i..i + 3] == b"\"\"\"" && orig[i - 1] != b'\\' {
                    return i + 3;
                }
                i += 1;
            }
            return limit;
        }
        let mut i = start + 1;
        while i < limit {
            match orig[i] {
                b'\\' => i += 2,
                b'"' => return i + 1,
                _ => i += 1,
            }
        }
        return limit;
    }
    let mut i = start;
    while i < limit {
        let c = orig[i];
        if c.is_ascii_whitespace() || matches!(c, b',' | b')' | b']' | b'}') {
            return i;
        }
        i += 1;
    }
    limit
}

/// Every call of `field(...)` in `document`, as the raw (original, unblanked)
/// text of each top-level `name: value` argument pair. Every call in the
/// document is returned as its own entry (never unioned across calls), so a
/// caller can tell "this case's one call omits the argument" from "some
/// other call in the same file happens to pass it".
fn call_arg_texts(document: &str, field: &str) -> Vec<Vec<(String, String)>> {
    let code = graphql::blank(document);
    let call = match Regex::new(&format!(r"\b{}\s*\(", regex::escape(field))) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    // Deliberately no trailing `\s*` after the colon: `graphql::blank`
    // turns a string literal's quotes and content into plain spaces, so a
    // greedy `\s*` here cannot tell "real whitespace" from "a blanked
    // string value" and would swallow the value entirely for anything
    // starting right after a single space (`id: "w1"` -> "id:" + 5
    // indistinguishable spaces). Whitespace after the colon is skipped
    // separately below, over the ORIGINAL (unblanked) bytes, where a real
    // quote character is never mistaken for whitespace.
    let name_re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*:").unwrap();
    let cbytes = code.as_bytes();
    let dbytes = document.as_bytes();
    let mut calls = Vec::new();
    for m in call.find_iter(&code) {
        // `mutation f($v: T) { f(a: $v) }`: the operation's own name, and its
        // variable definitions, are not a call of the field.
        if is_operation_name(&code, m.start()) {
            continue;
        }
        let open = m.end() - 1;
        let close = match matching_close(cbytes, open) {
            Some(c) => c,
            None => continue,
        };
        let mut out: Vec<(String, String)> = Vec::new();
        let mut depth = 0i32;
        let mut i = open + 1;
        while i < close {
            match cbytes[i] {
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth -= 1,
                _ => {}
            }
            // A byte inside a multibyte character (invalid GraphQL outside a
            // string, but a case file is user input) is never a slice start:
            // slicing a `str` there panics.
            if depth == 0 && code.is_char_boundary(i) {
                if let Some(nm) = name_re.captures(&code[i..close]) {
                    let name = nm[1].to_string();
                    let mut vstart = i + nm.get(0).unwrap().end();
                    while vstart < close
                        && dbytes
                            .get(vstart)
                            .map(|b| b.is_ascii_whitespace())
                            .unwrap_or(false)
                    {
                        vstart += 1;
                    }
                    let vend = value_end(cbytes, dbytes, vstart, close);
                    let raw = document
                        .get(vstart..vend)
                        .unwrap_or("")
                        .trim()
                        .trim_end_matches(',')
                        .trim()
                        .to_string();
                    out.push((name, raw));
                    i = vend.max(i + 1);
                    continue;
                }
            }
            i += 1;
        }
        calls.push(out);
    }
    // A field whose every argument is optional is valid GraphQL called with
    // no parentheses at all (`field { ... }`); the paren-anchored regex
    // above never sees it, so its omission/null proof was unreachable no
    // matter how the case was written (B6 -- the seven Mailchimp false
    // negatives all trace to this).
    if let Ok(bare) = Regex::new(&format!(r"\b{}\b", regex::escape(field))) {
        for m in bare.find_iter(&code) {
            if code[m.end()..].trim_start().starts_with('(') {
                continue; // already counted above
            }
            if is_operation_name(&code, m.start()) {
                continue; // `mutation f { f(...) }`: the name, not a call
            }
            calls.push(Vec::new());
        }
    }
    calls
}

/// Whether the identifier starting at `start` in `code` (blanked) is an
/// operation's name: the word right after `query`, `mutation` or
/// `subscription`.
fn is_operation_name(code: &str, start: usize) -> bool {
    let before = code[..start].trim_end();
    if before.len() == code[..start].len() {
        return false; // no whitespace between the keyword and the name
    }
    ["query", "mutation", "subscription"].iter().any(|kw| {
        before.strip_suffix(kw).is_some_and(|rest| {
            rest.chars()
                .next_back()
                .map_or(true, |c| !(c.is_ascii_alphanumeric() || c == '_'))
        })
    })
}

/// Top-level `name: value` pairs inside a `{ ... }` object literal, as raw
/// (original) text -- the same extraction `call_arg_texts` does for a call's
/// argument list, applied to an arbitrary object literal so a nested `input`
/// member can be read the same way a top-level argument is.
fn object_field_texts(literal: &str) -> Vec<(String, String)> {
    let code = graphql::blank(literal);
    let bytes = code.as_bytes();
    let open = match bytes.iter().position(|&b| b == b'{') {
        Some(p) => p,
        None => return Vec::new(),
    };
    let close = match matching_close(bytes, open) {
        Some(c) => c,
        None => return Vec::new(),
    };
    // See `call_arg_texts`'s identical comment: no trailing `\s*` here
    // either, and whitespace after the colon is skipped over the ORIGINAL
    // bytes, not the blanked ones.
    let name_re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*:").unwrap();
    let lbytes = literal.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut i = open + 1;
    while i < close {
        match bytes[i] {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            _ => {}
        }
        // See `call_arg_texts`: never slice inside a multibyte character.
        if depth == 0 && code.is_char_boundary(i) {
            if let Some(nm) = name_re.captures(&code[i..close]) {
                let name = nm[1].to_string();
                let mut vstart = i + nm.get(0).unwrap().end();
                while vstart < close
                    && lbytes
                        .get(vstart)
                        .map(|b| b.is_ascii_whitespace())
                        .unwrap_or(false)
                {
                    vstart += 1;
                }
                let vend = value_end(bytes, lbytes, vstart, close);
                let raw = literal
                    .get(vstart..vend)
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(',')
                    .trim()
                    .to_string();
                out.push((name, raw));
                i = vend.max(i + 1);
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Top-level elements of a `[ ... ]` literal, as trimmed raw (original) text.
/// GraphQL commas between list elements are optional, so this tokenizes by
/// value boundary (`value_end`, the same one `call_arg_texts` and
/// `object_field_texts` use), never by splitting on a literal `,` byte in
/// the ORIGINAL text: that byte can sit inside a quoted string (`"red,blue"`
/// is one element, not two -- B5) and a comma-free list (`["a" "b"]`) has no
/// separator to split on at all.
fn list_elements_text(literal: &str) -> Vec<String> {
    let code = graphql::blank(literal);
    let bytes = code.as_bytes();
    let lbytes = literal.as_bytes();
    let open = match bytes.iter().position(|&b| b == b'[') {
        Some(p) => p,
        None => return Vec::new(),
    };
    let close = matching_close(bytes, open).unwrap_or(bytes.len().saturating_sub(1));
    let mut out = Vec::new();
    let mut i = open + 1;
    while i < close {
        while i < close && (lbytes[i].is_ascii_whitespace() || lbytes[i] == b',') {
            i += 1;
        }
        if i >= close {
            break;
        }
        let end = value_end(bytes, lbytes, i, close);
        if let Some(raw) = literal.get(i..end) {
            let t = raw.trim();
            if !t.is_empty() {
                out.push(t.to_string());
            }
        }
        i = end.max(i + 1);
    }
    out
}

fn is_null_literal(v: &str) -> bool {
    v.trim() == "null"
}

/// Any GraphQL value -- container or scalar -- as JSON, built structurally
/// from `object_field_texts`/`list_elements_text` (which already read
/// GraphQL's optional-comma, whitespace-separated syntax correctly)
/// rather than patching bracket/comma text and hoping `serde_json` accepts
/// it: that approach could parse neither a comma-free object/list literal
/// nor a bare enum token, both valid GraphQL this instrument's own fixtures
/// use (B6). A bare identifier that is not `true`/`false`/`null` is read as
/// its JSON string wire form.
fn graphql_member_to_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.starts_with('{') {
        let mut map = serde_json::Map::new();
        for (k, v) in object_field_texts(t) {
            map.insert(k, graphql_member_to_json(&v)?);
        }
        Some(Value::Object(map))
    } else if t.starts_with('[') {
        list_elements_text(t)
            .iter()
            .map(|e| graphql_member_to_json(e))
            .collect::<Option<Vec<Value>>>()
            .map(Value::Array)
    } else if let Ok(v) = serde_json::from_str::<Value>(t) {
        Some(v)
    } else if Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap().is_match(t) {
        Some(Value::String(t.to_string()))
    } else {
        None
    }
}

// ── GraphQL argument model ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ArgInfo {
    name: String,
    type_text: String,
    /// Exactly one level of the argument's own `input` type members, when
    /// its bare type resolves to a known `input` declaration; empty
    /// otherwise. Deeper nesting is a named, bounded gap (see module doc).
    /// Shared, not copied: every argument of every operation whose type is
    /// the same `input` reads the one expansion `SchemaCache` holds.
    nested: Rc<[ArgInfo]>,
}

impl ArgInfo {
    fn bare_type(&self) -> &str {
        self.type_text
            .trim_end_matches('!')
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('!')
    }
    fn is_list(&self) -> bool {
        self.type_text.trim_end_matches('!').starts_with('[')
    }
    fn is_required(&self) -> bool {
        self.type_text.ends_with('!')
    }
}

/// Every `input Name { field: Type }` member at any depth (a member whose
/// own bare type is itself a known `input` type recurses the same way,
/// cycles stopped by `seen` rather than silently dropped), read the same way
/// `crate::graphql::field_names` reads a type body's own field names (a
/// depth-tracked scan so a nested brace -- a directive argument list, most
/// commonly -- does not read as a sibling field), extended to also capture
/// the declared type text.
fn input_type_fields_rec(
    bodies: &graphql::BodyIndex,
    inputs: &HashSet<String>,
    type_name: &str,
    seen: &mut Vec<String>,
) -> Vec<ArgInfo> {
    if seen.iter().any(|s| s == type_name) {
        return Vec::new();
    }
    let body = match bodies.type_body(type_name) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let code = graphql::blank(&body.body);
    static MEMBER_RE: OnceLock<Regex> = OnceLock::new();
    let re = MEMBER_RE.get_or_init(|| {
        Regex::new(
            r"([A-Za-z_][A-Za-z0-9_]*)\s*:\s*((?:\[\s*)*[A-Za-z_][A-Za-z0-9_]*\s*!?(?:\s*\]\s*!?)*)",
        )
        .unwrap()
    });
    seen.push(type_name.to_string());
    let mut depth = 0i32;
    let mut out = Vec::new();
    for raw in code.split('\n') {
        let line = raw.trim();
        if depth == 0 {
            // Every field on the line, not only the first: GraphQL commas
            // are optional, so more than one field can share a line, and a
            // single-match scan silently dropped every sibling after the
            // first (B1). `line_depth` gates matching on parens/braces
            // opened earlier on this same line (a directive's argument
            // list, most commonly), which a match must never read into.
            let mut pos = 0usize;
            let mut line_depth = 0i32;
            let bytes = line.as_bytes();
            while pos < bytes.len() {
                if line_depth == 0 {
                    if let Some(m) = re.captures(&line[pos..]) {
                        let whole = m.get(0).unwrap();
                        if whole.start() == 0 {
                            let name = m[1].to_string();
                            let type_text: String =
                                m[2].chars().filter(|c| !c.is_whitespace()).collect();
                            let bare = type_text
                                .trim_end_matches('!')
                                .trim_start_matches('[')
                                .trim_end_matches(']')
                                .trim_end_matches('!')
                                .to_string();
                            let nested = if inputs.contains(&bare) {
                                input_type_fields_rec(bodies, inputs, &bare, seen)
                            } else {
                                Vec::new()
                            };
                            out.push(ArgInfo {
                                name,
                                type_text,
                                nested: nested.into(),
                            });
                            pos += whole.end();
                            continue;
                        }
                    }
                }
                match bytes[pos] {
                    b'(' | b'{' => line_depth += 1,
                    b')' | b'}' => line_depth -= 1,
                    _ => {}
                }
                pos += 1;
            }
        }
        let opens = line.chars().filter(|c| matches!(c, '(' | '{')).count() as i32;
        let closes = line.chars().filter(|c| matches!(c, ')' | '}')).count() as i32;
        depth += opens - closes;
        if depth < 0 {
            depth = 0;
        }
    }
    seen.pop();
    out
}

/// What the report reads from the schema text. Every selected operation
/// asks the same questions of the same document (which names are `input`
/// types, what a root type's body and fields declare, what an `input`
/// type's members are), and each answer used to be a scan of the whole
/// SDL: asked per operation, HubSpot's 1.4 MB schema with 113 operations
/// never finished (0.5.75). The answers do not depend on the operation, so
/// the document is scanned twice per report (`type_declarations` and
/// `BodyIndex::new`) and every answer after that is cached.
struct SchemaCache<'s> {
    bodies: graphql::BodyIndex<'s>,
    inputs: HashSet<String>,
    roots: RefCell<HashMap<String, Rc<RootBody>>>,
    input_fields: RefCell<HashMap<String, Rc<[ArgInfo]>>>,
}

/// One root type's body, read once: its text, `graphql::blank` of it, and
/// its fields' declared arguments. Empty when the schema has no such root.
#[derive(Default)]
struct RootBody {
    body: String,
    code: String,
    args: Vec<(String, Vec<Arg>)>,
}

impl<'s> SchemaCache<'s> {
    fn new(sdl: &'s str) -> Self {
        let inputs = graphql::type_declarations(sdl)
            .into_iter()
            .filter(|d| d.kind == "input")
            .map(|d| d.name)
            .collect();
        SchemaCache {
            bodies: graphql::BodyIndex::new(sdl),
            inputs,
            roots: RefCell::new(HashMap::new()),
            input_fields: RefCell::new(HashMap::new()),
        }
    }

    // Each lookup releases its borrow before computing a miss and borrows
    // again to insert, so a computation that itself asks the cache (a
    // memoized nested expansion, say) never meets a live `borrow_mut`.

    fn root(&self, root_title: &str) -> Rc<RootBody> {
        let hit = self.roots.borrow().get(root_title).cloned();
        if let Some(root) = hit {
            return root;
        }
        let root = Rc::new(match self.bodies.type_body(root_title) {
            Some(b) => {
                let code = graphql::blank(&b.body);
                RootBody {
                    args: body_field_args(&code),
                    body: b.body,
                    code,
                }
            }
            None => RootBody::default(),
        });
        self.roots
            .borrow_mut()
            .insert(root_title.to_string(), root.clone());
        root
    }

    fn input_type_fields(&self, type_name: &str) -> Rc<[ArgInfo]> {
        let hit = self.input_fields.borrow().get(type_name).cloned();
        if let Some(fields) = hit {
            return fields;
        }
        let fields: Rc<[ArgInfo]> =
            input_type_fields_rec(&self.bodies, &self.inputs, type_name, &mut Vec::new()).into();
        self.input_fields
            .borrow_mut()
            .insert(type_name.to_string(), fields.clone());
        fields
    }

    /// A map-shaped argument (the request-side mirror of a response's
    /// `->entries`): its bare type is a known `input` type whose own fields
    /// include both `key` and `value`.
    fn is_map_entry_type(&self, type_name: &str) -> bool {
        let fields = self.input_type_fields(type_name);
        fields.iter().any(|f| f.name == "key") && fields.iter().any(|f| f.name == "value")
    }
}

fn args_with_nesting(schema: &SchemaCache, top: &[Arg]) -> Vec<ArgInfo> {
    top.iter()
        .map(|a| {
            let bare = a
                .type_
                .trim_end_matches('!')
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_end_matches('!');
            let nested = if schema.inputs.contains(bare) {
                schema.input_type_fields(bare)
            } else {
                Rc::from(Vec::new())
            };
            ArgInfo {
                name: a.name.clone(),
                type_text: a.type_.clone(),
                nested,
            }
        })
        .collect()
}

// ── Source request-body shape (ground truth from the inventory) ────────────

fn deref<'a>(shape: &'a Value, shapes: &'a Object) -> Option<&'a Value> {
    let mut cur = shape;
    for _ in 0..32 {
        match get_str(cur, "$ref") {
            Some(r) => cur = shapes.get(r.trim_start_matches("#/shapes/"))?,
            None => return Some(cur),
        }
    }
    None
}

/// The top-level property names of a selected operation's documented
/// `request_body` shape (deref'd through `$ref`), one level only -- the same
/// bound this module uses for GraphQL `input` nesting. Empty when the
/// operation documents no request body or the body has no object shape.
/// The members the source marks required in the operation's request body
/// (the resolved shape's own `required` list), whatever the connector does.
pub fn required_request_members(op: &Value, shapes: &Object) -> Vec<String> {
    let Some(shape_ref) = get(op, "request_body").and_then(|rb| get_str(rb, "shape_ref")) else {
        return Vec::new();
    };
    shapes
        .get(shape_ref.trim_start_matches("#/shapes/"))
        .and_then(|s| deref(s, shapes))
        .and_then(|r| get_arr(r, "required"))
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn request_body_member_names(op: &Value, shapes: &Object) -> Vec<String> {
    let shape_ref = match get(op, "request_body").and_then(|rb| get_str(rb, "shape_ref")) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let shape = match shapes.get(shape_ref.trim_start_matches("#/shapes/")) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let resolved = match deref(shape, shapes) {
        Some(s) => s,
        None => return Vec::new(),
    };
    get_obj(resolved, "properties")
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default()
}

// ── Cases, mappings, evidence ───────────────────────────────────────────────

fn read(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap_or_default()
}

fn load_cases(dir: &Path) -> Vec<(String, String)> {
    let case_dir = dir.join("tests").join("cases");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&case_dir) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "graphql").unwrap_or(false))
            .collect();
        files.sort();
        for f in files {
            out.push((
                f.file_stem().unwrap().to_string_lossy().to_string(),
                read(&f),
            ));
        }
    }
    out
}

fn load_mappings(dir: &Path) -> Vec<(String, Value)> {
    let mapping_dir = dir.join("tests").join("fixtures").join("mappings");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&mapping_dir) {
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
            .collect();
        files.sort();
        for f in files {
            if let Ok(m) = crate::json::parse(&read(&f)) {
                out.push((f.file_name().unwrap().to_string_lossy().to_string(), m));
            }
        }
    }
    out
}

/// Same case/stub classification `e2e.sh` and `crate::lint`'s test-shape
/// rules already use: an explicit `x-cases` tag, an explicit `x-shared`
/// stub, or a matching basename (hyphen/underscore interchangeable).
fn serves(mapping_name: &str, mapping: &Value, case: &str) -> bool {
    let case = case.replace('-', "_");
    let metadata = get(mapping, "metadata");
    if let Some(Value::Array(list)) = metadata.and_then(|m| crate::json::field(m, "x-cases")) {
        return list
            .iter()
            .filter_map(Value::as_str)
            .any(|c| c.replace('-', "_") == case);
    }
    if metadata.and_then(|m| crate::json::field(m, "x-shared")) == Some(&Value::Bool(true)) {
        return true;
    }
    mapping_name.trim_end_matches(".json").replace('-', "_") == case
}

fn load_evidence(dir: &Path) -> Option<Value> {
    crate::factory_io::read_to_string_optional(dir, ".factory/evidence/latest.json")
        .ok()
        .flatten()
        .and_then(|s| crate::json::parse(&s).ok())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaseExec {
    /// The layer that would prove this case passed, and the case's own
    /// `PASS:`/absence-of-log line does not contradict it.
    Proven,
    /// The layer ran and this specific case is recorded as having failed.
    Failed,
    /// No evidence file, the relevant layer did not pass, or (when a log is
    /// readable) this case's name never appears in it -- a fixture existing
    /// on disk is never read as "executed".
    NotRecorded(String),
}

/// Whether `case_name` counts as an *executed, passing* proof under
/// `layer_key` (`"wiremock_e2e"` or `"connector_unit"`): the layer passed,
/// its recorded log is readable, and the log carries this case's own
/// `PASS:` line. An operation-level `pass` with no readable per-case record
/// is not proof.
fn case_executed(
    workspace: &Path,
    evidence: Option<&Value>,
    layer_key: &str,
    case_name: &str,
) -> CaseExec {
    // What THIS case's own record says, read whatever the aggregate status
    // is: a failed case is reported failed even when the layer failed.
    let record = case_record(workspace, evidence, layer_key, case_name);
    if matches!(record, CaseExec::Failed) {
        return record;
    }
    // Whether a case can carry an obligation is a separate question: a
    // layer that did not pass supports nothing, even a case that passed.
    let status = evidence
        .and_then(|e| get(e, "layers"))
        .and_then(|l| get(l, layer_key))
        .and_then(|l| get_str(l, "status"))
        .unwrap_or("");
    if status != "pass" && !matches!(record, CaseExec::NotRecorded(_)) {
        return CaseExec::NotRecorded(format!("{} status is {:?}, not pass", layer_key, status));
    }
    record
}

/// The per-case verdict the layer's log records for `case_name`, without
/// judging the layer's aggregate status (see [`case_executed`]).
fn case_record(
    workspace: &Path,
    evidence: Option<&Value>,
    layer_key: &str,
    case_name: &str,
) -> CaseExec {
    let evidence = match evidence {
        Some(e) => e,
        None => return CaseExec::NotRecorded("no .factory/evidence/latest.json".into()),
    };
    let layer = match get(evidence, "layers").and_then(|l| get(l, layer_key)) {
        Some(l) => l,
        None => return CaseExec::NotRecorded(format!("no {} layer recorded", layer_key)),
    };
    let log_rel = match get_str(layer, "log") {
        Some(l) => l,
        None => {
            return CaseExec::NotRecorded(format!(
                "{} layer recorded no per-case log path -- an aggregate layer status is not per-case proof",
                layer_key
            ))
        }
    };
    // The log lives under `.factory/evidence/`: custody's (ADR 0025).
    let log_text = match crate::factory_io::read_to_string(workspace, log_rel) {
        Ok(t) => t,
        Err(e) => {
            return CaseExec::NotRecorded(format!(
                "{} layer's recorded log ({}) is unreadable: {} -- a missing/unreadable per-case record blocks, it does not fall back to the aggregate status",
                layer_key, log_rel, e
            ))
        }
    };
    let case_norm = case_name.replace('-', "_");
    let case_re = Regex::new(r"^(PASS|FAIL):\s*([A-Za-z0-9_-]+)").unwrap();
    let mut seen = false;
    for line in log_text.lines() {
        if let Some(m) = case_re.captures(line.trim()) {
            if m[2].replace('-', "_") == case_norm {
                seen = true;
                if &m[1] == "FAIL" {
                    return CaseExec::Failed;
                }
            }
        }
    }
    if seen {
        CaseExec::Proven
    } else {
        CaseExec::NotRecorded(format!(
            "no per-case PASS/FAIL line for \"{}\" in {}",
            case_name, log_rel
        ))
    }
}

// ── Per-operation analysis ──────────────────────────────────────────────────

struct OpCtx<'a> {
    key: String,
    field: String,
    verb: String,
    args: Vec<ArgInfo>,
    wire: Wiring,
    places: WireMap,
    body_flat: Option<Vec<(String, String)>>, // (arg, body key), only when Flat
    calls: Vec<(&'a str, Vec<(String, String)>)>, // (case name, one actual call's own args -- never unioned with a sibling call in the same file, B2)
    /// Source `request_body` shape members with no corresponding argument
    /// anywhere the connector's text references (by name, or via a bare
    /// `$args.<name>` scan) -- populated after `build_ctx` returns, once the
    /// caller also has the inventory operation and shape table in hand.
    body_args_unmatched: Vec<String>,
    /// Source members at a body position this reader does not classify.
    body_members_unclassified: Vec<String>,
    /// Arguments the reader could not place, whose place an executed case
    /// established: argument name -> the proving case.
    exec: std::collections::BTreeMap<String, String>,
    /// Arguments the reader could not place and the cases cannot either:
    /// argument name -> why (`ambiguous_value`).
    ambiguous: std::collections::BTreeMap<String, String>,
    /// Required source body members a resolved omit decision covers:
    /// (member, decision id).
    required_omitted: Vec<(String, String)>,
}

fn selected_ops<'a>(selection: &'a Value, prefix: &str) -> Vec<(String, &'a Value, String)> {
    get_obj(selection, "operations")
        .into_iter()
        .flatten()
        .filter(|(_, e)| truthy(get(e, "include")))
        .filter_map(|(k, e)| {
            let name = get(e, "graphql").and_then(|g| get_str(g, "name"))?;
            Some((k.clone(), e, format!("{}_{}", prefix, name)))
        })
        .collect()
}

/// The marker `e2e.sh` greps a case file for: the case passes only when its
/// upstream request matches no loaded stub.
const EXPECT_UNMATCHED: &str = "expect-unmatched-upstream";

fn build_ctx<'a>(
    schema: &SchemaCache,
    key: &str,
    entry: &Value,
    field: &str,
    inventory_method: Option<String>,
    cases: &'a [(String, String)],
) -> Option<OpCtx<'a>> {
    let root = get(entry, "graphql")
        .and_then(|g| get_str(g, "root"))
        .unwrap_or("query");
    let root_title = if root == "query" { "Query" } else { "Mutation" };
    let root_body = schema.root(root_title);
    let text = body_field_text(&root_body.body, &root_body.code, field);
    // A field with no arguments is not in `root_field_args` at all, but it
    // is still a selected operation: a write with a constant body must be
    // judged, not dropped from the report. Only a field the schema does not
    // declare is skipped (lint's own selection rules report that).
    let args_top = match root_body.args.iter().find(|(f, _)| f.as_str() == field) {
        Some((_, a)) => a.clone(),
        None if text.is_some() => Vec::new(),
        None => return None,
    };
    let args = args_with_nesting(schema, &args_top);
    let wire = text.as_deref().map(wiring).unwrap_or_default();
    let places = text
        .as_deref()
        .map(|t| wire_map(t, &wire))
        .unwrap_or_default();
    let body_flat = text.as_deref().and_then(|t| match body_mapping(t) {
        BodyMapping::Flat(v) => Some(v),
        _ => None,
    });
    let verb = wire.verb.clone().or(inventory_method).unwrap_or_default();
    // Every actual call keeps its own args, never unioned with a sibling
    // call in the same file: two aliased partial calls must not look like
    // one full call, and a null passed by one alias must not be silently
    // overwritten by a value a sibling alias happens to pass for the same
    // argument (B2).
    //
    // A case marked `expect-unmatched-upstream` passes at e2e exactly when
    // its request matched NO stub (`e2e.sh` greps the file for the marker),
    // so its `PASS:` line says nothing about what any stub demands: it never
    // proves anything here.
    let calls: Vec<(&str, Vec<(String, String)>)> = cases
        .iter()
        .filter(|(_, doc)| !doc.contains(EXPECT_UNMATCHED))
        .flat_map(|(name, doc)| {
            call_arg_texts(doc, field)
                .into_iter()
                .map(move |call| (name.as_str(), call))
        })
        .collect();
    Some(OpCtx {
        key: key.to_string(),
        field: field.to_string(),
        verb,
        args,
        wire,
        places,
        body_flat,
        calls,
        body_args_unmatched: Vec::new(),
        body_members_unclassified: Vec::new(),
        exec: std::collections::BTreeMap::new(),
        ambiguous: std::collections::BTreeMap::new(),
        required_omitted: Vec::new(),
    })
}

// ── Where each argument goes on the wire ────────────────────────────────────

/// One place a connector puts an argument (or one member of an input
/// argument) in the outbound request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Location {
    /// A JSON pointer into the request body.
    Body(Vec<String>),
    /// The elements of a list argument, each at the same place under one
    /// array: a JSON pointer whose array-index segments are `*`. Only an
    /// executed placement produces it.
    BodyElements(Vec<String>),
    /// A query-parameter key.
    Query(String),
    /// The URL path template the argument is a `{$args.…}` placeholder of.
    Path(String),
    /// A header name and the value template the argument is a placeholder of.
    Header(String, String),
}

/// The connector's request, read structurally: every argument path
/// (`["title"]`, `["input", "name"]`) it places and where, the body
/// positions it fills with a literal, and the body positions whose
/// expression this reader does not classify (a method chain, `??`, a
/// `$this` read...). An argument found nowhere here has an unproven
/// mapping: this instrument never falls back to "the value appears
/// somewhere".
#[derive(Debug, Clone, Default)]
struct WireMap {
    places: Vec<(Vec<String>, Location)>,
    /// A body position the connector fills with a fixed value: its pointer
    /// and the literal's own text.
    literals: Vec<(Vec<String>, String)>,
    unclassified: Vec<Vec<String>>,
    /// The connector's own URL path template (`{$args.…}` placeholders
    /// included, exactly as declared), used to check that a candidate
    /// mapping actually targets THIS operation's endpoint before its body
    /// assertions are trusted as proof (B4) -- `serves()` alone only checks
    /// case ownership by name/tag, not endpoint.
    path_template: Option<String>,
}

/// `$args.a.b` as `["a", "b"]`; `None` for anything but a plain path.
fn args_path(expr: &str) -> Option<Vec<String>> {
    let rest = expr.trim().strip_prefix("$args.")?;
    // `$args.x?` is the same argument: the `?` only drops the key when the
    // value is null, so the pointer and the proof are unchanged.
    let rest = rest.strip_suffix('?').unwrap_or(rest);
    let segs: Vec<String> = rest.split('.').map(str::to_string).collect();
    let ident = |s: &str| {
        s.chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    segs.iter().all(|s| ident(s)).then_some(segs)
}

fn pushed(pointer: &[String], key: &str) -> Vec<String> {
    let mut p = pointer.to_vec();
    p.push(key.to_string());
    p
}

fn walk_body(expr: &str, pointer: Vec<String>, out: &mut WireMap) {
    let e = expr.trim();
    if e.is_empty() {
        return;
    }
    if crate::obligations::is_literal(e) {
        out.literals.push((pointer, e.to_string()));
        return;
    }
    if let Some(group) = crate::obligations::strip_dollar_group(e) {
        if crate::obligations::is_literal(group) {
            out.literals.push((pointer, group.trim().to_string()));
            return;
        }
        if group.starts_with('{') {
            walk_object(group, pointer, out);
            return;
        }
    }
    if e.starts_with('{') && e.ends_with('}') {
        walk_object(e, pointer, out);
        return;
    }
    // `$args.x { Wire: field … }`: an argument sub-selection.
    if e.starts_with("$args.") && e.ends_with('}') {
        if let Some(open) = e.find('{') {
            if let Some(arg) = args_path(&e[..open]) {
                let nodes = crate::reconcile::parse_selection(&e[open + 1..e.len() - 1]);
                walk_sub_selection(&nodes, &arg, &pointer, out);
                return;
            }
        }
    }
    match args_path(e) {
        Some(arg) => out.places.push((arg, Location::Body(pointer))),
        None => out.unclassified.push(pointer),
    }
}

fn walk_object(e: &str, pointer: Vec<String>, out: &mut WireMap) {
    for (key, value) in crate::obligations::parse_body_pairs(e) {
        walk_body(&value, pushed(&pointer, &key), out);
    }
}

fn walk_sub_selection(
    nodes: &[crate::reconcile::Node],
    arg: &[String],
    pointer: &[String],
    out: &mut WireMap,
) {
    for n in nodes {
        let Some(key) = n.key.as_ref().filter(|k| !k.is_empty()) else {
            out.unclassified
                .push(pushed(pointer, n.alias.as_deref().unwrap_or("?")));
            continue;
        };
        let wire_name = n
            .alias
            .clone()
            .unwrap_or_else(|| key[key.len() - 1].clone());
        let at = pushed(pointer, &wire_name);
        if n.rooted
            || n.opaque
            || !n.methods.is_empty()
            || !n.fallbacks.is_empty()
            || n.spread.is_some()
        {
            out.unclassified.push(at);
            continue;
        }
        let mut path = arg.to_vec();
        path.extend(key.iter().cloned());
        match &n.children {
            Some(children) => walk_sub_selection(children, &path, &at, out),
            None => out.places.push((path, Location::Body(at))),
        }
    }
}

/// Every `{$args.…}` placeholder of `template` that is a plain argument path.
fn template_args(template: &str) -> Vec<Vec<String>> {
    let re = Regex::new(r"\{([^{}]*)\}").unwrap();
    re.captures_iter(template)
        .filter_map(|c| args_path(&c[1]))
        .collect()
}

fn wire_map(field_text: &str, wire: &Wiring) -> WireMap {
    let mut out = WireMap::default();
    if let Some(body) = crate::lint::connector_block(field_text, "body") {
        let b = body.trim();
        if b.starts_with("$args.") {
            walk_body(b, Vec::new(), &mut out);
        } else {
            walk_object(b, Vec::new(), &mut out);
        }
    }
    for (path, key) in &wire.plain_query_keys {
        out.places
            .push((path.clone(), Location::Query(key.clone())));
    }
    let url = Regex::new(r#"\b(?:GET|POST|PUT|PATCH|DELETE)\s*:\s*"([^"]*)""#).unwrap();
    if let Some(c) = url.captures(field_text) {
        let path_part = c[1].split('?').next().unwrap_or("").to_string();
        out.path_template = Some(path_part.clone());
        for arg in template_args(&path_part) {
            out.places.push((arg, Location::Path(path_part.clone())));
        }
    }
    // A header entry's `name`/`value` field order does not change its
    // meaning; read each `{...}` entry structurally (the same
    // `object_field_texts` reader an object literal's members use
    // elsewhere), rather than a fixed-order regex that made the argument
    // invisible whenever a connector happened to write `value` first (B6).
    let blanked = graphql::blank(field_text);
    if let Some(m) = Regex::new(r"headers\s*:\s*\[").unwrap().find(&blanked) {
        let open = m.end() - 1;
        if let Some(close) = matching_close(blanked.as_bytes(), open) {
            if let Some(list_text) = field_text.get(open..=close) {
                for entry in list_elements_text(list_text) {
                    let fields = object_field_texts(&entry);
                    let name = fields
                        .iter()
                        .find(|(k, _)| k == "name")
                        .map(|(_, v)| wire_text(v));
                    let value = fields
                        .iter()
                        .find(|(k, _)| k == "value")
                        .map(|(_, v)| wire_text(v));
                    if let (Some(name), Some(value)) = (name, value) {
                        for arg in template_args(&value) {
                            out.places
                                .push((arg, Location::Header(name.clone(), value.clone())));
                        }
                    }
                }
            }
        }
    }
    out
}

/// Where `path` goes: its own places, or, under a forwarded parent, the
/// parent's body pointer extended by the rest of the path.
fn locations_of(map: &WireMap, path: &[String]) -> Vec<Location> {
    let own: Vec<Location> = map
        .places
        .iter()
        .filter(|(p, _)| p.as_slice() == path)
        .map(|(_, l)| l.clone())
        .collect();
    if !own.is_empty() {
        return own;
    }
    for cut in (1..path.len()).rev() {
        let derived: Vec<Location> = map
            .places
            .iter()
            .filter(|(p, _)| p.as_slice() == &path[..cut])
            .filter_map(|(_, l)| match l {
                Location::Body(ptr) => {
                    let mut ptr = ptr.clone();
                    ptr.extend(path[cut..].iter().cloned());
                    Some(Location::Body(ptr))
                }
                _ => None,
            })
            .collect();
        if !derived.is_empty() {
            return derived;
        }
    }
    Vec::new()
}

/// An argument, or a member of one, whose members the connector places one
/// by one (a sub-selection, `$args.x.y` entries) but never whole.
fn is_container(map: &WireMap, path: &[String]) -> bool {
    map.places
        .iter()
        .any(|(p, _)| p.len() > path.len() && p.starts_with(path))
}

/// The value text a case passes for `path`: the argument, or one member of
/// its object literal.
fn case_value(passed: &[(String, String)], path: &[String]) -> Option<String> {
    let mut value = passed
        .iter()
        .find(|(n, _)| n == &path[0])
        .map(|(_, v)| v.clone())?;
    for seg in &path[1..] {
        value = object_field_texts(&value)
            .into_iter()
            .find(|(n, _)| n == seg)
            .map(|(_, v)| v)?;
    }
    Some(value)
}

fn at_pointer<'a>(body: &'a Value, pointer: &[String]) -> Option<&'a Value> {
    pointer.iter().try_fold(body, |v, k| v.get(k))
}

/// The scalar text a query parameter, path segment or header carries for a
/// GraphQL value literal: a string without its quotes, anything else as
/// written.
fn wire_text(value: &str) -> String {
    let v = value.trim();
    match serde_json::from_str::<Value>(v) {
        Ok(Value::String(s)) => s,
        _ => v.to_string(),
    }
}

/// `template` with `path`'s placeholder demanding `value`, every other
/// placeholder any text, matched against `actual` whole. `prefix` allows a
/// base-URL path ahead of the template.
fn template_demands(
    template: &str,
    path: &[String],
    value: &str,
    actual: &str,
    prefix: bool,
) -> bool {
    let re = Regex::new(r"\{([^{}]*)\}").unwrap();
    let mut pattern = String::from(if prefix { "^.*?" } else { "^" });
    let mut last = 0;
    for c in re.captures_iter(template) {
        let m = c.get(0).unwrap();
        pattern.push_str(&regex::escape(&template[last..m.start()]));
        if args_path(&c[1]).as_deref() == Some(path) {
            pattern.push_str(&regex::escape(value));
        } else {
            pattern.push_str(if prefix { "[^/]+" } else { ".*?" });
        }
        last = m.end();
    }
    pattern.push_str(&regex::escape(&template[last..]));
    pattern.push('$');
    // `actual` is the request's real, percent-encoded path; `value` is the
    // plain text a case passed. Comparing them as raw bytes never agrees
    // for a segment with a space or other reserved character correctly
    // encoded on the wire (B6): decode `actual` first.
    let decoded = percent_decode(actual);
    Regex::new(&pattern)
        .map(|r| r.is_match(&decoded))
        .unwrap_or(false)
}

/// A minimal `%XX` percent-decoder: everything else, including a `%` not
/// followed by two hex digits, passes through unchanged.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // The two hex digits are read as bytes, never as a `&str` slice:
        // a `%` followed by a multibyte character would put the slice's end
        // inside it, and slicing a `str` there panics.
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Does `mapping` demand `value` for `path` at `loc`, exactly there?
fn demands_at(mapping: &Value, loc: &Location, path: &[String], value: &str) -> bool {
    let Some(req) = get(mapping, "request") else {
        return false;
    };
    match loc {
        Location::Body(ptr) => {
            let Some(body) = demanded_body(mapping) else {
                return false;
            };
            let Some(have) = at_pointer(&body, ptr) else {
                return false;
            };
            match graphql_member_to_json(value) {
                Some(want) => have == &want,
                None => false,
            }
        }
        Location::BodyElements(shape) => {
            let Some(body) = demanded_body(mapping) else {
                return false;
            };
            let Some(have) = elements_at(&body, shape) else {
                return false;
            };
            let mut have: Vec<Value> = have.into_iter().cloned().collect();
            let Some(mut want) = list_elements_text(value)
                .iter()
                .map(|e| graphql_member_to_json(e))
                .collect::<Option<Vec<Value>>>()
            else {
                return false;
            };
            have.sort_by_key(|v| v.to_string());
            want.sort_by_key(|v| v.to_string());
            have == want
        }
        Location::Query(key) => {
            let Some(q) = get_obj(req, "queryParameters") else {
                return false;
            };
            let is_list = value.trim_start().starts_with('[');
            // A `key[]` matcher is WireMock's own convention for a repeated
            // query parameter; it is never a substitute for a plain scalar
            // key, only for an actual list value (B5) -- accepting it
            // either way let a mismatched encoding (`wire` vs `wire[]`)
            // falsely pass.
            let matcher = if is_list {
                q.get(&format!("{}[]", key))
                    .or_else(|| q.get(key))
                    .and_then(Value::as_object)
            } else {
                q.get(key).and_then(Value::as_object)
            };
            let Some(matcher) = matcher else {
                return false;
            };
            if is_list {
                exact_list_match(matcher, &list_elements_text(value))
            } else {
                matcher.get("equalTo").and_then(Value::as_str) == Some(wire_text(value).as_str())
            }
        }
        Location::Path(template) => {
            let actual = get_str(req, "urlPath")
                .or_else(|| get_str(req, "url").map(|u| u.split('?').next().unwrap_or("")));
            actual
                .map(|a| template_demands(template, path, &wire_text(value), a, true))
                .unwrap_or(false)
        }
        Location::Header(name, template) => {
            let Some(headers) = get_obj(req, "headers") else {
                return false;
            };
            headers
                .iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .filter_map(|(_, m)| crate::json::field(m, "equalTo").and_then(Value::as_str))
                .any(|actual| template_demands(template, path, &wire_text(value), actual, false))
        }
    }
}

fn location_label(loc: &Location) -> String {
    match loc {
        Location::Body(p) => format!("body /{}", p.join("/")),
        Location::BodyElements(p) => format!("body elements /{}", p.join("/")),
        Location::Query(k) => format!("query {}", k),
        Location::Path(t) => format!("path {}", t),
        Location::Header(n, _) => format!("header {}", n),
    }
}

/// A structural check -- ignoring every placeholder's actual value -- that
/// `template` and `actual` have the same static shape: every `{...}`
/// segment matches anything, everything else must agree exactly. Reused
/// here (empty `path`, which no real argument path ever equals) to ask
/// "does this URL have the same shape as this operation's own template",
/// with no argument singled out for its real value.
fn path_template_matches(template: &str, actual: &str) -> bool {
    template_demands(template, &[], "", actual, true)
}

/// How a stub's request matcher relates to THIS operation's endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    /// A different method or a different static URL shape: a request to this
    /// operation never reaches it.
    No,
    /// The same method and (when the connector's own path template is known)
    /// the same static URL shape, written as `urlPath` or `url`.
    Exact,
    /// It may answer a request to this operation, but the files cannot say:
    /// a `urlPathPattern`/`urlPattern`/`urlPathTemplate` matcher, no URL
    /// matcher at all, or no or an `ANY` method.
    Possible,
}

/// Whether `mapping` could be the stub a real request to THIS operation's
/// own endpoint hits. `serves()` only checks case ownership by file name or
/// an `x-cases`/`x-shared` tag -- it says nothing about where the stub
/// actually sits, so an owned-but-unrelated stub (a decoy at a different
/// endpoint, verified never hit by any real request) could otherwise credit
/// proof no request to the real endpoint ever demonstrated (B4). A pattern
/// matcher is never read as a regex here: it is `Possible`, which never
/// credits a proof and disqualifies the case's other stubs (see
/// [`proof_stubs`]).
fn endpoint_match(ctx: &OpCtx, mapping: &Value) -> Endpoint {
    let Some(req) = get(mapping, "request") else {
        return Endpoint::No;
    };
    let method = match get_str(req, "method") {
        None => Endpoint::Possible,
        Some(m) if m.eq_ignore_ascii_case("ANY") => Endpoint::Possible,
        Some(m) if m.eq_ignore_ascii_case(&ctx.verb) => Endpoint::Exact,
        Some(_) => return Endpoint::No,
    };
    let literal = get_str(req, "urlPath")
        .or_else(|| get_str(req, "url").map(|u| u.split('?').next().unwrap_or("")));
    let url = match literal {
        Some(actual) => match &ctx.places.path_template {
            Some(template) if !path_template_matches(template, actual) => return Endpoint::No,
            // The same static shape, or no path template recorded (a
            // query-only or bodiless route): the method is all there is to
            // check.
            _ => Endpoint::Exact,
        },
        // `urlPathPattern`, `urlPattern`, `urlPathTemplate`, or no URL
        // matcher at all (which matches every URL).
        None => Endpoint::Possible,
    };
    if method == Endpoint::Exact && url == Endpoint::Exact {
        Endpoint::Exact
    } else {
        Endpoint::Possible
    }
}

/// The stubs whose demands can prove something about `case`'s request to
/// this operation: every stub that serves the case at exactly this
/// endpoint -- or none at all when any stub serving the case might answer
/// that request without demanding what the proof reads. A passing case
/// shows only that *some* of its stubs matched. A fallback stub (no body
/// pattern, a loose `ignoreExtraElements` one, or a body that is not a
/// JSON `equalToJson`, on a body-sending operation) or a pattern-matched
/// stub (`urlPathPattern`, `urlPattern`, no URL, `ANY` method) could have
/// answered any request, so an exact sibling's demand proves nothing.
fn proof_stubs<'m>(
    ctx: &OpCtx,
    case: &str,
    mappings: &'m [(String, Value)],
) -> Vec<&'m (String, Value)> {
    let mut out = Vec::new();
    for entry in mappings {
        let (mname, mapping) = entry;
        if !serves(mname, mapping, case) {
            continue;
        }
        match endpoint_match(ctx, mapping) {
            Endpoint::No => continue,
            Endpoint::Possible => return Vec::new(),
            Endpoint::Exact => {
                if ctx.wire.sends_body && demanded_body(mapping).is_none() {
                    return Vec::new();
                }
                out.push(entry);
            }
        }
    }
    out
}

/// The executed case, if any, that supplies `path` and whose stub demands
/// its value at `loc`.
fn proven_at(
    ctx: &OpCtx,
    path: &[String],
    loc: &Location,
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    workspace: &Path,
) -> Option<CaseExec> {
    let mut found: Option<CaseExec> = None;
    for (case, passed) in &ctx.calls {
        let Some(value) = case_value(passed, path) else {
            continue;
        };
        if is_null_literal(&value) {
            continue;
        }
        for (_, mapping) in proof_stubs(ctx, case, mappings) {
            if demands_at(mapping, loc, path, &value) {
                let exec = case_executed(workspace, evidence, "wiremock_e2e", case);
                if exec == CaseExec::Proven {
                    return Some(exec);
                }
                found.get_or_insert(exec);
            }
        }
    }
    found
}

// ── Executed placement (ADR 0079): where a case's own stub says it went ────
//
// The reader above places what it can parse. What it cannot (a method chain,
// `??`, a value the connector reshapes) it does not guess at and does not
// try to learn the spelling of: an executed, passing case already shows
// where the router put the value, because the case's stub demands a request
// body and the case passed. So the argument's own value, taken from the
// case's call, is looked up in that stub's demanded body. Exactly one place
// is a placement; none stays unproven; more than one, or a value too plain to
// tell from its neighbours, is `ambiguous_value`, and the author fixes it in
// the case by choosing a distinctive value. The expression text is never read
// on this path.

/// Every pointer at which `body` holds the scalar `target`, skipping the
/// pointers in `except` (a place the connector already fills with another
/// argument or a fixed value).
fn leaf_pointers(
    body: &Value,
    target: &Value,
    except: &[Vec<String>],
    at: &mut Vec<String>,
    out: &mut Vec<Vec<String>>,
) {
    match body {
        Value::Object(o) => {
            for (k, v) in o {
                at.push(k.clone());
                leaf_pointers(v, target, except, at, out);
                at.pop();
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                at.push(i.to_string());
                leaf_pointers(v, target, except, at, out);
                at.pop();
            }
        }
        leaf => {
            if leaf == target && !except.contains(at) {
                out.push(at.clone());
            }
        }
    }
}

/// The values `shape` (a pointer whose array-index segments are `*`) reaches
/// in `body`, every array element in turn; `None` when a segment is missing.
fn elements_at<'a>(body: &'a Value, shape: &[String]) -> Option<Vec<&'a Value>> {
    let Some((head, rest)) = shape.split_first() else {
        return Some(vec![body]);
    };
    if head == "*" {
        let mut out = Vec::new();
        for v in body.as_array()? {
            out.extend(elements_at(v, rest)?);
        }
        return Some(out);
    }
    elements_at(body.get(head)?, rest)
}

/// A value the stub's body can be searched for without mistaking it for
/// something else: a string of at least three characters, or a number of at
/// least four digits (or a fraction). A boolean, a null, a short number or a
/// nearly empty string recurs by chance in any body.
fn distinctive(v: &Value) -> bool {
    match v {
        Value::String(s) => s.trim().chars().count() >= 3,
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.unsigned_abs() >= 1000,
            None => true,
        },
        _ => false,
    }
}

/// What one passing case says about one argument's place.
enum Seen {
    /// The case's stub demands the value at exactly this place (a list: the
    /// elements' common shape).
    Placed(Vec<String>, bool),
    /// The value cannot be told from another at one place.
    Ambiguous(String),
    /// The value is nowhere in the stub's body (or the case gives no exact
    /// stub): the case says nothing.
    Silent,
}

/// The one stub that answers `case`'s request to this operation and demands
/// an exact body, or `None` when there is none or more than one (which of
/// them the router hit cannot be told from the files).
fn own_stub_body(ctx: &OpCtx, case: &str, mappings: &[(String, Value)]) -> Option<Value> {
    let mut bodies = proof_stubs(ctx, case, mappings)
        .into_iter()
        .filter_map(|(_, m)| demanded_body(m));
    let first = bodies.next()?;
    bodies.next().is_none().then_some(first)
}

fn observe(
    ctx: &OpCtx,
    passed: &[(String, String)],
    name: &str,
    body: &Value,
    except: &[Vec<String>],
) -> Seen {
    let Some(value) = passed.iter().find(|(n, _)| n == name).map(|(_, v)| v) else {
        return Seen::Silent;
    };
    let Some(json) = graphql_member_to_json(value) else {
        return Seen::Silent;
    };
    if json.is_null() {
        return Seen::Silent;
    }
    let others: Vec<(&str, Value)> = passed
        .iter()
        .filter(|(n, _)| n != name)
        .filter_map(|(n, v)| Some((n.as_str(), graphql_member_to_json(v)?)))
        .collect();
    let shared = |v: &Value| others.iter().find(|(_, o)| o == v).map(|(n, _)| *n);
    let scalars: Vec<Value> = match &json {
        Value::Array(items) => items.clone(),
        v => vec![v.clone()],
    };
    let is_list = json.is_array();
    if is_list {
        let distinct: std::collections::HashSet<String> =
            scalars.iter().map(|v| v.to_string()).collect();
        if scalars.len() < 2 || distinct.len() != scalars.len() {
            return Seen::Silent;
        }
    }
    let mut found: Vec<Vec<String>> = Vec::new();
    for v in &scalars {
        if !distinctive(v) {
            return Seen::Ambiguous(format!(
                "passes {} in case, a value too common to find in a request body; give it a distinctive value in the case",
                v
            ));
        }
        if let Some(other) = shared(v) {
            return Seen::Ambiguous(format!(
                "passes {} in case, the same value as argument {}; give it a distinctive value in the case",
                v, other
            ));
        }
        let mut at = Vec::new();
        let mut hits = Vec::new();
        leaf_pointers(body, v, except, &mut at, &mut hits);
        match hits.len() {
            0 => return Seen::Silent,
            1 => found.push(hits.remove(0)),
            n => {
                return Seen::Ambiguous(format!(
                    "passes {} in case, and the stub's body holds that value at {} places ({}); give it a distinctive value in the case",
                    v,
                    n,
                    hits.iter()
                        .map(|p| format!("/{}", p.join("/")))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        }
    }
    let _ = ctx;
    if !is_list {
        return Seen::Placed(found.remove(0), false);
    }
    let len = found[0].len();
    if found.iter().any(|p| p.len() != len) {
        return Seen::Silent;
    }
    let mut shape = Vec::with_capacity(len);
    for i in 0..len {
        let col: Vec<&String> = found.iter().map(|p| &p[i]).collect();
        if col.iter().all(|c| *c == col[0]) {
            shape.push(col[0].clone());
        } else if col.iter().all(|c| c.parse::<usize>().is_ok()) {
            shape.push("*".to_string());
        } else {
            return Seen::Silent;
        }
    }
    if !shape.iter().any(|s| s == "*") {
        return Seen::Silent;
    }
    Seen::Placed(shape, true)
}

/// Place, by execution, each top-level argument the reader could not place.
fn executed_placements(
    ctx: &mut OpCtx,
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    workspace: &Path,
) {
    if !ctx.wire.sends_body {
        return;
    }
    let taken: Vec<Vec<String>> = ctx
        .places
        .places
        .iter()
        .filter_map(|(_, l)| match l {
            Location::Body(p) => Some(p.clone()),
            _ => None,
        })
        .chain(ctx.places.literals.iter().map(|(p, _)| p.clone()))
        .collect();
    let names: Vec<String> = ctx
        .args
        .iter()
        .map(|a| a.name.clone())
        .filter(|n| {
            locations_of(&ctx.places, std::slice::from_ref(n)).is_empty()
                && !is_container(&ctx.places, std::slice::from_ref(n))
        })
        .collect();
    for name in names {
        let mut placed: Vec<(Vec<String>, bool, String)> = Vec::new();
        let mut ambiguous: Option<String> = None;
        for (case, passed) in &ctx.calls {
            if case_executed(workspace, evidence, "wiremock_e2e", case) != CaseExec::Proven {
                continue;
            }
            let Some(body) = own_stub_body(ctx, case, mappings) else {
                continue;
            };
            match observe(ctx, passed, &name, &body, &taken) {
                Seen::Placed(p, list) => placed.push((p, list, case.to_string())),
                Seen::Ambiguous(why) => {
                    ambiguous.get_or_insert(why);
                }
                Seen::Silent => {}
            }
        }
        if let Some((shape, list, case)) = placed.first().cloned() {
            if let Some((other, _, other_case)) = placed.iter().find(|(p, _, _)| p != &shape) {
                ctx.ambiguous.insert(
                    name.clone(),
                    format!(
                        "is found at /{} by case {} and at /{} by case {}; the cases disagree about where it goes",
                        shape.join("/"),
                        case,
                        other.join("/"),
                        other_case
                    ),
                );
                continue;
            }
            let loc = if list {
                Location::BodyElements(shape)
            } else {
                Location::Body(shape)
            };
            ctx.places.places.push((vec![name.clone()], loc));
            ctx.exec.insert(name, case);
        } else if let Some(why) = ambiguous {
            ctx.ambiguous.insert(name, why);
        }
    }
}

/// Where each write argument goes, and whether the reader or an executed
/// case established it.
fn placements_of(ctxs: &[OpCtx]) -> Vec<Placement> {
    let mut out = Vec::new();
    for ctx in ctxs.iter().filter(|c| is_write(c)) {
        for arg in &ctx.args {
            for loc in locations_of(&ctx.places, std::slice::from_ref(&arg.name)) {
                let case = ctx.exec.get(&arg.name).cloned();
                out.push(Placement {
                    operation: ctx.key.clone(),
                    argument: arg.name.clone(),
                    via: if case.is_some() { "executed" } else { "static" }.to_string(),
                    location: location_label(&loc),
                    case,
                });
            }
        }
    }
    out
}

// ── Obligation 1: every argument has an exact outbound-request assertion ───

/// A connector literal (`"fixed"`, `'fixed'`, `7`, `true`) as the JSON text
/// a stub's `equalToJson` would hold.
fn literal_wire_text(literal: &str) -> String {
    let t = literal.trim();
    match t.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        Some(inner) => Value::String(inner.to_string()).to_string(),
        None => t.to_string(),
    }
}

/// Whether an executed, passing case sits behind a stub that demands
/// `literal` at body `pointer` of this operation's request.
fn literal_proven_at(
    ctx: &OpCtx,
    pointer: &[String],
    literal: &str,
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    workspace: &Path,
) -> Option<CaseExec> {
    let loc = Location::Body(pointer.to_vec());
    let want = literal_wire_text(literal);
    let mut found: Option<CaseExec> = None;
    for (case, _) in &ctx.calls {
        for (_, mapping) in proof_stubs(ctx, case, mappings) {
            if demands_at(mapping, &loc, &[], &want) {
                let exec = case_executed(workspace, evidence, "wiremock_e2e", case);
                if exec == CaseExec::Proven {
                    return Some(exec);
                }
                found.get_or_insert(exec);
            }
        }
    }
    found
}

struct Tally {
    covered: usize,
    total: usize,
    gaps: Vec<Gap>,
}
impl Tally {
    fn new() -> Self {
        Tally {
            covered: 0,
            total: 0,
            gaps: Vec::new(),
        }
    }
    fn require(&mut self, ok: bool, op: &str, kind: GapKind, gap: impl FnOnce() -> String) {
        self.total += 1;
        if ok {
            self.covered += 1;
        } else {
            self.gaps.push(Gap {
                operation: op.to_string(),
                kind,
                message: gap(),
            });
        }
    }
}

/// The mapping's demanded request body, from an exact `equalToJson`
/// `bodyPattern` (never a loose `ignoreExtraElements`/`matchesJsonPath`/
/// `contains` pattern -- those assert a fragment, not the body, exactly the
/// distinction the retired `loose-write-body` lint drew).
fn demanded_body(mapping: &Value) -> Option<Value> {
    let req = get(mapping, "request")?;
    let patterns = get_arr(req, "bodyPatterns")?;
    patterns.iter().find_map(|p| {
        if crate::json::field(p, "ignoreExtraElements") == Some(&Value::Bool(true)) {
            return None;
        }
        // WireMock takes the demanded body as a JSON value or as a string
        // holding one; the pilots write the string form.
        match crate::json::field(p, "equalToJson")? {
            Value::String(text) => crate::json::parse(text).ok(),
            v => Some(v.clone()),
        }
    })
}

/// The *exact* demanded body regardless of looseness, used only for
/// omission/null proof where looseness itself would need its own separate
/// finding (kept simple: those proofs already require exactness, so a loose
/// pattern simply never satisfies them).
fn exact_demanded_body(mapping: &Value) -> Option<Value> {
    demanded_body(mapping)
}

/// Every path under `prefix`'s `nested` members, at any depth (B1): each one
/// pushed as `(path, true)` -- `true` marks it a nested member, matching the
/// existing dedup rule that skips a member of an argument with no place of
/// its own at all.
fn nested_paths(prefix: &[String], nested: &[ArgInfo], out: &mut Vec<(Vec<String>, bool)>) {
    for m in nested {
        let mut path = prefix.to_vec();
        path.push(m.name.clone());
        out.push((path.clone(), true));
        nested_paths(&path, &m.nested, out);
    }
}

fn write_body_assertions(
    ctxs: &[OpCtx],
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    workspace: &Path,
) -> Obligation {
    let id = "serialization.write-body-assertions";
    let mut t = Tally::new();
    let mut any_arg = false;
    for ctx in ctxs {
        for arg in &ctx.args {
            any_arg = true;
            let mut paths: Vec<(Vec<String>, bool)> = vec![(vec![arg.name.clone()], false)];
            // A list of input objects is proven whole: the exact match of
            // the array at its pointer covers every element's members. A
            // non-list argument's nested `input` members are walked at
            // every depth (B1): a member two or more levels down is just as
            // real an obligation as a top-level one.
            if !arg.is_list() {
                nested_paths(&[arg.name.clone()], &arg.nested, &mut paths);
            }
            for (path, nested) in paths {
                let label = path.join(".");
                let locs = locations_of(&ctx.places, &path);
                if locs.is_empty() {
                    // Placed member by member (a sub-selection): each
                    // member carries the proof, the argument itself none.
                    if is_container(&ctx.places, &path) {
                        continue;
                    }
                    // A member of an argument that has no place either is
                    // already counted once, on the argument.
                    if nested
                        && locations_of(&ctx.places, &path[..1]).is_empty()
                        && !is_container(&ctx.places, &path[..1])
                    {
                        continue;
                    }
                    if let Some(why) = ctx.ambiguous.get(&path.join(".")) {
                        t.require(false, &ctx.key, GapKind::AmbiguousValue, || {
                            format!("{}: {}({}) {}", ctx.key, ctx.field, label, why)
                        });
                        continue;
                    }
                    t.require(false, &ctx.key, GapKind::UnprovenMapping, || {
                        format!(
                            "{}: {}({}) is not placed anywhere this instrument classifies (a body pointer, a query key, a path or header placeholder)",
                            ctx.key, ctx.field, label
                        )
                    });
                    continue;
                }
                for loc in &locs {
                    let exec = proven_at(ctx, &path, loc, mappings, evidence, workspace);
                    let ok = matches!(exec, Some(CaseExec::Proven));
                    t.require(ok, &ctx.key, GapKind::ArgumentUnasserted, || {
                        format!(
                            "{}: {}({}) has no executed case whose stub demands its value at {} ({})",
                            ctx.key,
                            ctx.field,
                            label,
                            location_label(loc),
                            match &exec {
                                Some(CaseExec::Failed) =>
                                    "the proving case is recorded as failed".to_string(),
                                Some(CaseExec::NotRecorded(r)) => r.clone(),
                                Some(CaseExec::Proven) => unreachable!(),
                                None => "no case supplies it behind a stub that demands it there"
                                    .to_string(),
                            }
                        )
                    });
                }
            }
        }
        // A fixed value the connector writes into the body is as much an
        // outbound value as an argument: nothing else checks it, so an
        // executed case must demand it at its own pointer.
        for (pointer, literal) in &ctx.places.literals {
            any_arg = true;
            let exec = literal_proven_at(ctx, pointer, literal, mappings, evidence, workspace);
            let ok = matches!(exec, Some(CaseExec::Proven));
            t.require(ok, &ctx.key, GapKind::FixedValueUnasserted, || {
                format!(
                    "{}: {} writes the fixed value {} at body /{} and no executed case has a stub that demands it there ({})",
                    ctx.key,
                    ctx.field,
                    literal.trim(),
                    pointer.join("/"),
                    match &exec {
                        Some(CaseExec::Failed) => "the proving case is recorded as failed".to_string(),
                        Some(CaseExec::NotRecorded(r)) => r.clone(),
                        Some(CaseExec::Proven) => unreachable!(),
                        None => "no case calls it behind a stub that demands it there".to_string(),
                    }
                )
            });
        }
        // Source-documented request-body members with no corresponding
        // argument at all: "missing or unsupported inputs must produce
        // explicit unmet obligations" (task requirement), not silence.
        for member in &ctx.body_members_unclassified {
            t.require(false, &ctx.key, GapKind::UnprovenMapping, || {
                format!(
                    "{}: the source request body documents member \"{}\" at a body position this instrument does not classify",
                    ctx.key, member
                )
            });
        }
        for (member, decision) in &ctx.required_omitted {
            t.require(false, &ctx.key, GapKind::RequiredMemberOmitted, || {
                format!(
                    "{}: the source marks request body member \"{}\" required and no argument feeds it; decision {} omits it, but an omit cannot excuse a required member -- send it, or record why the source is wrong (a patch to the source description)",
                    ctx.key, member, decision
                )
            });
        }
        for missing in &ctx.body_args_unmatched {
            t.require(false, &ctx.key, GapKind::SourceMemberUnmapped, || {
                format!(
                    "{}: the source request body documents member \"{}\" with no corresponding connector argument",
                    ctx.key, missing
                )
            });
        }
    }
    if !any_arg {
        return not_applicable(
            id,
            "no selected operation declares any argument or writes a fixed body value",
        );
    }
    finish(id, t)
}

// ── Obligation 2: list arguments (>=2 DISTINCT values, exact encoding) ──────

fn list_argument_proof(
    ctxs: &[OpCtx],
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    workspace: &Path,
) -> Obligation {
    let id = "serialization.list-argument-proof";
    let mut t = Tally::new();
    let mut any_list = false;
    for ctx in ctxs {
        for arg in ctx.args.iter().filter(|a| a.is_list()) {
            any_list = true;
            // Already one `ambiguous_value` gap, on the argument.
            if ctx.ambiguous.contains_key(&arg.name) {
                continue;
            }
            let key = ctx
                .wire
                .query_keys
                .iter()
                .find(|(a, _)| a == &arg.name)
                .map(|(_, k)| k.clone());
            let in_body = ctx.wire.body_args.contains(&arg.name);
            let mut proven_exec: Option<CaseExec> = None;
            let mut best_reason = "no case passes this argument with two or more distinct elements behind a stub asserting them exactly".to_string();
            for (case, passed) in &ctx.calls {
                let Some(value) = passed
                    .iter()
                    .find(|(n, _)| n == &arg.name)
                    .map(|(_, v)| v.clone())
                else {
                    continue;
                };
                let elements = list_elements_text(&value);
                let distinct: std::collections::HashSet<&str> =
                    elements.iter().map(|s| s.as_str()).collect();
                if distinct.len() < 2 {
                    best_reason = format!(
                        "{}: {} distinct element(s) passed for {}, at least 2 required",
                        ctx.key,
                        distinct.len(),
                        arg.name
                    );
                    continue;
                }
                for (_, mapping) in proof_stubs(ctx, case, mappings) {
                    let asserted = if in_body {
                        locations_of(&ctx.places, std::slice::from_ref(&arg.name))
                            .iter()
                            .filter(|l| matches!(l, Location::Body(_) | Location::BodyElements(_)))
                            .any(|l| {
                                demands_at(mapping, l, std::slice::from_ref(&arg.name), &value)
                            })
                    } else {
                        let q = get(mapping, "request").and_then(|r| get_obj(r, "queryParameters"));
                        q.map(|q| {
                            q.iter().any(|(k, v)| {
                                let base = k.trim_end_matches("[]");
                                // No query key of its own: never "any
                                // key whose list happens to match".
                                let key_ok = key
                                    .as_deref()
                                    .map(|kk| kk == k || kk == base)
                                    .unwrap_or(false);
                                key_ok
                                    && v.as_object()
                                        .map(|o| exact_list_match(o, &elements))
                                        .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false)
                    };
                    if asserted {
                        let exec = case_executed(workspace, evidence, "wiremock_e2e", case);
                        if matches!(exec, CaseExec::Proven) {
                            proven_exec = Some(exec);
                        } else if proven_exec.is_none() {
                            best_reason = match &exec {
                                CaseExec::Failed => {
                                    "the proving case is recorded as failed".to_string()
                                }
                                CaseExec::NotRecorded(r) => r.clone(),
                                CaseExec::Proven => unreachable!(),
                            };
                        }
                    }
                }
            }
            t.require(
                matches!(proven_exec, Some(CaseExec::Proven)),
                &ctx.key,
                GapKind::ListUnproven,
                || {
                    format!(
                        "{}: {}({}: {}) -- {}",
                        ctx.key, ctx.field, arg.name, arg.type_text, best_reason
                    )
                },
            );
        }
    }
    if !any_list {
        return not_applicable(id, "no selected operation declares a list-typed argument");
    }
    finish(id, t)
}

/// `hasExactly`/`includes`/`equalTo` on the query-parameter matcher, checked
/// against the actual distinct element texts passed (order-insensitive),
/// so a stub demanding stale or wrong values -- not just *some* two values
/// -- fails this check (catches "incorrectly encoded").
fn exact_list_match(matcher: &Object, elements: &[String]) -> bool {
    // Compared as sorted multisets: `[a, a, b]` is not `[a, b, b]`.
    let mut want: Vec<String> = elements
        .iter()
        .map(|e| e.trim().trim_matches('"').to_string())
        .collect();
    want.sort();
    if let Some(Value::Array(have)) = matcher.get("hasExactly") {
        let mut have: Vec<String> = have
            .iter()
            .filter_map(|h| crate::json::field(h, "equalTo").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        have.sort();
        return have == want;
    }
    if let Some(Value::String(joined)) = matcher.get("equalTo") {
        // `equalTo` on a comma-joined key demands the joined string itself,
        // exactly: split it back into elements and require the same
        // multiset, not merely that each wanted value occurs as a
        // substring somewhere in it (`"redblue"` is not `["red","blue"]`,
        // and `"12"` is not `["1","2"]` -- B5).
        let mut have: Vec<&str> = joined.split(',').map(str::trim).collect();
        have.sort();
        return have == want.iter().map(String::as_str).collect::<Vec<_>>();
    }
    // `includes` is WireMock's own substring/contains assertion: it can
    // never prove an EXACT array match (nothing rules out extra values), so
    // it never counts as list-argument proof.
    false
}

// ── Obligation 3: mutation-cases (full/required-only, omitted/explicit-null,
//    map five-case) ──────────────────────────────────────────────────────────

fn is_write(ctx: &OpCtx) -> bool {
    ctx.wire.sends_body && matches!(ctx.verb.as_str(), "POST" | "PUT" | "PATCH")
}

fn mutation_cases(
    ctxs: &[OpCtx],
    schema: &SchemaCache,
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    decisions: &[crate::decisions::NullHandling],
    null_default: Option<&str>,
    workspace: &Path,
) -> Obligation {
    let id = "serialization.mutation-cases";
    let mut t = Tally::new();
    let mut any_write = false;
    for ctx in ctxs.iter().filter(|c| is_write(c)) {
        if ctx.args.is_empty() {
            continue;
        }
        any_write = true;
        let all: Vec<&str> = ctx.args.iter().map(|a| a.name.as_str()).collect();
        let required: Vec<&str> = ctx
            .args
            .iter()
            .filter(|a| a.is_required())
            .map(|a| a.name.as_str())
            .collect();
        let optional: Vec<&ArgInfo> = ctx.args.iter().filter(|a| !a.is_required()).collect();

        // Full-body case: some executed case passes every argument. Every
        // matching case is tried, not only the first: an unexecuted one
        // sorting ahead must not hide an executed one.
        let full_case = ctx.calls.iter().any(|call| {
            all.iter()
                .all(|n| call.1.iter().any(|(pn, v)| pn == n && !is_null_literal(v)))
                && exec_ok(Some(call), evidence, workspace)
        });
        t.require(full_case, &ctx.key, GapKind::FullCaseMissing, || {
            format!(
                "{}: no executed case calls {} with every argument ({})",
                ctx.key,
                ctx.field,
                all.join(", ")
            )
        });

        // Required-only case: some case passes exactly the required set
        // (when it differs from the full set at all).
        if required.len() != all.len() {
            let req_case = ctx.calls.iter().any(|call| {
                let passed = &call.1;
                required
                    .iter()
                    .all(|n| passed.iter().any(|(pn, v)| pn == n && !is_null_literal(v)))
                    && optional
                        .iter()
                        .all(|o| !passed.iter().any(|(pn, _)| pn == &o.name))
                    && exec_ok(Some(call), evidence, workspace)
            });
            t.require(req_case, &ctx.key, GapKind::RequiredOnlyCaseMissing, || {
                format!(
                    "{}: no executed case calls {} with only its required arguments ({})",
                    ctx.key,
                    ctx.field,
                    if required.is_empty() {
                        "none".to_string()
                    } else {
                        required.join(", ")
                    }
                )
            });
        }

        // Every optional body argument: an executed case that omits it,
        // behind an exact body assertion proving the key is absent (not
        // null) at the argument's own pointer.
        for opt in &optional {
            let pointers: Vec<Vec<String>> =
                locations_of(&ctx.places, std::slice::from_ref(&opt.name))
                    .into_iter()
                    .filter_map(|l| match l {
                        Location::Body(p) => Some(p),
                        // The elements of a list: the array they sit in is
                        // what an omitted argument leaves out.
                        Location::BodyElements(p) => {
                            let cut = p.iter().position(|s| s == "*").unwrap_or(p.len());
                            Some(p[..cut].to_vec())
                        }
                        _ => None,
                    })
                    .collect();
            let body_carried = ctx.wire.body_args.contains(&opt.name);
            let container = is_container(&ctx.places, std::slice::from_ref(&opt.name));
            // A container has no single whole-argument pointer -- its
            // members are placed one by one -- but omission and null policy
            // are still checkable across every one of those member pointers
            // at once: all absent proves the whole thing omitted, and (for
            // an explicit-null call) all JSON null proves the whole thing
            // nulled. Skipping this just because there is no single pointer
            // was the bug (B3): a nullable container passed no test at all.
            let container_pointers: Vec<Vec<String>> = if container {
                ctx.places
                    .places
                    .iter()
                    .filter(|(p, _)| p.len() > 1 && p[0] == opt.name)
                    .filter_map(|(_, l)| match l {
                        Location::Body(ptr) => Some(ptr.clone()),
                        _ => None,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let check_pointers: Vec<Vec<String>> = if !pointers.is_empty() {
                vec![pointers[0].clone()]
            } else if !container_pointers.is_empty() {
                container_pointers
            } else {
                // An ambiguous value is already one gap, on the argument.
                if body_carried && !container && !ctx.ambiguous.contains_key(&opt.name) {
                    t.require(false, &ctx.key, GapKind::OmissionUnprovable, || {
                        format!(
                            "{}: {} is sent in the body at a position this instrument does not classify, so its omission cannot be proven",
                            ctx.key, opt.name
                        )
                    });
                }
                continue;
            };
            let key = check_pointers
                .iter()
                .map(|p| p.join("/"))
                .collect::<Vec<_>>()
                .join(", ");
            let omission_ok = ctx.calls.iter().any(|(case, passed)| {
                !passed.iter().any(|(n, _)| n == &opt.name)
                    && proof_stubs(ctx, case, mappings).iter().any(|(_, mapping)| {
                        exact_demanded_body(mapping)
                            .map(|b| check_pointers.iter().all(|p| at_pointer(&b, p).is_none()))
                            .unwrap_or(false)
                    })
                    && case_executed(workspace, evidence, "wiremock_e2e", case) == CaseExec::Proven
            });
            t.require(omission_ok, &ctx.key, GapKind::OmissionUnproven, || {
                format!(
                    "{}: no executed case omits {} behind an exact (non-loose) body assertion proving /{} is entirely absent",
                    ctx.key, opt.name, key
                )
            });

            // Explicit null (ADR 0079). The choice that applies is the
            // argument's resolved `null_handling` entry, else the
            // workspace's `defaults.null_handling`; with neither it is
            // undecided: there is no implicit default. Tests win: an
            // executed case contradicting the choice is unproven.
            let entry = decisions
                .iter()
                .find(|n| n.operation == ctx.key && n.argument == opt.name);
            let choice = entry
                .map(|n| (n.behavior.as_str(), "its null_handling decision"))
                .or(null_default.map(|d| (d, "defaults.null_handling")));
            let Some((behavior, from)) = choice else {
                t.require(false, &ctx.key, GapKind::NullHandlingUndecided, || {
                    format!(
                        "{}: neither a resolved null_handling decision for {} nor defaults.null_handling in selection.yaml says what an explicit null sends, so it is unproven",
                        ctx.key, opt.name
                    )
                });
                continue;
            };
            let send_null = behavior == "send_null";
            // What each executed case passing `null` demands across every
            // check pointer: `Some(true)` every one is JSON null (the whole
            // argument nulled), `Some(false)` every one is absent (the whole
            // argument omitted despite the explicit null), `None` a mix or
            // some other value -- for a flat argument there is exactly one
            // pointer, so this reduces to the original single-pointer check.
            let observed: Vec<(&str, Option<bool>)> = ctx
                .calls
                .iter()
                .filter(|(case, passed)| {
                    // A case passing an explicit null, or one leaving the
                    // argument out. The second only counts when its stub
                    // demands JSON null (an omitted argument the router
                    // sends as null): filtered below.
                    !passed
                        .iter()
                        .any(|(n, v)| n == &opt.name && !is_null_literal(v))
                        && case_executed(workspace, evidence, "wiremock_e2e", case)
                            == CaseExec::Proven
                })
                .flat_map(|(case, _)| {
                    let check_pointers = &check_pointers;
                    proof_stubs(ctx, case, mappings)
                        .into_iter()
                        .filter_map(move |(_, mapping)| {
                            let body = exact_demanded_body(mapping)?;
                            let states: Vec<Option<&Value>> = check_pointers
                                .iter()
                                .map(|p| at_pointer(&body, p))
                                .collect();
                            let outcome = if states.iter().all(|s| matches!(s, Some(Value::Null))) {
                                Some(true)
                            } else if states.iter().all(Option::is_none) {
                                Some(false)
                            } else {
                                None
                            };
                            Some((*case, outcome))
                        })
                })
                .collect();
            // A case that left the argument out proves nothing by its stub
            // showing the key absent (that is what omitting looks like);
            // only one whose stub demands JSON null there is evidence of
            // what the connector sends.
            let absent_cases: Vec<&str> = ctx
                .calls
                .iter()
                .filter(|(_, passed)| !passed.iter().any(|(n, _)| n == &opt.name))
                .map(|(c, _)| *c)
                .collect();
            let observed: Vec<(&str, Option<bool>)> = observed
                .into_iter()
                .filter(|(c, o)| !absent_cases.contains(c) || *o == Some(true))
                .collect();
            let contradicting: Vec<&str> = observed
                .iter()
                .filter(|(_, o)| *o != Some(send_null))
                .map(|(c, _)| *c)
                .collect();
            let supported = if send_null {
                observed.iter().any(|(_, o)| *o == Some(true))
            } else {
                omission_ok || observed.iter().any(|(_, o)| *o == Some(false))
            };
            t.require(
                contradicting.is_empty() && supported,
                &ctx.key,
                GapKind::NullUnproven,
                || {
                    if !contradicting.is_empty() {
                        format!(
                            "{}: {} for {} is {}, but executed case {} demands /{} otherwise",
                            ctx.key,
                            from,
                            opt.name,
                            behavior,
                            contradicting.join(", "),
                            key
                        )
                    } else if send_null {
                        format!(
                            "{}: {} for {} is send_null, but no executed case passes null behind a stub demanding /{} as JSON null",
                            ctx.key, from, opt.name, key
                        )
                    } else {
                        format!(
                            "{}: {} for {} is omit, but no executed case proves /{} absent",
                            ctx.key, from, opt.name, key
                        )
                    }
                },
            );
        }

        // Map five-case contract.
        for arg in ctx
            .args
            .iter()
            .filter(|a| a.is_list() && schema.is_map_entry_type(a.bare_type()))
        {
            for (label, ok) in map_five_cases(ctx, arg, mappings, evidence, workspace) {
                t.require(ok, &ctx.key, GapKind::MapCaseMissing, || {
                    format!(
                        "{}: map argument {} is missing its {} case",
                        ctx.key, arg.name, label
                    )
                });
            }
        }
    }
    if !any_write {
        return not_applicable(
            id,
            "no selected write operation (a POST/PUT/PATCH sending a body) declares any argument",
        );
    }
    finish(id, t)
}

fn exec_ok(
    case: Option<&(&str, Vec<(String, String)>)>,
    evidence: Option<&Value>,
    workspace: &Path,
) -> bool {
    match case {
        Some((name, _)) => matches!(
            case_executed(workspace, evidence, "wiremock_e2e", name),
            CaseExec::Proven
        ),
        None => false,
    }
}

/// Every `null_handling` entry on a resolved decision in
/// `.factory/decisions.json` (ADR 0079). An open or superseded record's
/// entries do not count; prose is never read.
fn null_decisions(doc: &Value) -> Vec<crate::decisions::NullHandling> {
    get_arr(doc, "decisions")
        .into_iter()
        .flatten()
        .filter(|d| get_str(d, "status") == Some("resolved"))
        .flat_map(|d| get_arr(d, "null_handling").cloned().unwrap_or_default())
        .filter_map(|n| {
            Some(crate::decisions::NullHandling {
                operation: get_str(&n, "operation")?.to_string(),
                argument: get_str(&n, "argument")?.to_string(),
                behavior: get_str(&n, "behavior")?.to_string(),
            })
        })
        .collect()
}

/// Every required map variant needs its own executed, asserted proof --
/// naming the case in `tests/cases/` is not proof by itself: the file (and
/// its stub) can sit there, untouched, after its `PASS:` line drops out of
/// the recorded log.
fn map_five_cases(
    ctx: &OpCtx,
    arg: &ArgInfo,
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    workspace: &Path,
) -> Vec<(&'static str, bool)> {
    let mut empty = false;
    let mut one = false;
    let mut several_distinct = false;
    let mut nested_value = false;
    let mut bad_key = false;
    let key_re = Regex::new(r#"key\s*:\s*"((?:[^"\\]|\\.)*)""#).unwrap();
    let value_obj_re = Regex::new(r"value\s*:\s*\{").unwrap();
    let name_re = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
    for (case, passed) in &ctx.calls {
        let Some(value) = passed
            .iter()
            .find(|(n, _)| n == &arg.name)
            .map(|(_, v)| v.clone())
        else {
            continue;
        };
        let has_proof = mappings
            .iter()
            .any(|(mname, mapping)| serves(mname, mapping, case))
            && case_executed(workspace, evidence, "wiremock_e2e", case) == CaseExec::Proven;
        if !has_proof {
            continue;
        }
        let elements = list_elements_text(&value);
        if elements.is_empty() && value.trim() == "[]" {
            empty = true;
        }
        if elements.len() == 1 {
            one = true;
        }
        let keys: Vec<String> = elements
            .iter()
            .filter_map(|e| key_re.captures(e).map(|m| m[1].to_string()))
            .collect();
        let distinct_keys: std::collections::HashSet<&str> =
            keys.iter().map(String::as_str).collect();
        if elements.len() >= 2 && distinct_keys.len() >= 2 {
            several_distinct = true;
        }
        if elements.iter().any(|e| value_obj_re.is_match(e)) {
            nested_value = true;
        }
        if keys.iter().any(|k| !name_re.is_match(k)) {
            bad_key = true;
        }
    }
    vec![
        ("empty map ([])", empty),
        ("single entry", one),
        ("several entries with distinct keys", several_distinct),
        ("a value needing its own nested typing", nested_value),
        ("a key that is not a valid GraphQL name", bad_key),
    ]
}

// ── Obligation 4: unit-coverage, cross-checked against executed e2e ─────────

/// An operation lint's `missing-unit` class would flag: its connector sends
/// a body that is not `Flat` (an object-valued `$args` a unit tool cannot
/// express, per `references/testing.md`), or it declares a list-typed
/// argument (a repeated query key `rover` cannot assert either). Recomputed
/// here rather than read from `lint`'s own finding so this obligation does
/// not depend on lint having already run in the same process.
fn needs_e2e_routing(ctx: &OpCtx) -> bool {
    (ctx.wire.sends_body && ctx.body_flat.is_none() && !ctx.wire.body_args.is_empty())
        || ctx.args.iter().any(|a| a.is_list())
}

fn unit_coverage(
    ctxs: &[OpCtx],
    mappings: &[(String, Value)],
    evidence: Option<&Value>,
    workspace: &Path,
) -> Obligation {
    let id = "serialization.unit-coverage";
    let mut t = Tally::new();
    let mut any = false;
    for ctx in ctxs.iter().filter(|c| needs_e2e_routing(c)) {
        any = true;
        let has_e2e_proof = ctx.calls.iter().any(|(case, _)| {
            mappings
                .iter()
                .any(|(mname, mapping)| serves(mname, mapping, case))
                && matches!(
                    case_executed(workspace, evidence, "wiremock_e2e", case),
                    CaseExec::Proven
                )
        });
        t.require(has_e2e_proof, &ctx.key, GapKind::UnitWithoutE2e, || {
            format!(
                "{}: {} cannot be proven at the connector-unit layer (object-valued or list-valued $args) and has no executed, passing e2e case to carry the proof instead",
                ctx.key, ctx.field
            )
        });
    }
    if !any {
        return not_applicable(
            id,
            "no selected operation is outside the connector-unit layer's expressive range",
        );
    }
    finish(id, t)
}

// ── Shared result plumbing ──────────────────────────────────────────────────

fn not_applicable(id: &str, message: &str) -> Obligation {
    Obligation {
        id: id.to_string(),
        status: ObligationStatus::NotApplicable,
        message: message.to_string(),
        evidence_ref: Some(".factory/selection.yaml".into()),
        denominator: Some((0, 0)),
        gaps: Vec::new(),
    }
}

fn unexecuted(id: &str, message: &str, evidence_ref: &str) -> Obligation {
    Obligation {
        id: id.to_string(),
        status: ObligationStatus::Unexecuted,
        message: message.to_string(),
        evidence_ref: Some(evidence_ref.to_string()),
        denominator: None,
        gaps: Vec::new(),
    }
}

fn finish(id: &str, t: Tally) -> Obligation {
    let message = if t.gaps.is_empty() {
        format!(
            "{} of {} required outbound assertion(s) executed and proven",
            t.covered, t.total
        )
    } else {
        format!(
            "{} of {} required outbound assertion(s) unproven: {}",
            t.total - t.covered,
            t.total,
            t.gaps
                .iter()
                .take(6)
                .map(|g| g.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    Obligation {
        id: id.to_string(),
        status: if t.gaps.is_empty() {
            ObligationStatus::Pass
        } else {
            ObligationStatus::Fail
        },
        message,
        evidence_ref: Some(
            "tests/cases + tests/fixtures/mappings + .factory/evidence/latest.json".into(),
        ),
        denominator: Some((t.covered, t.total)),
        gaps: t.gaps,
    }
}

// ── Entry point ──────────────────────────────────────────────────────────────

const OBLIGATION_IDS: [&str; 4] = [
    "serialization.write-body-assertions",
    "serialization.list-argument-proof",
    "serialization.mutation-cases",
    "serialization.unit-coverage",
];

fn all_unexecuted(message: &str, evidence_ref: &str) -> Report {
    Report::new(
        Vec::new(),
        OBLIGATION_IDS
            .iter()
            .map(|id| unexecuted(id, message, evidence_ref))
            .collect(),
        std::collections::BTreeMap::new(),
        Vec::new(),
    )
}

/// The obligations alone, as `report` computes them.
pub fn obligations(workspace: &Path) -> Vec<Obligation> {
    report(workspace).obligations
}

/// Every selected operation's outbound-request proof, read from the
/// workspace's schema, selection, inventory, test cases, WireMock stubs,
/// resolved decisions and `.factory/evidence/latest.json` on disk.
/// Read-only. The standalone CLI's entry point: a caller running mid-way
/// through its own evidence run (`cmd/evidence.rs`'s new layer) must use
/// `report_with_evidence` instead, passing its own in-memory results --
/// `latest.json` on disk is the *previous* run's file until this run
/// finishes writing it (the same-run evidence contract, ADR 0079 Step 2).
pub fn report(workspace: &Path) -> Report {
    report_with_evidence(workspace, load_evidence(workspace).as_ref())
}

/// `report`'s logic, taking the evidence value as a parameter instead of
/// reading `.factory/evidence/latest.json` itself. `evidence` is shaped the
/// same way that file is: `{"layers": {"wiremock_e2e": {"status", "log"},
/// "connector_unit": {...}}}`. A per-case log is still read from the
/// filesystem at the path evidence names -- current-run callers pass their
/// own fresh, uniquely-timestamped log path, never the previous run's.
pub fn report_with_evidence(workspace: &Path, evidence: Option<&Value>) -> Report {
    let read_factory = |rel: &str| {
        crate::factory_io::read_to_string_optional(workspace, rel)
            .ok()
            .flatten()
    };
    let selection =
        read_factory(".factory/selection.yaml").and_then(|s| crate::yaml::parse(&s).ok());
    let inventory =
        read_factory(".factory/inventory.json").and_then(|s| crate::json::parse(&s).ok());
    let (selection, inventory) = match (selection, inventory) {
        (Some(s), Some(i)) => (s, i),
        _ => {
            return all_unexecuted(
                "no .factory/selection.yaml or .factory/inventory.json to check",
                ".factory/inventory.json",
            )
        }
    };
    let workspace_yaml = read_factory(".factory/workspace.yaml")
        .and_then(|s| crate::yaml::parse(&s).ok())
        .unwrap_or(Value::Null);
    let Some(directory) = get_str(&workspace_yaml, "directory") else {
        return all_unexecuted(
            "no directory in .factory/workspace.yaml, so no schema file to read",
            ".factory/workspace.yaml",
        );
    };
    let schema_path = workspace.join(format!("{}.graphql", directory));
    // Read through the same custody-aware path `.factory/*` reads use
    // elsewhere (ADR 0025): a hand-edited `directory: .factory/probe` with a
    // symlink planted at `.factory/probe.graphql` must not be followed.
    let sdl = match crate::factory_io::read_named_path(&schema_path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
    {
        Some(s) => s,
        None => {
            let msg = format!("no schema file yet at {}", schema_path.display());
            return all_unexecuted(&msg, &schema_path.to_string_lossy());
        }
    };
    let (decisions, omits) = match crate::decisions::load_present(workspace, None) {
        Ok(doc) => doc
            .map(|d| (null_decisions(&d), crate::obligations::omits_from_doc(&d)))
            .unwrap_or_default(),
        Err(e) => {
            // An unreadable decisions log could hide a documented
            // explicit-null distinction: refuse rather than under-require.
            return all_unexecuted(&e, ".factory/decisions.json");
        }
    };
    // A request omit on a current finding counts as one on a resolved
    // decision (ADR 0113 §2); `null_handling` lives on decisions only. An
    // unreadable findings.json contributes nothing and blanks nothing: the
    // decisions' omits keep counting, and lint reports the file.
    let omits = match crate::findings::load_present(workspace, None) {
        Ok(Some(f)) => {
            let mut omits = omits;
            omits.extend(crate::obligations::omits_from_doc(&crate::findings::union(
                &crate::decisions::empty(),
                &f,
            )));
            omits
        }
        Ok(None) | Err(_) => omits,
    };

    let prefix = get_str(&workspace_yaml, "field_prefix").unwrap_or("");
    let shapes = get_obj(&inventory, "shapes").cloned().unwrap_or_default();
    let inv_ops = get_arr(&inventory, "operations")
        .cloned()
        .unwrap_or_default();
    let cases = load_cases(workspace);
    let mappings = load_mappings(workspace);

    let schema = SchemaCache::new(&sdl);
    let mut ctxs: Vec<OpCtx> = Vec::new();
    for (key, entry, field) in selected_ops(&selection, prefix) {
        let inv_op = inv_ops
            .iter()
            .find(|o| get_str(o, "key") == Some(key.as_str()));
        let inventory_method = inv_op
            .and_then(|o| get_str(o, "method"))
            .map(str::to_uppercase);
        if let Some(mut ctx) = build_ctx(&schema, &key, entry, &field, inventory_method, &cases) {
            executed_placements(&mut ctx, &mappings, evidence, workspace);
            // Source request-body members the connector never writes: a
            // member is mapped when the wire map places an argument or a
            // literal at its key, unproven when the reader could not
            // classify that position (or the whole body), unmapped only
            // when the body is fully read and never writes it.
            if let Some(op) = inv_op {
                let top = |p: &Vec<String>| p.first().cloned();
                let placed: Vec<String> = ctx
                    .places
                    .places
                    .iter()
                    .filter_map(|(_, l)| match l {
                        Location::Body(p) | Location::BodyElements(p) => top(p),
                        _ => None,
                    })
                    .chain(ctx.places.literals.iter().filter_map(|(p, _)| top(p)))
                    .collect();
                let whole_body_unread = ctx.places.unclassified.iter().any(|p| p.is_empty());
                let unread: Vec<String> = ctx.places.unclassified.iter().filter_map(top).collect();
                let required = required_request_members(op, &shapes);
                for m in request_body_member_names(op, &shapes) {
                    if placed.contains(&m) {
                        continue;
                    }
                    if whole_body_unread || unread.contains(&m) {
                        ctx.body_members_unclassified.push(m);
                    } else if let Some(omit) = crate::obligations::find_omit(
                        &omits,
                        &key,
                        crate::obligations::Direction::Request,
                        &m,
                    ) {
                        // A resolved decision records why the connector
                        // does not send this member (`decisions add
                        // --omit`); source-coverage reads the same record.
                        // It covers an optional member only: a member the
                        // source requires stays a gap, and names the decision.
                        if required.contains(&m) {
                            ctx.required_omitted.push((m, omit.decision.clone()));
                        }
                    } else {
                        ctx.body_args_unmatched.push(m);
                    }
                }
            }
            ctxs.push(ctx);
        }
    }

    let obligations = vec![
        write_body_assertions(&ctxs, &mappings, evidence, workspace),
        list_argument_proof(&ctxs, &mappings, evidence, workspace),
        mutation_cases(
            &ctxs,
            &schema,
            &mappings,
            evidence,
            &decisions,
            get(&selection, "defaults").and_then(|d| get_str(d, "null_handling")),
            workspace,
        ),
        unit_coverage(&ctxs, &mappings, evidence, workspace),
    ];
    let writes = ctxs
        .iter()
        .filter(|c| is_write(c))
        .map(|ctx| {
            let gaps: Vec<Gap> = obligations
                .iter()
                .flat_map(|o| o.gaps.iter())
                .filter(|g| g.operation == ctx.key)
                .cloned()
                .collect();
            // Any gap on this write -- not just an unasserted argument or an
            // unmapped source member -- means some outbound value is not
            // proven: an unproven mapping, an unproven list, an unproven
            // omission or null, or a missing map-shaped case all count.
            let body_proven = gaps.is_empty();
            WriteReport {
                operation: ctx.key.clone(),
                field: ctx.field.clone(),
                method: ctx.verb.clone(),
                arguments: ctx.args.len(),
                body_proven,
                gaps,
            }
        })
        .collect();
    let case_proofs = case_proofs_for(&ctxs, evidence, workspace);
    let placements = placements_of(&ctxs);
    Report::new(writes, obligations, case_proofs, placements)
}

/// Every case any selected write references, verdict `pass` when the e2e
/// log records it executed and passing, `fail` when recorded failed,
/// `unproven` when the log carries no record of it at all -- ADR 0079 Step
/// 2's `case_proofs`, attached to `wiremock_e2e`'s own evidence-layer entry
/// by the caller (`cmd/evidence.rs`), never a separate layer's own field.
fn case_proofs_for(
    ctxs: &[OpCtx],
    evidence: Option<&Value>,
    workspace: &Path,
) -> std::collections::BTreeMap<String, CaseProof> {
    let mut out = std::collections::BTreeMap::new();
    for ctx in ctxs.iter().filter(|c| is_write(c)) {
        for (case, _) in &ctx.calls {
            out.entry(case.to_string()).or_insert_with(|| {
                match case_record(workspace, evidence, "wiremock_e2e", case) {
                    CaseExec::Proven => CaseProof {
                        verdict: "pass".to_string(),
                        findings: Vec::new(),
                    },
                    CaseExec::Failed => CaseProof {
                        verdict: "fail".to_string(),
                        findings: vec!["the case is recorded as failed".to_string()],
                    },
                    CaseExec::NotRecorded(reason) => CaseProof {
                        verdict: "unproven".to_string(),
                        findings: vec![reason],
                    },
                }
            });
        }
    }
    out
}

#[cfg(test)]
mod tests;

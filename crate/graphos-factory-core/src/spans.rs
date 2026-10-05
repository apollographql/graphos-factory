//! Schema spans: the units the hand-edit contract is written in.
//!
//! The schema is cut into top-level spans — the `extend schema`/`@source`
//! header, every non-root type declaration, and every root field of `Query`
//! and `Mutation` — each with a stable key and a SHA-256 of its exact text.
//! `applied.lock.yaml` records those hashes when the agent last wrote (or
//! acknowledged) the schema, so the next run can name every span a human
//! changed since; `selection.yaml`'s `overrides` are keyed by the same
//! names.
//!
//! Keys:
//!   `header`            everything before the first type declaration
//!   `type:<Name>`       a type / interface / input / enum / union / scalar
//!   `<method>:<path>`   a root field whose @connect matches an inventory
//!                       operation (several fields on one operation share
//!                       the key; their texts are concatenated in order)
//!   `Query.<f>` /
//!   `Mutation.<f>`      a root field with no connector, or one the
//!                       inventory does not explain
//!
//! A span's text runs from the end of the previous span's body to the end
//! of its own, so doc strings and `#` comments belong to the declaration
//! they precede — exactly what the engineer thinks of as "that operation".

use crate::graphql::{blank, field_names, type_declarations};
use crate::op_match::OpHints;
use crate::reconcile::{field_spans, match_operation};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct Span {
    pub key: String,
    /// `header` | `type` | `operation` | `field`
    pub kind: &'static str,
    pub line: usize,
    pub text: String,
    pub sha256: String,
    /// Argument names for root fields; field names for types.
    pub names: Vec<String>,
    pub tags: Vec<String>,
    /// Why a root field with a connector carries `Query.<f>` rather than an
    /// operation key: its path matches several operations equally and the
    /// selection declares none of them (ADR 0044).
    pub unattributed: Option<String>,
}

pub fn sha256_hex(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

fn span(
    key: String,
    kind: &'static str,
    line: usize,
    text: &str,
    names: Vec<String>,
    tags: Vec<String>,
) -> Span {
    Span {
        key,
        kind,
        line,
        sha256: sha256_hex(text),
        text: text.to_string(),
        names,
        tags,
        unattributed: None,
    }
}

fn tag_names(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r#"name\s*:\s*"([^"]*)""#).unwrap();
    crate::graphql::directives(text, "tag")
        .iter()
        .filter_map(|d| re.captures(&d.args).map(|m| m[1].to_string()))
        .collect()
}

/// The body of a braced declaration in `code` (the blanked SDL): the
/// offsets of its opening `{` and of the byte after its matching `}`.
/// Directives on the declaration (`@connect(... http: { ... } ...)`) carry
/// braces inside parentheses; the body is the first `{` at bracket depth
/// zero.
fn braced_body(code: &str, from: usize, limit: usize) -> Option<(usize, usize)> {
    let cb = code.as_bytes();
    let mut depth = 0i32;
    let mut open = None;
    let mut i = from;
    while i < cb.len() {
        let c = cb[i];
        match open {
            None => {
                if i >= limit {
                    return None;
                }
                match c {
                    b'(' | b'[' => depth += 1,
                    b')' | b']' => depth -= 1,
                    b'{' if depth == 0 => {
                        open = Some(i);
                        depth = 1;
                    }
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
            }
            Some(o) => match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((o, i + 1));
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Last code byte (exclusive) before `limit`, at or after `from`.
fn statement_end(code: &str, from: usize, limit: usize) -> usize {
    let cb = code.as_bytes();
    let mut end = limit.min(cb.len());
    while end > from && cb[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}

/// Brace depth of every byte offset in `code` — a declaration keyword inside
/// a body (an enum value named `type`) is not a declaration.
fn top_level(code: &str, index: usize) -> bool {
    let mut depth = 0i32;
    for &c in &code.as_bytes()[..index.min(code.len())] {
        match c {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
    }
    depth == 0
}

/// Cut the schema into spans. `inventory` lets root fields take their
/// operation key; without it they are `Query.<field>` / `Mutation.<field>`.
/// `hints` settles a path several operations match equally.
pub fn spans(sdl: &str, inventory: Option<&Value>, hints: &OpHints) -> Vec<Span> {
    let code = blank(sdl);
    let decls: Vec<_> = type_declarations(sdl)
        .into_iter()
        .filter(|d| top_level(&code, d.index))
        .collect();
    let mut out: Vec<Span> = Vec::new();

    // Header: up to the last code byte before the first declaration.
    let first = decls.first().map(|d| d.index).unwrap_or(sdl.len());
    let header_end = statement_end(&code, 0, first);
    out.push(span(
        "header".into(),
        "header",
        1,
        &sdl[..header_end],
        vec![],
        vec![],
    ));

    let mut prev_end = header_end;
    for (i, d) in decls.iter().enumerate() {
        let limit = decls.get(i + 1).map(|n| n.index).unwrap_or(sdl.len());
        let braced = matches!(d.kind.as_str(), "type" | "interface" | "input" | "enum");
        let body = if braced {
            braced_body(&code, d.index, limit)
        } else {
            None
        };
        let end = match body {
            Some((_, e)) => e,
            None => statement_end(&code, d.index, limit),
        };
        let text = &sdl[prev_end..end];
        let is_root = d.kind == "type" && (d.name == "Query" || d.name == "Mutation");
        if !is_root {
            let names = body
                .map(|(o, e)| field_names(&sdl[o + 1..e - 1]))
                .unwrap_or_default();
            out.push(span(
                format!("type:{}", d.name),
                "type",
                d.line,
                text,
                names,
                tag_names(text),
            ));
        } else {
            for f in field_spans(sdl, &d.name) {
                let matched = match (&f.connect, inventory) {
                    (Some(c), Some(inv)) => match_operation(
                        inv,
                        c.method.as_deref(),
                        c.path.as_deref(),
                        &hints.for_field(&d.name, &f.name),
                    ),
                    _ => Ok(None),
                };
                let (key, unattributed) = match matched {
                    Ok(op) => (
                        op.and_then(|op| crate::json::get_str(op, "key"))
                            .map(str::to_string),
                        None,
                    ),
                    Err(e) => (None, Some(e)),
                };
                match key {
                    Some(k) => {
                        if let Some(existing) = out.iter_mut().find(|s| s.key == k) {
                            existing.text.push('\n');
                            existing.text.push_str(&f.text);
                            existing.sha256 = sha256_hex(&existing.text);
                            existing.names.extend(f.args.iter().cloned());
                            existing.tags.extend(f.tags.iter().cloned());
                        } else {
                            out.push(span(
                                k,
                                "operation",
                                f.line,
                                &f.text,
                                f.args.clone(),
                                f.tags.clone(),
                            ));
                        }
                    }
                    None => out.push(Span {
                        unattributed,
                        ..span(
                            format!("{}.{}", d.name, f.name),
                            "field",
                            f.line,
                            &f.text,
                            f.args.clone(),
                            f.tags.clone(),
                        )
                    }),
                }
            }
        }
        prev_end = end;
    }
    out
}

#[derive(Debug, Clone)]
pub struct HandEdit {
    pub key: String,
    /// `modified` | `added` | `removed`
    pub change: &'static str,
    pub line: Option<usize>,
}

/// What settles an unattributed span: a declaration in selection.yaml, never
/// `codify` — its `Query.<f>` key would move to an operation key the moment
/// the selection names the field (ADR 0044).
pub const SETTLE_TIE: &str = "declare each field's operation with graphql.root and graphql.name in selection.yaml, then lock again; codify cannot record it, because its key moves to the operation's once the selection names it";

/// `<key>: <why>` for every root field whose path matches several
/// operations equally and that the selection does not settle.
pub fn unattributed(current: &[Span]) -> Vec<String> {
    current
        .iter()
        .filter_map(|s| s.unattributed.as_ref().map(|e| format!("{}: {}", s.key, e)))
        .collect()
}

/// Every span that differs from what the lock recorded. An unattributed
/// span is not a hand edit to codify but a tie to settle; `unattributed`
/// reports it.
pub fn compare_with_lock(current: &[Span], lock: &Value) -> Vec<HandEdit> {
    let recorded = crate::json::get_obj(lock, "spans")
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for s in current.iter().filter(|s| s.unattributed.is_none()) {
        match recorded.get(&s.key).and_then(Value::as_str) {
            None => out.push(HandEdit {
                key: s.key.clone(),
                change: "added",
                line: Some(s.line),
            }),
            Some(h) if h != s.sha256 => out.push(HandEdit {
                key: s.key.clone(),
                change: "modified",
                line: Some(s.line),
            }),
            _ => {}
        }
    }
    for k in recorded.keys() {
        if !current.iter().any(|s| &s.key == k) {
            out.push(HandEdit {
                key: k.clone(),
                change: "removed",
                line: None,
            });
        }
    }
    out
}

/// The lock document for the current spans.
pub fn lock_document(schema_file: &str, current: &[Span]) -> Value {
    let mut spans = crate::json::obj();
    for s in current {
        spans.insert(s.key.clone(), Value::from(s.sha256.as_str()));
    }
    crate::json::object(vec![
        ("contract_version", Value::from(1)),
        ("schema", Value::from(schema_file)),
        ("written_at", Value::from(crate::now_iso())),
        ("written_by", Value::from(crate::written_by())),
        ("spans", Value::Object(spans)),
    ])
}

pub const LOCK_FILE: &str = ".factory/applied.lock.yaml";
pub const INVENTORY_FILE: &str = ".factory/inventory.json";

/// The content hash of a workspace's `inventory.json`, over its compact JSON
/// so re-indenting never counts as an edit. `None` when there is no
/// inventory or it does not parse.
pub fn inventory_hash(dir: &std::path::Path) -> Option<String> {
    let text = crate::factory_io::read_to_string(dir, INVENTORY_FILE).ok()?;
    let value = crate::json::parse(&text).ok()?;
    Some(sha256_hex(&crate::json::compact(&value)))
}

/// Has `inventory.json` changed since the lock acknowledged it? `None` when
/// there is nothing to compare (no lock entry, or no readable inventory);
/// `Some(false)` when it is in sync.
pub fn inventory_changed(dir: &std::path::Path, lock: Option<&Value>) -> Option<bool> {
    let recorded = crate::json::get_str(lock?, "inventory")?;
    let current = inventory_hash(dir)?;
    Some(current != recorded)
}

pub fn read_lock(dir: &std::path::Path) -> Result<Option<Value>, String> {
    match crate::factory_io::read_to_string_optional(dir, LOCK_FILE)? {
        Some(text) => crate::yaml::parse(&text)
            .map_err(|e| format!("{}: {}", LOCK_FILE, e))
            .map(Some),
        None => Ok(None),
    }
}

pub fn write_lock(dir: &std::path::Path, lock: &Value) -> Result<(), String> {
    crate::factory_io::write_in_place(dir, LOCK_FILE, crate::yaml::stringify(lock, 0).as_bytes())
        .map_err(String::from)
}

/// Whitespace-insensitive containment: runs of whitespace compare equal.
pub fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Evaluate one override assertion against a span. Err for an unknown kind
/// or a bad regex.
pub fn check_assertion(kind: &str, value: &str, s: &Span) -> Result<bool, String> {
    Ok(match kind {
        "contains" => squash(&s.text).contains(&squash(value)),
        "not_contains" => !squash(&s.text).contains(&squash(value)),
        "matches" => regex::Regex::new(value)
            .map_err(|e| format!("matches: invalid regex {:?}: {}", value, e))?
            .is_match(&s.text),
        "tag" => s.tags.iter().any(|t| t == value),
        "arg" | "field" => s.names.iter().any(|n| n == value),
        "no_arg" | "no_field" => !s.names.iter().any(|n| n == value),
        other => return Err(format!("unknown assertion kind {:?}", other)),
    })
}

pub const ASSERTION_KINDS: [&str; 8] = [
    "contains",
    "not_contains",
    "matches",
    "tag",
    "arg",
    "no_arg",
    "field",
    "no_field",
];

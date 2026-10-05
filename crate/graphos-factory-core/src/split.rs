//! `decisions migrate --split` (ADR 0113 §5): one pass over an existing
//! `decisions.json` that leaves it holding decisions only.
//!
//! Every record is classified first, mechanically:
//!
//! - **kept** — it carries a `question`, `choices` or an `editorial` omit:
//!   it stays a decision, id and all. A record carrying any instrument
//!   payload (`omits`, `json_reasons`, `null_handling`, `secret_fields`) is
//!   never treated as provenance, whatever its title (Asana's D-0038).
//! - **provenance** — titled `Hand edit codified:`, `Source patch:` or
//!   `Conformance waiver:`: moved onto the override, patch or waiver whose
//!   `decision:` names it — its `context` copied into the entry's `context`
//!   unless it opens with the binary's template sentence — and dropped. A
//!   record no entry cites (what `--expressed` left) becomes a finding,
//!   `source: codify`.
//! - **sources** — titled `Upstream replaced:` / `Upstream refreshed:`: a
//!   finding, `source: sources`.
//! - **hand** — everything else, which the agent running the migration
//!   sorts in a `--sorted` file: `decision` (re-recorded with its question
//!   and choices), `finding`, `memory` (a line under `## Tried and rejected`
//!   in memory.md) or `drop`. The split refuses to run while one is
//!   unassigned.
//!
//! Decision ids are preserved; findings get fresh `F-nnnn` ids. Every
//! `decision:` reference in selection.yaml and sources.lock.yaml that names
//! a record which is no longer a decision is removed (replaced by the
//! carried `context` where there is one), line by line, and the selection's
//! `contract_version` becomes 2. Nothing is written until every output
//! validates against its schema.

use crate::decisions::Omit;
use crate::findings::NewFinding;
use crate::json::{self, get, get_arr, get_str};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const SELECTION: &str = ".factory/selection.yaml";
pub const MEMORY: &str = ".factory/memory.md";
pub const TRIED_AND_REJECTED: &str = "## Tried and rejected";

/// What kind of entry a provenance record shadows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    Override,
    Patch,
    Waiver,
}

impl Provenance {
    fn title_prefix(self) -> &'static str {
        match self {
            Provenance::Override => "Hand edit codified:",
            Provenance::Patch => "Source patch:",
            Provenance::Waiver => "Conformance waiver:",
        }
    }

    /// Whether `context` opens the way the binary wrote it when `--context`
    /// was not given (`codify::decision_entry`, `source_decision_entry`,
    /// `waiver_decision_entry`): such a context says nothing the entry
    /// does not, and is not carried. A waiver's names the binary, under
    /// whichever name wrote it.
    fn is_template(self, context: &str) -> bool {
        match self {
            Provenance::Override => context.starts_with("The engineer edited "),
            Provenance::Patch => context.starts_with("The working copy "),
            Provenance::Waiver => context
                .strip_prefix('`')
                .and_then(|rest| rest.split_once(' '))
                .is_some_and(|(bin, rest)| {
                    !bin.is_empty() && rest.starts_with("validate` reports ")
                }),
        }
    }

    fn entry(self) -> &'static str {
        match self {
            Provenance::Override => "override",
            Provenance::Patch => "patch",
            Provenance::Waiver => "waiver",
        }
    }
}

/// A record's mechanical class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Kept,
    Provenance(Provenance),
    Sources,
    Hand,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Kept => "kept",
            Class::Provenance(_) => "provenance",
            Class::Sources => "sources",
            Class::Hand => "hand",
        }
    }
}

fn non_empty_arr(rec: &Value, key: &str) -> bool {
    get_arr(rec, key).is_some_and(|a| !a.is_empty())
}

/// The payload fields only a decision holds (a finding holds `omits` and
/// `affects` only).
const DECISION_ONLY: &[&str] = &["json_reasons", "null_handling", "secret_fields"];

fn carries_payload(rec: &Value) -> bool {
    non_empty_arr(rec, "omits") || DECISION_ONLY.iter().any(|k| non_empty_arr(rec, k))
}

fn has_editorial_omit(rec: &Value) -> bool {
    get_arr(rec, "omits")
        .into_iter()
        .flatten()
        .any(|o| get_str(o, "reason") == Some("editorial"))
}

/// Whether a record names its alternative: a question, two or more
/// choices, or an `editorial` omit (its alternative is to expose the path)
/// — what `decisions add` requires (ADR 0113 §1) and lint's
/// `decision-without-alternative` asks for.
pub fn has_alternative(rec: &Value) -> bool {
    get_str(rec, "question").is_some_and(|q| !q.trim().is_empty())
        || get_arr(rec, "choices").is_some_and(|c| c.len() >= 2)
        || has_editorial_omit(rec)
}

/// The mechanical class of one record (ADR 0113 §5).
pub fn classify(rec: &Value) -> Class {
    let has_question = get_str(rec, "question").is_some_and(|q| !q.trim().is_empty());
    if has_question || non_empty_arr(rec, "choices") || has_editorial_omit(rec) {
        return Class::Kept;
    }
    if carries_payload(rec) {
        return Class::Hand;
    }
    let title = get_str(rec, "title").unwrap_or("");
    for p in [Provenance::Override, Provenance::Patch, Provenance::Waiver] {
        if title.starts_with(p.title_prefix()) {
            return Class::Provenance(p);
        }
    }
    if title.starts_with("Upstream replaced:") || title.starts_with("Upstream refreshed:") {
        return Class::Sources;
    }
    Class::Hand
}

/// A record's prose, for a finding's body: its context, then its decision
/// text and note, in that order.
fn prose(rec: &Value) -> String {
    let res = get(rec, "resolution");
    [
        get_str(rec, "context"),
        res.and_then(|r| get_str(r, "decision")),
        res.and_then(|r| get_str(r, "note")),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

fn strings_of(v: Option<&Value>) -> Vec<String> {
    json::strings(v)
}

fn omits_of(rec: &Value) -> Vec<Omit> {
    get_arr(rec, "omits")
        .into_iter()
        .flatten()
        .map(|o| Omit {
            operation: get_str(o, "operation").unwrap_or("").to_string(),
            direction: get_str(o, "direction").unwrap_or("").to_string(),
            path: get_str(o, "path").unwrap_or("").to_string(),
            reason: get_str(o, "reason").unwrap_or("").to_string(),
        })
        .collect()
}

/// Why the split refused.
#[derive(Debug)]
pub enum Refusal {
    /// Records only a person can sort, with no `--sorted` assignment.
    Unsorted(Vec<(String, String)>),
    /// Anything else: an invalid `--sorted` entry, an output that would not
    /// validate, a reference the line-level rewrite cannot reach.
    Invalid(String),
}

/// One record's fate, for the report.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub id: String,
    pub title: String,
    pub class: Class,
    /// `kept`, `decision`, `moved`, `finding`, `memory` or `dropped`.
    pub outcome: &'static str,
    /// The finding's id, when it became one.
    pub finding: Option<String>,
    /// The entries a moved record's context landed on, or why it did not.
    pub detail: Option<String>,
}

/// Everything the split would write, computed without writing.
pub struct Plan {
    pub decisions: Value,
    pub findings: Value,
    /// The new text, when it changed.
    pub selection: Option<String>,
    pub sources_lock: Option<String>,
    pub memory: Option<String>,
    pub outcomes: Vec<Outcome>,
    pub warnings: Vec<String>,
}

impl Plan {
    pub fn count(&self, outcome: &str) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.outcome == outcome)
            .count()
    }

    /// The files [`write`] writes, in order.
    pub fn writes(&self) -> Vec<&'static str> {
        let mut out = vec![crate::findings::FILE, crate::decisions::FILE];
        if self.selection.is_some() {
            out.push(SELECTION);
        }
        if self.sources_lock.is_some() {
            out.push(crate::sources::SOURCES_LOCK);
        }
        if self.memory.is_some() {
            out.push(MEMORY);
        }
        out
    }
}

/// The `--sorted` file: a mapping from each D-id to its assignment.
///
/// ```yaml
/// D-0001:
///   as: decision            # re-recorded with its alternative
///   question: "…"
///   choices:
///     - { id: twelve, label: "Twelve operations across five resources" }
///     - { id: all, label: "Every operation the spec lists" }
///   chosen: [twelve]        # required on a resolved record with choices
///   by: agent               # default agent when the record names no one
///   note: "…"               # optional: replaces resolution.note
/// D-0005:
///   as: finding
///   cites: "references/naming.md § Enums"
///   body: "…"               # optional: default the record's prose
///   title: "…"              # optional; affects, evidence, related too
/// D-0012:
///   as: memory
///   line: "…"               # the line under `## Tried and rejected`
/// D-0021:
///   as: drop
/// ```
pub fn parse_sorted(text: &str) -> Result<Value, String> {
    let value = if text.trim_start().starts_with('{') {
        json::parse(text)?
    } else {
        crate::yaml::parse(text)?
    };
    if !value.is_object() {
        return Err(
            "the --sorted file must be a mapping from each D-nnnn to its assignment".into(),
        );
    }
    Ok(value)
}

const AS: &[&str] = &["decision", "finding", "memory", "drop"];
const SORTED_KEYS: &[&str] = &[
    "as", "question", "choices", "chosen", "by", "note", "title", "body", "cites", "affects",
    "evidence", "related", "line",
];

fn choice_values(entry: &Value, id: &str) -> Result<Option<Vec<Value>>, String> {
    let Some(raw) = get(entry, "choices") else {
        return Ok(None);
    };
    let items = raw
        .as_array()
        .ok_or_else(|| format!("{}: choices must be a list", id))?;
    let mut out = Vec::new();
    for c in items {
        let (Some(cid), Some(label)) = (get_str(c, "id"), get_str(c, "label")) else {
            return Err(format!("{}: every choice needs an id and a label", id));
        };
        let mut m = json::obj();
        m.insert("id".into(), Value::from(cid));
        m.insert("label".into(), Value::from(label));
        if let Some(d) = get_str(c, "detail") {
            m.insert("detail".into(), Value::from(d));
        }
        out.push(Value::Object(m));
    }
    Ok(Some(out))
}

/// Re-record `rec` as a decision with what the sorted entry adds.
fn as_decision(rec: &Value, entry: &Value, id: &str) -> Result<Value, String> {
    let mut out = rec.clone();
    for k in ["title", "question"] {
        if let Some(v) = get_str(entry, k) {
            json::set(&mut out, k, Value::from(v));
        }
    }
    if let Some(choices) = choice_values(entry, id)? {
        json::set(&mut out, "choices", Value::Array(choices));
    }
    if !has_alternative(&out) {
        return Err(format!(
            "{}: sorted as a decision, but it names no alternative — give it a question or two or more choices (no-alternative)",
            id
        ));
    }
    let resolved = get_str(&out, "status") != Some("open");
    if resolved {
        let mut res = get(&out, "resolution")
            .cloned()
            .unwrap_or_else(|| json::object(vec![]));
        if let Some(chosen) = get(entry, "chosen") {
            json::set(&mut res, "chosen", Value::from(json::strings(Some(chosen))));
        }
        if let Some(note) = get_str(entry, "note") {
            json::set(&mut res, "note", Value::from(note));
        }
        let by = get_str(entry, "by")
            .map(str::to_string)
            .or_else(|| get_str(&res, "by").map(str::to_string))
            .unwrap_or_else(|| "agent".to_string());
        json::set(&mut res, "by", Value::from(by));
        json::set(&mut out, "resolution", res);
    }
    let choice_ids: Vec<String> = get_arr(&out, "choices")
        .into_iter()
        .flatten()
        .filter_map(|c| get_str(c, "id").map(str::to_string))
        .collect();
    let chosen = json::strings(get(&out, "resolution").and_then(|r| get(r, "chosen")));
    if resolved && !choice_ids.is_empty() && chosen.is_empty() {
        return Err(format!(
            "{}: a resolved decision with choices needs `chosen` (which of {} was taken)",
            id,
            choice_ids.join(", ")
        ));
    }
    if let Some(bad) = chosen.iter().find(|c| !choice_ids.contains(c)) {
        return Err(format!(
            "{}: chosen {:?} is not one of its choices ({})",
            id,
            bad,
            choice_ids.join(", ")
        ));
    }
    Ok(out)
}

fn refuse_payload(rec: &Value, id: &str, as_: &str) -> Result<(), String> {
    if let Some(k) = DECISION_ONLY.iter().find(|k| non_empty_arr(rec, k)) {
        return Err(format!(
            "{}: carries {}, which only a decision holds; sort it as a decision (with its question and choices), not {}",
            id, k, as_
        ));
    }
    if as_ != "finding" && non_empty_arr(rec, "omits") {
        return Err(format!(
            "{}: carries omits an instrument reads; sort it as a decision or a finding, not {}",
            id, as_
        ));
    }
    if as_ == "finding" && has_editorial_omit(rec) {
        return Err(format!(
            "{}: carries an editorial omit, which lives only on a decision; sort it as a decision",
            id
        ));
    }
    Ok(())
}

/// One `decision:` line found in a YAML file, and the top-level key it
/// sits under (`overrides`, `waivers`, `links`, `sources`).
struct DecisionLine {
    index: usize,
    id: String,
    block: String,
    /// The text before `decision:` (indent, and `- ` on a list item's
    /// first key).
    lead: String,
}

fn decision_lines(text: &str) -> Vec<DecisionLine> {
    let re =
        regex::Regex::new(r#"^(\s*(?:-\s+)?)decision:\s*["']?(D-\d{4})["']?\s*(?:#.*)?$"#).unwrap();
    let top = regex::Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*):").unwrap();
    let mut block = String::new();
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if let Some(c) = top.captures(line) {
            block = c[1].to_string();
            continue;
        }
        if let Some(c) = re.captures(line) {
            out.push(DecisionLine {
                index,
                id: c[2].to_string(),
                block: block.clone(),
                lead: c[1].to_string(),
            });
        }
    }
    out
}

/// Every `decision` string value anywhere in a parsed document.
fn decision_values(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "decision" {
                    if let Some(s) = x.as_str() {
                        out.insert(s.to_string());
                    }
                }
                decision_values(x, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|x| decision_values(x, out)),
        _ => {}
    }
}

/// Rewrite the `decision:` lines of `text` that name an id in `gone`: a
/// line whose id has a context to carry in this block becomes `context:`,
/// any other is removed. Returns the new text, the entries each id's
/// context landed on, and a warning per removed reference that was not a
/// provenance record's own.
fn rewrite_references(
    text: &str,
    file: &str,
    gone: &BTreeMap<String, &'static str>,
    carry: &BTreeMap<String, (String, Option<String>)>,
) -> Result<(String, BTreeMap<String, usize>, Vec<String>), String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut replace: BTreeMap<usize, Option<String>> = BTreeMap::new();
    let mut landed: BTreeMap<String, usize> = BTreeMap::new();
    let mut warnings = Vec::new();
    for d in decision_lines(text) {
        let Some(fate) = gone.get(&d.id) else {
            continue;
        };
        let carried = carry.get(&d.id).filter(|(block, _)| block == &d.block);
        match carried {
            Some((_, Some(context))) => {
                let newline = if lines[d.index].ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                replace.insert(
                    d.index,
                    Some(format!(
                        "{}context: {}{}",
                        d.lead,
                        crate::yaml::scalar(context),
                        newline
                    )),
                );
                *landed.entry(d.id.clone()).or_default() += 1;
            }
            Some((_, None)) => {
                if d.lead.trim_start().starts_with('-') {
                    return Err(format!(
                        "{} line {}: `decision: {}` is a list item's first key; move it below the item's other keys, then run the split again",
                        file,
                        d.index + 1,
                        d.id
                    ));
                }
                replace.insert(d.index, None);
                *landed.entry(d.id.clone()).or_default() += 1;
            }
            None => {
                if d.lead.trim_start().starts_with('-') {
                    return Err(format!(
                        "{} line {}: `decision: {}` is a list item's first key; move it below the item's other keys, then run the split again",
                        file,
                        d.index + 1,
                        d.id
                    ));
                }
                replace.insert(d.index, None);
                warnings.push(format!(
                    "{} line {}: `decision: {}` named a record that is now {}; the reference is removed{}",
                    file,
                    d.index + 1,
                    d.id,
                    fate,
                    if d.block == "links" {
                        " — a stale link now needs a decision of its own to stay"
                    } else {
                        ""
                    }
                ));
            }
        }
    }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        match replace.get(&i) {
            Some(Some(new)) => out.push_str(new),
            Some(None) => {}
            None => out.push_str(line),
        }
    }
    Ok((out, landed, warnings))
}

/// `contract_version: 1` at column 0 becomes 2 (ADR 0113 §3).
pub fn bump_selection_version(text: &str) -> String {
    // The value may be quoted and followed by spacing and a comment, which
    // are kept: only the `1` itself becomes `2`.
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"^(contract_version:[ \t]*)(?:1|"1"|'1')([ \t]*(?:#.*)?)$"#).unwrap()
    });
    let mut done = false;
    text.split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches(['\n', '\r']);
            match re.captures(body).filter(|_| !done) {
                Some(c) => {
                    done = true;
                    format!("{}2{}{}", &c[1], &c[2], &line[body.len()..])
                }
                None => line.to_string(),
            }
        })
        .collect()
}

/// `lines` appended to memory.md's `## Tried and rejected` section, created
/// at the end of the file when absent.
pub fn append_tried_and_rejected(text: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return text.to_string();
    }
    let items: String = lines
        .iter()
        .map(|l| {
            let l = l.trim();
            if l.starts_with("- ") {
                format!("{}\n", l)
            } else {
                format!("- {}\n", l)
            }
        })
        .collect();
    let all: Vec<&str> = text.split_inclusive('\n').collect();
    if let Some(start) = all.iter().position(|l| l.trim() == TRIED_AND_REJECTED) {
        let mut end = all.len();
        for (i, l) in all.iter().enumerate().skip(start + 1) {
            if l.starts_with("# ") || l.starts_with("## ") {
                end = i;
                break;
            }
        }
        let mut insert = end;
        while insert > start + 1 && all[insert - 1].trim().is_empty() {
            insert -= 1;
        }
        let mut out = all[..insert].concat();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if insert == start + 1 {
            out.push('\n');
        }
        out.push_str(&items);
        out.push_str(&all[insert..].concat());
        return out;
    }
    let mut out = text.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
    }
    out.push_str(TRIED_AND_REJECTED);
    out.push_str("\n\n");
    out.push_str(&items);
    out
}

fn schema_check(value: &Value, schema: &str, file: &str) -> Result<(), String> {
    let schema_doc =
        crate::schemas::load(schema, None).ok_or_else(|| format!("cannot load {}", schema))?;
    let errors = crate::jsonschema::validate(value, &schema_doc);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{}: the split would write an invalid file: {}",
            file,
            errors.join("; ")
        ))
    }
}

/// Every record of the log that only a person can sort: `(id, title)`.
pub fn unsorted(decisions: &Value, sorted: Option<&Value>) -> Vec<(String, String)> {
    get_arr(decisions, "decisions")
        .into_iter()
        .flatten()
        .filter(|r| classify(r) == Class::Hand)
        .filter_map(|r| {
            let id = get_str(r, "id")?;
            let assigned = sorted
                .and_then(|s| get(s, id))
                .and_then(|e| get_str(e, "as"))
                .is_some();
            (!assigned).then(|| {
                (
                    id.to_string(),
                    get_str(r, "title").unwrap_or("").to_string(),
                )
            })
        })
        .collect()
}

/// Compute the split. Reads through custody; writes nothing.
pub fn plan(
    dir: &Path,
    decisions: &Value,
    sorted: Option<&Value>,
    schemas_dir: Option<&Path>,
) -> Result<Plan, Refusal> {
    let invalid = Refusal::Invalid;
    let records: Vec<Value> = get_arr(decisions, "decisions").cloned().unwrap_or_default();
    let ids: BTreeSet<String> = records
        .iter()
        .filter_map(|r| get_str(r, "id").map(str::to_string))
        .collect();

    // The --sorted file names only records that exist, with a known
    // assignment and keys; a mechanically sorted record may only gain its
    // alternative (`as: decision` on a kept record).
    if let Some(s) = sorted {
        for (id, entry) in s.as_object().into_iter().flatten() {
            if !ids.contains(id) {
                return Err(invalid(format!(
                    "--sorted names {}, which decisions.json does not record",
                    id
                )));
            }
            let Some(as_) = get_str(entry, "as") else {
                return Err(invalid(format!(
                    "--sorted {}: `as` is required: one of {}",
                    id,
                    AS.join(", ")
                )));
            };
            if !AS.contains(&as_) {
                return Err(invalid(format!(
                    "--sorted {}: `as: {}` is not one of {}",
                    id,
                    as_,
                    AS.join(", ")
                )));
            }
            if let Some(bad) = entry
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, _)| k)
                .find(|k| !SORTED_KEYS.contains(&k.as_str()))
            {
                return Err(invalid(format!(
                    "--sorted {}: unknown key {:?} (one of {})",
                    id,
                    bad,
                    SORTED_KEYS.join(", ")
                )));
            }
            let rec = records
                .iter()
                .find(|r| get_str(r, "id") == Some(id.as_str()))
                .unwrap();
            match classify(rec) {
                Class::Hand => {}
                Class::Kept if as_ == "decision" => {}
                other => {
                    return Err(invalid(format!(
                        "--sorted {}: the split sorts this record itself ({}); only a hand-sorted record, or a kept decision gaining its alternative, belongs in --sorted",
                        id,
                        other.label()
                    )))
                }
            }
        }
    }
    let missing = unsorted(decisions, sorted);
    if !missing.is_empty() {
        return Err(Refusal::Unsorted(missing));
    }

    let selection_text = crate::factory_io::read_to_string_optional(dir, SELECTION)
        .map_err(|e| invalid(e.to_string()))?;
    let lock_text = crate::factory_io::read_to_string_optional(dir, crate::sources::SOURCES_LOCK)
        .map_err(|e| invalid(e.to_string()))?;
    let memory_text = crate::factory_io::read_to_string_optional(dir, MEMORY)
        .map_err(|e| invalid(e.to_string()))?;
    let selection_lines = selection_text
        .as_deref()
        .map(decision_lines)
        .unwrap_or_default();
    let lock_lines = lock_text.as_deref().map(decision_lines).unwrap_or_default();
    let cited = |id: &str, kind: Provenance| -> bool {
        match kind {
            Provenance::Override => selection_lines
                .iter()
                .any(|d| d.id == id && d.block == "overrides"),
            Provenance::Waiver => selection_lines
                .iter()
                .any(|d| d.id == id && d.block == "waivers"),
            Provenance::Patch => lock_lines.iter().any(|d| d.id == id),
        }
    };

    let mut findings = crate::findings::load(dir, schemas_dir).map_err(invalid)?;
    let mut kept: Vec<Value> = Vec::new();
    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut memory_lines: Vec<String> = Vec::new();
    // id -> what it became, for every record that is no longer a decision.
    let mut gone: BTreeMap<String, &'static str> = BTreeMap::new();
    // id -> (block, context to carry), for a cited provenance record.
    let mut carry_selection: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    let mut carry_lock: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();

    for rec in &records {
        let id = get_str(rec, "id").unwrap_or("").to_string();
        let title = get_str(rec, "title").unwrap_or("").to_string();
        let date = get_str(rec, "date").unwrap_or("unknown").to_string();
        let class = classify(rec);
        let mut outcome = Outcome {
            id: id.clone(),
            title: title.clone(),
            class,
            outcome: "kept",
            finding: None,
            detail: None,
        };
        let entry = sorted.and_then(|s| get(s, &id));
        let mut add_finding = |new: NewFinding, outcome: &mut Outcome| -> Result<(), Refusal> {
            let fid = crate::findings::add_numbered(&mut findings, new)
                .map_err(|e| invalid(format!("{}: {}", id, e)))?;
            outcome.outcome = "finding";
            outcome.finding = Some(fid);
            Ok(())
        };
        match class {
            Class::Kept => match entry {
                Some(e) => {
                    kept.push(as_decision(rec, e, &id).map_err(invalid)?);
                    outcome.outcome = "decision";
                }
                None => kept.push(rec.clone()),
            },
            Class::Provenance(kind) => {
                if cited(&id, kind) {
                    let context = get_str(rec, "context")
                        .map(str::trim)
                        .filter(|c| !c.is_empty() && !kind.is_template(c))
                        .map(str::to_string);
                    outcome.outcome = "moved";
                    // Completed with the entry count once the rewrite ran.
                    outcome.detail = Some(match &context {
                        Some(_) => format!("context carried onto {{n}} {}", kind.entry()),
                        None => format!(
                            "template context dropped; {{n}} {} keep their reason",
                            kind.entry()
                        ),
                    });
                    let block = match kind {
                        Provenance::Override => "overrides",
                        Provenance::Waiver => "waivers",
                        Provenance::Patch => "sources",
                    };
                    let map = if kind == Provenance::Patch {
                        &mut carry_lock
                    } else {
                        &mut carry_selection
                    };
                    map.insert(id.clone(), (block.to_string(), context));
                    gone.insert(id.clone(), "moved onto its entry");
                } else {
                    let affects = strings_of(get(rec, "affects"));
                    let (affects, evidence) = if kind == Provenance::Override {
                        (affects, Vec::new())
                    } else {
                        (Vec::new(), affects)
                    };
                    add_finding(
                        NewFinding {
                            title: title.clone(),
                            date: date.clone(),
                            body: prose(rec),
                            source: "codify".into(),
                            affects,
                            evidence,
                            ..Default::default()
                        },
                        &mut outcome,
                    )?;
                    outcome.detail = Some(format!("no {} cites it", kind.entry()));
                    gone.insert(id.clone(), "a finding");
                }
            }
            Class::Sources => {
                add_finding(
                    NewFinding {
                        title: title.clone(),
                        date: date.clone(),
                        body: prose(rec),
                        source: "sources".into(),
                        evidence: strings_of(get(rec, "affects")),
                        ..Default::default()
                    },
                    &mut outcome,
                )?;
                gone.insert(id.clone(), "a finding");
            }
            Class::Hand => {
                let e = entry.expect("unsorted() refused every unassigned record");
                let as_ = get_str(e, "as").unwrap_or("");
                match as_ {
                    "decision" => {
                        kept.push(as_decision(rec, e, &id).map_err(invalid)?);
                        outcome.outcome = "decision";
                    }
                    "finding" => {
                        refuse_payload(rec, &id, "finding").map_err(invalid)?;
                        let affects = match get(e, "affects") {
                            Some(a) => json::strings(Some(a)),
                            None => strings_of(get(rec, "affects")),
                        };
                        add_finding(
                            NewFinding {
                                title: get_str(e, "title").unwrap_or(&title).to_string(),
                                date: date.clone(),
                                body: get_str(e, "body")
                                    .map(str::to_string)
                                    .unwrap_or_else(|| prose(rec)),
                                cites: get_str(e, "cites").map(str::to_string),
                                source: "agent".into(),
                                affects,
                                omits: omits_of(rec),
                                evidence: json::strings(get(e, "evidence")),
                                related: json::strings(get(e, "related")),
                            },
                            &mut outcome,
                        )?;
                        gone.insert(id.clone(), "a finding");
                    }
                    "memory" => {
                        refuse_payload(rec, &id, "memory").map_err(invalid)?;
                        let Some(line) = get_str(e, "line").filter(|l| !l.trim().is_empty()) else {
                            return Err(invalid(format!(
                                "--sorted {}: `as: memory` needs `line`, the text for memory.md's {}",
                                id, TRIED_AND_REJECTED
                            )));
                        };
                        memory_lines.push(line.to_string());
                        outcome.outcome = "memory";
                        gone.insert(id.clone(), "a memory.md line");
                    }
                    _ => {
                        refuse_payload(rec, &id, "drop").map_err(invalid)?;
                        outcome.outcome = "dropped";
                        gone.insert(id.clone(), "dropped");
                    }
                }
            }
        }
        outcomes.push(outcome);
    }

    let mut warnings = Vec::new();
    let mut decisions_out = decisions.clone();
    json::set(&mut decisions_out, "decisions", Value::Array(kept));
    // A finding's `related` may only name what still exists.
    let kept_ids: BTreeSet<String> = get_arr(&decisions_out, "decisions")
        .into_iter()
        .flatten()
        .filter_map(|r| get_str(r, "id").map(str::to_string))
        .collect();
    for f in get_arr(&findings, "findings").into_iter().flatten() {
        for r in json::strings(get(f, "related")) {
            if r.starts_with("D-") && !kept_ids.contains(&r) {
                warnings.push(format!(
                    "{} relates to {}, which is no longer a decision",
                    get_str(f, "id").unwrap_or(""),
                    r
                ));
            }
        }
    }

    // Before ADR 0113 any resolved decision a `links:` entry named kept the
    // link however the rules moved; now only a resolved `keep` does. Say so
    // for each included entry whose decision chooses neither keep nor drop,
    // so the exemption does not lapse unseen after the upgrade.
    if let Some(selection) = selection_text
        .as_deref()
        .and_then(|t| crate::yaml::parse(t).ok())
    {
        for link in crate::reconcile::read_links(&selection)
            .iter()
            .filter(|l| l.include)
        {
            if let crate::reconcile::LinkDecision::Unanswered(id) =
                crate::reconcile::link_decision(link, Some(&decisions_out))
            {
                warnings.push(format!(
                    "selection.yaml links entry {}: decision: {} chooses neither keep nor drop, so it no longer keeps the link if the rules stop backing it; give it the choices (`--sorted` `{}: {{as: decision, choices: [keep, drop, …], chosen: [keep]}}`), or reopen it (`graphos-factory-core decisions reopen . --id {}`) and answer `graphos-factory-core decisions resolve . --id {} --chosen keep` or `--chosen drop`",
                    link.key(),
                    id,
                    id,
                    id,
                    id
                ));
            }
        }
    }

    let mut selection_new: Option<String> = None;
    if let Some(text) = &selection_text {
        let (rewritten, landed, w) =
            rewrite_references(text, "selection.yaml", &gone, &carry_selection).map_err(invalid)?;
        warnings.extend(w);
        let bumped = bump_selection_version(&rewritten);
        let parsed = crate::yaml::parse(&bumped).map_err(|e| {
            invalid(format!(
                "selection.yaml: the rewritten file does not parse ({}); nothing was written",
                e
            ))
        })?;
        let mut left = BTreeSet::new();
        decision_values(&parsed, &mut left);
        if let Some(id) = left.iter().find(|id| gone.contains_key(*id)) {
            return Err(invalid(format!(
                "selection.yaml still names {} in a `decision:` the line-level rewrite cannot reach (a flow-style entry?); edit it to block style, then run the split again",
                id
            )));
        }
        schema_check(&parsed, "selection.schema.json", "selection.yaml").map_err(invalid)?;
        for o in outcomes.iter_mut().filter(|o| o.outcome == "moved") {
            if let Some(n) = landed.get(&o.id) {
                o.detail = o.detail.as_ref().map(|d| {
                    let d = d.replace("{n}", &n.to_string());
                    if *n == 1 {
                        d.replace(" keep their ", " keeps its ")
                    } else {
                        d.replace("override", "overrides")
                            .replace("waiver", "waivers")
                            .replace("patch", "patches")
                    }
                });
            }
        }
        if bumped != *text {
            selection_new = Some(bumped);
        }
    }
    let mut lock_new: Option<String> = None;
    if let Some(text) = &lock_text {
        let (rewritten, landed, w) =
            rewrite_references(text, "sources.lock.yaml", &gone, &carry_lock).map_err(invalid)?;
        warnings.extend(w);
        let parsed = crate::yaml::parse(&rewritten).map_err(|e| {
            invalid(format!(
                "sources.lock.yaml: the rewritten file does not parse ({}); nothing was written",
                e
            ))
        })?;
        let mut left = BTreeSet::new();
        decision_values(&parsed, &mut left);
        if let Some(id) = left.iter().find(|id| gone.contains_key(*id)) {
            return Err(invalid(format!(
                "sources.lock.yaml still names {} in a `decision:` the line-level rewrite cannot reach; edit it to block style, then run the split again",
                id
            )));
        }
        schema_check(&parsed, "sources-lock.schema.json", "sources.lock.yaml").map_err(invalid)?;
        for o in outcomes.iter_mut().filter(|o| o.outcome == "moved") {
            if let Some(n) = landed.get(&o.id) {
                o.detail = o.detail.as_ref().map(|d| {
                    let d = d.replace("{n}", &n.to_string());
                    if *n == 1 {
                        d.replace(" keep their ", " keeps its ")
                    } else {
                        d.replace("override", "overrides")
                            .replace("waiver", "waivers")
                            .replace("patch", "patches")
                    }
                });
            }
        }
        if rewritten != *text {
            lock_new = Some(rewritten);
        }
    }
    let memory_new = (!memory_lines.is_empty())
        .then(|| append_tried_and_rejected(memory_text.as_deref().unwrap_or(""), &memory_lines));

    schema_check(
        &decisions_out,
        "decisions.schema.json",
        crate::decisions::FILE,
    )
    .map_err(invalid)?;
    schema_check(&findings, "findings.schema.json", crate::findings::FILE).map_err(invalid)?;

    Ok(Plan {
        decisions: decisions_out,
        findings,
        selection: selection_new,
        sources_lock: lock_new,
        memory: memory_new,
        outcomes,
        warnings,
    })
}

/// Write a computed plan through custody, in [`Plan::writes`] order. On a
/// failure, the error names what was already written.
pub fn write(dir: &Path, plan: &Plan, schemas_dir: Option<&Path>) -> Result<(), String> {
    let mut written: Vec<&str> = Vec::new();
    let fail = |written: &[&str], e: String| -> String {
        if written.is_empty() {
            e
        } else {
            format!(
                "{} (already written: {}; `git checkout -- .factory` restores the workspace)",
                e,
                written.join(", ")
            )
        }
    };
    crate::findings::save_single(dir, &plan.findings, schemas_dir)
        .map_err(|e| fail(&written, e))?;
    written.push(crate::findings::FILE);
    crate::decisions::save_single(dir, &plan.decisions, schemas_dir)
        .map_err(|e| fail(&written, e))?;
    written.push(crate::decisions::FILE);
    if let Some(t) = &plan.selection {
        crate::factory_io::write_in_place(dir, SELECTION, t.as_bytes())
            .map_err(|e| fail(&written, e.to_string()))?;
        written.push(SELECTION);
    }
    if let Some(t) = &plan.sources_lock {
        crate::factory_io::write_in_place(dir, crate::sources::SOURCES_LOCK, t.as_bytes())
            .map_err(|e| fail(&written, e.to_string()))?;
        written.push(crate::sources::SOURCES_LOCK);
    }
    if let Some(t) = &plan.memory {
        crate::factory_io::create_parent_dir(dir, MEMORY)
            .map_err(|e| fail(&written, e.to_string()))?;
        crate::factory_io::write_in_place(dir, MEMORY, t.as_bytes())
            .map_err(|e| fail(&written, e.to_string()))?;
    }
    Ok(())
}

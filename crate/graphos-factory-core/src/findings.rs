//! The workspace's findings: facts the agent established that an
//! instrument must read, or that the next session needs, and that a
//! reference, an ADR or the wire settled, so there is no alternative to
//! choose (ADR 0113). A decision carries its alternative and lives in
//! `decisions.json`; a finding carries the fact, the rule that settles it
//! (`cites`) and the payload an instrument reads (`omits`, `affects`).
//!
//! `graphos-factory-core findings` is the only writer, and every read and write
//! goes through custody (`crate::factory_io`, ADR 0025). Every mutation
//! validates against `findings.schema.json` before it is written.
//!
//! The readers of `omits` and `affects` — `source-coverage`, `stale-omit`,
//! lint's description waivers, `closed-enum-as-string`, `sparse-fieldsets`,
//! the write-body proof — read [`load_union`]: the resolved decisions and
//! the current findings, as one decision-shaped document. The kept-link
//! rule reads decisions only (ADR 0113 §4).

use crate::decisions::Omit;
use crate::json;
use serde_json::Value;
use std::path::Path;

pub const FILE: &str = ".factory/findings.json";

/// Who wrote a finding: the agent (`findings add`), `codify --expressed
/// --context`, or `sources refresh` / `sources pin --force`.
pub const SOURCES: &[&str] = &["agent", "codify", "sources"];

/// A brand-new finding to append; its status is `current`.
#[derive(Default)]
pub struct NewFinding {
    pub title: String,
    pub date: String,
    pub body: String,
    pub cites: Option<String>,
    /// One of [`SOURCES`].
    pub source: String,
    pub affects: Vec<String>,
    pub omits: Vec<Omit>,
    pub evidence: Vec<String>,
    pub related: Vec<String>,
}

/// Why a finding verb refused. `code` is the stable slug a `--json` caller
/// branches on.
#[derive(Debug)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

/// Whether `direction` and `reason` are a spelling a finding may carry: a
/// wire omit (`response`, `request`) is `consumed`, a behaviour waiver is
/// `not-applicable`. `editorial` is always refused: "expose it" is the
/// alternative, so an editorial omit lives only on a decision (ADR 0113 §1).
pub fn check_omit(direction: &str, reason: &str) -> Result<(), String> {
    if reason == "editorial" {
        return Err(
            "an editorial omit (deliberately not exposed) is a choice, and lives only on a decision: record it with `decisions add --question … --choice … --omit …`"
                .to_string(),
        );
    }
    crate::decisions::check_omit_vocabulary(direction, reason)
}

/// An empty log.
pub fn empty() -> Value {
    json::object(vec![
        ("contract_version", Value::from(1)),
        ("findings", Value::Array(vec![])),
    ])
}

/// Read the log, or an empty one when there is none: `findings.json`'s
/// findings, then `.factory/findings/`'s (ADR 0118). Present but invalid is an error, never a
/// silent reset.
pub fn load(dir: &Path, schemas_dir: Option<&Path>) -> Result<Value, String> {
    Ok(load_present(dir, schemas_dir)?.unwrap_or_else(empty))
}

/// Read the log, or `None` when there is neither a `findings.json` nor a
/// `.factory/findings/`. Every read goes through custody, so a symlinked file is
/// refused rather than read through (ADR 0025).
pub fn load_present(dir: &Path, schemas_dir: Option<&Path>) -> Result<Option<Value>, String> {
    crate::record_log::read(dir, &crate::record_log::FINDINGS, schemas_dir)
}

/// Validate then write: each finding `findings.json` holds goes back into it,
/// every other one to its own file under `.factory/findings/` (ADR 0118).
/// Refuses a document the schema rejects or that holds an id twice.
pub fn save(dir: &Path, doc: &Value, schemas_dir: Option<&Path>) -> Result<(), String> {
    crate::record_log::write(dir, &crate::record_log::FINDINGS, doc, schemas_dir)
}

/// The id a new finding gets: `F-` and six random base36 characters no
/// finding holds (ADR 0118).
pub fn new_id(doc: &Value) -> String {
    let taken = crate::record_log::ids(doc, "findings");
    crate::record_log::new_id("F-", |id| taken.contains(id))
}

/// Write the whole log as `findings.json`, as before ADR 0118, for
/// `migrate --split`.
pub fn save_single(dir: &Path, doc: &Value, schemas_dir: Option<&Path>) -> Result<(), String> {
    crate::record_log::write_single(dir, &crate::record_log::FINDINGS, doc, schemas_dir)
}

/// The next `F-nnnn` id after the highest one already recorded: only
/// `migrate --split` numbers a finding now.
pub fn next_id(doc: &Value) -> String {
    let max = json::get_arr(doc, "findings")
        .into_iter()
        .flatten()
        .filter_map(|f| json::get_str(f, "id"))
        .filter_map(|id| id.strip_prefix("F-"))
        .filter_map(|n| n.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("F-{:04}", max + 1)
}

/// The finding with this id, if any.
pub fn find<'a>(doc: &'a Value, id: &str) -> Option<&'a Value> {
    json::get_arr(doc, "findings")?
        .iter()
        .find(|f| json::get_str(f, "id") == Some(id))
}

fn omit_value(o: &Omit) -> Value {
    json::object(vec![
        ("operation", Value::from(o.operation.as_str())),
        ("direction", Value::from(o.direction.as_str())),
        ("path", Value::from(o.path.as_str())),
        ("reason", Value::from(o.reason.as_str())),
    ])
}

/// The record a [`NewFinding`] becomes under `id`.
pub fn record(id: &str, new: NewFinding) -> Result<Value, String> {
    if new.title.trim().is_empty() {
        return Err("a finding needs a title".into());
    }
    if new.body.trim().is_empty() {
        return Err("a finding needs a body: the fact, in prose".into());
    }
    if !SOURCES.contains(&new.source.as_str()) {
        return Err(format!(
            "source must be one of {}, got {:?}",
            SOURCES.join(", "),
            new.source
        ));
    }
    for o in &new.omits {
        check_omit(&o.direction, &o.reason)?;
    }
    let mut rec = json::obj();
    rec.insert("id".into(), Value::from(id));
    rec.insert("title".into(), Value::from(new.title));
    rec.insert("date".into(), Value::from(new.date));
    rec.insert("status".into(), Value::from("current"));
    rec.insert("body".into(), Value::from(new.body));
    if let Some(c) = new.cites.filter(|c| !c.trim().is_empty()) {
        rec.insert("cites".into(), Value::from(c));
    }
    rec.insert("source".into(), Value::from(new.source));
    let mut affects: Vec<String> = Vec::new();
    for a in new.affects {
        if !affects.contains(&a) {
            affects.push(a);
        }
    }
    if !affects.is_empty() {
        rec.insert("affects".into(), Value::from(affects));
    }
    if !new.omits.is_empty() {
        rec.insert(
            "omits".into(),
            Value::Array(new.omits.iter().map(omit_value).collect()),
        );
    }
    if !new.evidence.is_empty() {
        rec.insert("evidence".into(), Value::from(new.evidence));
    }
    let mut related: Vec<String> = Vec::new();
    for r in new.related {
        if !related.contains(&r) {
            related.push(r);
        }
    }
    if !related.is_empty() {
        rec.insert("related".into(), Value::from(related));
    }
    Ok(Value::Object(rec))
}

/// Append a new finding under a random id and return the id (ADR 0118).
/// In-memory only; the caller persists with [`save`], which gives it its own
/// file under `.factory/findings/`.
pub fn add(doc: &mut Value, new: NewFinding) -> Result<String, String> {
    let id = new_id(doc);
    add_with_id(doc, &id, new)
}

/// [`add`], under an id the caller already drew with [`new_id`] (so a
/// message can name it before the finding is built).
pub fn add_with_id(doc: &mut Value, id: &str, new: NewFinding) -> Result<String, String> {
    let mut rec = record(id, new)?;
    let slug = crate::record_log::slugify(json::get_str(&rec, "title").unwrap_or(""));
    if !slug.is_empty() {
        rec = crate::record_log::with_slug(&rec, &slug);
    }
    match doc.get_mut("findings") {
        Some(Value::Array(items)) => items.push(rec),
        _ => return Err("findings is not an array".into()),
    }
    Ok(id.to_string())
}

/// Append a finding under the next number, for `migrate --split`, which
/// writes the single file as before ADR 0118 ([`save_single`]).
pub fn add_numbered(doc: &mut Value, new: NewFinding) -> Result<String, String> {
    let id = next_id(doc);
    let rec = record(&id, new)?;
    match doc.get_mut("findings") {
        Some(Value::Array(items)) => items.push(rec),
        _ => return Err("findings is not an array".into()),
    }
    Ok(id)
}

/// Mark a `current` finding `superseded`: a later finding, or a change to
/// the service, replaced it. Everything on the record is kept; its `omits`
/// and `affects` stop counting. In-memory only.
pub fn supersede(doc: &mut Value, id: &str) -> Result<(), Refusal> {
    let rec = doc
        .get_mut("findings")
        .and_then(Value::as_array_mut)
        .and_then(|items| {
            items
                .iter_mut()
                .find(|f| json::get_str(f, "id") == Some(id))
        })
        .ok_or_else(|| Refusal {
            code: "unknown-finding",
            message: format!("no finding {}", id),
        })?;
    if json::get_str(rec, "status") == Some("superseded") {
        return Err(Refusal {
            code: "already-superseded",
            message: format!("{} is already superseded", id),
        });
    }
    json::set(rec, "status", Value::from("superseded"));
    Ok(())
}

/// A finding as the decision-shaped record the `omits`/`affects` readers
/// take: `current` reads as `resolved`, `superseded` stays `superseded`,
/// `body` stands where a decision's text stands (`resolution.decision`) and
/// `cites` where its `context` does. Never written anywhere.
pub fn as_decision_record(finding: &Value) -> Value {
    let status = match json::get_str(finding, "status") {
        Some("current") => "resolved",
        Some(other) => other,
        None => "superseded",
    };
    let mut rec = json::obj();
    for k in ["id", "title", "date"] {
        if let Some(v) = json::get(finding, k) {
            rec.insert(k.into(), v.clone());
        }
    }
    rec.insert("status".into(), Value::from(status));
    if let Some(c) = json::get(finding, "cites") {
        rec.insert("context".into(), c.clone());
    }
    for k in ["affects", "omits"] {
        if let Some(v) = json::get(finding, k) {
            rec.insert(k.into(), v.clone());
        }
    }
    let mut res = json::obj();
    if let Some(b) = json::get(finding, "body") {
        res.insert("decision".into(), b.clone());
    }
    rec.insert("resolution".into(), Value::Object(res));
    Value::Object(rec)
}

/// The decisions document with every finding appended as a decision-shaped
/// record ([`as_decision_record`]): the one document the `omits` and
/// `affects` readers take (ADR 0113 §2). A reader that filters on
/// `status == resolved` then counts a resolved decision and a current
/// finding alike, and neither an open or superseded decision nor a
/// superseded finding.
pub fn union(decisions: &Value, findings: &Value) -> Value {
    let mut out = decisions.clone();
    let extra: Vec<Value> = json::get_arr(findings, "findings")
        .into_iter()
        .flatten()
        .map(as_decision_record)
        .collect();
    match out.get_mut("decisions") {
        Some(Value::Array(items)) => items.extend(extra),
        _ => json::set(&mut out, "decisions", Value::Array(extra)),
    }
    out
}

/// [`union`] of the workspace's two files, each read through custody. An
/// absent file is empty. An invalid decisions.json is an error, as for
/// `crate::decisions::load`; an invalid or unreadable findings.json is not:
/// the union falls back to the decisions alone, so one corrupt findings file
/// never blanks every resolved decision's omits, affects and waivers. Lint
/// reports the findings file itself ([`load_union_reporting`]).
pub fn load_union(dir: &Path, schemas_dir: Option<&Path>) -> Result<Value, String> {
    load_union_reporting(dir, schemas_dir).0
}

/// [`load_union`], and why findings.json did not load when it did not (the
/// union then holds the decisions alone).
pub fn load_union_reporting(
    dir: &Path,
    schemas_dir: Option<&Path>,
) -> (Result<Value, String>, Option<String>) {
    let decisions = crate::decisions::load(dir, schemas_dir);
    union_of(
        decisions.as_ref().map_err(String::clone),
        load(dir, schemas_dir),
    )
}

/// [`union`] of an already-loaded decisions result and findings result,
/// with the same fallback as [`load_union`]: the findings' error is
/// returned beside the union, never in place of it.
pub fn union_of(
    decisions: Result<&Value, String>,
    findings: Result<Value, String>,
) -> (Result<Value, String>, Option<String>) {
    let (findings, error) = match findings {
        Ok(f) => (f, None),
        Err(e) => (empty(), Some(e)),
    };
    (decisions.map(|d| union(d, &findings)), error)
}

/// Where the finding with this id lives: `findings.json` for a numbered
/// one, else the record directory (ADR 0118).
pub fn file_of(id: &str) -> &'static str {
    if crate::record_log::is_random(id) {
        crate::record_log::FINDINGS.dir
    } else {
        FILE
    }
}

/// Whether an id names a finding (`F-nnnn`) rather than a decision.
pub fn is_finding_id(id: &str) -> bool {
    id.starts_with("F-")
}

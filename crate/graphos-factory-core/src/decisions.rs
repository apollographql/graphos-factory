//! The workspace decision log: judgement calls already made and open questions
//! still awaiting the user, as individually tracked records. Replaces the
//! free-form `decisions.md` (ADR 0026).
//!
//! The agent never hand-edits this file; `graphos-factory-core decisions` is the
//! only writer, so a headless agent and a desktop UI record resolutions through
//! the same path. Every mutation validates against `decisions.schema.json`
//! before it is written, so an invalid document is never produced.

use crate::json;
use serde_json::Value;
use std::path::Path;

pub const FILE: &str = ".factory/decisions.json";

/// One agent-suggested option on an open decision.
pub struct Choice {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

/// One wire path a decision has judged out of scope, recorded so the
/// `source-coverage` classifier reads a machine-checkable reason instead of
/// re-deriving it from prose. `direction`/`reason` are the fixed vocabulary
/// `decisions.schema.json` enforces (ADR 0036).
pub struct Omit {
    pub operation: String,
    pub direction: String,
    pub path: String,
    pub reason: String,
}

/// One credential-shaped response field the engineer has reviewed and
/// recorded a disposition for, so `lint`'s `secret-field-exposed` rule (ADR
/// 0078) stops warning about it. `type`/`field` are the rendered SDL names
/// (`type_name` here to dodge the Rust keyword); `disposition` says whether
/// exposing it was a deliberate choice or whether it is meant to come out.
pub struct SecretField {
    pub type_name: String,
    pub field: String,
    pub disposition: String,
    pub reason: String,
}

/// Why one response field is left typed as the workspace's own JSON scalar,
/// keyed the same way `crate::json_accounting` keys a field: the GraphQL
/// type name and the field name. Read by `crate::json_accounting`, only
/// from a resolved record (ADR 0073; per-field reasons live here, never in
/// `selection.yaml` or a schema doc comment — Adam's rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonReason {
    pub type_name: String,
    pub field: String,
    /// One of `crate::json_accounting::REASONS`.
    pub reason: String,
}

/// What an operation's connector does with one nullable argument passed as
/// an explicit `null`: send the key as JSON `null`, or leave it out. Read by
/// `crate::request_serialization`, only from a resolved record (ADR 0079).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullHandling {
    pub operation: String,
    pub argument: String,
    /// `send_null` or `omit`.
    pub behavior: String,
}

/// How an open decision was settled. Recorded by the user (a button or typed
/// answer) or by the agent codifying a judgement call it already made.
#[derive(Default)]
pub struct Resolution {
    pub chosen: Vec<String>,
    pub note: Option<String>,
    pub decision: Option<String>,
    pub by: Option<String>,
    pub at: Option<String>,
}

/// A brand-new record to append. Its `status` is derived: a decision carrying a
/// resolution is `resolved`, otherwise `open`.
#[derive(Default)]
pub struct NewDecision {
    pub title: String,
    pub date: String,
    pub phase: Option<String>,
    pub question: Option<String>,
    pub context: Option<String>,
    pub requested_by: Option<String>,
    pub multiple: bool,
    pub affects: Vec<String>,
    pub choices: Vec<Choice>,
    pub resolution: Option<Resolution>,
    pub omits: Vec<Omit>,
    pub secret_fields: Vec<SecretField>,
    pub json_reasons: Vec<JsonReason>,
    pub null_handling: Vec<NullHandling>,
}

/// Whether `direction` and `reason` are a spelling `decisions.schema.json`
/// accepts for an omit (ADR 0036, 0095). `response` and `request` waive a
/// wire path, for `editorial` or `consumed`; `behaviour` waives an omission
/// sentence the source states and an argument's doc comment leaves out, for
/// `editorial` (chosen not to say it) or `not-applicable` (the sentence is
/// not about omitting the argument).
pub fn check_omit_vocabulary(direction: &str, reason: &str) -> Result<(), String> {
    let (reasons, name): (&[&str], &str) = match direction {
        "response" | "request" => (&["editorial", "consumed"], "editorial or consumed"),
        "behaviour" => (
            &["editorial", "not-applicable"],
            "editorial or not-applicable",
        ),
        other => {
            return Err(format!(
                "direction must be response, request or behaviour, got {:?}",
                other
            ))
        }
    };
    if reasons.contains(&reason) {
        Ok(())
    } else {
        Err(format!(
            "reason for a {} omit must be {}, got {:?}",
            direction, name, reason
        ))
    }
}

/// An empty log, stamped with the current contract version.
pub fn empty() -> Value {
    json::object(vec![
        ("contract_version", Value::from(1)),
        ("decisions", Value::Array(vec![])),
    ])
}

fn schema_errors(value: &Value, schemas_dir: Option<&Path>) -> Vec<String> {
    match crate::schemas::load("decisions.schema.json", schemas_dir) {
        Some(schema) => crate::jsonschema::validate(value, &schema),
        None => vec!["cannot load decisions.schema.json".into()],
    }
}

/// Read the log, or an empty one when the file is absent. A present-but-invalid
/// file is an error rather than a silent reset — the agent must not lose a
/// recorded decision to a bad edit.
pub fn load(dir: &Path, schemas_dir: Option<&Path>) -> Result<Value, String> {
    Ok(load_present(dir, schemas_dir)?.unwrap_or_else(empty))
}

/// Read the log, or `None` when the file is absent — for a verb that acts
/// on a recorded decision and so has nothing to do without one. Present
/// but unreadable or invalid is an error, as for `load`.
pub fn load_present(dir: &Path, schemas_dir: Option<&Path>) -> Result<Option<Value>, String> {
    // `.factory/` — custody's, so a symlinked log is refused rather than
    // read through, on every verb that records a decision (ADR 0025).
    let text = match crate::factory_io::read_to_string_optional(dir, FILE)? {
        Some(text) => text,
        None => return Ok(None),
    };
    let value = json::parse(&text)?;
    let errors = schema_errors(&value, schemas_dir);
    if !errors.is_empty() {
        return Err(format!("{}: {}", FILE, errors.join("; ")));
    }
    Ok(Some(value))
}

/// Validate then write. Refuses to write a document the schema rejects.
pub fn save(dir: &Path, doc: &Value, schemas_dir: Option<&Path>) -> Result<(), String> {
    let errors = schema_errors(doc, schemas_dir);
    if !errors.is_empty() {
        return Err(format!(
            "{}: refusing to write an invalid document: {}",
            FILE,
            errors.join("; ")
        ));
    }
    // Written in place through custody, never renamed over: a symlinked log
    // is refused, not silently replaced (ADR 0025).
    crate::factory_io::create_parent_dir(dir, FILE)?;
    crate::factory_io::write_in_place(dir, FILE, json::pretty(doc).as_bytes()).map_err(String::from)
}

/// The next `D-nnnn` id after the highest one already recorded.
pub fn next_id(doc: &Value) -> String {
    let max = json::get_arr(doc, "decisions")
        .map(|items| {
            items
                .iter()
                .filter_map(|d| json::get_str(d, "id"))
                .filter_map(|id| id.strip_prefix("D-"))
                .filter_map(|n| n.parse::<u32>().ok())
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0);
    format!("D-{:04}", max + 1)
}

fn index_of(doc: &Value, id: &str) -> Option<usize> {
    json::get_arr(doc, "decisions")?
        .iter()
        .position(|d| json::get_str(d, "id") == Some(id))
}

/// The record with this id, if any. Lets a caller check that a cited decision
/// is actually recorded.
pub fn find<'a>(doc: &'a Value, id: &str) -> Option<&'a Value> {
    json::get_arr(doc, "decisions")?
        .iter()
        .find(|d| json::get_str(d, "id") == Some(id))
}

fn strip_trailing_dot(s: &str) -> String {
    s.trim().trim_end_matches('.').trim().to_string()
}

/// Parse a legacy `decisions.md` — the `## D-nnnn · date · title` blocks that
/// `codify`/`sources` and the pilots wrote — into resolved records, preserving
/// the `D-nnnn` ids. Line-oriented and best-effort: each field is one line, as
/// the generator wrote them (`Context:` → context, `Decision:` →
/// resolution.decision, `Requested by:` → requested_by, `Affects:` → affects,
/// `Alternatives:`/`Evidence:` and anything unrecognized → resolution.note).
/// A fenced block whose first line is exactly `Omits:` is parsed as YAML
/// into structured `omits` records (`decisions.schema.json`, ADR 0036)
/// instead of being carried as prose; any other fenced block is kept as
/// opaque text, same as before. Returns `(records, warnings)` on success;
/// the caller wraps the records in a document and saves. `Err` names the
/// decision id and what about its `Omits:` block could not be parsed — a
/// block the tool cannot express structurally is refused, never silently
/// demoted to prose. This is the one-way importer that lets an existing
/// workspace upgrade without losing its history or colliding ids.
pub fn parse_markdown(text: &str) -> Result<(Vec<Value>, Vec<String>), String> {
    let header = regex::Regex::new(r"^##\s+(D-\d{4})(?:\s+·\s+(.*))?\s*$").unwrap();
    let known = [
        "Context",
        "Decision",
        "Requested by",
        "Affects",
        "Alternatives",
        "Evidence",
    ];
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let Some(caps) = header.captures(lines[i]) else {
            i += 1;
            continue;
        };
        let id = caps.get(1).unwrap().as_str().to_string();
        let rest = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        let (mut date, mut title) = match rest.split_once(" · ") {
            Some((d, t)) => (d.trim().to_string(), t.trim().to_string()),
            None => (String::new(), rest.to_string()),
        };

        let mut context = String::new();
        let mut decision = String::new();
        let mut requested_by = String::new();
        let mut affects_raw = String::new();
        let mut notes: Vec<String> = Vec::new();
        let mut leftover: Vec<String> = Vec::new();
        let mut omits: Vec<Omit> = Vec::new();
        let mut j = i + 1;
        while j < lines.len() && !header.is_match(lines[j]) {
            let line = lines[j].trim();
            // An opening fence may carry an info string (```yaml, ```json);
            // only a bare ``` closes one. Matching the opener exactly used to
            // miss ```yaml, take its closing ``` as an opener, and swallow
            // every decision up to the next fence into one note.
            // Markdown fences: ``` or ~~~, an opener may carry an info
            // string, and only a bare run of the same character closes it.
            let fence = ["```", "~~~"].into_iter().find(|f| line.starts_with(f));
            if let Some(fence) = fence {
                let fence_start = j;
                j += 1;
                let mut body: Vec<&str> = Vec::new();
                while j < lines.len() && lines[j].trim() != fence {
                    // A decision header inside a fence means the fences do
                    // not balance: refuse rather than drop that decision.
                    if header.is_match(lines[j]) {
                        return Err(format!(
                            "{}: the fenced block opened at line {} of decisions.md runs into the decision header at line {}; the fences do not balance, so nothing is migrated",
                            id,
                            fence_start + 1,
                            j + 1
                        ));
                    }
                    body.push(lines[j]);
                    j += 1;
                }
                if j >= lines.len() {
                    return Err(format!(
                        "{}: a fenced block opened at line {} of decisions.md is never closed",
                        id,
                        fence_start + 1
                    ));
                }
                j += 1; // past the closing fence
                if body.first().map(|l| l.trim()) == Some("Omits:") {
                    omits.extend(parse_omits_fence(&id, &body.join("\n"))?);
                } else {
                    leftover.push(body.join("\n"));
                }
                continue;
            }
            j += 1;
            if line.is_empty() {
                continue;
            }
            let matched = known.iter().find_map(|k| {
                line.strip_prefix(&format!("{}: ", k))
                    .map(|v| (*k, v.trim().to_string()))
                    .or_else(|| (line == format!("{}:", k)).then(|| (*k, String::new())))
            });
            match matched {
                Some(("Context", v)) => context = v,
                Some(("Decision", v)) => decision = v,
                Some(("Requested by", v)) => requested_by = v,
                Some(("Affects", v)) => affects_raw = v,
                Some((k, v)) => notes.push(format!("{}: {}", k, v)),
                None => leftover.push(line.to_string()),
            }
        }

        if date.is_empty() {
            warnings.push(format!(
                "{}: no date in the header; recorded as \"unknown\"",
                id
            ));
            date = "unknown".to_string();
        }
        if title.is_empty() {
            warnings.push(format!(
                "{}: no title in the header; recorded as \"(untitled)\"",
                id
            ));
            title = "(untitled)".to_string();
        }
        // A second block with the same id is never skipped silently: a
        // skipped block is lost once decisions.md is removed, and which of
        // the two is real cannot be decided here.
        if !seen.insert(id.clone()) {
            return Err(format!(
                "{}: appears twice in decisions.md (the second header is at line {}); nothing is migrated — rename or merge one of them",
                id,
                i + 1
            ));
        }

        let affects: Vec<Value> = {
            let mut out = Vec::new();
            let mut seen_a = std::collections::HashSet::new();
            for part in strip_trailing_dot(&affects_raw).split(", ") {
                let p = part.trim();
                if !p.is_empty() && seen_a.insert(p.to_string()) {
                    out.push(Value::from(p));
                }
            }
            out
        };
        if !leftover.is_empty() {
            notes.push(leftover.join(" "));
        }

        let mut rec = json::obj();
        rec.insert("id".into(), Value::from(id));
        rec.insert("title".into(), Value::from(title));
        rec.insert("status".into(), Value::from("resolved"));
        rec.insert("date".into(), Value::from(date));
        if !context.is_empty() {
            rec.insert("context".into(), Value::from(context));
        }
        if !requested_by.is_empty() {
            rec.insert(
                "requested_by".into(),
                Value::from(strip_trailing_dot(&requested_by)),
            );
        }
        if !affects.is_empty() {
            rec.insert("affects".into(), Value::Array(affects));
        }
        let mut res = json::obj();
        if !decision.is_empty() {
            res.insert("decision".into(), Value::from(decision));
        }
        if !notes.is_empty() {
            res.insert("note".into(), Value::from(notes.join("\n\n")));
        }
        rec.insert("resolution".into(), Value::Object(res));
        if !omits.is_empty() {
            rec.insert(
                "omits".into(),
                Value::Array(omits.iter().map(omit_value).collect()),
            );
        }
        records.push(Value::Object(rec));
        i = j;
    }
    Ok((records, warnings))
}

/// A fenced block's body, first line `Omits:`, as YAML: a mapping with one
/// key, `Omits`, whose value is a list of `{operation, direction, path,
/// reason}` records — exactly `decisions.md`'s own hand-authored shape. Any
/// entry missing a required key or using a `direction`/`reason` outside
/// `decisions.schema.json`'s fixed vocabulary is a parse failure, not a
/// silent drop.
fn parse_omits_fence(decision_id: &str, fence_body: &str) -> Result<Vec<Omit>, String> {
    let parsed = crate::yaml::parse(fence_body)
        .map_err(|e| format!("{}: Omits: block is not valid YAML: {}", decision_id, e))?;
    let entries = json::get_arr(&parsed, "Omits").ok_or_else(|| {
        format!(
            "{}: Omits: block has no list under the Omits key",
            decision_id
        )
    })?;
    entries
        .iter()
        .enumerate()
        .map(|(n, entry)| {
            let field = |name: &str| {
                json::get_str(entry, name)
                    .map(str::to_string)
                    .ok_or_else(|| {
                        format!("{}: Omits: entry {} has no {}", decision_id, n + 1, name)
                    })
            };
            let operation = field("operation")?;
            let direction = field("direction")?;
            let path = field("path")?;
            let reason = field("reason")?;
            check_omit_vocabulary(&direction, &reason)
                .map_err(|e| format!("{}: Omits: entry {}: {}", decision_id, n + 1, e))?;
            Ok(Omit {
                operation,
                direction,
                path,
                reason,
            })
        })
        .collect()
}

fn choice_value(c: Choice) -> Value {
    let mut m = json::obj();
    m.insert("id".into(), Value::from(c.id));
    m.insert("label".into(), Value::from(c.label));
    if let Some(d) = c.detail {
        m.insert("detail".into(), Value::from(d));
    }
    Value::Object(m)
}

fn omit_value(o: &Omit) -> Value {
    json::object(vec![
        ("operation", Value::from(o.operation.as_str())),
        ("direction", Value::from(o.direction.as_str())),
        ("path", Value::from(o.path.as_str())),
        ("reason", Value::from(o.reason.as_str())),
    ])
}

fn secret_field_value(s: &SecretField) -> Value {
    json::object(vec![
        ("type", Value::from(s.type_name.as_str())),
        ("field", Value::from(s.field.as_str())),
        ("disposition", Value::from(s.disposition.as_str())),
        ("reason", Value::from(s.reason.as_str())),
    ])
}

fn json_reason_value(j: &JsonReason) -> Value {
    json::object(vec![
        ("type", Value::from(j.type_name.as_str())),
        ("field", Value::from(j.field.as_str())),
        ("reason", Value::from(j.reason.as_str())),
    ])
}

fn null_handling_value(n: &NullHandling) -> Value {
    json::object(vec![
        ("operation", Value::from(n.operation.as_str())),
        ("argument", Value::from(n.argument.as_str())),
        ("behavior", Value::from(n.behavior.as_str())),
    ])
}

fn resolution_value(r: Resolution) -> Value {
    let mut m = json::obj();
    if !r.chosen.is_empty() {
        m.insert("chosen".into(), Value::from(r.chosen));
    }
    if let Some(n) = r.note {
        m.insert("note".into(), Value::from(n));
    }
    if let Some(d) = r.decision {
        m.insert("decision".into(), Value::from(d));
    }
    if let Some(b) = r.by {
        m.insert("by".into(), Value::from(b));
    }
    if let Some(a) = r.at {
        m.insert("at".into(), Value::from(a));
    }
    Value::Object(m)
}

/// Append a new record and return its assigned id. In-memory only; the caller
/// persists with `save`.
pub fn add(doc: &mut Value, new: NewDecision) -> Result<String, String> {
    if new.title.trim().is_empty() {
        return Err("a decision needs a title".into());
    }
    let id = next_id(doc);
    let status = if new.resolution.is_some() {
        "resolved"
    } else {
        "open"
    };
    let mut rec = json::obj();
    rec.insert("id".into(), Value::from(id.clone()));
    rec.insert("title".into(), Value::from(new.title));
    rec.insert("status".into(), Value::from(status));
    rec.insert("date".into(), Value::from(new.date));
    if let Some(p) = new.phase {
        rec.insert("phase".into(), Value::from(p));
    }
    if let Some(q) = new.question {
        rec.insert("question".into(), Value::from(q));
    }
    if let Some(c) = new.context {
        rec.insert("context".into(), Value::from(c));
    }
    if let Some(r) = new.requested_by {
        rec.insert("requested_by".into(), Value::from(r));
    }
    if new.multiple {
        rec.insert("multiple".into(), Value::from(true));
    }
    if !new.affects.is_empty() {
        rec.insert("affects".into(), Value::from(new.affects));
    }
    if !new.choices.is_empty() {
        rec.insert(
            "choices".into(),
            Value::Array(new.choices.into_iter().map(choice_value).collect()),
        );
    }
    if let Some(res) = new.resolution {
        rec.insert("resolution".into(), resolution_value(res));
    }
    if !new.omits.is_empty() {
        rec.insert(
            "omits".into(),
            Value::Array(new.omits.iter().map(omit_value).collect()),
        );
    }
    if !new.secret_fields.is_empty() {
        rec.insert(
            "secret_fields".into(),
            Value::Array(new.secret_fields.iter().map(secret_field_value).collect()),
        );
    }
    if !new.json_reasons.is_empty() {
        rec.insert(
            "json_reasons".into(),
            Value::Array(new.json_reasons.iter().map(json_reason_value).collect()),
        );
    }
    if !new.null_handling.is_empty() {
        rec.insert(
            "null_handling".into(),
            Value::Array(new.null_handling.iter().map(null_handling_value).collect()),
        );
    }
    if let Some(Value::Array(items)) = doc.get_mut("decisions") {
        items.push(Value::Object(rec));
    }
    Ok(id)
}

/// Mark an existing decision resolved. Refuses an unknown id, and refuses to
/// re-resolve an already-resolved decision unless `force`.
pub fn resolve(
    doc: &mut Value,
    id: &str,
    resolution: Resolution,
    force: bool,
) -> Result<(), String> {
    let idx = index_of(doc, id).ok_or_else(|| format!("no decision {}", id))?;
    let items = doc
        .get_mut("decisions")
        .and_then(Value::as_array_mut)
        .ok_or("decisions is not an array")?;
    let rec = &mut items[idx];
    if json::get_str(rec, "status") == Some("resolved") && !force {
        return Err(format!(
            "{} is already resolved; pass --force to re-resolve it",
            id
        ));
    }
    // A re-resolve replaces the whole resolution. One that omits a recorded
    // choice or a recorded decision would erase it, which is never what
    // `--force` with only a `--note`, or with only the other of the two,
    // means.
    let existing = json::get(rec, "resolution");
    let had_choice = existing
        .and_then(|r| json::get_arr(r, "chosen"))
        .is_some_and(|c| !c.is_empty());
    let had_decision = existing
        .and_then(|r| json::get_str(r, "decision"))
        .is_some();
    let erased: Vec<&str> = [
        (had_choice && resolution.chosen.is_empty(), "--chosen"),
        (had_decision && resolution.decision.is_none(), "--decision"),
    ]
    .iter()
    .filter(|(lost, _)| *lost)
    .map(|(_, flag)| *flag)
    .collect();
    if !erased.is_empty() {
        return Err(format!(
            "{} already records {}; re-resolving without {} would erase it — pass what it should now record",
            id,
            match (had_choice, had_decision) {
                (true, true) => "a choice and a decision",
                (true, false) => "a choice",
                _ => "a decision",
            },
            erased.join(" or ")
        ));
    }
    json::set(rec, "status", Value::from("resolved"));
    json::set(rec, "resolution", resolution_value(resolution));
    Ok(())
}

/// Mark a `resolved` decision `superseded` (ADR 0103): a later decision, or
/// a change to the service, replaced it. Everything on the record is kept,
/// its `resolution` included, so the log still says what was decided; only
/// the status changes, and a superseded record's `omits` no longer count
/// (`source-coverage`). An `open` record is refused (`not-resolved`: there
/// is no recorded decision to supersede; `reopen` is not needed either), as
/// is one already superseded (`already-superseded`) and an unknown id.
/// In-memory only; the caller persists with `save`.
pub fn supersede(doc: &mut Value, id: &str) -> Result<(), ReopenRefusal> {
    let unknown = || ReopenRefusal {
        code: "unknown-decision",
        message: format!("no decision {}", id),
    };
    let idx = index_of(doc, id).ok_or_else(unknown)?;
    let rec = doc
        .get_mut("decisions")
        .and_then(Value::as_array_mut)
        .and_then(|items| items.get_mut(idx))
        .ok_or_else(unknown)?;
    match json::get_str(rec, "status").unwrap_or("open") {
        "resolved" => {}
        "superseded" => {
            return Err(ReopenRefusal {
                code: "already-superseded",
                message: format!("{} is already superseded", id),
            })
        }
        other => {
            return Err(ReopenRefusal {
                code: "not-resolved",
                message: format!(
                    "{} is {}; only a resolved decision can be superseded",
                    id, other
                ),
            })
        }
    }
    json::set(rec, "status", Value::from("superseded"));
    Ok(())
}

/// Why `reopen` refused a record. `code` is the stable slug a `--json`
/// caller branches on.
#[derive(Debug)]
pub struct ReopenRefusal {
    pub code: &'static str,
    pub message: String,
}

/// What `reopen` changed: the status the record had, and the resolution it
/// cleared (absent when a `superseded` record carried none).
#[derive(Debug)]
pub struct Reopened {
    pub previous_status: String,
    pub cleared: Option<Value>,
}

/// Put a `resolved` or `superseded` decision back to `open` so the user can
/// answer it again (ADR 0060). The recorded answer — the whole `resolution`
/// block, which holds everything `resolve` writes (`chosen`, `note`,
/// `decision`, `by`, `at`) — is removed; every other field (title, question,
/// context, choices, affects, omits, …) and the record's position are kept.
/// The log keeps no history field, so the answer that was cleared survives
/// only in the workspace's git history and in the caller's report. An open
/// record is refused: there is nothing to reopen. In-memory only; the caller
/// persists with `save`.
pub fn reopen(doc: &mut Value, id: &str) -> Result<Reopened, ReopenRefusal> {
    let idx = index_of(doc, id).ok_or_else(|| ReopenRefusal {
        code: "unknown-decision",
        message: format!("no decision {}", id),
    })?;
    let Some(rec) = doc
        .get_mut("decisions")
        .and_then(Value::as_array_mut)
        .and_then(|items| items.get_mut(idx))
    else {
        return Err(ReopenRefusal {
            code: "unknown-decision",
            message: format!("no decision {}", id),
        });
    };
    let previous_status = json::get_str(rec, "status").unwrap_or("open").to_string();
    if previous_status == "open" {
        return Err(ReopenRefusal {
            code: "already-open",
            message: format!("{} is already open; there is no answer to clear", id),
        });
    }
    let cleared = json::get(rec, "resolution").cloned();
    json::remove(rec, "resolution");
    json::set(rec, "status", Value::from("open"));
    Ok(Reopened {
        previous_status,
        cleared,
    })
}

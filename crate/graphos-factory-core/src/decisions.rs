//! The workspace decision log: judgement calls already made and open questions
//! still awaiting the user, as individually tracked records. Replaces the
//! free-form `decisions.md` (ADR 0026).
//!
//! The agent never hand-edits this file; `graphos-factory-core decisions` is the
//! only writer, so a headless agent and a host UI record resolutions through
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
/// `selection.yaml` or a schema doc comment — the decisions-only rule).
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
    /// The record file's name after the id; derived from the title when
    /// absent (ADR 0118).
    pub slug: Option<String>,
    /// Decisions this one presumes.
    pub after: Vec<String>,
    /// Decisions whose answer this one changes where their `affects`
    /// overlap.
    pub amends: Vec<String>,
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

/// An empty log.
pub fn empty() -> Value {
    json::object(vec![
        ("contract_version", Value::from(1)),
        ("decisions", Value::Array(vec![])),
    ])
}

/// Read the log, or an empty one when there is none: the records of
/// `decisions.json`, then those of `.factory/decisions/` (ADR 0118,
/// `crate::record_log`). A present-but-invalid log
/// is an error rather than a silent reset — the agent must not lose a
/// recorded decision to a bad edit.
pub fn load(dir: &Path, schemas_dir: Option<&Path>) -> Result<Value, String> {
    Ok(load_present(dir, schemas_dir)?.unwrap_or_else(empty))
}

/// Read the log, or `None` when there is neither a `decisions.json` nor a
/// `.factory/decisions/` — for a verb that acts on a recorded decision and so
/// has nothing to do without one. Present but unreadable or invalid is an error,
/// as for `load`. Every read goes through custody, so a symlinked log or
/// record file is refused rather than read through (ADR 0025).
pub fn load_present(dir: &Path, schemas_dir: Option<&Path>) -> Result<Option<Value>, String> {
    crate::record_log::read(dir, &crate::record_log::DECISIONS, schemas_dir)
}

/// Validate then write: each record `decisions.json` holds goes back into it
/// (it stays contract_version 1 and changes only when one of its records
/// did), every other record to its own file under `.factory/decisions/`
/// (ADR 0118). Refuses a document the schema rejects, that holds an id
/// twice, or whose `after`/`amends` dangle or loop.
pub fn save(dir: &Path, doc: &Value, schemas_dir: Option<&Path>) -> Result<(), String> {
    check_links(doc)?;
    crate::record_log::write(dir, &crate::record_log::DECISIONS, doc, schemas_dir)
}

/// Write the whole log as `decisions.json`, as before ADR 0118: for the
/// migrations that produce the single file (`migrate` from `decisions.md`,
/// `migrate --split`).
pub fn save_single(dir: &Path, doc: &Value, schemas_dir: Option<&Path>) -> Result<(), String> {
    crate::record_log::write_single(dir, &crate::record_log::DECISIONS, doc, schemas_dir)
}

/// Where the record with this id lives, for a message or a lint finding to
/// name: `decisions.json` for a numbered record, written before ADR 0118,
/// else the record directory.
pub fn file_of(id: &str) -> &'static str {
    if crate::record_log::is_random(id) {
        crate::record_log::DECISIONS.dir
    } else {
        FILE
    }
}

/// The edges a record names: `after` then `amends`.
pub fn edges(rec: &Value) -> Vec<&str> {
    ["after", "amends"]
        .iter()
        .flat_map(|k| json::get_arr(rec, k).into_iter().flatten())
        .filter_map(Value::as_str)
        .collect()
}

/// Every `after` or `amends` id that names no decision in the log, as
/// `(from, to)`.
pub fn unresolved_links(doc: &Value) -> Vec<(String, String)> {
    let ids = crate::record_log::ids(doc, "decisions");
    let mut out = Vec::new();
    for rec in json::get_arr(doc, "decisions").into_iter().flatten() {
        let from = json::get_str(rec, "id").unwrap_or("");
        for to in edges(rec) {
            if !ids.contains(to) {
                out.push((from.to_string(), to.to_string()));
            }
        }
    }
    out
}

/// A cycle through `after` and `amends`, as the ids along it with the first
/// repeated at the end, or `None`. Depth-first from each record in log
/// order, so the same log always names the same cycle.
pub fn link_cycle(doc: &Value) -> Option<Vec<String>> {
    let recs: Vec<&Value> = json::get_arr(doc, "decisions")
        .into_iter()
        .flatten()
        .collect();
    let index: std::collections::HashMap<&str, usize> = recs
        .iter()
        .enumerate()
        .filter_map(|(i, r)| json::get_str(r, "id").map(|id| (id, i)))
        .collect();
    // 0 unvisited, 1 on the current path, 2 done.
    let mut state = vec![0u8; recs.len()];
    let mut path: Vec<usize> = Vec::new();
    fn visit(
        i: usize,
        recs: &[&Value],
        index: &std::collections::HashMap<&str, usize>,
        state: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        state[i] = 1;
        path.push(i);
        for to in edges(recs[i]) {
            let Some(&j) = index.get(to) else { continue };
            if state[j] == 1 {
                let start = path.iter().position(|&p| p == j).unwrap_or(0);
                let mut cycle = path[start..].to_vec();
                cycle.push(j);
                return Some(cycle);
            }
            if state[j] == 0 {
                if let Some(c) = visit(j, recs, index, state, path) {
                    return Some(c);
                }
            }
        }
        path.pop();
        state[i] = 2;
        None
    }
    for i in 0..recs.len() {
        if state[i] == 0 {
            if let Some(c) = visit(i, &recs, &index, &mut state, &mut path) {
                return Some(
                    c.into_iter()
                        .map(|k| json::get_str(recs[k], "id").unwrap_or("").to_string())
                        .collect(),
                );
            }
        }
    }
    None
}

/// The writer's refusal of a dangling or circular `after`/`amends` (ADR
/// 0118). `lint` reports the same two on a hand-merged log.
fn check_links(doc: &Value) -> Result<(), String> {
    if let Some((from, to)) = unresolved_links(doc).into_iter().next() {
        return Err(format!(
            "{} names {} in after or amends, and the log has no {}",
            from, to, to
        ));
    }
    if let Some(cycle) = link_cycle(doc) {
        return Err(format!(
            "after and amends form a cycle: {}",
            cycle.join(" -> ")
        ));
    }
    Ok(())
}

/// One ordering edge between two decisions: `before` comes first, and `via`
/// says why (`after`, `amends`, or `affects <span>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub before: String,
    pub after: String,
    pub via: String,
}

/// The spans two records share: an exact `affects` string both name, or
/// `every operation` on either side (ADR 0118 §3.3).
fn shared_spans(a: &Value, b: &Value) -> Vec<String> {
    let sa = json::strings(json::field(a, "affects"));
    let sb = json::strings(json::field(b, "affects"));
    const EVERY: &str = "every operation";
    if sa.iter().any(|s| s == EVERY) || sb.iter().any(|s| s == EVERY) {
        if sa.is_empty() || sb.is_empty() {
            return vec![];
        }
        return vec![EVERY.to_string()];
    }
    sa.into_iter().filter(|s| sb.contains(s)).collect()
}

/// Whether `a` is older than `b` for the implicit order: by number when
/// both are numbered; a numbered record before a random one, whatever the
/// dates say, since every numbered record was written before ADR 0118 and
/// only a random one can carry the edge (`decisions link` refuses an old
/// record, so the newer of a mixed pair must be the random one); else by
/// `date`, then id.
fn older(a: &Value, b: &Value) -> bool {
    let ia = json::get_str(a, "id").unwrap_or("");
    let ib = json::get_str(b, "id").unwrap_or("");
    match (crate::record_log::number(ia), crate::record_log::number(ib)) {
        (Some(x), Some(y)) => x < y,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => {
            let da = json::get_str(a, "date").unwrap_or("");
            let db = json::get_str(b, "date").unwrap_or("");
            (da, ia) < (db, ib)
        }
    }
}

/// Which records each record can reach through `after` and `amends`,
/// followed from the later record to the one it names.
fn explicit_reach(recs: &[&Value]) -> Vec<std::collections::HashSet<usize>> {
    let index: std::collections::HashMap<&str, usize> = recs
        .iter()
        .enumerate()
        .filter_map(|(i, r)| json::get_str(r, "id").map(|id| (id, i)))
        .collect();
    (0..recs.len())
        .map(|start| {
            let mut seen = std::collections::HashSet::new();
            let mut stack = vec![start];
            while let Some(i) = stack.pop() {
                for to in edges(recs[i]) {
                    if let Some(&j) = index.get(to) {
                        if seen.insert(j) {
                            stack.push(j);
                        }
                    }
                }
            }
            seen
        })
        .collect()
}

/// Two resolved decisions that name the same span with no `after`/`amends`
/// path between them either way, as `(older, newer, span)`. With
/// `random_only`, a pair of numbered records is left out: their numbers
/// already order them (ADR 0118's `decision-overlap`).
pub fn unordered_overlaps(doc: &Value, random_only: bool) -> Vec<(String, String, String)> {
    let recs: Vec<&Value> = json::get_arr(doc, "decisions")
        .into_iter()
        .flatten()
        .filter(|r| json::get_str(r, "status") == Some("resolved"))
        .collect();
    let reach = explicit_reach(&recs);
    let mut out = Vec::new();
    for i in 0..recs.len() {
        for j in i + 1..recs.len() {
            let (a, b) = (recs[i], recs[j]);
            let ia = json::get_str(a, "id").unwrap_or("");
            let ib = json::get_str(b, "id").unwrap_or("");
            if random_only && !crate::record_log::is_random(ia) && !crate::record_log::is_random(ib)
            {
                continue;
            }
            if reach[i].contains(&j) || reach[j].contains(&i) {
                continue;
            }
            if let Some(span) = shared_spans(a, b).into_iter().next() {
                let (old, new) = if older(a, b) { (ia, ib) } else { (ib, ia) };
                out.push((old.to_string(), new.to_string(), span));
            }
        }
    }
    out
}

/// The causal order of the log (ADR 0118 §3.3): every decision after the
/// ones it names in `after` or `amends`, and after an older resolved
/// decision that names one of its spans when no explicit path relates the
/// two. Topological, oldest first; ties keep the log's order. Returns each
/// record with the edges into it, or, when there is no order, the records
/// left unplaced: a cycle through `after`/`amends`, or explicit edges that
/// run against the age order of spans a third record shares with both.
pub fn causal_order(doc: &Value) -> Result<Vec<(Value, Vec<Edge>)>, Vec<String>> {
    if let Some(cycle) = link_cycle(doc) {
        return Err(cycle);
    }
    let recs: Vec<&Value> = json::get_arr(doc, "decisions")
        .into_iter()
        .flatten()
        .collect();
    let id_of = |r: &Value| json::get_str(r, "id").unwrap_or("").to_string();
    let index: std::collections::HashMap<String, usize> = recs
        .iter()
        .enumerate()
        .map(|(i, r)| (id_of(r), i))
        .collect();
    let mut incoming: Vec<Vec<Edge>> = vec![Vec::new(); recs.len()];
    for (i, r) in recs.iter().enumerate() {
        for (key, list) in [("after", "after"), ("amends", "amends")] {
            for to in json::strings(json::field(r, key)) {
                if index.contains_key(&to) {
                    incoming[i].push(Edge {
                        before: to,
                        after: id_of(r),
                        via: list.to_string(),
                    });
                }
            }
        }
    }
    for (old, new, span) in unordered_overlaps(doc, false) {
        if let Some(&n) = index.get(&new) {
            incoming[n].push(Edge {
                before: old,
                after: new,
                via: format!("affects {}", span),
            });
        }
    }
    let mut placed = vec![false; recs.len()];
    let mut out = Vec::new();
    while out.len() < recs.len() {
        let next = (0..recs.len()).find(|&i| {
            !placed[i]
                && incoming[i]
                    .iter()
                    .all(|e| index.get(&e.before).is_none_or(|&b| placed[b]))
        });
        let Some(i) = next else {
            // `after`/`amends` alone are acyclic (checked above), so an
            // implicit edge closed this loop: an explicit edge points
            // against age, and a third record orders the two the other way.
            let rest: Vec<String> = (0..recs.len())
                .filter(|&i| !placed[i])
                .map(|i| id_of(recs[i]))
                .collect();
            return Err(rest);
        };
        placed[i] = true;
        out.push((recs[i].clone(), std::mem::take(&mut incoming[i])));
    }
    Ok(out)
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// The id a new record gets: `D-` and six random base36 characters no
/// record holds (ADR 0118). Numbered ids are only ever the ones already
/// recorded.
pub fn new_id(doc: &Value) -> String {
    let taken = crate::record_log::ids(doc, "decisions");
    crate::record_log::new_id("D-", |id| taken.contains(id))
}

/// The next `D-nnnn` after the highest number recorded: how ids were minted
/// before ADR 0118. Nothing mints one now; it says where the numbered
/// sequence ends.
pub fn next_id(doc: &Value) -> String {
    let max = json::get_arr(doc, "decisions")
        .into_iter()
        .flatten()
        .filter_map(|d| json::get_str(d, "id"))
        .filter_map(crate::record_log::number)
        .max()
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
    let slug = match new.slug {
        Some(s) if !crate::record_log::is_slug(&s) => {
            return Err(format!(
                "--slug {:?}: lower-case letters and digits in hyphen-separated runs, at most {} characters",
                s,
                crate::record_log::SLUG_MAX
            ))
        }
        Some(s) => Some(s),
        None => Some(crate::record_log::slugify(&new.title)).filter(|s| !s.is_empty()),
    };
    let id = new_id(doc);
    let status = if new.resolution.is_some() {
        "resolved"
    } else {
        "open"
    };
    let mut rec = json::obj();
    rec.insert("id".into(), Value::from(id.clone()));
    if let Some(s) = slug {
        rec.insert("slug".into(), Value::from(s));
    }
    rec.insert("title".into(), Value::from(new.title));
    rec.insert("status".into(), Value::from(status));
    rec.insert("date".into(), Value::from(new.date));
    if !new.after.is_empty() {
        rec.insert("after".into(), Value::from(dedup(new.after)));
    }
    if !new.amends.is_empty() {
        rec.insert("amends".into(), Value::from(dedup(new.amends)));
    }
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

/// Add `after`/`amends` edges to a decision recorded since ADR 0118: how
/// `decision-overlap` is answered for two records added independently. An
/// old, numbered record never gains a field (`decisions.json` stays what
/// every older binary reads), so it is refused (`old-record`), as is an
/// unknown id; a dangling or circular edge is refused at `save`. In-memory
/// only; the caller persists with `save`.
pub fn link(
    doc: &mut Value,
    id: &str,
    after: Vec<String>,
    amends: Vec<String>,
) -> Result<(), ReopenRefusal> {
    if !crate::record_log::is_random(id) {
        return Err(ReopenRefusal {
            code: "old-record",
            message: format!(
                "{} was recorded before ADR 0118 and keeps the fields it has; put the edge on the newer record instead, which may name {}",
                id, id
            ),
        });
    }
    let idx = index_of(doc, id).ok_or_else(|| ReopenRefusal {
        code: "unknown-decision",
        message: format!("no decision {}", id),
    })?;
    let rec = &mut doc["decisions"][idx];
    for (key, new) in [("after", after), ("amends", amends)] {
        if new.is_empty() {
            continue;
        }
        let mut all = json::strings(json::field(rec, key));
        all.extend(new);
        json::set(rec, key, Value::from(dedup(all)));
    }
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

//! selection — draft the judgements the inventory deliberately does not make.
//!
//!   selection draft [workspace] [--op KEY]… [--links | --envelopes] [--force] [--dry-run] [--json]
//!
//! `inventory.json` records facts; `selection.yaml` records judgements
//! (ADR 0018). Two judgements the tool can *propose* deterministically:
//!
//! - the response envelope — which root property the payload's useful
//!   content sits under — written into each included operation's
//!   `response:` block;
//! - a relationship link (ADR 0069) — a property whose name and type family
//!   match a canonical GET-by-id operation's path parameter
//!   (`candidate_entity_link`) becomes an entry of the top-level `links:`
//!   list, with the GraphQL field name the tool derives from the
//!   operation's path.
//!
//! Every block and every entry lands `confirmed: false`. The agent agrees
//! each one with the user (SKILL.md, `select`) and sets `confirmed: true` or
//! drops it; reconcile notes every unconfirmed draft and lint warns on it,
//! so a draft can never govern a schema by default. `--force` redraws a
//! response block, and a links entry only while it is still a draft: a
//! confirmed link (or one with no `confirmed` key, the user's word) and a
//! declined one (`include: false`) are never replaced. `--links` and
//! `--envelopes` together are a usage error.
//!
//! The file is edited in place, line by line: a `response:` block is
//! spliced under the operation's key, a link entry is appended to the
//! `links:` section (created at the end of the file when absent), and every
//! other line, comment and anchor is left exactly as it was. The result is
//! parsed and validated against `selection.schema.json` before it is
//! written; if it does not parse, nothing is written.
//!
//! Exit codes: 0 written · 1 usage, an unreadable workspace, an `--op` that
//! matches nothing, or a splice that would not parse · 2 nothing to do.

use crate::args::{Args, Flags};
use crate::cmd::inventory_links::CandidateLink;
use crate::json::{get, get_obj, get_str, truthy};
use serde_json::Value;
use std::path::Path;

/// The usage text: `selection draft --help` prints it on stdout (ADR 0086).
pub const USAGE: &str = "usage: selection draft [workspace] [--op KEY]… [--links | --envelopes] [--force] [--dry-run] [--json]";

fn usage(msg: &str) -> i32 {
    eprintln!("selection: {}", msg);
    eprintln!("{}", USAGE);
    1
}

pub const SELECTION_FILE: &str = ".factory/selection.yaml";

/// One operation's key line inside `operations:`: where it is, how deep its
/// own keys sit, and where its block ends.
pub(crate) struct Entry {
    pub(crate) line: usize,
    pub(crate) child_indent: usize,
    pub(crate) end: usize,
}

pub(crate) fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty() || line.trim_start().starts_with('#')
}

/// The YAML key a line declares, unquoted, when the line is `key:` with
/// nothing but a comment after it.
pub(crate) fn key_line(line: &str) -> Option<String> {
    let t = line.trim_start();
    let (raw, rest) = if let Some(r) = t.strip_prefix('"') {
        let end = r.find('"')?;
        (r[..end].to_string(), &r[end + 1..])
    } else if let Some(r) = t.strip_prefix('\'') {
        let end = r.find('\'')?;
        (r[..end].to_string(), &r[end + 1..])
    } else {
        // A plain key ends at YAML's mapping indicator, a `:` followed by
        // whitespace or the end of the line, not at the first `:`: operation
        // keys carry colons (`get:/colors:` declares `get:/colors`).
        let end = t
            .char_indices()
            .find(|&(i, c)| c == ':' && t[i + 1..].chars().next().map_or(true, char::is_whitespace))
            .map(|(i, _)| i)?;
        (t[..end].to_string(), &t[end..])
    };
    let rest = rest.strip_prefix(':')?;
    if rest.trim().is_empty() || rest.trim_start().starts_with('#') {
        Some(raw)
    } else {
        None
    }
}

/// Locate one operation entry in the selection's text.
pub(crate) fn find_entry(lines: &[&str], key: &str) -> Option<Entry> {
    let ops = lines
        .iter()
        .position(|l| indent_of(l) == 0 && key_line(l).as_deref() == Some("operations"))?;
    let mut i = ops + 1;
    while i < lines.len() {
        let line = lines[i];
        if !is_blank(line) && indent_of(line) == 0 {
            return None;
        }
        if !is_blank(line) && key_line(line).as_deref() == Some(key) {
            let indent = indent_of(line);
            let mut end = i + 1;
            let mut child_indent = indent + 2;
            let mut seen_child = false;
            while end < lines.len() {
                if is_blank(lines[end]) {
                    end += 1;
                    continue;
                }
                if indent_of(lines[end]) <= indent {
                    break;
                }
                if !seen_child {
                    child_indent = indent_of(lines[end]);
                    seen_child = true;
                }
                end += 1;
            }
            // Trailing blank/comment lines belong to whatever follows.
            while end > i + 1 && is_blank(lines[end - 1]) {
                end -= 1;
            }
            return Some(Entry {
                line: i,
                child_indent,
                end,
            });
        }
        i += 1;
    }
    None
}

/// The `response:` block inside an entry, as a line range, when it has one.
fn response_block(lines: &[&str], entry: &Entry) -> Option<(usize, usize)> {
    for i in entry.line + 1..entry.end {
        if is_blank(lines[i]) || indent_of(lines[i]) != entry.child_indent {
            continue;
        }
        if key_line(lines[i]).as_deref() == Some("response") {
            let mut end = i + 1;
            while end < entry.end
                && (is_blank(lines[end]) || indent_of(lines[end]) > entry.child_indent)
            {
                end += 1;
            }
            while end > i + 1 && is_blank(lines[end - 1]) {
                end -= 1;
            }
            return Some((i, end));
        }
    }
    None
}

/// A line that opens a block-sequence entry: `- key: …` (or `- {…}`), or a
/// line holding only `-`, whose keys sit on the more-indented lines below.
fn opens_entry(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ") || t.trim_end() == "-"
}

/// The top-level `links:` section as a line range — its key line to the
/// line after its last entry (trailing blank and comment lines stay with
/// what follows) — when the file has one written as a block. A flow
/// sequence on the key's own line (`links: []`) is not a section this
/// writer can extend. Entries may open in column 0 (`- shape:`, or a bare
/// `-`) or be indented. A column-0 `#` comment between entries belongs to
/// the section, as YAML reads it (`is_blank` counts a comment line as
/// blank). The one walker of the section: `selection set --link`
/// (`selection_set.rs`; Phase 7as (d), deferred) is to import this and
/// `link_entries` rather than walk it again (ADR 0069, R18).
pub(crate) fn links_section(lines: &[&str]) -> Option<(usize, usize)> {
    let start = lines
        .iter()
        .position(|l| indent_of(l) == 0 && key_line(l).as_deref() == Some("links"))?;
    let mut end = start + 1;
    while end < lines.len() {
        let l = lines[end];
        // A comment is never a section; anything in column 0 other than an
        // entry's opening line is the next section.
        if !is_blank(l) && indent_of(l) == 0 && !opens_entry(l) {
            break;
        }
        end += 1;
    }
    while end > start + 1 && is_blank(lines[end - 1]) {
        end -= 1;
    }
    Some((start, end))
}

/// The entries of a `links:` section, in order, each as the line range from
/// the line that opens it (`opens_entry`, at the first entry's indent) to
/// the line before the next one (trailing blank and comment lines
/// excluded). For a block sequence, range `i` is the `i`-th element of the
/// parsed `links` array. A section written some other way — a flow
/// sequence on the line after `links:` — yields fewer ranges than the
/// array has elements, so a caller that splices by index must compare the
/// two counts and refuse on a mismatch, as `selection draft` does.
pub(crate) fn link_entries(lines: &[&str], section: (usize, usize)) -> Vec<(usize, usize)> {
    let (start, end) = section;
    let mut entries: Vec<(usize, usize)> = Vec::new();
    let mut dash_indent: Option<usize> = None;
    for (i, l) in lines.iter().enumerate().take(end).skip(start + 1) {
        if is_blank(l) {
            continue;
        }
        if opens_entry(l) && dash_indent.map_or(true, |d| d == indent_of(l)) {
            dash_indent = Some(indent_of(l));
            if let Some(last) = entries.last_mut() {
                last.1 = i;
            }
            entries.push((i, end));
        }
    }
    for e in &mut entries {
        while e.1 > e.0 + 1 && is_blank(lines[e.1 - 1]) {
            e.1 -= 1;
        }
    }
    entries
}

/// One drafted `links:` entry, indented two columns like every pilot's
/// `overrides:` and `waivers:` entries. Scalars go through `yaml::scalar`,
/// so an operation key and a `[]>` path are quoted and a plain name is not.
fn link_block(link: &CandidateLink, field: &str) -> Vec<String> {
    let s = crate::yaml::scalar;
    vec![
        format!("  - shape: {}", s(&link.shape)),
        format!("    path: {}", s(&link.path)),
        format!("    operation: {}", s(&link.operation)),
        format!("    parameter: {}", s(&link.parameter)),
        format!("    field: {}", s(field)),
        "    include: true".to_string(),
        "    confirmed: false   # drafted by `graphos-factory-core selection draft`".to_string(),
    ]
}

/// Re-indent a `link_block` (written at dash indent 2) to the indent the
/// section's existing entries use.
fn reindent(block: Vec<String>, dash_indent: usize) -> Vec<String> {
    let pad = " ".repeat(dash_indent);
    block
        .into_iter()
        .map(|l| format!("{}{}", pad, &l[2..]))
        .collect()
}

/// Splice drafted entries into the `links:` section: replacements in place
/// (bottom-up, so earlier ranges stay valid), new entries at the section's
/// end, and the section itself at the end of the file when there is none.
fn splice_links(
    lines: &mut Vec<String>,
    mut replacements: Vec<(usize, Vec<String>)>,
    appended: Vec<Vec<String>>,
) {
    let located = {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        links_section(&refs).map(|section| {
            let entries = link_entries(&refs, section);
            let dash = entries
                .first()
                .map(|(s, _)| indent_of(refs[*s]))
                .unwrap_or(2);
            (section, entries, dash)
        })
    };
    match located {
        Some((section, entries, dash)) => {
            let mut insert_at = section.1;
            replacements.sort_by_key(|(i, _)| std::cmp::Reverse(*i));
            for (i, block) in replacements {
                if let Some(&(s, e)) = entries.get(i) {
                    let block = reindent(block, dash);
                    let delta = block.len() as isize - (e - s) as isize;
                    lines.splice(s..e, block);
                    insert_at = (insert_at as isize + delta) as usize;
                }
            }
            let mut new_lines: Vec<String> = Vec::new();
            for block in appended {
                new_lines.extend(reindent(block, dash));
            }
            lines.splice(insert_at..insert_at, new_lines);
        }
        None => {
            if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
            lines.push("links:".to_string());
            for block in appended {
                lines.extend(block);
            }
        }
    }
}

/// The field a drafted link proposes on the object that carries its fk:
/// the derived name, or — when another link on that object already holds
/// it (`author_id` and `committer_id` both resolve through `/users/{id}`
/// and derive `user`) — the derived name followed by the fk's own name
/// (`userCommitterId`), numbered from 2 only if a hand-named entry holds
/// that as well. `taken` answers for the one object in question.
fn distinct_field(derived: &str, path: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(derived) {
        return derived.to_string();
    }
    let fk = path
        .rsplit('>')
        .next()
        .unwrap_or(path)
        .trim_end_matches("[]");
    let base = format!("{}{}", derived, crate::inventory::cap(fk));
    if !taken(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let numbered = format!("{}{}", base, n);
        if !taken(&numbered) {
            return numbered;
        }
        n += 1;
    }
}

/// A surviving fact as the entry the draft proposes (no `field` yet), so
/// its derived field name and its object key come from `reconcile::Link`'s
/// own accessors — the ones reconcile and lint read — never a second
/// derivation here (R17).
fn proposed(c: &CandidateLink) -> crate::reconcile::Link {
    crate::reconcile::Link {
        shape: c.shape.clone(),
        path: c.path.clone(),
        operation: c.operation.clone(),
        parameter: Some(c.parameter.clone()),
        include: true,
        field: None,
        confirmed: false,
        reason: None,
        decision: None,
    }
}

/// `2 response blocks and 1 links entry`, for the summary line.
fn describe(envelopes: usize, links: usize) -> String {
    let part =
        |n: usize, one: &str, many: &str| format!("{} {}", n, if n == 1 { one } else { many });
    match (envelopes, links) {
        (_, 0) => part(envelopes, "response block", "response blocks"),
        (0, _) => part(links, "links entry", "links entries"),
        _ => format!(
            "{} and {}",
            part(envelopes, "response block", "response blocks"),
            part(links, "links entry", "links entries")
        ),
    }
}

fn scalar(envelope: Option<&str>) -> String {
    match envelope {
        Some(e) => serde_json::to_string(e).unwrap_or_else(|_| format!("\"{}\"", e)),
        None => "null".to_string(),
    }
}

fn block(child_indent: usize, envelope: Option<&str>) -> Vec<String> {
    let pad = " ".repeat(child_indent);
    let inner = " ".repeat(child_indent + 2);
    vec![
        format!("{}response:", pad),
        format!(
            "{}envelope: {}   # drafted by `graphos-factory-core selection draft`",
            inner,
            scalar(envelope)
        ),
        format!("{}confirmed: false", inner),
    ]
}

struct Plan {
    key: String,
    envelope: Option<String>,
    was: Option<Option<String>>,
    action: &'static str,
}

struct LinkPlan {
    shape: String,
    path: String,
    field: String,
    operation: String,
    action: &'static str,
}

fn links_value(plans: &[LinkPlan]) -> Value {
    Value::Array(
        plans
            .iter()
            .map(|p| {
                crate::json::object(vec![
                    ("shape", Value::from(p.shape.as_str())),
                    ("path", Value::from(p.path.as_str())),
                    ("field", Value::from(p.field.as_str())),
                    ("operation", Value::from(p.operation.as_str())),
                    ("action", Value::from(p.action)),
                    ("confirmed", Value::Bool(false)),
                ])
            })
            .collect(),
    )
}

/// The flags `selection draft` accepts; dispatch refuses any other (ADR
/// 0097). `set` is [`super::selection_set::FLAGS`], `review`
/// [`super::selection_review::FLAGS`].
pub const DRAFT_FLAGS: Flags = Flags {
    boolean: &["force", "dry-run", "json", "links", "envelopes"],
    valued: &["op"],
};

pub fn main(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("review") => return super::selection_review::main(&argv[1..]),
        Some("set") => return super::selection_set::main(&argv[1..]),
        Some("draft") => {}
        Some(other) => {
            return usage(&format!(
                "unknown subcommand {:?} (expected: draft, set or review)",
                other
            ))
        }
        None => return usage("expected a subcommand: draft, set or review"),
    }
    let args = Args::parse(&argv[1..], &DRAFT_FLAGS);
    // Neither flag draws both; either one draws only its own kind; the two
    // together contradict each other, so the call is refused before
    // anything is read.
    if args.has("links") && args.has("envelopes") {
        return usage(
            "--links and --envelopes exclude each other: give one, or neither to draft both",
        );
    }
    let want_envelopes = args.has("envelopes") || !args.has("links");
    let want_links = args.has("links") || !args.has("envelopes");
    let dir = Path::new(&args.dir()).to_path_buf();
    let text = match crate::factory_io::read_to_string(&dir, SELECTION_FILE) {
        Ok(t) => t,
        // Draft proposes envelopes for the operations the selection already
        // includes; which to include, their roots and names are the select
        // verb's judgements, never the tool's (ADR 0018 § 5).
        Err(e) if e.is_not_found() => {
            eprintln!(
                "selection: no {} yet. `selection draft` proposes a response.envelope (and links: entries) for the operations it already marks `include: true`; it does not choose them. Write it first (the select verb: the operations to include, each with its graphql root and name), then run draft.",
                SELECTION_FILE
            );
            return 1;
        }
        Err(e) => {
            eprintln!("selection: {}", e);
            return 1;
        }
    };
    let selection = match crate::yaml::parse(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("selection: {}: {}", SELECTION_FILE, e);
            return 1;
        }
    };
    let inventory = match crate::factory_io::read_to_string(&dir, ".factory/inventory.json")
        .map_err(String::from)
        .and_then(|t| crate::json::parse(&t).map_err(|e| format!(".factory/inventory.json: {}", e)))
    {
        Ok(i) => i,
        Err(e) => {
            eprintln!("selection: {}", e);
            return 1;
        }
    };
    let ops = crate::json::get_arr(&inventory, "operations")
        .cloned()
        .unwrap_or_default();
    let wanted: Vec<String> = args.all("op").iter().map(|s| s.to_string()).collect();
    let force = args.has("force");

    let sel_ops = get_obj(&selection, "operations")
        .cloned()
        .unwrap_or_default();
    let mut unknown: Vec<String> = wanted
        .iter()
        .filter(|k| !sel_ops.contains_key(k.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        eprintln!(
            "selection: {} is not an operation in {} — draft only touches operations the selection lists",
            unknown.join(", "),
            SELECTION_FILE
        );
        return 1;
    }

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut plans: Vec<Plan> = Vec::new();
    let mut skipped: Vec<(String, &'static str)> = Vec::new();

    // Splice from the bottom up so earlier line numbers stay valid.
    let mut targets: Vec<String> = if want_envelopes {
        sel_ops
            .iter()
            .filter(|(k, e)| (wanted.is_empty() || wanted.contains(k)) && truthy(get(e, "include")))
            .map(|(k, _)| k.clone())
            .collect()
    } else {
        Vec::new()
    };
    if want_envelopes {
        for k in &wanted {
            if !targets.contains(k) {
                skipped.push((k.clone(), "not included by the selection"));
            }
        }
    }
    let mut positioned: Vec<(usize, String)> = Vec::new();
    {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        for k in targets.drain(..) {
            match find_entry(&refs, &k) {
                Some(e) => positioned.push((e.line, k)),
                None => skipped.push((k, "its entry is not a block mapping in the file")),
            }
        }
    }
    // Bottom of the file first, so earlier line numbers stay valid.
    positioned.sort_by_key(|(line, _)| std::cmp::Reverse(*line));

    for (_, key) in &positioned {
        let op = match ops.iter().find(|o| get_str(o, "key") == Some(key.as_str())) {
            Some(o) => o,
            None => {
                skipped.push((key.clone(), "not in inventory.json"));
                continue;
            }
        };
        let suggestion = crate::envelope::suggest_envelope(get(op, "response"));
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let entry = match find_entry(&refs, key) {
            Some(e) => e,
            None => continue,
        };
        let existing = response_block(&refs, &entry);
        let was = existing.map(|_| {
            sel_ops
                .get(key)
                .and_then(|e| get(e, "response"))
                .and_then(|r| get_str(r, "envelope"))
                .map(str::to_string)
        });
        if existing.is_some() && !force {
            skipped.push((
                key.clone(),
                "already has a response block (--force replaces it)",
            ));
            continue;
        }
        let new_lines = block(entry.child_indent, suggestion.as_deref());
        match existing {
            Some((start, end)) => {
                lines.splice(start..end, new_lines);
            }
            None => {
                lines.splice(entry.line + 1..entry.line + 1, new_lines);
            }
        }
        plans.push(Plan {
            key: key.clone(),
            envelope: suggestion,
            was,
            action: if existing.is_some() {
                "replaced"
            } else {
                "added"
            },
        });
    }
    plans.reverse();

    // ── Links (ADR 0069). The fact is read wherever it sits in the response
    // shape; the judgement goes to the top-level `links:` list, never into
    // an operation's entry, so a flow-style entry (ADR 0027) is no obstacle.
    // Only a fact worth confirming is drafted: its host shape must be
    // returned by an included operation — directly, or reached from that
    // operation's response shape through `$ref` (R31) — and its by-id
    // operation must itself be included: that operation is the field's
    // provenance. A stale fact (a non-GET operation from a pre-PR-A
    // inventory, one the inventory no longer has, or a shape's own id
    // pointing at the operation that returns it) is skipped with a reason
    // rather than left for the schema check to turn into an exit 1. A fact
    // whose `<shape> > <path>` already has an entry is skipped in every
    // state; `--force` replaces only an entry still marked
    // `confirmed: false` and included — a confirmed or declined one is the
    // user's answer.
    let mut link_plans: Vec<LinkPlan> = Vec::new();
    if want_links {
        let existing_links: Vec<Value> = crate::json::get_arr(&selection, "links")
            .cloned()
            .unwrap_or_default();
        // A replacement is spliced by index, so the walker must see exactly
        // the entries the parser does; when it does not (a flow sequence on
        // the line after `links:`), nothing is spliced at all.
        let (flow_links, miscounted) = {
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            match links_section(&refs) {
                None => (get(&selection, "links").is_some(), false),
                Some(section) => (
                    false,
                    link_entries(&refs, section).len() != existing_links.len(),
                ),
            }
        };
        let included = |k: &str| {
            sel_ops
                .get(k)
                .map(|e| truthy(get(e, "include")))
                .unwrap_or(false)
        };
        let shapes = get_obj(&inventory, "shapes").cloned().unwrap_or_default();
        let returned: Vec<String> = crate::inventory::shape_closure(
            ops.iter()
                .filter(|o| get_str(o, "key").map(included).unwrap_or(false))
                .filter_map(|o| get(o, "response").and_then(|r| get_str(r, "shape_ref")))
                .map(|r| crate::inventory::shape_name(r).to_string()),
            &shapes,
        );
        let mut candidates = crate::cmd::inventory_links::candidate_links(&inventory);
        // Shape, then path: the order the file grows in on every run.
        candidates.sort_by(|a, b| {
            (a.shape.as_str(), a.path.as_str()).cmp(&(b.shape.as_str(), b.path.as_str()))
        });
        // Which facts survive, and the entry each one would replace.
        let mut surviving: Vec<(CandidateLink, Option<usize>)> = Vec::new();
        for c in candidates {
            if !wanted.is_empty() && !wanted.contains(&c.operation) {
                continue;
            }
            let link_key = format!("{} > {}", c.shape, c.path);
            if flow_links {
                skipped.push((link_key, "links: is not a block sequence in the file"));
                continue;
            }
            if miscounted {
                skipped.push((
                    link_key,
                    "links: section could not be indexed line by line (entry count mismatch); nothing written",
                ));
                continue;
            }
            if !c.operation.starts_with("get:") {
                skipped.push((
                    link_key,
                    "its operation is not a GET (regenerate inventory.json)",
                ));
                continue;
            }
            if !ops
                .iter()
                .any(|o| get_str(o, "key") == Some(c.operation.as_str()))
            {
                skipped.push((link_key, "not in inventory.json"));
                continue;
            }
            if c.returned_by.iter().any(|k| k == &c.operation) {
                skipped.push((link_key, "its operation returns the shape itself"));
                continue;
            }
            if !returned.contains(&c.shape) {
                skipped.push((link_key, "no included operation returns the shape"));
                continue;
            }
            if !included(&c.operation) {
                skipped.push((
                    link_key,
                    "its by-id operation is not included by the selection",
                ));
                continue;
            }
            let existing = existing_links.iter().position(|l| {
                get_str(l, "shape") == Some(c.shape.as_str())
                    && get_str(l, "path") == Some(c.path.as_str())
            });
            if let Some(i) = existing {
                if get(&existing_links[i], "include") == Some(&Value::Bool(false)) {
                    skipped.push((
                        link_key,
                        "already has a declined links entry (include: false); nothing written",
                    ));
                    continue;
                }
                if get(&existing_links[i], "confirmed") != Some(&Value::Bool(false)) {
                    skipped.push((
                        link_key,
                        "already has a confirmed links entry (--force replaces only a draft)",
                    ));
                    continue;
                }
                if !force {
                    skipped.push((link_key, "already has a links entry (--force replaces it)"));
                    continue;
                }
            }
            surviving.push((c, existing));
        }
        // Field names are unique per (shape, object) among included links,
        // exactly as lint's link-duplicate-field counts them: every included
        // entry this run leaves in place holds its name (a declined one holds
        // none), and each drafted entry claims its own as it is written.
        let replaced: Vec<(String, String)> = surviving
            .iter()
            .filter(|(_, existing)| existing.is_some())
            .map(|(c, _)| (c.shape.clone(), c.path.clone()))
            .collect();
        let mut taken: Vec<(String, String, String)> = crate::reconcile::read_links(&selection)
            .iter()
            .filter(|l| l.include && !replaced.contains(&(l.shape.clone(), l.path.clone())))
            .map(|l| (l.shape.clone(), l.object_key(), l.field_name()))
            .collect();
        let mut replacements: Vec<(usize, Vec<String>)> = Vec::new();
        let mut appended: Vec<Vec<String>> = Vec::new();
        for (c, existing) in surviving {
            let link = proposed(&c);
            let derived = link.field_name();
            let object = link.object_key();
            let field = distinct_field(&derived, &c.path, |f| {
                taken
                    .iter()
                    .any(|(s, o, t)| s == &c.shape && o == &object && t == f)
            });
            taken.push((c.shape.clone(), object, field.clone()));
            match existing {
                Some(i) => replacements.push((i, link_block(&c, &field))),
                None => appended.push(link_block(&c, &field)),
            }
            link_plans.push(LinkPlan {
                shape: c.shape.clone(),
                path: c.path.clone(),
                field,
                operation: c.operation.clone(),
                action: if existing.is_some() {
                    "replaced"
                } else {
                    "added"
                },
            });
        }
        if !replacements.is_empty() || !appended.is_empty() {
            splice_links(&mut lines, replacements, appended);
        }
    }
    skipped.sort();

    if plans.is_empty() && link_plans.is_empty() {
        if args.has("json") {
            print!(
                "{}",
                crate::json::pretty(&crate::json::object(vec![
                    ("file", Value::from(SELECTION_FILE)),
                    ("dry_run", Value::Bool(args.has("dry-run"))),
                    ("drafted", Value::Array(vec![])),
                    ("links", Value::Array(vec![])),
                    ("skipped", skipped_value(&skipped)),
                ]))
            );
        } else {
            println!("selection draft: nothing to do — every included operation already has a response block and every candidate link already has a links entry or is skipped below (--force redraws a response block, and a links entry only while it is a draft)");
            for (k, why) in &skipped {
                println!("  · {}   ({})", k, why);
            }
        }
        return 2;
    }

    let out = format!("{}\n", lines.join("\n"));
    // Never write a file that does not parse, and never one lint would
    // reject as a contract error.
    let reparsed = match crate::yaml::parse(&out) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "selection: refused — the spliced {} does not parse ({}); nothing was written",
                SELECTION_FILE, e
            );
            return 1;
        }
    };
    if let Some(schema) = crate::schemas::load("selection.schema.json", None) {
        let errors = crate::jsonschema::validate(&reparsed, &schema);
        if !errors.is_empty() {
            eprintln!(
                "selection: refused — the spliced {} does not satisfy selection.schema.json; nothing was written:",
                SELECTION_FILE
            );
            for e in errors.iter().take(10) {
                eprintln!("  contract: {}", e);
            }
            return 1;
        }
    }
    if !args.has("dry-run") {
        if let Err(e) = crate::factory_io::write_in_place(&dir, SELECTION_FILE, out.as_bytes()) {
            eprintln!("selection: {}", e);
            return 1;
        }
    }

    if args.has("json") {
        let drafted: Vec<Value> = plans
            .iter()
            .map(|p| {
                crate::json::object(vec![
                    ("key", Value::from(p.key.as_str())),
                    ("action", Value::from(p.action)),
                    (
                        "envelope",
                        p.envelope.clone().map(Value::from).unwrap_or(Value::Null),
                    ),
                    (
                        "was",
                        match &p.was {
                            None => Value::Null,
                            Some(None) => Value::Null,
                            Some(Some(e)) => Value::from(e.as_str()),
                        },
                    ),
                    ("confirmed", Value::Bool(false)),
                ])
            })
            .collect();
        print!(
            "{}",
            crate::json::pretty(&crate::json::object(vec![
                ("file", Value::from(SELECTION_FILE)),
                ("dry_run", Value::Bool(args.has("dry-run"))),
                ("drafted", Value::Array(drafted)),
                ("links", links_value(&link_plans)),
                ("skipped", skipped_value(&skipped)),
            ]))
        );
        return 0;
    }

    println!(
        "selection draft: {} {} in {}{}",
        describe(plans.len(), link_plans.len()),
        if args.has("dry-run") {
            "would be written"
        } else {
            "written"
        },
        SELECTION_FILE,
        if args.has("dry-run") {
            " (dry run)"
        } else {
            ""
        }
    );
    for p in &plans {
        println!(
            "  {} {}   envelope: {}{}",
            if p.action == "added" { "+" } else { "~" },
            p.key,
            p.envelope.as_deref().unwrap_or("null"),
            match &p.was {
                Some(was) => format!("   (was {})", was.as_deref().unwrap_or("null")),
                None => String::new(),
            }
        );
    }
    for l in &link_plans {
        println!(
            "  {} link {} > {}   field: {} via {}",
            if l.action == "added" { "+" } else { "~" },
            l.shape,
            l.path,
            l.field,
            l.operation
        );
    }
    for (k, why) in &skipped {
        println!("  · {}   ({})", k, why);
    }
    println!();
    println!(
        "Every block and every links entry is `confirmed: false`: the envelope and the relationship are judgements, and this is only the tool's proposal from the inventory's facts. Confirm each one with the user, then set `confirmed: true` (or drop it). An unconfirmed link is a reconcile note and a lint warning, never drift; the field is applied only once confirmed (connectors-language.md § Relationship fields)."
    );
    0
}

fn skipped_value(skipped: &[(String, &'static str)]) -> Value {
    Value::Array(
        skipped
            .iter()
            .map(|(k, why)| {
                crate::json::object(vec![
                    ("key", Value::from(k.as_str())),
                    ("reason", Value::from(*why)),
                ])
            })
            .collect(),
    )
}

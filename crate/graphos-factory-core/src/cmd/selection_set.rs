//! selection set — flip one judgement in selection.yaml without touching the rest.
//!
//!   selection set [workspace] --op KEY… --include true|false [--dry-run] [--json]
//!
//! `inventory.json` records facts; `selection.yaml` records judgements (ADR
//! 0018). Whether an operation is exposed is a judgement — its `include:`
//! flag — and this is the one-toggle verb a UI drives so a person can turn an
//! operation on or off without hand-editing YAML or regenerating the file.
//!
//! Each named operation's `include` value is replaced in place. An entry
//! written as a block mapping has its whole `include:` line rewritten; an
//! entry written as a single-line flow mapping — `"get:/x": { include: false,
//! reason: "…" }`, the form the pilots' excluded operations use — has only the
//! bytes of that one value replaced. Either way every other line, byte,
//! comment and anchor is left exactly as it was. The result is parsed and
//! validated against `selection.schema.json` before it is written; if it does
//! not parse or validate, nothing is written. Only operations the selection
//! already lists are touched — adding a new operation is a larger edit than a
//! toggle and is out of scope here.
//!
//! Exit codes: 0 written · 1 usage, an unreadable workspace, an `--op` the
//! selection does not list, or a structural failure on any target (its entry is
//! neither a block mapping nor a single-line flow mapping, has no `include`,
//! spans more than one flow line, or is declared twice) — refused, nothing
//! written · 2 nothing to do (every named operation already has the requested
//! value).

use super::selection::{find_entry, indent_of, key_line, SELECTION_FILE};
use crate::args::{Args, Flags};
use crate::json::{get, get_obj, truthy};
use serde_json::Value;
use std::path::Path;

/// The usage text: `selection set --help` prints it on stdout (ADR 0086).
pub const USAGE: &str =
    "usage: selection set [workspace] --op KEY… --include true|false [--dry-run] [--json]";

fn usage(msg: &str) -> i32 {
    eprintln!("selection set: {}", msg);
    eprintln!("{}", USAGE);
    1
}

/// Rewrite an `include:` line to a new boolean, preserving indent and any
/// trailing comment. The value is a bare boolean, so the first `#` after the
/// colon is unambiguously a comment.
fn set_include_line(line: &str, value: bool) -> String {
    let indent = indent_of(line);
    let pad = &line[..indent];
    let after_colon = line[indent..]
        .find(':')
        .map(|c| indent + c + 1)
        .unwrap_or(line.len());
    match line[after_colon..].find('#') {
        Some(hash) => format!(
            "{}include: {}   {}",
            pad,
            value,
            &line[after_colon + hash..]
        ),
        None => format!("{}include: {}", pad, value),
    }
}

/// Where one target's `include` value lives in the file.
enum Site {
    /// A block mapping: the whole `include:` line is rewritten.
    Line(usize),
    /// A single-line flow mapping: only the value's byte span is replaced, so
    /// the braces, the spacing, the reason and its quoting survive untouched.
    Span(usize, usize, usize),
}

/// The line range that `operations:` covers, as `[start, end)` over its
/// children — the same region `find_entry` walks — with the indent its own
/// keys sit at, so a deeper nested key cannot be mistaken for an entry.
fn operations_region(lines: &[&str]) -> Option<(usize, usize, usize)> {
    let start = lines
        .iter()
        .position(|l| indent_of(l) == 0 && key_line(l).as_deref() == Some("operations"))?;
    let mut end = start + 1;
    let mut key_indent = None;
    while end < lines.len() {
        let t = lines[end].trim();
        if !t.is_empty() && !t.starts_with('#') {
            if indent_of(lines[end]) == 0 {
                break;
            }
            key_indent.get_or_insert(indent_of(lines[end]));
        }
        end += 1;
    }
    Some((start + 1, end, key_indent.unwrap_or(2)))
}

/// The byte index just past the closing quote of a scalar starting at `i`
/// (which must be `"` or `'`), or None when the quote never closes on the line.
fn end_of_quoted(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() {
        if q == b'"' && b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == q {
            if q == b'\'' && b.get(j + 1) == Some(&b'\'') {
                j += 2;
                continue;
            }
            return Some(j + 1);
        }
        j += 1;
    }
    None
}

fn unquote(token: &str) -> &str {
    let t = token.trim();
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return &t[1..t.len() - 1];
        }
    }
    t
}

/// The byte index just past the `:` when `line` declares `key` at its start,
/// whether the key is quoted or bare. Keys carry colons (`get:/orgs`), so a
/// bare key is matched by text rather than by splitting on the first colon.
/// A quoted key is compared raw, exactly as `key_line` compares it, so the
/// block and flow paths agree on which keys they recognise.
fn declares_key(line: &str, key: &str) -> Option<usize> {
    let indent = indent_of(line);
    let t = &line[indent..];
    let after_key = match t.as_bytes().first()? {
        q @ (b'"' | b'\'') => {
            let close = end_of_quoted(t, 0)?;
            if &t[1..close - 1] != key || *q == b'\'' && key.contains('\'') {
                return None;
            }
            indent + close
        }
        _ => {
            if !t.starts_with(key) {
                return None;
            }
            indent + key.len()
        }
    };
    if line[after_key..].starts_with(':') {
        Some(after_key + 1)
    } else {
        None
    }
}

/// What a single-line flow entry yielded: the byte span of its `include`
/// value, or why it cannot be edited.
enum Flow {
    Value(usize, usize),
    NoInclude,
    Unclosed,
}

/// Scan the flow mapping that opens at `open` for its top-level `include` key
/// and return that value's byte span. The scan is quote-aware and
/// depth-aware, so a `reason:` holding braces or the text `include:` inside a
/// quoted string, and a nested `graphql: { … }`, are all stepped over rather
/// than matched.
fn flow_include(line: &str, open: usize) -> Flow {
    let b = line.as_bytes();
    let mut i = open + 1;
    let mut depth = 1usize;
    let mut tok: Option<usize> = None;
    let mut expect_value = false;
    let mut value_start: Option<usize> = None;
    let mut span: Option<(usize, usize)> = None;
    // The value token runs to the delimiter; trailing spaces are not part of it.
    let close = |start: usize, at: usize| (start, line[start..at].trim_end().len() + start);
    while i < b.len() {
        let c = b[i];
        if !c.is_ascii_whitespace() && c != b',' && c != b'}' && c != b']' && depth == 1 {
            if tok.is_none() {
                tok = Some(i);
            }
            if expect_value {
                value_start = Some(i);
                expect_value = false;
            }
        }
        match c {
            b'"' | b'\'' => match end_of_quoted(line, i) {
                Some(next) => i = next,
                None => return Flow::Unclosed,
            },
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = value_start.take() {
                        span = Some(close(start, i));
                    } else if expect_value {
                        return Flow::NoInclude;
                    }
                    return match span {
                        Some((s, e)) if e > s => Flow::Value(s, e),
                        _ => Flow::NoInclude,
                    };
                }
                i += 1;
            }
            b',' if depth == 1 => {
                if let Some(start) = value_start.take() {
                    span = Some(close(start, i));
                } else if expect_value {
                    return Flow::NoInclude;
                }
                tok = None;
                i += 1;
            }
            b':' if depth == 1 && value_start.is_none() && !expect_value => {
                if let Some(start) = tok {
                    if unquote(&line[start..i]) == "include" && span.is_none() {
                        expect_value = true;
                    }
                }
                tok = None;
                i += 1;
            }
            _ => i += 1,
        }
    }
    Flow::Unclosed
}

/// Resolve one operation key to the site of its `include` value, or to the
/// reason it is refused.
fn locate(lines: &[&str], key: &str) -> Result<Site, String> {
    let (start, end, key_indent) = match operations_region(lines) {
        Some(r) => r,
        None => return Err("the file has no operations: block".into()),
    };
    // A key declared twice never reaches here: `crate::yaml::parse` refuses a
    // duplicate mapping key outright, so the call was refused before this.
    //
    // A block mapping keeps the original path: rewrite the whole `include:`
    // line, indent, trailing comment and all.
    if let Some(entry) = find_entry(lines, key) {
        // `include:` is a scalar assignment (`include: true`), so match the key
        // directly rather than with key_line (which is for block keys). The
        // indent guard skips a nested `fields.include:` list one level deeper.
        return (entry.line + 1..entry.end)
            .find(|&i| {
                indent_of(lines[i]) == entry.child_indent
                    && lines[i].trim_start().starts_with("include:")
            })
            .map(Site::Line)
            .ok_or_else(|| "its entry has no include: line to set".into());
    }
    // Otherwise the entry may be a flow mapping, which `key_line` — and so
    // `find_entry` — cannot see, because content follows the colon.
    let found = (start..end).find_map(|i| {
        let t = lines[i].trim_start();
        if t.is_empty() || t.starts_with('#') || indent_of(lines[i]) != key_indent {
            return None;
        }
        declares_key(lines[i], key).map(|after| (i, after))
    });
    let (line, after_colon) = match found {
        Some(f) => f,
        None => {
            return Err(
                "its entry is neither a block mapping nor a single-line flow mapping".into(),
            )
        }
    };
    let rest = &lines[line][after_colon..];
    let open = match rest.find(|c: char| !c.is_whitespace()) {
        Some(o) if rest.as_bytes()[o] == b'{' => after_colon + o,
        _ => {
            return Err(
                "its entry is neither a block mapping nor a single-line flow mapping".into(),
            )
        }
    };
    match flow_include(lines[line], open) {
        Flow::Value(s, e) => Ok(Site::Span(line, s, e)),
        Flow::NoInclude => Err("its flow mapping has no include: value to set".into()),
        Flow::Unclosed => Err(
            "its flow mapping does not close on one line — put the whole { … } entry on one line, or rewrite it as a block mapping, then retry"
                .into(),
        ),
    }
}

struct Change {
    key: String,
    from: bool,
    to: bool,
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["dry-run", "json"],
    valued: &["op", "include"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let target = match args.get("include") {
        Some("true") => true,
        Some("false") => false,
        Some(other) => return usage(&format!("--include takes true or false, not {:?}", other)),
        None => return usage("--include true|false is required"),
    };
    let wanted: Vec<String> = args.all("op");
    if wanted.is_empty() {
        return usage("at least one --op KEY is required");
    }

    let dir = Path::new(&args.dir()).to_path_buf();
    let text = match crate::factory_io::read_to_string(&dir, SELECTION_FILE) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("selection set: {}", e);
            return 1;
        }
    };
    let selection = match crate::yaml::parse(&text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("selection set: {}: {}", SELECTION_FILE, e);
            return 1;
        }
    };
    let sel_ops = get_obj(&selection, "operations")
        .cloned()
        .unwrap_or_default();

    let mut unknown: Vec<String> = wanted
        .iter()
        .filter(|k| !sel_ops.contains_key(k.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    unknown.dedup();
    if !unknown.is_empty() {
        if args.has("json") {
            // Match the structural-error refusal: a machine caller gets the same
            // `report()` envelope with the offending keys in `errors`, never a
            // bare stderr line it cannot parse.
            let errors: Vec<(String, String)> = unknown
                .iter()
                .map(|k| {
                    (
                        k.clone(),
                        format!(
                            "not an operation in {} — set only toggles operations it already lists",
                            SELECTION_FILE
                        ),
                    )
                })
                .collect();
            print!("{}", report(&[], &[], &errors, args.has("dry-run")));
        } else {
            eprintln!(
                "selection set: {} is not an operation in {} — set only toggles operations the selection already lists",
                unknown.join(", "),
                SELECTION_FILE
            );
        }
        return 1;
    }

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut changes: Vec<Change> = Vec::new();
    // A benign skip is nothing-to-do (`already <value>`); a structural error is a
    // real failure (an entry that is not a block mapping, or has no include line)
    // and is refused with exit 1 — never downgraded to nothing-to-do.
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut errors: Vec<(String, String)> = Vec::new();

    // Resolve each target to the site of its include value first, then apply
    // from the bottom up so earlier line numbers stay valid.
    let mut edits: Vec<(usize, Site, String, bool)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for key in &wanted {
        if seen.contains(key) {
            continue;
        }
        seen.push(key.clone());
        // Resolve every target's structure first — that its entry is an
        // editable mapping carrying an `include` — before deciding whether the
        // change is a no-op. A structural failure is refused with exit 1 in
        // either direction and must never be masked by the "already at the
        // requested value" skip below: a missing `include` reads as `false`,
        // which would otherwise swallow `--include false` as nothing-to-do
        // (exit 2) instead of the refusal the contract promises.
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let site = match locate(&refs, key) {
            Ok(s) => s,
            Err(why) => {
                errors.push((key.clone(), why));
                continue;
            }
        };
        // Structure is sound and the include value is located: only now is a
        // matching current value a genuine nothing-to-do skip.
        let from = sel_ops
            .get(key)
            .map(|e| truthy(get(e, "include")))
            .unwrap_or(false);
        if from == target {
            skipped.push((key.clone(), format!("already {}", target)));
            continue;
        }
        let at = match site {
            Site::Line(i) => i,
            Site::Span(i, _, _) => i,
        };
        edits.push((at, site, key.clone(), from));
    }

    // A structural failure on any target is a real error: write nothing and
    // refuse, rather than downgrading it to a partial success or nothing-to-do.
    errors.sort();
    if !errors.is_empty() {
        if args.has("json") {
            print!("{}", report(&[], &skipped, &errors, args.has("dry-run")));
        } else {
            eprintln!(
                "selection set: refused — {} operation{} cannot be set; nothing was written:",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            );
            for (k, why) in &errors {
                eprintln!("  ✗ {}   ({})", k, why);
            }
        }
        return 1;
    }

    edits.sort_by_key(|(i, _, _, _)| std::cmp::Reverse(*i));
    for (i, site, key, from) in edits {
        match site {
            Site::Line(_) => lines[i] = set_include_line(&lines[i], target),
            // Byte-for-byte splice: only the value token changes, so the
            // braces, spacing, ordering, reason and its quoting all survive.
            Site::Span(_, s, e) => lines[i].replace_range(s..e, &target.to_string()),
        }
        changes.push(Change {
            key,
            from,
            to: target,
        });
    }
    changes.sort_by(|a, b| a.key.cmp(&b.key));
    skipped.sort();

    if changes.is_empty() {
        if args.has("json") {
            print!("{}", report(&changes, &skipped, &[], args.has("dry-run")));
        } else {
            println!(
                "selection set: nothing to do — every named operation already has include: {}",
                target
            );
            for (k, why) in &skipped {
                println!("  · {}   ({})", k, why);
            }
        }
        return 2;
    }

    let out = format!("{}\n", lines.join("\n"));
    let reparsed = match crate::yaml::parse(&out) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "selection set: refused — the edited {} does not parse ({}); nothing was written",
                SELECTION_FILE, e
            );
            return 1;
        }
    };
    if let Some(schema) = crate::schemas::load("selection.schema.json", None) {
        let errors = crate::jsonschema::validate(&reparsed, &schema);
        if !errors.is_empty() {
            eprintln!(
                "selection set: refused — the edited {} does not satisfy selection.schema.json; nothing was written:",
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
            eprintln!("selection set: {}", e);
            return 1;
        }
    }

    if args.has("json") {
        print!("{}", report(&changes, &skipped, &[], args.has("dry-run")));
        return 0;
    }

    println!(
        "selection set: include set for {} operation{} in {}{}",
        changes.len(),
        if changes.len() == 1 { "" } else { "s" },
        SELECTION_FILE,
        if args.has("dry-run") {
            " (dry run)"
        } else {
            ""
        }
    );
    for c in &changes {
        println!("  ~ {}   include: {} (was {})", c.key, c.to, c.from);
    }
    for (k, why) in &skipped {
        println!("  · {}   ({})", k, why);
    }
    0
}

fn report(
    changes: &[Change],
    skipped: &[(String, String)],
    errors: &[(String, String)],
    dry_run: bool,
) -> String {
    let changed: Vec<Value> = changes
        .iter()
        .map(|c| {
            crate::json::object(vec![
                ("key", Value::from(c.key.as_str())),
                ("field", Value::from("include")),
                ("from", Value::Bool(c.from)),
                ("to", Value::Bool(c.to)),
            ])
        })
        .collect();
    let reasons = |items: &[(String, String)]| {
        Value::Array(
            items
                .iter()
                .map(|(k, why)| {
                    crate::json::object(vec![
                        ("key", Value::from(k.as_str())),
                        ("reason", Value::from(why.as_str())),
                    ])
                })
                .collect(),
        )
    };
    crate::json::pretty(&crate::json::object(vec![
        ("file", Value::from(SELECTION_FILE)),
        ("dry_run", Value::Bool(dry_run)),
        ("changed", Value::Array(changed)),
        ("skipped", reasons(skipped)),
        ("errors", reasons(errors)),
    ]))
}

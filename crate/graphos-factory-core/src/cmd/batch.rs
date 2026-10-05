//! batch — bulk-by-keys lookups for keyed types (`crate::batch`).
//!
//!   batch find [workspace] [--json] [--all-types] [--check]
//!
//! Reports, per type the selection keys (`graphql.entity: true`), every
//! read that returns an array of it and whether that read takes a list of
//! its key, with a verdict: `batchable`, `partial-shape`, `needs-scope`,
//! `style-conflict`, `paginated`, `list-no-key-filter` or `none`
//! (`crate::batch`). `--all-types` adds every response object type with an
//! id-like field. It reads `.factory/inventory.json` and
//! `.factory/selection.yaml` through custody (ADR 0025) and the schema named
//! by `workspace.yaml`; a schema file that does not exist yet has no
//! connectors.
//!
//! Exit codes: 0 reported; 1 with `--check`, a keyed type is batchable but
//! the schema has no `$batch` connector for it; 2 the workspace cannot be
//! read, or its schema file exists but cannot be read or parsed.

use crate::args::{Args, Flags};
use crate::batch::{self, TypeReport, Verdict};
use serde_json::{json, Value};
use std::path::Path;

fn usage(msg: &str) -> i32 {
    eprintln!(
        "batch: {}\nusage: graphos-factory-core batch find [workspace] [--json] [--all-types] [--check]",
        msg
    );
    2
}

pub fn main(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("find") => find(&argv[1..]),
        Some(other) => usage(&format!("unknown subcommand {:?} (expected: find)", other)),
        None => usage("expected a subcommand: find"),
    }
}

/// The inventory, the selection (when present) and the schema text (empty
/// when the workspace names none or the file does not exist yet; an error
/// when it exists and cannot be read).
fn load(dir: &Path) -> Result<(Value, Option<Value>, String), String> {
    let inventory = crate::factory_io::read_to_string(dir, ".factory/inventory.json")
        .map_err(String::from)
        .and_then(|t| {
            crate::json::parse(&t).map_err(|e| format!(".factory/inventory.json: {}", e))
        })?;
    let selection =
        match crate::factory_io::read_to_string_optional(dir, ".factory/selection.yaml")? {
            Some(text) => Some(
                crate::yaml::parse(&text).map_err(|e| format!(".factory/selection.yaml: {}", e))?,
            ),
            None => None,
        };
    // The schema is an ordinary workspace file (ADR 0025), named by
    // workspace.yaml's `directory`.
    let sdl = match crate::factory_io::read_to_string_optional(dir, ".factory/workspace.yaml")? {
        Some(text) => {
            let workspace =
                crate::yaml::parse(&text).map_err(|e| format!(".factory/workspace.yaml: {}", e))?;
            let schema = crate::reconcile::schema_file_of(&workspace)?;
            // A workspace with no schema yet is an empty one; anything else
            // that goes wrong reading it is an error (ADR 0075).
            if std::fs::symlink_metadata(dir.join(&schema)).is_err() {
                String::new()
            } else {
                crate::reconcile::read_schema_file(dir, &workspace)?.1
            }
        }
        None => String::new(),
    };
    Ok((inventory, selection, sdl))
}

pub fn report_json(r: &TypeReport) -> Value {
    json!({
        "type": r.type_name,
        "key": r.key,
        "keyed_by": r.keyed_by,
        "verdict": r.verdict.label(),
        "has_batch_connector": r.has_batch_connector,
        "batch": r.batch,
        "note": r.note,
        "draft": shown_draft(r),
        "draft_note": shown_draft_note(r),
        "candidates": r.candidates.iter().map(|c| json!({
            "operation": c.operation,
            "item_shape": c.item_shape,
            "via": c.via,
            "key_param": c.key_param.as_ref().map(|k| json!({
                "name": k.name,
                "in": k.location,
                "passing": k.passing,
                "max_size": k.max_size,
                "companions": k.companions,
            })),
            "problems": c.problems.iter().map(|p| json!({ "kind": p.kind, "detail": p.detail })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// The connector draft, shown only for a type the selection says to batch
/// (`graphql.batch: true`) that has none yet: the agent pastes it; no
/// instrument writes the schema (plan.md §3.1).
fn shown_draft(r: &TypeReport) -> Option<String> {
    if r.batch == Some(true) && !r.has_batch_connector {
        r.draft.clone()
    } else {
        None
    }
}

/// Why no draft was shown, under the same gate as the draft itself: a type
/// that already has its `$batch` connector needs neither.
fn shown_draft_note(r: &TypeReport) -> Option<String> {
    if r.batch == Some(true) && !r.has_batch_connector && r.draft.is_none() {
        r.draft_note.clone()
    } else {
        None
    }
}

/// The one-line summary: how many types were looked at and each verdict.
pub fn summary(reports: &[TypeReport]) -> Value {
    let count = |v: Verdict| reports.iter().filter(|r| r.verdict == v).count();
    json!({
        "types": reports.len(),
        "keyed": reports.iter().filter(|r| r.keyed_by.is_some()).count(),
        "batchable": count(Verdict::Batchable),
        "partial_shape": count(Verdict::PartialShape),
        "needs_scope": count(Verdict::NeedsScope),
        "style_conflict": count(Verdict::StyleConflict),
        "paginated": count(Verdict::Paginated),
        "list_no_key_filter": count(Verdict::ListNoKeyFilter),
        "none": count(Verdict::None),
        "missing_batch_connector": batch::missing_batch(reports).len(),
    })
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FIND_FLAGS: Flags = Flags {
    boolean: &["json", "all-types", "check"],
    valued: &[],
};

fn find(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FIND_FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    let (inventory, selection, sdl) = match load(&dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("batch: {}", e);
            return 2;
        }
    };
    let reports = match batch::find(&inventory, selection.as_ref(), &sdl, args.has("all-types")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("batch: the schema does not parse, so whether a $batch connector exists is unknown:\n{}", e);
            return 2;
        }
    };
    let summary = summary(&reports);

    if args.has("json") {
        let out = json!({
            "types": reports.iter().map(report_json).collect::<Vec<_>>(),
            "summary": summary,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        for r in &reports {
            println!(
                "{} (key {}{}): {}{}",
                r.type_name,
                r.key,
                r.keyed_by
                    .as_ref()
                    .map(|k| format!(", keyed by {}", k))
                    .unwrap_or_default(),
                r.verdict.label(),
                if r.has_batch_connector {
                    " · has $batch"
                } else {
                    ""
                }
            );
            if let Some(note) = &r.note {
                println!("  note: {}", note);
            }
            match r.batch {
                Some(true) => println!("  selection: batch: true"),
                Some(false) => println!("  selection: batch: false (declined)"),
                None => {}
            }
            for c in &r.candidates {
                match &c.key_param {
                    Some(k) => println!(
                        "  {} -> [{}] ({}): {} {} {}{}",
                        c.operation,
                        c.item_shape,
                        c.via,
                        k.location,
                        k.name,
                        k.passing,
                        k.max_size
                            .map(|m| format!(", max {}", m))
                            .unwrap_or_default()
                    ),
                    None => println!(
                        "  {} -> [{}] ({}): no key-list parameter",
                        c.operation, c.item_shape, c.via
                    ),
                }
                for p in &c.problems {
                    println!("      {}: {}", p.kind, p.detail);
                }
            }
        }
        for r in &reports {
            if let Some(draft) = shown_draft(r) {
                println!(
                    "\n# draft for {} — paste into the schema, then audit:\n{}",
                    r.type_name, draft
                );
            } else if let Some(why) = shown_draft_note(r) {
                println!("\n# no draft for {}: {}", r.type_name, why);
            }
        }
        println!("{}", serde_json::to_string(&summary).unwrap());
    }

    if args.has("check") {
        let missing = batch::missing_batch(&reports);
        if !missing.is_empty() {
            for r in missing {
                eprintln!(
                    "batch: {} is keyed and batchable ({}) but the schema has no $batch connector for it",
                    r.type_name,
                    r.candidates
                        .iter()
                        .filter(|c| c.key_param.is_some())
                        .map(|c| c.operation.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            return 1;
        }
    }
    0
}

//! lock — record (or check) the schema as the agent last wrote it.
//!
//!   lock [workspace-dir] [--check [--provenance]] [--json] [--skill-dir DIR] [--model MODEL]
//!
//! Writes .factory/applied.lock.yaml: one SHA-256 per schema span, one
//! content hash per pinned source document (sources.lock.yaml entries of kind
//! openapi/swagger), and one for `.factory/inventory.json`. Run at the end of
//! every `apply`, after the schema is what the selection asks for. `--check`
//! writes nothing and exits 3 when any span, pinned document or the inventory
//! differs from the lock (a hand edit nobody has codified yet), when a pinned
//! document's upstream copy was modified or its patches no longer reproduce
//! the working copy, or when there is no lock. It also rehashes every file
//! the lock's provenance recorded and lists the ones that differ; that
//! listing changes the exit code only with `--provenance`, because provenance
//! is traceability and the apply gate is about uncodified hand edits. The
//! listing advises a relock only when that gate is clean: the schema, the
//! pinned documents and the inventory are provenance files too, and a relock
//! over an uncodified edit would erase the only record of it. `--json` carries
//! that judgement as `relock_advisable`.
//!
//! A write prints one `lock: warning:` line to stderr for each provenance
//! value that fell back: the model recorded as `unknown`, or the skill
//! recorded as `binary-build`. Each line names the flag and the variable
//! that supply the value. The lock and the exit code are unchanged (ADR 0090).
//!
//! `selection.yaml` is read too: it declares which operation a root field
//! (or an entity type) serves when its path matches several equally (ADR
//! 0044). `load` is shared with `codify` and `source-coverage`, so all
//! three fail on a selection that does not parse.
//!
//! Exit codes: 0 written / in sync; 2 a workspace file (selection.yaml
//! included) is unreadable or does not parse, `--provenance` without
//! `--check`, or (writing) a root field's path matches several operations
//! equally and the selection does not say which — nothing is written;
//! 3 (--check) hand edits exist, such a tie exists, or there is no lock, or
//! (--provenance) a recorded provenance file differs from the lock.

use crate::args::{Args, Flags};
use crate::op_match::OpHints;
use crate::spans::{compare_with_lock, lock_document, read_lock, spans, write_lock, LOCK_FILE};
use serde_json::Value;
use std::path::Path;

pub struct Loaded {
    pub schema_file: String,
    pub sdl: String,
    pub inventory: Option<Value>,
    /// The operation `selection.yaml` declares for each root field, which
    /// settles a path several operations match equally (ADR 0044).
    pub hints: OpHints,
    /// `.factory/selection.yaml` as parsed, or `None` when there is none.
    pub selection: Option<Value>,
}

pub fn load(dir: &Path) -> Result<Loaded, String> {
    // `.factory/*` goes through custody; the schema is an ordinary workspace
    // file the user edits by hand (ADR 0025) — but only once `directory` is
    // proven not to point back inside `.factory/` itself (Phase 7az).
    let factory = |rel: &str| crate::factory_io::read_to_string(dir, rel);
    let workspace = crate::yaml::parse(&factory(".factory/workspace.yaml").map_err(String::from)?)?;
    let (schema_file, sdl) = crate::reconcile::read_schema_file(dir, &workspace)?;
    let inventory = match factory(".factory/inventory.json") {
        Ok(text) => Some(crate::json::parse(&text)?),
        // A missing inventory is normal; a refused one is not.
        Err(e) if e.is_not_found() => None,
        Err(e) => return Err(e.to_string()),
    };
    let selection = match crate::factory_io::read_to_string_optional(dir, ".factory/selection.yaml")
        .map_err(|e| e.to_string())?
    {
        Some(text) => {
            Some(crate::yaml::parse(&text).map_err(|e| format!(".factory/selection.yaml: {}", e))?)
        }
        None => None,
    };
    let hints = OpHints::from_selection(Some(&workspace), selection.as_ref(), Some(&sdl));
    Ok(Loaded {
        schema_file,
        sdl,
        inventory,
        hints,
        selection,
    })
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["check", "json", "provenance"],
    valued: &["skill-dir", "model"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    // `--provenance` only changes what `--check` enforces. Without `--check`
    // it would fall through to a write and silently relock the workspace.
    if args.has("provenance") && !args.has("check") {
        eprintln!(
            "lock: --provenance is a --check option; run `graphos-factory-core lock --check --provenance`"
        );
        return 2;
    }
    let dir = Path::new(&args.dir()).to_path_buf();
    let loaded = match load(&dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lock: {}", e);
            return 2;
        }
    };
    let current = spans(&loaded.sdl, loaded.inventory.as_ref(), &loaded.hints);
    let existing = match read_lock(&dir) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("lock: {}", e);
            return 2;
        }
    };
    let sources = match crate::sources::statuses(&dir, existing.as_ref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("lock: {}", e);
            return 2;
        }
    };
    // A pinned document is out of sync when its working copy changed or no
    // longer parses, when the lock has no entry for it, when the vendor copy
    // was touched, or when the recorded patches no longer reproduce the
    // working copy (SourceStatus::blocks_apply, shared with reconcile).
    let source_problems: Vec<&crate::sources::SourceStatus> = sources
        .iter()
        .filter(|s| s.blocks_apply(existing.is_some()))
        .collect();
    let unpinned = crate::sources::unpinned(&dir, existing.as_ref());
    // inventory.json is the tool's own output: a hand edit to it is invisible
    // to every other check, and `inventory build` would silently revert it
    // (ADR 0018).
    let inventory_edited = crate::spans::inventory_changed(&dir, existing.as_ref()) == Some(true);

    // A span keyed `Query.<f>` only because its path is ambiguous would be
    // re-keyed the moment the selection names its operation: `--check`
    // reports it as a tie to settle, and a write refuses rather than record a
    // key nothing chose.
    let unattributed = crate::spans::unattributed(&current);

    if args.has("check") {
        let (edits, present) = match &existing {
            Some(l) => (compare_with_lock(&current, l), true),
            None => (vec![], false),
        };
        let drift = existing
            .as_ref()
            .map(|l| crate::provenance::drift(&dir, l))
            .unwrap_or_default();
        // The schema, the pinned documents and inventory.json are recorded
        // provenance files too, so a hand edit to any of them also shows up
        // as drift. Relocking would acknowledge it and `codify --key` would
        // then refuse the span as in sync, so a relock is advised only when
        // no span, pinned source or inventory is out of sync. A document
        // sources.lock.yaml no longer pins is not a hand edit: its remedy is
        // restoring the entry or relocking on purpose, so it does not
        // withhold the advice.
        // An unattributed span (ADR 0044) withholds it too: a relock refuses
        // to write while a root field's operation is ambiguous.
        let gate_clean = edits.is_empty()
            && unattributed.is_empty()
            && source_problems.is_empty()
            && !inventory_edited;
        let relock_advisable = present && gate_clean && (!drift.is_empty() || !unpinned.is_empty());
        if args.has("json") {
            let list: Vec<Value> = edits
                .iter()
                .map(|e| {
                    crate::json::object(vec![
                        ("key", Value::from(e.key.as_str())),
                        ("change", Value::from(e.change)),
                        ("line", e.line.map(Value::from).unwrap_or(Value::Null)),
                    ])
                })
                .collect();
            print!(
                "{}",
                crate::json::pretty(&crate::json::object(vec![
                    ("lock", Value::from(present)),
                    ("spans", Value::from(current.len())),
                    ("hand_edits", Value::Array(list)),
                    (
                        "sources",
                        Value::Array(sources.iter().map(|s| s.to_value()).collect()),
                    ),
                    (
                        "unpinned",
                        Value::Array(unpinned.iter().map(|p| Value::from(p.as_str())).collect()),
                    ),
                    ("inventory_edited", Value::Bool(inventory_edited)),
                    (
                        "source_problems",
                        Value::Array(
                            source_problems
                                .iter()
                                .map(|s| Value::from(s.path.as_str()))
                                .collect(),
                        ),
                    ),
                    ("relock_advisable", Value::Bool(relock_advisable)),
                    (
                        "provenance_drift",
                        Value::Array(
                            drift
                                .iter()
                                .map(|d| {
                                    crate::json::object(vec![
                                        ("section", Value::from(d.section)),
                                        ("path", Value::from(d.path.as_str())),
                                        ("change", Value::from(d.change)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    (
                        "unattributed",
                        Value::Array(
                            unattributed
                                .iter()
                                .map(|u| Value::from(u.as_str()))
                                .collect()
                        ),
                    ),
                ]))
            );
        } else if !present {
            println!(
                "lock: no {} — run `graphos-factory-core lock` to acknowledge the schema as it stands",
                LOCK_FILE
            );
        } else if edits.is_empty() && unattributed.is_empty() {
            println!("lock: {} spans in sync with {}", current.len(), LOCK_FILE);
        } else if !edits.is_empty() {
            println!(
                "lock: {} span{} changed since {} — codify each (graphos-factory-core codify --key K --reason …) before applying:",
                edits.len(),
                if edits.len() == 1 { "" } else { "s" },
                LOCK_FILE
            );
            for e in &edits {
                println!(
                    "  {} {}{}",
                    match e.change {
                        "added" => "+",
                        "removed" => "-",
                        _ => "~",
                    },
                    e.key,
                    e.line
                        .map(|l| format!("   (line {})", l))
                        .unwrap_or_default()
                );
            }
        }
        if !args.has("json") && !unattributed.is_empty() {
            println!(
                "lock: {} root field{} match{} several operations equally and the selection does not say which:",
                unattributed.len(),
                if unattributed.len() == 1 { "" } else { "s" },
                if unattributed.len() == 1 { "es" } else { "" }
            );
            for u in &unattributed {
                println!("  {}", u);
            }
            println!("lock: {}", crate::spans::SETTLE_TIE);
        }
        if !args.has("json") && !source_problems.is_empty() {
            println!(
                "lock: {} pinned source{} out of sync:",
                source_problems.len(),
                if source_problems.len() == 1 { "" } else { "s" }
            );
            let problems: Vec<crate::sources::SourceStatus> =
                source_problems.iter().map(|s| (*s).clone()).collect();
            for l in crate::cmd::sources::render_lines(&problems) {
                println!("{}", l);
            }
        }
        if !args.has("json") && inventory_edited {
            println!(
                "lock: {} changed since {} — the inventory is built, never edited: rebuild it from the description document, or move the correction into the pinned spec (`graphos-factory-core codify --source`) or selection.yaml (a judgement), then run `graphos-factory-core lock`",
                crate::spans::INVENTORY_FILE,
                LOCK_FILE
            );
        }
        if !args.has("json") {
            for p in &unpinned {
                println!(
                    "lock: {} is acknowledged in {} but {} no longer pins it — restore the entry, or run `graphos-factory-core lock` to stop watching it on purpose",
                    p,
                    LOCK_FILE,
                    crate::sources::SOURCES_LOCK
                );
            }
        }
        if !args.has("json") && !drift.is_empty() {
            println!(
                "lock: {} file{} recorded in {}'s provenance differ{} from disk{} — {}:",
                drift.len(),
                if drift.len() == 1 { "" } else { "s" },
                LOCK_FILE,
                if drift.len() == 1 { "s" } else { "" },
                if args.has("provenance") {
                    ""
                } else {
                    " (reported, not enforced; --provenance enforces)"
                },
                if gate_clean {
                    "run `graphos-factory-core lock` to record the workspace as it stands"
                } else {
                    "do not relock yet: settle the problems above first (codify each hand edit, rebuild an edited inventory), and relock only once a plain `graphos-factory-core lock --check` exits 0"
                }
            );
            for d in &drift {
                println!(
                    "  {} {}   ({}{})",
                    match d.change {
                        "missing" => "-",
                        "added" => "+",
                        _ => "~",
                    },
                    d.path,
                    d.section,
                    match d.change {
                        "missing" => ", missing",
                        "added" => ", added since the lock",
                        _ => "",
                    }
                );
            }
        }
        let provenance_ok = !args.has("provenance") || drift.is_empty();
        return if present
            && edits.is_empty()
            && unattributed.is_empty()
            && source_problems.is_empty()
            && unpinned.is_empty()
            && !inventory_edited
            && provenance_ok
        {
            0
        } else {
            3
        };
    }

    if !unattributed.is_empty() {
        for u in &unattributed {
            eprintln!("lock: {}", u);
        }
        eprintln!("lock: {}", crate::spans::SETTLE_TIE);
        return 2;
    }
    let mut doc = lock_document(&loaded.schema_file, &current);
    if let Some(h) = crate::spans::inventory_hash(&dir) {
        crate::json::set(&mut doc, "inventory", Value::from(h.as_str()));
    }
    // Pinned documents: acknowledge the working copies as they stand. The
    // upstream copy is never acknowledged — a modified upstream stays an error.
    let mut acknowledged = 0;
    if !sources.is_empty() {
        let entries = crate::sources::read_sources_lock(&dir)
            .ok()
            .flatten()
            .map(|l| crate::sources::document_entries(&l))
            .unwrap_or_default();
        let hashes = crate::sources::applied_hashes(&dir, &entries);
        acknowledged = hashes.as_object().map(|m| m.len()).unwrap_or(0);
        crate::json::set(&mut doc, "sources", hashes);
    }
    let skill_dir = args.get("skill-dir").map(Path::new);
    if let Err(e) = crate::provenance::refresh(&dir, &mut doc, skill_dir, args.get("model")) {
        eprintln!("lock: authoring provenance: {}", e);
        return 2;
    }
    if let Err(e) = write_lock(&dir, &doc) {
        eprintln!("lock: {}", e);
        return 2;
    }
    if let Some(provenance) = doc.get("provenance") {
        for w in crate::provenance::fallback_warnings(provenance) {
            eprintln!("lock: {}", w);
        }
    }
    if args.has("json") {
        print!("{}", crate::json::pretty(&doc));
    } else {
        let changed = existing
            .as_ref()
            .map(|l| compare_with_lock(&current, l).len())
            .unwrap_or(current.len());
        println!(
            "lock: wrote {} ({} spans, {} changed since the previous lock{})",
            LOCK_FILE,
            current.len(),
            changed,
            if sources.is_empty() {
                String::new()
            } else {
                format!(
                    "; {} of {} pinned source{} acknowledged",
                    acknowledged,
                    sources.len(),
                    if sources.len() == 1 { "" } else { "s" }
                )
            }
        );
        for s in &sources {
            if s.upstream_ok == Some(false) {
                eprintln!(
                    "lock: warning: {} — the upstream copy {} was modified; it is never edited, restore it",
                    s.path, s.upstream
                );
            }
        }
    }
    0
}

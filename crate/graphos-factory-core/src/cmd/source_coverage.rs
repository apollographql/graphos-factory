//! source-coverage [workspace] [OP-KEY] [--json] [--check]
//!
//! Source coverage (ADR 0082): every fact the source offers is covered by the
//! schema, or a recorded decision says why not. For one operation this prints
//! every request-body and response path (not parameters) the source offers and
//! what the schema does with it (`crate::obligations`, the classifier) —
//! `--check` exits 1 when anything is left `unaccounted`, `unresolved`,
//! `unverified-default` **or `transport-expansion-missing`** in either direction (it fails closed: an
//! unresolved path is not known to be handled, and an expansion boundary
//! whose default projection is unverified leaves its offered paths unknown,
//! ADR 0047), naming which on stderr, for a script to gate on. It also
//! exits 1 when the workspace cannot be loaded, a selection.yaml that does
//! not parse included: the selection settles which operation a bare-id
//! field serves (ADR 0044).
//!
//! Since ADR 0095 it also reads what an optional argument's source says about
//! omitting it (the omission sentence), as a `behaviour` section: documented,
//! waived by a resolved `behaviour` decision, or unaccounted, and `--check`
//! fails on unaccounted.
//!
//! With no OP-KEY (ADR 0101) it runs the same classifier on every operation
//! `selection.yaml` includes, in inventory order: one counts line per
//! operation, a totals line and the number that fail the bar; `--json`
//! wraps each operation's single-key object, unchanged, in `operations`;
//! `--check` exits 1 when any selected operation fails, naming each on
//! stderr. An operation the classifier cannot build a report for is listed
//! with its error, named on stderr and counted failing, and a selection that
//! includes no operation exits 1: nothing checked is not a pass.
//!
//! A resolved `omits` entry that no offered row needs any more — every row
//! it covers is mapped, or read by the envelope while the entry says
//! `editorial`, or the source offers no row at its path, or the inventory
//! has no such operation — is a **stale omit** (ADR 0103): counted per
//! direction as `stale-omit N` and listed with its decision id, but not
//! part of the `--check` bar, since it leaves no offered path unaccounted.
//! `lint` warns on each (`stale-omit`). With no OP-KEY the list covers
//! every operation a resolved entry names, selected or not.
//!
//! It was `spans obligations` until ADR 0082; that spelling still runs, as an
//! alias, and says so on stderr.

use crate::args::{Args, Flags};
use crate::json::{get, get_arr, get_obj, get_str, truthy};
use crate::obligations::{Counts, Direction, DroppedStatus, Report, Row, StaleOmit};
use serde_json::Value;
use std::path::Path;

/// The usage text: `source-coverage --help` prints it on stdout (ADR 0086).
pub const USAGE: &str =
    "usage: graphos-factory-core source-coverage [workspace] [OP-KEY] [--json] [--check]\n  OP-KEY: that operation, every row; none: every operation selection.yaml includes, one counts line each";

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["json", "check"],
    valued: &[],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    match args.positional.get(1) {
        Some(k) => single(&args, &dir, k),
        // One positional is the workspace. An OP-KEY given alone, as
        // before ADR 0101, is not a directory: say how to name both. Any
        // other argument that is not a directory is a workspace path that
        // is wrong, and is reported as one.
        None if args.positional.len() == 1 && !dir.is_dir() => {
            let arg = dir.display().to_string();
            if looks_like_op_key(&arg) {
                eprintln!(
                    "source-coverage: {} is an operation key, not a workspace directory; to check one operation name the workspace first (`source-coverage . {}`)\n{}",
                    arg, arg, USAGE
                );
            } else if dir.exists() {
                eprintln!("source-coverage: {}: not a directory\n{}", arg, USAGE);
            } else {
                eprintln!(
                    "source-coverage: {}: no such workspace directory\n{}",
                    arg, USAGE
                );
            }
            1
        }
        None => every_selected(&args, &dir),
    }
}

/// `method:/path`, the shape of an inventory operation key.
fn looks_like_op_key(arg: &str) -> bool {
    const METHODS: &[&str] = &[
        "get", "put", "post", "delete", "patch", "head", "options", "trace",
    ];
    match arg.split_once(':') {
        Some((method, path)) => {
            METHODS.contains(&method.to_ascii_lowercase().as_str()) && path.starts_with('/')
        }
        None => false,
    }
}

fn row_json(r: &Row) -> Value {
    let mut row = serde_json::json!({
        "path": r.path,
        "class": r.class.label(),
        "structurally_consumed": r.structurally_consumed,
    });
    if let Some(note) = &r.note {
        row["note"] = Value::from(note.as_str());
    }
    row
}

fn counts_json(c: &Counts) -> Value {
    let mut counts = serde_json::json!({
        "offered": c.offered,
        "mapped": c.mapped,
        "consumed": c.consumed,
        "omitted": c.omitted,
        "unaccounted": c.unaccounted,
        "unresolved": c.unresolved,
    });
    // Only an operation that meets an expansion boundary (ADR 0047)
    // carries the key, so output for every other workspace is
    // unchanged.
    if c.unverified_default > 0 {
        counts["unverified_default"] = Value::from(c.unverified_default);
    }
    if c.transport_expansion_missing > 0 {
        counts["transport_expansion_missing"] = Value::from(c.transport_expansion_missing);
    }
    // Likewise only a workspace with a stale omit (ADR 0103).
    if c.stale_omit > 0 {
        counts["stale_omit"] = Value::from(c.stale_omit);
    }
    counts
}

fn stale_json(s: &StaleOmit) -> Value {
    serde_json::json!({
        "decision": s.decision,
        "operation": s.operation,
        "direction": s.direction.label(),
        "path": s.path,
        "why": s.why.label(),
        "message": s.describe(),
        "fix": s.fix(),
    })
}

/// One direction's object: counts, rows and, only when there is one, the
/// stale omits.
fn direction_json(report: &Report, direction: Direction, counts: &Counts, rows: &[Row]) -> Value {
    let mut out = serde_json::json!({
        "counts": counts_json(counts),
        "rows": rows.iter().map(row_json).collect::<Vec<_>>(),
    });
    let stale: Vec<Value> = report.stale_in(direction).map(stale_json).collect();
    if !stale.is_empty() {
        out["stale_omits"] = Value::from(stale);
    }
    out
}

fn dropped_label(status: &DroppedStatus) -> String {
    match status {
        DroppedStatus::Ran => "ran".to_string(),
        DroppedStatus::Skipped(why) => format!(
            "skipped ({})",
            why.trim_start_matches("skipped (").trim_end_matches(')')
        ),
    }
}

/// One operation's `--json` object. The no-key form puts exactly this, per
/// operation, in its `operations` array.
fn report_json(report: &Report) -> Value {
    let mut json = serde_json::json!({
        "op_key": report.op_key,
        "dropped_construct_detection": dropped_label(&report.dropped_status),
        "response": direction_json(report, Direction::Response, &report.response_counts(), &report.response),
        "request": direction_json(report, Direction::Request, &report.request_counts(), &report.request),
    });
    // Only an operation whose source states what omitting an argument does
    // (ADR 0095) carries the key, so every other report is unchanged.
    if !report.behaviour.is_empty() {
        json["behaviour"] = serde_json::json!({
            "counts": behaviour_counts_json(&report.behaviour_counts()),
            "rows": report.behaviour.iter().map(|r| {
                let mut row = serde_json::json!({
                    "path": r.path,
                    "carrier": r.carrier,
                    "sentence": r.sentence,
                    "class": r.class.label(),
                });
                if let Some(note) = &r.note {
                    row["note"] = Value::from(note.as_str());
                }
                row
            }).collect::<Vec<_>>(),
        });
    }
    json
}

fn behaviour_counts_json(c: &crate::obligations::BehaviourCounts) -> Value {
    serde_json::json!({
        "offered": c.offered,
        "documented": c.documented,
        "waived": c.waived,
        "unaccounted": c.unaccounted,
    })
}

/// ` · unverified-default N · transport-expansion-missing N`, each only
/// when nonzero.
fn boundary_extra(c: &Counts) -> String {
    let mut extra = String::new();
    if c.unverified_default > 0 {
        extra.push_str(&format!(" · unverified-default {}", c.unverified_default));
    }
    if c.transport_expansion_missing > 0 {
        extra.push_str(&format!(
            " · transport-expansion-missing {}",
            c.transport_expansion_missing
        ));
    }
    if c.stale_omit > 0 {
        extra.push_str(&format!(" · stale-omit {}", c.stale_omit));
    }
    extra
}

fn print_stale(report: &Report, direction: Direction) {
    for s in report.stale_in(direction) {
        println!("  stale omit: {}; {}", s.describe(), s.fix());
    }
}

fn single(args: &Args, dir: &Path, op_key: &str) -> i32 {
    let report = match crate::obligations::build(dir, op_key) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("source-coverage: {}", e);
            return 1;
        }
    };

    if args.has("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(&report)).unwrap()
        );
    } else {
        let rc = report.response_counts();
        let qc = report.request_counts();
        println!("=== source-coverage: {} ===", report.op_key);
        match &report.dropped_status {
            DroppedStatus::Ran => {
                println!("dropped-construct detection: ran")
            }
            DroppedStatus::Skipped(why) => {
                println!("dropped-construct detection: {}", why)
            }
        }
        println!(
            "\n=== response: offered {} · mapped {} · consumed {} · omitted {} · unaccounted {} · unresolved {}{} ===",
            rc.offered, rc.mapped, rc.consumed, rc.omitted, rc.unaccounted, rc.unresolved, boundary_extra(&rc)
        );
        for row in &report.response {
            println!(
                "  {} {}{}{}",
                row.path,
                row.class.label(),
                if row.structurally_consumed {
                    " [consumed]"
                } else {
                    ""
                },
                row.note
                    .as_deref()
                    .map(|n| format!(" — {}", n))
                    .unwrap_or_default()
            );
        }
        print_stale(&report, Direction::Response);
        println!(
            "\n=== request: offered {} · mapped {} · omitted {} · unaccounted {} · unresolved {}{} ===",
            qc.offered, qc.mapped, qc.omitted, qc.unaccounted, qc.unresolved, boundary_extra(&qc)
        );
        for row in &report.request {
            println!(
                "  {} {}{}",
                row.path,
                row.class.label(),
                row.note
                    .as_deref()
                    .map(|n| format!(" — {}", n))
                    .unwrap_or_default()
            );
        }
        print_stale(&report, Direction::Request);
        if !report.behaviour.is_empty() {
            let c = report.behaviour_counts();
            println!(
                "\n=== behaviour: offered {} · documented {} · waived {} · unaccounted {} ===",
                c.offered, c.documented, c.waived, c.unaccounted
            );
            for row in &report.behaviour {
                println!(
                    "  {} {} on {} — \"{}\"{}",
                    row.path,
                    row.class.label(),
                    row.carrier,
                    row.sentence,
                    row.note
                        .as_deref()
                        .map(|n| format!(" — {}", n))
                        .unwrap_or_default()
                );
            }
        }
    }

    if !args.has("check") {
        return 0;
    }
    match report.check_failure() {
        Some(why) => {
            eprintln!("source-coverage --check: {} fails — {}", report.op_key, why);
            1
        }
        None => 0,
    }
}

/// The operations `selection.yaml` includes (`include: true`), in inventory
/// order; an included key the inventory does not have follows, in selection
/// order, so its error is reported rather than the key dropped.
fn selected_keys(selection: Option<&Value>, inventory: &Value) -> Vec<String> {
    let included: Vec<&str> = selection
        .and_then(|s| get_obj(s, "operations"))
        .into_iter()
        .flatten()
        .filter(|(_, e)| truthy(get(e, "include")))
        .map(|(k, _)| k.as_str())
        .collect();
    let in_inventory: Vec<&str> = get_arr(inventory, "operations")
        .into_iter()
        .flatten()
        .filter_map(|o| get_str(o, "key"))
        .collect();
    let mut keys: Vec<String> = in_inventory
        .iter()
        .filter(|k| included.contains(k))
        .map(|k| k.to_string())
        .collect();
    let mut seen: std::collections::BTreeSet<String> = keys.iter().cloned().collect();
    for k in included {
        if !in_inventory.contains(&k) && seen.insert(k.to_string()) {
            keys.push(k.to_string());
        }
    }
    keys
}

fn add(total: &mut Counts, c: &Counts) {
    total.offered += c.offered;
    total.mapped += c.mapped;
    total.consumed += c.consumed;
    total.omitted += c.omitted;
    total.unaccounted += c.unaccounted;
    total.unresolved += c.unresolved;
    total.unverified_default += c.unverified_default;
    total.transport_expansion_missing += c.transport_expansion_missing;
    total.stale_omit += c.stale_omit;
}

fn response_line(c: &Counts) -> String {
    format!(
        "response offered {} · mapped {} · consumed {} · omitted {} · unaccounted {} · unresolved {}{}",
        c.offered, c.mapped, c.consumed, c.omitted, c.unaccounted, c.unresolved, boundary_extra(c)
    )
}

fn request_line(c: &Counts) -> String {
    format!(
        "request offered {} · mapped {} · omitted {} · unaccounted {} · unresolved {}{}",
        c.offered,
        c.mapped,
        c.omitted,
        c.unaccounted,
        c.unresolved,
        boundary_extra(c)
    )
}

/// No OP-KEY: every selected operation (ADR 0101).
/// ` | behaviour offered N · documented N · waived N · unaccounted N`, only
/// when there are behaviour rows (ADR 0095), so a workspace without them
/// prints what it printed before.
fn behaviour_segment(c: &crate::obligations::BehaviourCounts) -> String {
    if c.offered == 0 {
        return String::new();
    }
    format!(
        " | behaviour offered {} · documented {} · waived {} · unaccounted {}",
        c.offered, c.documented, c.waived, c.unaccounted
    )
}

fn every_selected(args: &Args, dir: &Path) -> i32 {
    let prepared = match crate::obligations::prepare(dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("source-coverage: {}", e);
            return 1;
        }
    };
    let inventory = prepared
        .loaded
        .inventory
        .as_ref()
        .expect("prepare refuses a workspace without inventory.json");
    let keys = selected_keys(prepared.loaded.selection.as_ref(), inventory);
    if keys.is_empty() {
        let why = if prepared.loaded.selection.is_none() {
            "there is no .factory/selection.yaml"
        } else {
            "selection.yaml has no `include: true` entry"
        };
        eprintln!(
            "source-coverage: no operation is selected ({}), so nothing was checked; name an OP-KEY or select operations",
            why
        );
        return 1;
    }

    // (key, report or error, why it fails the bar)
    let results: Vec<(String, Result<Report, String>)> = keys
        .into_iter()
        .map(|k| {
            let r = crate::obligations::build_prepared(&prepared, &k);
            (k, r)
        })
        .collect();
    let failure = |r: &Result<Report, String>| match r {
        Ok(report) => report.check_failure(),
        Err(e) => Some(format!("error: {}", e)),
    };
    let mut response_total = Counts::default();
    let mut request_total = Counts::default();
    let mut behaviour_total = crate::obligations::BehaviourCounts::default();
    let mut failing: Vec<(&str, String)> = Vec::new();
    let mut errors = 0usize;
    for (k, r) in &results {
        match r {
            Ok(report) => {
                add(&mut response_total, &report.response_counts());
                add(&mut request_total, &report.request_counts());
                let b = report.behaviour_counts();
                behaviour_total.offered += b.offered;
                behaviour_total.documented += b.documented;
                behaviour_total.waived += b.waived;
                behaviour_total.unaccounted += b.unaccounted;
            }
            Err(_) => errors += 1,
        }
        if let Some(why) = failure(r) {
            failing.push((k.as_str(), why));
        }
    }
    let selected = results.len();
    // Every operation a resolved omit names, selected or not (ADR 0103).
    let stale = crate::obligations::stale_omits(&prepared);

    if args.has("json") {
        let operations: Vec<Value> = results
            .iter()
            .map(|(k, r)| match r {
                Ok(report) => report_json(report),
                Err(e) => serde_json::json!({"op_key": k, "error": e}),
            })
            .collect();
        let mut json = serde_json::json!({
            "operations": operations,
            "totals": {
                "selected": selected,
                "passing": selected - failing.len(),
                "failing": failing.len(),
                "errors": errors,
                "response": counts_json(&response_total),
                "request": counts_json(&request_total),
            },
            "failing": failing.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        });
        // Only when some operation has behaviour rows (ADR 0095).
        if behaviour_total.offered > 0 {
            json["totals"]["behaviour"] = behaviour_counts_json(&behaviour_total);
        }
        if !stale.is_empty() {
            json["stale_omits"] = Value::from(stale.iter().map(stale_json).collect::<Vec<_>>());
        }
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        println!(
            "=== source-coverage: {} selected operation{} ===",
            selected,
            if selected == 1 { "" } else { "s" }
        );
        if let Some(status) = results.iter().find_map(|(_, r)| r.as_ref().ok()) {
            println!(
                "dropped-construct detection: {}",
                dropped_label(&status.dropped_status)
            );
        }
        for (k, r) in &results {
            match r {
                Ok(report) => println!(
                    "{}  {} | {}{}{}",
                    k,
                    response_line(&report.response_counts()),
                    request_line(&report.request_counts()),
                    behaviour_segment(&report.behaviour_counts()),
                    if report.check_failure().is_some() {
                        "  FAIL"
                    } else {
                        ""
                    }
                ),
                Err(e) => println!("{}  error: {}  FAIL", k, e),
            }
        }
        println!(
            "\ntotals: {} | {}{}",
            response_line(&response_total),
            request_line(&request_total),
            behaviour_segment(&behaviour_total)
        );
        println!(
            "{} of {} selected operations fail the bar{}",
            failing.len(),
            selected,
            if errors > 0 {
                format!(" ({} could not be classified)", errors)
            } else {
                String::new()
            }
        );
        if !stale.is_empty() {
            println!(
                "{} stale omit{} (not part of the bar; lint warns):",
                stale.len(),
                if stale.len() == 1 { "" } else { "s" }
            );
            for s in &stale {
                println!("  {}; {}", s.describe(), s.fix());
            }
        }
    }

    if args.has("check") {
        for (k, why) in &failing {
            eprintln!("source-coverage --check: {} fails — {}", k, why);
        }
        if !failing.is_empty() {
            eprintln!(
                "source-coverage --check: {} of {} selected operations fail",
                failing.len(),
                selected
            );
            return 1;
        }
    }
    // An operation that could not be classified was not checked, --check or
    // not: the single-key form exits 1 on the same error, and says why on
    // stderr. (Under --check the failure lines above already name it.)
    if errors > 0 {
        for (k, r) in &results {
            if let Err(e) = r {
                eprintln!("source-coverage: {}: {}", k, e);
            }
        }
        eprintln!(
            "source-coverage: {} of {} selected operations could not be classified",
            errors, selected
        );
        return 1;
    }
    0
}

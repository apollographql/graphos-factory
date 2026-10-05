//! spans — precise, per-span reads of the schema.
//!
//!   spans obligations [workspace] [OP-KEY] [--json] [--check]
//!   spans json-accounting [workspace] [--json] [--check]
//!
//! `spans obligations` is the pre-ADR 0082 spelling of `source-coverage`
//! (`crate::cmd::source_coverage`). It still runs, with the same output and
//! exit code, and names the new verb on stderr so a script or an agent
//! following an old note moves over. `show`/`closure`/`replace` (the
//! span-instrument branch's hand-edit tooling) come later, as their own
//! branch.
//!
//! `json-accounting` prints, for the whole workspace, every response field
//! the current SDL still leaves as the workspace's own JSON scalar and
//! whether a **resolved** `.factory/decisions.json` record's `json_reasons`
//! names it with a reason from the closed vocabulary
//! `crate::json_accounting::REASONS` — never `selection.yaml` or a schema
//! doc comment (ADR 0073, Adam's rule) — `--check` exits 1 when any is
//! unaccounted.

use crate::args::{Args, Flags};
use std::path::Path;

/// The usage text: `spans json-accounting --help` prints it on stdout.
pub const JSON_ACCOUNTING_USAGE: &str =
    "usage: graphos-factory-core spans json-accounting [workspace] [--json] [--check]";

fn usage(msg: &str) -> i32 {
    eprintln!(
        "spans: {}\n{}\n{}",
        msg,
        super::source_coverage::USAGE,
        JSON_ACCOUNTING_USAGE
    );
    1
}

pub fn main(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("obligations") => {
            eprintln!(
                "spans: `spans obligations` is now `graphos-factory-core source-coverage` (ADR 0082); running it"
            );
            crate::cmd::source_coverage::main(&argv[1..])
        }
        Some("json-accounting") => json_accounting(&argv[1..]),
        Some(other) => usage(&format!("unknown subcommand {:?}", other)),
        None => {
            usage("expected a subcommand (obligations, now `source-coverage`; json-accounting)")
        }
    }
}

/// The flags `spans json-accounting` accepts; dispatch refuses any other (ADR 0097).
pub const JSON_ACCOUNTING_FLAGS: Flags = Flags {
    boolean: &["json", "check"],
    valued: &[],
};

fn json_accounting(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &JSON_ACCOUNTING_FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    let report = match crate::json_accounting::build(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("spans: {}", e);
            // No SDL yet is not_run, not a failure to report — the
            // evidence layer maps this exit code the same way it maps
            // compose/unit/e2e/live's (src/cmd/evidence.rs).
            return if e.starts_with("not_run:") { 3 } else { 1 };
        }
    };

    if args.has("json") {
        let by_type_json = report
            .by_type()
            .iter()
            .map(|(t, c)| {
                serde_json::json!({
                    "type": t,
                    "accounted": c.accounted,
                    "unaccounted": c.unaccounted,
                    "stale": c.stale,
                    "recoverable": c.recoverable,
                    "unresolved": c.unresolved,
                })
            })
            .collect::<Vec<_>>();
        let rows_json = report
            .rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "type": r.type_name,
                    "field": r.field_name,
                    "reason": r.reason,
                    "decision": r.decision,
                    "status": r.status.label(),
                })
            })
            .collect::<Vec<_>>();
        let json = serde_json::json!({
            "json_scalar": report.json_scalar,
            "total_fields": report.total_fields,
            "json_fields": report.rows.len(),
            "accounted": report.accounted_count(),
            "unaccounted": report.unaccounted_count(),
            "stale": report.stale_count(),
            "recoverable": report.recoverable_count(),
            "unresolved": report.unresolved_count(),
            "ratio": report.typed_to_json_ratio().map(|(t, j)| format!("{}:{}", t, j)),
            "by_type": by_type_json,
            "rows": rows_json,
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        println!("=== json-accounting: {} ===", report.json_scalar);
        let ratio = report
            .typed_to_json_ratio()
            .map(|(t, j)| format!(" · ratio {}:{}", t, j))
            .unwrap_or_default();
        println!(
            "json {} · accounted {} · unaccounted {} · stale {} · recoverable {} · unresolved {}{}",
            report.rows.len(),
            report.accounted_count(),
            report.unaccounted_count(),
            report.stale_count(),
            report.recoverable_count(),
            report.unresolved_count(),
            ratio
        );
        for (t, c) in report.by_type() {
            println!(
                "  {}: accounted {} · unaccounted {} · stale {} · recoverable {} · unresolved {}",
                t, c.accounted, c.unaccounted, c.stale, c.recoverable, c.unresolved
            );
        }
        for r in &report.rows {
            println!(
                "    {} {}{}",
                r.key(),
                r.status.label(),
                r.reason
                    .as_deref()
                    .map(|x| format!(" ({})", x))
                    .unwrap_or_default()
            );
        }
    }

    if !args.has("check") {
        return 0;
    }
    match report.check_failure() {
        Some(why) => {
            eprintln!("spans json-accounting --check: {}", why);
            1
        }
        None => 0,
    }
}

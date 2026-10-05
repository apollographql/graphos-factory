//! serialization — every selected write's outbound-request proof, from
//! `crate::request_serialization`.
//!
//!   serialization [workspace-dir] [--json]
//!
//! Read-only. Its surface is decided (ADR 0079 Step 2): a verb of its own,
//! reading `.factory/evidence/latest.json` on disk for a standalone check;
//! `cmd/evidence.rs` wires the same `request_serialization::report_with_evidence`
//! logic as the `write_body_proof` layer instead, passing its own
//! same-run results (never latest.json mid-run). Exit code 1 when any
//! obligation fails or could not run, 0 otherwise.

use crate::args::{Args, Flags};
use crate::request_serialization::{report, ObligationStatus};
use std::path::Path;

/// The flags `serialization` accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &[],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let dir = args.dir();
    let r = report(Path::new(&dir));
    if args.has("json") {
        match serde_json::to_value(&r) {
            Ok(v) => print!("{}", crate::json::pretty(&v)),
            Err(e) => {
                eprintln!("serialization: {}", e);
                return 2;
            }
        }
    } else {
        for w in &r.writes {
            println!(
                "{}  {} {}  body {}  {} gap{}",
                w.operation,
                w.method,
                w.field,
                if w.body_proven { "proven" } else { "unproven" },
                w.gaps.len(),
                if w.gaps.len() == 1 { "" } else { "s" }
            );
            for g in &w.gaps {
                println!("    [{:?}] {}", g.kind, g.message);
            }
        }
        for o in &r.obligations {
            println!("{}  {}  {}", o.status.as_str(), o.id, o.message);
        }
        println!("gaps: {} on writes, {} on reads", r.write_gaps, r.read_gaps);
    }
    let failed = r.obligations.iter().any(|o| {
        matches!(
            o.status,
            ObligationStatus::Fail | ObligationStatus::Unexecuted
        )
    });
    i32::from(failed)
}

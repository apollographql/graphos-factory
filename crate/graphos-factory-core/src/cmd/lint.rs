//! lint — the mechanical invariants of a service workspace.
//!
//!   lint [workspace-dir] [--json] [--warnings-as-errors] [--skip-evidence] [--schemas DIR]
//!
//! Exit code 1 on any error finding. --skip-evidence leaves
//! .factory/evidence/latest.json unchecked; only `evidence` should pass it,
//! because it is about to rewrite that file.

use crate::args::{Args, Flags};
use crate::lint::{lint_workspace, LintOptions};
use std::path::{Path, PathBuf};

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["json", "warnings-as-errors", "skip-evidence"],
    valued: &["schemas"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let dir = args.dir();
    let schemas = args.get("schemas").map(PathBuf::from);
    let result = lint_workspace(
        Path::new(&dir),
        &LintOptions {
            schemas_dir: schemas.as_deref(),
            skip_evidence: args.has("skip-evidence"),
            target: crate::target::active(),
        },
    );

    if args.has("json") {
        // Which target's rules ran beside the core's, and which core rules
        // it adjusted, so a reviewer can see the whole contract applied.
        let target = crate::target::active();
        let out = serde_json::json!({
            "findings": result.findings,
            "errors": result.errors,
            "warnings": result.warnings,
            "target": {
                "name": target.name,
                "rules": target.lint_rules,
                "overrides": target.overridden_rules(),
            },
        });
        print!("{}", crate::json::pretty(&out));
    } else {
        for f in &result.findings {
            let where_ = match (&f.file, f.line) {
                (Some(file), Some(line)) => format!("{}:{}", file, line),
                (Some(file), None) => file.clone(),
                _ => dir.clone(),
            };
            println!(
                "{}  {}  [{}] {}",
                match f.severity.as_str() {
                    "error" => "error",
                    "info" => "info ",
                    _ => "warn ",
                },
                where_,
                f.rule,
                f.message
            );
        }
        if result.findings.is_empty() {
            println!("lint: {} is clean", dir);
        } else {
            println!(
                "\nlint: {} error{}, {} warning{}",
                result.errors,
                if result.errors == 1 { "" } else { "s" },
                result.warnings,
                if result.warnings == 1 { "" } else { "s" }
            );
        }
    }
    let failed = result.errors > 0 || (args.has("warnings-as-errors") && result.warnings > 0);
    if failed {
        1
    } else {
        0
    }
}

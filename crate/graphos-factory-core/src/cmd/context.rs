//! context check|capture
//!
//! `check` validates declared requirements. `capture` copies one already-local
//! artifact byte for byte and records its integrity and provenance; it never
//! fetches from the network and accepts no credential.

use crate::args::{Args, Flags};
use std::path::{Path, PathBuf};

/// The usage text: `context --help` prints it on stdout (ADR 0086), a
/// usage error on stderr.
pub const USAGE: &str = "usage: graphos-factory-core context check [workspace] [--phase build|live] [--json]\n       graphos-factory-core context capture [workspace] (--requirement ID|--input) --id ID --from FILE --representation raw|derived|transcribed --capture-method TEXT --source TEXT --scope TEXT… [--authority authoritative|supporting] [--format opaque|json|yaml] [--refresh-by YYYY-MM-DD] [--derived-from ID]… [--json]";

fn usage() {
    eprintln!("{}", USAGE);
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const CHECK_FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["phase", "schemas"],
};

fn check(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &CHECK_FLAGS);
    let phase = args.get("phase").unwrap_or("build");
    if !matches!(phase, "build" | "live") || (args.has("phase") && args.get("phase").is_none()) {
        eprintln!("context: --phase must be build or live");
        return 1;
    }
    let schemas = args.get("schemas").map(PathBuf::from);
    let report = crate::context::check(Path::new(&args.dir()), schemas.as_deref());
    if args.has("json") {
        print!(
            "{}",
            crate::json::pretty(&serde_json::to_value(&report).unwrap())
        );
    } else {
        if report.mode.is_none() && report.errors.is_empty() {
            println!("context_mode: not recorded; read as generic");
        }
        for error in &report.errors {
            println!("error: {}", error);
        }
        for blocker in &report.blockers {
            println!(
                "{} [{}; {}]: {}\n  next: {}",
                blocker.id,
                blocker.phase,
                blocker.affects.join(", "),
                blocker.reason,
                blocker.resolve_with
            );
        }
        println!(
            "context: build {}; live {} (context readiness only)",
            if report.build_ready {
                "ready"
            } else {
                "needs_context"
            },
            if report.live_ready {
                "ready"
            } else {
                "needs_context"
            }
        );
    }
    report.exit_code(phase)
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const CAPTURE_FLAGS: Flags = Flags {
    boolean: &["json", "input"],
    valued: &[
        "requirement",
        "id",
        "from",
        "representation",
        "authority",
        "format",
        "captured-at",
        "capture-method",
        "source",
        "scope",
        "refresh-by",
        "derived-from",
        "schemas",
    ],
};

fn capture(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &CAPTURE_FLAGS);
    let required = |name: &str| -> Result<String, String> {
        args.get(name)
            .map(str::to_string)
            .ok_or_else(|| format!("context capture: --{} is required", name))
    };
    let options: Result<crate::context::CaptureOptions, String> = (|| {
        Ok(crate::context::CaptureOptions {
            requirement: args.get("requirement").map(str::to_string),
            input: args.has("input"),
            id: required("id")?,
            from: PathBuf::from(required("from")?),
            representation: required("representation")?,
            authority: args.get("authority").unwrap_or("supporting").to_string(),
            format: args.get("format").unwrap_or("opaque").to_string(),
            captured_at: args
                .get("captured-at")
                .map(str::to_string)
                .unwrap_or_else(crate::now_iso),
            capture_method: required("capture-method")?,
            source: required("source")?,
            scope: args.all("scope"),
            refresh_by: args.get("refresh-by").map(str::to_string),
            derived_from: args.all("derived-from"),
        })
    })();
    let options = match options {
        Ok(options) => options,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    let schemas = args.get("schemas").map(PathBuf::from);
    match crate::context::capture(Path::new(&args.dir()), options, schemas.as_deref()) {
        Ok(report) => {
            if args.has("json") {
                print!(
                    "{}",
                    crate::json::pretty(&serde_json::to_value(&report).unwrap())
                );
            } else {
                println!(
                    "context: captured {} as {} ({} bytes, sha256 {})",
                    report.id, report.path, report.byte_count, report.sha256
                );
            }
            0
        }
        Err(e) => {
            eprintln!("context capture: {}", e);
            1
        }
    }
}

pub fn main(argv: &[String]) -> i32 {
    let Some((command, rest)) = argv.split_first() else {
        usage();
        return 1;
    };
    match command.as_str() {
        "check" => check(rest),
        "capture" => capture(rest),
        _ => {
            usage();
            1
        }
    }
}

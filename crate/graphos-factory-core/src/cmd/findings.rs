//! `findings <list|add|supersede>`: the workspace's findings (ADR 0113) —
//! facts a reference, an ADR or the wire settled, recorded because an
//! instrument reads their `omits` or `affects`, or the next session needs
//! the fact. The only writer of `.factory/findings.json`. A decision, which
//! has an alternative, is `decisions add`; a finding has none, so it has no
//! question, choices, resolution or reopen: it is `current` until
//! `supersede` replaces it.
//!
//! Exit codes: 0 done · 1 usage / unreadable / invalid / unknown id /
//! refused. With `--json` a refusal prints `{error, code, exit}` on stdout;
//! the codes are `usage`, `editorial-omit`, `findings-invalid`,
//! `findings-missing`, `unknown-finding`, `already-superseded` and
//! `write-failed`.

use crate::args::{Args, Flags};
use crate::decisions::Omit;
use crate::findings::{self, NewFinding, Refusal};
use crate::json;
use serde_json::Value;
use std::path::Path;

pub const USAGE: &str = "usage: graphos-factory-core findings <list|add|supersede> [workspace] …
  list      [workspace] [--json]
  add       [workspace] --title T --body TEXT [--cites C] [--source agent|codify|sources]
            [--affects PATH]… [--omit 'operation|direction|path|reason']… [--evidence E]…
            [--related D-nnnn|F-nnnn]… [--date YYYY-MM-DD] [--json]
            --omit reason: consumed (direction response|request) or not-applicable (behaviour);
            editorial is refused: a path left off on judgement is a decision (`decisions add`)
  supersede [workspace] --id F-nnnn [--json]   a later finding or a change replaced it; its omits and affects stop counting";

fn schemas_dir(args: &Args) -> Option<&Path> {
    args.get("schemas").map(Path::new)
}

/// A refusal: the message on stderr (with the usage for a usage error),
/// and with `--json` the `{error, code, exit}` object on stdout. Exit 1.
fn refuse(json_out: bool, refusal: Refusal) -> i32 {
    eprintln!("findings: {}", refusal.message);
    if refusal.code == "usage" {
        eprintln!("{}", USAGE);
    }
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("error", Value::from(refusal.message)),
                ("code", Value::from(refusal.code)),
                ("exit", Value::from(1)),
            ]))
        );
    }
    1
}

fn usage_refusal(message: impl Into<String>) -> Refusal {
    Refusal {
        code: "usage",
        message: message.into(),
    }
}

pub fn main(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("list") => list(&argv[1..]),
        Some("add") => add(&argv[1..]),
        Some("supersede") => supersede(&argv[1..]),
        _ => refuse(
            false,
            usage_refusal("expected a subcommand (list, add or supersede)"),
        ),
    }
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const LIST_FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["schemas"],
};

fn list(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &LIST_FLAGS);
    let json_out = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    let doc = match findings::load(&dir, schemas_dir(&args)) {
        Ok(doc) => doc,
        Err(e) => {
            return refuse(
                json_out,
                Refusal {
                    code: "findings-invalid",
                    message: e,
                },
            )
        }
    };
    let items: Vec<Value> = json::get_arr(&doc, "findings").cloned().unwrap_or_default();
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![("findings", Value::Array(items))]))
        );
        return 0;
    }
    if items.is_empty() {
        println!("No findings recorded.");
        return 0;
    }
    for f in &items {
        let status = json::get_str(f, "status").unwrap_or("current");
        println!(
            "[{:<10}] {}  {}",
            status,
            json::get_str(f, "id").unwrap_or("F-????"),
            json::get_str(f, "title").unwrap_or("")
        );
        if let Some(c) = json::get_str(f, "cites") {
            println!("              cites {}", c);
        }
        let omits = json::get_arr(f, "omits").map(Vec::len).unwrap_or(0);
        let affects = json::get_arr(f, "affects").map(Vec::len).unwrap_or(0);
        if omits + affects > 0 {
            println!("              {} omit(s), {} affects", omits, affects);
        }
    }
    0
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const ADD_FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &[
        "title", "body", "cites", "source", "affects", "omit", "evidence", "related", "date",
        "schemas",
    ],
};

/// `--omit operation|direction|path|reason`, as `decisions add` spells it,
/// with `editorial` refused (ADR 0113 §2).
fn collect_omits(args: &Args) -> Result<Vec<Omit>, Refusal> {
    let mut omits = Vec::new();
    for spec in args.all("omit") {
        let parts: Vec<&str> = spec.splitn(4, '|').collect();
        let [operation, direction, path, reason] = parts[..] else {
            return Err(usage_refusal(format!(
                "--omit expects operation|direction|path|reason, got {:?}",
                spec
            )));
        };
        if let Err(e) = findings::check_omit(direction, reason) {
            return Err(Refusal {
                code: if reason == "editorial" {
                    "editorial-omit"
                } else {
                    "usage"
                },
                message: format!("--omit {:?}: {}", spec, e),
            });
        }
        omits.push(Omit {
            operation: operation.to_string(),
            direction: direction.to_string(),
            path: path.to_string(),
            reason: reason.to_string(),
        });
    }
    Ok(omits)
}

fn add(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &ADD_FLAGS);
    let json_out = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    let Some(title) = args.get("title").filter(|t| !t.trim().is_empty()) else {
        return refuse(json_out, usage_refusal("add: --title is required"));
    };
    let Some(body) = args.get("body").filter(|b| !b.trim().is_empty()) else {
        return refuse(
            json_out,
            usage_refusal("add: --body is required (the fact, in prose)"),
        );
    };
    let source = args.get("source").unwrap_or("agent");
    if !findings::SOURCES.contains(&source) {
        return refuse(
            json_out,
            usage_refusal(format!(
                "add: --source must be one of {}, got {:?}",
                findings::SOURCES.join(", "),
                source
            )),
        );
    }
    let omits = match collect_omits(&args) {
        Ok(o) => o,
        Err(r) => return refuse(json_out, r),
    };
    let related = args.all("related");
    let id_re = regex::Regex::new(r"^[DF]-\d{4}$").unwrap();
    if let Some(bad) = related.iter().find(|r| !id_re.is_match(r)) {
        return refuse(
            json_out,
            usage_refusal(format!(
                "add: --related must look like D-0019 or F-0003, got {:?}",
                bad
            )),
        );
    }
    let mut doc = match findings::load(&dir, schemas_dir(&args)) {
        Ok(doc) => doc,
        Err(e) => {
            return refuse(
                json_out,
                Refusal {
                    code: "findings-invalid",
                    message: e,
                },
            )
        }
    };
    // A related id nobody recorded is a dangling pointer: say so, never fail.
    let decisions = crate::decisions::load(&dir, schemas_dir(&args)).ok();
    for r in &related {
        let known = if findings::is_finding_id(r) {
            findings::find(&doc, r).is_some()
        } else {
            decisions
                .as_ref()
                .is_some_and(|d| crate::decisions::find(d, r).is_some())
        };
        if !known {
            eprintln!(
                "findings: warning: --related {} names no recorded {}",
                r,
                if findings::is_finding_id(r) {
                    "finding"
                } else {
                    "decision"
                }
            );
        }
    }
    let new = NewFinding {
        title: title.to_string(),
        date: args
            .get("date")
            .map(str::to_string)
            .unwrap_or_else(crate::today),
        body: body.to_string(),
        cites: args.get("cites").map(str::to_string),
        source: source.to_string(),
        affects: args.all("affects"),
        omits,
        evidence: args.all("evidence"),
        related,
    };
    let id = match findings::add(&mut doc, new) {
        Ok(id) => id,
        Err(e) => return refuse(json_out, usage_refusal(e)),
    };
    if let Err(e) = findings::save(&dir, &doc, schemas_dir(&args)) {
        return refuse(
            json_out,
            Refusal {
                code: "write-failed",
                message: e,
            },
        );
    }
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("id", Value::from(id)),
                ("exit", Value::from(0)),
            ]))
        );
    } else {
        println!("recorded {}", id);
    }
    0
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const SUPERSEDE_FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["id", "schemas"],
};

fn supersede(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &SUPERSEDE_FLAGS);
    let json_out = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    let Some(id) = args.get("id") else {
        return refuse(
            json_out,
            usage_refusal("supersede: --id F-nnnn is required"),
        );
    };
    let mut doc = match findings::load_present(&dir, schemas_dir(&args)) {
        Ok(Some(doc)) => doc,
        Ok(None) => {
            return refuse(
                json_out,
                Refusal {
                    code: "findings-missing",
                    message: format!(
                        "{} does not exist; there is no finding to supersede",
                        findings::FILE
                    ),
                },
            )
        }
        Err(e) => {
            return refuse(
                json_out,
                Refusal {
                    code: "findings-invalid",
                    message: e,
                },
            )
        }
    };
    if let Err(r) = findings::supersede(&mut doc, id) {
        return refuse(json_out, r);
    }
    if let Err(e) = findings::save(&dir, &doc, schemas_dir(&args)) {
        return refuse(
            json_out,
            Refusal {
                code: "write-failed",
                message: e,
            },
        );
    }
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("id", Value::from(id)),
                ("status", Value::from("superseded")),
                ("previous_status", Value::from("current")),
                ("exit", Value::from(0)),
            ]))
        );
    } else {
        println!("superseded {} (was current)", id);
    }
    0
}

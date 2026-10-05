//! reconcile — the delta between the schema, selection.yaml and inventory.json.
//!
//!   reconcile [workspace-dir] [--json] [--baseline <git-rev>]
//!
//! Exit codes: 0 clean; 1 a delta exists (or an override's assertion fails,
//! a pinned override was modified, an override expired); 2 the selection is
//! invalid, a workspace file is unreadable, or the schema has connectors and
//! not one pairs with an inventory operation (the zero-match guard, ADR
//! 0074: a base-URL split, reported once with the URLs to compare). Hand edits since the applied
//! lock are reported but do not change the exit code — `lock --check` and
//! `lint` are the gates for those.

use crate::args::{Args, Flags};
use crate::json::{get, get_arr};
use std::path::Path;
use std::process::Command;

/// The workspace's schema file, named by `workspace.yaml`'s `directory` as
/// `reconcile_workspace`, `lock` and `export` name it — never the first
/// `.graphql` the directory listing happens to return.
fn schema_file(dir: &Path) -> Result<String, String> {
    let text =
        crate::factory_io::read_to_string(dir, ".factory/workspace.yaml").map_err(String::from)?;
    crate::reconcile::schema_file_of(&crate::yaml::parse(&text)?)
}

/// The text of `schema` at `rev`. `Ok(None)` when `rev` is a commit that holds
/// neither the schema nor an applied lock yet — a first apply — so the caller
/// can read every span as new rather than fail.
fn baseline_sdl(dir: &Path, rev: &str, schema: &str) -> Result<Option<String>, String> {
    let prefix = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "rev-parse", "--show-prefix"])
        .output()
        .map_err(|e| e.to_string())?;
    if !prefix.status.success() {
        return Err(String::from_utf8_lossy(&prefix.stderr).trim().to_string());
    }
    let prefix = String::from_utf8_lossy(&prefix.stdout).trim().to_string();
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map_err(|e| e.to_string())
    };
    // A revision that does not name a commit stays an error.
    let commit = git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{}^{{commit}}", rev),
    ])?;
    if !commit.status.success() {
        return Err(format!("--baseline {}: not a commit", rev));
    }
    let spec = format!("{}:{}{}", rev, prefix, schema);
    if !git(&["cat-file", "-e", &spec])?.status.success() {
        // Only a commit from before any apply is a first apply. One that
        // already holds an applied lock lost the schema (removed, renamed,
        // or `directory` changed): an empty baseline there would read as
        // every span new and hide the real delta.
        let lock = format!("{}:{}.factory/applied.lock.yaml", rev, prefix);
        if git(&["cat-file", "-e", &lock])?.status.success() {
            return Err(format!(
                "--baseline {}: {} is not in {}, but .factory/applied.lock.yaml is — the schema is missing from a commit that was already applied, so this is not a first apply",
                rev, schema, rev
            ));
        }
        return Ok(None);
    }
    let show = git(&["show", &spec])?;
    if !show.status.success() {
        return Err(String::from_utf8_lossy(&show.stderr).trim().to_string());
    }
    Ok(Some(String::from_utf8_lossy(&show.stdout).to_string()))
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["baseline"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let dir = args.dir();
    let rev = args.get("baseline");
    let baseline = match rev {
        Some(r) => match schema_file(Path::new(&dir))
            .and_then(|f| baseline_sdl(Path::new(&dir), r, &f).map(|b| (f, b)))
        {
            Ok((_, Some(b))) => Some(b),
            // First apply: the schema is not in the baseline commit yet, so
            // the baseline is empty and every span reads as new.
            Ok((f, None)) => {
                eprintln!(
                    "reconcile: first apply — {} is not in {}; every span is new",
                    f, r
                );
                Some(String::new())
            }
            Err(e) => {
                eprintln!("reconcile: {}", e);
                return 2;
            }
        },
        None => None,
    };
    let report = match crate::reconcile::reconcile_workspace(Path::new(&dir), baseline.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("reconcile: {}", e);
            return 2;
        }
    };
    if args.has("json") {
        print!("{}", crate::json::pretty(&report));
    } else {
        print!("{}", crate::reconcile::render_report(&report, &dir, rev));
    }
    let errors = get_arr(&report, "selection_errors")
        .map(|a| a.len())
        .unwrap_or(0);
    // No connector pairs at all: one base-URL split, not N deltas (ADR 0074).
    if let Some(z) = get(&report, "zero_match") {
        eprintln!("{}", crate::reconcile::zero_match_message(z));
        return 2;
    }
    if errors > 0 {
        2
    } else if get(&report, "clean").and_then(serde_json::Value::as_bool) == Some(true) {
        0
    } else {
        1
    }
}

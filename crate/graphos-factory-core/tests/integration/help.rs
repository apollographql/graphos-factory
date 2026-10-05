//! `--help` and `-h` print a usage and touch nothing, for every subcommand
//! and every sub-verb (ADR 0086). Before the fix, `selection draft --help`
//! wrote `.factory/selection.yaml`, `lock --help` wrote
//! `.factory/applied.lock.yaml` and `evidence --help` wrote
//! `.factory/evidence/latest.json`; most of the rest ran the command or
//! failed with a usage error.
//!
//! The invocations come from `cmd::COMMANDS` and `help::VERBS`, so a
//! command or verb added to either is covered here without a new line.

use graphos_factory_core::{cmd, help};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Every entry under `root`: a file's bytes, a symlink's target, a
/// directory as an empty marker (so a created empty directory counts).
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).unwrap();
                out.insert(rel, format!("-> {}", target.display()).into_bytes());
            } else if meta.is_dir() {
                out.insert(rel, b"<dir>".to_vec());
                walk(root, &path, out);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The paths that differ between two snapshots.
fn changed(before: &BTreeMap<PathBuf, Vec<u8>>, after: &BTreeMap<PathBuf, Vec<u8>>) -> Vec<String> {
    let mut paths: Vec<&PathBuf> = before.keys().chain(after.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .map(|p| p.display().to_string())
        .collect()
}

fn gitea_copy() -> tempfile::TempDir {
    let pilot = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&pilot, dir.path());
    dir
}

/// Every `<command> [verb]` the binary accepts.
fn invocations() -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (name, _) in cmd::COMMANDS {
        out.push(vec![name.to_string()]);
        if let Some((_, verbs)) = help::VERBS.iter().find(|(c, _)| c == name) {
            for verb in verbs.iter() {
                out.push(vec![name.to_string(), verb.to_string()]);
            }
        }
    }
    out
}

/// Run the binary in `cwd`; (exit code, stdout, stderr).
fn run(cwd: &Path, args: &[String]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(args)
        .current_dir(cwd)
        .env_remove("GRAPHOS_FACTORY_CORE_SCRIPTS")
        .env_remove("GRAPHOS_FACTORY_CORE_BIN")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The assertion each invocation is held to: exit 0, a usage on stdout
/// that names the command (and the verb), nothing on stderr, and the
/// workspace byte-identical.
fn assert_help(ws: &Path, cwd: &Path, args: &[String]) {
    let before = tree(ws);
    let (code, stdout, stderr) = run(cwd, args);
    let label = args.join(" ");
    let diff = changed(&before, &tree(ws));
    assert!(
        diff.is_empty(),
        "`{}` changed the workspace: {:?}",
        label,
        diff
    );
    assert_eq!(
        code,
        Some(0),
        "`{}`: stdout {} stderr {}",
        label,
        stdout,
        stderr
    );
    assert!(stderr.is_empty(), "`{}` wrote to stderr: {}", label, stderr);
    assert!(
        stdout.starts_with("usage:"),
        "`{}` printed no usage: {}",
        label,
        stdout
    );
    // `spans` is the old spelling of `source-coverage` and prints its usage.
    let command =
        if args[0] == "spans" && args.get(1).map(String::as_str) != Some("json-accounting") {
            "source-coverage"
        } else {
            args[0].as_str()
        };
    assert!(stdout.contains(command), "`{}`: {}", label, stdout);
    let is_verb = |v: &str| {
        help::VERBS
            .iter()
            .any(|(c, verbs)| *c == args[0] && verbs.contains(&v))
    };
    if args.len() > 1 && args[0] != "spans" && is_verb(&args[1]) {
        assert!(stdout.contains(&args[1]), "`{}`: {}", label, stdout);
    }
}

#[test]
fn every_command_and_verb_answers_help_without_touching_the_workspace() {
    let ws = gitea_copy();
    let mut count = 0;
    for inv in invocations() {
        for flag in ["--help", "-h"] {
            let mut args = inv.clone();
            args.push(flag.to_string());
            assert_help(ws.path(), ws.path(), &args);
            count += 1;
        }
    }
    // The core's 27 commands and 27 verbs, each with both spellings; a
    // target's commands are its own suite's.
    assert_eq!(count, 108);
}

/// The flag wins wherever it appears, and with the flags that make a
/// writer write: `selection draft --force`, `lock`, `evidence --scripts`.
#[test]
fn help_wins_over_every_other_argument_and_position() {
    let ws = gitea_copy();
    let elsewhere = tempfile::tempdir().unwrap();
    let dir = ws.path().to_string_lossy().into_owned();
    let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let cases = [
        // The workspace named positionally, run from another directory.
        s(&["selection", "draft", &dir, "--force", "--links", "--help"]),
        s(&["selection", "draft", "--help", &dir, "--force"]),
        s(&[
            "selection",
            "set",
            &dir,
            "--op",
            "get:/version",
            "--include",
            "false",
            "-h",
        ]),
        s(&["lock", &dir, "--model", "m", "--help"]),
        s(&["evidence", &dir, "--scripts", "/nonexistent", "--help"]),
        s(&["scaffold", &dir, "--force", "-h"]),
        s(&["decisions", "add", &dir, "--title", "T", "--help"]),
        s(&[
            "codify", &dir, "--key", "k", "--reason", "r", "--pin", "--help",
        ]),
        s(&[
            "inventory",
            "build",
            &format!("{}/openapi.json", dir),
            "--out",
            &format!("{}/x.json", dir),
            "--help",
        ]),
        s(&[
            "init",
            &format!("{}/new", dir),
            "--name",
            "n",
            "--spec",
            "s.json",
            "--help",
        ]),
    ];
    for args in cases {
        assert_help(ws.path(), elsewhere.path(), &args);
    }
    // Nothing appeared where the commands ran, either.
    assert!(tree(elsewhere.path()).is_empty());
}

/// A usage exists for every dispatched command, so the dispatcher's
/// fallback to the top-level usage is never what a user sees.
#[test]
fn every_dispatched_command_has_its_own_usage() {
    for (name, _) in cmd::COMMANDS {
        assert!(help::usage(name, &[]).is_some(), "{} has no usage", name);
    }
    for (name, verbs) in help::VERBS {
        assert!(
            cmd::COMMANDS.iter().any(|(c, _)| c == name),
            "help::VERBS names {}, which COMMANDS does not dispatch",
            name
        );
        for verb in verbs.iter() {
            let text = help::usage(name, &[verb.to_string()]).unwrap();
            assert!(text.ends_with('\n'), "{} {}", name, verb);
        }
    }
    // A verb picks its own lines out of the top-level usage.
    let review = help::usage("selection", &["review".to_string()]).unwrap();
    assert!(
        review.starts_with("usage: graphos-factory-core selection review "),
        "{}",
        review
    );
    assert!(!review.contains(" draft "), "{}", review);
    assert_eq!(
        help::usage("selection", &["set".to_string()]).unwrap(),
        format!("{}\n", cmd::selection_set::USAGE)
    );
    assert!(help::usage("no-such-command", &[]).is_none());
}

/// Unknown commands still fail, `--help` or not; the bare binary and
/// `--help` print the top-level usage.
#[test]
fn the_top_level_usage_and_an_unknown_command() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _, stderr) = run(dir.path(), &["nope".to_string(), "--help".to_string()]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("unknown command \"nope\""), "{}", stderr);
    for args in [vec![], vec!["--help".to_string()], vec!["-h".to_string()]] {
        let (code, stdout, _) = run(dir.path(), &args);
        assert_eq!(code, Some(0));
        // The usage is written with the shared name; the binary prints its own.
        assert_eq!(
            stdout,
            help::USAGE.replace("graphos-factory-core ", "graphos-factory-bare ")
        );
    }
}

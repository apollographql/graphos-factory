//! An unknown flag fails closed, for every subcommand and every sub-verb
//! (ADR 0097). Before the fix the shared `Args` grammar recorded any
//! `--flag` it did not know as one more bare flag, and a verb that never
//! asked for it ran anyway: `lock <ws> '--check --provenance'` (one
//! argument) wrote `.factory/applied.lock.yaml` and exited 0, and so did
//! `lock --chekc`.
//!
//! The invocations come from `cmd::COMMANDS` and `help::VERBS`, so a
//! command or verb added to either is covered here without a new line; the
//! flag sets come from `cmd::flags`, the table the parsers read.

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

fn gitea_copy() -> tempfile::TempDir {
    let pilot = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&pilot, dir.path());
    dir
}

/// Every `<command>` and `<command> <verb>` the binary accepts.
fn invocations() -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for (name, _) in cmd::COMMANDS {
        out.push(vec![name.to_string()]);
        for verb in help::verbs(name) {
            out.push(vec![name.to_string(), verb.to_string()]);
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

fn assert_refused(ws: &Path, args: &[String], names: &str) {
    let before = tree(ws);
    let (code, stdout, stderr) = run(ws, args);
    let label = format!("{:?}", args);
    let after = tree(ws);
    let changed: std::collections::BTreeSet<_> = before
        .keys()
        .chain(after.keys())
        .filter(|p| before.get(*p) != after.get(*p))
        .collect();
    assert!(
        changed.is_empty(),
        "{} changed the workspace: {:?}",
        label,
        changed
    );
    assert_eq!(
        code,
        Some(2),
        "{}: stdout {} stderr {}",
        label,
        stdout,
        stderr
    );
    assert!(stdout.is_empty(), "{} wrote to stdout: {}", label, stdout);
    assert!(
        stderr.contains(names),
        "{}: stderr does not name {}: {}",
        label,
        names,
        stderr
    );
}

#[test]
fn every_command_and_verb_refuses_an_unknown_flag_and_touches_nothing() {
    let ws = gitea_copy();
    let dir = ws.path().to_string_lossy().into_owned();
    let mut count = 0;
    for inv in invocations() {
        let bare_with_verbs = inv.len() == 1 && !help::verbs(&inv[0]).is_empty();
        for bogus in ["--bogus", "--bogus=1", "--json --bogus", "-q"] {
            // The workspace positionally, and the flag before and after it.
            for flag_first in [false, true] {
                let mut args = inv.clone();
                if flag_first {
                    args.push(bogus.to_string());
                    args.push(dir.clone());
                } else {
                    args.push(dir.clone());
                    args.push(bogus.to_string());
                }
                let names = if bare_with_verbs {
                    "where a verb belongs"
                } else {
                    bogus.split('=').next().unwrap()
                };
                // A bare command with verbs given the workspace first is
                // an unknown verb, which the command refuses on its own.
                if bare_with_verbs && !flag_first {
                    continue;
                }
                assert_refused(ws.path(), &args, names);
                count += 1;
            }
        }
    }
    // The core's 27 commands (9 with verbs) and 28 verbs: 46 invocations
    // that take flags, 8 spellings each; 9 bare verb-commands, 4 each.
    assert_eq!(count, 46 * 8 + 9 * 4);
}

/// The case that found it: the flags quoted into one argument. `lock`
/// never saw `--check`, wrote the lock, and exited 0.
#[test]
fn lock_refuses_check_and_provenance_passed_as_one_argument() {
    let ws = gitea_copy();
    let dir = ws.path().to_string_lossy().into_owned();
    let lock = ws.path().join(".factory/applied.lock.yaml");
    let before = std::fs::read(&lock).unwrap();
    for bad in ["--check --provenance", "--chekc"] {
        let args = vec!["lock".to_string(), dir.clone(), bad.to_string()];
        assert_refused(ws.path(), &args, &format!("{:?}", bad));
        let (_, _, stderr) = run(ws.path(), &args);
        assert!(stderr.contains("lock accepts: --check"), "{}", stderr);
        if bad.contains(' ') {
            assert!(stderr.contains("each flag as its own word"), "{}", stderr);
        }
    }
    assert_eq!(std::fs::read(&lock).unwrap(), before);
    // The same flags as two words still run the check, read-only.
    let (code, _, stderr) = run(
        ws.path(),
        &["lock".into(), dir, "--check".into(), "--provenance".into()],
    );
    assert_ne!(code, Some(2), "{}", stderr);
    assert_eq!(std::fs::read(&lock).unwrap(), before);
}

/// A valued flag's value is not a flag, even when it starts with a dash,
/// and `--name=value` is the same flag as `--name value`.
#[test]
fn a_valued_flags_value_is_never_checked_as_a_flag() {
    let flags = cmd::flags("inventory", Some("list")).unwrap();
    let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    assert!(flags
        .check(&s(&["--grep", "-x", "--offset=-1", "--json"]))
        .is_ok());
    assert!(flags.check(&s(&["--limit", "-1"])).is_ok());
    assert_eq!(flags.check(&s(&["--json", "-x"])), Err("-x".to_string()));
    assert_eq!(flags.check(&s(&["--tags=a"])), Err("--tags=a".to_string()));
}

/// The `--flag` tokens in `text` that `command`/`verb` does not accept.
fn undeclared(command: &str, verb: Option<&str>, text: &str) -> Vec<String> {
    let flags = cmd::flags(command, verb)
        .unwrap_or_else(|| panic!("no flag set for {} {:?}", command, verb));
    let re = regex::Regex::new(r"--([a-z][a-z0-9-]*)").unwrap();
    re.captures_iter(text)
        .map(|c| c[1].to_string())
        .filter(|f| !flags.accepts(f) && f != "help")
        .collect()
}

/// The lines of a usage text that describe `verb`: a line whose first
/// word (after `usage:`, the binary's name and the command) is the verb,
/// and the indented lines that continue it. A command's usage may list
/// every verb; each is held to its own verb's flags.
fn verb_lines(command: &str, verb: &str, text: &str) -> String {
    // The usage is written with the shared name; the bare binary prints its own.
    let is_bin = |w: &str| w == graphos_factory_core::CORE_NAME || w == "graphos-factory-bare";
    let mut current: Option<String> = None;
    let mut out = String::new();
    for line in text.lines() {
        let words: Vec<&str> = line
            .split_whitespace()
            .skip_while(|w| *w == "usage:" || is_bin(w) || *w == command)
            .collect();
        match words.first() {
            Some(w) if help::is_verb(command, w) => current = Some(w.to_string()),
            _ if line.starts_with(' ') && !line.split_whitespace().next().is_some_and(is_bin) => {}
            _ => current = None,
        }
        if current.as_deref() == Some(verb) {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(
        !out.is_empty(),
        "no usage line for {} {}:\n{}",
        command,
        verb,
        text
    );
    out
}

/// Every flag a command's own `--help` text names is one it accepts.
#[test]
fn every_flag_a_usage_names_is_accepted() {
    let mut problems = Vec::new();
    for inv in invocations() {
        let verb = inv.get(1).map(String::as_str);
        if verb.is_none() && !help::verbs(&inv[0]).is_empty() {
            continue;
        }
        let text = help::usage(&inv[0], &inv[1..]).unwrap();
        // `spans obligations` prints source-coverage's usage (ADR 0082).
        let text = match verb {
            Some(v) if inv[0] != "spans" => verb_lines(&inv[0], v, &text),
            _ => text,
        };
        for f in undeclared(&inv[0], verb, &text) {
            problems.push(format!("{:?}: --{}", inv, f));
        }
    }
    assert!(problems.is_empty(), "{:#?}", problems);
}

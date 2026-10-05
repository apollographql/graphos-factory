//! The unit layer's verdicts from the real `unit.sh` (ADR 0046): a suite
//! with zero cases is `not_run` with its header's reason, never a pass, and
//! fails when that reason cites no decision; cases are counted per suite; a
//! skipped case is never a pass; a suite with cases passes as before; a
//! missing suite fails. rover is a stub on PATH that lists suites and cases
//! the way the real one does, so what is under test is the wrapper's reading
//! of that listing and evidence's recording of it.

use serde_json::Value;
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

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Env {
    _root: tempfile::TempDir,
    ws: PathBuf,
    bin: PathBuf,
    home: PathBuf,
}

const EMPTY_SUITE: &str = "# rover connector test suite - layer 2 of the validation stack.\n# Deliberately empty (D-0099). No request here can be asserted by rover.\nconfig:\n  schema: gitea.graphql\n  common:\n    variables:\n      $config:\n        GITEA_TOKEN: test-token\ntests: []\n";

/// rover 0.41's `connector test` output, reduced to what unit.sh reads: a
/// `TEST SUITE: <path> ` line per suite in `-d`, a `TEST CASE:` line per case
/// that is not `skip: true` (a skipped case prints none), and the summary
/// with its double space. Two assertions per case, as `passed` counts
/// assertions, not cases. Like rover, it walks subdirectories of `-d`. Two
/// files beside it steer a test: `summary` replaces the summary line (`none`
/// drops it), `extra-suite` names a suite to list that is not in `-d`, and
/// `omit` holds a path fragment whose suites it does not list.
const STUB_ROVER: &str = r##"#!/usr/bin/env bash
here="$(dirname "$0")"
case "$1" in
  --version) echo 'Rover 0.41.0' ;;
  install) ;;
  connector)
    while [ $# -gt 0 ]; do [ "$1" = -d ] && dir="$2"; shift; done
    cases=0; skipped=0
    [ -f "$here/extra-suite" ] && echo "TEST SUITE: $(cat "$here/extra-suite") "
    find "$dir" -type f -name '*.connector.yaml' | sort > "$here/suites"
    if [ -f "$here/omit" ]; then { grep -v -F -f "$here/omit" "$here/suites" || true; } > "$here/kept"; mv "$here/kept" "$here/suites"; fi
    while IFS= read -r s; do
      echo "TEST SUITE: $s "
      while IFS= read -r line; do
        case "$line" in
          SKIP) skipped=$((skipped + 1)) ;;
          CASE*) echo "TEST CASE: ${line#CASE } @Query.stub"; cases=$((cases + 1)) ;;
        esac
      done < <(awk '/^  - name: "/ { if (n != "") print n; n = $0; sub(/^  - name: "/, "CASE ", n); sub(/"\r?$/, "", n); next } /^    skip: true/ { n = "SKIP" } END { if (n != "") print n }' "$s")
    done < "$here/suites"
    echo
    if [ -f "$here/summary" ]; then
      [ "$(cat "$here/summary")" = none ] || cat "$here/summary"
    else
      echo "TEST RESULTS: SUCCESSFUL $((cases * 2))  passed; 0 failed; $skipped skipped"
    fi
    ;;
esac
"##;

enum Suite<'a> {
    /// The pilot's own suite, with cases.
    Pilot,
    /// No suite file at all.
    Missing,
    /// The pilot's suite replaced by this text.
    Text(&'a str),
}

/// The gitea pilot with its suite set by `suite`, a stub rover, and a HOME
/// whose rover cache holds a plugin.
fn setup(suite: Suite) -> Env {
    let root = tempfile::tempdir().unwrap();
    let ws = root.path().join("gitea");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea"),
        &ws,
    );
    let suite_file = ws.join("tests/gitea.connector.yaml");
    match suite {
        Suite::Pilot => {}
        Suite::Text(text) => std::fs::write(&suite_file, text).unwrap(),
        Suite::Missing => std::fs::remove_file(&suite_file).unwrap(),
    }
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    executable(&bin.join("rover"), STUB_ROVER);
    let home = root.path().join("home");
    std::fs::create_dir_all(home.join(".rover/bin")).unwrap();
    std::fs::write(home.join(".rover/bin/supergraph-v2.12.0"), "").unwrap();
    Env {
        _root: root,
        ws,
        bin,
        home,
    }
}

fn pilot_suite() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../pilots/graphos/gitea/tests/gitea.connector.yaml"),
    )
    .unwrap()
}

fn scripts() -> PathBuf {
    Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR")).to_path_buf()
}

fn command(env: &Env, program: &str) -> Command {
    let mut c = Command::new(program);
    c.env(
        "PATH",
        format!("{}:{}", env.bin.display(), std::env::var("PATH").unwrap()),
    )
    .env("HOME", &env.home)
    .env(
        "GRAPHOS_FACTORY_CORE_BIN",
        env!("CARGO_BIN_EXE_graphos-factory-bare"),
    );
    c
}

fn unit(env: &Env, args: &[&str]) -> (i32, String) {
    let out = command(env, "bash")
        .arg(scripts().join("unit.sh"))
        .arg(&env.ws)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn evidence(env: &Env) -> Value {
    let out = command(env, env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["evidence", env.ws.to_str().unwrap(), "--scripts"])
        .arg(scripts())
        .args(["--only", "unit"])
        .output()
        .unwrap();
    let text = std::fs::read_to_string(env.ws.join(".factory/evidence/latest.json"))
        .unwrap_or_else(|e| panic!("{}: {}", e, String::from_utf8_lossy(&out.stderr)));
    serde_json::from_str(&text).unwrap()
}

#[test]
fn a_suite_with_zero_cases_is_not_run_with_its_header_reason() {
    let env = setup(Suite::Text(EMPTY_SUITE));
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 3, "{}", out);
    assert!(
        out.contains(
            "unit: no runnable cases: gitea.connector.yaml: Deliberately empty (D-0099). (not_run)"
        ),
        "{}",
        out
    );
    assert!(!out.contains("unit: pass"), "{}", out);
}

#[test]
fn an_empty_suite_whose_header_cites_no_decision_fails() {
    // No header at all: the fallback text is not a reason the gate can take.
    let body = EMPTY_SUITE.split_once("config:").unwrap().1;
    let env = setup(Suite::Text(&format!("config:{}", body)));
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("gitea.connector.yaml has no cases and its header's first sentence cites no decision (D-nnnn): the suite header gives no reason"),
        "{}",
        out
    );
    assert!(!out.contains("no runnable cases"), "{}", out);

    // A header that says nothing about why the suite is empty: the pilots'
    // own opening sentence, with no decision in it.
    let env = setup(Suite::Text(&format!(
        "# rover connector test suite - layer 2 of the validation stack.\n# Asserts the OUTBOUND request each connector builds. It never reaches the network.\nconfig:{}",
        body
    )));
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains(
            "cites no decision (D-nnnn): Asserts the OUTBOUND request each connector builds."
        ),
        "{}",
        out
    );
}

#[test]
fn a_decision_id_past_four_digits_and_a_crlf_header_are_read() {
    // `decisions` numbers D-{:04}, so D-10000 follows D-9999; and a suite
    // saved with CRLF line ends must not carry a carriage return into the
    // recorded reason.
    let body = EMPTY_SUITE.split_once("config:").unwrap().1;
    let text = format!("# Deliberately empty (D-10000)\nconfig:{}", body).replace('\n', "\r\n");
    let env = setup(Suite::Text(&text));
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 3, "{}", out);
    assert!(
        out.contains(
            "unit: no runnable cases: gitea.connector.yaml: Deliberately empty (D-10000) (not_run)"
        ),
        "{:?}",
        out
    );
}

#[test]
fn a_skipped_case_is_never_a_pass() {
    let suite = pilot_suite();
    let cases = suite.matches("\n  - name: \"").count();
    assert!(cases > 1);

    // Every case skipped, under a header that would satisfy the empty-suite
    // rule: rover says SUCCESSFUL with 0 passed, and it is still not the
    // zero-case not_run.
    let all = skip_every_case(&format!("# Deliberately empty (D-0099).\n{}", suite));
    let env = setup(Suite::Text(&all));
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains(&format!("unit: FAIL — {} case(s) marked skip: true", cases)),
        "{}",
        out
    );
    assert!(!out.contains("no runnable cases"), "{}", out);

    // One case skipped beside cases that ran: still not a pass.
    let one = skip_first_case(&suite);
    let env = setup(Suite::Text(&one));
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("unit: FAIL — 1 case(s) marked skip: true"),
        "{}",
        out
    );
    assert!(!out.contains("unit: pass"), "{}", out);
}

fn skip_every_case(suite: &str) -> String {
    let mut out = String::new();
    for line in suite.split_inclusive('\n') {
        out.push_str(line);
        if line.starts_with("  - name: \"") {
            out.push_str("    skip: true\n");
        }
    }
    out
}

fn skip_first_case(suite: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for line in suite.split_inclusive('\n') {
        out.push_str(line);
        if !done && line.starts_with("  - name: \"") {
            out.push_str("    skip: true\n");
            done = true;
        }
    }
    out
}

#[test]
fn an_empty_suite_beside_one_with_cases_must_cite_its_decision() {
    let env = setup(Suite::Pilot);
    let body = EMPTY_SUITE.split_once("config:").unwrap().1;
    let extra = env.ws.join("tests/aaa.connector.yaml");

    std::fs::write(
        &extra,
        format!("# rover connector test suite\nconfig:{}", body),
    )
    .unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("aaa.connector.yaml has no cases and its header's first sentence cites no decision (D-nnnn)"),
        "{}",
        out
    );
    assert!(!out.contains("unit: pass"), "{}", out);

    std::fs::write(
        &extra,
        format!(
            "# Reserved for the admin API (D-0100). Nothing yet.\nconfig:{}",
            body
        ),
    )
    .unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 0, "{}", out);
    assert!(
        out.contains(
            "unit: empty beside suites that ran: aaa.connector.yaml: Reserved for the admin API (D-0100)."
        ),
        "{}",
        out
    );
    assert!(out.contains("unit: pass"), "{}", out);
}

#[test]
fn only_counts_the_suites_it_selected() {
    // An uncited empty suite that --only leaves out is not part of the run.
    let env = setup(Suite::Pilot);
    let body = EMPTY_SUITE.split_once("config:").unwrap().1;
    std::fs::write(
        env.ws.join("tests/aaa.connector.yaml"),
        format!("config:{}", body),
    )
    .unwrap();
    let (code, out) = unit(&env, &["--only", "gitea_version"]);
    assert_eq!(code, 0, "{}", out);
    assert!(
        out.contains("unit: --only 'gitea_version' matched 1 case(s)"),
        "{}",
        out
    );
    assert!(out.contains("unit: pass"), "{}", out);
    assert!(!out.contains("aaa.connector.yaml"), "{}", out);

    // The same with the empty list spaced and commented.
    std::fs::write(
        env.ws.join("tests/aaa.connector.yaml"),
        format!("config:{}", body).replace("tests: []", "tests: [ ]  # none yet"),
    )
    .unwrap();
    let (code, out) = unit(&env, &["--only", "gitea_version"]);
    assert_eq!(code, 0, "{}", out);
    assert!(out.contains("unit: pass"), "{}", out);
}

#[test]
fn a_failed_run_names_rovers_summary() {
    let env = setup(Suite::Pilot);
    std::fs::write(
        env.bin.join("summary"),
        "TEST RESULTS: FAILED 16  passed; 2 failed; 0 skipped\n",
    )
    .unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    let evidence = evidence(&env);
    assert_eq!(
        evidence["layers"]["connector_unit"]["reason"],
        "unit: FAIL — TEST RESULTS: FAILED 16  passed; 2 failed; 0 skipped",
        "{}",
        out
    );

    std::fs::write(env.bin.join("summary"), "none").unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("unit: FAIL — rover printed no TEST RESULTS summary line"),
        "{}",
        out
    );
}

#[test]
fn a_suite_in_a_subdirectory_is_counted_by_its_path() {
    // rover's -d walks subdirectories; an uncited empty suite there fails.
    let env = setup(Suite::Pilot);
    let body = EMPTY_SUITE.split_once("config:").unwrap().1;
    std::fs::create_dir_all(env.ws.join("tests/sub")).unwrap();
    std::fs::write(
        env.ws.join("tests/sub/zzz.connector.yaml"),
        format!("config:{}", body),
    )
    .unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("unit: FAIL — sub/zzz.connector.yaml has no cases"),
        "{}",
        out
    );

    // Two cited empty suites with one file name in two directories are two
    // suites, not one name counted twice.
    std::fs::remove_file(env.ws.join("tests/sub/zzz.connector.yaml")).unwrap();
    let cited = format!("# Reserved (D-0100).\nconfig:{}", body);
    std::fs::write(env.ws.join("tests/aaa.connector.yaml"), &cited).unwrap();
    std::fs::write(env.ws.join("tests/sub/aaa.connector.yaml"), &cited).unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 0, "{}", out);
    assert!(
        out.contains("unit: empty beside suites that ran: aaa.connector.yaml: Reserved (D-0100).; sub/aaa.connector.yaml: Reserved (D-0100)."),
        "{}",
        out
    );
}

#[test]
fn a_suite_rover_and_unit_sh_do_not_both_account_for_fails() {
    // rover lists a suite that is no file under tests/.
    let env = setup(Suite::Pilot);
    std::fs::write(
        env.bin.join("extra-suite"),
        "/elsewhere/tests/x.connector.yaml",
    )
    .unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("unit: FAIL — rover ran /elsewhere/tests/x.connector.yaml, which unit.sh cannot account for"),
        "{}",
        out
    );

    // A suite file under tests/ that rover does not list.
    let env = setup(Suite::Pilot);
    std::fs::write(env.bin.join("omit"), "gitea.connector.yaml\n").unwrap();
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("unit: FAIL — rover printed no TEST SUITE line for gitea.connector.yaml"),
        "{}",
        out
    );
}

#[test]
fn a_suite_with_cases_still_passes_and_a_missing_suite_still_fails() {
    let env = setup(Suite::Pilot);
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 0, "{}", out);
    assert!(out.contains("unit: pass"), "{}", out);
    assert!(!out.contains("empty beside"), "{}", out);

    let env = setup(Suite::Missing);
    let (code, out) = unit(&env, &[]);
    assert_eq!(code, 1, "{}", out);
    assert!(out.contains("this is a failure, not a skip"), "{}", out);
}

#[test]
fn evidence_records_a_zero_case_unit_layer_as_not_run_and_every_operation_n_a() {
    let env = setup(Suite::Text(EMPTY_SUITE));
    let evidence = evidence(&env);
    let layer = &evidence["layers"]["connector_unit"];
    assert_eq!(layer["status"], "not_run", "{}", layer);
    assert_eq!(layer["exit_code"], 3, "{}", layer);
    assert_eq!(layer["cases"], 0, "{}", layer);
    // The whole line, as workspace-contract.md describes it: a reader that
    // matches on its prefix must find `unit: no runnable cases:`.
    assert_eq!(
        layer["reason"],
        "unit: no runnable cases: gitea.connector.yaml: Deliberately empty (D-0099). (not_run)",
        "{}",
        layer
    );
    let ops = evidence["operations"].as_object().unwrap();
    assert!(!ops.is_empty());
    for (key, op) in ops {
        assert_eq!(op["unit"], "n/a", "{}: {}", key, op);
    }
}

#[test]
fn evidence_names_why_the_unit_layer_failed() {
    let body = EMPTY_SUITE.split_once("config:").unwrap().1;
    let env = setup(Suite::Text(&format!("config:{}", body)));
    let evidence = evidence(&env);
    let layer = &evidence["layers"]["connector_unit"];
    assert_eq!(layer["status"], "fail", "{}", layer);
    assert_eq!(
        layer["reason"],
        "unit: FAIL — gitea.connector.yaml has no cases and its header's first sentence cites no decision (D-nnnn): the suite header gives no reason",
        "{}",
        layer
    );
}

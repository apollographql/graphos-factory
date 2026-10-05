//! `source-coverage --check` against a checked-in pilot, read-only, and the
//! `spans obligations` alias it replaced (ADR 0082).

use std::path::Path;

fn check(pilot: &str, op: &str) -> i32 {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pilots/graphos")
        .join(pilot);
    graphos_factory_core::cmd::source_coverage::main(&[
        dir.to_str().unwrap().into(),
        op.into(),
        "--check".into(),
    ])
}

/// gitea `get:/user` had nothing unaccounted but 3 unresolved response rows,
/// a `->match` the walk did not follow, and `--check` failed closed on them.
/// The walk now reads `->match` as value translation (ADR 0050), so the rows
/// are mapped and the check passes. Failing closed on a row that stays
/// unresolved is pinned in tests/obligations_grammar.rs.
#[test]
fn check_passes_once_match_rows_are_read() {
    assert_eq!(check("gitea", "get:/user"), 0);
}

/// `spans obligations` is the pre-ADR 0082 spelling. It runs the same check
/// and gives the same exit code: 0 on a clean operation, 1 on one the
/// workspace does not have.
#[test]
fn spans_obligations_alias_runs_source_coverage() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pilots/graphos/gitea")
        .to_str()
        .unwrap()
        .to_string();
    let alias = |op: &str| {
        graphos_factory_core::cmd::spans::main(&[
            "obligations".into(),
            dir.clone(),
            op.into(),
            "--check".into(),
        ])
    };
    assert_eq!(alias("get:/user"), check("gitea", "get:/user"));
    assert_eq!(alias("get:/user"), 0);
    assert_eq!(alias("get:/no/such/operation"), 1);
    assert_eq!(check("gitea", "get:/no/such/operation"), 1);
}

// ─── No OP-KEY: every selected operation (ADR 0101) ─────────────────────────

fn pilot(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pilots/graphos")
        .join(name)
}

/// Runs the built binary, so stdout and stderr are what a script sees.
fn run(args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("source-coverage")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The operations selection.yaml includes, in inventory order.
fn selected_in_inventory_order(dir: &Path) -> Vec<String> {
    let inv: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(".factory/inventory.json")).unwrap(),
    )
    .unwrap();
    let sel = std::fs::read_to_string(dir.join(".factory/selection.yaml")).unwrap();
    let sel = graphos_factory_core::yaml::parse(&sel).unwrap();
    let ops = sel.get("operations").and_then(|o| o.as_object()).unwrap();
    inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["key"].as_str())
        .filter(|k| {
            ops.get(*k)
                .and_then(|e| e.get("include"))
                .and_then(|i| i.as_bool())
                == Some(true)
        })
        .map(String::from)
        .collect()
}

/// A scratch copy of a pilot, removed on drop.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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

fn scratch(pilot_name: &str, tag: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!(
        "source-coverage-all-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    copy_dir(&pilot(pilot_name), &dir);
    Scratch(dir)
}

/// With no OP-KEY, `--json` lists every selected operation in inventory
/// order, and each object is byte for byte what the single-key `--json`
/// prints for that operation.
#[test]
fn no_key_json_is_the_single_key_json_for_every_selected_operation() {
    let dir = pilot("gitea");
    let d = dir.to_str().unwrap();
    let (code, stdout, _) = run(&[d, "--json"]);
    assert_eq!(code, 0);
    let all: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ops = all["operations"].as_array().unwrap();
    let keys: Vec<&str> = ops.iter().map(|o| o["op_key"].as_str().unwrap()).collect();
    let expected = selected_in_inventory_order(&dir);
    assert_eq!(keys, expected);
    assert_eq!(expected.len(), 8);
    for op in ops {
        let key = op["op_key"].as_str().unwrap();
        let (code, single, _) = run(&[d, key, "--json"]);
        assert_eq!(code, 0, "{}", key);
        assert_eq!(
            format!("{}\n", serde_json::to_string_pretty(op).unwrap()),
            single,
            "{}",
            key
        );
    }
    assert_eq!(all["totals"]["selected"], 8);
    assert_eq!(all["totals"]["failing"], 3);
    assert_eq!(all["totals"]["passing"], 5);
    assert_eq!(all["totals"]["errors"], 0);
    // The totals are the per-operation counts summed, key by key.
    for side in ["response", "request"] {
        let totals = all["totals"][side].as_object().unwrap();
        for key in [
            "offered",
            "mapped",
            "consumed",
            "omitted",
            "unaccounted",
            "unresolved",
        ] {
            let sum: u64 = ops
                .iter()
                .map(|o| o[side]["counts"][key].as_u64().unwrap())
                .sum();
            assert_eq!(totals[key].as_u64().unwrap(), sum, "{}.{}", side, key);
        }
    }
    let offered = all["totals"]["response"]["offered"].as_u64().unwrap()
        + all["totals"]["request"]["offered"].as_u64().unwrap();
    assert!(offered > 0, "the summed counts are not all zero");
    assert_eq!(
        all["failing"],
        serde_json::json!([
            "get:/repos/search",
            "get:/repos/{owner}/{repo}",
            "post:/repos/{owner}/{repo}/issues"
        ])
    );
}

/// `--check` with no OP-KEY exits 1 and names each failing operation on
/// stderr: the same three a per-key loop fails, and only those.
#[test]
fn no_key_check_names_every_failing_operation() {
    let dir = pilot("gitea");
    let d = dir.to_str().unwrap();
    let (code, stdout, stderr) = run(&[d, "--check"]);
    assert_eq!(code, 1, "{}", stderr);
    for key in selected_in_inventory_order(&dir) {
        let single = check("gitea", &key);
        let named = stderr.contains(&format!("source-coverage --check: {} fails", key));
        assert_eq!(named, single == 1, "{}: {}", key, stderr);
        // Every operation gets one line on stdout, passing or not, and
        // `FAIL` ends it exactly when the operation fails the bar.
        let line = stdout
            .lines()
            .find(|l| l.starts_with(&format!("{}  ", key)))
            .unwrap_or_else(|| panic!("{}: {}", key, stdout));
        assert_eq!(line.ends_with("  FAIL"), single == 1, "{}", line);
    }
    assert!(
        stderr.contains("3 of 8 selected operations fail"),
        "{}",
        stderr
    );
}

/// Nothing selected is nothing checked, and nothing checked is not a pass.
#[test]
fn no_key_with_no_selected_operation_exits_1() {
    let ws = scratch("gitea", "none");
    std::fs::write(
        ws.0.join(".factory/selection.yaml"),
        "contract_version: 1\noperations: {}\n",
    )
    .unwrap();
    let (code, _, stderr) = run(&[ws.0.to_str().unwrap(), "--check"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("no operation is selected"), "{}", stderr);
}

/// An operation the classifier cannot build a report for is reported and
/// counted failing, never skipped.
#[test]
fn no_key_reports_an_operation_the_classifier_errors_on() {
    let ws = scratch("gitea", "missing");
    let sel = ws.0.join(".factory/selection.yaml");
    // Inserted first under `operations:`: the pilot's selection carries
    // `waivers:` and `links:` after it.
    let text = std::fs::read_to_string(&sel).unwrap().replacen(
        "\noperations:\n",
        "\noperations:\n  \"get:/no/such/operation\":\n    include: true\n    graphql: { root: query, name: nothing }\n",
        1,
    );
    std::fs::write(&sel, text).unwrap();
    let d = ws.0.to_str().unwrap();
    let (code, stdout, stderr) = run(&[d, "--json"]);
    assert_eq!(code, 1, "{}", stderr);
    let all: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ops = all["operations"].as_array().unwrap();
    assert_eq!(ops.len(), 9);
    let last = ops.last().unwrap();
    assert_eq!(last["op_key"], "get:/no/such/operation");
    assert!(last["error"]
        .as_str()
        .unwrap()
        .contains("no operation \"get:/no/such/operation\" in inventory.json"));
    assert_eq!(
        all["failing"].as_array().unwrap().last().unwrap(),
        "get:/no/such/operation"
    );
    assert_eq!(all["totals"]["errors"], 1);
    // Without --check the reason is on stderr too, not only in stdout.
    assert!(
        stderr.contains(
            "source-coverage: get:/no/such/operation: no operation \"get:/no/such/operation\" in inventory.json"
        ),
        "{}",
        stderr
    );
    let (code, stdout, stderr) = run(&[d]);
    assert_eq!(code, 1);
    assert!(
        stdout.contains("get:/no/such/operation  error: "),
        "{}",
        stdout
    );
    assert!(
        stdout.contains("4 of 9 selected operations fail the bar (1 could not be classified)"),
        "{}",
        stdout
    );
    assert!(
        stderr.contains("1 of 9 selected operations could not be classified"),
        "{}",
        stderr
    );
    let (code, _, stderr) = run(&[d, "--check"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("source-coverage --check: get:/no/such/operation fails"),
        "{}",
        stderr
    );
}

/// With no selection.yaml the message says the file is missing, not that
/// it selects nothing.
#[test]
fn no_key_without_selection_yaml_says_the_file_is_missing() {
    let ws = scratch("gitea", "nosel");
    std::fs::remove_file(ws.0.join(".factory/selection.yaml")).unwrap();
    let (code, _, stderr) = run(&[ws.0.to_str().unwrap(), "--check"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("no operation is selected (there is no .factory/selection.yaml)"),
        "{}",
        stderr
    );
}

/// A workspace path that does not exist is reported as one, not as an
/// OP-KEY given without a workspace.
#[test]
fn a_mistyped_workspace_path_is_reported_as_missing() {
    let missing = pilot("gitae");
    let m = missing.to_str().unwrap();
    let (code, _, stderr) = run(&[m]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains(&format!("{}: no such workspace directory", m)),
        "{}",
        stderr
    );
    assert!(!stderr.contains("source-coverage . "), "{}", stderr);
    // A file where the workspace should be is not a directory.
    let file = pilot("gitea").join(".factory/selection.yaml");
    let (code, _, stderr) = run(&[file.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(stderr.contains(": not a directory"), "{}", stderr);
}

/// An OP-KEY given without a workspace is not a directory: the command says
/// how to name both rather than reporting a missing workspace.yaml.
#[test]
fn a_lone_op_key_is_told_to_name_the_workspace_first() {
    let (code, _, stderr) = run(&["get:/no/such/dir"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("get:/no/such/dir is an operation key, not a workspace directory"),
        "{}",
        stderr
    );
    assert!(
        stderr.contains("source-coverage . get:/no/such/dir"),
        "{}",
        stderr
    );
}

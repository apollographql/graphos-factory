//! The real `scripts/live.sh`, run against stubs: `rover` prints a schema,
//! the router is a process that sleeps, and `curl` answers the health check
//! and every query with data. What is under test is the script's own case and
//! exclusion loops, not the router or an upstream.
//!
//! `seq` is stubbed with BSD's behaviour — `seq 0 -1` counts down and prints
//! `0` and `-1` — so an index loop built on it fails here on every platform,
//! not only on macOS.

use std::path::Path;
use std::process::Command;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn stub(dir: &Path, name: &str, body: &str) {
    let f = dir.join(name);
    std::fs::write(&f, format!("#!/usr/bin/env bash\n{}\n", body)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Run live.sh on a copy of pilots/graphos/gitea whose tests/live.yaml is
/// `live_yaml`, with a stand-in for the credential its AUTH_EXPR names.
/// Returns (exit code, stdout).
fn run_live(live_yaml: &str) -> (Option<i32>, String) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let ws = tempfile::tempdir().unwrap();
    copy_dir(&repo.join("pilots/graphos/gitea"), ws.path());
    std::fs::write(ws.path().join("tests/live.yaml"), live_yaml).unwrap();
    std::fs::write(
        ws.path().join("tests/live/ping.graphql"),
        "query { __typename }\n",
    )
    .unwrap();

    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "rover", "echo 'type Query { ok: Int }'");
    // A query arrives on stdin (`jq … | curl … -d @-`): the stub drains it as
    // the real curl does. Exiting without reading would race jq's write, and
    // under live.sh's `pipefail` a broken pipe reads as a curl error.
    stub(
        bin.path(),
        "curl",
        r#"case "$*" in
  *"/health"*) exit 0 ;;
  *"-X POST"*) cat > /dev/null; echo '{"data":{"ok":1}}' ;;
  *) exit 7 ;;
esac"#,
    );
    stub(
        bin.path(),
        "seq",
        r#"first=$1; last=$2
if [ "$first" -le "$last" ]; then i=$first; while [ "$i" -le "$last" ]; do echo "$i"; i=$((i+1)); done
else i=$first; while [ "$i" -ge "$last" ]; do echo "$i"; i=$((i-1)); done; fi"#,
    );
    let cache = tempfile::tempdir().unwrap();
    stub(cache.path(), "router-2.17.0", "sleep 30");

    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("bash")
        .arg(Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR")).join("live.sh"))
        .arg(ws.path())
        .env("PATH", path)
        .env(
            "GRAPHOS_FACTORY_CORE_BIN",
            env!("CARGO_BIN_EXE_graphos-factory-bare"),
        )
        .env("GRAPHOS_FACTORY_CORE_CACHE", cache.path())
        .env("ROUTER_VERSION", "2.17.0")
        .env("GITEA_TOKEN", "stand-in")
        .env("APOLLO_ELV2_LICENSE", "accept")
        .env_remove("GRAPHOS_FACTORY_CORE_LIVE_ENV")
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
}

#[test]
fn no_cases_and_no_exclusions_is_zero_of_each_not_a_failure() {
    let (code, stdout) = run_live("cases: []\nexclusions: []\n");
    assert!(!stdout.contains("FAIL"), "{}", stdout);
    assert!(
        stdout.contains("live: 0 passed, 0 failed, 0 excluded"),
        "{}",
        stdout
    );
    assert_eq!(code, Some(0), "{}", stdout);
}

#[test]
fn cases_and_exclusions_are_each_visited_once() {
    let (code, stdout) = run_live(
        "cases:\n  - name: ping\nexclusions:\n  - operation: \"get:/a\"\n    reason: needs an id\n  - operation: \"get:/b\"\n    reason: a write\n",
    );
    assert!(stdout.contains("PASS: ping (read)"), "{}", stdout);
    assert!(
        stdout.contains("EXCLUDED: get:/a — needs an id"),
        "{}",
        stdout
    );
    assert!(stdout.contains("EXCLUDED: get:/b — a write"), "{}", stdout);
    assert!(
        stdout.contains("live: 1 passed, 0 failed, 2 excluded"),
        "{}",
        stdout
    );
    assert_eq!(code, Some(0), "{}", stdout);
}

/// ADR 0106: a relationship field is excluded by name, on its own prefix,
/// so evidence cannot read it as an operation row.
#[test]
fn a_field_exclusion_is_printed_on_its_own_prefix_and_counted() {
    let (code, stdout) = run_live(
        "cases: []\nexclusions:\n  - field: \"X_Payment.card\"\n    reason: its only parent is a write\n  - operation: \"get:/a\"\n    reason: needs an id\n",
    );
    assert!(
        stdout.contains("EXCLUDED FIELD: X_Payment.card — its only parent is a write\n"),
        "{}",
        stdout
    );
    assert!(!stdout.contains("EXCLUDED: X_Payment.card"), "{}", stdout);
    assert!(
        stdout.contains("EXCLUDED: get:/a — needs an id"),
        "{}",
        stdout
    );
    assert!(
        stdout.contains("live: 0 passed, 0 failed, 2 excluded"),
        "{}",
        stdout
    );
    assert_eq!(code, Some(0), "{}", stdout);
}

#[test]
fn an_exclusion_naming_both_neither_or_no_reason_fails() {
    for (yaml, line) in [
        (
            "cases: []\nexclusions:\n  - operation: \"get:/a\"\n    field: \"X_Payment.card\"\n    reason: r\n",
            "FAIL: exclusion 0 names both an operation (get:/a) and a field (X_Payment.card)",
        ),
        (
            "cases: []\nexclusions:\n  - reason: r\n",
            "FAIL: exclusion 0 names no operation or field",
        ),
        (
            "cases: []\nexclusions:\n  - field: \"X_Payment.card\"\n",
            "FAIL: exclusion for X_Payment.card gives no reason",
        ),
    ] {
        let (code, stdout) = run_live(yaml);
        assert!(stdout.contains(line), "{}: {}", yaml, stdout);
        assert!(!stdout.contains("EXCLUDED"), "{}: {}", yaml, stdout);
        assert!(
            stdout.contains("live: 0 passed, 1 failed, 0 excluded"),
            "{}: {}",
            yaml,
            stdout
        );
        assert_eq!(code, Some(1), "{}: {}", yaml, stdout);
    }
}

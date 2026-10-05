//! A resolved `omits` entry no offered row needs any more (ADR 0103):
//! `source-coverage` counts and lists it, `--check` does not fail on it, and
//! `lint` warns `stale-omit` naming the decision and the row. Each way an
//! entry goes stale — the row is now mapped, the row is now read by the
//! envelope, the source no longer offers the path, the inventory no longer
//! has the operation — has its own case, and each case has a control that
//! is not stale. The stale cases read the deck-of-cards pilot, which is a
//! target's, and live in that target's suite; this file keeps the control
//! that an entry still in use is not stale, on the public gitea pilot.

use serde_json::Value;
use std::path::{Path, PathBuf};

fn pilot(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pilots/graphos")
        .join(name)
}

/// A scratch copy of a pilot, removed on drop.
struct Scratch(PathBuf);

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
        "stale-omit-{}-{}-{}",
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

fn sf(args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// Records a resolved decision carrying `omit` through the only writer, and
/// returns its id.
fn omit(ws: &Path, omit: &str) -> String {
    let (code, stdout, stderr) = sf(&[
        "decisions",
        "add",
        "--question",
        "What does this record decide?",
        ws.to_str().unwrap(),
        "--title",
        "Leave it out",
        "--resolved",
        "--decision",
        "Not exposed.",
        "--omit",
        omit,
    ]);
    assert_eq!(code, 0, "{}{}", stdout, stderr);
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(ws.join(".factory/decisions.json")).unwrap())
            .unwrap();
    doc["decisions"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn coverage_json(ws: &Path, op: &str) -> Value {
    let (code, stdout, stderr) = sf(&["source-coverage", ws.to_str().unwrap(), op, "--json"]);
    assert_eq!(code, 0, "{}", stderr);
    serde_json::from_str(&stdout).unwrap()
}

fn stale_findings(ws: &Path) -> Vec<String> {
    let (_, stdout, _) = sf(&["lint", ws.to_str().unwrap(), "--json", "--skip-evidence"]);
    let lint: Value = serde_json::from_str(&stdout).unwrap();
    lint["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["rule"] == "stale-omit")
        .map(|f| {
            assert_eq!(f["severity"], "warn", "{}", f);
            assert_eq!(f["file"], ".factory/decisions.json", "{}", f);
            f["message"].as_str().unwrap().to_string()
        })
        .collect()
}

/// An entry still in use is not stale: a row it alone accounts for, and a
/// parent path whose children are part mapped, part omitted.
#[test]
fn an_omit_still_in_use_is_not_stale() {
    let ws = scratch("gitea", "in-use");
    omit(
        &ws.0,
        "get:/repos/search|response|data[].allow_manual_merge|editorial",
    );
    omit(
        &ws.0,
        "get:/repos/search|response|data[].external_tracker|editorial",
    );
    let report = coverage_json(&ws.0, "get:/repos/search");
    assert_eq!(report["response"]["counts"]["omitted"], 5);
    assert!(report["response"].get("stale_omits").is_none());
    assert!(stale_findings(&ws.0).is_empty());

    // `data[]` covers mapped and unaccounted rows alike; the unaccounted
    // ones need it, so it is in use.
    let parent = scratch("gitea", "parent");
    omit(&parent.0, "get:/repos/search|response|data[]|editorial");
    let report = coverage_json(&parent.0, "get:/repos/search");
    assert_eq!(report["response"]["counts"]["unaccounted"], 0);
    assert!(report["response"].get("stale_omits").is_none());
    assert!(stale_findings(&parent.0).is_empty());
}

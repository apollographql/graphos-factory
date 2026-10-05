//! `graphos-factory-core findings` (ADR 0113): the facts a reference, an ADR or
//! the wire settled, recorded because an instrument reads their `omits` or
//! `affects`. The verb is the only writer of `.factory/findings.json`;
//! ids are `F-nnnn`; a finding is `current` until superseded; an
//! `editorial` omit is refused, since "expose it" is always its alternative
//! and it lives on a decision.

use serde_json::{json, Value};
use std::path::Path;
use tempfile::TempDir;

fn findings(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec![args[0].to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args[1..].iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::findings::main(&argv)
}

/// The binary, for the `--json` refusal shape on stdout.
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

fn read_doc(dir: &Path) -> Value {
    graphos_factory_core::json::parse(
        &std::fs::read_to_string(dir.join(".factory/findings.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn add_records_every_field_with_sequential_ids_and_supersede_flips_only_the_status() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        findings(
            d,
            &[
                "add",
                "--title",
                "errors[] and success are consumed by the error mapping",
                "--body",
                "The @source errors configuration reads both on every operation.",
                "--cites",
                "ADR 0103",
                "--affects",
                "post:/candidate.info",
                "--omit",
                "post:/candidate.info|response|errors|consumed",
                "--omit",
                "post:/candidate.info|behaviour|query:x|not-applicable",
                "--evidence",
                "graphos-factory-core source-coverage . post:/candidate.info",
                "--related",
                "D-0001",
                "--date",
                "2026-10-01",
            ],
        ),
        0
    );
    assert_eq!(
        findings(
            d,
            &[
                "add",
                "--title",
                "Enums keep wire casing",
                "--body",
                "naming.md settles it.",
                "--related",
                "F-0001"
            ]
        ),
        0
    );
    let doc = read_doc(d);
    assert_eq!(doc["contract_version"], 1);
    let recs = doc["findings"].as_array().unwrap();
    assert_eq!(recs.len(), 2);
    assert_eq!(
        recs[0],
        json!({
            "id": "F-0001",
            "title": "errors[] and success are consumed by the error mapping",
            "date": "2026-10-01",
            "status": "current",
            "body": "The @source errors configuration reads both on every operation.",
            "cites": "ADR 0103",
            "source": "agent",
            "affects": ["post:/candidate.info"],
            "omits": [
                {"operation": "post:/candidate.info", "direction": "response", "path": "errors", "reason": "consumed"},
                {"operation": "post:/candidate.info", "direction": "behaviour", "path": "query:x", "reason": "not-applicable"}
            ],
            "evidence": ["graphos-factory-core source-coverage . post:/candidate.info"],
            "related": ["D-0001"]
        })
    );
    assert_eq!(recs[1]["id"], "F-0002");
    assert_eq!(recs[1]["source"], "agent");
    // No question, choices or resolution: a finding has no alternative.
    for k in ["question", "choices", "resolution"] {
        assert!(recs[1].get(k).is_none(), "{}", k);
    }

    let (code, stdout, _) = sf(&["findings", "list", d.to_str().unwrap(), "--json"]);
    assert_eq!(code, 0);
    let listed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(listed["findings"].as_array().unwrap().len(), 2);

    let before = read_doc(d);
    let (code, stdout, _) = sf(&[
        "findings",
        "supersede",
        d.to_str().unwrap(),
        "--id",
        "F-0001",
        "--json",
    ]);
    assert_eq!(code, 0);
    let out: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(out["status"], "superseded");
    let after = read_doc(d);
    let mut expected = before.clone();
    expected["findings"][0]["status"] = json!("superseded");
    assert_eq!(after, expected, "only the status changes");

    // Again: already superseded; an unknown id: unknown-finding.
    for (id, code_name) in [
        ("F-0001", "already-superseded"),
        ("F-0099", "unknown-finding"),
    ] {
        let (code, stdout, _) = sf(&[
            "findings",
            "supersede",
            d.to_str().unwrap(),
            "--id",
            id,
            "--json",
        ]);
        assert_eq!(code, 1);
        let out: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(out["code"], code_name, "{}", stdout);
        assert_eq!(out["exit"], 1);
    }
    assert_eq!(read_doc(d), after, "a refusal writes nothing");
}

#[test]
fn add_refuses_an_editorial_omit_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let (code, stdout, stderr) = sf(&[
        "findings",
        "add",
        d.to_str().unwrap(),
        "--title",
        "Left out",
        "--body",
        "Not exposed.",
        "--omit",
        "get:/x|response|secret|editorial",
        "--json",
    ]);
    assert_eq!(code, 1, "{}", stderr);
    let out: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(out["code"], "editorial-omit");
    assert!(stderr.contains("lives only on a decision"), "{}", stderr);
    assert!(!d.join(".factory/findings.json").exists());
    // The direction's vocabulary still holds: consumed is a wire reason,
    // not-applicable a behaviour one.
    for spec in [
        "get:/x|behaviour|query:y|consumed",
        "get:/x|response|y|not-applicable",
        "get:/x|sideways|y|consumed",
        "get:/x|response|y",
    ] {
        assert_eq!(
            findings(d, &["add", "--title", "t", "--body", "b", "--omit", spec]),
            1,
            "{}",
            spec
        );
    }
    assert!(!d.join(".factory/findings.json").exists());
}

#[test]
fn add_needs_a_title_a_body_and_a_known_source() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(findings(d, &["add", "--body", "b"]), 1);
    assert_eq!(findings(d, &["add", "--title", "t"]), 1);
    assert_eq!(
        findings(
            d,
            &["add", "--title", "t", "--body", "b", "--source", "user"]
        ),
        1
    );
    assert_eq!(
        findings(
            d,
            &["add", "--title", "t", "--body", "b", "--related", "X-1"]
        ),
        1
    );
    assert!(!d.join(".factory/findings.json").exists());
    assert_eq!(
        findings(
            d,
            &["add", "--title", "t", "--body", "b", "--source", "sources"]
        ),
        0
    );
    assert_eq!(read_doc(d)["findings"][0]["source"], "sources");
    assert_eq!(findings(d, &["bogus"]), 1);
}

#[cfg(unix)]
#[test]
fn a_symlinked_findings_file_is_refused_and_its_target_untouched() {
    // Custody (ADR 0025): every `.factory/findings.json` read and write.
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let elsewhere = TempDir::new().unwrap();
    let target = elsewhere.path().join("someone-elses.json");
    let valid = "{\"contract_version\": 1, \"findings\": []}\n";
    std::fs::write(&target, valid).unwrap();
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    std::os::unix::fs::symlink(&target, d.join(".factory/findings.json")).unwrap();
    assert_eq!(findings(d, &["add", "--title", "t", "--body", "b"]), 1);
    assert_eq!(findings(d, &["list"]), 1);
    assert_eq!(findings(d, &["supersede", "--id", "F-0001"]), 1);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), valid);
}

#[test]
fn an_invalid_file_is_refused_never_reset() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    let bad = "{\"contract_version\": 1, \"findings\": [{\"id\": \"D-0001\"}]}";
    std::fs::write(d.join(".factory/findings.json"), bad).unwrap();
    assert_eq!(findings(d, &["add", "--title", "t", "--body", "b"]), 1);
    assert_eq!(
        std::fs::read_to_string(d.join(".factory/findings.json")).unwrap(),
        bad
    );
    // And supersede with no file at all: findings-missing.
    let empty = TempDir::new().unwrap();
    let (code, stdout, _) = sf(&[
        "findings",
        "supersede",
        empty.path().to_str().unwrap(),
        "--id",
        "F-0001",
        "--json",
    ]);
    assert_eq!(code, 1);
    let out: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(out["code"], "findings-missing");
    assert!(!empty.path().join(".factory").exists());
}

#[test]
fn the_union_reads_a_current_finding_as_resolved_and_a_superseded_one_as_superseded() {
    let decisions = json!({"contract_version": 1, "decisions": [
        {"id": "D-0001", "title": "t", "status": "resolved", "date": "d", "resolution": {"decision": "x"}}
    ]});
    let findings = json!({"contract_version": 1, "findings": [
        {"id": "F-0001", "title": "a", "date": "d", "status": "current", "body": "b1", "source": "agent", "cites": "ADR 0032", "affects": ["T.f"]},
        {"id": "F-0002", "title": "b", "date": "d", "status": "superseded", "body": "b2", "source": "agent"}
    ]});
    let u = graphos_factory_core::findings::union(&decisions, &findings);
    let recs = u["decisions"].as_array().unwrap();
    assert_eq!(recs.len(), 3);
    assert_eq!(recs[1]["id"], "F-0001");
    assert_eq!(recs[1]["status"], "resolved");
    assert_eq!(recs[1]["context"], "ADR 0032");
    assert_eq!(recs[1]["resolution"]["decision"], "b1");
    assert_eq!(recs[1]["affects"], json!(["T.f"]));
    assert_eq!(recs[2]["status"], "superseded");
}

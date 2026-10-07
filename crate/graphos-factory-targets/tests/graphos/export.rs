//! `export` (Phase 8j): the gate over the evidence, the render with the
//! production values, the refusals, and the rover hand-off, on copies of
//! this target's pilot. Export refuses evidence that is not for the
//! workspace as it is now, by the `inputs` digests `evidence` records, so
//! a copy that should get past that carries the pilot's evidence with the
//! `inputs` a real `evidence` run on the copy recorded (`stamped_copy`),
//! and no git at all. Evidence with no `inputs` (written before they
//! existed) is checked by its `commit` against git instead: such a copy is
//! made a git repository, committed, and its evidence's `commit` set to
//! that commit (`current_copy`).

use super::{bin, copy_of, graphos_pilot, script};
use graphos_factory_targets::targets::graphos::export;
use serde_json::Value;
use std::path::Path;

pub(super) const HOST: &str = "https://git.example.com/api/v1";

/// `graphos-factory export <ws> <args…>` with no `GITEA_*` override from
/// the environment the suite runs in, and `extra_env` set.
fn export_in(
    ws: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String, String) {
    let mut c = bin();
    c.env_remove("GITEA_BASE_URL").env_remove("GITEA_AUTH_EXPR");
    for (k, v) in extra_env {
        c.env(k, v);
    }
    let out = c.arg("export").arg(ws).args(args).output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

pub(super) fn export_ok(ws: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    export_in(ws, args, &[])
}

/// `git <args>` in `dir`, with no user or system configuration; its stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.test")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.test")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {:?}: {:?}", args, out);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A copy of the pilot, `edit`ed, outside git, whose evidence is the
/// pilot's with the `inputs` that `evidence` itself records on the copy as
/// it is now: a run of the one layer a stub wrapper answers, whose
/// `latest.json` gives its `inputs` and is then replaced. What export
/// compares is what the writer wrote.
fn stamped_copy_with(edit: impl FnOnce(&Path)) -> tempfile::TempDir {
    let ws = copy_of(&graphos_pilot());
    edit(ws.path());
    let pilot_evidence = std::fs::read_to_string(evidence_path(ws.path())).unwrap();
    let scripts = tempfile::tempdir().unwrap();
    script(scripts.path(), "compose.sh", "echo 'compose: pass'; exit 0");
    let out = bin()
        .arg("evidence")
        .arg(ws.path())
        .arg("--scripts")
        .arg(scripts.path())
        .args(["--only", "compose"])
        .output()
        .unwrap();
    let run: Value =
        serde_json::from_str(&std::fs::read_to_string(evidence_path(ws.path())).unwrap())
            .unwrap_or_else(|e| panic!("{}: {:?}", e, out));
    let inputs = run["inputs"].clone();
    assert!(inputs["digest"].is_string(), "{:?}", out);
    std::fs::write(evidence_path(ws.path()), pilot_evidence).unwrap();
    edit_evidence(ws.path(), |e| e["inputs"] = inputs);
    assert!(!ws.path().join(".git").exists());
    ws
}

fn stamped_copy() -> tempfile::TempDir {
    stamped_copy_with(|_| {})
}

/// A copy of the pilot, `edit`ed, committed as a git repository of its
/// own, with its evidence's `commit` the new HEAD in `evidence`'s form and
/// no `inputs`: evidence written before they were recorded.
fn current_copy_with(edit: impl FnOnce(&Path)) -> tempfile::TempDir {
    let ws = copy_of(&graphos_pilot());
    edit(ws.path());
    edit_evidence(ws.path(), |e| {
        e.as_object_mut().unwrap().remove("inputs");
    });
    git(ws.path(), &["init", "-q"]);
    git(ws.path(), &["add", "-A"]);
    git(ws.path(), &["commit", "-q", "-m", "pilot"]);
    let head = git(ws.path(), &["rev-parse", "--short", "HEAD"]);
    edit_evidence(ws.path(), |e| e["commit"] = head.into());
    ws
}

pub(super) fn current_copy() -> tempfile::TempDir {
    current_copy_with(|_| {})
}

fn evidence_path(ws: &Path) -> std::path::PathBuf {
    ws.join(".factory/evidence/latest.json")
}

pub(super) fn edit_evidence(ws: &Path, edit: impl FnOnce(&mut Value)) {
    let file = evidence_path(ws);
    let mut e: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    edit(&mut e);
    std::fs::write(&file, serde_json::to_string_pretty(&e).unwrap()).unwrap();
}

/// The committed pilot passes the gate as it is: its `live` and
/// `supergraph_check` are `not_run`, and neither blocks.
#[test]
fn the_committed_pilot_passes_the_gate() {
    let text = std::fs::read_to_string(evidence_path(&graphos_pilot())).unwrap();
    let evidence: Value = serde_json::from_str(&text).unwrap();
    let verdict = export::gate(&evidence);
    assert!(verdict.pass, "{:?}", verdict.reasons);
    let layers: Vec<String> = export::not_verified(&evidence, &[])
        .into_iter()
        .map(|(l, s, _)| format!("{}: {}", l, s))
        .collect();
    assert_eq!(
        layers,
        vec![
            "live: not_run",
            "supergraph_check: not_run",
            "graphos_cloud_router: not_run"
        ]
    );
}

#[test]
fn no_evidence_is_refused() {
    let ws = copy_of(&graphos_pilot());
    std::fs::remove_file(evidence_path(ws.path())).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("deploy");
    let dest_s = dest.to_string_lossy().into_owned();
    let (code, stdout, stderr) =
        export_ok(ws.path(), &["--out", &dest_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(1), "{} {}", stdout, stderr);
    assert!(
        stderr.contains("no .factory/evidence/latest.json") && stderr.contains("not exported"),
        "{}",
        stderr
    );
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["code"], "no-evidence");
    assert_eq!(report["exit"], 1);
    assert!(!dest.exists(), "a refusal writes nothing");
}

#[test]
fn a_layer_or_operation_not_passing_is_refused_and_named() {
    let ws = stamped_copy();
    edit_evidence(ws.path(), |e| {
        e["layers"]["wiremock_e2e"]["status"] = "skipped".into();
        e["layers"]["wiremock_e2e"]["reason"] = "java is not installed".into();
        e["operations"]["get:/version"]["unit"] = "fail".into();
    });
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let (code, stdout, stderr) =
        export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(1), "{} {}", stdout, stderr);
    assert!(
        stderr.contains("the workspace is not validated, so it is not exported"),
        "{}",
        stderr
    );
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["code"], "not-validated");
    let reasons: Vec<&str> = report["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        reasons.contains(&"wiremock_e2e: skipped (java is not installed)"),
        "{:?}",
        reasons
    );
    assert!(
        reasons.contains(&"get:/version unit: fail"),
        "{:?}",
        reasons
    );
    assert!(!out.path().join("gitea.graphql").exists());

    // A live run that failed is refused too; one that did not run is not.
    let ws = stamped_copy();
    edit_evidence(ws.path(), |e| {
        e["layers"]["live"]["status"] = "fail".into();
        e["layers"]["live"]["reason"] = "401 from the real API".into();
    });
    let (code, _, stderr) = export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(stderr.contains("live: fail"), "{}", stderr);
}

#[test]
fn a_local_base_url_is_refused() {
    let ws = stamped_copy();
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    // No --base-url: template.yaml's test default, the local Gitea.
    let (code, _, stderr) = export_ok(ws.path(), &["--out", &out_s]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(
        stderr.contains(
            "http://127.0.0.1:3000/api/v1 (from template.yaml test_default) is this machine"
        ) && stderr.contains("Pass --base-url with the production host"),
        "{}",
        stderr
    );
    // Given, but local.
    let (code, stdout, stderr) = export_ok(
        ws.path(),
        &[
            "--out",
            &out_s,
            "--base-url",
            "http://localhost:4000/api/v1",
            "--json",
        ],
    );
    assert_eq!(code, Some(1), "{}", stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap()["code"],
        "base-url-local"
    );
    // From the environment, the core's override.
    let (code, _, stderr) = export_in(
        ws.path(),
        &["--out", &out_s],
        &[("GITEA_BASE_URL", "http://[::1]:3000/api/v1")],
    );
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(stderr.contains("(from GITEA_BASE_URL)"), "{}", stderr);
    assert!(!out.path().join("gitea.graphql").exists());
}

#[test]
fn a_literal_credential_is_refused_and_never_echoed() {
    let ws = stamped_copy();
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let (code, stdout, stderr) = export_in(
        ws.path(),
        &["--out", &out_s, "--base-url", HOST, "--json"],
        &[("GITEA_AUTH_EXPR", "s3cr3t-token-value")],
    );
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(
        stderr.contains("GraphOS stores the published schema"),
        "{}",
        stderr
    );
    assert!(!stderr.contains("s3cr3t") && !stdout.contains("s3cr3t"));
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap()["code"],
        "auth-expr-literal"
    );
}

#[test]
fn out_inside_the_workspace_is_refused() {
    let ws = copy_of(&graphos_pilot());
    for inside in ["deploy", ".", "tests/../deploy"] {
        let dest = ws.path().join(inside);
        let dest_s = dest.to_string_lossy().into_owned();
        let (code, stdout, stderr) =
            export_ok(ws.path(), &["--out", &dest_s, "--base-url", HOST, "--json"]);
        assert_eq!(code, Some(1), "{}: {}", inside, stderr);
        assert!(stderr.contains("is inside the workspace"), "{}", stderr);
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).unwrap()["code"],
            "out-inside-workspace"
        );
    }
    assert!(!ws.path().join("deploy").exists());
    // No --out is a usage error.
    let (code, _, stderr) = export_ok(ws.path(), &["--base-url", HOST]);
    assert_eq!(code, Some(2), "{}", stderr);
    assert!(
        stderr.contains("usage: graphos-factory export"),
        "{}",
        stderr
    );
}

#[test]
fn a_validated_workspace_is_rendered_and_handed_off() {
    let ws = stamped_copy();
    let schema_before = std::fs::read(ws.path().join("gitea.graphql")).unwrap();
    let out = tempfile::tempdir().unwrap();
    let dest = out.path().join("new/dir");
    let dest_s = dest.to_string_lossy().into_owned();
    let (code, stdout, stderr) = export_ok(ws.path(), &["--out", &dest_s, "--base-url", HOST]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    let rendered = std::fs::read_to_string(dest.join("gitea.graphql")).unwrap();
    assert!(!rendered.contains("{{"), "{}", rendered);
    assert!(rendered.contains(&format!("baseURL: \"{}\"", HOST)));
    assert!(rendered.contains("token {$env.GITEA_TOKEN}"));
    // The workspace is untouched: the committed schema keeps its placeholders.
    assert_eq!(
        std::fs::read(ws.path().join("gitea.graphql")).unwrap(),
        schema_before
    );
    for needle in [
        "rover subgraph check <GRAPH_REF> --name gitea --schema ",
        "rover subgraph publish <GRAPH_REF> --name gitea --schema ",
        "--routing-url http://localhost",
        "The router process needs: GITEA_TOKEN",
        "gitea.gitea:",
        "override_url: \"${env.GITEA_BASE_URL}\"",
        "Router: connect/v0.4 needs Apollo Router 2.15.0 or later, with no opt-in from 2.16.0",
        "Composition: validated at federation_version 2.15.2",
        "supergraph_check: not_run",
        "live: not_run",
        "graphos_cloud_router: not_run",
    ] {
        assert!(stdout.contains(needle), "{} missing: {}", needle, stdout);
    }
    // The gate line names the inputs digest the evidence ran against.
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(evidence_path(ws.path())).unwrap()).unwrap();
    let gate_line = stdout.lines().find(|l| l.starts_with("gate: ")).unwrap();
    assert!(
        gate_line.ends_with(&format!(
            "; evidence inputs {} ({} files) unchanged since it ran",
            &recorded["inputs"]["digest"].as_str().unwrap()[..12],
            recorded["inputs"]["files"].as_object().unwrap().len()
        )),
        "{}",
        gate_line
    );
    assert!(!stdout.contains("APOLLO_KEY"), "{}", stdout);
    assert!(!stdout.contains("2.14.1"), "{}", stdout);
    assert!(stdout.lines().count() <= 26, "{}", stdout);

    // Again over the same file: truncated in place, same bytes.
    let (code, _, stderr) = export_ok(ws.path(), &["--out", &dest_s, "--base-url", HOST]);
    assert_eq!(code, Some(0), "{}", stderr);
    assert_eq!(
        std::fs::read_to_string(dest.join("gitea.graphql")).unwrap(),
        rendered
    );
}

#[test]
fn json_carries_the_same_facts() {
    let ws = stamped_copy();
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let (code, stdout, stderr) =
        export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(0), "{}", stderr);
    let r: Value = serde_json::from_str(&stdout).unwrap();
    let file = std::fs::canonicalize(out.path())
        .unwrap()
        .join("gitea.graphql");
    assert_eq!(r["out"], file.to_string_lossy().as_ref());
    assert_eq!(r["service"], "gitea");
    assert_eq!(r["subgraph"], "gitea");
    assert_eq!(r["base_url"], HOST);
    assert_eq!(r["base_url_from"], "--base-url");
    assert_eq!(r["connect_spec"], "v0.4");
    assert_eq!(r["router_min"], "2.15.0");
    assert_eq!(r["router_no_opt_in_from"], "2.16.0");
    assert_eq!(r["federation_version"], "2.15.2");
    assert!(r.get("federation_min").is_none(), "{}", r);
    let recorded: Value =
        serde_json::from_str(&std::fs::read_to_string(evidence_path(ws.path())).unwrap()).unwrap();
    assert_eq!(r["evidence_commit"], recorded["commit"]);
    assert_eq!(r["evidence_checked"], "inputs");
    assert_eq!(
        r["evidence_digest"],
        &recorded["inputs"]["digest"].as_str().unwrap()[..12]
    );
    assert_eq!(
        r["evidence_files"],
        recorded["inputs"]["files"].as_object().unwrap().len()
    );
    assert_eq!(r["credential_env"], serde_json::json!(["GITEA_TOKEN"]));
    assert_eq!(r["sources"], serde_json::json!(["gitea.gitea"]));
    assert!(r["rover"]["check"]
        .as_str()
        .unwrap()
        .starts_with("rover subgraph check <GRAPH_REF> --name gitea --schema "));
    assert!(r["rover"]["publish"]
        .as_str()
        .unwrap()
        .ends_with(" --routing-url http://localhost"));
    assert_eq!(
        r["router_yaml"],
        "connectors:\n  sources:\n    gitea.gitea:\n      override_url: \"${env.GITEA_BASE_URL}\"\n"
    );
    assert_eq!(
        r["gate"]["passed"],
        serde_json::json!([
            "compose",
            "connector_unit",
            "wiremock_e2e",
            "conformance",
            "lint"
        ])
    );
    let not_verified: Vec<&str> = r["not_verified"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["layer"].as_str().unwrap())
        .collect();
    assert_eq!(
        not_verified,
        vec!["live", "supergraph_check", "graphos_cloud_router"]
    );
}

#[test]
fn help_lists_export_under_the_target() {
    let mut c = bin();
    let out = c.args(["export", "--help"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with(
            "usage: graphos-factory export [workspace] --out DIR [--base-url URL] [--json]\n"
        ),
        "{}",
        stdout
    );
    let out = bin().arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let at = |needle: &str| {
        stdout
            .find(needle)
            .unwrap_or_else(|| panic!("{} missing: {}", needle, stdout))
    };
    assert!(at("\n  [target graphos-factory]\n") < at("\n  export     [workspace] --out DIR"));
    assert!(at("\n  export     ") < at("\n  version\n"));
    // An undeclared flag is a usage error before anything runs (ADR 0097).
    let out = bin()
        .args(["export", ".", "--out", "/tmp/x", "--force"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// A base URL that is not an absolute `http`/`https` URL with a host is
/// refused wherever it came from, and so is a host the earlier check let
/// through: an IPv4-mapped loopback, `host.docker.internal`.
#[test]
fn a_base_url_that_is_not_an_absolute_url_or_is_local_is_refused() {
    let ws = stamped_copy();
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    for (url, code) in [
        ("localhost:3000/api", "base-url-invalid"),
        ("127.0.0.1:3000/api", "base-url-invalid"),
        ("not a url", "base-url-invalid"),
        ("http://[::ffff:127.0.0.1]:3000/api/v1", "base-url-local"),
        ("http://host.docker.internal:3000/api/v1", "base-url-local"),
    ] {
        let (status, stdout, stderr) =
            export_ok(ws.path(), &["--out", &out_s, "--base-url", url, "--json"]);
        assert_eq!(status, Some(1), "{}: {}", url, stderr);
        assert_eq!(
            serde_json::from_str::<Value>(&stdout).unwrap()["code"],
            code,
            "{}: {}",
            url,
            stderr
        );
    }
    let (_, _, stderr) = export_ok(
        ws.path(),
        &["--out", &out_s, "--base-url", "localhost:3000/api"],
    );
    assert!(
        stderr.contains("the base URL localhost:3000/api (from --base-url) is not an absolute http or https URL with a host"),
        "{}",
        stderr
    );
    // From the environment too.
    let (status, _, stderr) = export_in(
        ws.path(),
        &["--out", &out_s],
        &[("GITEA_BASE_URL", "127.0.0.1:3000/api")],
    );
    assert_eq!(status, Some(1), "{}", stderr);
    assert!(
        stderr.contains("(from GITEA_BASE_URL) is not an absolute"),
        "{}",
        stderr
    );
    assert!(!out.path().join("gitea.graphql").exists());
}

/// A base URL carrying userinfo is refused, and the userinfo appears in
/// neither stream nor the JSON.
#[test]
fn a_base_url_with_userinfo_is_refused_and_never_echoed() {
    let ws = stamped_copy();
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let (code, stdout, stderr) = export_ok(
        ws.path(),
        &[
            "--out",
            &out_s,
            "--base-url",
            "https://user:s3cret@git.example.com/api/v1",
            "--json",
        ],
    );
    assert_eq!(code, Some(1), "{}", stderr);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["code"], "base-url-userinfo");
    assert!(
        stderr.contains(
            "https://<redacted>@git.example.com/api/v1 (from --base-url) carries userinfo"
        ),
        "{}",
        stderr
    );
    for stream in [&stdout, &stderr] {
        assert!(
            !stream.contains("s3cret") && !stream.contains("user:"),
            "{}",
            stream
        );
    }
    assert!(!out.path().join("gitea.graphql").exists());
}

/// A literal credential in `template.yaml`'s AUTH_EXPR test default is a
/// lint error the gate names by rule and file; the value appears nowhere.
#[test]
fn a_literal_test_default_is_named_but_never_echoed() {
    const LITERAL: &str = "l1teral-t0ken-4f9a";
    let ws = stamped_copy_with(|ws| {
        let file = ws.join("template.yaml");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("test_default: \"{$env.GITEA_TOKEN}\""));
        std::fs::write(
            &file,
            text.replace("\"{$env.GITEA_TOKEN}\"", &format!("\"{}\"", LITERAL)),
        )
        .unwrap();
    });
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    for json in [true, false] {
        let mut args = vec!["--out", out_s.as_str(), "--base-url", HOST];
        if json {
            args.push("--json");
        }
        let (code, stdout, stderr) = export_ok(ws.path(), &args);
        assert_eq!(code, Some(1), "{} {}", stdout, stderr);
        assert!(
            stderr.contains("lint: [auth-test-default] template.yaml: AUTH_EXPR.test_default must be a complete {$env.NAME} expression, got <redacted>"),
            "{}",
            stderr
        );
        assert!(!stdout.contains(LITERAL), "{}", stdout);
        assert!(!stderr.contains(LITERAL), "{}", stderr);
        if json {
            let report: Value = serde_json::from_str(&stdout).unwrap();
            assert_eq!(report["code"], "not-validated");
            assert!(!report.to_string().contains(LITERAL));
        }
    }
    assert!(!out.path().join("gitea.graphql").exists());
}

/// A relationship field lint leaves not validated is listed as not
/// verified, by field and warning, and does not refuse the export.
#[test]
fn a_link_field_without_tests_is_named_not_verified() {
    let ws = stamped_copy_with(|ws| {
        for case in [
            "issue_repository_owner",
            "list_issues_repository_owner_null",
        ] {
            for ext in ["graphql", "expected.json"] {
                std::fs::remove_file(ws.join(format!("tests/cases/{}.{}", case, ext))).unwrap();
            }
        }
    });
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let (code, stdout, stderr) =
        export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    let r: Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        r["not_verified"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({
                "layer": "link field Gitea_RepositoryMeta.ownerUser",
                "status": "link-untested",
                "reason": "no e2e case",
            })),
        "{}",
        r
    );
    let (code, stdout, stderr) = export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST]);
    assert_eq!(code, Some(0), "{}", stderr);
    assert!(
        stdout.contains("  link field Gitea_RepositoryMeta.ownerUser: link-untested (no e2e case)"),
        "{}",
        stdout
    );
}

/// Older evidence, with no `inputs`: recorded on uncommitted changes, or
/// with uncommitted changes in the workspace now, it is refused, and the
/// refusal says the evidence predates input hashing.
#[test]
fn older_dirty_evidence_or_a_dirty_workspace_is_refused() {
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let ws = current_copy();
    let head = git(ws.path(), &["rev-parse", "--short", "HEAD"]);
    edit_evidence(ws.path(), |e| {
        e["commit"] = format!("{}-dirty", head).into()
    });
    let (code, stdout, stderr) =
        export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(
        stderr.contains(&format!(
            "the evidence predates input hashing, so export checks its commit against git: evidence was recorded at {}-dirty, the workspace is at {}: re-run evidence",
            head, head
        )),
        "{}",
        stderr
    );
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap()["code"],
        "evidence-stale"
    );

    let ws = current_copy();
    let head = git(ws.path(), &["rev-parse", "--short", "HEAD"]);
    std::fs::write(ws.path().join("README.md"), "edited after the evidence\n").unwrap();
    let (code, _, stderr) = export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(
        stderr.contains(&format!(
            "evidence was recorded at {}, the workspace is at {}-dirty: re-run evidence",
            head, head
        )),
        "{}",
        stderr
    );
    assert!(!out.path().join("gitea.graphql").exists());
}

/// Older evidence, with no `inputs`, is checked against git and says so:
/// recorded at another commit it is refused when the workspace changed
/// since; a commit that adds only the evidence keeps it current. Such
/// evidence in a workspace outside git is refused.
#[test]
fn older_evidence_recorded_at_another_commit_is_refused() {
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let ws = current_copy();
    let recorded = git(ws.path(), &["rev-parse", "--short", "HEAD"]);
    // Committing the evidence alone: still the evidence for these files.
    git(ws.path(), &["add", "-A"]);
    git(ws.path(), &["commit", "-q", "-m", "evidence"]);
    let (code, stdout, stderr) = export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    assert!(
        stdout.contains(&format!(
            "evidence at {} (it predates input hashing; checked against git)",
            recorded
        )),
        "{}",
        stdout
    );
    let (code, stdout, _) = export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(0));
    let r: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(r["evidence_checked"], "commit");
    assert_eq!(r["evidence_digest"], Value::Null);
    std::fs::remove_file(out.path().join("gitea.graphql")).unwrap();

    std::fs::write(ws.path().join("README.md"), "edited after the evidence\n").unwrap();
    git(ws.path(), &["commit", "-q", "-am", "edit"]);
    let head = git(ws.path(), &["rev-parse", "--short", "HEAD"]);
    let (code, stdout, stderr) =
        export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(
        stderr.contains(&format!(
            "evidence was recorded at {}, the workspace is at {}: re-run evidence",
            recorded, head
        )),
        "{}",
        stderr
    );
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap()["code"],
        "evidence-stale"
    );
    assert!(!out.path().join("gitea.graphql").exists());

    // Outside git, nothing says what such evidence ran against.
    let plain = copy_of(&graphos_pilot());
    edit_evidence(plain.path(), |e| {
        e.as_object_mut().unwrap().remove("inputs");
    });
    let (code, _, stderr) = export_ok(plain.path(), &["--out", &out_s, "--base-url", HOST]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(
        stderr.contains("the evidence predates input hashing, so export checks its commit against git: the workspace is not in a git repository"),
        "{}",
        stderr
    );
}

/// With `inputs`, git plays no part: a copy that was never a git
/// repository exports, and so does one whose README changed since (the
/// layers never read it).
#[test]
fn evidence_with_inputs_exports_without_git() {
    let ws = stamped_copy();
    assert!(!ws.path().join(".git").exists());
    std::fs::write(ws.path().join("README.md"), "edited after the evidence\n").unwrap();
    std::fs::write(ws.path().join(".factory/memory.md"), "edited too\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let (code, stdout, stderr) = export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    assert!(out.path().join("gitea.graphql").exists());
    assert!(stdout.contains("unchanged since it ran"), "{}", stdout);
}

/// A file the layers read that changed, appeared or went away since the
/// evidence ran is refused, each one named, the schema, a test case and
/// `template.yaml` among them.
#[test]
fn a_file_the_layers_read_changed_since_is_refused_by_name() {
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let cases: [(&str, fn(&Path), &str); 4] = [
        (
            "the schema",
            |ws| {
                let f = ws.join("gitea.graphql");
                let text = std::fs::read_to_string(&f).unwrap();
                std::fs::write(&f, format!("{}\n# edited\n", text)).unwrap();
            },
            "gitea.graphql changed since evidence ran",
        ),
        (
            "a new test case",
            |ws| {
                std::fs::write(
                    ws.join("tests/cases/added_later.graphql"),
                    "query { gitea_version { version } }\n",
                )
                .unwrap();
            },
            "tests/cases/added_later.graphql added since evidence ran",
        ),
        (
            "template.yaml",
            |ws| {
                let f = ws.join("template.yaml");
                let text = std::fs::read_to_string(&f).unwrap();
                std::fs::write(&f, format!("{}# edited\n", text)).unwrap();
            },
            "template.yaml changed since evidence ran",
        ),
        (
            "a removed fixture",
            |ws| std::fs::remove_file(ws.join("tests/fixtures/mappings/issue.json")).unwrap(),
            "tests/fixtures/mappings/issue.json removed since evidence ran",
        ),
    ];
    for (what, edit, reason) in cases {
        let ws = stamped_copy();
        edit(ws.path());
        let (code, stdout, stderr) =
            export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST, "--json"]);
        assert_eq!(code, Some(1), "{}: {}", what, stderr);
        assert_eq!(
            stderr,
            format!("export: {}: re-run evidence\n", reason),
            "{}",
            what
        );
        let report: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(report["code"], "evidence-stale", "{}", what);
        assert_eq!(report["reasons"], serde_json::json!([reason]), "{}", what);
        assert!(!out.path().join("gitea.graphql").exists(), "{}", what);
    }
}

/// An `inputs` whose digest its own files do not reproduce was edited by
/// hand, and is refused rather than trusted.
#[test]
fn an_inputs_record_edited_by_hand_is_refused() {
    let ws = stamped_copy();
    edit_evidence(ws.path(), |e| {
        e["inputs"]["files"]["gitea.graphql"] = "0".repeat(64).into();
    });
    let out = tempfile::tempdir().unwrap();
    let out_s = out.path().to_string_lossy().into_owned();
    let (code, _, stderr) = export_ok(ws.path(), &["--out", &out_s, "--base-url", HOST]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert!(
        stderr.contains("(inputs.digest is not the digest of inputs.files): re-run evidence"),
        "{}",
        stderr
    );
}

/// Evidence that could not hash its inputs says why (`inputs_error`), and
/// export refuses with that reason and asks for the fix first: never "the
/// evidence predates input hashing", whose re-run would fail the same way.
#[test]
fn evidence_that_could_not_hash_its_inputs_is_refused_with_the_reason() {
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("elsewhere.graphql");
    std::fs::write(&target, "query { x }\n").unwrap();
    let ws = copy_of(&graphos_pilot());
    std::os::unix::fs::symlink(&target, ws.path().join("tests/cases/linked.graphql")).unwrap();
    let pilot_evidence = std::fs::read_to_string(evidence_path(ws.path())).unwrap();
    let scripts = tempfile::tempdir().unwrap();
    script(scripts.path(), "compose.sh", "echo 'compose: pass'; exit 0");
    let out = bin()
        .arg("evidence")
        .arg(ws.path())
        .arg("--scripts")
        .arg(scripts.path())
        .args(["--only", "compose"])
        .output()
        .unwrap();
    let run: Value =
        serde_json::from_str(&std::fs::read_to_string(evidence_path(ws.path())).unwrap())
            .unwrap_or_else(|e| panic!("{}: {:?}", e, out));
    assert!(run.get("inputs").is_none(), "{}", run);
    let why = run["inputs_error"].clone();
    assert_eq!(
        why,
        "tests/cases/linked.graphql: file resolves outside the workspace"
    );
    // The pilot's passing layers, with this run's reason in place of inputs.
    std::fs::write(evidence_path(ws.path()), pilot_evidence).unwrap();
    edit_evidence(ws.path(), |e| {
        e.as_object_mut().unwrap().remove("inputs");
        e["inputs_error"] = why;
    });
    let dest = tempfile::tempdir().unwrap();
    let dest_s = dest.path().to_string_lossy().into_owned();
    let (code, stdout, stderr) =
        export_ok(ws.path(), &["--out", &dest_s, "--base-url", HOST, "--json"]);
    assert_eq!(code, Some(1), "{}", stderr);
    assert_eq!(
        stderr,
        "export: evidence could not hash its inputs (tests/cases/linked.graphql: file resolves outside the workspace): fix that, then re-run evidence\n"
    );
    assert!(!stderr.contains("predates"), "{}", stderr);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["code"], "evidence-stale");
    assert!(!dest.path().join("gitea.graphql").exists());
}

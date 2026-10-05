use graphos_factory_core::context::{check, FILE};
use graphos_factory_core::spans::sha256_hex;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

/// The companion file body. The generic/specialized decision itself lives in
/// workspace.yaml's context_mode, written by `mark`.
fn context_body() -> Value {
    json!({"contract_version": 1, "intent": "Expose the customer's chosen analytics fields.", "inputs": [], "requirements": []})
}

fn requirement(id: &str, phase: &str) -> Value {
    json!({"id": id, "phase": phase, "affects": ["customer metrics"], "reason": "Field definitions live outside the API description.", "resolve_with": "Read the authorized metadata export or ask for the missing scope.", "status": "missing"})
}

/// Record the assessment result in workspace.yaml, as the skill would.
fn mark(dir: &Path, mode: &str) {
    std::fs::create_dir_all(dir.join(".factory")).unwrap();
    std::fs::write(
        dir.join(".factory/workspace.yaml"),
        format!("context_mode: {}\n", mode),
    )
    .unwrap();
}

/// A readable workspace.yaml that records no `context_mode`.
fn unmarked(dir: &Path) {
    std::fs::create_dir_all(dir.join(".factory")).unwrap();
    std::fs::write(dir.join(".factory/workspace.yaml"), "service: widget_co\n").unwrap();
}

fn save(dir: &Path, value: &Value) {
    std::fs::create_dir_all(dir.join(".factory")).unwrap();
    std::fs::write(dir.join(FILE), serde_json::to_string(value).unwrap()).unwrap();
}

#[test]
fn an_unrecorded_mode_reads_as_generic_and_does_not_block() {
    // ADR 0081: the marker is self-attested, so its absence is not a gap.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("openapi.json"), "{}").unwrap();
    unmarked(dir.path());
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 0);
    assert_eq!(report.exit_code("live"), 0);
    assert!(report.errors.is_empty());
    assert!(report.blockers.is_empty(), "{:?}", report.blockers);
    assert_eq!(report.mode, None, "the report does not invent a marker");

    // Recording generic gives the same answer, and still needs no companion file.
    mark(dir.path(), "generic");
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 0);
    assert!(report.live_ready);
    assert!(!dir.path().join(FILE).exists());
    // Readiness does not require a schema, fixtures, or live credentials.
    assert!(!dir.path().join("service.graphql").exists());
}

#[test]
fn a_context_file_without_a_marker_is_checked_as_generic() {
    let dir = TempDir::new().unwrap();
    unmarked(dir.path());
    let mut body = context_body();
    save(dir.path(), &body);
    let report = check(dir.path(), None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.exit_code("build"), 0);

    // Its declared requirements still block, phase by phase.
    body["requirements"] = json!([requirement("model-fields", "build")]);
    save(dir.path(), &body);
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 2);
    assert_eq!(report.blockers[0].id, "model-fields");

    body["requirements"] = json!([requirement("identity", "live")]);
    save(dir.path(), &body);
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 0);
    assert_eq!(report.exit_code("live"), 2);
    assert_eq!(report.blockers[0].id, "identity");
}

#[test]
fn only_an_absent_key_reads_as_generic() {
    // A workspace.yaml the check cannot read is not "no marker": a mistyped
    // path or a refused file must not report ready (ADR 0081, ADR 0025).
    let missing = TempDir::new().unwrap();
    let report = check(missing.path(), None);
    assert_eq!(report.exit_code("build"), 1, "{:?}", report);
    assert!(
        report.errors[0].contains("workspace.yaml"),
        "{:?}",
        report.errors
    );

    for body in [
        "context_mode: [generic\n",
        "context_mode: 1\n",
        "context_mode:\n",
    ] {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".factory")).unwrap();
        std::fs::write(dir.path().join(".factory/workspace.yaml"), body).unwrap();
        let report = check(dir.path(), None);
        assert_eq!(report.exit_code("build"), 1, "{}: {:?}", body, report);
        assert!(!report.build_ready && !report.live_ready, "{}", body);
    }

    #[cfg(unix)]
    {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".factory")).unwrap();
        std::fs::write(
            dir.path().join("elsewhere.yaml"),
            "context_mode: specialized\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("elsewhere.yaml"),
            dir.path().join(".factory/workspace.yaml"),
        )
        .unwrap();
        let report = check(dir.path(), None);
        assert_eq!(report.exit_code("build"), 1, "{:?}", report);
        assert!(!report.build_ready);
    }
}

#[test]
fn unresolved_intent_cannot_silently_default_to_generic() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "undecided");
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 2);
    assert_eq!(report.blockers[0].id, "context-scope");
}

#[test]
fn specialized_mode_requires_a_companion_file() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "specialized");
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 1);
    assert!(report.errors[0].contains("context.yaml"));
}

#[test]
fn specialized_scope_resumes_from_recorded_answers_and_offline_metadata() {
    let dir = TempDir::new().unwrap();
    let metadata = "{\"fields\":[\"monthly_revenue\"]}";
    std::fs::write(dir.path().join("metadata.json"), metadata).unwrap();
    mark(dir.path(), "specialized");
    let mut context = context_body();
    context["requirements"] = json!([
        requirement("model-scope", "build"),
        requirement("live-identity", "live")
    ]);
    save(dir.path(), &context);
    let report = check(dir.path(), None);
    assert!(!report.build_ready);
    assert_eq!(report.blockers.len(), 2);

    context["requirements"][0]["status"] = json!("resolved");
    context["requirements"][0]["resolution"] =
        json!("The user selected the finance model; use the supplied metadata export.");
    context["requirements"][0]["evidence"] =
        json!([{"path":"metadata.json", "sha256":sha256_hex(metadata)}]);
    save(dir.path(), &context);
    let before = std::fs::read(dir.path().join(FILE)).unwrap();
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 0);
    assert_eq!(report.exit_code("live"), 2);
    assert_eq!(report.blockers.len(), 1);
    assert_eq!(report.blockers[0].id, "live-identity");
    assert_eq!(std::fs::read(dir.path().join(FILE)).unwrap(), before);
}

#[test]
fn changed_metadata_blocks_its_dependent_phase_without_rewriting_evidence() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "generic");
    let mut context = context_body();
    let mut req = requirement("live-identity", "live");
    req["status"] = json!("resolved");
    req["resolution"] = json!("Read identity details in the scrubbed response.");
    req["evidence"] = json!([{"path":"identity.json", "sha256":sha256_hex("old")}]);
    context["requirements"] = json!([req]);
    std::fs::write(dir.path().join("identity.json"), "changed").unwrap();
    save(dir.path(), &context);
    let report = check(dir.path(), None);
    assert!(report.build_ready);
    assert!(!report.live_ready);
    assert!(report.blockers[0].reason.contains("content changed"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("identity.json")).unwrap(),
        "changed"
    );
    std::fs::remove_file(dir.path().join("identity.json")).unwrap();
    assert!(!check(dir.path(), None).live_ready);

    context["requirements"][0]["phase"] = json!("build");
    save(dir.path(), &context);
    assert!(!check(dir.path(), None).build_ready);
}

#[test]
fn changed_spec_requires_reassessment_even_for_a_previously_ready_wrapper() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "generic");
    let mut context = context_body();
    context["inputs"] = json!([{"path":"openapi.json", "sha256":sha256_hex("{}")}]);
    std::fs::write(dir.path().join("openapi.json"), "{}").unwrap();
    save(dir.path(), &context);
    assert!(check(dir.path(), None).build_ready);
    std::fs::write(dir.path().join("openapi.json"), "{\"changed\":true}").unwrap();
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 2);
    assert_eq!(report.blockers[0].id, "context-input");
}

#[test]
fn explicitly_stale_context_cannot_pass_with_an_old_resolution() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "specialized");
    let mut context = context_body();
    let mut req = requirement("model-scope", "build");
    req["status"] = json!("stale");
    req["resolution"] = json!("The previous customer scope.");
    context["requirements"] = json!([req]);
    save(dir.path(), &context);
    assert_eq!(check(dir.path(), None).exit_code("build"), 2);
}

#[test]
fn invalid_records_fail_instead_of_dropping_requirements() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "generic");
    let mut versions = context_body();
    versions["contract_version"] = json!(2);
    let mut duplicate = context_body();
    duplicate["requirements"] =
        json!([requirement("scope", "build"), requirement("scope", "live")]);
    let mut false_resolution = context_body();
    let mut req = requirement("scope", "build");
    req["status"] = json!("resolved");
    false_resolution["requirements"] = json!([req]);
    let mut typo = context_body();
    typo["requirement"] = json!([]);
    let mut blank = context_body();
    blank["intent"] = json!("  ");
    for context in [versions, duplicate, false_resolution, typo, blank] {
        save(dir.path(), &context);
        let report = check(dir.path(), None);
        assert_eq!(report.exit_code("build"), 1, "{context}: {report:?}");
        assert!(!report.build_ready);
        assert!(!report.live_ready);
    }
    std::fs::write(dir.path().join(FILE), "intent: [broken").unwrap();
    assert_eq!(check(dir.path(), None).exit_code("build"), 1);

    // specialized mode with no build requirement cannot silently pass.
    mark(dir.path(), "specialized");
    save(dir.path(), &context_body());
    assert_eq!(check(dir.path(), None).exit_code("build"), 1);
}

#[test]
fn evidence_must_resolve_inside_the_workspace() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "generic");
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("external.json"), "{}").unwrap();
    let mut context = context_body();
    for path in [
        "../external.json".to_string(),
        outside.path().join("external.json").display().to_string(),
    ] {
        context["inputs"] = json!([{"path":path, "sha256":sha256_hex("{}")}]);
        save(dir.path(), &context);
        assert!(!check(dir.path(), None).build_ready);
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            outside.path().join("external.json"),
            dir.path().join("link.json"),
        )
        .unwrap();
        context["inputs"] = json!([{"path":"link.json", "sha256":sha256_hex("{}")}]);
        save(dir.path(), &context);
        assert!(check(dir.path(), None).blockers[0]
            .reason
            .contains("outside the workspace"));
    }
}

#[test]
fn cli_reports_both_phases_and_uses_distinct_exit_codes() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "generic");
    let mut context = context_body();
    context["requirements"] = json!([requirement("identity", "live")]);
    save(dir.path(), &context);
    for (phase, code) in [("build", 0), ("live", 2)] {
        let output = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
            .args([
                "context",
                "check",
                dir.path().to_str().unwrap(),
                "--phase",
                phase,
                "--json",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["build_ready"], true);
        assert_eq!(report["live_ready"], false);
        assert_eq!(report["blockers"][0]["id"], "identity");
    }
    for args in [
        vec!["context"],
        vec!["context", "check", "--phase", "typo"],
        vec!["context", "check", "--phase"],
    ] {
        assert_eq!(
            Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
                .args(args)
                .output()
                .unwrap()
                .status
                .code(),
            Some(1)
        );
    }
}

fn capture_cli(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"));
    command.args(["context", "capture", dir.to_str().unwrap()]);
    command.args(args);
    command.output().unwrap()
}

#[test]
fn capture_hashes_exact_raw_bytes_before_parsing_and_detects_a_stripped_final_lf() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "specialized");
    let mut context = context_body();
    context["requirements"] = json!([{
        "id": "field-metadata",
        "phase": "build",
        "affects": ["typed customer fields"],
        "reason": "The selected field definitions are external to the API description.",
        "resolve_with": "Use the scoped administrative export.",
        "status": "missing"
    }]);
    save(dir.path(), &context);

    // Valid JSON with trailing whitespace, exactly 24,320 bytes and ending LF.
    // A parser sees the same value after the LF is stripped; byte evidence must not.
    let mut canonical = br#"{"fields":[]}"#.to_vec();
    canonical.resize(24_319, b' ');
    canonical.push(b'\n');
    assert_eq!(canonical.len(), 24_320);
    let supplied = dir.path().join("supplied-fields.json");
    std::fs::write(&supplied, &canonical).unwrap();

    let output = capture_cli(
        dir.path(),
        &[
            "--requirement",
            "field-metadata",
            "--id",
            "field-metadata-raw",
            "--from",
            supplied.to_str().unwrap(),
            "--representation",
            "raw",
            "--authority",
            "authoritative",
            "--format",
            "json",
            "--capture-method",
            "administrative export",
            "--source",
            "approved customer metadata export",
            "--scope",
            "selected field definitions",
            "--refresh-by",
            "2999-01-01",
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["byte_count"], 24_320);
    let artifact = dir.path().join(report["path"].as_str().unwrap());
    assert_eq!(std::fs::read(&artifact).unwrap(), canonical);

    let mut saved = graphos_factory_core::yaml::parse_file(&dir.path().join(FILE)).unwrap();
    let evidence = &saved["requirements"][0]["evidence"][0];
    assert_eq!(evidence["byte_count"], 24_320);
    assert_eq!(evidence["representation"], "raw");
    assert_eq!(evidence["authority"], "authoritative");
    assert_eq!(evidence["capture_method"], "administrative export");
    assert_eq!(evidence["source"], "approved customer metadata export");
    assert_eq!(evidence["scope"], json!(["selected field definitions"]));
    assert_eq!(evidence["refresh_by"], "2999-01-01");
    assert_eq!(
        evidence["sha256"],
        graphos_factory_core::patch::bytes_sha256(&canonical)
    );
    saved["requirements"][0]["status"] = json!("resolved");
    saved["requirements"][0]["resolution"] = json!("Captured from the approved scoped export.");
    save(dir.path(), &saved);
    assert!(check(dir.path(), None).build_ready);

    let parsed_before: Value = serde_json::from_slice(&canonical).unwrap();
    let mut stripped = canonical;
    assert_eq!(stripped.pop(), Some(b'\n'));
    let parsed_after: Value = serde_json::from_slice(&stripped).unwrap();
    assert_eq!(
        parsed_after, parsed_before,
        "the parser is the negative control: only byte evidence can detect this regression"
    );
    std::fs::write(&artifact, stripped).unwrap();
    let report = check(dir.path(), None);
    assert_eq!(report.exit_code("build"), 2);
    assert!(report.blockers[0]
        .reason
        .contains("byte count changed from 24320 to 24319"));
    assert!(report.blockers[0].reason.contains("SHA-256 mismatch"));
}

#[test]
fn independent_captures_have_independent_cadences_and_derived_lineage() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "specialized");
    let mut context = context_body();
    context["requirements"] = json!([
        {
            "id": "field-metadata", "phase": "build", "affects": ["typed fields"],
            "reason": "Definitions are external.", "resolve_with": "Refresh the field export.",
            "status": "resolved", "resolution": "Approved raw export."
        },
        {
            "id": "live-labels", "phase": "live", "affects": ["live assertions"],
            "reason": "Labels change independently.", "resolve_with": "Refresh the label export.",
            "status": "resolved", "resolution": "Derived labels."
        }
    ]);
    save(dir.path(), &context);
    let raw = dir.path().join("raw.json");
    let derived = dir.path().join("derived.json");
    std::fs::write(&raw, "{\"fields\":[\"a\"]}\n").unwrap();
    std::fs::write(&derived, "{\"labels\":[\"A\"]}\n").unwrap();
    let first = capture_cli(
        dir.path(),
        &[
            "--requirement",
            "field-metadata",
            "--id",
            "fields-raw",
            "--from",
            raw.to_str().unwrap(),
            "--representation",
            "raw",
            "--authority",
            "authoritative",
            "--format",
            "json",
            "--capture-method",
            "export",
            "--source",
            "approved field export",
            "--scope",
            "typed fields",
            "--refresh-by",
            "2999-01-01",
        ],
    );
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = capture_cli(
        dir.path(),
        &[
            "--requirement",
            "live-labels",
            "--id",
            "labels-derived",
            "--from",
            derived.to_str().unwrap(),
            "--representation",
            "derived",
            "--authority",
            "supporting",
            "--format",
            "json",
            "--capture-method",
            "local transform",
            "--source",
            "fields-raw mapping",
            "--scope",
            "live labels",
            "--derived-from",
            "fields-raw",
            "--refresh-by",
            "2000-01-01",
        ],
    );
    assert_eq!(
        second.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let report = check(dir.path(), None);
    assert!(report.build_ready, "{report:?}");
    assert!(!report.live_ready);
    assert_eq!(report.blockers[0].id, "live-labels");
    assert!(report.blockers[0]
        .reason
        .contains("refresh was due 2000-01-01"));

    let rejected = capture_cli(
        dir.path(),
        &[
            "--requirement",
            "live-labels",
            "--id",
            "bad-derived",
            "--from",
            derived.to_str().unwrap(),
            "--representation",
            "derived",
            "--authority",
            "authoritative",
            "--capture-method",
            "transform",
            "--source",
            "fields-raw",
            "--scope",
            "labels",
            "--derived-from",
            "fields-raw",
        ],
    );
    assert_eq!(rejected.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("only a raw artifact can be authoritative"));
}

#[test]
fn capture_refuses_credentials_as_context_evidence() {
    let dir = TempDir::new().unwrap();
    mark(dir.path(), "specialized");
    let mut context = context_body();
    context["requirements"] = json!([requirement("credential-class", "build")]);
    save(dir.path(), &context);
    let token = dir.path().join("token.txt");
    std::fs::write(
        &token,
        "Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456\n",
    )
    .unwrap();
    let output = capture_cli(
        dir.path(),
        &[
            "--requirement",
            "credential-class",
            "--id",
            "credential-proof",
            "--from",
            token.to_str().unwrap(),
            "--representation",
            "raw",
            "--authority",
            "supporting",
            "--capture-method",
            "administrative attestation",
            "--source",
            "service administrator",
            "--scope",
            "credential capability",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing an artifact"));
    assert!(!dir
        .path()
        .join(".factory/context-artifacts/credential-proof")
        .exists());
}

#[cfg(unix)]
#[test]
fn capture_refuses_an_artifact_directory_symlinked_outside_the_workspace() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    mark(dir.path(), "specialized");
    let mut context = context_body();
    context["requirements"] = json!([requirement("field-metadata", "build")]);
    save(dir.path(), &context);
    std::os::unix::fs::symlink(
        outside.path(),
        dir.path().join(".factory/context-artifacts"),
    )
    .unwrap();
    let supplied = dir.path().join("metadata.json");
    std::fs::write(&supplied, "{}\n").unwrap();

    let output = capture_cli(
        dir.path(),
        &[
            "--requirement",
            "field-metadata",
            "--id",
            "escaped-artifact",
            "--from",
            supplied.to_str().unwrap(),
            "--representation",
            "raw",
            "--authority",
            "authoritative",
            "--format",
            "json",
            "--capture-method",
            "export",
            "--source",
            "approved local export",
            "--scope",
            "selected fields",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("outside the workspace"));
    assert!(!outside.path().join("escaped-artifact").exists());
}

use graphos_factory_core::cmd::selection_review::review;
use std::fs;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".factory")).unwrap();
    let pilot = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pilots/graphos/gitea/.factory");
    for file in ["workspace.yaml", "inventory.json", "selection.yaml"] {
        fs::copy(pilot.join(file), dir.path().join(".factory").join(file)).unwrap();
    }
    dir
}
/// The published contract, read from the schemas directory beside the crate.
fn review_schema() -> String {
    fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schemas/experimental/selection-review.schema.json"),
    )
    .unwrap()
}
fn token(dir: &std::path::Path) -> String {
    review(dir, None, None, None).unwrap()["input_token"]
        .as_str()
        .unwrap()
        .into()
}
const CANDIDATE: &str = "# keep my comment\ncontract_version: 1\noperations:\n  'post:/markdown':\n    include: true\n    graphql: {root: query, name: markdown}\n    response: {envelope: null, confirmed: false}\n";

#[test]
fn review_exposes_exact_candidate_changes_and_unconfirmed_decisions_without_writing() {
    let dir = fixture();
    let before = fs::read(dir.path().join(".factory/selection.yaml")).unwrap();
    let input = token(dir.path());
    let result = review(dir.path(), Some(CANDIDATE), Some(&input), None).unwrap();
    assert_eq!(result["candidate_yaml"], CANDIDATE);
    assert_eq!(
        result["query_write_decisions"],
        serde_json::json!(["post:/markdown"])
    );
    assert_eq!(
        result["unconfirmed_responses"],
        serde_json::json!(["post:/markdown"])
    );
    assert!(result["operation_changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["key"] == "get:/version" && change["after"].is_null()));
    assert_eq!(
        before,
        fs::read(dir.path().join(".factory/selection.yaml")).unwrap()
    );
    assert_eq!(input, token(dir.path()));
    review(
        dir.path(),
        Some(CANDIDATE),
        Some(&input),
        result["review_token"].as_str(),
    )
    .unwrap();
}

#[test]
fn every_bound_file_change_including_missing_to_present_invalidates_input() {
    for name in [
        "workspace.yaml",
        "inventory.json",
        "selection.yaml",
        "sources.lock.yaml",
        "applied.lock.yaml",
        "decisions.json",
        "findings.json",
        "memory.md",
    ] {
        let dir = fixture();
        let input = token(dir.path());
        fs::write(dir.path().join(".factory").join(name), "changed").unwrap();
        assert!(
            review(dir.path(), Some(CANDIDATE), Some(&input), None)
                .unwrap_err()
                .contains("stale selection inputs"),
            "{name}"
        );
    }
}

#[test]
fn identical_files_in_another_workspace_and_candidate_comment_edits_need_new_review() {
    let first = fixture();
    let second = fixture();
    let input = token(first.path());
    assert!(review(second.path(), Some(CANDIDATE), Some(&input), None)
        .unwrap_err()
        .contains("stale"));
    let result = review(first.path(), Some(CANDIDATE), Some(&input), None).unwrap();
    assert!(review(
        first.path(),
        Some(&format!("{CANDIDATE}# changed\n")),
        Some(&input),
        result["review_token"].as_str()
    )
    .unwrap_err()
    .contains("stale review"));
}

#[test]
fn invalid_unknown_and_duplicate_selection_keys_are_rejected() {
    let dir = fixture();
    let input = token(dir.path());
    for candidate in [
        "contract_version: 999\noperations: {}",
        "contract_version: 1\noperations: {}\noperations: {}",
        "contract_version: 1\noperations:\n  'get:/invented': {include: true}",
    ] {
        assert!(
            review(dir.path(), Some(candidate), Some(&input), None).is_err(),
            "{candidate}"
        );
    }
    assert!(review(dir.path(), Some(CANDIDATE), None, None)
        .unwrap_err()
        .contains("requires"));
}

#[test]
fn first_selection_is_reviewable_and_its_creation_makes_the_snapshot_stale() {
    let dir = fixture();
    let path = dir.path().join(".factory/selection.yaml");
    fs::remove_file(&path).unwrap();
    let input = token(dir.path());
    let result = review(dir.path(), Some(CANDIDATE), Some(&input), None).unwrap();
    assert!(result["before"].is_null());
    assert!(!path.exists());
    fs::write(&path, CANDIDATE).unwrap();
    assert!(review(dir.path(), Some(CANDIDATE), Some(&input), None).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_inputs_and_factory_directory_are_rejected() {
    let dir = fixture();
    let path = dir.path().join(".factory/selection.yaml");
    fs::rename(&path, dir.path().join("other.yaml")).unwrap();
    std::os::unix::fs::symlink("../other.yaml", &path).unwrap();
    assert!(review(dir.path(), None, None, None)
        .unwrap_err()
        .contains("regular file"));
    fs::remove_file(path).unwrap();
    fs::rename(dir.path().join(".factory"), dir.path().join("metadata")).unwrap();
    std::os::unix::fs::symlink("metadata", dir.path().join(".factory")).unwrap();
    assert!(review(dir.path(), None, None, None)
        .unwrap_err()
        .contains("not a symlink"));
}

#[test]
fn cli_rejects_misspelled_missing_and_duplicate_flags() {
    for args in [
        vec!["--expect-input"],
        vec!["--expect-inpt", "bad"],
        vec!["--expect-input", "a", "--expect-input", "b"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
            .args(["selection", "review"])
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn cli_snapshot_and_candidate_reports_match_the_published_contract() {
    let dir = fixture();
    let candidate = dir.path().join("candidate.yaml");
    fs::write(&candidate, CANDIDATE).unwrap();
    let schema: serde_json::Value = serde_json::from_str(&review_schema()).unwrap();
    let first = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["selection", "review"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let snapshot: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert!(graphos_factory_core::jsonschema::validate(&snapshot, &schema).is_empty());
    let second = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["selection", "review"])
        .arg(dir.path())
        .arg("--candidate")
        .arg(&candidate)
        .args(["--expect-input", snapshot["input_token"].as_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert!(graphos_factory_core::jsonschema::validate(&report, &schema).is_empty());
    assert_eq!(report["candidate_yaml"], CANDIDATE);
}

#[test]
fn omitted_response_confirmation_preserves_the_selection_contract() {
    let dir = fixture();
    let input = token(dir.path());
    for response in [
        "",
        "    response: {envelope: null}\n",
        "    response: {envelope: null, confirmed: true}\n",
    ] {
        let candidate = format!(
            "contract_version: 1\noperations:\n  'post:/markdown':\n    include: true\n{response}"
        );
        let report = review(dir.path(), Some(&candidate), Some(&input), None).unwrap();
        assert_eq!(report["unconfirmed_responses"], serde_json::json!([]));
    }
}

#[test]
fn cli_rejects_stale_inputs_and_candidate_bytes() {
    let dir = fixture();
    let input = token(dir.path());
    let report = review(dir.path(), Some(CANDIDATE), Some(&input), None).unwrap();
    let candidate = dir.path().join("candidate.yaml");
    for changed_input in [true, false] {
        fs::write(
            &candidate,
            if changed_input {
                CANDIDATE.to_string()
            } else {
                format!("{CANDIDATE}# changed\n")
            },
        )
        .unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
            .args(["selection", "review"])
            .arg(dir.path())
            .arg("--candidate")
            .arg(&candidate)
            .args([
                "--expect-input",
                if changed_input { "stale-input" } else { &input },
            ])
            .args(["--expect-review", report["review_token"].as_str().unwrap()])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(if changed_input {
                "stale selection inputs"
            } else {
                "stale review"
            })
        );
    }
}

// ADR 0069 — a links: section is a reviewable section like overrides and
// waivers, and its drafts are listed the way unconfirmed envelopes are.
const LINK_CANDIDATE: &str = "contract_version: 1\noperations:\n  'get:/users/{username}':\n    include: true\n    graphql: {root: query, name: user}\nlinks:\n  - shape: TrackedTime\n    path: user_name\n    operation: 'get:/users/{username}'\n    parameter: username\n    include: true\n    confirmed: false\n  - shape: TrackedTime\n    path: user_id\n    operation: 'get:/users/{username}'\n    include: false\n    confirmed: false\n    reason: user_id is not a username\n";

#[test]
fn a_links_section_is_a_changed_section_and_its_drafts_are_listed() {
    let dir = fixture();
    let input = token(dir.path());
    let result = review(dir.path(), Some(LINK_CANDIDATE), Some(&input), None).unwrap();
    let sections: Vec<&str> = result["changed_sections"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.as_str())
        .collect();
    assert!(sections.contains(&"links"), "{:?}", sections);
    // Only the included draft is listed; the declined entry is an answer.
    assert_eq!(
        result["unconfirmed_links"],
        serde_json::json!(["TrackedTime > user_name"])
    );
    assert_eq!(result["unconfirmed_responses"], serde_json::json!([]));
    let schema: serde_json::Value = serde_json::from_str(&review_schema()).unwrap();
    assert_eq!(
        graphos_factory_core::jsonschema::validate(&result, &schema),
        Vec::<String>::new()
    );
}

#[test]
fn a_candidate_without_links_reports_no_links_section_and_an_empty_list() {
    let dir = fixture();
    // The gitea pilot's selection carries a links: section (ADR 0089). This
    // test is about a before and a candidate that both have none, so drop it
    // from the copied before; otherwise the candidate reads as removing it.
    let path = dir.path().join(".factory/selection.yaml");
    let before = fs::read_to_string(&path).unwrap();
    if let Some(at) = before.find("\nlinks:\n") {
        fs::write(&path, &before[..=at]).unwrap();
    }
    let input = token(dir.path());
    let result = review(dir.path(), Some(CANDIDATE), Some(&input), None).unwrap();
    let sections: Vec<&str> = result["changed_sections"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s.as_str())
        .collect();
    assert!(!sections.contains(&"links"), "{:?}", sections);
    assert_eq!(result["unconfirmed_links"], serde_json::json!([]));
}

/// ADR 0118: a decision added as a record file changes the reviewed inputs,
/// as an append to `decisions.json` does.
#[test]
fn a_decision_record_file_binds_the_review() {
    let dir = fixture();
    let input = token(dir.path());
    let argv: Vec<String> = [
        "add",
        dir.path().to_str().unwrap(),
        "--title",
        "Paginate by cursor",
        "--question",
        "Which way?",
        "--date",
        "2026-10-05",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(graphos_factory_core::cmd::decisions::main(&argv), 0);
    assert_eq!(
        graphos_factory_core::record_log::record_paths(dir.path())
            .unwrap()
            .len(),
        1
    );
    assert!(review(dir.path(), Some(CANDIDATE), Some(&input), None)
        .unwrap_err()
        .contains("stale selection inputs"));
}

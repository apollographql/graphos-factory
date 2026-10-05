use graphos_factory_core::jsonschema::validate;
use graphos_factory_core::schemas;
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

#[test]
fn lock_v1_records_optional_skill_binary_toolchain_inputs_and_outputs() {
    let dir = TempDir::new().unwrap();
    let workspace = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill: { name: example, version: 0.5.0 }\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: 2.12.0\nintake: spec\ncreated_at: 2026-09-16T00:00:00Z\ncontext_mode: generic\n";
    let selection = "contract_version: 1\ndefaults: { fields: all, max_depth: 6, opaque_json_policy: forbid }\noperations: {}\n";
    let schema = "type Query {\n  widget_co_ping: String\n}\n";
    let decisions = "{\"contract_version\":1,\"decisions\":[{\"id\":\"D-0001\",\"title\":\"Keep ping scalar\",\"status\":\"resolved\",\"date\":\"2026-09-16\",\"resolution\":{\"decision\":\"keep it\"}}]}\n";
    write(&dir.path().join(".factory/workspace.yaml"), workspace);
    write(&dir.path().join(".factory/selection.yaml"), selection);
    write(
        &dir.path().join(graphos_factory_core::decisions::FILE),
        decisions,
    );
    write(
        &dir.path().join(".factory/memory.md"),
        "# Memory\n\n## Auth\n\n- None.\n",
    );
    write(&dir.path().join("widget-co.graphql"), schema);
    write(&dir.path().join("template.yaml"), "variables: []\n");
    write(
        &dir.path().join("tests/cases/ping.graphql"),
        "query { widget_co_ping }\n",
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            "ping.graphql",
            dir.path().join("tests/cases/ping-link.graphql"),
        )
        .unwrap();
        std::os::unix::fs::symlink("..", dir.path().join("tests/cases/loop")).unwrap();
    }

    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "lock",
            dir.path().to_str().unwrap(),
            "--skill-dir",
            repo.to_str().unwrap(),
            "--model",
            "review-test-model",
            "--json",
        ])
        .env("GRAPHOS_FACTORY_CORE_LLM_MODEL", "ignored-env-model")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lock: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(lock["contract_version"], 1);
    assert_eq!(
        validate(
            &lock,
            &schemas::load("applied-lock.schema.json", None).unwrap()
        ),
        Vec::<String>::new()
    );
    let mut missing_provenance = lock.clone();
    missing_provenance
        .as_object_mut()
        .unwrap()
        .shift_remove("provenance");
    assert_eq!(
        validate(
            &missing_provenance,
            &schemas::load("applied-lock.schema.json", None).unwrap()
        ),
        Vec::<String>::new(),
        "v1 locks remain valid without optional provenance"
    );

    let revision = Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        lock["provenance"]["skill"]["revision"],
        String::from_utf8_lossy(&revision.stdout).trim()
    );
    assert!(lock["provenance"]["skill"]["dirty"].is_boolean());
    assert_eq!(lock["provenance"]["skill"]["source"], "--skill-dir");
    assert_eq!(
        lock["provenance"]["authoring_agent"]["model"],
        "review-test-model"
    );
    assert_eq!(lock["provenance"]["authoring_agent"]["attested"], true);
    assert_eq!(
        lock["provenance"]["binary"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(lock["provenance"]["binary"]["rustc"]
        .as_str()
        .unwrap()
        .starts_with("rustc "));
    let build_revision = lock["provenance"]["binary"]["revision"].as_str().unwrap();
    let build_dirty = lock["provenance"]["binary"]["dirty"].as_bool().unwrap();
    if build_revision == "unknown" {
        assert!(build_dirty, "an unknown build state must not report clean");
    } else {
        let status = Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "status",
                "--porcelain",
                "--untracked-files=all",
            ])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(
            build_dirty,
            !status.stdout.is_empty(),
            "the build stamp must match the source tree used by this Cargo invocation"
        );
    }
    assert_eq!(lock["provenance"]["toolchain"]["connect_spec"], "v0.3");
    assert_eq!(lock["provenance"]["toolchain"]["federation"], "2.12.0");
    assert_eq!(lock["provenance"]["toolchain"]["rover_pin"], "0.41.0");
    assert_eq!(lock["provenance"]["toolchain"]["router_pin"], "2.17.0");
    assert_eq!(lock["provenance"]["toolchain"]["wiremock_pin"], "3.13.2");

    let workspace_bytes = workspace.as_bytes();
    assert_eq!(
        lock["provenance"]["inputs"][".factory/workspace.yaml"]["bytes"],
        workspace_bytes.len() as u64
    );
    assert_eq!(
        lock["provenance"]["inputs"][".factory/workspace.yaml"]["sha256"],
        graphos_factory_core::patch::bytes_sha256(workspace_bytes)
    );
    assert_eq!(
        lock["provenance"]["inputs"][graphos_factory_core::decisions::FILE]["bytes"],
        decisions.len() as u64
    );
    assert_eq!(
        lock["provenance"]["outputs"]["widget-co.graphql"]["sha256"],
        graphos_factory_core::patch::bytes_sha256(schema.as_bytes())
    );
    assert_eq!(
        lock["provenance"]["outputs"]["tests/cases/ping.graphql"]["bytes"],
        25
    );
    #[cfg(unix)]
    {
        assert!(lock["provenance"]["outputs"]
            .get("tests/cases/ping-link.graphql")
            .is_none());
        assert_eq!(
            lock["provenance"]["omitted_outputs"]["tests/cases/ping-link.graphql"],
            "symlink not followed"
        );
        assert_eq!(
            lock["provenance"]["omitted_outputs"]["tests/cases/loop"],
            "symlink not followed"
        );
    }
    assert!(lock["provenance"]["reproducibility"]
        .as_str()
        .unwrap()
        .contains("model is self-reported"));
    assert!(!dir.path().join(".factory/context.yaml").exists());

    let env_output = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["lock", dir.path().to_str().unwrap(), "--json"])
        .env("GRAPHOS_FACTORY_CORE_LLM_MODEL", "environment-test-model")
        .output()
        .unwrap();
    assert_eq!(env_output.status.code(), Some(0));
    let env_lock: Value = serde_json::from_slice(&env_output.stdout).unwrap();
    assert_eq!(
        env_lock["provenance"]["authoring_agent"]["model"],
        "environment-test-model"
    );

    let unknown_output = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["lock", dir.path().to_str().unwrap(), "--json"])
        .env_remove("GRAPHOS_FACTORY_CORE_LLM_MODEL")
        .output()
        .unwrap();
    assert_eq!(unknown_output.status.code(), Some(0));
    let unknown_lock: Value = serde_json::from_slice(&unknown_output.stdout).unwrap();
    assert_eq!(
        unknown_lock["provenance"]["authoring_agent"]["model"],
        "unknown"
    );

    let check = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["lock", dir.path().to_str().unwrap(), "--check"])
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(0));
}

#[test]
fn provenance_includes_specialized_context_contract_and_exact_artifact_bytes() {
    let dir = TempDir::new().unwrap();
    write(
        &dir.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill: { name: example, version: 0.5.0 }\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: 2.12.0\nintake: spec\ncreated_at: 2026-09-16T00:00:00Z\ncontext_mode: specialized\n",
    );
    write(
        &dir.path().join("widget-co.graphql"),
        "type Query { ping: String }\n",
    );
    let artifact = b"{\"fields\":[\"revenue\"]}\n";
    let artifact_rel = ".factory/context-artifacts/fields-raw/artifact.json";
    std::fs::create_dir_all(dir.path().join(".factory/context-artifacts/fields-raw")).unwrap();
    std::fs::write(dir.path().join(artifact_rel), artifact).unwrap();
    let context = format!(
        "contract_version: 1\nintent: Typed fields\ninputs: []\nrequirements:\n  - id: fields\n    phase: build\n    affects: [typed fields]\n    reason: External definitions\n    resolve_with: Capture export\n    status: resolved\n    resolution: Approved export\n    evidence:\n      - id: fields-raw\n        path: {artifact_rel}\n        sha256: {}\n        byte_count: {}\n        representation: raw\n        authority: authoritative\n        format: json\n        captured_at: 2026-09-16T00:00:00Z\n        capture_method: export\n        source: approved export\n        scope: [selected fields]\n",
        graphos_factory_core::patch::bytes_sha256(artifact),
        artifact.len()
    );
    write(&dir.path().join(".factory/context.yaml"), &context);

    let provenance =
        graphos_factory_core::provenance::record(dir.path(), "widget-co.graphql", None, None)
            .unwrap();
    assert_eq!(
        provenance["inputs"][".factory/context.yaml"]["sha256"],
        graphos_factory_core::patch::bytes_sha256(context.as_bytes())
    );
    assert_eq!(
        provenance["inputs"][artifact_rel]["sha256"],
        graphos_factory_core::patch::bytes_sha256(artifact)
    );
    assert_eq!(
        provenance["inputs"][artifact_rel]["bytes"],
        artifact.len() as u64
    );
}

fn run(args: &[&str]) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
    )
}

#[test]
fn lock_check_lists_provenance_drift_and_only_provenance_enforces_it() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    write(
        &dir.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill: { name: example, version: 0.5.0 }\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: 2.12.0\nintake: spec\ncreated_at: 2026-09-16T00:00:00Z\n",
    );
    write(
        &dir.path().join("widget-co.graphql"),
        "type Query {\n  widget_co_ping: String\n}\n",
    );
    write(
        &dir.path().join("tests/fixtures/mappings/ping.json"),
        "{\"request\": {\"url\": \"/ping\"}}\n",
    );
    write(
        &dir.path().join("tests/cases/ping.graphql"),
        "query { widget_co_ping }\n",
    );
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let (code, _) = run(&["lock", ws, "--check", "--provenance"]);
    assert_eq!(code, Some(0), "a fresh lock has no provenance drift");

    // A stub changes after the lock and a case is deleted: neither is a span,
    // a pinned source or the inventory, so only provenance sees it.
    write(
        &dir.path().join("tests/fixtures/mappings/ping.json"),
        "{\"request\": {\"url\": \"/ping?x=1\"}}\n",
    );
    std::fs::remove_file(dir.path().join("tests/cases/ping.graphql")).unwrap();

    let (code, stdout) = run(&["lock", ws, "--check"]);
    assert_eq!(
        code,
        Some(0),
        "plain --check reports drift without enforcing it"
    );
    assert!(stdout.contains("2 files recorded"), "{}", stdout);
    assert!(
        stdout.contains("~ tests/fixtures/mappings/ping.json   (outputs)"),
        "{}",
        stdout
    );
    assert!(
        stdout.contains("- tests/cases/ping.graphql   (outputs, missing)"),
        "{}",
        stdout
    );

    let (code, _) = run(&["lock", ws, "--check", "--provenance"]);
    assert_eq!(code, Some(3), "--provenance enforces the drift");

    let (_, json) = run(&["lock", ws, "--check", "--json"]);
    let report: Value = serde_json::from_str(&json).unwrap();
    let drift = report["provenance_drift"].as_array().unwrap();
    assert_eq!(drift.len(), 2);
    assert!(drift
        .iter()
        .any(|d| d["path"] == "tests/fixtures/mappings/ping.json"
            && d["section"] == "outputs"
            && d["change"] == "changed"));
    assert!(drift
        .iter()
        .any(|d| d["path"] == "tests/cases/ping.graphql" && d["change"] == "missing"));

    // Relocking records the workspace as it stands.
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let (code, stdout) = run(&["lock", ws, "--check", "--provenance"]);
    assert_eq!(code, Some(0), "{}", stdout);
    assert!(!stdout.contains("provenance differ"), "{}", stdout);
}

#[test]
fn lock_check_withholds_relock_advice_while_a_hand_edit_is_uncodified() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    write(
        &dir.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill: { name: example, version: 0.5.0 }\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: 2.12.0\nintake: spec\ncreated_at: 2026-09-16T00:00:00Z\n",
    );
    let schema = "type Query {\n  widget_co_ping: String\n}\n";
    write(&dir.path().join("widget-co.graphql"), schema);
    write(&dir.path().join(".factory/inventory.json"), "{}\n");
    write(&dir.path().join(".factory/memory.md"), "# a lesson\n");
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));

    let relock = "record the workspace as it stands";
    let withheld = "do not relock yet";

    // An engineer hand-adds a root field. The schema is a recorded output, so
    // provenance lists it next to the span, and a relock would erase the
    // only record of the edit (`codify --key` then refuses it as in sync).
    write(
        &dir.path().join("widget-co.graphql"),
        "type Query {\n  widget_co_ping: String\n  widget_co_pong: String\n}\n",
    );
    let (code, stdout) = run(&["lock", ws, "--check"]);
    assert_eq!(code, Some(3), "{}", stdout);
    assert!(stdout.contains("codify each"), "{}", stdout);
    assert!(
        stdout.contains("~ widget-co.graphql   (outputs)"),
        "{}",
        stdout
    );
    assert!(!stdout.contains(relock), "{}", stdout);
    assert!(stdout.contains(withheld), "{}", stdout);
    write(&dir.path().join("widget-co.graphql"), schema);

    // An edited inventory is a recorded input: rebuild it, never relock it.
    write(
        &dir.path().join(".factory/inventory.json"),
        "{\"edited\": true}\n",
    );
    let (code, stdout) = run(&["lock", ws, "--check"]);
    assert_eq!(code, Some(3), "{}", stdout);
    assert!(
        stdout.contains("~ .factory/inventory.json   (inputs)"),
        "{}",
        stdout
    );
    assert!(!stdout.contains(relock), "{}", stdout);
    assert!(stdout.contains(withheld), "{}", stdout);
    write(&dir.path().join(".factory/inventory.json"), "{}\n");

    // Drift that is provenance alone keeps the relock advice.
    write(
        &dir.path().join(".factory/memory.md"),
        "# a lesson, edited\n",
    );
    let (code, stdout) = run(&["lock", ws, "--check"]);
    assert_eq!(code, Some(0), "{}", stdout);
    assert!(
        stdout.contains("~ .factory/memory.md   (inputs)"),
        "{}",
        stdout
    );
    assert!(stdout.contains(relock), "{}", stdout);
    assert!(!stdout.contains(withheld), "{}", stdout);
}

const WIDGET_WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill: { name: example, version: 0.5.0 }\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: 2.12.0\nintake: spec\ncreated_at: 2026-09-16T00:00:00Z\n";

fn widget_workspace(dir: &Path) {
    write(&dir.join(".factory/workspace.yaml"), WIDGET_WORKSPACE);
    write(
        &dir.join("widget-co.graphql"),
        "type Query {\n  widget_co_ping: String\n}\n",
    );
}

fn lock_json(ws: &str) -> Value {
    let (_, json) = run(&["lock", ws, "--check", "--json"]);
    serde_json::from_str(&json).unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&status.stderr)
    );
}

#[test]
fn lock_provenance_without_check_is_refused_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let before = std::fs::read(dir.path().join(".factory/applied.lock.yaml")).unwrap();

    // `--provenance` belongs to `--check`; on its own it used to fall through
    // to a write and relock, re-attesting the model and the skill source.
    let output = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["lock", ws, "--provenance", "--model", "someone-else"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--provenance is a --check option"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(dir.path().join(".factory/applied.lock.yaml")).unwrap(),
        before,
        "the lock must not be rewritten"
    );
}

#[test]
fn provenance_records_only_the_files_git_would_commit_and_never_dotfiles() {
    // A workspace nested in a larger repository, as in a monorepo of workspaces.
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "-q"]);
    let dir = repo.path().join("services/widget-co");
    let ws = dir.to_str().unwrap();
    widget_workspace(&dir);
    write(
        &dir.join("tests/cases/tracked.graphql"),
        "query { widget_co_ping }\n",
    );
    write(
        &dir.join("tests/cases/scaffolded.graphql"),
        "query { widget_co_ping }\n",
    );
    write(&dir.join("tests/.DS_Store"), "finder noise\n");
    write(
        &dir.join("tests/cases/.tracked.graphql.swp"),
        "editor noise\n",
    );
    write(&dir.join("tests/cases/scratch.log"), "ignored\n");
    write(&repo.path().join(".gitignore"), "*.log\n");
    git(
        repo.path(),
        &["add", "services/widget-co/tests/cases/tracked.graphql"],
    );

    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let lock: Value = graphos_factory_core::yaml::parse(
        &std::fs::read_to_string(dir.join(".factory/applied.lock.yaml")).unwrap(),
    )
    .unwrap();
    let outputs = lock["provenance"]["outputs"].as_object().unwrap();
    let tests: Vec<&str> = outputs
        .keys()
        .map(String::as_str)
        .filter(|k| k.starts_with("tests/"))
        .collect();
    // Tracked, and untracked-but-not-ignored (a case scaffolded in the same
    // apply is not committed until after `lock`), relative to the workspace.
    assert_eq!(
        tests,
        vec![
            "tests/cases/scaffolded.graphql",
            "tests/cases/tracked.graphql"
        ],
        "{:?}",
        outputs.keys().collect::<Vec<_>>()
    );

    // A checkout never has the local noise, so nothing reads as missing.
    std::fs::remove_file(dir.join("tests/.DS_Store")).unwrap();
    std::fs::remove_file(dir.join("tests/cases/scratch.log")).unwrap();
    let (code, stdout) = run(&["lock", ws, "--check", "--provenance"]);
    assert_eq!(code, Some(0), "{}", stdout);

    // Outside a git work tree every file is recorded, still without dotfiles.
    let bare = TempDir::new().unwrap();
    let bare_ws = bare.path().to_str().unwrap();
    widget_workspace(bare.path());
    write(
        &bare.path().join("tests/cases/ping.graphql"),
        "query { widget_co_ping }\n",
    );
    write(&bare.path().join("tests/.DS_Store"), "finder noise\n");
    write(&bare.path().join("tests/.cache/x.json"), "{}\n");
    let (code, _) = run(&["lock", bare_ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let lock: Value = graphos_factory_core::yaml::parse(
        &std::fs::read_to_string(bare.path().join(".factory/applied.lock.yaml")).unwrap(),
    )
    .unwrap();
    let tests: Vec<&str> = lock["provenance"]["outputs"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|k| k.starts_with("tests/"))
        .collect();
    assert_eq!(tests, vec!["tests/cases/ping.graphql"]);
}

#[test]
fn lock_check_relock_advice_follows_the_remedy_for_pinned_sources() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    write(
        &dir.path().join(".factory/sources.lock.yaml"),
        "contract_version: 1\nsources: []\n",
    );
    write(
        &dir.path().join("openapi.json"),
        "{\n  \"openapi\": \"3.0.3\",\n  \"info\": {\"title\": \"Widget Co\", \"version\": \"1.0.0\"},\n  \"paths\": {}\n}\n",
    );
    let (code, stdout) = run(&["sources", "pin", ws, "--path", "openapi.json"]);
    assert_eq!(code, Some(0), "{}", stdout);
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        lock_json(ws)["relock_advisable"],
        false,
        "nothing to relock"
    );

    let relock = "record the workspace as it stands";
    let withheld = "do not relock yet";

    // A hand edit to the pinned document: a source problem, and the working
    // copy is a recorded input too. Relocking would acknowledge the edit
    // before `codify --source` records it as a patch.
    let pinned = std::fs::read_to_string(dir.path().join("openapi.json")).unwrap();
    write(
        &dir.path().join("openapi.json"),
        &pinned.replace("Widget Co", "Widget Company"),
    );
    let (code, stdout) = run(&["lock", ws, "--check"]);
    assert_eq!(code, Some(3), "{}", stdout);
    assert!(stdout.contains("pinned source"), "{}", stdout);
    assert!(stdout.contains("~ openapi.json   (inputs)"), "{}", stdout);
    assert!(!stdout.contains(relock), "{}", stdout);
    assert!(stdout.contains(withheld), "{}", stdout);
    let report = lock_json(ws);
    assert_eq!(report["relock_advisable"], false);
    assert_eq!(
        report["source_problems"],
        serde_json::json!(["openapi.json"])
    );
    write(&dir.path().join("openapi.json"), &pinned);

    // The entry removed from sources.lock.yaml: an unpinned document is not a
    // hand edit, and its own remedy is a relock, so the drift advice agrees.
    write(
        &dir.path().join(".factory/sources.lock.yaml"),
        "contract_version: 1\nsources: []\n",
    );
    let (code, stdout) = run(&["lock", ws, "--check"]);
    assert_eq!(code, Some(3), "{}", stdout);
    assert!(stdout.contains("no longer pins it"), "{}", stdout);
    assert!(
        stdout.contains("~ .factory/sources.lock.yaml   (inputs)"),
        "{}",
        stdout
    );
    assert!(stdout.contains(relock), "{}", stdout);
    assert!(!stdout.contains(withheld), "{}", stdout);
    assert_eq!(lock_json(ws)["relock_advisable"], true);

    // Following the advice settles it.
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let (code, stdout) = run(&["lock", ws, "--check", "--provenance"]);
    assert_eq!(code, Some(0), "{}", stdout);
}

/// `lock` with the provenance environment cleared, so the developer's own
/// shell (which may export these) cannot change what the test sees. It runs
/// from inside the workspace, as an agent does, so a relative or empty
/// variable resolves against the workspace and not this repo's checkout.
fn lock_with(ws: &str, extra: &[&str], env: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"));
    cmd.current_dir(ws).arg("lock").arg(ws).args(extra);
    for var in ["LLM_MODEL", "SCRIPTS", "SKILL_ROOT"] {
        cmd.env_remove(format!("GRAPHOS_FACTORY_CORE_{}", var));
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// A committed Git checkout laid out like this repo, so
/// `GRAPHOS_FACTORY_CORE_SCRIPTS` two levels down resolves to its root.
fn skill_checkout() -> TempDir {
    let skill = TempDir::new().unwrap();
    write(
        &skill.path().join("example/scripts/unit.sh"),
        "#!/usr/bin/env bash\n",
    );
    git(skill.path(), &["init", "-q"]);
    git(skill.path(), &["add", "."]);
    git(
        skill.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "skill",
        ],
    );
    skill
}

const MODEL_WARNING: &str = "lock: warning: provenance.authoring_agent.model recorded as unknown";
const SKILL_WARNING: &str = "lock: warning: provenance.skill.source recorded as binary-build";

#[test]
fn lock_warns_once_per_provenance_fallback_without_changing_the_exit_code() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());

    // Neither the model nor the skill checkout is named: both fall back.
    let (code, stderr) = lock_with(ws, &[], &[]);
    assert_eq!(code, Some(0), "{}", stderr);
    assert_eq!(stderr.matches(MODEL_WARNING).count(), 1, "{}", stderr);
    assert_eq!(stderr.matches(SKILL_WARNING).count(), 1, "{}", stderr);
    // Each warning is one line and names both ways to supply the value.
    let model_line = stderr.lines().find(|l| l.contains(MODEL_WARNING)).unwrap();
    assert!(model_line.contains("--model MODEL"), "{}", model_line);
    assert!(
        model_line.contains("GRAPHOS_FACTORY_CORE_LLM_MODEL"),
        "{}",
        model_line
    );
    let skill_line = stderr.lines().find(|l| l.contains(SKILL_WARNING)).unwrap();
    assert!(
        skill_line.contains("neither --skill-dir nor GRAPHOS_FACTORY_CORE_SCRIPTS is set"),
        "{}",
        skill_line
    );
    assert!(skill_line.contains("--skill-dir DIR"), "{}", skill_line);
    assert!(
        skill_line.contains("GRAPHOS_FACTORY_CORE_SCRIPTS"),
        "{}",
        skill_line
    );
    // The lock content is unchanged: the fallbacks are still what is recorded.
    let lock = lock_json(ws);
    assert_eq!(lock["lock"], true);
    let text = std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap();
    assert!(text.contains("model: unknown"), "{}", text);
    assert!(text.contains("source: binary-build"), "{}", text);

    // --check writes nothing and so records nothing to warn about.
    let (code, stderr) = lock_with(ws, &["--check"], &[]);
    assert_eq!(code, Some(0), "{}", stderr);
    assert!(!stderr.contains("warning"), "{}", stderr);

    // Only the model named: only the skill warning remains.
    let (code, stderr) = lock_with(ws, &["--model", "m-1"], &[]);
    assert_eq!(code, Some(0), "{}", stderr);
    assert!(!stderr.contains(MODEL_WARNING), "{}", stderr);
    assert!(stderr.contains(SKILL_WARNING), "{}", stderr);
}

#[test]
fn lock_is_silent_when_model_and_skill_checkout_are_supplied() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    let skill = skill_checkout();

    // By flag.
    let (code, stderr) = lock_with(
        ws,
        &[
            "--model",
            "m-1",
            "--skill-dir",
            skill.path().to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, Some(0), "{}", stderr);
    assert!(!stderr.contains("warning"), "{}", stderr);

    // By environment, the way SKILL.md sets up a session.
    let scripts = skill.path().join("example/scripts");
    let (code, stderr) = lock_with(
        ws,
        &[],
        &[
            ("GRAPHOS_FACTORY_CORE_LLM_MODEL", "m-1"),
            ("GRAPHOS_FACTORY_CORE_SCRIPTS", scripts.to_str().unwrap()),
        ],
    );
    assert_eq!(code, Some(0), "{}", stderr);
    assert!(!stderr.contains("warning"), "{}", stderr);
    let text = std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap();
    assert!(
        text.contains("source: GRAPHOS_FACTORY_CORE_SCRIPTS"),
        "{}",
        text
    );
    // The variable the skill root was read under is a value the lock's
    // schema accepts, so lint's contract check passes a lock written from a
    // session's environment.
    let lock_file = dir.path().join(".factory/applied.lock.yaml");
    let (_, json) = run(&["yaml2json", lock_file.to_str().unwrap()]);
    let lock: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        validate(
            &lock,
            &schemas::load("applied-lock.schema.json", None).unwrap()
        ),
        Vec::<String>::new()
    );
}

/// The new name wins over nothing: with GRAPHOS_FACTORY_CORE_LLM_MODEL and GRAPHOS_FACTORY_CORE_SCRIPTS
/// set the lock records both as read, and the binary that wrote it is named
/// beside its version. (That the pre-split names are not read is a
/// target suite's `legacy_env`.)
#[test]
fn the_graphos_factory_core_variable_names_are_read_and_recorded_as_read() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    let skill = skill_checkout();
    let scripts = skill.path().join("example/scripts");
    let scripts = scripts.to_str().unwrap();
    let (code, stderr) = lock_with(
        ws,
        &[],
        &[
            ("GRAPHOS_FACTORY_CORE_LLM_MODEL", "m-new"),
            ("GRAPHOS_FACTORY_CORE_SCRIPTS", scripts),
        ],
    );
    assert_eq!(code, Some(0), "{}", stderr);
    let text = std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap();
    assert!(
        text.contains("source: GRAPHOS_FACTORY_CORE_SCRIPTS"),
        "{}",
        text
    );
    assert!(text.contains("model: m-new"), "{}", text);
    // The binary that wrote the lock is named beside its version.
    assert!(
        text.contains(&format!(
            "written_by: \"graphos-factory-bare {}\"",
            env!("CARGO_PKG_VERSION")
        )),
        "{}",
        text
    );
    assert!(
        text.contains("\n    name: graphos-factory-bare\n"),
        "{}",
        text
    );
}

/// Runs `cmd`, retrying while exec fails with ETXTBSY. On Linux a binary
/// just written by `fs::copy` cannot be exec'd while any process holds a
/// write descriptor to it, and a child another test thread forks while the
/// copy is open inherits that descriptor until it execs itself
/// (rust-lang/rust#114554). The window is short, so a few retries close it.
fn output_retrying_busy(cmd: &mut Command) -> std::process::Output {
    let mut attempt = 0;
    loop {
        match cmd.output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 50 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => return result.unwrap(),
        }
    }
}

/// The recorded writer is the constant its composition root names (here
/// `graphos-factory-bare`), never argv[0]: a copy of the binary invoked under
/// another file name (the `graphos-factory-core` link every product installs,
/// an alias) writes the same `written_by` and `binary.name`, so a later relock
/// by the binary itself does not churn the lock.
#[test]
fn a_binary_invoked_under_another_name_still_records_its_own() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    let bins = TempDir::new().unwrap();
    let copy = bins.path().join("old-name");
    std::fs::copy(env!("CARGO_BIN_EXE_graphos-factory-bare"), &copy).unwrap();
    let link = bins.path().join("graphos-factory-core");
    std::os::unix::fs::symlink(&copy, &link).unwrap();
    for bin in [&copy, &link] {
        let out = output_retrying_busy(
            Command::new(bin)
                .current_dir(ws)
                .args(["lock", ws, "--model", "m-1"])
                .env_remove("GRAPHOS_FACTORY_CORE_SCRIPTS")
                .env_remove("GRAPHOS_FACTORY_CORE_SKILL_ROOT"),
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let text = std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap();
        assert!(
            text.contains(&format!(
                "written_by: \"graphos-factory-bare {}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "{}: {}",
            bin.display(),
            text
        );
        assert!(
            text.contains("\n    name: graphos-factory-bare\n"),
            "{}: {}",
            bin.display(),
            text
        );
        assert!(!text.contains("old-name"), "{}: {}", bin.display(), text);
    }
}

#[test]
fn lock_skill_warning_names_a_scripts_dir_outside_any_checkout() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    // An installed plugin copy: the right layout, but not a Git checkout.
    let plugin = TempDir::new().unwrap();
    let scripts = plugin.path().join("a/scripts");
    std::fs::create_dir_all(&scripts).unwrap();

    let (code, stderr) = lock_with(
        ws,
        &["--model", "m-1"],
        &[("GRAPHOS_FACTORY_CORE_SCRIPTS", scripts.to_str().unwrap())],
    );
    assert_eq!(code, Some(0), "{}", stderr);
    let line = stderr.lines().find(|l| l.contains(SKILL_WARNING));
    let line = line.unwrap_or_else(|| panic!("{}", stderr));
    let expected = format!(
        "GRAPHOS_FACTORY_CORE_SCRIPTS is set to '{}', so the skill root is taken as {} (two levels up), which is not a Git checkout",
        scripts.display(),
        plugin.path().canonicalize().unwrap().display()
    );
    assert!(line.contains(&expected), "{}", line);
}

/// The skill line of a `lock --model m-1` run, which must exist.
fn skill_warning(ws: &str, env: &[(&str, &str)]) -> String {
    let (code, stderr) = lock_with(ws, &["--model", "m-1"], env);
    assert_eq!(code, Some(0), "{}", stderr);
    let line = stderr.lines().find(|l| l.contains(SKILL_WARNING));
    line.unwrap_or_else(|| panic!("{:?}: {}", env, stderr))
        .to_string()
}

#[test]
fn lock_skill_warning_names_a_set_scripts_value_that_resolves_nowhere() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());

    // SKILL.md's `export GRAPHOS_FACTORY_CORE_SCRIPTS=$S` with S unset: the
    // variable is set, so the line must not say it is not.
    let line = skill_warning(ws, &[("GRAPHOS_FACTORY_CORE_SCRIPTS", "")]);
    assert!(
        line.contains("GRAPHOS_FACTORY_CORE_SCRIPTS is set but empty"),
        "{}",
        line
    );
    assert!(!line.contains("neither"), "{}", line);

    // A short absolute path and a relative one, both with fewer than two
    // parents to walk up.
    for value in ["/x", "scripts"] {
        let line = skill_warning(ws, &[("GRAPHOS_FACTORY_CORE_SCRIPTS", value)]);
        let expected = format!(
            "GRAPHOS_FACTORY_CORE_SCRIPTS is set to '{}', which is not inside a Git checkout",
            value
        );
        assert!(line.contains(&expected), "{}", line);
        assert!(!line.contains("neither"), "{}", line);
    }
}

#[test]
fn lock_skill_warning_follows_skill_root_when_it_overrides_scripts() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    let skill = skill_checkout();
    let scripts = skill.path().join("example/scripts");
    let elsewhere = TempDir::new().unwrap();

    // GRAPHOS_FACTORY_CORE_SCRIPTS is right; GRAPHOS_FACTORY_CORE_SKILL_ROOT wins and
    // is wrong. Setting GRAPHOS_FACTORY_CORE_SCRIPTS again would change nothing.
    let line = skill_warning(
        ws,
        &[
            (
                "GRAPHOS_FACTORY_CORE_SKILL_ROOT",
                elsewhere.path().to_str().unwrap(),
            ),
            ("GRAPHOS_FACTORY_CORE_SCRIPTS", scripts.to_str().unwrap()),
        ],
    );
    assert!(
        line.contains(&format!(
            "GRAPHOS_FACTORY_CORE_SKILL_ROOT is set to {}, which is not a Git checkout, and it takes precedence over GRAPHOS_FACTORY_CORE_SCRIPTS",
            elsewhere.path().display()
        )),
        "{}",
        line
    );
    assert!(
        line.contains("point GRAPHOS_FACTORY_CORE_SKILL_ROOT at that checkout or unset it"),
        "{}",
        line
    );
    assert!(
        !line.contains("set GRAPHOS_FACTORY_CORE_SCRIPTS"),
        "{}",
        line
    );
}

#[test]
fn lock_never_records_the_workspace_commit_as_the_skill() {
    // `git -C ""` runs in the current directory. With the workspace itself a
    // Git checkout, an empty GRAPHOS_FACTORY_CORE_SKILL_ROOT, or a relative
    // GRAPHOS_FACTORY_CORE_SCRIPTS exactly two components deep, used to record
    // the workspace's own commit as the skill revision.
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    widget_workspace(dir.path());
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "workspace",
        ],
    );
    let lock_file = dir.path().join(".factory/applied.lock.yaml");
    for (var, value) in [
        ("GRAPHOS_FACTORY_CORE_SKILL_ROOT", ""),
        ("GRAPHOS_FACTORY_CORE_SCRIPTS", "a/b"),
    ] {
        let (code, stderr) = lock_with(ws, &["--model", "m-1"], &[(var, value)]);
        assert_eq!(code, Some(0), "{}", stderr);
        let text = std::fs::read_to_string(&lock_file).unwrap();
        assert!(
            text.contains("source: binary-build"),
            "{}={:?}: {}",
            var,
            value,
            text
        );
        assert!(stderr.contains(SKILL_WARNING), "{}", stderr);
    }

    // An empty GRAPHOS_FACTORY_CORE_SKILL_ROOT counts as unset, so a valid
    // GRAPHOS_FACTORY_CORE_SCRIPTS still names the skill.
    let skill = skill_checkout();
    let scripts = skill.path().join("example/scripts");
    let (code, stderr) = lock_with(
        ws,
        &["--model", "m-1"],
        &[
            ("GRAPHOS_FACTORY_CORE_SKILL_ROOT", ""),
            ("GRAPHOS_FACTORY_CORE_SCRIPTS", scripts.to_str().unwrap()),
        ],
    );
    assert_eq!(code, Some(0), "{}", stderr);
    assert!(!stderr.contains("warning"), "{}", stderr);
    let text = std::fs::read_to_string(&lock_file).unwrap();
    assert!(
        text.contains("source: GRAPHOS_FACTORY_CORE_SCRIPTS"),
        "{}",
        text
    );
}

/// `findings.json` is an authoring input (ADR 0113 §2): the lock hashes it,
/// and an edit to it is provenance drift, as for decisions.json.
#[test]
fn findings_json_is_a_recorded_input() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    write(
        &dir.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill: { name: example, version: 0.5.0 }\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: 2.12.0\nintake: spec\ncreated_at: 2026-09-16T00:00:00Z\n",
    );
    write(
        &dir.path().join("widget-co.graphql"),
        "type Query {\n  widget_co_ping: String\n}\n",
    );
    write(&dir.path().join(".factory/inventory.json"), "{}\n");
    write(
        &dir.path().join(".factory/findings.json"),
        "{\"contract_version\": 1, \"findings\": []}\n",
    );
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let lock = std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap();
    assert!(lock.contains(".factory/findings.json"), "{}", lock);
    write(
        &dir.path().join(".factory/findings.json"),
        "{\"contract_version\": 1, \"findings\": [ ]}\n",
    );
    let (code, stdout) = run(&["lock", ws, "--check"]);
    assert_eq!(code, Some(0), "{}", stdout);
    assert!(
        stdout.contains("~ .factory/findings.json   (inputs)"),
        "{}",
        stdout
    );
}

/// ADR 0118: a decision is added as a new record file, not appended to
/// `decisions.json`, so the lock must name the file a relock records and
/// `--check --provenance` must see one it never recorded, or a lock would
/// describe a log it never saw.
#[test]
fn a_decision_record_added_after_the_lock_is_provenance_drift() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path().to_str().unwrap();
    write(
        &dir.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill: { name: example, version: 0.5.0 }\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: 2.12.0\nintake: spec\ncreated_at: 2026-09-16T00:00:00Z\n",
    );
    write(
        &dir.path().join("widget-co.graphql"),
        "type Query {\n  widget_co_ping: String\n}\n",
    );
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let (code, _) = run(&[
        "decisions",
        "add",
        ws,
        "--title",
        "Paginate by cursor",
        "--question",
        "Which way?",
        "--date",
        "2026-10-05",
    ]);
    assert_eq!(code, Some(0));
    let record = graphos_factory_core::record_log::record_paths(dir.path()).unwrap();
    assert_eq!(record.len(), 1, "{:?}", record);
    let record = &record[0];

    let (code, stdout) = run(&["lock", ws, "--check", "--provenance"]);
    assert_eq!(
        code,
        Some(3),
        "an unrecorded record file is drift: {}",
        stdout
    );
    assert!(
        stdout.contains(&format!("+ {}   (inputs, added since the lock)", record)),
        "{}",
        stdout
    );
    let (_, json) = run(&["lock", ws, "--check", "--json"]);
    let report: Value = serde_json::from_str(&json).unwrap();
    assert!(report["provenance_drift"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["path"] == record.as_str()
            && d["section"] == "inputs"
            && d["change"] == "added"));

    // A relock records the file and its hash, and the check is clean again.
    let (code, _) = run(&["lock", ws, "--model", "review-test-model"]);
    assert_eq!(code, Some(0));
    let lock = graphos_factory_core::yaml::parse(
        &std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        lock["provenance"]["inputs"][record.as_str()]["sha256"],
        graphos_factory_core::patch::bytes_sha256(&std::fs::read(dir.path().join(record)).unwrap())
    );
    let (code, _) = run(&["lock", ws, "--check", "--provenance"]);
    assert_eq!(code, Some(0));
}

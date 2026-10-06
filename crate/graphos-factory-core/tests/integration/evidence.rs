//! `graphos-factory-core evidence` against stub wrappers: the layer outputs are
//! canned, so what is under test is how the command reads them into
//! .factory/evidence/latest.json — statuses, per-operation results, live
//! exclusions — and that the file satisfies its schema.

use graphos_factory_core::cmd;
use graphos_factory_core::jsonschema::validate;
use graphos_factory_core::schemas;
use serde_json::{json, Value};
use std::path::Path;

const WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\ncontext_mode: generic\n";

const SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/widgets\":\n    include: true\n    graphql: { root: query, name: listWidgets }\n  \"get:/widgets/{id}\":\n    include: true\n    graphql: { root: query, name: widget }\n  \"post:/widgets\":\n    include: true\n    graphql: { root: mutation, name: createWidget }\n";

fn write(dir: &Path, rel: &str, text: &str) {
    let f = dir.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, text).unwrap();
}

fn script(dir: &Path, name: &str, body: &str) {
    let f = dir.join(name);
    std::fs::write(&f, format!("#!/usr/bin/env bash\n{}\n", body)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A workspace plus a scripts directory whose wrappers print canned output.
fn setup(live_body: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let ws = tempfile::tempdir().unwrap();
    let d = ws.path();
    write(d, ".factory/workspace.yaml", WORKSPACE);
    write(d, ".factory/selection.yaml", SELECTION);
    // lint stops at a missing schema file, so the stub has a minimal one.
    write(
        d,
        "widget-co.graphql",
        "extend schema\n  @link(url: \"https://specs.apollo.dev/federation/v2.12\", import: [\"@key\"])\n  @link(url: \"https://specs.apollo.dev/connect/v0.3\", import: [\"@source\", \"@connect\"])\n\n@source(name: \"widget_co\", http: { baseURL: \"{{BASE_URL}}\" })\n\ntype Widget_Co_Widget { id: ID }\n\ntype Query {\n  widget_co_listWidgets: [Widget_Co_Widget] @connect(source: \"widget_co\", http: { GET: \"/widgets\" }, selection: \"$.widgets { id }\")\n  widget_co_widget(id: ID!): Widget_Co_Widget @connect(source: \"widget_co\", http: { GET: \"/widgets/{$args.id}\" }, selection: \"id\")\n}\n\ntype Mutation {\n  widget_co_createWidget(name: String): Widget_Co_Widget @connect(source: \"widget_co\", http: { POST: \"/widgets\", body: \"name: $args.name\" }, selection: \"id\")\n}\n",
    );
    write(
        d,
        "tests/cases/list_widgets.graphql",
        "query { widget_co_listWidgets { id } }\n",
    );
    write(
        d,
        "tests/cases/widget.graphql",
        "query { widget_co_widget(id: \"1\") { id } }\n",
    );
    write(
        d,
        "tests/live/list_widgets.graphql",
        "query { widget_co_listWidgets { id } }\n",
    );
    let scripts = tempfile::tempdir().unwrap();
    let s = scripts.path();
    script(
        s,
        "compose.sh",
        "echo 'compose: Rover 0.40.0, federation 2.12.0, connect v0.3'; exit 0",
    );
    script(s, "unit.sh", "echo 'TEST CASE: list @Query.widget_co_listWidgets'; echo 'TEST RESULTS: SUCCESSFUL 1  passed; 0 failed; 0 skipped'; exit 0");
    script(
        s,
        "e2e.sh",
        "echo 'PASS: list_widgets'; echo 'PASS: widget'; echo 'e2e: 2 passed, 0 failed'; exit 0",
    );
    script(s, "live.sh", live_body);
    (ws, scripts)
}

fn run(ws: &Path, scripts: &Path, skip: &str) -> (i32, Value) {
    let mut argv = vec![
        ws.to_str().unwrap().to_string(),
        "--scripts".to_string(),
        scripts.to_str().unwrap().to_string(),
    ];
    if !skip.is_empty() {
        argv.push("--skip".into());
        argv.push(skip.into());
    }
    let code = cmd::evidence::main(&argv);
    let text = std::fs::read_to_string(ws.join(".factory/evidence/latest.json")).unwrap();
    (code, graphos_factory_core::json::parse(&text).unwrap())
}

#[test]
fn live_exclusions_become_excluded_operations_with_their_reason() {
    let (ws, scripts) = setup(
        "echo 'live: Apollo Router 2.14.0 against the real API, 1 case(s)'; \
         echo 'PASS: list_widgets (read)'; \
         echo 'EXCLUDED: get:/widgets/{id} — needs an id from a previous response'; \
         echo 'EXCLUDED: post:/widgets — a write against the shared account'; \
         echo 'live: 1 passed, 0 failed, 2 excluded'; exit 0",
    );
    // conformance and lint are the binary's own subcommands; they need files
    // this stub workspace does not have, so they are skipped here.
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 0);
    assert_eq!(
        validate(&ev, &schemas::load("evidence.schema.json", None).unwrap()),
        Vec::<String>::new()
    );
    let live = &ev["layers"]["live"];
    assert_eq!(live["status"], "pass");
    assert_eq!(live["cases"], 1);
    assert_eq!(
        live["exclusions"],
        json!([
            {"operation": "get:/widgets/{id}", "reason": "needs an id from a previous response"},
            {"operation": "post:/widgets", "reason": "a write against the shared account"}
        ])
    );
    let ops = &ev["operations"];
    assert_eq!(ops["get:/widgets"]["live"], "pass");
    assert_eq!(ops["get:/widgets/{id}"]["live"], "excluded");
    assert_eq!(ops["post:/widgets"]["live"], "excluded");
    // e2e results map to operations through the case documents' root fields.
    assert_eq!(ops["get:/widgets"]["e2e"], "pass");
    assert_eq!(ops["get:/widgets/{id}"]["e2e"], "pass");
    assert_eq!(
        ops["post:/widgets"]["e2e"], "n/a",
        "nothing covered it at a layer that ran"
    );
    assert_eq!(ops["get:/widgets"]["unit"], "pass");
    // The core alone adds no target layer, so the key is absent and the
    // six core layers are the whole record.
    assert!(ev.get("target_evidence_layers").is_none(), "{}", ev);
}

#[test]
fn an_uncovered_operation_stays_n_a_and_a_not_run_live_layer_records_nothing_per_operation() {
    let (ws, scripts) =
        setup("echo 'live: PASS_TOKEN is not set — skipping live smoke (not_run)'; exit 3");
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 0, "not_run is not a failure");
    assert_eq!(ev["layers"]["live"]["status"], "not_run");
    assert!(ev["layers"]["live"].get("exclusions").is_none());
    for op in ["get:/widgets", "get:/widgets/{id}", "post:/widgets"] {
        assert_eq!(ev["operations"][op]["live"], "not_run", "{}", op);
    }

    // A live layer that ran and covered one operation leaves the others n/a —
    // the state lint then reports as live-unaccounted.
    let (ws, scripts) = setup(
        "echo 'PASS: list_widgets (read)'; echo 'live: 1 passed, 0 failed, 0 excluded'; exit 0",
    );
    let (_, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(ev["operations"]["get:/widgets"]["live"], "pass");
    assert_eq!(ev["operations"]["get:/widgets/{id}"]["live"], "n/a");
    let lint = graphos_factory_core::lint::lint_workspace(
        ws.path(),
        &graphos_factory_core::lint::LintOptions {
            schemas_dir: None,
            skip_evidence: false,
            target: &graphos_factory_core::target::BARE,
        },
    );
    let unaccounted: Vec<&str> = lint
        .findings
        .iter()
        .filter(|f| f.rule == "live-unaccounted")
        .map(|f| f.message.as_str())
        .collect();
    assert_eq!(unaccounted.len(), 2, "{:?}", unaccounted);
    assert!(unaccounted
        .iter()
        .any(|m| m.starts_with("get:/widgets/{id} ")));
    assert!(unaccounted.iter().any(|m| m.starts_with("post:/widgets ")));
}

#[test]
fn a_failing_live_case_fails_the_layer_and_the_run() {
    let (ws, scripts) = setup(
        "echo 'FAIL: list_widgets (read) — 1 error(s):'; echo 'live: 0 passed, 1 failed, 0 excluded'; exit 1",
    );
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 1);
    assert_eq!(ev["layers"]["live"]["status"], "fail");
    assert_eq!(ev["layers"]["live"]["failed"], 1);
    assert_eq!(ev["operations"]["get:/widgets"]["live"], "fail");
}

#[test]
fn missing_live_context_prevents_the_request_and_resumes_after_resolution() {
    let (ws, scripts) = setup("touch \"$1/live-attempted\"; echo 'PASS: list_widgets (read)'; echo 'live: 1 passed, 0 failed'; exit 0");
    let mut context = json!({
        "contract_version": 1, "intent": "Wrap documented widgets", "inputs": [],
        "requirements": [{"id":"identity", "phase":"live", "affects":["live queries"], "reason":"Effective identity is unknown", "resolve_with":"Verify the approved test identity", "status":"missing"}]
    });
    write(ws.path(), ".factory/context.yaml", &context.to_string());
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 0);
    assert_eq!(ev["layers"]["wiremock_e2e"]["status"], "pass");
    assert_eq!(ev["layers"]["live"]["status"], "not_run");
    assert!(ev["layers"]["live"]["reason"]
        .as_str()
        .unwrap()
        .contains("identity"));
    assert_eq!(ev["operations"]["get:/widgets"]["live"], "not_run");
    assert!(!ws.path().join("live-attempted").exists());
    assert!(validate(&ev, &schemas::load("evidence.schema.json", None).unwrap()).is_empty());

    context["requirements"][0]["status"] = json!("resolved");
    context["requirements"][0]["resolution"] =
        json!("The approved test identity is recorded in D-0001.");
    write(ws.path(), ".factory/context.yaml", &context.to_string());
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 0);
    assert_eq!(ev["layers"]["live"]["status"], "pass");
    assert!(ws.path().join("live-attempted").exists());
}

#[test]
fn invalid_context_fails_live_evidence_without_executing_the_wrapper() {
    let (ws, scripts) = setup("touch \"$1/live-attempted\"; exit 0");
    write(ws.path(), ".factory/context.yaml", "contract_version: 99");
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 1);
    assert_eq!(ev["layers"]["live"]["status"], "fail");
    assert!(!ws.path().join("live-attempted").exists());
}

#[test]
fn specialized_mode_without_a_context_file_fails_live_without_executing_the_wrapper() {
    // context_mode: specialized with no companion file is an invalid state.
    // The live gate is keyed on the assessment (the marker), not only the
    // file, so the wrapper must not run.
    let (ws, scripts) = setup("touch \"$1/live-attempted\"; exit 0");
    write(
        ws.path(),
        ".factory/workspace.yaml",
        &WORKSPACE.replace("context_mode: generic", "context_mode: specialized"),
    );
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 1);
    assert_eq!(ev["layers"]["live"]["status"], "fail");
    assert!(!ws.path().join("live-attempted").exists());
}

#[test]
fn an_unrecorded_mode_still_gates_live_on_declared_requirements() {
    // ADR 0081: an absent context_mode reads as generic, so the gate runs on
    // every workspace and a companion file's live gap holds the wrapper back.
    let (ws, scripts) = setup("touch \"$1/live-attempted\"; exit 0");
    write(
        ws.path(),
        ".factory/workspace.yaml",
        &WORKSPACE.replace("context_mode: generic\n", ""),
    );
    let context = json!({
        "contract_version": 1, "intent": "Wrap documented widgets", "inputs": [],
        "requirements": [{"id":"identity", "phase":"live", "affects":["live queries"], "reason":"Effective identity is unknown", "resolve_with":"Verify the approved test identity", "status":"missing"}]
    });
    write(ws.path(), ".factory/context.yaml", &context.to_string());
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 0);
    assert_eq!(ev["layers"]["live"]["status"], "not_run");
    assert!(!ws.path().join("live-attempted").exists());
}

/// Git in `dir`, isolated from the developer's global and system config (a
/// `commit.gpgsign` or `core.hooksPath` there would fail or slow the commit).
fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.test")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.test")
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {:?}", args);
}

/// The commit label is taken before the layers write .factory/evidence/, so a
/// clean tree is labelled clean — twice in a row, the second run seeing the
/// first run's uncommitted logs — and a modified tracked file outside the
/// evidence directory is `-dirty`.
#[test]
fn the_commit_label_is_clean_on_a_clean_tree_and_dirty_on_a_tracked_edit() {
    let (ws, scripts) = setup("echo 'live: skipped'; exit 0");
    git(ws.path(), &["init", "-q"]);
    git(ws.path(), &["add", "-A"]);
    git(ws.path(), &["commit", "-q", "-m", "init"]);

    let (_, first) = run(ws.path(), scripts.path(), "live");
    let label = first["commit"].as_str().unwrap().to_string();
    assert!(!label.ends_with("-dirty"), "clean tree labelled {}", label);
    let (_, second) = run(ws.path(), scripts.path(), "live");
    assert_eq!(
        second["commit"], label,
        "a previous run's logs made it dirty"
    );

    let schema = ws.path().join("widget-co.graphql");
    let text = std::fs::read_to_string(&schema).unwrap();
    std::fs::write(&schema, format!("{}\n# edited\n", text)).unwrap();
    let (_, edited) = run(ws.path(), scripts.path(), "live");
    assert_eq!(edited["commit"], format!("{}-dirty", label));
}

/// Lint's `link-untested` findings are named on evidence's own report, so a
/// run whose layers all pass still says which relationship fields have no
/// test (ADR 0094): evidence/latest.json has no row for a link field.
#[test]
fn evidence_names_every_relationship_field_lint_finds_untested() {
    let (ws, scripts) = setup("echo 'live: skipped'; exit 0");
    let schema = ws.path().join("widget-co.graphql");
    let sdl = std::fs::read_to_string(&schema).unwrap().replacen(
        "type Widget_Co_Widget { id: ID }",
        "type Widget_Co_Owner { id: ID }\n\ntype Widget_Co_Widget {\n  id: ID\n  ownerId: ID!\n  owner: Widget_Co_Owner @connect(source: \"widget_co\", http: { GET: \"/owners/{$this.ownerId}\" }, selection: \"id\")\n}",
        1,
    );
    assert!(sdl.contains("{$this.ownerId}"), "fixture drifted");
    std::fs::write(&schema, sdl).unwrap();
    let evidence = |ws: &Path| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
            .args(["evidence", ws.to_str().unwrap(), "--scripts"])
            .arg(scripts.path())
            .args(["--skip", "conformance,live"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let stdout = evidence(ws.path());
    assert!(
        stdout.contains("evidence: 1 relationship field(s) with no unit entry or no e2e case (lint link-untested): Widget_Co_Widget.owner (neither, not validated)\n"),
        "{}",
        stdout
    );
    // A suite with no entry for it: still neither.
    let root_unit = "tests:\n  - name: \"list\"\n    target: \"Query.widget_co_listWidgets\"\n";
    write(ws.path(), "tests/widget-co.connector.yaml", root_unit);
    let stdout = evidence(ws.path());
    assert!(
        stdout.contains("(lint link-untested): Widget_Co_Widget.owner (neither, not validated)\n"),
        "{}",
        stdout
    );
    // A case that selects the field leaves the unit entry.
    write(
        ws.path(),
        "tests/cases/widget_owner.graphql",
        "query { widget_co_widget(id: \"1\") { id owner { id } } }\n",
    );
    let stdout = evidence(ws.path());
    assert!(
        stdout.contains("(lint link-untested): Widget_Co_Widget.owner (no unit entry)\n"),
        "{}",
        stdout
    );
    // A unit entry and no case leaves the e2e case.
    std::fs::remove_file(ws.path().join("tests/cases/widget_owner.graphql")).unwrap();
    write(
        ws.path(),
        "tests/widget-co.connector.yaml",
        &format!(
            "{}  - name: \"owner\"\n    target: \"Widget_Co_Widget.owner\"\n",
            root_unit
        ),
    );
    let stdout = evidence(ws.path());
    assert!(
        stdout.contains("(lint link-untested): Widget_Co_Widget.owner (no e2e case)\n"),
        "{}",
        stdout
    );
    // Both clear it.
    write(
        ws.path(),
        "tests/cases/widget_owner.graphql",
        "query { widget_co_widget(id: \"1\") { id owner { id } } }\n",
    );
    let stdout = evidence(ws.path());
    assert!(!stdout.contains("relationship field"), "{}", stdout);
}

// ── The write-body-proof layer (ADR 0079 Step 2) ────────────────────────────

/// `write_body_proof` reads THIS run's own e2e status and log -- built
/// earlier in the same `evidence` invocation -- and its per-case verdicts
/// land on `wiremock_e2e`'s own entry, not a separate structure.
#[test]
fn the_write_body_proof_layer_runs_same_run_and_attaches_case_proofs_to_e2e() {
    let (ws, scripts) = setup("echo 'live: skipped'; exit 0");
    let d = ws.path();
    write(
        d,
        ".factory/inventory.json",
        r#"{"operations":[{"key":"post:/widgets","method":"POST","path":"/widgets"}],"shapes":{}}"#,
    );
    write(
        d,
        "tests/cases/create_widget.graphql",
        "mutation { widget_co_createWidget(name: \"Chair\") { id } }\n",
    );
    write(
        d,
        "tests/fixtures/mappings/create_widget.json",
        r#"{"request":{"method":"POST","urlPath":"/widgets","bodyPatterns":[{"equalToJson":{"name":"Chair"}}]},"response":{"status":200,"jsonBody":{"id":"w1"}}}"#,
    );
    // Extend the stubbed e2e.sh so its (canned) log also proves the new case.
    script(
        scripts.path(),
        "e2e.sh",
        "echo 'PASS: list_widgets'; echo 'PASS: widget'; echo 'PASS: create_widget'; echo 'e2e: 3 passed, 0 failed'; exit 0",
    );

    // conformance/lint fail independently in this deliberately minimal
    // fixture (no full inventory shapes); the overall run's exit code is not
    // under test here. `createWidget`'s only argument is optional, so
    // mutation-cases' required-only/omission/null obligations still fail
    // too -- this fixture proves the wiring (same-run status/log + case
    // attribution), not full coverage of every obligation.
    let (_, ev) = run(d, scripts.path(), "live");

    let wbp = &ev["layers"]["write_body_proof"];
    assert_ne!(wbp["status"], "not_run", "{:#?}", wbp);
    assert_eq!(wbp["cases"], 1, "{:#?}", wbp);

    let case_proofs = &ev["layers"]["wiremock_e2e"]["case_proofs"];
    assert_eq!(case_proofs["create_widget"]["verdict"], "pass", "{:#?}", ev);
    // list_widgets/widget are read-only: out of this layer's scope, so they
    // must not appear here even though they too passed at e2e.
    assert!(case_proofs.get("list_widgets").is_none(), "{:#?}", ev);

    validate_ok(&ev);
}

/// A case that never ran at all (no log line for it) reports `unproven`,
/// not silently absent and not folded into a pass.
#[test]
fn a_case_with_no_e2e_log_line_reports_unproven_not_a_silent_pass() {
    let (ws, scripts) = setup("echo 'live: skipped'; exit 0");
    let d = ws.path();
    write(
        d,
        ".factory/inventory.json",
        r#"{"operations":[{"key":"post:/widgets","method":"POST","path":"/widgets"}],"shapes":{}}"#,
    );
    write(
        d,
        "tests/cases/create_widget.graphql",
        "mutation { widget_co_createWidget(name: \"Chair\") { id } }\n",
    );
    write(
        d,
        "tests/fixtures/mappings/create_widget.json",
        r#"{"request":{"method":"POST","urlPath":"/widgets","bodyPatterns":[{"equalToJson":{"name":"Chair"}}]},"response":{"status":200,"jsonBody":{"id":"w1"}}}"#,
    );
    // e2e.sh's canned log is left as-is: no PASS/FAIL line names create_widget.

    let (_, ev) = run(d, scripts.path(), "live");
    let wbp = &ev["layers"]["write_body_proof"];
    assert_eq!(wbp["status"], "fail", "{:#?}", wbp);
    assert_eq!(wbp["failed"], 1, "{:#?}", wbp);
    // The summary counts writes and reads apart, and says so in the reason.
    assert!(wbp["write_gaps"].as_u64().unwrap() >= 1, "{:#?}", wbp);
    assert!(wbp["read_gaps"].as_u64().unwrap() >= 1, "{:#?}", wbp);
    let reason = wbp["reason"].as_str().unwrap();
    assert!(
        reason.contains("write gap") && reason.contains("read gap"),
        "{}",
        reason
    );

    let case_proofs = &ev["layers"]["wiremock_e2e"]["case_proofs"];
    assert_eq!(
        case_proofs["create_widget"]["verdict"], "unproven",
        "{:#?}",
        ev
    );

    validate_ok(&ev);
}

fn validate_ok(ev: &Value) {
    let schema = schemas::load("evidence.schema.json", None).unwrap();
    let errors = validate(ev, &schema);
    assert!(errors.is_empty(), "{:#?}", errors);
}

fn minimal_evidence(layers: Value) -> Value {
    json!({
        "contract_version": 2,
        "commit": "abc1234",
        "run_at": "2026-09-29T00:00:00Z",
        "toolchain": {"rover": "0.41.0", "federation": "2.15.2", "connect_spec": "v0.4"},
        "layers": layers,
        "operations": {}
    })
}

// ── Schema: the cases/case_proofs field collision (ADR 0079 Step 2) ────────

#[test]
fn the_schema_validates_case_proofs_alongside_the_untouched_integer_cases() {
    let ev = minimal_evidence(json!({
        "compose": {"status": "pass"},
        "connector_unit": {"status": "pass"},
        "wiremock_e2e": {
            "status": "pass",
            "cases": 2,
            "case_proofs": {
                "create_widget": {"verdict": "pass", "findings": []},
                "update_widget": {"verdict": "fail", "findings": ["the case is recorded as failed"]}
            }
        },
        "write_body_proof": {"status": "pass", "cases": 2, "failed": 0},
        "conformance": {"status": "pass"},
        "lint": {"status": "pass"},
        "live": {"status": "skipped", "reason": "not run"}
    }));
    validate_ok(&ev);
}

#[test]
fn the_zero_case_check_still_fires_with_no_case_proofs_present() {
    let ev = minimal_evidence(json!({
        "compose": {"status": "pass"},
        "connector_unit": {"status": "not_run", "reason": "no runnable cases", "cases": 0},
        "wiremock_e2e": {"status": "pass", "cases": 3},
        "conformance": {"status": "pass"},
        "lint": {"status": "pass"},
        "live": {"status": "skipped", "reason": "not run"}
    }));
    // Schema-valid with cases: 0 and no case_proofs key anywhere: the new
    // field is additive, never required for the old zero-case shape to
    // keep validating.
    validate_ok(&ev);
    assert_eq!(ev["layers"]["connector_unit"]["cases"], 0);
    assert!(ev["layers"]["connector_unit"]
        .as_object()
        .unwrap()
        .get("case_proofs")
        .is_none());
}

/// ADR 0106: lint's `link-null-untested` and `link-live-unaccounted`, and a
/// live `field:` exclusion, are named on evidence's report as a
/// relationship field not validated; the exclusion is kept as a live
/// finding and never becomes an operations row.
#[test]
fn evidence_names_a_relationship_field_with_no_null_parent_case_no_live_case_or_a_field_exclusion()
{
    let live_body = |excluded: bool| {
        format!(
            "echo 'live: Apollo Router 2.14.0 against the real API, 1 case(s)'; echo 'PASS: list_widgets (read)'; {}echo 'live: 1 passed, 0 failed, {} excluded'; exit 0",
            if excluded {
                "echo 'EXCLUDED FIELD: Widget_Co_Widget.owner — its only parent is a write'; "
            } else {
                ""
            },
            if excluded { 1 } else { 0 }
        )
    };
    let (ws, scripts) = setup(&live_body(false));
    let schema = ws.path().join("widget-co.graphql");
    let sdl = std::fs::read_to_string(&schema).unwrap().replacen(
        "type Widget_Co_Widget { id: ID }",
        "type Widget_Co_Owner { id: ID }\n\ntype Widget_Co_Widget {\n  id: ID\n  ownerId: ID\n  owner: Widget_Co_Owner @connect(source: \"widget_co\", http: { GET: \"/owners/{$this.ownerId}\" }, selection: \"id\")\n}",
        1,
    );
    assert!(sdl.contains("  ownerId: ID\n"), "fixture drifted");
    std::fs::write(&schema, sdl).unwrap();
    write(
        ws.path(),
        "tests/widget-co.connector.yaml",
        "tests:\n  - name: \"list\"\n    target: \"Query.widget_co_listWidgets\"\n  - name: \"owner\"\n    target: \"Widget_Co_Widget.owner\"\n",
    );
    write(
        ws.path(),
        "tests/cases/widget_owner.graphql",
        "query { widget_co_widget(id: \"1\") { id owner { id } } }\n",
    );
    write(
        ws.path(),
        "tests/live.yaml",
        "cases:\n  - name: list_widgets\n",
    );
    let evidence = |ws: &Path| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
            .args(["evidence", ws.to_str().unwrap(), "--scripts"])
            .arg(scripts.path())
            .args(["--skip", "conformance"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    // Unit and e2e, but no null-parent mapping and no live case.
    let stdout = evidence(ws.path());
    assert!(!stdout.contains("(lint link-untested)"), "{}", stdout);
    assert!(
        stdout.contains("evidence: 1 relationship field(s) not validated: Widget_Co_Widget.owner (no null-parent case; no live case or exclusion)\n"),
        "{}",
        stdout
    );
    // The null-parent mapping and a live `field:` exclusion: the field is
    // still named, with the exclusion's reason, and is no operation.
    write(
        ws.path(),
        "tests/fixtures/mappings/owner_empty_segment.json",
        "{\"request\": {\"method\": \"GET\", \"urlPath\": \"/owners/\"}, \"response\": {\"status\": 307}, \"metadata\": {\"x-cases\": [\"widget_owner\"]}}\n",
    );
    write(
        ws.path(),
        "tests/live.yaml",
        "cases:\n  - name: list_widgets\nexclusions:\n  - field: \"Widget_Co_Widget.owner\"\n    reason: its only parent is a write\n",
    );
    script(scripts.path(), "live.sh", &live_body(true));
    let stdout = evidence(ws.path());
    assert!(
        stdout.contains("evidence: 1 relationship field(s) not validated: Widget_Co_Widget.owner (excluded live: its only parent is a write)\n"),
        "{}",
        stdout
    );
    let ev = graphos_factory_core::json::parse(
        &std::fs::read_to_string(ws.path().join(".factory/evidence/latest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        validate(&ev, &schemas::load("evidence.schema.json", None).unwrap()),
        Vec::<String>::new()
    );
    let live = &ev["layers"]["live"];
    assert_eq!(live["status"], "pass");
    assert!(live.get("exclusions").is_none(), "{}", live);
    assert!(
        live["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "EXCLUDED FIELD: Widget_Co_Widget.owner — its only parent is a write"),
        "{}",
        live
    );
    assert!(ev["operations"].get("Widget_Co_Widget.owner").is_none());
    // A live case that selects the field, and no exclusion: quiet.
    write(
        ws.path(),
        "tests/live/widget_owner.graphql",
        "query { widget_co_widget(id: \"1\") { id owner { id } } }\n",
    );
    write(
        ws.path(),
        "tests/live.yaml",
        "cases:\n  - name: list_widgets\n  - name: widget_owner\n",
    );
    script(scripts.path(), "live.sh", &live_body(false));
    let stdout = evidence(ws.path());
    assert!(!stdout.contains("relationship field"), "{}", stdout);
}

#[test]
fn an_unproven_e2e_case_counts_as_a_case_and_leaves_its_operation_unchecked() {
    let (ws, scripts) = setup("echo 'live: 0 passed, 0 failed, 0 excluded'; exit 0");
    write(
        ws.path(),
        "tests/cases/widget_404.graphql",
        "query { widget_co_widget(id: \"404\") { id } }\n",
    );
    // The unproven case runs before the passing one: a later pass does not
    // clear it (ADR 0077).
    script(
        scripts.path(),
        "e2e.sh",
        "echo 'PASS: list_widgets'; echo 'UNPROVEN: widget_404 — its stub answered 404, and widget_co_widget documents 403,404'; echo 'PASS: widget'; echo 'e2e: 2 passed, 0 failed, 1 unproven'; exit 0",
    );
    let (code, ev) = run(ws.path(), scripts.path(), "conformance,lint");
    assert_eq!(code, 0);
    assert_eq!(
        validate(&ev, &schemas::load("evidence.schema.json", None).unwrap()),
        Vec::<String>::new()
    );
    let e2e = &ev["layers"]["wiremock_e2e"];
    assert_eq!(e2e["cases"], 3);
    assert_eq!(e2e["failed"], 0);
    assert_eq!(ev["operations"]["get:/widgets"]["e2e"], "pass");
    assert_eq!(ev["operations"]["get:/widgets/{id}"]["e2e"], "unchecked");
}

/// The widgets inventory with documented errors: `get:/widgets/{id}` answers
/// 403 or 404 (ADR 0077).
fn document_errors(ws: &Path) {
    write(
        ws,
        ".factory/inventory.json",
        &json!({"contract_version": 1, "operations": [
            {"key": "get:/widgets", "errors": []},
            {"key": "get:/widgets/{id}", "errors": [{"status": "403"}, {"status": "404"}]},
            {"key": "post:/widgets", "errors": []}
        ], "shapes": {}})
        .to_string(),
    );
}

#[test]
fn a_documented_status_no_case_served_is_not_run_and_the_operation_is_unchecked() {
    let (ws, scripts) = setup("echo 'live: 0 passed, 0 failed, 0 excluded'; exit 0");
    document_errors(ws.path());
    write(
        ws.path(),
        "tests/cases/widget_404.graphql",
        "query { widget_co_widget(id: \"404\") { id } }\n",
    );
    // The 404 case ran and passed; nothing ever served a 403.
    script(
        scripts.path(),
        "e2e.sh",
        "echo 'PASS: list_widgets'; echo 'PASS: widget_404'; echo 'SERVED: widget_404 get:/widgets/{id} 404'; echo 'PASS: widget'; echo 'e2e: 3 passed, 0 failed'; exit 0",
    );
    let (code, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, 0);
    assert_eq!(
        validate(&ev, &schemas::load("evidence.schema.json", None).unwrap()),
        Vec::<String>::new()
    );
    let e2e = &ev["layers"]["wiremock_e2e"];
    assert_eq!(e2e["status"], "pass");
    assert_eq!(
        e2e["error_coverage"],
        json!({"get:/widgets/{id}": {"documented": ["403", "404"], "executed": ["404"], "not_run": ["403"]}})
    );
    assert!(
        e2e["findings"].as_array().unwrap().iter().any(|f| f
            .as_str()
            .unwrap()
            .contains("error coverage incomplete")
            && f.as_str().unwrap().contains("get:/widgets/{id} (403)")),
        "{}",
        e2e["findings"]
    );
    // Green cases do not make the operation pass: it is unproven for the 403.
    assert_eq!(ev["operations"]["get:/widgets/{id}"]["e2e"], "unchecked");
    assert_eq!(ev["operations"]["get:/widgets"]["e2e"], "pass");
    assert!(ev["operations"]["get:/widgets/{id}"]["note"]
        .as_str()
        .unwrap()
        .contains("403"));
    // That the CI gate (.github/scripts/assert-evidence.sh, not in the
    // public tree) refuses it is pinned in the target suite beside it.
}

#[test]
fn every_documented_status_served_to_its_own_operation_leaves_it_passing() {
    let (ws, scripts) = setup("echo 'live: 0 passed, 0 failed, 0 excluded'; exit 0");
    document_errors(ws.path());
    script(
        scripts.path(),
        "e2e.sh",
        "echo 'PASS: list_widgets'; echo 'PASS: widget'; echo 'SERVED: widget get:/widgets/{id} 403'; echo 'SERVED: widget get:/widgets/{id} 404'; echo 'e2e: 2 passed, 0 failed'; exit 0",
    );
    let (_, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(
        ev["layers"]["wiremock_e2e"]["error_coverage"]["get:/widgets/{id}"]["not_run"],
        json!([])
    );
    assert_eq!(ev["operations"]["get:/widgets/{id}"]["e2e"], "pass");
}

#[test]
fn a_status_served_to_another_operation_does_not_cover_this_one() {
    let (ws, scripts) = setup("echo 'live: 0 passed, 0 failed, 0 excluded'; exit 0");
    document_errors(ws.path());
    // Both statuses were served, but to the list operation's request.
    script(
        scripts.path(),
        "e2e.sh",
        "echo 'PASS: list_widgets'; echo 'PASS: widget'; echo 'SERVED: list_widgets get:/widgets 403'; echo 'SERVED: list_widgets get:/widgets 404'; echo 'e2e: 2 passed, 0 failed'; exit 0",
    );
    let (_, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(
        ev["layers"]["wiremock_e2e"]["error_coverage"]["get:/widgets/{id}"]["not_run"],
        json!(["403", "404"])
    );
    assert_eq!(ev["operations"]["get:/widgets/{id}"]["e2e"], "unchecked");
}

#[test]
fn an_unproven_case_and_a_missing_status_together_stay_unchecked() {
    let (ws, scripts) = setup("echo 'live: 0 passed, 0 failed, 0 excluded'; exit 0");
    document_errors(ws.path());
    write(
        ws.path(),
        "tests/cases/widget_404.graphql",
        "query { widget_co_widget(id: \"404\") { id } }\n",
    );
    script(
        scripts.path(),
        "e2e.sh",
        "echo 'PASS: list_widgets'; echo 'UNPROVEN: widget_404 — its stub answered 404, and widget_co_widget documents 403,404'; echo 'PASS: widget'; echo 'e2e: 2 passed, 0 failed, 1 unproven'; exit 0",
    );
    let (_, ev) = run(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(ev["operations"]["get:/widgets/{id}"]["e2e"], "unchecked");
    assert_eq!(
        ev["layers"]["wiremock_e2e"]["error_coverage"]["get:/widgets/{id}"]["executed"],
        json!([])
    );
}

/// Review of #156: `--skip`/`--only` take the name latest.json prints as
/// well as the LAYERS name, and refuse a name that matches no layer instead
/// of silently selecting nothing.
#[test]
fn skip_and_only_take_either_layer_name_and_refuse_an_unknown_one() {
    let (ws, scripts) = setup("echo 'live: 0 passed, 0 failed, 0 excluded'; exit 0");
    let (code, doc) = run(
        ws.path(),
        scripts.path(),
        "write_body_proof,connector_unit,conformance,lint,json_accounting,live",
    );
    assert_eq!(code, 0);
    for key in ["write_body_proof", "connector_unit"] {
        assert_eq!(doc["layers"][key]["status"], "skipped", "{}", key);
    }
    assert_eq!(doc["layers"]["compose"]["status"], "pass");

    let before = std::fs::read_to_string(ws.path().join(".factory/evidence/latest.json")).unwrap();
    for flag in ["--skip", "--only"] {
        let argv = vec![
            ws.path().to_str().unwrap().to_string(),
            "--scripts".to_string(),
            scripts.path().to_str().unwrap().to_string(),
            flag.to_string(),
            "compose,sreialization".to_string(),
        ];
        assert_eq!(cmd::evidence::main(&argv), 1, "{}", flag);
    }
    let after = std::fs::read_to_string(ws.path().join(".factory/evidence/latest.json")).unwrap();
    assert_eq!(before, after, "a refused run writes nothing");
}

/// The report as the binary prints it: (exit code, stdout, latest.json).
fn report(ws: &Path, scripts: &Path, skip: &str) -> (Option<i32>, String, Value) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("evidence")
        .arg(ws)
        .arg("--scripts")
        .arg(scripts)
        .args(["--skip", skip])
        .env_remove("GRAPHOS_FACTORY_CORE_SCRIPTS")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let text = std::fs::read_to_string(ws.join(".factory/evidence/latest.json"))
        .unwrap_or_else(|e| panic!("{}: {}{}", e, stdout, String::from_utf8_lossy(&out.stderr)));
    (
        out.status.code(),
        stdout,
        graphos_factory_core::json::parse(&text).unwrap(),
    )
}

/// The count line, computed here from latest.json's own `operations`.
fn expected_operations_line(ev: &Value) -> String {
    let ops: Vec<&Value> = ev["operations"].as_object().unwrap().values().collect();
    let executed = ops
        .iter()
        .filter(|o| ["unit", "e2e", "live"].iter().any(|c| o[*c] == "pass"))
        .count();
    let unchecked = |c: &str| ops.iter().filter(|o| o[c] == "unchecked").count();
    format!(
        "operations: {} selected; {} with executed evidence (unit, e2e or live pass); {} unchecked at e2e (an unproven case, or a documented status with no case), {} unchecked at conformance",
        ops.len(),
        executed,
        unchecked("e2e"),
        unchecked("conformance")
    )
}

/// None of compose, unit and e2e ran: the report closes on why, and no
/// operation is credited with executed evidence.
#[test]
fn a_run_where_no_executed_layer_ran_says_not_validated_with_the_first_reason() {
    let (ws, scripts) = setup("echo 'live: TOKEN is not set (not_run)'; exit 3");
    for layer in ["compose", "unit", "e2e"] {
        script(
            scripts.path(),
            &format!("{}.sh", layer),
            &format!(
                "echo '{}: TOOL_LICENSE is not set (not_run)'; exit 3",
                layer
            ),
        );
    }
    let (_, stdout, ev) = report(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    for key in ["compose", "connector_unit", "wiremock_e2e"] {
        assert_eq!(ev["layers"][key]["status"], "not_run", "{}", key);
    }
    assert!(
        stdout.contains(&expected_operations_line(&ev)),
        "{}",
        stdout
    );
    let not_validated: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("not validated: "))
        .collect();
    assert_eq!(
        not_validated,
        vec![
            "not validated: no executed layer ran (compose: TOOL_LICENSE is not set (not_run))",
            "not validated: 3 selected operation(s) have no executed evidence: get:/widgets, get:/widgets/{id}, post:/widgets"
        ],
        "{}",
        stdout
    );
    assert_eq!(
        stdout.lines().last(),
        not_validated.last().copied(),
        "the report ends on it"
    );
}

/// One selected operation (post:/widgets) has no unit entry, no e2e case
/// and no live case: the report names it, and only it, even with every
/// layer that ran green.
#[test]
fn an_operation_with_no_executed_evidence_is_named_not_validated() {
    let (ws, scripts) = setup("echo 'live: TOKEN is not set (not_run)'; exit 3");
    let (code, stdout, ev) = report(
        ws.path(),
        scripts.path(),
        "conformance,lint,json_accounting",
    );
    assert_eq!(code, Some(0), "{}", stdout);
    assert_eq!(ev["layers"]["wiremock_e2e"]["status"], "pass");
    assert_eq!(ev["operations"]["post:/widgets"]["e2e"], "n/a");
    assert!(
        stdout.contains(&expected_operations_line(&ev)),
        "{}",
        stdout
    );
    assert!(
        stdout.contains("operations: 3 selected; 2 with executed evidence"),
        "{}",
        stdout
    );
    let not_validated: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("not validated: "))
        .collect();
    assert_eq!(
        not_validated,
        vec!["not validated: 1 selected operation(s) have no executed evidence: post:/widgets"],
        "{}",
        stdout
    );

    // Past ten, the line names ten and counts the rest.
    let mut many = ev.clone();
    let row = many["operations"]["post:/widgets"].clone();
    for i in 0..12 {
        many["operations"][format!("get:/more/{:02}", i)] = row.clone();
    }
    let lines = cmd::evidence::not_validated_lines(&many);
    assert_eq!(lines.len(), 1, "{:?}", lines);
    assert!(
        lines[0].starts_with("not validated: 13 selected operation(s) have no executed evidence: "),
        "{}",
        lines[0]
    );
    assert!(lines[0].ends_with(" and 3 more"), "{}", lines[0]);
    assert_eq!(lines[0].matches(", ").count(), 9, "{}", lines[0]);
}

/// A run of the live layer alone that passes: compose, unit and e2e are
/// `skipped` by the flag, but a layer did execute, so the report does not
/// say none ran; the operation live covered holds executed evidence.
#[test]
fn a_live_only_run_that_passes_is_not_no_executed_layer() {
    let (ws, scripts) = setup(
        "echo 'PASS: list_widgets (read)'; echo 'live: 1 passed, 0 failed, 0 excluded'; exit 0",
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("evidence")
        .arg(ws.path())
        .arg("--scripts")
        .arg(scripts.path())
        .args(["--only", "live"])
        .env_remove("GRAPHOS_FACTORY_CORE_SCRIPTS")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let ev: Value = graphos_factory_core::json::parse(
        &std::fs::read_to_string(ws.path().join(".factory/evidence/latest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ev["layers"]["live"]["status"], "pass", "{}", stdout);
    assert_eq!(ev["layers"]["compose"]["status"], "skipped");
    assert!(!stdout.contains("no executed layer ran"), "{}", stdout);
    assert!(
        stdout.contains(
            "not validated: 2 selected operation(s) have no executed evidence: get:/widgets/{id}, post:/widgets"
        ),
        "{}",
        stdout
    );
}

/// The public pilot's committed evidence, the run CI records: every
/// selected operation holds executed evidence, so the report has its count
/// line and no `not validated:` line.
#[test]
fn the_pilot_run_has_the_count_line_and_no_not_validated_line() {
    let latest = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pilots/graphos/gitea/.factory/evidence/latest.json");
    let ev = graphos_factory_core::json::parse(&std::fs::read_to_string(latest).unwrap()).unwrap();
    assert!(!ev["operations"].as_object().unwrap().is_empty());
    assert_eq!(
        cmd::evidence::operations_line(&ev),
        expected_operations_line(&ev)
    );
    assert_eq!(
        cmd::evidence::not_validated_lines(&ev),
        Vec::<String>::new()
    );
}

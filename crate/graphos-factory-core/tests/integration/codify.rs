use graphos_factory_core::cmd::codify::{expressed_finding, splice_overrides};
use graphos_factory_core::decisions;
use serde_json::json;
use std::path::Path;

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key", "@tag"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

type Widget_Co_Widget {
  id: ID
  name: String
}

type Query {
  # Hand-edited: the engineer wants the limit first.
  widget_co_listWidgets(limit: Int, offset: Int): [Widget_Co_Widget]
    @tag(name: "internal")
    @connect(source: "widget_co", http: { GET: "/widgets" }, selection: "$.widgets { id name }")
}
"#;

const SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  # the one operation\n  \"get:/widgets\":\n    include: true\n    graphql: { root: query, name: listWidgets }\n    pagination: { expose: [limit, offset] }\n";

const WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\n";

const DECISIONS: &str = r#"{"contract_version":1,"decisions":[
{"id":"D-0001","title":"Pilot scope","status":"resolved","date":"2026-09-08","context":"x.","resolution":{"decision":"y."}},
{"id":"D-0002","title":"Limit first","status":"resolved","date":"2026-09-08","context":"x.","resolution":{"decision":"y."}}
]}"#;

fn inventory() -> serde_json::Value {
    json!({"contract_version": 1, "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test"]},
        "operations": [{"key": "get:/widgets", "operation_id": "listWidgets", "method": "GET", "path": "/widgets", "semantics": "read", "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null,
            "response": {"shape_ref": "#/shapes/WidgetList", "root_property_count": 1, "array_root_properties": ["widgets"], "sole_root_property": "widgets"}, "errors": [], "support": "supported", "support_reason": null}],
        "shapes": {"Widget": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}},
                   "WidgetList": {"type": "object", "properties": {"widgets": {"type": "array", "items": {"$ref": "#/shapes/Widget"}}}}},
        "unresolved": []})
}

/// A workspace whose lock was written for `locked_sdl` while the schema on
/// disk is `sdl` — i.e. a hand edit has happened when they differ.
fn workspace(sdl: &str, locked_sdl: &str, selection: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w("widget-co.graphql", sdl);
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", selection);
    w(".factory/decisions.json", DECISIONS);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inventory()),
    );
    let spans =
        graphos_factory_core::spans::spans(locked_sdl, Some(&inventory()), &Default::default());
    let lock = graphos_factory_core::spans::lock_document("widget-co.graphql", &spans);
    w(
        ".factory/applied.lock.yaml",
        &graphos_factory_core::yaml::stringify(&lock, 0),
    );
    dir
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

fn decisions_doc(dir: &Path) -> serde_json::Value {
    graphos_factory_core::json::parse(&read(dir, ".factory/decisions.json")).unwrap()
}

fn decision_ids(dir: &Path) -> Vec<String> {
    decisions_doc(dir)
        .get("decisions")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|d| d.get("id").and_then(|x| x.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn has_decision(dir: &Path, id: &str) -> bool {
    decision_ids(dir).iter().any(|i| i == id)
}

fn codify(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec![dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::codify::main(&argv)
}

fn untagged() -> String {
    SDL.replace("    @tag(name: \"internal\")\n", "")
        .replace("  # Hand-edited: the engineer wants the limit first.\n", "")
}

#[test]
fn splice_adds_replaces_and_removes_the_overrides_block_without_touching_other_lines() {
    let one = json!([{"key": "get:/widgets", "reason": "r", "assert": [{"tag": "internal"}]}]);
    let added = splice_overrides(SELECTION, one.as_array().unwrap());
    assert!(
        added.starts_with(SELECTION),
        "everything before the block is untouched"
    );
    assert!(added.contains("\n# Hand edits codified by `graphos-factory-core codify`"));
    assert!(added.ends_with(
        "overrides:\n  - key: \"get:/widgets\"\n    reason: r\n    assert:\n      - tag: internal\n"
    ));
    let parsed = graphos_factory_core::yaml::parse(&added).unwrap();
    assert_eq!(parsed["overrides"], one);

    let with_tail = format!(
        "{}\n# trailing comment about something else\nother_key: 1\n",
        added
    );
    let two = json!([{"key": "get:/widgets", "reason": "r2"}, {"key": "type:Widget_Co_Widget", "reason": "t"}]);
    let replaced = splice_overrides(&with_tail, two.as_array().unwrap());
    assert!(
        replaced.contains("# the one operation"),
        "comments elsewhere survive"
    );
    assert!(replaced.contains("\n# trailing comment about something else\nother_key: 1\n"));
    assert_eq!(
        graphos_factory_core::yaml::parse(&replaced).unwrap()["overrides"],
        two
    );
    assert_eq!(replaced.matches("overrides:").count(), 1);

    let removed = splice_overrides(&replaced, &[]);
    assert!(!removed.contains("overrides:"));
    assert!(removed.contains("other_key: 1"));
    assert_eq!(splice_overrides(SELECTION, &[]), SELECTION);
}

#[test]
fn decision_numbers_continue_from_the_highest_existing_entry() {
    let seed = graphos_factory_core::json::parse(DECISIONS).unwrap();
    assert_eq!(decisions::next_id(&seed), "D-0003");
    assert_eq!(decisions::next_id(&decisions::empty()), "D-0001");
    let two = json!({"contract_version": 1, "decisions": [
        {"id": "D-0019", "title": "x", "status": "resolved", "date": "d", "resolution": {"decision": "y"}},
        {"id": "D-0007", "title": "y", "status": "resolved", "date": "d", "resolution": {"decision": "y"}}
    ]});
    assert_eq!(decisions::next_id(&two), "D-0020");

    // An --expressed codification with --context leaves no override, so
    // the context becomes a finding (ADR 0113 §3).
    let f = expressed_finding(
        "get:/widgets",
        "Ctx.",
        "Tagged internal.",
        Some("D-0002"),
        "widget-co.graphql",
    );
    assert_eq!(f.source, "codify");
    assert!(f.body.starts_with("Ctx.\n\nTagged internal. selection.yaml now expresses the edit to get:/widgets in widget-co.graphql, so there is no override"), "{}", f.body);
    assert_eq!(f.affects, vec!["get:/widgets".to_string()]);
    assert_eq!(f.related, vec!["D-0002".to_string()]);
}

#[test]
fn codify_writes_the_override_the_decision_and_refreshes_the_lock() {
    let dir = workspace(SDL, &untagged(), SELECTION);
    let mut applied =
        graphos_factory_core::yaml::parse(&read(dir.path(), ".factory/applied.lock.yaml")).unwrap();
    graphos_factory_core::provenance::refresh(dir.path(), &mut applied, None, None).unwrap();
    std::fs::write(
        dir.path().join(".factory/applied.lock.yaml"),
        graphos_factory_core::yaml::stringify(&applied, 0),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/context.yaml"),
        format!(
            "evidence:\n  - path: {}\n    sha256: recorded\n",
            dir.path().join("widget-co.graphql").display()
        ),
    )
    .unwrap();
    // Before: the span is a hand edit.
    let before = graphos_factory_core::reconcile::reconcile_workspace(dir.path(), None).unwrap();
    assert_eq!(before["lock"]["hand_edits"][0]["key"], "get:/widgets");

    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/widgets",
            "--reason",
            "User data is personal; the engineer tags the list internal and wants limit first.",
            "--assert",
            "tag=internal",
            "--assert",
            "contains=# Hand-edited: the engineer wants the limit first.",
            "--until",
            "a internal policy lands in selection defaults",
            "--context",
            "Reviewers read pagination arguments in reading order.",
        ],
    );
    assert_eq!(code, 0);

    // The entry carries the why (reason and context) and no decision: codify
    // writes no record (ADR 0113 §3). An entry with `context` makes the
    // selection version 2.
    let selection = read(dir.path(), ".factory/selection.yaml");
    assert!(selection.starts_with(&SELECTION.replace("contract_version: 1", "contract_version: 2")));
    let parsed = graphos_factory_core::yaml::parse(&selection).unwrap();
    assert_eq!(
        parsed["overrides"],
        json!([{
            "key": "get:/widgets",
            "reason": "User data is personal; the engineer tags the list internal and wants limit first.",
            "context": "Reviewers read pagination arguments in reading order.",
            "assert": [{"tag": "internal"}, {"contains": "# Hand-edited: the engineer wants the limit first."}],
            "until": "a internal policy lands in selection defaults"
        }])
    );
    assert_eq!(decision_ids(dir.path()), vec!["D-0001", "D-0002"]);
    assert_eq!(read(dir.path(), ".factory/decisions.json"), DECISIONS);
    assert!(!dir.path().join(".factory/findings.json").exists());
    assert!(!dir.path().join(".factory/findings").exists());

    // After: no hand edit, the override holds, reconcile is clean apart from
    // the drift the engineer created on purpose.
    let after = graphos_factory_core::reconcile::reconcile_workspace(dir.path(), None).unwrap();
    assert_eq!(after["lock"]["hand_edits"], json!([]));
    assert_eq!(after["overrides"][0]["failed"], 0);
    assert_eq!(after["overrides"][0]["decision"], serde_json::Value::Null);
    assert_eq!(
        after["overrides"][0]["context"],
        "Reviewers read pagination arguments in reading order."
    );
    assert!(after["overrides"][0]["drift"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.as_str().unwrap().contains("@tag(name: \"internal\")")));
    assert_eq!(after["clean"], true);
    let lock =
        graphos_factory_core::yaml::parse(&read(dir.path(), ".factory/applied.lock.yaml")).unwrap();
    assert!(lock["written_by"]
        .as_str()
        .unwrap()
        .starts_with("graphos-factory-core "));
    assert!(
        lock.get("provenance").is_none(),
        "failed collection must remove stale provenance"
    );

    // Codifying the same key again replaces the entry and reuses a named decision.
    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/widgets",
            "--reason",
            "Same intent, one assertion.",
            "--assert",
            "arg=limit",
            "--decision",
            "D-0002",
        ],
    );
    assert_eq!(code, 0);
    let parsed =
        graphos_factory_core::yaml::parse(&read(dir.path(), ".factory/selection.yaml")).unwrap();
    assert_eq!(parsed["overrides"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["overrides"][0]["decision"], "D-0002");
    assert_eq!(parsed["overrides"][0]["assert"], json!([{"arg": "limit"}]));
    assert_eq!(decision_ids(dir.path()), vec!["D-0001", "D-0002"]);
}

#[test]
fn codify_refuses_an_intent_the_text_does_not_satisfy_and_a_missing_span() {
    let dir = workspace(SDL, &untagged(), SELECTION);
    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/widgets",
            "--reason",
            "r",
            "--assert",
            "tag=beta",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(
        read(dir.path(), ".factory/selection.yaml"),
        SELECTION,
        "nothing written"
    );
    assert_eq!(
        codify(
            dir.path(),
            &["--key", "type:Nope", "--reason", "r", "--pin"]
        ),
        2
    );
    assert_eq!(
        codify(dir.path(), &["--key", "get:/widgets", "--reason", "r"]),
        1,
        "needs --assert, --pin or --expressed"
    );
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--assert",
                "weird=x"
            ]
        ),
        1
    );
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--expressed",
                "--pin"
            ]
        ),
        1
    );
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--pin",
                "--decision",
                "19"
            ]
        ),
        1
    );
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--pin",
                "--expires",
                "soon"
            ]
        ),
        1
    );
}

#[test]
fn codify_expressed_needs_the_selection_to_agree_and_drops_a_previous_override() {
    let dir = workspace(SDL, &untagged(), SELECTION);
    // The selection does not list the internal tag yet: refused.
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "Tagged internal in the selection.",
                "--expressed"
            ]
        ),
        2
    );
    // Register an override first, then move the intent into the selection.
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--assert",
                "tag=internal"
            ]
        ),
        0
    );
    let with_tag = read(dir.path(), ".factory/selection.yaml").replace(
        "    pagination: { expose: [limit, offset] }\n",
        "    pagination: { expose: [limit, offset] }\n    tags: [internal]\n",
    );
    std::fs::write(dir.path().join(".factory/selection.yaml"), with_tag).unwrap();
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "The internal tag is now in selection.tags.",
                "--expressed",
                "--context",
                "Security asked for the internal tag on every user-data list.",
                "--decision",
                "D-0002"
            ]
        ),
        0
    );
    let parsed =
        graphos_factory_core::yaml::parse(&read(dir.path(), ".factory/selection.yaml")).unwrap();
    assert!(
        parsed.get("overrides").is_none(),
        "the block is removed when empty: {:?}",
        parsed
    );
    assert_eq!(
        parsed["operations"]["get:/widgets"]["tags"],
        json!(["internal"])
    );
    // No override carries the context, so it is a finding (ADR 0113 §3);
    // no decision is written.
    assert_eq!(decision_ids(dir.path()), vec!["D-0001", "D-0002"]);
    // A new finding: a random id, in its own file (ADR 0118).
    let findings = graphos_factory_core::findings::load(dir.path(), None).unwrap();
    let f = &findings["findings"][0];
    assert!(graphos_factory_core::record_log::is_random(
        f["id"].as_str().unwrap()
    ));
    assert!(!dir.path().join(".factory/findings.json").exists());
    assert_eq!(f["source"], "codify");
    assert_eq!(f["status"], "current");
    assert_eq!(f["affects"], json!(["get:/widgets"]));
    assert_eq!(f["related"], json!(["D-0002"]));
    let body = f["body"].as_str().unwrap();
    assert!(body.starts_with("Security asked for the internal tag on every user-data list."));
    assert!(body.contains("selection.yaml now expresses the edit"));
    let report = graphos_factory_core::reconcile::reconcile_workspace(dir.path(), None).unwrap();
    assert_eq!(report["clean"], true);
    assert_eq!(report["overrides"], json!([]));
    assert_eq!(report["lock"]["hand_edits"], json!([]));
}

#[test]
fn codify_needs_a_lock_and_dry_run_writes_nothing() {
    let dir = workspace(SDL, &untagged(), SELECTION);
    let lock_path = dir.path().join(".factory/applied.lock.yaml");
    let lock_text = std::fs::read_to_string(&lock_path).unwrap();
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--assert",
                "tag=internal",
                "--dry-run"
            ]
        ),
        0
    );
    assert_eq!(read(dir.path(), ".factory/selection.yaml"), SELECTION);
    assert_eq!(read(dir.path(), ".factory/decisions.json"), DECISIONS);
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), lock_text);
    std::fs::remove_file(&lock_path).unwrap();
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--assert",
                "tag=internal"
            ]
        ),
        2
    );
}

// ── --waive ────────────────────────────────────────────────────────────────

/// The codify workspace plus a 404 fixture for a status the spec does not
/// document: validate reports it `unchecked`.
fn workspace_with_gap() -> tempfile::TempDir {
    let dir = workspace(SDL, SDL, SELECTION);
    let f = dir
        .path()
        .join("tests/fixtures/mappings/widget_missing.json");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(
        &f,
        json!({"request": {"method": "GET", "urlPath": "/widgets"}, "response": {"status": 404, "jsonBody": {"message": "gone"}}}).to_string(),
    )
    .unwrap();
    dir
}

#[test]
fn waive_records_the_gap_in_selection_and_validate_reports_it_waived() {
    let dir = workspace_with_gap();
    let before = graphos_factory_core::cmd::validate::validate_workspace(dir.path()).unwrap();
    assert_eq!(before.json["counts"]["unchecked"], 1);
    let code = codify(
        dir.path(),
        &[
            "--waive",
            "tests/fixtures/mappings/widget_missing.json",
            "--status",
            "unchecked",
            "--reason",
            "Widget Co documents no 404 body; the fixture carries the one observed live.",
            "--until",
            "the vendor documents it",
        ],
    );
    assert_eq!(code, 0);
    let selection = read(dir.path(), ".factory/selection.yaml");
    assert!(
        selection.starts_with(SELECTION),
        "the rest of the file is untouched:\n{}",
        selection
    );
    assert!(selection.contains("waivers:\n  - where: tests/fixtures/mappings/widget_missing.json\n    status: unchecked\n"), "{}", selection);
    // The waiver carries its reason and no decision; codify writes no
    // record (ADR 0113 §3).
    assert!(
        selection.contains("observed live.\"\n    until: \"the vendor documents it\"\n"),
        "{}",
        selection
    );
    assert!(!selection.contains("decision:"), "{}", selection);
    assert_eq!(read(dir.path(), ".factory/decisions.json"), DECISIONS);
    let after = graphos_factory_core::cmd::validate::validate_workspace(dir.path()).unwrap();
    assert_eq!(after.json["counts"]["unchecked"], 0);
    assert_eq!(after.json["counts"]["waived"], 1);
    assert_eq!(after.json["waivers"]["unused"].as_array().unwrap().len(), 0);
    // Waiving the same target again replaces the entry rather than adding a second one.
    let code = codify(
        dir.path(),
        &[
            "--waive",
            "tests/fixtures/mappings/widget_missing.json",
            "--status",
            "unchecked",
            "--reason",
            "Reworded.",
            "--decision",
            "D-0002",
            "--context",
            "The vendor's support ticket 4411 confirms the body.",
        ],
    );
    assert_eq!(code, 0);
    let selection = read(dir.path(), ".factory/selection.yaml");
    assert_eq!(
        selection.matches("widget_missing.json").count(),
        1,
        "{}",
        selection
    );
    assert!(selection.contains("reason: Reworded."));
    assert!(selection.contains("    decision: D-0002\n    context: \"The vendor's support ticket 4411 confirms the body.\"\n"), "{}", selection);
    assert!(
        selection.starts_with("contract_version: 2\n"),
        "{}",
        selection
    );
    assert_eq!(decision_ids(dir.path()), vec!["D-0001", "D-0002"]);
    let after = graphos_factory_core::cmd::validate::validate_workspace(dir.path()).unwrap();
    assert_eq!(after.json["counts"]["waived"], 1);
}

#[test]
fn waive_refuses_a_gap_validate_does_not_report() {
    let dir = workspace_with_gap();
    // The right body, the wrong status.
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--waive",
                "tests/fixtures/mappings/widget_missing.json",
                "--status",
                "unmatched",
                "--reason",
                "x"
            ]
        ),
        2
    );
    // An operation with no such body.
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--waive",
                "get:/widgets",
                "--status",
                "unmatched",
                "--reason",
                "x"
            ]
        ),
        2
    );
    // An operation the inventory does not have.
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--waive",
                "get:/gadgets",
                "--status",
                "unchecked",
                "--reason",
                "x"
            ]
        ),
        2
    );
    // Usage errors.
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--waive",
                "tests/fixtures/mappings/widget_missing.json",
                "--reason",
                "x"
            ]
        ),
        1
    );
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--waive",
                "tests/fixtures/mappings/widget_missing.json",
                "--status",
                "pass",
                "--reason",
                "x"
            ]
        ),
        1
    );
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--waive",
                "tests/fixtures/mappings/widget_missing.json",
                "--status",
                "unchecked"
            ]
        ),
        1
    );
    assert!(
        !read(dir.path(), ".factory/selection.yaml").contains("waivers"),
        "nothing was written"
    );
}

#[test]
fn waive_by_operation_and_dry_run() {
    let dir = workspace_with_gap();
    let code = codify(
        dir.path(),
        &[
            "--waive",
            "get:/widgets",
            "--status",
            "unchecked",
            "--reason",
            "Every undocumented error body of the list.",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0);
    assert!(
        !read(dir.path(), ".factory/selection.yaml").contains("waivers"),
        "dry run writes nothing"
    );
    let code = codify(
        dir.path(),
        &[
            "--waive",
            "get:/widgets",
            "--status",
            "unchecked",
            "--reason",
            "Every undocumented error body of the list.",
        ],
    );
    assert_eq!(code, 0);
    let selection = read(dir.path(), ".factory/selection.yaml");
    assert!(
        selection.contains("  - operation: \"get:/widgets\"\n    status: unchecked\n"),
        "{}",
        selection
    );
    let after = graphos_factory_core::cmd::validate::validate_workspace(dir.path()).unwrap();
    assert_eq!(after.json["counts"]["waived"], 1);
    assert_eq!(
        after.json["operations"]["get:/widgets"], "waived",
        "the 404 is the operation's only body here, so per operation it reads waived"
    );
}

/// `--key` codifies a *hand edit*. A span that still hashes to what the lock
/// recorded is not one, and the decision codify would append ("the engineer
/// edited … by hand; reconcile reported it") would be a false record —
/// which is how a correction to an inference got written up as an edit
/// before ADR 0018.
#[test]
fn codify_key_refuses_a_span_that_is_in_sync_with_the_lock() {
    let dir = workspace(SDL, SDL, SELECTION);
    let d = dir.path();
    let before = read(d, ".factory/selection.yaml");
    assert_eq!(
        codify(
            d,
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "r",
                "--assert",
                "tag=internal"
            ]
        ),
        2
    );
    assert_eq!(
        read(d, ".factory/selection.yaml"),
        before,
        "nothing is written when codify refuses"
    );
    assert!(!has_decision(d, "D-0003"));

    // The one exception: an override already exists for the key, so the
    // engineer is amending its reason or its assertions, not recording a
    // new edit.
    let with_override = graphos_factory_core::cmd::codify::splice_overrides(
        SELECTION,
        json!([{"key": "get:/widgets", "reason": "old", "assert": [{"tag": "internal"}]}])
            .as_array()
            .unwrap(),
    );
    let dir = workspace(SDL, SDL, &with_override);
    assert_eq!(
        codify(
            dir.path(),
            &[
                "--key",
                "get:/widgets",
                "--reason",
                "reworded",
                "--assert",
                "tag=internal"
            ]
        ),
        0
    );
    assert!(read(dir.path(), ".factory/selection.yaml").contains("reason: reworded"));
}

/// A `contract_version: 1` line carrying a comment is still upgraded when
/// codify writes a `context` (ADR 0113, review): the file stays valid.
#[test]
fn codify_context_upgrades_a_commented_version_line() {
    let commented = SELECTION.replace(
        "contract_version: 1\n",
        "contract_version: 1   # the selection contract\n",
    );
    let dir = workspace(SDL, &untagged(), &commented);
    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/widgets",
            "--reason",
            "internal",
            "--assert",
            "tag=internal",
            "--context",
            "Security asked for it.",
        ],
    );
    assert_eq!(code, 0);
    let selection = read(dir.path(), ".factory/selection.yaml");
    assert!(
        selection.starts_with("contract_version: 2   # the selection contract\n"),
        "{}",
        selection
    );
    let parsed = graphos_factory_core::yaml::parse(&selection).unwrap();
    let schema = graphos_factory_core::schemas::load("selection.schema.json", None).unwrap();
    assert_eq!(
        graphos_factory_core::jsonschema::validate(&parsed, &schema),
        Vec::<String>::new()
    );
}

/// codify holds the spliced selection to its schema before writing: a
/// file the schema would reject is refused, exit 2, nothing written.
#[test]
fn codify_refuses_a_splice_the_selection_schema_rejects() {
    let invalid = format!("{}not_a_selection_key: 1\n", SELECTION);
    let dir = workspace(SDL, &untagged(), &invalid);
    let lock = read(dir.path(), ".factory/applied.lock.yaml");
    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/widgets",
            "--reason",
            "internal",
            "--assert",
            "tag=internal",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(read(dir.path(), ".factory/selection.yaml"), invalid);
    assert_eq!(read(dir.path(), ".factory/applied.lock.yaml"), lock);
}

// ── a random decision id (ADR 0118) ────────────────────────────────────────

/// `decisions add` beside the numbered `decisions.json`: the new record's
/// random id, found by its title.
fn add_random_decision(dir: &Path, title: &str) -> String {
    let argv: Vec<String> = [
        "add",
        &dir.to_string_lossy(),
        "--title",
        title,
        "--question",
        "Accept it?",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(graphos_factory_core::cmd::decisions::main(&argv), 0);
    let doc = decisions::load(dir, None).unwrap();
    let id = doc["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == title)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(graphos_factory_core::record_log::is_random(&id), "{}", id);
    id
}

/// A decision added since ADR 0118 is cited exactly as a numbered one is:
/// `--decision` on a waiver and on an override takes the random id, the
/// selection schema accepts what codify wrote, and lint reads it back with
/// no contract finding.
#[test]
fn codify_cites_a_random_decision_id_on_a_waiver_and_an_override() {
    let dir = workspace_with_gap();
    let id = add_random_decision(dir.path(), "Accept the undocumented 404");
    let code = codify(
        dir.path(),
        &[
            "--waive",
            "tests/fixtures/mappings/widget_missing.json",
            "--status",
            "unchecked",
            "--reason",
            "Widget Co documents no 404 body.",
            "--decision",
            &id,
        ],
    );
    assert_eq!(code, 0);
    let parsed =
        graphos_factory_core::yaml::parse(&read(dir.path(), ".factory/selection.yaml")).unwrap();
    assert_eq!(parsed["waivers"][0]["decision"], id.as_str());

    let dir = workspace(SDL, &untagged(), SELECTION);
    let id = add_random_decision(dir.path(), "Tag the list internal");
    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/widgets",
            "--reason",
            "User data is personal.",
            "--assert",
            "tag=internal",
            "--decision",
            &id,
        ],
    );
    assert_eq!(code, 0);
    let parsed =
        graphos_factory_core::yaml::parse(&read(dir.path(), ".factory/selection.yaml")).unwrap();
    assert_eq!(parsed["overrides"][0]["decision"], id.as_str());
    let lint = graphos_factory_core::lint::lint_workspace(
        dir.path(),
        &graphos_factory_core::lint::LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    );
    let contract: Vec<_> = lint
        .findings
        .iter()
        .filter(|f| f.rule == "contract" && f.message.starts_with(".factory/selection.yaml"))
        .map(|f| f.message.clone())
        .collect();
    assert!(contract.is_empty(), "{:?}", contract);
    // A malformed id is still a usage error.
    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/widgets",
            "--reason",
            "x.",
            "--assert",
            "tag=internal",
            "--decision",
            "D-series1",
        ],
    );
    assert_eq!(code, 1);
}

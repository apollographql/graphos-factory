//! `selection draft` and the lock's watch over inventory.json (ADR 0018).

use serde_json::{json, Value};
use std::path::Path;

const WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\n";

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key", "@tag"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

type Widget_Co_Widget {
  id: ID
}

type Query {
  widget_co_listWidgets: [Widget_Co_Widget]
    @connect(source: "widget_co", http: { GET: "/widgets" }, selection: "$.widgets { id }")
}
"#;

/// Four operations: a list wrapper, a resource that embeds a list, one with
/// no response body at all, and the by-id operation `Widget.owner_id` links
/// to (ADR 0069).
fn inventory() -> Value {
    json!({
        "contract_version": 1,
        "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test"]},
        "operations": [
            {"key": "get:/widgets", "operation_id": "listWidgets", "method": "GET", "path": "/widgets",
             "semantics": "read", "provenance": "spec", "confidence": 1,
             "response": {"status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetList",
                          "root_property_count": 2, "array_root_properties": ["widgets"], "total_items_property": "total"},
             "support": "supported", "support_reason": null},
            {"key": "get:/widgets/{id}", "operation_id": "getWidget", "method": "GET", "path": "/widgets/{id}",
             "semantics": "read", "provenance": "spec", "confidence": 1,
             "response": {"status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Widget",
                          "root_property_count": 5, "array_root_properties": ["parts"]},
             "support": "supported", "support_reason": null},
            {"key": "delete:/widgets/{id}", "operation_id": "deleteWidget", "method": "DELETE", "path": "/widgets/{id}",
             "semantics": "write", "provenance": "spec", "confidence": 1,
             "response": {"status": "204", "content_type": null},
             "support": "supported", "support_reason": null},
            {"key": "get:/owners/{ownerId}", "operation_id": "getOwner", "method": "GET", "path": "/owners/{ownerId}",
             "semantics": "read", "provenance": "spec", "confidence": 1,
             "parameters": [{"name": "ownerId", "in": "path", "required": true, "type": "string"}],
             "response": {"status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Owner",
                          "root_property_count": 2},
             "support": "supported", "support_reason": null}
        ],
        "shapes": {
            "Widget": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"},
                        "colour": {"type": "string"}, "owner_id": {"type": "string"},
                        "parts": {"type": "array", "items": {"type": "string"}}}},
            "WidgetList": {"type": "object", "properties": {
                "widgets": {"type": "array", "items": {"$ref": "#/shapes/Widget"}}, "total": {"type": "integer"}}},
            "Owner": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}}
        },
        "unresolved": []
    })
}

const SELECTION: &str = r#"contract_version: 1
defaults:
  fields: all
operations:
  # the list, with a comment the splice must not disturb
  "get:/widgets":
    include: true
    graphql: { root: query, name: listWidgets }

  "get:/widgets/{id}":
    include: true
    graphql:
      root: query
      name: widget
  "delete:/widgets/{id}":
    include: false
    reason: "read-only first release"
"#;

fn workspace(selection: &str) -> tempfile::TempDir {
    workspace_with(selection, &inventory())
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

fn lint_rules(dir: &Path) -> Vec<String> {
    graphos_factory_core::lint::lint_workspace(
        dir,
        &graphos_factory_core::lint::LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings
    .into_iter()
    .map(|f| f.rule)
    .collect()
}

fn draft(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec!["draft".to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::selection::main(&argv)
}

#[test]
fn draft_writes_one_unconfirmed_block_per_included_operation_and_leaves_the_rest_alone() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    assert_eq!(draft(d, &[]), 0);
    let after = read(d, ".factory/selection.yaml");
    assert!(
        after.contains("  # the list, with a comment the splice must not disturb\n"),
        "comments survive: {}",
        after
    );
    let parsed = graphos_factory_core::yaml::parse(&after).unwrap();
    // The list wrapper earns its envelope; the resource that embeds a list
    // does not; the excluded operation is not touched at all.
    assert_eq!(
        parsed["operations"]["get:/widgets"]["response"],
        json!({"envelope": "widgets", "confirmed": false})
    );
    assert_eq!(
        parsed["operations"]["get:/widgets/{id}"]["response"],
        json!({"envelope": null, "confirmed": false})
    );
    assert!(parsed["operations"]["delete:/widgets/{id}"]
        .get("response")
        .is_none());
    // Everything else about each entry is as it was.
    assert_eq!(
        parsed["operations"]["get:/widgets"]["graphql"],
        json!({"root": "query", "name": "listWidgets"})
    );
    assert_eq!(
        parsed["operations"]["delete:/widgets/{id}"]["reason"],
        "read-only first release"
    );

    // A second run has nothing to do and says so.
    assert_eq!(draft(d, &[]), 2);
    assert_eq!(read(d, ".factory/selection.yaml"), after);
}

#[test]
fn draft_finds_an_unquoted_operation_key_that_yaml_loads_identically() {
    // `get:/widgets:` loads as the key `get:/widgets` exactly as the quoted
    // form does; the splice must find it too, not split it at the first colon.
    let unquoted = SELECTION
        .replace("\"get:/widgets\":", "get:/widgets:")
        .replace(
            "\"get:/widgets/{id}\":",
            "get:/widgets/{id}:  # a trailing comment",
        );
    let dir = workspace(&unquoted);
    let d = dir.path();
    assert_eq!(draft(d, &[]), 0);
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(
        parsed["operations"]["get:/widgets"]["response"],
        json!({"envelope": "widgets", "confirmed": false})
    );
    assert_eq!(
        parsed["operations"]["get:/widgets/{id}"]["response"],
        json!({"envelope": null, "confirmed": false})
    );
    assert_eq!(draft(d, &[]), 2);
}

#[test]
fn a_drafted_envelope_never_governs_until_it_is_confirmed() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    assert_eq!(draft(d, &[]), 0);
    // reconcile names every unconfirmed block; lint warns on it.
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let notes: Vec<&str> = report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["message"].as_str())
        .collect();
    assert_eq!(
        notes
            .iter()
            .filter(|m| m.contains("still the tool's draft"))
            .count(),
        2,
        "{:?}",
        notes
    );
    let rules = lint_rules(d);
    assert_eq!(
        rules
            .iter()
            .filter(|r| *r == "response-envelope-unconfirmed")
            .count(),
        2,
        "{:?}",
        rules
    );
}

#[test]
fn force_redraws_a_block_and_dry_run_writes_nothing() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    assert_eq!(draft(d, &[]), 0);
    // Confirm one by hand, as the agent would after asking the user.
    let text = read(d, ".factory/selection.yaml").replace(
        "      envelope: \"widgets\"\n      confirmed: false",
        "      envelope: null\n      confirmed: true",
    );
    std::fs::write(d.join(".factory/selection.yaml"), &text).unwrap();

    // --dry-run reports what --force would do and writes nothing.
    assert_eq!(
        draft(d, &["--op", "get:/widgets", "--force", "--dry-run"]),
        0
    );
    assert_eq!(read(d, ".factory/selection.yaml"), text);

    assert_eq!(draft(d, &["--op", "get:/widgets", "--force"]), 0);
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(
        parsed["operations"]["get:/widgets"]["response"],
        json!({"envelope": "widgets", "confirmed": false}),
        "the confirmed answer is replaced by a fresh draft"
    );
    // The other operation is untouched by a targeted --force.
    assert_eq!(
        parsed["operations"]["get:/widgets/{id}"]["response"],
        json!({"envelope": null, "confirmed": false})
    );

    // An --op the selection does not list is an error, not a silent no-op.
    assert_eq!(draft(d, &["--op", "get:/nope"]), 1);
}

#[test]
fn an_envelope_that_names_nothing_is_an_error_in_both_instruments() {
    let dir = workspace(&SELECTION.replace(
        "  \"get:/widgets\":\n    include: true\n",
        "  \"get:/widgets\":\n    include: true\n    response: { envelope: widgetz }\n",
    ));
    let d = dir.path();
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let errors: Vec<&str> = report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("\"widgetz\" is not a root property")),
        "{:?}",
        errors
    );
    assert_eq!(report["clean"], false);
    assert!(lint_rules(d).contains(&"unknown-envelope".to_string()));
}

#[test]
fn reconcile_reports_a_selection_lint_would_reject_as_a_contract_error() {
    // `include` is required by selection.schema.json. Before ADR 0018
    // reconcile printed "selection: valid against inventory.json" for a file
    // lint rejected outright.
    let dir = workspace(&SELECTION.replace("    include: false\n", "    include: \"no\"\n"));
    let report = graphos_factory_core::reconcile::reconcile_workspace(dir.path(), None).unwrap();
    let errors: Vec<&str> = report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        errors.iter().any(|e| e.starts_with("selection.yaml")),
        "{:?}",
        errors
    );
    assert_eq!(report["clean"], false);
}

#[test]
fn a_hand_edited_inventory_is_named_by_the_lock_reconcile_lint_and_the_next_build() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    assert_eq!(draft(d, &[]), 0);
    // Acknowledge the workspace as it stands.
    assert_eq!(
        graphos_factory_core::cmd::lock::main(&[d.to_string_lossy().to_string()]),
        0
    );
    let lock = graphos_factory_core::spans::read_lock(d).unwrap().unwrap();
    assert!(lock.get("inventory").is_some(), "{:?}", lock);
    assert_eq!(
        graphos_factory_core::cmd::lock::main(&[d.to_string_lossy().to_string(), "--check".into()]),
        0
    );

    // Now "correct" the inventory by hand, the way a reader who disagreed
    // with an inference would have before ADR 0018.
    let mut inv = inventory();
    graphos_factory_core::json::set(
        &mut inv["operations"][1]["response"],
        "root_property_count",
        json!(1),
    );
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(
        graphos_factory_core::cmd::lock::main(&[d.to_string_lossy().to_string(), "--check".into()]),
        3,
        "lock --check names it"
    );
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["lock"]["inventory_edited"], true);
    let rules = lint_rules(d);
    assert!(
        rules.contains(&"unacknowledged-inventory-edit".to_string()),
        "{:?}",
        rules
    );

    // Re-indenting is not an edit: the hash is over the compact JSON.
    std::fs::write(
        d.join(".factory/inventory.json"),
        serde_json::to_string(&inventory()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        graphos_factory_core::cmd::lock::main(&[d.to_string_lossy().to_string(), "--check".into()]),
        0
    );
}

// ADR 0069 — links: in selection.yaml. A relationship is a judgement: the
// inventory fact only proposes it, and the top-level `links:` list records
// what the user chose, keyed by shape + path. Task 8 reuses this constant:
// the comment line under `operations:` is what its splice test checks
// survives, and `unlinked_selection()` is this text cut at `links:`.
const LINKED_SELECTION: &str = r#"contract_version: 1
defaults:
  fields: all
operations:
  # the list, with a comment the splice must not disturb
  "get:/widgets":
    include: true
    graphql: { root: query, name: listWidgets }
  "get:/widgets/{id}":
    include: true
    graphql: { root: query, name: widget }
  "get:/owners/{ownerId}":
    include: true
    graphql: { root: query, name: owner }
  "delete:/widgets/{id}":
    include: false
    reason: "read-only first release"
links:
  # Widget.owner_id resolves through the owner by-id operation
  - shape: Widget
    path: owner_id
    operation: "get:/owners/{ownerId}"
    parameter: ownerId
    field: owner
    include: true
    confirmed: false
"#;

#[test]
fn a_links_section_is_part_of_the_selection_contract() {
    let dir = workspace(LINKED_SELECTION);
    let d = dir.path();
    let rules = lint_rules(d);
    assert!(
        !rules.contains(&"contract".to_string()),
        "a links: section must satisfy selection.schema.json: {:?}",
        rules
    );
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let errors: Vec<&str> = report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        !errors.iter().any(|e| e.starts_with("selection.yaml")),
        "{:?}",
        errors
    );
    // The file is read back exactly as written: the section is a list of
    // block mappings, comment included.
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(
        parsed["links"],
        json!([{"shape": "Widget", "path": "owner_id", "operation": "get:/owners/{ownerId}",
                "parameter": "ownerId", "field": "owner", "include": true, "confirmed": false}])
    );
}

#[test]
fn a_malformed_link_is_refused_by_lint_and_reconcile_alike() {
    // Each variant breaks one rule of $defs/link; the validator names the
    // entry by its index so the agent can find it in a long list.
    let variants: [(&str, &str, &str); 7] = [
        (
            "    include: true\n    confirmed: false\n",
            "",
            "selection.yaml/links/0 is missing required property \"include\"",
        ),
        (
            "    path: owner_id\n",
            "    path: \"songs[]owner_id\"\n",
            "selection.yaml/links/0/path must match",
        ),
        (
            "    operation: \"get:/owners/{ownerId}\"\n",
            "    operation: \"delete:/widgets/{id}\"\n",
            "selection.yaml/links/0/operation must match ^get:/",
        ),
        (
            "    field: owner\n",
            "    field: owner\n    host: Widget_Co_Widget\n",
            "selection.yaml/links/0/host is not a known property",
        ),
        (
            "    field: owner\n",
            "    field: Owner\n",
            "selection.yaml/links/0/field must match",
        ),
        (
            "    confirmed: false\n",
            "    confirmed: false\n    decision: D-13\n",
            "selection.yaml/links/0/decision must match",
        ),
        (
            "  - shape: Widget\n    path: owner_id\n",
            "  - path: owner_id\n",
            "selection.yaml/links/0 is missing required property \"shape\"",
        ),
    ];
    for (from, to, expected) in variants {
        let text = LINKED_SELECTION.replace(from, to);
        assert_ne!(
            text, LINKED_SELECTION,
            "the variant must change the file: {}",
            from
        );
        let dir = workspace(&text);
        let d = dir.path();
        assert!(
            lint_rules(d).contains(&"contract".to_string()),
            "lint must reject: {}",
            to
        );
        let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
        let errors: Vec<&str> = report["selection_errors"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            errors.iter().any(|e| e.starts_with(expected)),
            "expected {:?} in {:?}",
            expected,
            errors
        );
        assert_eq!(report["clean"], false);
    }
    // The path pattern follows the walker (ADR 0069, R36): a wire-name
    // segment may hold any character but `>`, `[`, `]` or whitespace, and
    // each segment may carry any number of `[]` list markers, chained by
    // `>`. These are real facts `walk_link_hosts` produces.
    let schema = graphos_factory_core::schemas::load("selection.schema.json", None).unwrap();
    for path in [
        "[]>owner_id",
        "[][]>owner_id",
        "songs[]>album_id",
        "owner>account_id",
        "matrix[][]>owner_id",
    ] {
        let accepted = graphos_factory_core::yaml::parse(
            &LINKED_SELECTION.replace("    path: owner_id\n", &format!("    path: \"{path}\"\n")),
        )
        .unwrap();
        assert_eq!(
            graphos_factory_core::jsonschema::validate(&accepted, &schema),
            Vec::<String>::new(),
            "{} must validate",
            path
        );
    }
    // A missing segment before or after `>`, and a raw space inside a
    // segment, stay rejected.
    for path in [">album_id", "songs[]>", "album id"] {
        let rejected = graphos_factory_core::yaml::parse(
            &LINKED_SELECTION.replace("    path: owner_id\n", &format!("    path: \"{path}\"\n")),
        )
        .unwrap();
        assert_ne!(
            graphos_factory_core::jsonschema::validate(&rejected, &schema),
            Vec::<String>::new(),
            "{} must be rejected",
            path
        );
    }
}

fn workspace_with(selection: &str, inventory: &Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w("widget-co.graphql", SDL);
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", selection);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(inventory),
    );
    dir
}

/// `selection draft … --json` as a machine caller sees it: stdout is the
/// report, the exit code is the verdict.
fn draft_json(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("selection")
        .arg("draft")
        .arg(dir)
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{}: {}", e, stdout));
    (out.status.code().unwrap_or(-1), report)
}

// ─── ADR 0069 — links: drafted from candidate_entity_link facts ─────────────

/// The four operations of `inventory()` (Task 6 already added the owner
/// by-id lookup, the `Owner` shape and `Widget.owner_id`), with the fact
/// written on `Widget.owner_id` and an `Orphan` shape no operation returns
/// that carries the same fact. Nothing is pushed onto `operations`: the
/// owner operation is already there (R13).
fn linked_inventory() -> Value {
    let mut inv = inventory();
    assert!(
        inv["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["key"] == "get:/owners/{ownerId}"),
        "Task 6's inventory() carries the owner operation"
    );
    let fact = json!({"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false});
    inv["shapes"]["Widget"]["properties"]["owner_id"] =
        json!({"type": "string", "candidate_entity_link": fact});
    inv["shapes"]["Orphan"] = json!({"type": "object", "properties": {
        "owner_id": {"type": "string", "candidate_entity_link": fact}}});
    inv
}

/// Task 6's `LINKED_SELECTION` cut at its `links:` section: the same four
/// operations (with the comment line the splice must not disturb) and no
/// `links:` yet — what every draft here starts from (R13: one fixture, no
/// second `const LINKED_SELECTION`).
fn unlinked_selection() -> String {
    let cut = LINKED_SELECTION
        .find("links:")
        .expect("Task 6's fixture has a links: section");
    let text = LINKED_SELECTION[..cut].to_string();
    assert!(
        text.ends_with("    reason: \"read-only first release\"\n"),
        "{}",
        text
    );
    assert!(
        text.contains("  # the list, with a comment the splice must not disturb\n"),
        "{}",
        text
    );
    text
}

/// `get:/widgets` returns a bare array whose items carry the foreign key:
/// the fact sits at `shapes.WidgetArray.items.properties.owner_id`.
fn array_inventory() -> Value {
    let mut inv = linked_inventory();
    inv["operations"][0]["response"] = json!({
        "status": "200", "content_type": "application/json",
        "shape_ref": "#/shapes/WidgetArray", "root_is_array": true});
    inv["shapes"]["WidgetArray"] = json!({"type": "array", "items": {"type": "object", "properties": {
        "id": {"type": "string"},
        "owner_id": {"type": "string", "candidate_entity_link":
            {"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}}}}});
    inv
}

fn drafted_link() -> Value {
    json!({
        "shape": "Widget", "path": "owner_id", "operation": "get:/owners/{ownerId}",
        "parameter": "ownerId", "field": "owner", "include": true, "confirmed": false
    })
}

/// `(key, reason)` of every skip a `--json` report lists.
fn skips(report: &Value) -> Vec<(String, String)> {
    report["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["key"].as_str().unwrap().to_string(),
                s["reason"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn draft_writes_one_unconfirmed_link_per_surviving_fact_and_creates_the_section_at_eof() {
    let dir = workspace_with(&unlinked_selection(), &linked_inventory());
    let d = dir.path();
    assert_eq!(draft(d, &[]), 0);
    let after = read(d, ".factory/selection.yaml");
    assert!(
        after.contains("  # the list, with a comment the splice must not disturb\n"),
        "comments survive: {}",
        after
    );
    // The section is new, so it is appended after the last line — one blank
    // line between the file's last line and `links:` — as a block sequence
    // indented like every pilot's overrides.
    assert!(
        after.ends_with(concat!(
            "    reason: \"read-only first release\"\n",
            "\n",
            "links:\n",
            "  - shape: Widget\n",
            "    path: owner_id\n",
            "    operation: \"get:/owners/{ownerId}\"\n",
            "    parameter: ownerId\n",
            "    field: owner\n",
            "    include: true\n",
            "    confirmed: false   # drafted by `graphos-factory-core selection draft`\n"
        )),
        "{}",
        after
    );
    let parsed = graphos_factory_core::yaml::parse(&after).unwrap();
    // One link: Widget.owner_id. Orphan is returned by no operation.
    assert_eq!(parsed["links"], json!([drafted_link()]));
    // Both passes ran: the envelopes are drafted too.
    assert_eq!(
        parsed["operations"]["get:/widgets"]["response"],
        json!({"envelope": "widgets", "confirmed": false})
    );
    assert_eq!(
        parsed["operations"]["get:/owners/{ownerId}"]["response"],
        json!({"envelope": null, "confirmed": false})
    );

    // A second run has nothing to do and changes nothing.
    assert_eq!(draft(d, &[]), 2);
    assert_eq!(read(d, ".factory/selection.yaml"), after);
}

#[test]
fn links_and_envelopes_flags_isolate_the_two_drafts() {
    let dir = workspace_with(&unlinked_selection(), &linked_inventory());
    let d = dir.path();
    assert_eq!(draft(d, &["--links"]), 0);
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(parsed["links"], json!([drafted_link()]));
    assert!(
        parsed["operations"]["get:/widgets"]
            .get("response")
            .is_none(),
        "--links leaves envelopes alone: {}",
        parsed
    );
    assert_eq!(draft(d, &["--envelopes"]), 0);
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(
        parsed["operations"]["get:/widgets"]["response"],
        json!({"envelope": "widgets", "confirmed": false})
    );
    assert_eq!(
        parsed["links"],
        json!([drafted_link()]),
        "--envelopes leaves links alone"
    );

    // And the other way round on a fresh workspace: --envelopes alone never
    // opens a links: section.
    let dir = workspace_with(&unlinked_selection(), &linked_inventory());
    let d = dir.path();
    let (code, report) = draft_json(d, &["--envelopes"]);
    assert_eq!(code, 0, "{}", report);
    assert_eq!(report["links"], json!([]), "{}", report);
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert!(
        parsed.get("links").is_none(),
        "--envelopes writes no links: {}",
        parsed
    );
    assert_eq!(
        parsed["operations"]["get:/widgets"]["response"],
        json!({"envelope": "widgets", "confirmed": false})
    );

    // Both flags at once contradict each other: a usage error (exit 1, as
    // every usage error of the instrument) that prints the usage and
    // touches nothing — never read as "both", never as "nothing to do".
    let before = read(d, ".factory/selection.yaml");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["selection", "draft"])
        .arg(d)
        .args(["--links", "--envelopes"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "usage: selection draft [workspace] [--op KEY]… [--links | --envelopes] [--force] [--dry-run] [--json]"
        ),
        "{}",
        stderr
    );
    assert_eq!(read(d, ".factory/selection.yaml"), before);
}

#[test]
fn force_replaces_a_drafted_links_entry_and_never_a_confirmed_one() {
    let dir = workspace_with(&unlinked_selection(), &linked_inventory());
    let d = dir.path();
    assert_eq!(draft(d, &["--links"]), 0);
    let drafted = read(d, ".factory/selection.yaml");

    // A draft nobody has settled yet, touched by hand: without --force it
    // is skipped and the file is not rewritten.
    let edited = drafted.replace("    field: owner\n", "    field: proprietor\n");
    assert_ne!(edited, drafted);
    std::fs::write(d.join(".factory/selection.yaml"), &edited).unwrap();
    let (code, report) = draft_json(d, &["--links"]);
    assert_eq!(code, 2, "{}", report);
    assert!(
        skips(&report).contains(&(
            "Widget > owner_id".to_string(),
            "already has a links entry (--force replaces it)".to_string()
        )),
        "{}",
        report
    );
    assert_eq!(read(d, ".factory/selection.yaml"), edited);

    // --dry-run reports what --force would do and writes nothing.
    assert_eq!(draft(d, &["--links", "--force", "--dry-run"]), 0);
    assert_eq!(read(d, ".factory/selection.yaml"), edited);

    // --force redraws the draft in place, byte for byte — and again, since
    // the entry it replaces does not hold its own field name against it.
    assert_eq!(draft(d, &["--links", "--force"]), 0);
    assert_eq!(read(d, ".factory/selection.yaml"), drafted);
    assert_eq!(draft(d, &["--links", "--force"]), 0);
    assert_eq!(read(d, ".factory/selection.yaml"), drafted);

    // Confirm it by hand, as the agent would after asking the user. The
    // link's line carries the draft comment; the response blocks' do not.
    let confirmed = drafted.replace(
        "    confirmed: false   # drafted by `graphos-factory-core selection draft`",
        "    confirmed: true",
    );
    assert_ne!(confirmed, drafted);
    std::fs::write(d.join(".factory/selection.yaml"), &confirmed).unwrap();
    assert_eq!(draft(d, &["--links"]), 2);
    assert_eq!(read(d, ".factory/selection.yaml"), confirmed);
    // --force never replaces the user's answer.
    let (code, report) = draft_json(d, &["--links", "--force"]);
    assert_eq!(code, 2, "{}", report);
    assert_eq!(report["links"], json!([]), "{}", report);
    assert!(
        skips(&report).contains(&(
            "Widget > owner_id".to_string(),
            "already has a confirmed links entry (--force replaces only a draft)".to_string()
        )),
        "{}",
        report
    );
    assert_eq!(read(d, ".factory/selection.yaml"), confirmed);

    // An entry with no `confirmed` key is the user's word too.
    let bare = drafted.replace(
        "    confirmed: false   # drafted by `graphos-factory-core selection draft`\n",
        "",
    );
    assert_ne!(bare, drafted);
    std::fs::write(d.join(".factory/selection.yaml"), &bare).unwrap();
    assert_eq!(draft(d, &["--links", "--force"]), 2);
    assert_eq!(read(d, ".factory/selection.yaml"), bare);

    // A declined entry is an answer as well, even one still marked
    // `confirmed: false`: --force never turns it back into a proposal, and
    // the skip says it was declined, not confirmed.
    let declined = drafted.replace(
        "    include: true\n    confirmed: false   # drafted by `graphos-factory-core selection draft`\n",
        "    include: false\n    confirmed: false\n    reason: \"owners stay ids\"\n",
    );
    assert_ne!(declined, drafted);
    std::fs::write(d.join(".factory/selection.yaml"), &declined).unwrap();
    for args in [&["--links"][..], &["--links", "--force"][..]] {
        let (code, report) = draft_json(d, args);
        assert_eq!(code, 2, "{:?}: {}", args, report);
        assert!(
            skips(&report).contains(&(
                "Widget > owner_id".to_string(),
                "already has a declined links entry (include: false); nothing written".to_string()
            )),
            "{:?}: {}",
            args,
            report
        );
        assert_eq!(read(d, ".factory/selection.yaml"), declined);
    }
}

#[test]
fn a_fact_whose_by_id_operation_is_excluded_or_whose_shape_nothing_returns_is_skipped_with_a_reason(
) {
    let excluded = unlinked_selection().replace(
        "  \"get:/owners/{ownerId}\":\n    include: true\n    graphql: { root: query, name: owner }\n",
        "  \"get:/owners/{ownerId}\":\n    include: false\n    reason: \"owners are not in this slice\"\n",
    );
    assert_ne!(excluded, unlinked_selection());
    let dir = workspace_with(&excluded, &linked_inventory());
    let d = dir.path();
    let (code, report) = draft_json(d, &["--links"]);
    assert_eq!(code, 2, "{}", report);
    assert_eq!(report["links"], json!([]));
    let skipped = skips(&report);
    assert!(
        skipped.contains(&(
            "Widget > owner_id".to_string(),
            "its by-id operation is not included by the selection".to_string()
        )),
        "{:?}",
        skipped
    );
    assert!(
        skipped.contains(&(
            "Orphan > owner_id".to_string(),
            "no included operation returns the shape".to_string()
        )),
        "{:?}",
        skipped
    );
    assert_eq!(read(d, ".factory/selection.yaml"), excluded);

    // A `links:` written as a flow sequence cannot be spliced into; every
    // candidate is skipped and the file is not touched (ADR 0027).
    let flow = format!("{}links: []\n", unlinked_selection());
    let dir = workspace_with(&flow, &linked_inventory());
    let (code, report) = draft_json(dir.path(), &["--links"]);
    assert_eq!(code, 2, "{}", report);
    assert!(!skips(&report).is_empty(), "{}", report);
    assert!(
        report["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["reason"] == "links: is not a block sequence in the file"),
        "{}",
        report
    );
    assert_eq!(read(dir.path(), ".factory/selection.yaml"), flow);
}

#[test]
fn a_stale_delete_or_self_link_fact_is_skipped_not_written() {
    let mut inv = linked_inventory();
    inv["shapes"]["Widget"]["properties"]["owner_id"]["candidate_entity_link"]["operation"] =
        json!("delete:/widgets/{id}");
    inv["shapes"]["Widget"]["properties"]["id"] = json!({"type": "string", "candidate_entity_link":
        {"operation": "get:/widgets/{id}", "parameter": "id", "list_context": false}});
    let before = unlinked_selection();
    let dir = workspace_with(&before, &inv);
    let (code, report) = draft_json(dir.path(), &["--links"]);
    assert_eq!(code, 2, "{}", report);
    let reasons: Vec<&str> = report["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["reason"].as_str().unwrap())
        .collect();
    assert!(
        reasons.contains(&"its operation is not a GET (regenerate inventory.json)"),
        "{:?}",
        reasons
    );
    assert!(
        reasons.contains(&"its operation returns the shape itself"),
        "{:?}",
        reasons
    );
    assert_eq!(read(dir.path(), ".factory/selection.yaml"), before);
}

#[test]
fn a_root_array_shapes_fact_is_written_with_the_item_path_and_the_file_needs_no_trailing_newline() {
    // The list returns a bare array: the fact is on the items' owner_id, so
    // the path is `[]>owner_id` — quoted, since `[` opens a flow sequence.
    let dir = workspace_with(unlinked_selection().trim_end(), &array_inventory());
    let d = dir.path();
    assert_eq!(draft(d, &["--links"]), 0);
    let after = read(d, ".factory/selection.yaml");
    assert!(after.contains("\n    path: \"[]>owner_id\"\n"), "{}", after);
    // With no trailing newline to start from, the join is still the last
    // line, one blank line, then the new section.
    assert!(
        after.contains("    reason: \"read-only first release\"\n\nlinks:\n  - shape: "),
        "{}",
        after
    );
    let parsed = graphos_factory_core::yaml::parse(&after).unwrap();
    let mut links = parsed["links"].as_array().unwrap().clone();
    links.sort_by_key(|l| l["shape"].as_str().unwrap().to_string());
    assert_eq!(
        links,
        vec![
            drafted_link(),
            json!({
                "shape": "WidgetArray", "path": "[]>owner_id", "operation": "get:/owners/{ownerId}",
                "parameter": "ownerId", "field": "owner", "include": true, "confirmed": false
            }),
        ]
    );
    assert_eq!(draft(d, &["--links"]), 2);
    assert_eq!(read(d, ".factory/selection.yaml"), after);
}

#[test]
fn the_json_report_lists_each_drafted_link_and_a_dry_run_writes_nothing() {
    // What `selection draft` itself reports on the written path (R10): the
    // reconcile note and lint warning a draft earns are Task 7's tests, and
    // the empty `links.add` bucket is Task 9's.
    let before = unlinked_selection();
    let dir = workspace_with(&before, &linked_inventory());
    let d = dir.path();
    let (code, report) = draft_json(d, &["--links", "--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["file"], ".factory/selection.yaml");
    assert_eq!(
        report["drafted"],
        json!([]),
        "--links draws no envelope: {}",
        report
    );
    assert_eq!(
        report["links"],
        json!([{"shape": "Widget", "path": "owner_id", "field": "owner",
                "operation": "get:/owners/{ownerId}", "action": "added", "confirmed": false}]),
        "{}",
        report
    );
    assert_eq!(
        report["skipped"],
        json!([{"key": "Orphan > owner_id", "reason": "no included operation returns the shape"}]),
        "{}",
        report
    );
    assert_eq!(
        read(d, ".factory/selection.yaml"),
        before,
        "a dry run writes nothing"
    );

    // Written for real: the same report with dry_run false, and the file
    // parses back to exactly the entry the report named.
    let (code, report) = draft_json(d, &["--links"]);
    assert_eq!(code, 0, "{}", report);
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["links"][0]["action"], "added");
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(parsed["links"], json!([drafted_link()]));

    // Replaced under --force: the action says so.
    let (code, report) = draft_json(d, &["--links", "--force"]);
    assert_eq!(code, 0, "{}", report);
    assert_eq!(report["links"][0]["action"], "replaced", "{}", report);
    assert_eq!(report["links"].as_array().unwrap().len(), 1);
}

#[test]
fn op_scopes_the_links_pass_to_facts_whose_by_id_operation_it_names() {
    let before = unlinked_selection();
    let dir = workspace_with(&before, &linked_inventory());
    let d = dir.path();
    // No fact resolves through the list operation: nothing to draft, and a
    // fact outside the scope is not even reported as a skip.
    let (code, report) = draft_json(d, &["--links", "--op", "get:/widgets"]);
    assert_eq!(code, 2, "{}", report);
    assert_eq!(report["links"], json!([]), "{}", report);
    assert_eq!(report["skipped"], json!([]), "{}", report);
    assert_eq!(read(d, ".factory/selection.yaml"), before);

    let (code, report) = draft_json(d, &["--links", "--op", "get:/owners/{ownerId}"]);
    assert_eq!(code, 0, "{}", report);
    assert_eq!(report["drafted"], json!([]), "{}", report);
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(parsed["links"], json!([drafted_link()]));
}

#[test]
fn a_fact_on_a_shape_an_included_operation_reaches_only_through_a_ref_is_drafted() {
    // With the by-id widget lookup excluded, no included operation returns
    // `Widget` directly — but the list does, as its items
    // (`WidgetList.widgets.items.$ref`), so the fact is worth confirming
    // (R31: a ref-only host resolves through the list's root field).
    let sel = unlinked_selection().replace(
        "  \"get:/widgets/{id}\":\n    include: true\n    graphql: { root: query, name: widget }\n",
        "  \"get:/widgets/{id}\":\n    include: false\n    reason: \"the list is enough\"\n",
    );
    assert_ne!(sel, unlinked_selection());
    let dir = workspace_with(&sel, &linked_inventory());
    let d = dir.path();
    let (code, report) = draft_json(d, &["--links"]);
    assert_eq!(code, 0, "{}", report);
    let parsed = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
    assert_eq!(parsed["links"], json!([drafted_link()]));
    // Orphan is referenced by nothing at all and stays skipped.
    assert_eq!(
        skips(&report),
        vec![(
            "Orphan > owner_id".to_string(),
            "no included operation returns the shape".to_string()
        )]
    );
}

#[test]
fn a_fact_whose_operation_is_not_in_the_inventory_is_skipped_with_a_reason() {
    let mut inv = linked_inventory();
    inv["shapes"]["Widget"]["properties"]["owner_id"]["candidate_entity_link"]["operation"] =
        json!("get:/makers/{makerId}");
    let before = unlinked_selection();
    let dir = workspace_with(&before, &inv);
    let (code, report) = draft_json(dir.path(), &["--links"]);
    assert_eq!(code, 2, "{}", report);
    assert!(
        skips(&report).contains(&(
            "Widget > owner_id".to_string(),
            "not in inventory.json".to_string()
        )),
        "{}",
        report
    );
    assert_eq!(read(dir.path(), ".factory/selection.yaml"), before);
}

#[test]
fn two_facts_deriving_one_field_name_on_one_object_get_distinct_fields() {
    // `owner_id` and `seller_id` both resolve through the owner lookup, so
    // both derive `owner`; the same fk one object down (`maker>owner_id`)
    // lands on another object and is no clash (lint's link-duplicate-field
    // keys on the object that carries the fk).
    let fact = json!({"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false});
    let mut inv = linked_inventory();
    inv["shapes"]["Widget"]["properties"]["seller_id"] =
        json!({"type": "string", "candidate_entity_link": fact});
    inv["shapes"]["Widget"]["properties"]["maker"] = json!({"type": "object", "properties": {
        "owner_id": {"type": "string", "candidate_entity_link": fact}}});
    let dir = workspace_with(&unlinked_selection(), &inv);
    let d = dir.path();
    assert_eq!(draft(d, &["--links"]), 0);
    let fields = |d: &Path| -> Vec<(String, String)> {
        let parsed =
            graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
        parsed["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                (
                    l["path"].as_str().unwrap().to_string(),
                    l["field"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        v.iter()
            .map(|(p, f)| (p.to_string(), f.to_string()))
            .collect()
    };
    assert_eq!(
        fields(d),
        pairs(&[
            ("maker>owner_id", "owner"),
            ("owner_id", "owner"),
            ("seller_id", "ownerSellerId"),
        ])
    );
    let rules = lint_rules(d);
    assert!(
        !rules.contains(&"link-duplicate-field".to_string()),
        "{:?}",
        rules
    );

    // A field an existing entry already holds counts too: the fact that
    // appears in a later inventory takes the disambiguated name, and when a
    // hand-named entry holds that one as well, the first free numbered one.
    let dir = workspace_with(&unlinked_selection(), &linked_inventory());
    let d = dir.path();
    assert_eq!(draft(d, &["--links"]), 0);
    let text = read(d, ".factory/selection.yaml");
    let hand = concat!(
        "  - shape: Widget\n",
        "    path: colour\n",
        "    operation: \"get:/owners/{ownerId}\"\n",
        "    field: ownerSellerId\n",
        "    include: true\n",
    );
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!("{}{}", text, hand),
    )
    .unwrap();
    let mut later = linked_inventory();
    later["shapes"]["Widget"]["properties"]["seller_id"] =
        json!({"type": "string", "candidate_entity_link": fact});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&later),
    )
    .unwrap();
    assert_eq!(draft(d, &["--links"]), 0);
    assert_eq!(
        fields(d),
        pairs(&[
            ("owner_id", "owner"),
            ("colour", "ownerSellerId"),
            ("seller_id", "ownerSellerId2"),
        ])
    );
    let rules = lint_rules(d);
    assert!(
        !rules.contains(&"link-duplicate-field".to_string()),
        "{:?}",
        rules
    );
}

#[test]
fn an_existing_links_section_grows_in_place_in_its_own_indent_and_keeps_its_comments() {
    // A section written with its dashes in column 0, a column-0 comment
    // between its key and its first entry, a declined hand entry, and a
    // top-level key after it: the draft lands after the last entry, in the
    // section's own indent, before the trailing blank and comment lines.
    let head = "contract_version: 1\ndefaults:\n  fields: all\n";
    let ops = unlinked_selection();
    assert!(ops.starts_with(head), "{}", ops);
    let section = concat!(
        "links:\n",
        "# declined by hand; a column-0 comment inside the section\n",
        "- shape: Owner\n",
        "  path: id\n",
        "  operation: \"get:/owners/{ownerId}\"\n",
        "  include: false\n",
        "  reason: \"an owner's own id\"\n",
        "\n",
        "# the operations follow\n",
    );
    let before = format!("{}{}{}", head, section, &ops[head.len()..]);
    let dir = workspace_with(&before, &linked_inventory());
    let d = dir.path();
    assert_eq!(draft(d, &["--links"]), 0);
    let grown = before.replace(
        "  reason: \"an owner's own id\"\n",
        concat!(
            "  reason: \"an owner's own id\"\n",
            "- shape: Widget\n",
            "  path: owner_id\n",
            "  operation: \"get:/owners/{ownerId}\"\n",
            "  parameter: ownerId\n",
            "  field: owner\n",
            "  include: true\n",
            "  confirmed: false   # drafted by `graphos-factory-core selection draft`\n",
        ),
    );
    assert_eq!(read(d, ".factory/selection.yaml"), grown);

    // --force redraws the draft at its own index, never the declined entry
    // before it.
    let edited = grown.replace("  field: owner\n", "  field: proprietor\n");
    assert_ne!(edited, grown);
    std::fs::write(d.join(".factory/selection.yaml"), &edited).unwrap();
    assert_eq!(draft(d, &["--links", "--force"]), 0);
    assert_eq!(read(d, ".factory/selection.yaml"), grown);
}

#[test]
fn a_bare_dash_entry_is_indexed_so_force_replaces_only_the_draft_it_holds() {
    // YAML lets an entry open on a line holding only `-`, its keys on the
    // lines below. The walker must count it, or entry i of the section is
    // entry i+1 of the parsed list and --force overwrites the confirmed
    // entry after it. Both dash indents: the pilots' two columns, and
    // column 0, where a bare `-` must not end the section either. The
    // flow-style third entry is indexed like any other `- ` line, and — as
    // a declined link — holds no field name against the redraft.
    for pad in ["  ", ""] {
        let bare_draft = concat!(
            "PAD-\n",
            "PAD  shape: Widget\n",
            "PAD  path: owner_id\n",
            "PAD  operation: \"get:/owners/{ownerId}\"\n",
            "PAD  field: proprietor\n",
            "PAD  include: true\n",
            "PAD  confirmed: false\n",
        )
        .replace("PAD", pad);
        let rest = concat!(
            "PAD- shape: Owner\n",
            "PAD  path: id\n",
            "PAD  operation: \"get:/owners/{ownerId}\"\n",
            "PAD  field: account\n",
            "PAD  include: true\n",
            "PAD  confirmed: true\n",
            "PAD- { shape: Widget, path: name, operation: \"get:/owners/{ownerId}\", include: false }\n",
        )
        .replace("PAD", pad);
        let before = format!("{}links:\n{}{}", unlinked_selection(), bare_draft, rest);
        let listed = graphos_factory_core::yaml::parse(&before).unwrap()["links"].clone();
        assert_eq!(listed.as_array().unwrap().len(), 3, "{}", before);
        let dir = workspace_with(&before, &linked_inventory());
        let d = dir.path();
        let (code, report) = draft_json(d, &["--links", "--force"]);
        assert_eq!(code, 0, "{:?}: {}", pad, report);
        assert_eq!(
            report["links"],
            json!([{"shape": "Widget", "path": "owner_id", "field": "owner",
                    "operation": "get:/owners/{ownerId}", "action": "replaced", "confirmed": false}]),
            "{}",
            report
        );
        let redrawn = concat!(
            "PAD- shape: Widget\n",
            "PAD  path: owner_id\n",
            "PAD  operation: \"get:/owners/{ownerId}\"\n",
            "PAD  parameter: ownerId\n",
            "PAD  field: owner\n",
            "PAD  include: true\n",
            "PAD  confirmed: false   # drafted by `graphos-factory-core selection draft`\n",
        )
        .replace("PAD", pad);
        // Only the draft's lines change; the confirmed and the declined
        // entries keep every byte, and none is lost.
        assert_eq!(
            read(d, ".factory/selection.yaml"),
            format!("{}links:\n{}{}", unlinked_selection(), redrawn, rest),
            "{:?}",
            pad
        );
        let after = graphos_factory_core::yaml::parse(&read(d, ".factory/selection.yaml")).unwrap();
        assert_eq!(after["links"][0], drafted_link());
        assert_eq!(after["links"][1], listed[1]);
        assert_eq!(after["links"][2], listed[2]);
        assert_eq!(after["links"].as_array().unwrap().len(), 3);
    }
}

#[test]
fn a_links_section_the_walker_cannot_index_is_skipped_whole_and_left_byte_identical() {
    // A flow sequence on the line after `links:` is a block section with
    // no entry line at all: the walker counts 0 entries where the list has
    // 1. Splicing by index there would be a guess, so every candidate is
    // skipped and nothing is written, with --force or without.
    let before = format!(
        "{}{}",
        unlinked_selection(),
        "links:\n  [{ shape: Widget, path: owner_id, operation: \"get:/owners/{ownerId}\", include: true, confirmed: false }]\n"
    );
    assert_eq!(
        graphos_factory_core::yaml::parse(&before).unwrap()["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let dir = workspace_with(&before, &linked_inventory());
    let d = dir.path();
    for args in [&["--links"][..], &["--links", "--force"][..]] {
        let (code, report) = draft_json(d, args);
        assert_eq!(code, 2, "{:?}: {}", args, report);
        assert_eq!(report["links"], json!([]), "{}", report);
        let skipped = skips(&report);
        assert_eq!(skipped.len(), 2, "{:?}", skipped);
        assert!(
            skipped.iter().all(|(_, why)| why
                == "links: section could not be indexed line by line (entry count mismatch); nothing written"),
            "{:?}",
            skipped
        );
        assert_eq!(read(d, ".factory/selection.yaml"), before);
    }
}

/// The Granola dry run (2026-09-29): on a fresh workspace `selection draft`
/// failed with a bare "No such file or directory". It proposes envelopes
/// for the operations selection.yaml already includes and never chooses
/// them (ADR 0018 § 5), so a missing file is refused with that precondition
/// named, and nothing is written.
#[test]
fn draft_without_a_selection_names_the_precondition_and_writes_nothing() {
    let dir = workspace("");
    std::fs::remove_file(dir.path().join(".factory/selection.yaml")).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["selection", "draft", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no .factory/selection.yaml") && err.contains("include"),
        "{}",
        err
    );
    assert!(!dir.path().join(".factory/selection.yaml").exists());
}

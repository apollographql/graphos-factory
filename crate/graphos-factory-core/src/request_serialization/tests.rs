//! Fixture-based tests for `request_serialization::obligations`. Follows
//! `tests/integration/lint.rs`'s own `make_workspace(overrides)` convention: one
//! fully-proven baseline workspace, then one override per negative control
//! (PROOF section of the serialization task) -- each asserted to fail the
//! specific obligation, then reverted in the same test and asserted to pass
//! again, so every control demonstrates both directions.

use super::*;
use serde_json::json;
use std::collections::HashMap;

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.14", import: ["@key", "@tag"])
  @link(url: "https://specs.apollo.dev/connect/v0.4", import: ["@source", "@connect"])

@source(
  name: "widget_co"
  http: {
    baseURL: "{{BASE_URL}}"
    headers: [{ name: "Authorization", value: "Bearer {{AUTH_EXPR}}" }]
  }
)

scalar Widget_Co_JSON

input Widget_Co_TagEntryInput {
  key: String!
  value: Widget_Co_JSON
}

input Widget_Co_AddressInput {
  city: String
  zip: String
}

type Widget_Co_Widget {
  id: ID
  name: String
}

type Query {
  "List widgets, optionally filtered to a bulk set of ids."
  widget_co_listWidgets(ids: [ID!]): [Widget_Co_Widget]
    @connect(
      source: "widget_co"
      http: { GET: "/widgets", queryParams: "ids: $args.ids" }
      selection: "id name"
    )

  """
  Search widgets. The vendor documents this as a POST (its filter payload
  is too large for a query string) but it is a read: no state changes.
  """
  widget_co_searchWidgets(term: String!, tags: [String!]): [Widget_Co_Widget]
    @connect(
      source: "widget_co"
      http: {
        POST: "/widgets/search"
        body: """
        term: $args.term
        tags: $args.tags
        """
      }
      selection: "$.results { id name }"
    )
}

type Mutation {
  "Update a widget."
  widget_co_updateWidget(
    id: ID!
    name: String
    note: String
    tags: [Widget_Co_TagEntryInput!]
    address: Widget_Co_AddressInput
  ): Widget_Co_Widget
    @tag(name: "internal")
    @connect(
      source: "widget_co"
      http: {
        PATCH: "/widgets/{$args.id}"
        body: """
        name: $args.name
        note: $args.note
        tags: $args.tags
        address: $args.address
        """
      }
      selection: "id name"
    )
}
"#;

const WORKSPACE: &str = "directory: widget-co\nfield_prefix: widget_co\n";

const SELECTION: &str = r#"operations:
  "get:/widgets":
    include: true
    graphql:
      root: query
      name: listWidgets
  "post:/widgets/search":
    include: true
    graphql:
      root: query
      name: searchWidgets
  "patch:/widgets/{id}":
    include: true
    graphql:
      root: mutation
      name: updateWidget
"#;

fn inventory() -> Value {
    json!({
        "operations": [
            {"key": "get:/widgets", "method": "GET", "path": "/widgets"},
            {"key": "post:/widgets/search", "method": "POST", "path": "/widgets/search",
             "request_body": {"shape_ref": "#/shapes/SearchWidgetsRequest"}},
            {"key": "patch:/widgets/{id}", "method": "PATCH", "path": "/widgets/{id}",
             "request_body": {"shape_ref": "#/shapes/UpdateWidgetRequest"}}
        ],
        "shapes": {
            "SearchWidgetsRequest": {"type": "object", "properties": {
                "term": {"type": "string"}, "tags": {"type": "array"}
            }},
            "UpdateWidgetRequest": {"type": "object", "properties": {
                "name": {"type": "string"},
                "note": {"type": "string"},
                "tags": {"type": "array"},
                "address": {"$ref": "#/shapes/Address"}
            }},
            "Address": {"type": "object", "properties": {
                "city": {"type": "string"}, "zip": {"type": "string"}
            }}
        }
    })
}

const DECISIONS: &str = r#"{
  "contract_version": 1,
  "decisions": [
    {
      "id": "D-0001",
      "title": "What an explicit null sends, per nullable body argument",
      "status": "resolved",
      "date": "2026-09-29",
      "context": "PATCH /widgets/{id} documents that an explicit null note clears the stored note, while omitting it leaves the value unchanged; the connector forwards every nullable argument's null as JSON null.",
      "affects": ["PATCH /widgets/{id}", "POST /widgets/search"],
      "resolution": {
        "decision": "send null for every nullable body argument."
      },
      "null_handling": [
        {"operation": "patch:/widgets/{id}", "argument": "name", "behavior": "send_null"},
        {"operation": "patch:/widgets/{id}", "argument": "note", "behavior": "send_null"},
        {"operation": "patch:/widgets/{id}", "argument": "tags", "behavior": "send_null"},
        {"operation": "patch:/widgets/{id}", "argument": "address", "behavior": "send_null"},
        {"operation": "post:/widgets/search", "argument": "tags", "behavior": "send_null"}
      ]
    }
  ]
}
"#;

fn evidence_with_log(log_body: &str) -> (Value, String) {
    let ev = json!({
        "layers": {
            "wiremock_e2e": {"status": "pass", "log": ".factory/evidence/runs/2026-09-17/e2e.log"}
        },
        "operations": {}
    });
    (ev, log_body.to_string())
}

const ALL_CASES: &[&str] = &[
    "list_widgets",
    "search_widgets_full",
    "search_widgets_required",
    "update_widget_full",
    "update_widget_required",
    "update_widget_note_null",
    "update_widget_tags_empty",
    "update_widget_tags_one",
    "update_widget_tags_badkey",
    "update_widget_nulls",
    "search_widgets_tags_null",
];

fn default_log() -> String {
    let mut s = String::from("e2e: 11 passed, 0 failed\n");
    for c in ALL_CASES {
        s.push_str(&format!("PASS: {}\n", c));
    }
    s
}

/// The default log with one case's `PASS:` line removed -- the case file
/// and its stub still exist on disk, but nothing records it as executed.
fn log_without(case: &str) -> String {
    let mut s = String::from("e2e: 10 passed, 0 failed\n");
    for c in ALL_CASES {
        if *c != case {
            s.push_str(&format!("PASS: {}\n", c));
        }
    }
    s
}

fn default_cases() -> Vec<(&'static str, String)> {
    vec![
        ("tests/cases/list_widgets.graphql", "query { widget_co_listWidgets(ids: [\"w1\", \"w2\"]) { id name } }\n".to_string()),
        ("tests/cases/search_widgets_full.graphql", "query { widget_co_searchWidgets(term: \"chair\", tags: [\"red\", \"blue\"]) { id name } }\n".to_string()),
        ("tests/cases/search_widgets_required.graphql", "query { widget_co_searchWidgets(term: \"chair\") { id name } }\n".to_string()),
        ("tests/cases/update_widget_full.graphql", concat!(
            "mutation { widget_co_updateWidget(",
            "id: \"w1\", name: \"Chair\", note: \"see attached\", ",
            "tags: [{key: \"color\", value: \"red\"}, {key: \"size\", value: {units: \"cm\", amount: 10}}], ",
            "address: {city: \"NYC\", zip: \"10001\"}",
            ") { id name } }\n"
        ).to_string()),
        ("tests/cases/update_widget_required.graphql", "mutation { widget_co_updateWidget(id: \"w1\") { id name } }\n".to_string()),
        ("tests/cases/update_widget_note_null.graphql", "mutation { widget_co_updateWidget(id: \"w1\", note: null) { id name } }\n".to_string()),
        ("tests/cases/update_widget_tags_empty.graphql", "mutation { widget_co_updateWidget(id: \"w1\", tags: []) { id name } }\n".to_string()),
        ("tests/cases/update_widget_tags_one.graphql", "mutation { widget_co_updateWidget(id: \"w1\", tags: [{key: \"color\", value: \"red\"}]) { id name } }\n".to_string()),
        ("tests/cases/update_widget_tags_badkey.graphql", "mutation { widget_co_updateWidget(id: \"w1\", tags: [{key: \"non-graphql-name\", value: \"x\"}]) { id name } }\n".to_string()),
        ("tests/cases/update_widget_nulls.graphql", "mutation { widget_co_updateWidget(id: \"w1\", name: null, tags: null, address: null) { id name } }\n".to_string()),
        ("tests/cases/search_widgets_tags_null.graphql", "query { widget_co_searchWidgets(term: \"chair\", tags: null) { id name } }\n".to_string()),
    ]
}

fn mapping_json(v: Value) -> String {
    serde_json::to_string(&v).unwrap()
}

fn default_mappings() -> Vec<(&'static str, String)> {
    vec![
        (
            "tests/fixtures/mappings/list_widgets.json",
            mapping_json(json!({
                "request": {"method": "GET", "urlPath": "/widgets",
                    "queryParameters": {"ids": {"hasExactly": [{"equalTo": "w1"}, {"equalTo": "w2"}]}}},
                "response": {"status": 200, "jsonBody": {"widgets": []}}
            })),
        ),
        (
            "tests/fixtures/mappings/search_widgets_full.json",
            mapping_json(json!({
                "request": {"method": "POST", "urlPath": "/widgets/search",
                    "bodyPatterns": [{"equalToJson": {"term": "chair", "tags": ["red", "blue"]}}]},
                "response": {"status": 200, "jsonBody": {"results": []}}
            })),
        ),
        (
            "tests/fixtures/mappings/search_widgets_required.json",
            mapping_json(json!({
                "request": {"method": "POST", "urlPath": "/widgets/search",
                    "bodyPatterns": [{"equalToJson": {"term": "chair"}}]},
                "response": {"status": 200, "jsonBody": {"results": []}}
            })),
        ),
        (
            "tests/fixtures/mappings/update_widget_full.json",
            mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {
                        "name": "Chair", "note": "see attached",
                        "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
                        "address": {"city": "NYC", "zip": "10001"}
                    }}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            })),
        ),
        (
            "tests/fixtures/mappings/update_widget_required.json",
            mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {}}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            })),
        ),
        (
            "tests/fixtures/mappings/update_widget_note_null.json",
            mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {"note": null}}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            })),
        ),
        (
            "tests/fixtures/mappings/update_widget_tags_empty.json",
            mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {"tags": []}}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            })),
        ),
        (
            "tests/fixtures/mappings/update_widget_tags_one.json",
            mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {"tags": [{"key": "color", "value": "red"}]}}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            })),
        ),
        (
            "tests/fixtures/mappings/update_widget_tags_badkey.json",
            mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {"tags": [{"key": "non-graphql-name", "value": "x"}]}}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            })),
        ),
        (
            "tests/fixtures/mappings/update_widget_nulls.json",
            mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {"name": null, "tags": null, "address": null}}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            })),
        ),
        (
            "tests/fixtures/mappings/search_widgets_tags_null.json",
            mapping_json(json!({
                "request": {"method": "POST", "urlPath": "/widgets/search",
                    "bodyPatterns": [{"equalToJson": {"term": "chair", "tags": null}}]},
                "response": {"status": 200, "jsonBody": {"results": []}}
            })),
        ),
    ]
}

/// Build a full baseline workspace, then apply `overrides` (a relative path
/// -> replacement content, or `None` to delete a default file / add a new
/// one when the path is not a default).
fn make_workspace(overrides: Vec<(&str, Option<String>)>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let (evidence, log) = evidence_with_log(&default_log());
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    files.insert("widget-co.graphql".into(), Some(SDL.to_string()));
    files.insert(
        ".factory/workspace.yaml".into(),
        Some(WORKSPACE.to_string()),
    );
    files.insert(
        ".factory/selection.yaml".into(),
        Some(SELECTION.to_string()),
    );
    files.insert(
        ".factory/inventory.json".into(),
        Some(crate::json::pretty(&inventory())),
    );
    files.insert(
        ".factory/evidence/latest.json".into(),
        Some(crate::json::pretty(&evidence)),
    );
    files.insert(
        ".factory/evidence/runs/2026-09-17/e2e.log".into(),
        Some(log),
    );
    files.insert(
        ".factory/decisions.json".into(),
        Some(DECISIONS.to_string()),
    );
    for (path, content) in default_cases() {
        files.insert(path.to_string(), Some(content));
    }
    for (path, content) in default_mappings() {
        files.insert(path.to_string(), Some(content));
    }
    for (path, content) in overrides {
        files.insert(path.to_string(), content);
    }
    for (rel, content) in files {
        if let Some(text) = content {
            let file = dir.path().join(&rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
    }
    dir
}

fn ob<'a>(obs: &'a [Obligation], id: &str) -> &'a Obligation {
    obs.iter()
        .find(|o| o.id == id)
        .unwrap_or_else(|| panic!("no obligation {}", id))
}

fn s(t: &str) -> Option<String> {
    Some(t.to_string())
}

// ── Baseline: everything proven ─────────────────────────────────────────────

#[test]
fn baseline_workspace_passes_every_obligation_with_real_counts() {
    let dir = make_workspace(vec![]);
    let obs = obligations(dir.path());
    assert_eq!(obs.len(), 4, "{:#?}", obs);

    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Pass), "{:#?}", wba);

    let lap = ob(&obs, "serialization.list-argument-proof");
    assert!(matches!(lap.status, ObligationStatus::Pass), "{:#?}", lap);
    // ids (listWidgets), tags (searchWidgets), tags (updateWidget).
    assert_eq!(lap.denominator, Some((3, 3)), "{:#?}", lap);

    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Pass), "{:#?}", mc);

    let uc = ob(&obs, "serialization.unit-coverage");
    assert!(matches!(uc.status, ObligationStatus::Pass), "{:#?}", uc);
}

/// A report asks the schema the same questions for every selected operation.
/// Asked per operation, each is a scan of the whole document, and HubSpot's
/// 1.4 MB schema (113 operations) never finished (0.5.75). The report blanks
/// the document twice (`type_declarations` and `BodyIndex::new`) whatever the
/// operation count: one selected operation and three cost the same. Any one
/// cache reverted (declarations per argument, a root body per operation, an
/// input body per nested type) makes the count grow and this fail.
#[test]
fn a_report_scans_the_schema_twice_whatever_the_operation_count() {
    let scans = |dir: &tempfile::TempDir| {
        crate::graphql::DOCUMENT_SCANS.with(|c| c.set(0));
        let _ = report(dir.path());
        crate::graphql::DOCUMENT_SCANS.with(|c| c.get())
    };
    let one = SELECTION[..SELECTION.find("  \"post:/widgets/search\"").unwrap()].to_string();
    let one = scans(&make_workspace(vec![(
        ".factory/selection.yaml",
        Some(one),
    )]));
    let three = scans(&make_workspace(vec![]));
    assert_eq!((one, three), (2, 2), "document scans: one operation, three");
}

#[test]
fn case_proofs_covers_only_write_referenced_cases_with_their_e2e_verdict() {
    let dir = make_workspace(vec![]);
    let rep = report(dir.path());
    assert_eq!(
        rep.case_proofs
            .get("update_widget_full")
            .map(|p| p.verdict.as_str()),
        Some("pass"),
        "{:#?}",
        rep.case_proofs
    );
    assert!(
        !rep.case_proofs.contains_key("list_widgets"),
        "a read-only case's field is out of this layer's scope: {:#?}",
        rep.case_proofs
    );
}

// ── Negative control 1: mapping/assertion missing for an argument ──────────

#[test]
fn negative_control_missing_argument_assertion_fails_write_body_assertions() {
    let broken = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": {
                "name": "Chair", "note": "see attached",
                "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}]
                // address dropped entirely -- the connector's own assertion
                // for this argument is missing.
            }}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full.json",
        s(&broken),
    )]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    assert!(wba.message.contains("address"), "{:#?}", wba);

    let restored = make_workspace(vec![]);
    let obs2 = obligations(restored.path());
    assert!(matches!(
        ob(&obs2, "serialization.write-body-assertions").status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 2a: list reduced to one element ───────────────────────

#[test]
fn negative_control_list_reduced_to_one_element_fails_list_argument_proof() {
    let dir = make_workspace(vec![(
        "tests/cases/list_widgets.graphql",
        s("query { widget_co_listWidgets(ids: [\"w1\"]) { id name } }\n"),
    )]);
    let obs = obligations(dir.path());
    let lap = ob(&obs, "serialization.list-argument-proof");
    assert!(matches!(lap.status, ObligationStatus::Fail), "{:#?}", lap);
    assert!(lap.message.contains("ids"), "{:#?}", lap);

    let restored = make_workspace(vec![]);
    let obs2 = obligations(restored.path());
    assert!(matches!(
        ob(&obs2, "serialization.list-argument-proof").status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 2b: list incorrectly encoded ──────────────────────────

#[test]
fn negative_control_list_incorrectly_encoded_fails_list_argument_proof() {
    // The stub demands a *different* pair of values than the case actually
    // passes -- a stale or wrong fixture, not a matching one.
    let wrong = mapping_json(json!({
        "request": {"method": "GET", "urlPath": "/widgets",
            "queryParameters": {"ids": {"hasExactly": [{"equalTo": "w9"}, {"equalTo": "w8"}]}}},
        "response": {"status": 200, "jsonBody": {"widgets": []}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/list_widgets.json",
        s(&wrong),
    )]);
    let obs = obligations(dir.path());
    let lap = ob(&obs, "serialization.list-argument-proof");
    assert!(matches!(lap.status, ObligationStatus::Fail), "{:#?}", lap);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.list-argument-proof"
        )
        .status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 3: omitted input serialized as null ───────────────────

#[test]
fn negative_control_omitted_input_serialized_as_null_fails_mutation_cases() {
    // Every stub that omits `address` now claims the API received it as an
    // explicit null instead: no executed case proves the key is absent.
    let nulled: Vec<(&str, Option<String>)> = default_mappings()
        .into_iter()
        .filter_map(|(path, text)| {
            let mut m: Value = serde_json::from_str(&text).unwrap();
            if m["request"]["method"] != "PATCH" {
                return None;
            }
            let body = m["request"]["bodyPatterns"][0]["equalToJson"].as_object_mut()?;
            if body.contains_key("address") {
                return None;
            }
            body.insert("address".into(), Value::Null);
            Some((path, Some(mapping_json(m))))
        })
        .collect();
    assert!(nulled.len() >= 2, "{:#?}", nulled);
    let dir = make_workspace(nulled);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(
        mc.gaps
            .iter()
            .any(|g| g.kind == GapKind::OmissionUnproven && g.message.contains("address")),
        "{:#?}",
        mc
    );

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.mutation-cases"
        )
        .status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 4: documented explicit null incorrectly omitted ──────

#[test]
fn negative_control_explicit_null_incorrectly_omitted_fails_mutation_cases() {
    // The stub for the explicit-null case now demands an EMPTY body --
    // as if `note: null` had been dropped like an omission, collapsing the
    // two documented-distinct cases into one.
    let collapsed = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": {}}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_note_null.json",
        s(&collapsed),
    )]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(mc.message.contains("null"), "{:#?}", mc);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.mutation-cases"
        )
        .status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 5: weak body matcher accepting an incorrect nested value

#[test]
fn negative_control_weak_body_matcher_fails_write_body_assertions() {
    // ignoreExtraElements: true would accept ANY nested address, including a
    // wrong one -- ceases to be an exact assertion for this argument.
    let loose = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": {"name": "Chair"}, "ignoreExtraElements": true}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full.json",
        s(&loose),
    )]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.write-body-assertions"
        )
        .status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 6: missing required-only/full case ───────────────────

#[test]
fn negative_control_missing_required_only_case_fails_mutation_cases() {
    let dir = make_workspace(vec![
        ("tests/cases/update_widget_required.graphql", None),
        ("tests/fixtures/mappings/update_widget_required.json", None),
    ]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(
        mc.message.contains("only its required arguments"),
        "{:#?}",
        mc
    );

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.mutation-cases"
        )
        .status,
        ObligationStatus::Pass
    ));
}

#[test]
fn negative_control_missing_full_case_fails_mutation_cases() {
    let dir = make_workspace(vec![
        ("tests/cases/update_widget_full.graphql", None),
        ("tests/fixtures/mappings/update_widget_full.json", None),
    ]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(mc.message.contains("every argument"), "{:#?}", mc);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.mutation-cases"
        )
        .status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 7: missing, stale or failed execution evidence ────────

#[test]
fn negative_control_missing_evidence_fails_every_computable_obligation() {
    let dir = make_workspace(vec![(".factory/evidence/latest.json", None)]);
    let obs = obligations(dir.path());
    for id in [
        "serialization.write-body-assertions",
        "serialization.list-argument-proof",
        "serialization.mutation-cases",
        "serialization.unit-coverage",
    ] {
        let o = ob(&obs, id);
        assert!(
            matches!(o.status, ObligationStatus::Fail),
            "{}: {:#?}",
            id,
            o
        );
    }
    let restored = make_workspace(vec![]);
    for id in [
        "serialization.write-body-assertions",
        "serialization.list-argument-proof",
        "serialization.mutation-cases",
        "serialization.unit-coverage",
    ] {
        assert!(matches!(
            ob(&obligations(restored.path()), id).status,
            ObligationStatus::Pass
        ));
    }
}

#[test]
fn negative_control_stale_log_without_the_case_fails_write_body_assertions() {
    // The layer claims an aggregate pass, but the log it points at was
    // never regenerated for this specific case -- a fixture existing on
    // disk with no executed record for it must not count as proven.
    let mut log = default_log();
    log = log.replace("PASS: update_widget_full\n", "");
    let dir = make_workspace(vec![(".factory/evidence/runs/2026-09-17/e2e.log", s(&log))]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.write-body-assertions"
        )
        .status,
        ObligationStatus::Pass
    ));
}

#[test]
fn negative_control_recorded_case_failure_fails_write_body_assertions() {
    let log = default_log().replace("PASS: update_widget_full", "FAIL: update_widget_full");
    let dir = make_workspace(vec![(".factory/evidence/runs/2026-09-17/e2e.log", s(&log))]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    assert!(wba.message.contains("recorded as failed"), "{:#?}", wba);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.write-body-assertions"
        )
        .status,
        ObligationStatus::Pass
    ));
}

// ── Negative control 8: missing unit proof without adequate e2e replacement

#[test]
fn negative_control_no_e2e_replacement_fails_unit_coverage() {
    // Deleting every case+mapping pair proving updateWidget's (object- and
    // list-valued) body leaves it with no unit proof (impossible for this
    // shape) and no e2e replacement either.
    let dir = make_workspace(vec![
        ("tests/cases/update_widget_full.graphql", None),
        ("tests/fixtures/mappings/update_widget_full.json", None),
        ("tests/cases/update_widget_required.graphql", None),
        ("tests/fixtures/mappings/update_widget_required.json", None),
        ("tests/cases/update_widget_note_null.graphql", None),
        ("tests/fixtures/mappings/update_widget_note_null.json", None),
        ("tests/cases/update_widget_tags_empty.graphql", None),
        (
            "tests/fixtures/mappings/update_widget_tags_empty.json",
            None,
        ),
        ("tests/cases/update_widget_tags_one.graphql", None),
        ("tests/fixtures/mappings/update_widget_tags_one.json", None),
        ("tests/cases/update_widget_tags_badkey.graphql", None),
        (
            "tests/fixtures/mappings/update_widget_tags_badkey.json",
            None,
        ),
        ("tests/cases/update_widget_nulls.graphql", None),
        ("tests/fixtures/mappings/update_widget_nulls.json", None),
    ]);
    let obs = obligations(dir.path());
    let uc = ob(&obs, "serialization.unit-coverage");
    assert!(matches!(uc.status, ObligationStatus::Fail), "{:#?}", uc);
    assert!(uc.message.contains("updateWidget"), "{:#?}", uc);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(&obligations(restored.path()), "serialization.unit-coverage").status,
        ObligationStatus::Pass
    ));
}

// ── Missing/unsupported source input (task item 1) ─────────────────────────

#[test]
fn negative_control_undocumented_source_member_is_an_explicit_unmet_obligation() {
    let mut inv = inventory();
    inv["shapes"]["UpdateWidgetRequest"]["properties"]["priority"] = json!({"type": "string"});
    let dir = make_workspace(vec![(
        ".factory/inventory.json",
        s(&crate::json::pretty(&inv)),
    )]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    assert!(wba.message.contains("priority"), "{:#?}", wba);

    let restored = make_workspace(vec![]);
    assert!(matches!(
        ob(
            &obligations(restored.path()),
            "serialization.write-body-assertions"
        )
        .status,
        ObligationStatus::Pass
    ));
}

// ── Read-only POST gets the same case-completeness as a mutation ──────────

#[test]
fn read_only_post_still_requires_a_required_only_and_a_full_case() {
    let dir = make_workspace(vec![
        ("tests/cases/search_widgets_required.graphql", None),
        ("tests/fixtures/mappings/search_widgets_required.json", None),
    ]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(mc.message.contains("searchWidgets"), "{:#?}", mc);
}

// ── Nested input member proof ───────────────────────────────────────────────

#[test]
fn negative_control_missing_nested_member_assertion_is_caught() {
    let broken = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": {
                "name": "Chair", "note": "see attached",
                "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
                "address": {"zip": "10001"}
            }}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full.json",
        s(&broken),
    )]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    assert!(wba.message.contains("address.city"), "{:#?}", wba);
}

// ── Map five-case contract, individually ────────────────────────────────────

#[test]
fn negative_control_missing_map_empty_case_fails_mutation_cases() {
    let dir = make_workspace(vec![
        ("tests/cases/update_widget_tags_empty.graphql", None),
        (
            "tests/fixtures/mappings/update_widget_tags_empty.json",
            None,
        ),
    ]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(mc.message.contains("empty map"), "{:#?}", mc);
}

#[test]
fn negative_control_missing_map_bad_key_case_fails_mutation_cases() {
    let dir = make_workspace(vec![
        ("tests/cases/update_widget_tags_badkey.graphql", None),
        (
            "tests/fixtures/mappings/update_widget_tags_badkey.json",
            None,
        ),
    ]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(mc.message.contains("not a valid GraphQL name"), "{:#?}", mc);
}

// ── Blocking/NotApplicable plumbing ──────────────────────────────────────────

#[test]
fn a_nullable_argument_with_no_null_handling_decision_is_unproven_even_when_a_case_sends_null() {
    // The plan's negative: no decision at all, and the note_null case shows
    // the connector sending `"note": null`. That is not proof of a decided
    // behavior: it must read unproven, never pass.
    let without_note = DECISIONS.replace(
        "        {\"operation\": \"patch:/widgets/{id}\", \"argument\": \"note\", \"behavior\": \"send_null\"},\n",
        "",
    );
    assert_ne!(without_note, DECISIONS);
    let dir = make_workspace(vec![(".factory/decisions.json", Some(without_note))]);
    let mc = ob(&obligations(dir.path()), "serialization.mutation-cases").clone();
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    let note: Vec<&Gap> = mc
        .gaps
        .iter()
        .filter(|g| g.message.contains(" note "))
        .collect();
    assert_eq!(note.len(), 1, "{:#?}", mc.gaps);
    assert_eq!(note[0].kind, GapKind::NullHandlingUndecided);

    let dir = make_workspace(vec![]);
    let mc = ob(&obligations(dir.path()), "serialization.mutation-cases").clone();
    assert!(matches!(mc.status, ObligationStatus::Pass), "{:#?}", mc);
}

#[test]
fn only_a_resolved_decisions_null_handling_counts() {
    let open = DECISIONS
        .replace("\"status\": \"resolved\"", "\"status\": \"open\"")
        .replace(
            ",\n      \"resolution\": {\n        \"decision\": \"send null for every nullable body argument.\"\n      }",
            "",
        );
    assert!(
        !open.contains("resolved") && !open.contains("\"resolution\""),
        "{}",
        open
    );
    let dir = make_workspace(vec![(".factory/decisions.json", Some(open))]);
    let mc = ob(&obligations(dir.path()), "serialization.mutation-cases").clone();
    let undecided = mc
        .gaps
        .iter()
        .filter(|g| g.kind == GapKind::NullHandlingUndecided)
        .count();
    assert_eq!(undecided, 5, "{:#?}", mc.gaps);
}

/// A superseded record's `null_handling` entries do not count either (ADR
/// 0103): `supersede` changes only the status and keeps the resolution, so
/// a check written as `status != "open"` would read the replaced choice as
/// still in force.
#[test]
fn a_superseded_decisions_null_handling_does_not_count() {
    let superseded = DECISIONS.replace("\"status\": \"resolved\"", "\"status\": \"superseded\"");
    assert!(
        !superseded.contains("\"resolved\"") && superseded.contains("\"resolution\""),
        "{}",
        superseded
    );
    let dir = make_workspace(vec![(".factory/decisions.json", Some(superseded))]);
    let mc = ob(&obligations(dir.path()), "serialization.mutation-cases").clone();
    let undecided = mc
        .gaps
        .iter()
        .filter(|g| g.kind == GapKind::NullHandlingUndecided)
        .count();
    assert_eq!(undecided, 5, "{:#?}", mc.gaps);
}

#[test]
fn a_decided_behavior_the_stub_contradicts_is_unproven() {
    // Decided `omit`, but the only null case's stub demands `"note": null`.
    let omit = DECISIONS.replace(
        "\"argument\": \"note\", \"behavior\": \"send_null\"",
        "\"argument\": \"note\", \"behavior\": \"omit\"",
    );
    assert_ne!(omit, DECISIONS);
    let dir = make_workspace(vec![(".factory/decisions.json", Some(omit))]);
    let mc = ob(&obligations(dir.path()), "serialization.mutation-cases").clone();
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(
        mc.gaps
            .iter()
            .any(|g| g.kind == GapKind::NullUnproven && g.message.contains("note is omit")),
        "{:#?}",
        mc.gaps
    );
}

#[test]
fn report_lists_each_body_sending_write_with_its_own_gaps() {
    let dir = make_workspace(vec![]);
    let r = report(dir.path());
    let ops: Vec<(&str, &str, bool)> = r
        .writes
        .iter()
        .map(|w| (w.operation.as_str(), w.method.as_str(), w.body_proven))
        .collect();
    // The read-only POST sends a body too, so it is counted with the PATCH.
    assert_eq!(
        ops,
        vec![
            ("post:/widgets/search", "POST", true),
            ("patch:/widgets/{id}", "PATCH", true)
        ],
        "{:#?}",
        r.writes
    );
    assert!(
        r.writes.iter().all(|w| w.gaps.is_empty()),
        "{:#?}",
        r.writes
    );

    // Drop the full-body case's `address` assertion: only the PATCH loses
    // its proven body, and the gap names the argument.
    let broken = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": {
                "name": "Chair", "note": "see attached",
                "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}]
            }}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full.json",
        s(&broken),
    )]);
    let r = report(dir.path());
    let patch = r
        .writes
        .iter()
        .find(|w| w.operation == "patch:/widgets/{id}")
        .unwrap();
    assert!(!patch.body_proven, "{:#?}", patch);
    assert!(
        patch
            .gaps
            .iter()
            .any(|g| g.kind == GapKind::ArgumentUnasserted && g.message.contains("address")),
        "{:#?}",
        patch
    );
    let search = r
        .writes
        .iter()
        .find(|w| w.operation == "post:/widgets/search")
        .unwrap();
    assert!(search.body_proven, "{:#?}", search);
}

#[test]
fn a_demanded_body_written_as_a_json_string_reads_like_the_object_form() {
    // WireMock accepts `equalToJson` as a string holding the JSON; the
    // pilots write it that way. Re-encode every default stub's demanded
    // body as a string: every obligation must still pass.
    let restrung: Vec<(&str, Option<String>)> = default_mappings()
        .into_iter()
        .map(|(path, text)| {
            let mut m: Value = serde_json::from_str(&text).unwrap();
            if let Some(ps) = m["request"]["bodyPatterns"].as_array_mut() {
                for p in ps {
                    if let Some(body) = p.get("equalToJson").cloned() {
                        p["equalToJson"] = Value::String(body.to_string());
                    }
                }
            }
            (path, Some(mapping_json(m)))
        })
        .collect();
    let dir = make_workspace(restrung);
    for o in obligations(dir.path()) {
        assert!(matches!(o.status, ObligationStatus::Pass), "{:#?}", o);
    }
}

#[test]
fn missing_selection_or_inventory_is_unexecuted_not_a_pass() {
    let dir = tempfile::tempdir().unwrap();
    let obs = obligations(dir.path());
    for o in &obs {
        assert!(matches!(o.status, ObligationStatus::Unexecuted), "{:#?}", o);
    }
}

// ── Unit-level tests of the smaller textual helpers ─────────────────────────

#[test]
fn call_arg_texts_reads_null_and_object_and_list_values() {
    let doc = r#"mutation { widget_co_updateWidget(id: "w1", note: null, tags: [{key: "a", value: 1}, {key: "b", value: {x: 1}}]) { id } }"#;
    let calls = call_arg_texts(doc, "widget_co_updateWidget");
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(
        call.iter()
            .find(|(n, _)| n == "id")
            .map(|(_, v)| v.as_str()),
        Some("\"w1\"")
    );
    assert_eq!(
        call.iter()
            .find(|(n, _)| n == "note")
            .map(|(_, v)| v.as_str()),
        Some("null")
    );
    let tags = call
        .iter()
        .find(|(n, _)| n == "tags")
        .map(|(_, v)| v.as_str())
        .unwrap();
    assert_eq!(list_elements_text(tags).len(), 2);
}

#[test]
fn quote_bare_keys_does_not_corrupt_string_contents() {
    let text = r#"{ note: "field: value", tags: ["a: b"] }"#;
    let json = graphql_member_to_json(text).unwrap();
    assert_eq!(json["note"], "field: value");
    assert_eq!(json["tags"][0], "a: b");
}

#[test]
fn map_five_cases_detects_each_class() {
    let empty_only = vec![("c", vec![("tags".to_string(), "[]".to_string())])];
    let arg = ArgInfo {
        name: "tags".into(),
        type_text: "[Widget_Co_TagEntryInput!]".into(),
        nested: Vec::new().into(),
    };
    let ctx = OpCtx {
        key: "k".into(),
        field: "f".into(),
        verb: "PATCH".into(),
        args: vec![],
        wire: Wiring::default(),
        places: WireMap::default(),
        body_flat: None,
        calls: empty_only,
        body_args_unmatched: vec![],
        body_members_unclassified: vec![],
        exec: Default::default(),
        ambiguous: Default::default(),
        required_omitted: vec![],
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".factory/evidence/runs/2026-09-17")).unwrap();
    std::fs::write(
        dir.path().join(".factory/evidence/runs/2026-09-17/e2e.log"),
        "PASS: c\n",
    )
    .unwrap();
    let evidence = json!({
        "layers": {"wiremock_e2e": {"status": "pass", "log": ".factory/evidence/runs/2026-09-17/e2e.log"}}
    });
    let mappings = vec![("c.json".to_string(), json!({"request": {}, "response": {}}))];
    let cases = map_five_cases(&ctx, &arg, &mappings, Some(&evidence), dir.path());
    assert_eq!(cases[0], ("empty map ([])", true));
    assert_eq!(cases[1], ("single entry", false));
}

#[test]
fn call_arg_texts_reads_arguments_separated_by_newlines_alone() {
    // GraphQL commas are optional; the pilots write one argument per line.
    let doc = r#"mutation {
  gitea_createIssue(
    owner: "fixture"
    title: "Router, smoke"
    note: """a block, string"""
    labels: [1, 2]
    milestone: 1
    closed: false
    ref: $ref
  ) { id }
}"#;
    let calls = call_arg_texts(doc, "gitea_createIssue");
    assert_eq!(
        calls,
        vec![vec![
            ("owner".to_string(), "\"fixture\"".to_string()),
            ("title".to_string(), "\"Router, smoke\"".to_string()),
            (
                "note".to_string(),
                "\"\"\"a block, string\"\"\"".to_string()
            ),
            ("labels".to_string(), "[1, 2]".to_string()),
            ("milestone".to_string(), "1".to_string()),
            ("closed".to_string(), "false".to_string()),
            ("ref".to_string(), "$ref".to_string()),
        ]]
    );
}

// ── Location-specific proof: a value that recurs elsewhere proves nothing ──

/// The full case with `address.city` and `address.zip` sharing one value,
/// and a stub that drops `city`: "Oslo" still appears in the body (at
/// `zip`), but nothing demands it at `/address/city`.
fn recurring_nested_value_workspace(stub_address: Value) -> tempfile::TempDir {
    let case = concat!(
        "mutation { widget_co_updateWidget(",
        "id: \"w1\", name: \"Chair\", note: \"see attached\", ",
        "tags: [{key: \"color\", value: \"red\"}, {key: \"size\", value: {units: \"cm\", amount: 10}}], ",
        "address: {city: \"Oslo\", zip: \"Oslo\"}",
        ") { id name } }\n"
    );
    let stub = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": {
                "name": "Chair", "note": "see attached",
                "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
                "address": stub_address
            }}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    make_workspace(vec![
        ("tests/cases/update_widget_full.graphql", s(case)),
        ("tests/fixtures/mappings/update_widget_full.json", s(&stub)),
    ])
}

#[test]
fn a_dropped_nested_member_whose_value_recurs_elsewhere_is_not_proven() {
    let dir = recurring_nested_value_workspace(json!({"zip": "Oslo"}));
    let wba = ob(
        &obligations(dir.path()),
        "serialization.write-body-assertions",
    )
    .clone();
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    assert!(wba.message.contains("address.city"), "{:#?}", wba);

    let dir = recurring_nested_value_workspace(json!({"city": "Oslo", "zip": "Oslo"}));
    let wba = ob(
        &obligations(dir.path()),
        "serialization.write-body-assertions",
    )
    .clone();
    assert!(matches!(wba.status, ObligationStatus::Pass), "{:#?}", wba);
}

#[test]
fn a_path_argument_is_proven_only_as_its_own_segment() {
    // `/widgets/w1-archive` contains "w1", but the id segment is not "w1".
    let url = |path: &str| {
        mapping_json(json!({
            "request": {"method": "PATCH", "urlPath": path,
                "bodyPatterns": [{"equalToJson": {
                    "name": "Chair", "note": "see attached",
                    "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
                    "address": {"city": "NYC", "zip": "10001"}
                }}]},
            "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
        }))
    };
    // Every PATCH case's stub moves to the look-alike path.
    let moved: Vec<(&str, Option<String>)> = default_mappings()
        .into_iter()
        .filter_map(|(path, text)| {
            let mut m: Value = serde_json::from_str(&text).unwrap();
            if m["request"]["method"] != "PATCH" {
                return None;
            }
            m["request"]["urlPath"] = Value::String("/widgets/w1-archive".into());
            Some((path, Some(mapping_json(m))))
        })
        .collect();
    let dir = make_workspace(moved);
    let wba = ob(
        &obligations(dir.path()),
        "serialization.write-body-assertions",
    )
    .clone();
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    assert!(wba.message.contains("updateWidget(id)"), "{:#?}", wba);

    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full.json",
        s(&url("/widgets/w1")),
    )]);
    let wba = ob(
        &obligations(dir.path()),
        "serialization.write-body-assertions",
    )
    .clone();
    assert!(matches!(wba.status, ObligationStatus::Pass), "{:#?}", wba);
}

#[test]
fn wire_map_places_sub_selection_members_headers_and_path_placeholders() {
    let field = r#"  sf_createAccount(owner: ID!, from: String, input: In!): Out
    @connect(
      source: "sf"
      http: {
        POST: "/owners/{$args.owner}/accounts?x=1"
        headers: [{ name: "From", value: "{$args.from}" }]
        body: """
        $args.input {
          Name: name
          type
          Billing: billing { Street: street }
          Rating: rating->match(["hot", "Hot"])
        }
        """
      }
      selection: "id"
    )"#;
    let map = wire_map(field, &wiring(field), &HashSet::new());
    let place =
        |p: &[&str]| locations_of(&map, &p.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(
        place(&["input", "name"]),
        vec![Location::Body(vec!["Name".into()])]
    );
    assert_eq!(
        place(&["input", "type"]),
        vec![Location::Body(vec!["type".into()])]
    );
    assert_eq!(
        place(&["input", "billing", "street"]),
        vec![Location::Body(vec!["Billing".into(), "Street".into()])]
    );
    assert_eq!(place(&["input", "rating"]), vec![]);
    assert_eq!(map.unclassified, vec![vec!["Rating".to_string()]]);
    assert_eq!(
        place(&["owner"]),
        vec![Location::Path("/owners/{$args.owner}/accounts".into())]
    );
    assert_eq!(
        place(&["from"]),
        vec![Location::Header("From".into(), "{$args.from}".into())]
    );
    assert!(is_container(&map, &["input".to_string()]));
    // `input.billing` is placed member by member too: a container, not an
    // unplaced argument.
    assert!(is_container(
        &map,
        &["input".to_string(), "billing".to_string()]
    ));
    assert!(!is_container(
        &map,
        &["input".to_string(), "name".to_string()]
    ));
}

#[test]
fn a_header_argument_is_proven_by_its_own_header_only() {
    let loc = Location::Header("From".into(), "{$args.from}".into());
    let path = vec!["from".to_string()];
    let stub = |headers: Value| json!({"request": {"method": "POST", "headers": headers}});
    let own = stub(json!({"From": {"equalTo": "a@b.c"}}));
    let other = stub(json!({"X-Reply-To": {"equalTo": "a@b.c"}}));
    assert!(demands_at(&own, &loc, &path, "\"a@b.c\""));
    assert!(!demands_at(&other, &loc, &path, "\"a@b.c\""));
    assert!(!demands_at(&own, &loc, &path, "\"x@y.z\""));
}

#[test]
fn an_argument_the_connector_transforms_reads_unproven_mapping_not_unasserted() {
    // `note` reaches the body through a method chain: no location to check,
    // and the value the case passes is not the value the stub demands (the
    // connector reshapes it -- here its case), so an executed case cannot
    // place it either. The value passed through unchanged would be placed by
    // execution (`executed_placement_*` below).
    let sdl = SDL.replace("note: $args.note", "note: $args.note->match([\"\", null])");
    assert_ne!(sdl, SDL);
    let reshaped = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": {
                "name": "Chair", "note": "SEE ATTACHED",
                "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
                "address": {"city": "NYC", "zip": "10001"}
            }}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![
        ("widget-co.graphql", Some(sdl)),
        (
            "tests/fixtures/mappings/update_widget_full.json",
            s(&reshaped),
        ),
    ]);
    let wba = ob(
        &obligations(dir.path()),
        "serialization.write-body-assertions",
    )
    .clone();
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    let note: Vec<&Gap> = wba
        .gaps
        .iter()
        .filter(|g| g.message.contains("updateWidget(note)"))
        .collect();
    assert_eq!(note.len(), 1, "{:#?}", wba.gaps);
    assert_eq!(note[0].kind, GapKind::UnprovenMapping, "{:#?}", note);
}

// ── The workspace default: defaults.null_handling in selection.yaml ─────────

fn selection_with_default(behavior: &str) -> Option<String> {
    Some(SELECTION.replacen(
        "operations:",
        &format!("defaults:\n  null_handling: {}\noperations:", behavior),
        1,
    ))
}

fn null_gaps(dir: &tempfile::TempDir) -> Vec<Gap> {
    ob(&obligations(dir.path()), "serialization.mutation-cases")
        .gaps
        .iter()
        .filter(|g| {
            matches!(
                g.kind,
                GapKind::NullHandlingUndecided | GapKind::NullUnproven
            )
        })
        .cloned()
        .collect()
}

const NULL_CASES: [&str; 3] = [
    "update_widget_nulls",
    "update_widget_note_null",
    "search_widgets_tags_null",
];

#[test]
fn the_default_alone_resolves_every_nullable_argument() {
    // No decisions.json at all: send_null is proven by the null cases.
    let dir = make_workspace(vec![
        (".factory/decisions.json", None),
        (
            ".factory/selection.yaml",
            selection_with_default("send_null"),
        ),
    ]);
    assert_eq!(null_gaps(&dir).len(), 0, "{:#?}", null_gaps(&dir));

    // omit is proven by the executed omitting cases, once no case shows a
    // null reaching the body.
    let mut overrides: Vec<(String, Option<String>)> = vec![
        (".factory/decisions.json".into(), None),
        (
            ".factory/selection.yaml".into(),
            selection_with_default("omit"),
        ),
    ];
    for c in NULL_CASES {
        overrides.push((format!("tests/cases/{}.graphql", c), None));
        overrides.push((format!("tests/fixtures/mappings/{}.json", c), None));
    }
    let dir = make_workspace(
        overrides
            .iter()
            .map(|(p, c)| (p.as_str(), c.clone()))
            .collect(),
    );
    assert_eq!(null_gaps(&dir).len(), 0, "{:#?}", null_gaps(&dir));
}

#[test]
fn a_decision_entry_overrides_the_default() {
    // defaults.null_handling: omit would be contradicted by every null case;
    // the send_null entries for all five arguments win.
    let dir = make_workspace(vec![(
        ".factory/selection.yaml",
        selection_with_default("omit"),
    )]);
    assert_eq!(null_gaps(&dir).len(), 0, "{:#?}", null_gaps(&dir));
}

#[test]
fn an_executed_case_contradicting_the_default_is_unproven() {
    // Tests win: omit stated, but the null cases' stubs demand JSON null.
    let dir = make_workspace(vec![
        (".factory/decisions.json", None),
        (".factory/selection.yaml", selection_with_default("omit")),
    ]);
    let gaps = null_gaps(&dir);
    assert_eq!(gaps.len(), 5, "{:#?}", gaps);
    assert!(
        gaps.iter().all(|g| g.kind == GapKind::NullUnproven
            && g.message.contains("defaults.null_handling")
            && g.message.contains("otherwise")),
        "{:#?}",
        gaps
    );
}

#[test]
fn no_default_and_no_entry_is_undecided_for_every_nullable_argument() {
    // No implicit default: without either, all five are undecided, even
    // though the null cases show what the connector sends.
    let dir = make_workspace(vec![(".factory/decisions.json", None)]);
    let gaps = null_gaps(&dir);
    assert_eq!(gaps.len(), 5, "{:#?}", gaps);
    assert!(
        gaps.iter()
            .all(|g| g.kind == GapKind::NullHandlingUndecided),
        "{:#?}",
        gaps
    );
}

#[test]
fn body_proven_is_false_when_any_gap_kind_remains_not_only_argument_unasserted() {
    let dir = make_workspace(vec![
        (
            "tests/cases/update_widget_full.graphql",
            s(concat!(
                "mutation { widget_co_updateWidget(",
                "id: \"w1\", name: \"Chair\", note: \"see attached\", ",
                "tags: [{key: \"color\", value: \"red\"}, {key: \"color\", value: \"red\"}], ",
                "address: {city: \"NYC\", zip: \"10001\"}",
                ") { id name } }\n"
            )),
        ),
        (
            "tests/fixtures/mappings/update_widget_full.json",
            Some(mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {
                        "name": "Chair", "note": "see attached",
                        "tags": [{"key": "color", "value": "red"}, {"key": "color", "value": "red"}],
                        "address": {"city": "NYC", "zip": "10001"}
                    }}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            }))),
        ),
    ]);
    let rep = report(dir.path());
    let write = rep
        .writes
        .iter()
        .find(|w| w.operation == "patch:/widgets/{id}")
        .unwrap();
    // Non-AU/SM gaps remain (two identical tag entries: not 2 distinct list
    // values, and not the map five-case's "distinct keys" entry either), and
    // write-body-assertions itself stays clean -- body_proven must still be
    // false, not just "no ArgumentUnasserted/SourceMemberUnmapped gap".
    assert!(
        write
            .gaps
            .iter()
            .any(|g| matches!(g.kind, GapKind::ListUnproven | GapKind::MapCaseMissing)),
        "{:#?}",
        write.gaps
    );
    assert!(!write.body_proven, "{:#?}", write);
}

#[test]
fn a_same_line_second_input_field_is_not_dropped_from_the_proof_denominator() {
    // `name: String! bio: String!` declares two fields on one line -- the
    // pre-fix reader matched only the first (`name`) per line, so a
    // connector that forwards only `name` and silently drops the required
    // `bio` sub-selection member passed with no gap at all: `bio` was never
    // in the denominator to begin with.
    let sdl = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.14", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.4", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

input Widget_Co_ProfileInput { name: String! bio: String! }

type Widget_Co_Widget {
  id: ID
}

type Mutation {
  "Update a profile."
  widget_co_updateProfile(id: ID!, profile: Widget_Co_ProfileInput!): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        PATCH: "/profiles/{$args.id}"
        body: "payload: $args.profile { name }"
      }
      selection: "id"
    )
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".factory/evidence/runs/2026-09-17")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests/cases")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests/fixtures/mappings")).unwrap();
    std::fs::write(dir.path().join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        dir.path().join(".factory/workspace.yaml"),
        "directory: widget-co\nfield_prefix: widget_co\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/selection.yaml"),
        "operations:\n  \"patch:/profiles/{id}\":\n    include: true\n    graphql:\n      root: mutation\n      name: updateProfile\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/inventory.json"),
        crate::json::pretty(&json!({
            "operations": [{"key": "patch:/profiles/{id}", "method": "PATCH", "path": "/profiles/{id}"}],
            "shapes": {}
        })),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/evidence/latest.json"),
        crate::json::pretty(&json!({
            "layers": {"wiremock_e2e": {"status": "pass", "log": ".factory/evidence/runs/2026-09-17/e2e.log"}}
        })),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/evidence/runs/2026-09-17/e2e.log"),
        "PASS: update_profile_full\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tests/cases/update_profile_full.graphql"),
        "mutation { widget_co_updateProfile(id: \"p1\", profile: {name: \"Ana\", bio: \"hi\"}) { id } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path()
            .join("tests/fixtures/mappings/update_profile_full.json"),
        mapping_json(json!({
            "request": {"method": "PATCH", "urlPath": "/profiles/p1",
                "bodyPatterns": [{"equalToJson": {"payload": {"name": "Ana"}}}]},
            "response": {"status": 200, "jsonBody": {"id": "p1"}}
        })),
    )
    .unwrap();
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(
        wba.gaps.iter().any(|g| g.message.contains("bio")),
        "the dropped required member `bio` must appear as a gap, not vanish from the denominator: {:#?}",
        wba.gaps
    );
}

// ── Mutation-hardening: M2 and M4 from the review's mutation table ─────────
// Both guards already existed and were correct; neither had a test that
// failed when the guard was removed (verified by hand: commenting out
// each one in turn and re-running these two tests turns them red).

#[test]
fn a_single_distinct_list_element_does_not_satisfy_the_two_element_requirement() {
    // M2: the `distinct.len() < 2` guard has no regression test with an
    // AGREEING one-element case and matcher (as opposed to one that merely
    // fails for an unrelated reason).
    let dir = make_workspace(vec![
        (
            "tests/cases/list_widgets.graphql",
            s("query { widget_co_listWidgets(ids: [\"w1\"]) { id name } }\n"),
        ),
        (
            "tests/fixtures/mappings/list_widgets.json",
            Some(mapping_json(json!({
                "request": {"method": "GET", "urlPath": "/widgets",
                    "queryParameters": {"ids": {"hasExactly": [{"equalTo": "w1"}]}}},
                "response": {"status": 200, "jsonBody": {"widgets": []}}
            }))),
        ),
    ]);
    let obs = obligations(dir.path());
    let lap = ob(&obs, "serialization.list-argument-proof");
    assert!(matches!(lap.status, ObligationStatus::Fail), "{:#?}", lap);
    assert!(
        lap.gaps
            .iter()
            .any(|g| g.kind == GapKind::ListUnproven && g.message.contains("at least 2 required")),
        "{:#?}",
        lap.gaps
    );
}

#[test]
fn a_hasexactly_matcher_with_an_extra_value_does_not_satisfy_two_element_proof() {
    // M4: `hasExactly`'s cardinality check (`have.len() == want.len()`) has
    // no regression test with an extra value beyond the two the case
    // actually passes.
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/list_widgets.json",
        Some(mapping_json(json!({
            "request": {"method": "GET", "urlPath": "/widgets",
                "queryParameters": {"ids": {"hasExactly": [
                    {"equalTo": "w1"}, {"equalTo": "w2"}, {"equalTo": "w3"}
                ]}}},
            "response": {"status": 200, "jsonBody": {"widgets": []}}
        }))),
    )]);
    let obs = obligations(dir.path());
    let lap = ob(&obs, "serialization.list-argument-proof");
    assert!(matches!(lap.status, ObligationStatus::Fail), "{:#?}", lap);
}

#[test]
fn a_stub_at_a_different_endpoint_does_not_credit_proof_even_if_it_owns_the_case_by_tag() {
    // `serves()` decides case OWNERSHIP by name/tag; it says nothing about
    // whether the stub sits at this operation's own endpoint. Before the
    // fix, a same-case-tagged stub at a wholly different method/path could
    // supply proof no request to the real endpoint ever demonstrated.
    let dir = make_workspace(vec![
        (
            "tests/fixtures/mappings/update_widget_full.json",
            Some(mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {
                        "name": "Chair", "note": "see attached",
                        "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}]
                    }}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            }))),
        ),
        (
            "tests/fixtures/mappings/update_widget_full_decoy.json",
            Some(mapping_json(json!({
                "metadata": {"x-cases": ["update_widget_full"]},
                "request": {"method": "PATCH", "urlPath": "/never-hit",
                    "bodyPatterns": [{"equalToJson": {
                        "name": "Chair", "note": "see attached",
                        "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
                        "address": {"city": "NYC", "zip": "10001"}
                    }}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            }))),
        ),
    ]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(
        wba.gaps.iter().any(|g| g.message.contains("address")),
        "a stub at an unrelated endpoint must not credit proof for this operation: {:#?}",
        wba.gaps
    );
}

#[test]
fn template_demands_percent_decodes_the_actual_path_before_comparing() {
    // A correctly URL-encoded path segment ("a b" -> "a%20b") must still
    // match the plain (unencoded) value the case passed; comparing the raw
    // encoded bytes against the raw value text never agrees for anything
    // with a space or other reserved character (B6).
    assert!(template_demands(
        "/items/{$args.name}",
        &["name".to_string()],
        "a b",
        "/items/a%20b",
        true,
    ));
}

#[test]
fn a_header_entry_with_value_before_name_is_still_recognized() {
    // Field order inside a header object does not change its meaning; a
    // fixed-order regex requiring "name" before "value" made the argument
    // invisible to the wire map whenever a connector wrote it the other way.
    let field_text =
        r#"http: { POST: "/ping", headers: [{ value: "{$args.token}", name: "X-Token" }] }"#;
    let map = wire_map(field_text, &Wiring::default(), &HashSet::new());
    assert!(
        map.places
            .iter()
            .any(|(p, l)| p == &vec!["token".to_string()]
                && matches!(l, Location::Header(n, _) if n == "X-Token")),
        "{:#?}",
        map.places
    );
}

#[test]
fn a_comma_free_object_literal_is_parsed_structurally() {
    // GraphQL commas are optional between object fields too; the old
    // quote-keys-then-serde_json approach never inserted the commas JSON
    // requires, so this literal simply failed to parse (B6).
    let json = graphql_member_to_json(r#"{city: "NYC" zip: "10001"}"#).unwrap();
    assert_eq!(json["city"], "NYC");
    assert_eq!(json["zip"], "10001");
}

#[test]
fn a_bare_enum_literal_is_compared_as_its_json_string_wire_form() {
    // `name: signup` is a valid GraphQL enum value; the wire form is the
    // JSON string "signup", not an unparseable bare token (B6).
    let dir = make_workspace(vec![
        (
            "tests/cases/update_widget_full.graphql",
            s(concat!(
                "mutation { widget_co_updateWidget(",
                "id: \"w1\", name: signup, note: \"see attached\", ",
                "tags: [{key: \"color\", value: \"red\"}, {key: \"size\", value: {units: \"cm\", amount: 10}}], ",
                "address: {city: \"NYC\", zip: \"10001\"}",
                ") { id name } }\n"
            )),
        ),
        (
            "tests/fixtures/mappings/update_widget_full.json",
            Some(mapping_json(json!({
                "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                    "bodyPatterns": [{"equalToJson": {
                        "name": "signup", "note": "see attached",
                        "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
                        "address": {"city": "NYC", "zip": "10001"}
                    }}]},
                "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
            }))),
        ),
    ]);
    let obs = obligations(dir.path());
    let wba = ob(&obs, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Pass), "{:#?}", wba);
}

#[test]
fn call_arg_texts_registers_a_no_parens_call_with_no_arguments() {
    // A field with every argument optional is valid GraphQL called with no
    // parentheses at all; the pre-fix regex required `(` right after the
    // field name, so this call was invisible -- no omission or null proof
    // could ever see it (the seven Mailchimp false negatives).
    let doc = "mutation { widget_co_updateWidget { id name } }";
    let calls = call_arg_texts(doc, "widget_co_updateWidget");
    assert_eq!(calls, vec![Vec::<(String, String)>::new()]);
}

#[test]
fn list_elements_text_does_not_split_a_comma_inside_a_quoted_element() {
    // One string element containing a literal comma is one element, not
    // two: a comma-blind splitter over the raw text (rather than a
    // blanked/bracket-aware, value-boundary scan) reads the comma inside
    // the quotes as a top-level separator.
    let elements = list_elements_text(r#"["red,blue"]"#);
    assert_eq!(elements, vec![r#""red,blue""#.to_string()]);
}

#[test]
fn list_elements_text_splits_a_whitespace_only_list_with_no_commas() {
    // GraphQL commas are optional between list elements too.
    let elements = list_elements_text(r#"["a" "b"]"#);
    assert_eq!(elements, vec!["\"a\"".to_string(), "\"b\"".to_string()]);
}

#[test]
fn a_query_list_matcher_must_match_exactly_not_by_substring_containment() {
    // want = ["1", "2"]; a joined "12" contains both substrings but is not
    // an equality proof of the two-element array.
    let dir = make_workspace(vec![
        (
            "tests/cases/list_widgets.graphql",
            s("query { widget_co_listWidgets(ids: [\"1\", \"2\"]) { id name } }\n"),
        ),
        (
            "tests/fixtures/mappings/list_widgets.json",
            Some(mapping_json(json!({
                "request": {"method": "GET", "urlPath": "/widgets",
                    "queryParameters": {"ids": {"equalTo": "12"}}},
                "response": {"status": 200, "jsonBody": {"widgets": []}}
            }))),
        ),
    ]);
    let obs = obligations(dir.path());
    let lap = ob(&obs, "serialization.list-argument-proof");
    assert!(matches!(lap.status, ObligationStatus::Fail), "{:#?}", lap);
}

#[test]
fn a_nullable_container_still_requires_omission_or_null_proof_not_a_silent_skip() {
    // A sub-selected ("container") optional input argument has no single
    // whole-argument body pointer -- before the fix, that alone made the
    // omission/null guards skip it entirely, no matter how it was (or was
    // not) tested.
    let sdl = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.14", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.4", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

input Widget_Co_ContactInput {
  name: String
  zip: String
}

type Widget_Co_Widget {
  id: ID
}

type Mutation {
  "Update a contact."
  widget_co_updateContact(id: ID!, contact: Widget_Co_ContactInput): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        PATCH: "/contacts/{$args.id}"
        body: "contact: $args.contact { Name: name Zip: zip }"
      }
      selection: "id"
    )
}
"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".factory/evidence/runs/2026-09-17")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests/cases")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests/fixtures/mappings")).unwrap();
    std::fs::write(dir.path().join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        dir.path().join(".factory/workspace.yaml"),
        "directory: widget-co\nfield_prefix: widget_co\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/selection.yaml"),
        "operations:\n  \"patch:/contacts/{id}\":\n    include: true\n    graphql:\n      root: mutation\n      name: updateContact\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/inventory.json"),
        crate::json::pretty(&json!({
            "operations": [{"key": "patch:/contacts/{id}", "method": "PATCH", "path": "/contacts/{id}"}],
            "shapes": {}
        })),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/evidence/latest.json"),
        crate::json::pretty(&json!({
            "layers": {"wiremock_e2e": {"status": "pass", "log": ".factory/evidence/runs/2026-09-17/e2e.log"}}
        })),
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".factory/evidence/runs/2026-09-17/e2e.log"),
        "PASS: update_contact_full\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tests/cases/update_contact_full.graphql"),
        "mutation { widget_co_updateContact(id: \"c1\", contact: {name: \"A\", zip: \"1\"}) { id } }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path()
            .join("tests/fixtures/mappings/update_contact_full.json"),
        mapping_json(json!({
            "request": {"method": "PATCH", "urlPath": "/contacts/c1",
                "bodyPatterns": [{"equalToJson": {"contact": {"Name": "A", "Zip": "1"}}}]},
            "response": {"status": 200, "jsonBody": {"id": "c1"}}
        })),
    )
    .unwrap();
    // No omitting case, and no null_handling decision or default anywhere.
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(
        mc.gaps
            .iter()
            .any(|g| g.kind == GapKind::OmissionUnproven || g.kind == GapKind::NullHandlingUndecided),
        "a container argument must still be checked for omission/null proof, not silently skipped: {:#?}",
        mc.gaps
    );
}

#[test]
fn a_full_case_split_across_aliased_calls_does_not_satisfy_the_full_case_requirement() {
    // Two aliased calls to the same field, in the same document, together
    // touch every argument -- but neither call, on its own, supplies all of
    // them. Unioning across calls in the same file (the pre-fix behavior)
    // would make this look like one full case; it must not.
    let dir = make_workspace(vec![(
        "tests/cases/update_widget_full.graphql",
        s(concat!(
            "mutation {\n",
            "  one: widget_co_updateWidget(id: \"w1\", name: \"Chair\", note: \"see attached\") { id name }\n",
            "  two: widget_co_updateWidget(id: \"w1\", tags: [{key: \"color\", value: \"red\"}, {key: \"size\", value: {units: \"cm\", amount: 10}}], address: {city: \"NYC\", zip: \"10001\"}) { id name }\n",
            "}\n"
        )),
    )]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(
        mc.gaps.iter().any(|g| g.kind == GapKind::FullCaseMissing),
        "{:#?}",
        mc.gaps
    );
}

#[test]
fn a_map_variant_with_no_executed_pass_line_is_not_proven_even_though_the_case_file_exists() {
    // Only `update_widget_full` supplies 2 distinct-keyed tags entries (one
    // of them nested); dropping its PASS line, alone, must reopen both
    // variants -- no other case backstops them.
    let dir = make_workspace(vec![(
        ".factory/evidence/runs/2026-09-17/e2e.log",
        Some(log_without("update_widget_full")),
    )]);
    let obs = obligations(dir.path());
    let mc = ob(&obs, "serialization.mutation-cases");
    assert!(matches!(mc.status, ObligationStatus::Fail), "{:#?}", mc);
    assert!(
        mc.gaps.iter().any(|g| g.kind == GapKind::MapCaseMissing
            && g.message.contains("several entries with distinct keys")),
        "{:#?}",
        mc.gaps
    );
    assert!(
        mc.gaps.iter().any(|g| g.kind == GapKind::MapCaseMissing
            && g.message.contains("a value needing its own nested typing")),
        "{:#?}",
        mc.gaps
    );
}

// ── Same-run evidence contract (ADR 0079 Step 2) ────────────────────────────
// `report_with_evidence` must never open `.factory/evidence/latest.json`
// itself; a caller mid-way through its own evidence run passes its own
// current-run results instead.

#[test]
fn a_clean_first_run_with_no_latest_json_on_disk_produces_a_correct_report() {
    let dir = make_workspace(vec![(".factory/evidence/latest.json", None)]);
    assert!(!dir.path().join(".factory/evidence/latest.json").exists());
    let evidence = json!({
        "layers": {"wiremock_e2e": {"status": "pass", "log": ".factory/evidence/runs/2026-09-17/e2e.log"}}
    });
    let rep = report_with_evidence(dir.path(), Some(&evidence));
    let wba = ob(&rep.obligations, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Pass), "{:#?}", wba);
}

#[test]
fn a_second_run_reflects_the_current_bodies_not_a_stale_pass_carried_from_disk() {
    let dir = make_workspace(vec![(".factory/evidence/latest.json", None)]);
    let evidence = json!({
        "layers": {"wiremock_e2e": {"status": "pass", "log": ".factory/evidence/runs/2026-09-17/e2e.log"}}
    });

    // First run: everything passes.
    let rep1 = report_with_evidence(dir.path(), Some(&evidence));
    let wba1 = ob(&rep1.obligations, "serialization.write-body-assertions");
    assert!(matches!(wba1.status, ObligationStatus::Pass), "{:#?}", wba1);

    // Second "run" in the same test, same evidence value and log path --
    // no latest.json is ever written to disk between the two -- but the
    // full case's own stub now drops `address` from its demanded body.
    std::fs::write(
        dir.path().join("tests/fixtures/mappings/update_widget_full.json"),
        mapping_json(json!({
            "request": {"method": "PATCH", "urlPath": "/widgets/w1",
                "bodyPatterns": [{"equalToJson": {
                    "name": "Chair", "note": "see attached",
                    "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}]
                }}]},
            "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
        })),
    )
    .unwrap();
    let rep2 = report_with_evidence(dir.path(), Some(&evidence));
    let wba2 = ob(&rep2.obligations, "serialization.write-body-assertions");
    assert!(matches!(wba2.status, ObligationStatus::Fail), "{:#?}", wba2);
    assert!(
        wba2.gaps.iter().any(|g| g.message.contains("address")),
        "{:#?}",
        wba2.gaps
    );
}

// ── Custody: the schema path is not an ordinary read (ADR 0025) ────────────

#[test]
fn a_symlinked_schema_via_a_hand_edited_directory_is_refused_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".factory")).unwrap();
    std::fs::write(
        dir.path().join(".factory/workspace.yaml"),
        "directory: .factory/probe\nfield_prefix: widget_co\n",
    )
    .unwrap();
    std::fs::write(dir.path().join(".factory/selection.yaml"), SELECTION).unwrap();
    std::fs::write(
        dir.path().join(".factory/inventory.json"),
        crate::json::pretty(&inventory()),
    )
    .unwrap();
    // The real schema sits OUTSIDE `.factory/`; `directory` names a path
    // INSIDE it, and a symlink planted there resolves back out -- exactly
    // the escape ADR 0025's custody boundary exists to refuse.
    std::fs::write(dir.path().join("widget-co.graphql"), SDL).unwrap();
    std::os::unix::fs::symlink(
        dir.path().join("widget-co.graphql"),
        dir.path().join(".factory/probe.graphql"),
    )
    .unwrap();
    let rep = report(dir.path());
    assert!(
        rep.obligations
            .iter()
            .all(|o| matches!(o.status, ObligationStatus::Unexecuted)),
        "the symlinked schema must be refused, not silently followed: {:#?}",
        rep.obligations
    );
}

// ── Fixed body values and argument-less writes (Codex review of #126, B1) ──

/// The baseline workspace plus two extra mutations: one with a path argument
/// and a block-string constant body, one with no arguments and a constant
/// body. `stub_bodies` are the `equalToJson` bodies of their stubs (`None`:
/// the stub asserts no body at all).
fn constant_body_workspace(
    path_stub_body: Option<Value>,
    bare_stub_body: Option<Value>,
    with_bare_case: bool,
) -> tempfile::TempDir {
    let sdl = format!(
        "{}\n  widget_co_createWidget(id: ID!): Widget_Co_Widget\n    @connect(\n      source: \"widget_co\"\n      http: {{\n        POST: \"/widgets/{{$args.id}}/create\"\n        body: \"\"\"\n        name: $(\"WRONG\")\n        \"\"\"\n      }}\n      selection: \"id name\"\n    )\n  widget_co_ping: Widget_Co_Widget\n    @connect(\n      source: \"widget_co\"\n      http: {{ POST: \"/ping\", body: \"\"\"\n        kind: $(\"fixed\")\n        \"\"\" }}\n      selection: \"id name\"\n    )\n}}\n",
        SDL.trim_end().strip_suffix('}').unwrap()
    );
    let selection = format!(
        "{}  \"post:/widgets/{{id}}/create\":\n    include: true\n    graphql:\n      root: mutation\n      name: createWidget\n  \"post:/ping\":\n    include: true\n    graphql:\n      root: mutation\n      name: ping\n",
        SELECTION
    );
    let stub = |url: &str, body: Option<Value>| {
        let mut request = json!({"method": "POST", "urlPath": url});
        if let Some(b) = body {
            request["bodyPatterns"] = json!([{"equalToJson": b}]);
        }
        mapping_json(
            json!({"request": request, "response": {"status": 200, "jsonBody": {"id": "w1"}}}),
        )
    };
    let mut log = default_log();
    log.push_str("PASS: create_widget\n");
    let mut overrides: Vec<(&str, Option<String>)> = vec![
        ("widget-co.graphql", s(&sdl)),
        (".factory/selection.yaml", s(&selection)),
        (".factory/evidence/runs/2026-09-17/e2e.log", Some(log)),
        (
            "tests/cases/create_widget.graphql",
            s("mutation { widget_co_createWidget(id: \"w1\") { id } }\n"),
        ),
        (
            "tests/fixtures/mappings/create_widget.json",
            Some(stub("/widgets/w1/create", path_stub_body)),
        ),
    ];
    if with_bare_case {
        let mut log = default_log();
        log.push_str("PASS: create_widget\nPASS: ping\n");
        overrides.push((".factory/evidence/runs/2026-09-17/e2e.log", Some(log)));
        overrides.push((
            "tests/cases/ping.graphql",
            s("mutation { widget_co_ping { id } }\n"),
        ));
        overrides.push((
            "tests/fixtures/mappings/ping.json",
            Some(stub("/ping", bare_stub_body)),
        ));
    }
    make_workspace(overrides)
}

fn gaps_of<'a>(rep: &'a Report, op: &str) -> Vec<&'a Gap> {
    rep.writes
        .iter()
        .filter(|w| w.operation == op)
        .flat_map(|w| w.gaps.iter())
        .collect()
}

#[test]
fn a_block_string_fixed_body_value_needs_an_assertion() {
    // The connector sends {"name": "WRONG"}; the stub asserts no body, so
    // e2e passes whatever the body is. Before the fix the literal counted as
    // mapped and the write was body_proven.
    let dir = constant_body_workspace(None, None, true);
    let rep = report(dir.path());
    let gaps = gaps_of(&rep, "post:/widgets/{id}/create");
    assert!(
        gaps.iter()
            .any(|g| g.kind == GapKind::FixedValueUnasserted && g.message.contains("\"WRONG\"")),
        "{:#?}",
        rep.writes
    );
    let write = rep
        .writes
        .iter()
        .find(|w| w.operation == "post:/widgets/{id}/create")
        .unwrap();
    assert!(!write.body_proven, "{:#?}", write);

    // A stub that demands the value at its pointer proves it.
    let proven = constant_body_workspace(Some(json!({"name": "WRONG"})), None, true);
    let rep = report(proven.path());
    assert!(
        gaps_of(&rep, "post:/widgets/{id}/create").is_empty(),
        "{:#?}",
        rep.writes
    );

    // A stub that demands a different value does not.
    let wrong = constant_body_workspace(Some(json!({"name": "fixed"})), None, true);
    let rep = report(wrong.path());
    assert!(
        gaps_of(&rep, "post:/widgets/{id}/create")
            .iter()
            .any(|g| g.kind == GapKind::FixedValueUnasserted),
        "{:#?}",
        rep.writes
    );
}

#[test]
fn a_write_with_no_arguments_is_still_reported() {
    // `widget_co_ping` has no arguments, so it was missing from
    // `root_field_args` and dropped from the report: no gap, layer pass.
    let dir = constant_body_workspace(Some(json!({"name": "WRONG"})), None, false);
    let rep = report(dir.path());
    assert!(
        rep.writes.iter().any(|w| w.operation == "post:/ping"),
        "an argument-less write must be in the report: {:#?}",
        rep.writes
    );
    let ping = gaps_of(&rep, "post:/ping");
    assert!(
        ping.iter().any(|g| g.kind == GapKind::FixedValueUnasserted),
        "no case at all for a constant-body write is a gap: {:#?}",
        rep.writes
    );

    // With its case and a stub demanding the body, it is proven and its
    // case appears in the per-case map.
    let proven = constant_body_workspace(
        Some(json!({"name": "WRONG"})),
        Some(json!({"kind": "fixed"})),
        true,
    );
    let rep = report(proven.path());
    assert!(gaps_of(&rep, "post:/ping").is_empty(), "{:#?}", rep.writes);
    assert_eq!(
        rep.case_proofs.get("ping").map(|p| p.verdict.as_str()),
        Some("pass"),
        "{:#?}",
        rep.case_proofs
    );
}

#[test]
fn a_failed_case_is_reported_failed_even_when_the_layer_failed() {
    // The aggregate layer status is `fail`; the case's own record is what
    // the per-case map reports (ADR 0079 Step 2), not "unproven".
    let (mut ev, _) = evidence_with_log("");
    ev["layers"]["wiremock_e2e"]["status"] = json!("fail");
    let mut log = default_log();
    log = log.replace("PASS: update_widget_full\n", "FAIL: update_widget_full\n");
    let dir = make_workspace(vec![
        (
            ".factory/evidence/latest.json",
            s(&crate::json::pretty(&ev)),
        ),
        (".factory/evidence/runs/2026-09-17/e2e.log", Some(log)),
    ]);
    let rep = report(dir.path());
    let verdict = |c: &str| rep.case_proofs.get(c).map(|p| p.verdict.clone());
    assert_eq!(
        verdict("update_widget_full").as_deref(),
        Some("fail"),
        "{:#?}",
        rep.case_proofs
    );
    assert_eq!(
        verdict("update_widget_required").as_deref(),
        Some("pass"),
        "{:#?}",
        rep.case_proofs
    );
    // A case that passed still cannot carry an obligation when the layer
    // failed.
    assert!(matches!(
        case_executed(
            dir.path(),
            Some(&ev),
            "wiremock_e2e",
            "update_widget_required"
        ),
        CaseExec::NotRecorded(_)
    ),);
}

#[test]
fn write_and_read_gaps_are_counted_apart() {
    // Every write is proven; only the read's `ids` query argument is not
    // (its stub demands no query). The layer still fails, but the summary
    // says there is no write gap.
    let weak_read = mapping_json(json!({
        "request": {"method": "GET", "urlPath": "/widgets"},
        "response": {"status": 200, "jsonBody": {"widgets": []}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/list_widgets.json",
        s(&weak_read),
    )]);
    let rep = report(dir.path());
    assert_eq!(rep.write_gaps, 0, "{:#?}", rep.writes);
    assert!(rep.read_gaps >= 1, "{:#?}", rep.obligations);
    assert!(
        rep.obligations
            .iter()
            .any(|o| matches!(o.status, ObligationStatus::Fail)),
        "{:#?}",
        rep.obligations
    );

    // A weak write stub counts as a write gap.
    let weak_write = mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1"},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full.json",
        s(&weak_write),
    )]);
    let rep = report(dir.path());
    assert!(rep.write_gaps >= 1, "{:#?}", rep.writes);
    assert_eq!(rep.read_gaps, 0, "{:#?}", rep.obligations);
}

#[test]
fn an_optional_body_member_written_with_a_trailing_question_mark_is_placed_like_any_other() {
    // `note: $args.note?` drops the key on null; it is the same argument at
    // the same pointer, so the baseline stays fully proven.
    let sdl = SDL.replace("note: $args.note\n", "note: $args.note?\n");
    assert_ne!(sdl, SDL);
    let dir = make_workspace(vec![("widget-co.graphql", s(&sdl))]);
    let rep = report(dir.path());
    assert!(
        rep.writes.iter().all(|w| w.body_proven),
        "{:#?}",
        rep.writes
    );
    assert_eq!(rep.write_gaps, 0, "{:#?}", rep.obligations);
}

#[test]
fn a_source_body_member_a_resolved_decision_omits_is_not_a_gap() {
    // The source documents a `legacy` body member no argument feeds.
    let mut inv = inventory();
    inv["shapes"]["UpdateWidgetRequest"]["properties"]["legacy"] = json!({"type": "string"});
    let with_inventory = |decisions: &str| {
        make_workspace(vec![
            (".factory/inventory.json", s(&crate::json::pretty(&inv))),
            (".factory/decisions.json", s(decisions)),
        ])
    };
    let unmapped = |rep: &Report| {
        rep.writes
            .iter()
            .flat_map(|w| w.gaps.iter())
            .any(|g| g.kind == GapKind::SourceMemberUnmapped && g.message.contains("legacy"))
    };
    let record = |status: &str| {
        DECISIONS.replace(
            "\"null_handling\": [",
            &format!(
                "\"omits\": [{{\"operation\": \"patch:/widgets/{{id}}\", \"direction\": \"request\", \"path\": \"legacy\", \"reason\": \"editorial\"}}],\n      \"null_handling\": ["
            ),
        )
        .replace("\"status\": \"resolved\"", &format!("\"status\": \"{}\"", status))
    };

    // No omit record: the gap stands.
    let dir = with_inventory(DECISIONS);
    assert!(unmapped(&report(dir.path())));
    // An open record does not count.
    let dir = with_inventory(&record("open"));
    assert!(unmapped(&report(dir.path())));
    // A resolved record does.
    let dir = with_inventory(&record("resolved"));
    assert!(
        !unmapped(&report(dir.path())),
        "{:#?}",
        report(dir.path()).writes
    );
}

// ── A resolved omit never excuses a member the source requires ──────────────

fn omit_workspace(required: bool, path: &str, status: &str) -> tempfile::TempDir {
    let mut inv = inventory();
    inv["shapes"]["UpdateWidgetRequest"]["properties"]["legacy"] = json!({"type": "string"});
    if required {
        inv["shapes"]["UpdateWidgetRequest"]["required"] = json!(["legacy"]);
    } else {
        // Something else is required, and the connector sends it.
        inv["shapes"]["UpdateWidgetRequest"]["required"] = json!(["name"]);
    }
    let decisions = DECISIONS
        .replace(
            "\"null_handling\": [",
            &format!(
                "\"omits\": [{{\"operation\": \"patch:/widgets/{{id}}\", \"direction\": \"request\", \"path\": \"{}\", \"reason\": \"editorial\"}}],\n      \"null_handling\": [",
                path
            ),
        )
        .replace("\"status\": \"resolved\"", &format!("\"status\": \"{}\"", status));
    make_workspace(vec![
        (".factory/inventory.json", s(&crate::json::pretty(&inv))),
        (".factory/decisions.json", s(&decisions)),
    ])
}

fn member_gaps(rep: &Report, member: &str) -> Vec<(GapKind, String)> {
    rep.writes
        .iter()
        .flat_map(|w| w.gaps.iter())
        .filter(|g| g.message.contains(&format!("\"{}\"", member)))
        .map(|g| (g.kind, g.message.clone()))
        .collect()
}

#[test]
fn a_resolved_omit_does_not_excuse_a_required_source_member() {
    let rep = report(omit_workspace(true, "legacy", "resolved").path());
    let gaps = member_gaps(&rep, "legacy");
    assert_eq!(gaps.len(), 1, "{:#?}", rep.writes);
    assert_eq!(gaps[0].0, GapKind::RequiredMemberOmitted);
    assert!(
        gaps[0].1.contains("D-0001"),
        "names the decision: {}",
        gaps[0].1
    );
    assert!(rep.writes.iter().any(|w| !w.body_proven));
}

#[test]
fn a_root_omit_does_not_excuse_a_required_source_member() {
    let rep = report(omit_workspace(true, ".", "resolved").path());
    let gaps = member_gaps(&rep, "legacy");
    assert_eq!(gaps.len(), 1, "{:#?}", rep.writes);
    assert_eq!(gaps[0].0, GapKind::RequiredMemberOmitted);
}

#[test]
fn a_resolved_omit_still_excuses_an_optional_source_member() {
    for path in ["legacy", "."] {
        let rep = report(omit_workspace(false, path, "resolved").path());
        assert!(
            member_gaps(&rep, "legacy").is_empty(),
            "{}: {:#?}",
            path,
            rep.writes
        );
        assert_eq!(rep.write_gaps, 0, "{}: {:#?}", path, rep.obligations);
    }
}

#[test]
fn a_reopened_omit_does_not_count_for_a_required_member_either() {
    let rep = report(omit_workspace(true, "legacy", "open").path());
    let gaps = member_gaps(&rep, "legacy");
    assert_eq!(gaps.len(), 1, "{:#?}", rep.writes);
    assert_eq!(gaps[0].0, GapKind::SourceMemberUnmapped);
}

// ── Executed placement: a mapping the reader cannot parse is proven by ─────
// ── an executed, passing case whose stub demands the value at one place ────

const CHAIN: &str = "note: $args.note->map({ text: @, kind: \"n\" })->first";

fn chained_sdl() -> String {
    let sdl = SDL.replacen("note: $args.note\n", &format!("{}\n", CHAIN), 1);
    assert_ne!(sdl, SDL);
    sdl
}

fn patch_stub(body: Value) -> Option<String> {
    s(&mapping_json(json!({
        "request": {"method": "PATCH", "urlPath": "/widgets/w1",
            "bodyPatterns": [{"equalToJson": body}]},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    })))
}

fn full_body(note: Value) -> Value {
    json!({
        "name": "Chair", "note": note,
        "tags": [{"key": "color", "value": "red"}, {"key": "size", "value": {"units": "cm", "amount": 10}}],
        "address": {"city": "NYC", "zip": "10001"}
    })
}

/// `note` written through a method chain the reader does not place, with the
/// stubs a router really answers to it: the note wrapped, an omitted or null
/// note dropped from the body. `decisions` says `omit` for it.
fn chained_workspace(mut overrides: Vec<(&str, Option<String>)>) -> tempfile::TempDir {
    let decisions = DECISIONS.replace(
        "\"argument\": \"note\", \"behavior\": \"send_null\"",
        "\"argument\": \"note\", \"behavior\": \"omit\"",
    );
    let mut base: Vec<(&str, Option<String>)> = vec![
        ("widget-co.graphql", Some(chained_sdl())),
        (".factory/decisions.json", Some(decisions)),
        (
            "tests/fixtures/mappings/update_widget_full.json",
            patch_stub(full_body(json!({"text": "see attached", "kind": "n"}))),
        ),
        (
            "tests/fixtures/mappings/update_widget_note_null.json",
            patch_stub(json!({})),
        ),
    ];
    base.append(&mut overrides);
    // Later entries win: dedupe by path keeping the last.
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<(&str, Option<String>)> = Vec::new();
    for (p, c) in base.into_iter().rev() {
        if seen.insert(p) {
            deduped.push((p, c));
        }
    }
    make_workspace(deduped)
}

fn gaps_for<'a>(rep: &'a Report, arg: &str) -> Vec<&'a Gap> {
    rep.obligations
        .iter()
        .flat_map(|o| o.gaps.iter())
        .filter(|g| {
            g.message.contains(&format!("({}", arg)) || g.message.contains(&format!(" {} ", arg))
        })
        .collect()
}

#[test]
fn executed_placement_proves_a_method_chain_mapping_from_one_passing_case() {
    let dir = chained_workspace(vec![]);
    let rep = report(dir.path());
    assert_eq!(rep.write_gaps, 0, "{:#?}", rep.obligations);
    let p: Vec<&Placement> = rep
        .placements
        .iter()
        .filter(|p| p.operation == "patch:/widgets/{id}" && p.argument == "note")
        .collect();
    assert_eq!(p.len(), 1, "{:#?}", rep.placements);
    assert_eq!(p[0].via, "executed");
    assert_eq!(p[0].location, "body /note/text");
    assert_eq!(p[0].case.as_deref(), Some("update_widget_full"));
    // The reader's own placements are recorded as static.
    assert!(rep
        .placements
        .iter()
        .any(|p| p.argument == "name" && p.via == "static" && p.case.is_none()));
}

#[test]
fn executed_placement_needs_the_proving_case_to_have_run() {
    let (ev, log) = evidence_with_log(&log_without("update_widget_full"));
    let dir = chained_workspace(vec![
        (
            ".factory/evidence/latest.json",
            s(&crate::json::pretty(&ev)),
        ),
        (".factory/evidence/runs/2026-09-17/e2e.log", Some(log)),
    ]);
    let rep = report(dir.path());
    let note = gaps_for(&rep, "note");
    assert!(
        note.iter().any(|g| g.kind == GapKind::UnprovenMapping),
        "{:#?}",
        rep.obligations
    );
    assert!(rep.placements.iter().all(|p| p.argument != "note"));
}

#[test]
fn executed_placement_reports_a_value_at_two_pointers_as_ambiguous_value() {
    let dir = chained_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full.json",
        patch_stub(full_body(
            json!({"text": "see attached", "kind": "see attached"}),
        )),
    )]);
    let rep = report(dir.path());
    let note = gaps_for(&rep, "note");
    assert_eq!(note.len(), 1, "{:#?}", rep.obligations);
    assert_eq!(note[0].kind, GapKind::AmbiguousValue);
    assert!(note[0].message.contains("2 places"), "{}", note[0].message);
    assert!(
        note[0].message.contains("distinctive value"),
        "{}",
        note[0].message
    );
}

#[test]
fn executed_placement_reports_a_too_common_value_as_ambiguous_value() {
    let dir = chained_workspace(vec![
        (
            "tests/cases/update_widget_full.graphql",
            s(&default_cases()[3].1.replace("see attached", "ab")),
        ),
        (
            "tests/fixtures/mappings/update_widget_full.json",
            patch_stub(full_body(json!({"text": "ab", "kind": "n"}))),
        ),
    ]);
    let rep = report(dir.path());
    let note = gaps_for(&rep, "note");
    assert_eq!(note.len(), 1, "{:#?}", rep.obligations);
    assert_eq!(note[0].kind, GapKind::AmbiguousValue);
    assert!(
        note[0].message.contains("too common"),
        "{}",
        note[0].message
    );
}

#[test]
fn executed_placement_reports_a_value_another_argument_shares_as_ambiguous_value() {
    let dir = chained_workspace(vec![
        (
            "tests/cases/update_widget_full.graphql",
            s(&default_cases()[3]
                .1
                .replace("name: \"Chair\"", "name: \"see attached\"")),
        ),
        (
            "tests/fixtures/mappings/update_widget_full.json",
            patch_stub({
                let mut b = full_body(json!({"text": "see attached", "kind": "n"}));
                b["name"] = json!("see attached");
                b
            }),
        ),
    ]);
    let rep = report(dir.path());
    let note = gaps_for(&rep, "note");
    assert_eq!(note.len(), 1, "{:#?}", rep.obligations);
    assert_eq!(note[0].kind, GapKind::AmbiguousValue);
    assert!(
        note[0].message.contains("argument name"),
        "{}",
        note[0].message
    );
}

#[test]
fn executed_placement_proves_omission_only_behind_an_absent_key_assertion() {
    // The required-only case's stub demands the note at its place: the key is
    // not absent, so omission is unproven.
    // Every case that leaves the note out demands it present.
    let noted = |b: Value| {
        let mut b = b;
        b["note"] = json!({"text": "see attached"});
        patch_stub(b)
    };
    let dir = chained_workspace(vec![
        (
            "tests/fixtures/mappings/update_widget_required.json",
            noted(json!({})),
        ),
        (
            "tests/fixtures/mappings/update_widget_tags_empty.json",
            noted(json!({"tags": []})),
        ),
        (
            "tests/fixtures/mappings/update_widget_tags_one.json",
            noted(json!({"tags": [{"key": "color", "value": "red"}]})),
        ),
        (
            "tests/fixtures/mappings/update_widget_tags_badkey.json",
            noted(json!({"tags": [{"key": "non-graphql-name", "value": "x"}]})),
        ),
        (
            "tests/fixtures/mappings/update_widget_nulls.json",
            noted(json!({"name": null, "tags": null, "address": null})),
        ),
        (
            "tests/fixtures/mappings/update_widget_note_null.json",
            noted(json!({})),
        ),
    ]);
    let rep = report(dir.path());
    assert!(
        rep.obligations
            .iter()
            .flat_map(|o| o.gaps.iter())
            .any(|g| g.kind == GapKind::OmissionUnproven && g.message.contains("note")),
        "{:#?}",
        rep.obligations
    );
    // With the key asserted absent (the baseline), it is proven.
    let ok = report(chained_workspace(vec![]).path());
    assert!(ok
        .obligations
        .iter()
        .flat_map(|o| o.gaps.iter())
        .all(|g| !g.message.contains("note")));
}

#[test]
fn executed_placement_holds_a_decision_to_what_the_executed_cases_show() {
    // The decision says send_null; the case passing null shows the key
    // dropped. A mismatch is a gap, not a pass.
    let send_null = DECISIONS.to_string();
    let dir = chained_workspace(vec![(".factory/decisions.json", Some(send_null))]);
    let rep = report(dir.path());
    let g: Vec<&Gap> = gaps_for(&rep, "note");
    assert_eq!(g.len(), 1, "{:#?}", rep.obligations);
    assert_eq!(g[0].kind, GapKind::NullUnproven);
    assert!(
        g[0].message.contains("update_widget_note_null"),
        "{}",
        g[0].message
    );
}

#[test]
fn an_absent_argument_behind_a_stub_demanding_null_proves_send_null() {
    let send_null = DECISIONS.to_string();
    let dir = chained_workspace(vec![
        (".factory/decisions.json", Some(send_null)),
        ("tests/cases/update_widget_note_null.graphql", None),
        (
            "tests/fixtures/mappings/update_widget_required.json",
            patch_stub(json!({"note": {"text": null}})),
        ),
    ]);
    // The null case file is gone; drop it from the on-disk workspace.
    std::fs::remove_file(
        dir.path()
            .join("tests/cases/update_widget_note_null.graphql"),
    )
    .ok();
    let rep = report(dir.path());
    assert_eq!(rep.write_gaps, 0, "{:#?}", rep.obligations);
}

#[test]
fn executed_placement_never_reads_the_expression_text() {
    // Three spellings of the same wire behavior, one the reader could never
    // parse: the executed proof, and where it says the value went, is the same.
    let spellings = [
        CHAIN.to_string(),
        "note: $args.note->match([null, null], [@, { text: @, kind: \"n\" }])".to_string(),
        "note: $args.note->as($zz)->map({ text: $zz, kind: \"n\" })->first->map(@)->first"
            .to_string(),
    ];
    let mut seen = Vec::new();
    for spelling in &spellings {
        let sdl = SDL.replacen("note: $args.note\n", &format!("{}\n", spelling), 1);
        let dir = chained_workspace(vec![("widget-co.graphql", Some(sdl))]);
        let rep = report(dir.path());
        assert_eq!(rep.write_gaps, 0, "{}: {:#?}", spelling, rep.obligations);
        let p = rep
            .placements
            .iter()
            .find(|p| p.argument == "note")
            .unwrap_or_else(|| panic!("{}: {:#?}", spelling, rep.placements));
        seen.push((p.via.clone(), p.location.clone(), p.case.clone()));
    }
    assert!(seen.windows(2).all(|w| w[0] == w[1]), "{:?}", seen);
    assert_eq!(seen[0].0, "executed");
}

#[test]
fn executed_placement_proves_each_list_element_under_one_array() {
    let sdl = SDL.replacen(
        "tags: $args.tags\n",
        "tags: $args.tags->map({ label: @ })\n",
        1,
    );
    assert_ne!(sdl, SDL);
    let dir = make_workspace(vec![
        ("widget-co.graphql", Some(sdl)),
        (
            "tests/fixtures/mappings/search_widgets_full.json",
            s(&mapping_json(json!({
                "request": {"method": "POST", "urlPath": "/widgets/search",
                    "bodyPatterns": [{"equalToJson": {"term": "chair", "tags": [{"label": "red"}, {"label": "blue"}]}}]},
                "response": {"status": 200, "jsonBody": {"results": []}}
            }))),
        ),
    ]);
    let rep = report(dir.path());
    assert_eq!(rep.write_gaps, 0, "{:#?}", rep.obligations);
    let p = rep
        .placements
        .iter()
        .find(|p| p.operation == "post:/widgets/search" && p.argument == "tags")
        .unwrap_or_else(|| panic!("{:#?}", rep.placements));
    assert_eq!(p.via, "executed");
    assert_eq!(p.location, "body elements /tags/*/label");
}

// ── Review of #156: stubs that could answer any request, negative cases,
//    and input the scanners must not panic on ──────────────────────────────

/// The full-case arguments, as `update_widget_full.graphql` passes them.
const FULL_CALL: &str = concat!(
    "mutation { widget_co_updateWidget(",
    "id: \"w1\", name: \"Chair\", note: \"see attached\", ",
    "tags: [{key: \"color\", value: \"red\"}, {key: \"size\", value: {units: \"cm\", amount: 10}}], ",
    "address: {city: \"NYC\", zip: \"10001\"}",
    ") { id name } }\n"
);

/// A passing case shows only that SOME stub it owns matched. When a sibling
/// stub at the same endpoint would answer any body (no body pattern, a loose
/// one) or might answer this request at all (a URL pattern, no URL, an `ANY`
/// method), the exact stub's demand proves nothing: the connector could
/// have sent any body and the case would still pass.
#[test]
fn a_sibling_stub_that_could_answer_any_request_disqualifies_the_proof() {
    let siblings = [
        (
            "no body pattern",
            json!({"method": "PATCH", "urlPath": "/widgets/w1"}),
        ),
        (
            "a loose body pattern",
            json!({"method": "PATCH", "urlPath": "/widgets/w1",
                "bodyPatterns": [{"equalToJson": {}, "ignoreExtraElements": true}]}),
        ),
        (
            "a URL path pattern",
            json!({"method": "PATCH", "urlPathPattern": "/widgets/.*",
                "bodyPatterns": [{"equalToJson": {"name": "Chair"}}]}),
        ),
        (
            "an ANY method",
            json!({"method": "ANY", "urlPath": "/widgets/w1",
                "bodyPatterns": [{"equalToJson": {"name": "Chair"}}]}),
        ),
    ];
    for (label, request) in siblings {
        let fallback = mapping_json(json!({
            "metadata": {"x-cases": ["update_widget_full"]},
            "request": request,
            "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
        }));
        let dir = make_workspace(vec![(
            "tests/fixtures/mappings/update_widget_full_fallback.json",
            Some(fallback),
        )]);
        let wba = ob(
            &obligations(dir.path()),
            "serialization.write-body-assertions",
        )
        .clone();
        assert!(
            matches!(wba.status, ObligationStatus::Fail)
                && wba
                    .gaps
                    .iter()
                    .any(|g| g.kind == GapKind::ArgumentUnasserted
                        && g.message.contains("updateWidget(note)")),
            "{}: {:#?}",
            label,
            wba
        );
    }
    // A sibling at another method is not a candidate: the proof stands.
    let other = mapping_json(json!({
        "metadata": {"x-cases": ["update_widget_full"]},
        "request": {"method": "GET", "urlPath": "/widgets/w1"},
        "response": {"status": 200, "jsonBody": {"id": "w1", "name": "Chair"}}
    }));
    let dir = make_workspace(vec![(
        "tests/fixtures/mappings/update_widget_full_lookup.json",
        Some(other),
    )]);
    let wba = ob(
        &obligations(dir.path()),
        "serialization.write-body-assertions",
    )
    .clone();
    assert!(matches!(wba.status, ObligationStatus::Pass), "{:#?}", wba);
}

/// `e2e.sh` passes a case marked `expect-unmatched-upstream` exactly when
/// its request matched NO stub, so its `PASS:` line is no proof of what its
/// stub demands.
#[test]
fn an_expect_unmatched_upstream_case_proves_nothing() {
    let marked = format!("# expect-unmatched-upstream\n{}", FULL_CALL);
    let dir = make_workspace(vec![(
        "tests/cases/update_widget_full.graphql",
        Some(marked),
    )]);
    let rep = report(dir.path());
    let wba = ob(&rep.obligations, "serialization.write-body-assertions");
    assert!(matches!(wba.status, ObligationStatus::Fail), "{:#?}", wba);
    let mc = ob(&rep.obligations, "serialization.mutation-cases");
    assert!(
        mc.gaps.iter().any(|g| g.kind == GapKind::FullCaseMissing),
        "{:#?}",
        mc.gaps
    );
}

/// Every case that passes every argument is tried, not only the first: an
/// unexecuted full case sorting ahead of the executed one must not hide it.
#[test]
fn an_unexecuted_full_case_sorting_first_does_not_hide_an_executed_one() {
    let dir = make_workspace(vec![(
        "tests/cases/update_widget_a_full.graphql",
        Some(FULL_CALL.to_string()),
    )]);
    let mc = ob(&obligations(dir.path()), "serialization.mutation-cases").clone();
    assert!(
        !mc.gaps.iter().any(|g| g.kind == GapKind::FullCaseMissing),
        "{:#?}",
        mc.gaps
    );
    assert!(matches!(mc.status, ObligationStatus::Pass), "{:#?}", mc);
}

/// The same for the required-only case.
#[test]
fn an_unexecuted_required_only_case_sorting_first_does_not_hide_an_executed_one() {
    let dir = make_workspace(vec![(
        "tests/cases/update_widget_a_required.graphql",
        Some("mutation { widget_co_updateWidget(id: \"w1\") { id } }\n".to_string()),
    )]);
    let mc = ob(&obligations(dir.path()), "serialization.mutation-cases").clone();
    assert!(
        !mc.gaps
            .iter()
            .any(|g| g.kind == GapKind::RequiredOnlyCaseMissing),
        "{:#?}",
        mc.gaps
    );
}

/// A `%` followed by a multibyte character is passed through, not sliced
/// into (which panicked and took the whole evidence run down with it).
#[test]
fn percent_decode_passes_a_percent_before_a_multibyte_character_through() {
    assert_eq!(percent_decode("/a/%a€b"), "/a/%a€b");
    assert_eq!(percent_decode("/a/%€"), "/a/%€");
    assert_eq!(percent_decode("/a/%20b%2F"), "/a/ b/");
}

/// Non-ASCII outside a string is invalid GraphQL, but a case file is user
/// input: the scanners read past it rather than slicing inside it.
#[test]
fn the_argument_scanners_do_not_panic_on_non_ascii_outside_a_string() {
    let calls = call_arg_texts("mutation { f(a: 1 é b: 2) { id } }", "f");
    assert_eq!(calls.len(), 1, "{:?}", calls);
    assert!(
        calls[0].contains(&("a".to_string(), "1".to_string())),
        "{:?}",
        calls
    );
    assert!(
        calls[0].contains(&("b".to_string(), "2".to_string())),
        "{:?}",
        calls
    );
    let fields = object_field_texts("{a: 1 € b: 2}");
    assert_eq!(
        fields,
        vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string())
        ]
    );
}

/// An operation named like the field it calls is not a second, empty call
/// (which read as "this case passes no arguments"), and its variable
/// definitions are not a call either.
#[test]
fn an_operation_named_like_its_field_is_not_a_call() {
    let calls = call_arg_texts("mutation f { f(a: \"x\") { id } }", "f");
    assert_eq!(calls.len(), 1, "{:?}", calls);
    let calls = call_arg_texts("mutation f($v: String) { f(a: $v) { id } }", "f");
    assert_eq!(calls, vec![vec![("a".to_string(), "$v".to_string())]]);
    // A field of the same name in the selection set is still a call.
    let calls = call_arg_texts("query { g { f } }", "f");
    assert_eq!(calls.len(), 1, "{:?}", calls);
}

/// A list argument the connector sends in no query key of its own is never
/// proven by some other query key whose list happens to match.
#[test]
fn a_list_argument_with_no_query_key_is_not_proven_by_any_matching_key() {
    let sdl = SDL.replacen(
        "http: { GET: \"/widgets\", queryParams: \"ids: $args.ids\" }",
        "http: { GET: \"/widgets\" }",
        1,
    );
    assert_ne!(sdl, SDL);
    let dir = make_workspace(vec![("widget-co.graphql", Some(sdl))]);
    let lap = ob(
        &obligations(dir.path()),
        "serialization.list-argument-proof",
    )
    .clone();
    assert!(
        lap.gaps
            .iter()
            .any(|g| g.kind == GapKind::ListUnproven && g.message.contains("(ids:")),
        "{:#?}",
        lap
    );
}

/// A list matcher is compared as a multiset: the same elements with
/// different counts is a different list.
#[test]
fn exact_list_match_compares_counts_not_membership() {
    let want = |xs: &[&str]| xs.iter().map(|x| format!("\"{}\"", x)).collect::<Vec<_>>();
    let has_exactly = |xs: &[&str]| {
        json!({"hasExactly": xs.iter().map(|x| json!({"equalTo": x})).collect::<Vec<_>>()})
            .as_object()
            .unwrap()
            .clone()
    };
    assert!(!exact_list_match(
        &has_exactly(&["a", "b", "b"]),
        &want(&["a", "a", "b"])
    ));
    assert!(exact_list_match(
        &has_exactly(&["b", "a", "a"]),
        &want(&["a", "a", "b"])
    ));
    let joined = |j: &str| json!({"equalTo": j}).as_object().unwrap().clone();
    assert!(!exact_list_match(&joined("a,b,b"), &want(&["a", "a", "b"])));
    assert!(exact_list_match(&joined("b,a,a"), &want(&["a", "a", "b"])));
}

/// A report that cannot run lists every obligation a full report has, each
/// `unexecuted`: unit-coverage included.
#[test]
fn an_unexecuted_report_lists_every_obligation() {
    let dir = tempfile::tempdir().unwrap();
    let ids: Vec<String> = obligations(dir.path()).into_iter().map(|o| o.id).collect();
    let full: Vec<String> = obligations(make_workspace(vec![]).path())
        .into_iter()
        .map(|o| o.id)
        .collect();
    assert_eq!(ids, full);
    assert!(ids.contains(&"serialization.unit-coverage".to_string()));
}

use graphos_factory_core::conformance::{
    conform, find_operation, path_matcher, response_shape_for,
};
use serde_json::{json, Map, Value};

fn shapes() -> Map<String, Value> {
    json!({
        "Reference": {"type": "object", "required": ["id", "type"], "properties": {"id": {"type": "string"}, "type": {"type": "string"}, "html_url": {"type": "string", "nullable": true}}},
        "Incident": {"type": "object", "properties": {
            "id": {"type": "string"},
            "status": {"type": "string", "enum": ["triggered", "acknowledged", "resolved"]},
            "resolved_at": {"type": "string"},
            "service": {"oneOf": [{"$ref": "#/shapes/ServiceReference"}, {"$ref": "#/shapes/Service"}]},
            "teams": {"type": "array", "items": {"$ref": "#/shapes/Reference"}},
            "secret": {"type": "string", "writeOnly": true}
        }},
        "ServiceReference": {"type": "object", "additionalProperties": true},
        "Service": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}},
        "Closed": {"type": "object", "properties": {"a": {"type": "integer"}}, "additionalProperties": false},
        "CreateIncident": {"type": "object", "required": ["title"], "properties": {"title": {"type": "string"}, "id": {"type": "string", "readOnly": true}}}
    })
    .as_object()
    .unwrap()
    .clone()
}

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/shapes/{}", name)})
}

#[test]
fn a_conformant_body_has_no_problems() {
    let body = json!({"id": "P1", "status": "resolved", "resolved_at": "2026-09-08T10:00:00Z", "service": {"id": "S1", "type": "service_reference"}, "teams": [{"id": "T1", "type": "team_reference", "html_url": null}]});
    assert!(conform(&body, &r("Incident"), &shapes(), "$", "response").is_empty());
}

#[test]
fn null_on_a_non_nullable_property_is_a_problem() {
    assert_eq!(
        conform(
            &json!({"id": "P1", "resolved_at": null}),
            &r("Incident"),
            &shapes(),
            "$",
            "response"
        ),
        vec!["$.resolved_at: null is not allowed (shape is not nullable)"]
    );
}

#[test]
fn nullable_true_accepts_null() {
    assert!(conform(
        &json!({"id": "T1", "type": "t", "html_url": null}),
        &r("Reference"),
        &shapes(),
        "$",
        "response"
    )
    .is_empty());
}

#[test]
fn omitted_optional_is_fine_omitted_required_is_not() {
    assert!(conform(
        &json!({"id": "P1"}),
        &r("Incident"),
        &shapes(),
        "$",
        "response"
    )
    .is_empty());
    assert_eq!(
        conform(
            &json!({"id": "T1"}),
            &r("Reference"),
            &shapes(),
            "$",
            "response"
        ),
        vec!["$: missing required property \"type\""]
    );
}

#[test]
fn enum_and_type_mismatches_are_reported_with_a_path() {
    let problems = conform(
        &json!({"id": 1, "status": "open"}),
        &r("Incident"),
        &shapes(),
        "$",
        "response",
    );
    assert_eq!(
        problems,
        vec![
            "$.id: expected string, got number",
            "$.status: \"open\" is not one of \"triggered\", \"acknowledged\", \"resolved\""
        ]
    );
}

#[test]
fn one_of_accepts_any_matching_variant_and_names_all_when_none_match() {
    assert!(conform(
        &json!({"id": "P1", "service": {"anything": true}}),
        &r("Incident"),
        &shapes(),
        "$",
        "response"
    )
    .is_empty());
    let problems = conform(
        &json!({"id": "P1", "service": "not-an-object"}),
        &r("Incident"),
        &shapes(),
        "$",
        "response",
    );
    assert_eq!(problems.len(), 1);
    assert!(problems[0].contains("matches none of 2 oneOf variants"));
}

#[test]
fn additional_properties_default_open_and_can_be_closed() {
    assert!(conform(
        &json!({"id": "P1", "extra": 1}),
        &r("Incident"),
        &shapes(),
        "$",
        "response"
    )
    .is_empty());
    assert_eq!(
        conform(
            &json!({"a": 1, "b": 2}),
            &r("Closed"),
            &shapes(),
            "$",
            "response"
        ),
        vec!["$.b: is not a documented property"]
    );
}

#[test]
fn array_items_are_checked_with_their_index() {
    let problems = conform(
        &json!({"id": "P1", "teams": [{"id": "T1", "type": "t"}, {"id": "T2"}]}),
        &r("Incident"),
        &shapes(),
        "$",
        "response",
    );
    assert_eq!(
        problems,
        vec!["$.teams[1]: missing required property \"type\""]
    );
}

#[test]
fn read_only_and_write_only_follow_the_direction() {
    assert_eq!(
        conform(
            &json!({"title": "x", "id": "P1"}),
            &r("CreateIncident"),
            &shapes(),
            "$",
            "request"
        ),
        vec!["$.id: is readOnly and must not be sent"]
    );
    assert!(conform(
        &json!({"title": "x"}),
        &r("CreateIncident"),
        &shapes(),
        "$",
        "request"
    )
    .is_empty());
    assert_eq!(
        conform(
            &json!({"id": "P1", "secret": "s"}),
            &r("Incident"),
            &shapes(),
            "$",
            "response"
        ),
        vec!["$.secret: is writeOnly and is never returned"]
    );
}

#[test]
fn unknown_ref_and_unreadable_shape_are_problems() {
    assert_eq!(
        conform(&json!({}), &r("Nope"), &shapes(), "$", "response"),
        vec!["$: unknown shape #/shapes/Nope"]
    );
    let broken = json!({"Broken": {"x-unresolved": "#/components/schemas/Broken"}})
        .as_object()
        .unwrap()
        .clone();
    assert!(conform(&json!({}), &r("Broken"), &broken, "$", "response")[0].contains("unreadable"));
}

#[test]
fn an_empty_shape_accepts_anything_including_null() {
    assert!(conform(&Value::Null, &json!({}), &shapes(), "$", "response").is_empty());
    assert!(conform(
        &json!({"a": [1, 2]}),
        &json!({"description": "free-form"}),
        &shapes(),
        "$",
        "response"
    )
    .is_empty());
}

#[test]
fn path_templates_match_concrete_paths_most_specific_first() {
    assert!(path_matcher("/incidents/{id}").is_match("/incidents/PINC001"));
    assert!(!path_matcher("/incidents/{id}").is_match("/incidents/PINC001/notes"));
    assert!(path_matcher("/incidents/{id}/notes").is_match("/incidents/PINC001/notes"));
    let inventory = json!({"operations": [
        {"key": "get:/users/{id}", "method": "GET", "path": "/users/{id}"},
        {"key": "get:/users/me", "method": "GET", "path": "/users/me"}
    ]});
    assert_eq!(
        find_operation(&inventory, "GET", "/users/me", &[])
            .unwrap()
            .unwrap()["key"],
        "get:/users/me"
    );
    assert_eq!(
        find_operation(&inventory, "GET", "/users/PUSR001", &[])
            .unwrap()
            .unwrap()["key"],
        "get:/users/{id}"
    );
    assert!(find_operation(&inventory, "POST", "/users/me", &[])
        .unwrap()
        .is_none());
}

#[test]
fn response_shape_for_picks_documented_then_errors_then_falls_back_for_2xx() {
    let op = json!({"response": {"status": "200", "shape_ref": "#/shapes/Incident"}, "errors": [{"status": "404", "shape_ref": "#/shapes/Err"}]});
    let t = response_shape_for(&op, "200").unwrap();
    assert_eq!(
        (t.shape_ref.as_deref(), t.documented),
        (Some("#/shapes/Incident"), true)
    );
    let t = response_shape_for(&op, "404").unwrap();
    assert_eq!(
        (t.shape_ref.as_deref(), t.documented),
        (Some("#/shapes/Err"), true)
    );
    let t = response_shape_for(&op, "201").unwrap();
    assert_eq!(
        (t.shape_ref.as_deref(), t.documented),
        (Some("#/shapes/Incident"), false)
    );
    assert!(response_shape_for(&op, "500").is_none());
}

#[test]
fn nullable_beside_a_ref_accepts_null_for_the_referencing_property() {
    // Swagger 2.0's `x-nullable: true` (and OpenAPI's `nullable`) written next
    // to a `$ref` describes the property, not the referenced shape; the oracle
    // must read it before dereferencing.
    let mut shapes = shapes();
    shapes.insert(
        "Issue".into(),
        json!({"type": "object", "properties": {
            "milestone": {"$ref": "#/shapes/Reference", "nullable": true},
            "assignee": {"$ref": "#/shapes/Reference"}
        }}),
    );
    assert!(conform(
        &json!({"milestone": null, "assignee": {"id": "U1", "type": "user"}}),
        &r("Issue"),
        &shapes,
        "$",
        "response"
    )
    .is_empty());
    assert_eq!(
        conform(
            &json!({"assignee": null}),
            &r("Issue"),
            &shapes,
            "$",
            "response"
        ),
        vec!["$.assignee: null is not allowed (shape is not nullable)".to_string()]
    );
}

#[test]
fn validate_strips_the_workspace_base_url_path_from_unit_suite_urls() {
    use graphos_factory_core::cmd::validate::{base_path_prefixes, strip_prefixes};
    // A spec with no usable server (a templated Swagger basePath) leaves the
    // inventory's base_urls empty; the unit suite's URLs still carry the
    // rendered BASE_URL's path, which template.yaml's test_default names.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("template.yaml"),
        "variables:\n  - name: BASE_URL\n    test_default: \"http://127.0.0.1:3000/api/v1\"\n",
    )
    .unwrap();
    let inventory = json!({"api": {"base_urls": ["https://api.example.com/v2/"]}});
    let prefixes = base_path_prefixes(&inventory, dir.path());
    assert_eq!(prefixes, vec!["/api/v1".to_string(), "/v2".to_string()]);
    assert_eq!(
        strip_prefixes("/api/v1/repos/search", &prefixes),
        "/repos/search"
    );
    assert_eq!(strip_prefixes("/v2/incidents", &prefixes), "/incidents");
    assert_eq!(strip_prefixes("/api/v1", &prefixes), "/");
    // A prefix must end at a path boundary: /v20/x is not under /v2.
    assert_eq!(strip_prefixes("/v20/x", &prefixes), "/v20/x");
    assert_eq!(strip_prefixes("/other/path", &prefixes), "/other/path");
    // No template.yaml: only the spec's servers count.
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        base_path_prefixes(&inventory, empty.path()),
        vec!["/v2".to_string()]
    );
}

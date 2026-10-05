use graphos_factory_core::jsonschema::validate;
use graphos_factory_core::schemas;
use graphos_factory_core::yaml::parse as parse_yaml;
use serde_json::{json, Value};

fn schema(name: &str) -> Value {
    schemas::load(name, None).unwrap()
}

#[test]
fn type_mismatches_are_reported_with_a_path() {
    let errors = validate(
        &json!({"a": "x"}),
        &json!({"type": "object", "properties": {"a": {"type": "integer"}}}),
    );
    assert_eq!(errors, vec!["/a expected integer, got string"]);
}

#[test]
fn required_properties_are_checked() {
    let errors = validate(
        &json!({}),
        &json!({"type": "object", "required": ["a", "b"]}),
    );
    assert_eq!(errors.len(), 2);
    assert!(errors[0].contains("missing required property \"a\""));
}

#[test]
fn additional_properties_false_rejects_unknown_keys() {
    let errors = validate(
        &json!({"a": 1, "b": 2}),
        &json!({"type": "object", "properties": {"a": {}}, "additionalProperties": false}),
    );
    assert_eq!(errors, vec!["/b is not a known property"]);
}

#[test]
fn pattern_properties_gate_keys_by_shape() {
    let s = json!({"type": "object", "patternProperties": {"^get:/": {"type": "object", "required": ["include"]}}, "additionalProperties": false});
    assert!(validate(&json!({"get:/a": {"include": true}}), &s).is_empty());
    assert_eq!(
        validate(&json!({"GET /a": {}}), &s),
        vec!["/GET /a is not a known property"]
    );
    assert_eq!(
        validate(&json!({"get:/a": {}}), &s),
        vec!["/get:/a is missing required property \"include\""]
    );
}

#[test]
fn ref_resolves_against_the_document_root() {
    let s = json!({"type": "object", "properties": {"child": {"$ref": "#/$defs/leaf"}}, "$defs": {"leaf": {"type": "string"}}});
    assert!(validate(&json!({"child": "ok"}), &s).is_empty());
    assert_eq!(
        validate(&json!({"child": 1}), &s),
        vec!["/child expected string, got number"]
    );
}

#[test]
fn enum_const_pattern_and_date_time() {
    assert!(validate(&json!("query"), &json!({"enum": ["query", "mutation"]})).is_empty());
    assert_eq!(
        validate(
            &json!("subscription"),
            &json!({"enum": ["query", "mutation"]})
        )
        .len(),
        1
    );
    assert_eq!(validate(&json!(2), &json!({"const": 1})).len(), 1);
    assert_eq!(
        validate(
            &json!("Widget-Co"),
            &json!({"type": "string", "pattern": "^[a-z_]+$"})
        )
        .len(),
        1
    );
    assert!(validate(
        &json!("2026-09-08T00:00:00Z"),
        &json!({"type": "string", "format": "date-time"})
    )
    .is_empty());
    assert_eq!(
        validate(
            &json!("2026-09-08"),
            &json!({"type": "string", "format": "date-time"})
        )
        .len(),
        1
    );
}

#[test]
fn any_of_accepts_a_union_one_of_demands_exactly_one() {
    let any_of =
        json!({"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]});
    assert!(validate(&json!(["a"]), &any_of).is_empty());
    assert_eq!(validate(&json!(1), &any_of).len(), 1);
    assert_eq!(
        validate(
            &json!("a"),
            &json!({"oneOf": [{"type": "string"}, {"type": "string"}]})
        )
        .len(),
        1
    );
}

#[test]
fn unique_items_catches_a_duplicate() {
    assert_eq!(
        validate(
            &json!(["a", "a"]),
            &json!({"type": "array", "uniqueItems": true})
        )
        .len(),
        1
    );
}

#[test]
fn the_shipped_contract_schemas_accept_their_documented_examples() {
    let workspace = parse_yaml(
        "contract_version: 1\nservice: incident_io\ndirectory: incident-io\ntype_prefix: Incident_Io\nfield_prefix: incident_io\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\n",
    )
    .unwrap();
    assert_eq!(
        validate(&workspace, &schema("workspace.schema.json")),
        Vec::<String>::new()
    );

    let selection = parse_yaml(
        "contract_version: 1\ndefaults:\n  fields: all\n  max_depth: 6\n  opaque_json_policy: forbid\noperations:\n  \"get:/v2/incidents\":\n    include: true\n    graphql:\n      root: query\n      name: listIncidents\n    fields:\n      exclude:\n        - \"incidents[]>external_issue_reference\"\n      rename:\n        \"incidents[]>incident_status\": status\n      tags:\n        \"incidents[]>creator>email\": [internal-low]\n    pagination: { expose: [page_size, after], next_cursor: \"pagination_meta.after\" }\n  \"post:/v2/incidents/{id}/actions\":\n    include: false\n    reason: \"customer asked for read-only in first release\"\noverrides:\n  - key: \"get:/v2/incidents/{id}\"\n    reason: \"the engineer orders the selection by severity\"\n    decision: D-0007\n    assert:\n      - contains: \"severity # first\"\n      - tag: internal\n    until: \"incident.io ships a sorted response\"\n    expires: \"2027-01-01\"\n",
    )
    .unwrap();
    assert_eq!(
        validate(&selection, &schema("selection.schema.json")),
        Vec::<String>::new()
    );
}

#[test]
fn selection_rejects_the_retired_customized_list_and_malformed_overrides() {
    let retired = json!({"contract_version": 1, "operations": {}, "customized": ["get:/x"]});
    assert_eq!(
        validate(&retired, &schema("selection.schema.json")).len(),
        1
    );
    let no_reason =
        json!({"contract_version": 1, "operations": {}, "overrides": [{"key": "get:/x"}]});
    assert_eq!(
        validate(&no_reason, &schema("selection.schema.json")).len(),
        1
    );
    let two_kinds = json!({"contract_version": 1, "operations": {}, "overrides": [{"key": "type:X", "reason": "r", "assert": [{"tag": "a", "arg": "b"}]}]});
    assert_eq!(
        validate(&two_kinds, &schema("selection.schema.json")).len(),
        1
    );
    let bad_key = json!({"contract_version": 1, "operations": {}, "overrides": [{"key": "Widget", "reason": "r"}]});
    assert_eq!(
        validate(&bad_key, &schema("selection.schema.json")).len(),
        1
    );
    let fine = json!({"contract_version": 1, "operations": {}, "overrides": [{"key": "Query.widget_co_ghost", "reason": "r", "assert": [{"matches": "ghost"}]}, {"key": "header", "reason": "h"}]});
    assert_eq!(
        validate(&fine, &schema("selection.schema.json")),
        Vec::<String>::new()
    );
}

/// ADR 0073, Adam's rule: a per-field reason is a decisions.json record, so
/// the selection has no `json_fields` map (nor a `json_reasons` one).
#[test]
fn selection_refuses_a_per_field_json_reason() {
    for key in ["json_fields", "json_reasons"] {
        let sel = json!({"contract_version": 1, "operations": {},
            key: {"Incident.body": {"reason": "recursive"}}});
        assert_eq!(
            validate(&sel, &schema("selection.schema.json")).len(),
            1,
            "{}",
            key
        );
    }
}

#[test]
fn selection_rejects_a_key_that_is_not_method_path() {
    let bad =
        json!({"contract_version": 1, "operations": {"GET /v2/incidents": {"include": true}}});
    assert_eq!(validate(&bad, &schema("selection.schema.json")).len(), 1);
}

#[test]
fn workspace_rejects_a_camel_case_service_name() {
    let base = json!({
        "contract_version": 1, "service": "incidentIo", "directory": "incident-io", "type_prefix": "Incident_Io", "field_prefix": "incident_io",
        "skill": {"name": "example", "version": "0.1.0"}, "source_kind": "rest", "connect_spec": "v0.3", "federation_version": "2.12.0", "intake": "spec",
        "created_at": "2026-09-08T00:00:00Z"
    });
    assert_eq!(validate(&base, &schema("workspace.schema.json")).len(), 1);
}

#[test]
fn evidence_rejects_a_status_outside_the_fixed_vocabulary() {
    let evidence = json!({
        "contract_version": 2, "commit": "a1b2c3d", "run_at": "2026-09-08T15:00:00Z",
        "toolchain": {"rover": "0.40.0", "federation": "2.12.0", "connect_spec": "v0.3"},
        "layers": {
            "compose": {"status": "green"}, "connector_unit": {"status": "pass"}, "wiremock_e2e": {"status": "skipped", "reason": "no docker"},
            "conformance": {"status": "pass"}, "lint": {"status": "pass"}, "live": {"status": "not_run", "reason": "no credential"}
        },
        "operations": {}
    });
    let errors = validate(&evidence, &schema("evidence.schema.json"));
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("compose/status"));
}

// ── if/then/else and propertyNames, which a target's embedded schema may use ──

#[test]
fn if_then_else_applies_the_branch_the_condition_selects() {
    let schema = json!({
        "type": "object",
        "properties": {"authorization_endpoint": {"type": "string"}, "token_endpoint": {"type": "string"}},
        "if": {"required": ["authorization_endpoint"]},
        "then": {"required": ["token_endpoint"]},
        "else": {"not": {"required": ["token_endpoint"]}}
    });
    assert!(validate(&json!({}), &schema).is_empty());
    assert!(validate(
        &json!({"authorization_endpoint": "a", "token_endpoint": "t"}),
        &schema
    )
    .is_empty());
    let errors = validate(&json!({"authorization_endpoint": "a"}), &schema);
    assert_eq!(errors.len(), 1, "{:?}", errors);
    assert!(errors[0].contains("token_endpoint"), "{:?}", errors);
    let errors = validate(&json!({"token_endpoint": "t"}), &schema);
    assert_eq!(errors.len(), 1, "{:?}", errors);
    assert!(errors[0].contains("does not hold"), "{:?}", errors);
}

#[test]
fn property_names_constrain_every_key() {
    let schema = json!({
        "type": "object",
        "propertyNames": {"allOf": [{"pattern": "^[a-z_]+$"}, {"not": {"enum": ["scope", "state"]}}]},
        "additionalProperties": {"type": "string"}
    });
    assert!(validate(&json!({"prompt": "login"}), &schema).is_empty());
    let errors = validate(&json!({"Prompt": "x", "scope": "y"}), &schema);
    assert_eq!(errors.len(), 2, "{:?}", errors);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("/Prompt is not an allowed property name")),
        "{:?}",
        errors
    );
    assert!(
        errors
            .iter()
            .any(|e| e.contains("/scope is not an allowed property name")),
        "{:?}",
        errors
    );
}

/// Every keyword in every embedded schema must be one the validator
/// implements: an unknown keyword is ignored, which would make lint accept
/// what the contract rejects. A target checks its own embedded schemas.
#[test]
fn every_embedded_schema_uses_only_keywords_the_validator_implements() {
    const SUPPORTED: &[&str] = &[
        "$schema",
        "$id",
        "$ref",
        "$defs",
        "$comment",
        "title",
        "description",
        "default",
        "examples",
        "type",
        "enum",
        "const",
        "required",
        "properties",
        "patternProperties",
        "additionalProperties",
        "propertyNames",
        "minProperties",
        "maxProperties",
        "items",
        "minItems",
        "uniqueItems",
        "minimum",
        "maximum",
        "pattern",
        "minLength",
        "format",
        "oneOf",
        "anyOf",
        "allOf",
        "not",
        "if",
        "then",
        "else",
    ];
    fn walk(schema: &Value, path: &str, unknown: &mut Vec<String>) {
        let s = match schema.as_object() {
            Some(s) => s,
            None => return,
        };
        for (k, v) in s {
            if !SUPPORTED.contains(&k.as_str()) {
                unknown.push(format!("{}/{}", path, k));
            }
            // `format` is implemented for one value only; any other value is
            // a constraint the validator would ignore.
            if k == "format" && v.as_str() != Some("date-time") {
                unknown.push(format!("{}/format={}", path, v));
            }
            match k.as_str() {
                "properties" | "patternProperties" | "$defs" => {
                    for (name, sub) in v.as_object().into_iter().flatten() {
                        walk(sub, &format!("{}/{}/{}", path, k, name), unknown);
                    }
                }
                // tuple-form `items: [ … ]` carries one subschema per position.
                "items" if v.is_array() => {
                    for (i, sub) in v.as_array().into_iter().flatten().enumerate() {
                        walk(sub, &format!("{}/{}/{}", path, k, i), unknown);
                    }
                }
                "items"
                | "additionalProperties"
                | "propertyNames"
                | "not"
                | "if"
                | "then"
                | "else" => walk(v, &format!("{}/{}", path, k), unknown),
                "oneOf" | "anyOf" | "allOf" => {
                    for (i, sub) in v.as_array().into_iter().flatten().enumerate() {
                        walk(sub, &format!("{}/{}/{}", path, k, i), unknown);
                    }
                }
                _ => {}
            }
        }
    }
    for name in [
        "workspace.schema.json",
        "inventory.schema.json",
        "selection.schema.json",
        "context.schema.json",
        "evidence.schema.json",
        "applied-lock.schema.json",
        "sources-lock.schema.json",
    ] {
        let schema = graphos_factory_core::schemas::load(name, None).unwrap();
        let mut unknown = Vec::new();
        walk(&schema, name, &mut unknown);
        assert!(
            unknown.is_empty(),
            "keywords the validator ignores: {:?}",
            unknown
        );
    }
}

#[test]
fn a_pattern_the_validator_cannot_compile_is_an_error_not_a_pass() {
    let schema = json!({"type": "string", "pattern": "([unclosed"});
    let errors = validate(&json!("anything"), &schema);
    assert_eq!(errors.len(), 1, "{:?}", errors);
    assert!(errors[0].contains("does not compile"), "{:?}", errors);
}

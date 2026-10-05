use graphos_factory_core::patch::{
    apply, bytes_sha256, canonical_sha256, describe, diff, escape_token, under,
};
use serde_json::{json, Value};

fn ops(from: &Value, to: &Value) -> Vec<Value> {
    diff(from, to).iter().map(|o| o.to_value()).collect()
}

#[test]
fn diff_produces_add_remove_and_replace_with_the_old_value_kept() {
    let from = json!({"a": 1, "b": {"c": "x", "d": [1, 2]}, "gone": true});
    let to = json!({"a": 2, "b": {"c": "x", "d": [1, 3], "new": null}});
    let got = ops(&from, &to);
    assert_eq!(
        got,
        vec![
            json!({"op": "replace", "path": "/a", "value": 2, "was": 1}),
            json!({"op": "replace", "path": "/b/d/1", "value": 3, "was": 2}),
            json!({"op": "add", "path": "/b/new", "value": null}),
            json!({"op": "remove", "path": "/gone", "was": true}),
        ]
    );
}

#[test]
fn arrays_of_different_length_are_one_replace_and_equal_length_recurse() {
    let from = json!({"required": ["id", "name"], "enum": ["a", "b"]});
    let to = json!({"required": ["id"], "enum": ["a", "c"]});
    let got = ops(&from, &to);
    assert_eq!(
        got,
        vec![
            json!({"op": "replace", "path": "/required", "value": ["id"], "was": ["id", "name"]}),
            json!({"op": "replace", "path": "/enum/1", "value": "c", "was": "b"}),
        ]
    );
}

#[test]
fn pointers_escape_slashes_and_tildes_and_round_trip_through_apply() {
    assert_eq!(escape_token("/widgets/{id}"), "~1widgets~1{id}");
    assert_eq!(escape_token("a~b"), "a~0b");
    let from =
        json!({"paths": {"/widgets/{id}": {"get": {"responses": {"200": {"description": "ok"}}}}}});
    let to = json!({"paths": {"/widgets/{id}": {"get": {"responses": {"200": {"description": "found"}}, "deprecated": true}}}});
    let got = ops(&from, &to);
    assert_eq!(
        got[0]["path"],
        "/paths/~1widgets~1{id}/get/responses/200/description"
    );
    assert_eq!(apply(&from, &got).unwrap(), to);
}

#[test]
fn apply_reproduces_the_target_for_nested_edits_and_root_replacement() {
    let cases = [
        (
            json!({"a": {"b": [1, {"c": 2}]}}),
            json!({"a": {"b": [1, {"c": 3, "d": 4}], "e": "f"}}),
        ),
        (json!([1, 2, 3]), json!([1, 2])),
        (json!("scalar"), json!({"now": "object"})),
        (json!({"x": 1}), json!({"x": 1})),
    ];
    for (from, to) in cases {
        let got = ops(&from, &to);
        assert_eq!(apply(&from, &got).unwrap(), to, "from {} to {}", from, to);
        if from == to {
            assert!(got.is_empty());
        }
    }
}

#[test]
fn apply_refuses_operations_that_do_not_fit_and_names_them() {
    let doc = json!({"a": {"b": 1}});
    let err = apply(
        &doc,
        &[json!({"op": "replace", "path": "/a/missing", "value": 2})],
    )
    .unwrap_err();
    assert!(
        err.contains("patch 0") && err.contains("does not exist"),
        "{}",
        err
    );
    let err = apply(&doc, &[json!({"op": "remove", "path": "/nope"})]).unwrap_err();
    assert!(err.contains("no member"), "{}", err);
    let err = apply(&doc, &[json!({"op": "test", "path": "/a/b", "value": 9})]).unwrap_err();
    assert!(err.contains("test"), "{}", err);
    let err = apply(&doc, &[json!({"op": "move", "path": "/a", "from": "/b"})]).unwrap_err();
    assert!(err.contains("unsupported op"), "{}", err);
    // Extra, non-RFC keys the lock adds are ignored.
    let ok = apply(
        &doc,
        &[json!({"op": "add", "path": "/a/c", "value": 3, "reason": "why", "decision": "D-0001", "was": null})],
    )
    .unwrap();
    assert_eq!(ok, json!({"a": {"b": 1, "c": 3}}));
    // Array append with "-" and insert by index.
    let arr = apply(
        &json!({"l": [1, 3]}),
        &[
            json!({"op": "add", "path": "/l/-", "value": 4}),
            json!({"op": "add", "path": "/l/1", "value": 2}),
        ],
    )
    .unwrap();
    assert_eq!(arr, json!({"l": [1, 2, 3, 4]}));
}

#[test]
fn canonical_hash_ignores_formatting_and_matches_across_json_and_yaml() {
    let pretty =
        graphos_factory_core::json::parse("{\n  \"openapi\": \"3.0.0\",\n  \"paths\": {}\n}\n")
            .unwrap();
    let compact =
        graphos_factory_core::json::parse("{\"openapi\":\"3.0.0\",\"paths\":{}}").unwrap();
    let yaml = graphos_factory_core::spec::load("openapi: 3.0.0\npaths: {}\n", "x.yaml").unwrap();
    assert_eq!(canonical_sha256(&pretty), canonical_sha256(&compact));
    assert_eq!(canonical_sha256(&pretty), canonical_sha256(&yaml));
    assert_ne!(
        canonical_sha256(&pretty),
        canonical_sha256(&json!({"openapi": "3.0.1", "paths": {}}))
    );
    // Bytes hashes do see formatting: that is what pins the vendor copy.
    assert_ne!(bytes_sha256(b"{\"a\":1}"), bytes_sha256(b"{ \"a\": 1 }"));
    assert_eq!(bytes_sha256(b"").len(), 64);
}

#[test]
fn under_and_describe_are_exact_about_prefixes() {
    assert!(under("/paths/~1widgets/get", "/paths/~1widgets"));
    assert!(under("/paths/~1widgets", "/paths/~1widgets"));
    assert!(
        !under("/paths/~1widgets~1{id}/get", "/paths/~1widgets"),
        "a sibling path must not match"
    );
    assert_eq!(
        describe(&json!({"op": "replace", "path": "/a", "value": 2, "was": 1})),
        "replace /a (was 1)"
    );
    assert_eq!(
        describe(&json!({"op": "add", "path": "/b", "value": 2})),
        "add /b"
    );
}

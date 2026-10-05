//! The envelope: facts in the inventory, the judgement in selection.yaml,
//! and the suggestion that connects them (ADR 0018).

use graphos_factory_core::envelope::{is_list, response_facts, suggest_envelope};
use graphos_factory_core::json::Object;
use serde_json::{json, Value};
use std::path::Path;

fn shapes(v: Value) -> Object {
    v.as_object().unwrap().clone()
}

/// The `response` record an inventory would carry for `shape`.
fn response(shape: &str, all: Value) -> Value {
    let s = shapes(all);
    let mut out = json!({"status": "200", "content_type": "application/json"});
    for (k, v) in response_facts(Some(&json!({"$ref": format!("#/shapes/{}", shape)})), &s) {
        graphos_factory_core::json::set(&mut out, k, v);
    }
    out
}

#[test]
fn a_key_that_carries_no_information_is_left_out_of_the_record() {
    // A resource object: no links, no arrays, no total, no cursor. Only the
    // count is recorded, so the record says exactly what is true.
    let r = response(
        "Widget",
        json!({"Widget": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}}}),
    );
    assert_eq!(r["root_property_count"], 2);
    for absent in [
        "root_is_array",
        "link_root_properties",
        "array_root_properties",
        "sole_root_property",
        "total_items_property",
        "cursor_root_properties",
        "envelope",
        "list",
    ] {
        assert!(
            r.get(absent).is_none(),
            "{} should be absent: {}",
            absent,
            r
        );
    }
    assert_eq!(suggest_envelope(Some(&r)), None);
}

#[test]
fn the_sole_content_property_is_the_envelope_whatever_its_type() {
    let r = response(
        "Version",
        json!({"Version": {"type": "object", "properties": {"version": {"type": "string"}}}}),
    );
    assert_eq!(r["sole_root_property"], "version");
    assert_eq!(suggest_envelope(Some(&r)), Some("version".to_string()));
    assert!(!is_list(Some(&r), Some("version")));

    // A hypermedia link does not count as a sibling: `{ sections, _links }`
    // still has one content property.
    let r = response(
        "Content",
        json!({"Content": {"type": "object", "properties": {
            "sections": {"type": "object"},
            "_links": {"type": "array", "items": {"type": "object"}}}}}),
    );
    assert_eq!(r["link_root_properties"], json!(["_links"]));
    assert_eq!(suggest_envelope(Some(&r)), Some("sections".to_string()));
}

#[test]
fn a_list_earns_an_envelope_from_a_total_a_cursor_or_a_single_sibling() {
    let with = |extra: Value| {
        let mut props = json!({"widgets": {"type": "array", "items": {"type": "object"}}});
        for (k, v) in extra.as_object().unwrap() {
            graphos_factory_core::json::set(&mut props, k, v.clone());
        }
        response("L", json!({"L": {"type": "object", "properties": props}}))
    };
    // A total names the whole collection: a wrapper.
    let r = with(
        json!({"total_items": {"type": "integer"}, "offset": {"type": "integer"}, "limit": {"type": "integer"}}),
    );
    assert_eq!(r["total_items_property"], "total_items");
    assert_eq!(suggest_envelope(Some(&r)), Some("widgets".to_string()));
    assert!(is_list(Some(&r), Some("widgets")));
    // A next cursor does the same.
    let r = with(
        json!({"next_cursor": {"type": ["string", "null"]}, "more": {"type": "boolean"}, "took": {"type": "integer"}}),
    );
    assert_eq!(r["cursor_root_properties"], json!(["next_cursor"]));
    assert_eq!(suggest_envelope(Some(&r)), Some("widgets".to_string()));
    // A HAL `next: { href }` is a link object, not a cursor, and three
    // unexplained siblings are a resource.
    let r = with(
        json!({"next": {"type": "object", "properties": {"href": {"type": "string"}}}, "id": {"type": "string"}, "name": {"type": "string"}}),
    );
    assert!(r.get("cursor_root_properties").is_none());
    assert_eq!(suggest_envelope(Some(&r)), None);
    // One sibling and nothing else: `{ ok, data[] }`.
    let r = with(json!({"ok": {"type": "boolean"}}));
    assert_eq!(suggest_envelope(Some(&r)), Some("widgets".to_string()));
}

#[test]
fn a_resource_that_embeds_a_list_is_not_a_wrapper() {
    // The Mailchimp failure in miniature: an audience with many root
    // properties, exactly one of them an array. The detector this replaced
    // answered `modules`; flattening to it would have dropped the other
    // twenty-one keys.
    let mut props = json!({"modules": {"type": "array", "items": {"type": "string"}},
                           "_links": {"type": "array", "items": {"type": "object"}}});
    for i in 0..20 {
        graphos_factory_core::json::set(
            &mut props,
            &format!("field_{}", i),
            json!({"type": "string"}),
        );
    }
    let r = response(
        "Audience",
        json!({"Audience": {"type": "object", "properties": props}}),
    );
    assert_eq!(r["root_property_count"], 22);
    assert_eq!(r["array_root_properties"], json!(["modules", "_links"]));
    assert_eq!(suggest_envelope(Some(&r)), None);
    assert!(!is_list(Some(&r), None));
}

#[test]
fn a_bare_array_has_nothing_to_unwrap() {
    let r = response(
        "Widgets",
        json!({"Widgets": {"type": "array", "items": {"type": "object"}},
               "Widget": {"type": "object"}}),
    );
    assert_eq!(r["root_is_array"], true);
    assert!(r.get("root_property_count").is_none());
    assert_eq!(suggest_envelope(Some(&r)), None);
    assert!(is_list(Some(&r), None), "the payload is still a list");
}

#[test]
fn an_envelope_is_never_a_hypermedia_link_however_lonely_the_array() {
    let r = response(
        "Thing",
        json!({"Thing": {"type": "object", "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"},
            "_links": {"type": "array", "items": {"type": "object"}}}}}),
    );
    assert_eq!(r["array_root_properties"], json!(["_links"]));
    assert_eq!(suggest_envelope(Some(&r)), None);
}

/// A pilot spec, so a change to the rule is a diff against a real vendor
/// document. A count, not a list: the named operations are pinned in
/// `inference.rs`. (PagerDuty's count is pinned beside that pilot, in its
/// target's suite.)
#[test]
fn the_suggestion_over_the_pilot_corpus() {
    for (pilot, file, want) in [("gitea", "swagger.json", 28)] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../pilots/graphos")
            .join(pilot)
            .join(file);
        assert!(path.exists(), "{} is not checked out", path.display());
        let text = std::fs::read_to_string(&path).unwrap();
        // The same reader `inventory build` uses: Swagger 2.0 is converted first.
        let spec = graphos_factory_core::spec::read(&text, path.to_str().unwrap())
            .unwrap()
            .document;
        let built = graphos_factory_core::openapi::build_inventory(&spec).unwrap();
        let proposed = built
            .inventory
            .get("operations")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter(|o| suggest_envelope(o.get("response")).is_some())
            .count();
        assert_eq!(
            proposed, want,
            "{}: the rule proposes an envelope for {} operations, expected {}",
            pilot, proposed, want
        );
    }
}

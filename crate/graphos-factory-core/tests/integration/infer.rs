use graphos_factory_core::conformance::conform;
use graphos_factory_core::infer::{
    collect_vocabulary, enum_groups, infer_operations, infer_shape, shape_name_for, Hints,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

fn shape(values: &[Value]) -> Value {
    let refs: Vec<Option<&Value>> = values.iter().map(Some).collect();
    infer_shape(&refs, &Hints::default(), "$")
}

fn hinted(
    values: &[Value],
    maps: &[&str],
    enums: &[&str],
    vocab: HashMap<String, BTreeSet<String>>,
) -> Value {
    let refs: Vec<Option<&Value>> = values.iter().map(Some).collect();
    let h = Hints {
        maps: maps.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
        enums: enums.iter().map(|s| s.to_string()).collect::<HashSet<_>>(),
        vocabulary: vocab,
    };
    infer_shape(&refs, &h, "$")
}

#[test]
fn a_single_sample_yields_types_but_no_required_list() {
    let s = shape(&[json!({"id": "a", "n": 1})]);
    assert_eq!(s["type"], "object");
    assert_eq!(s["properties"]["id"]["type"], "string");
    assert_eq!(s["properties"]["n"]["type"], "integer");
    assert!(s.get("required").is_none());
    assert_eq!(s["x-samples"], 1);
}

#[test]
fn required_means_present_in_every_sample() {
    let s = shape(&[
        json!({"id": "a", "extra": 1}),
        json!({"id": "b"}),
        json!({"id": "c"}),
    ]);
    assert_eq!(s["required"], json!(["id"]));
    assert_eq!(s["properties"]["extra"]["x-samples"], 1);
}

#[test]
fn nullable_union_and_integer_widening() {
    assert_eq!(shape(&[json!("x"), Value::Null])["nullable"], true);
    assert_eq!(
        shape(&[json!("x"), json!(1)])["type"],
        json!(["string", "integer"])
    );
    assert_eq!(shape(&[json!(1), json!(2.5)])["type"], "number");
    assert_eq!(shape(&[json!(1), json!(2)])["type"], "integer");
}

#[test]
fn only_null_is_untyped_and_nullable() {
    assert_eq!(
        shape(&[Value::Null, Value::Null]),
        json!({"nullable": true, "x-samples": 2})
    );
}

#[test]
fn strings_record_observed_values_but_need_a_hint_to_become_enums() {
    let s = shape(&[json!("HEARTS"), json!("SPADES"), json!("HEARTS")]);
    assert_eq!(s["x-observed-values"], json!(["HEARTS", "SPADES"]));
    assert!(s.get("enum").is_none());
    let e = hinted(
        &[json!("HEARTS"), json!("SPADES")],
        &[],
        &["$"],
        HashMap::new(),
    );
    assert_eq!(e["enum"], json!(["HEARTS", "SPADES"]));
}

#[test]
fn formats_are_detected_only_when_every_sample_matches() {
    assert_eq!(
        shape(&[json!("2026-09-08T09:12:44Z")])["format"],
        "date-time"
    );
    assert_eq!(
        shape(&[json!("https://a.test/x.png"), json!("https://a.test/y.png")])["format"],
        "uri"
    );
    assert!(shape(&[json!("https://a.test/x.png"), json!("not a url")])
        .get("format")
        .is_none());
}

#[test]
fn array_items_are_merged_across_every_element_of_every_sample() {
    let s = shape(&[
        json!({"cards": [{"code": "AS", "value": "ACE"}]}),
        json!({"cards": [{"code": "2S"}, {"code": "3S", "value": "3"}]}),
    ]);
    assert_eq!(s["properties"]["cards"]["items"]["type"], "object");
    assert_eq!(
        s["properties"]["cards"]["items"]["required"],
        json!(["code"])
    );
    assert_eq!(
        s["properties"]["cards"]["items"]["properties"]["value"]["x-observed-values"],
        json!(["3", "ACE"])
    );
    assert_eq!(shape(&[json!([])])["items"], json!({}));
}

#[test]
fn a_map_hint_turns_dynamic_keys_into_additional_properties() {
    let samples = [
        json!({"piles": {"hand": {"remaining": 3}}}),
        json!({"piles": {"discard": {"remaining": 1}, "hand": {"remaining": 0}}}),
    ];
    let plain = shape(&samples);
    let mut keys: Vec<&String> = plain["properties"]["piles"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["discard", "hand"]);
    let h = hinted(&samples, &["$.piles"], &[], HashMap::new());
    assert!(h["properties"]["piles"].get("properties").is_none());
    assert_eq!(
        h["properties"]["piles"]["additionalProperties"]["properties"]["remaining"]["type"],
        "integer"
    );
    assert_eq!(
        h["properties"]["piles"]["x-observed-keys"],
        json!(["hand", "discard"])
    );
}

#[test]
fn the_inferred_shape_is_a_usable_conformance_oracle() {
    let s = shape(&[
        json!({"id": "a", "tags": ["x"]}),
        json!({"id": "b", "tags": []}),
    ]);
    let mut shapes = Map::new();
    shapes.insert("R".into(), s);
    assert!(conform(
        &json!({"id": "c", "tags": ["y"]}),
        &json!({"$ref": "#/shapes/R"}),
        &shapes,
        "$",
        "response"
    )
    .is_empty());
    assert_eq!(
        conform(
            &json!({"tags": 1}),
            &json!({"$ref": "#/shapes/R"}),
            &shapes,
            "$",
            "response"
        ),
        vec![
            "$: missing required property \"id\"",
            "$.tags: expected array, got number"
        ]
    );
}

#[test]
fn operations_are_grouped_by_key_and_split_into_shape_and_errors() {
    let samples = vec![
        json!({"operation": "get:/deck/{deck_id}/draw/", "response": {"status": 200, "json": true, "body": {"success": true, "cards": []}}}),
        json!({"operation": "get:/deck/{deck_id}/draw/", "response": {"status": 200, "json": true, "body": {"success": false, "cards": [], "error": "Not enough"}}}),
        json!({"operation": "get:/deck/{deck_id}/draw/", "response": {"status": 404, "json": true, "body": {"success": false, "error": "Deck ID does not exist."}}}),
        json!({"operation": "get:/deck/{deck_id}/draw/", "response": {"status": 500, "json": false, "body": "<html>"}}),
        json!({"operation": "get:/deck/new/", "response": {"status": 200, "json": true, "body": {"deck_id": "x"}}}),
    ];
    let hints = json!({"maps": [{"operation": "*", "paths": ["$.piles"]}]});
    let ops = infer_operations(&samples, &hints, &HashMap::new());
    let draw = &ops["get:/deck/{deck_id}/draw/"];
    assert_eq!(draw["samples"], 4);
    assert_eq!(draw["response"]["status"], "200");
    assert_eq!(
        draw["response"]["shape"]["required"],
        json!(["success", "cards"])
    );
    assert_eq!(
        draw["response"]["shape"]["properties"]["error"]["x-samples"],
        1
    );
    assert_eq!(
        draw["errors"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["404", "500"]
    );
    assert_eq!(draw["errors"]["500"], json!({"x-note": "non-JSON body"}));
    assert!(ops["get:/deck/new/"].get("errors").is_none());
}

#[test]
fn shape_names_are_derived_from_the_operation_key() {
    assert_eq!(shape_name_for("get:/deck/new/"), "GetDeckNewResponse");
    assert_eq!(
        shape_name_for("get:/deck/{deck_id}/pile/{pile_name}/draw/"),
        "GetDeckByDeckIdPileByPileNameDrawResponse"
    );
}

#[test]
fn an_enum_hint_closes_over_the_whole_apis_vocabulary() {
    let bodies = vec![
        json!({"cards": [{"value": "2"}, {"value": "ACE"}]}),
        json!({"cards": [{"value": "KING"}]}),
        json!({"piles": {"hand": {"cards": [{"value": "JOKER"}]}}}),
    ];
    let refs: Vec<&Value> = bodies.iter().collect();
    let groups = enum_groups(
        &json!({"enums": [{"operation": "*", "paths": ["$.cards[].value", "$.piles.*.cards[].value"]}]}),
    );
    let vocabulary = collect_vocabulary(&refs, &groups);
    let sorted = |p: &str| vocabulary[p].iter().cloned().collect::<Vec<_>>();
    assert_eq!(sorted("$.cards[].value"), vec!["2", "ACE", "JOKER", "KING"]);
    assert_eq!(
        sorted("$.piles.*.cards[].value"),
        vec!["2", "ACE", "JOKER", "KING"]
    );
    let separate = collect_vocabulary(
        &refs,
        &enum_groups(
            &json!({"enums": [{"operation": "*", "paths": ["$.cards[].value"]}, {"operation": "*", "paths": ["$.piles.*.cards[].value"]}]}),
        ),
    );
    assert_eq!(
        separate["$.piles.*.cards[].value"]
            .iter()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["JOKER"]
    );
    let s = hinted(&[bodies[1].clone()], &[], &["$.cards[].value"], vocabulary);
    assert_eq!(
        s["properties"]["cards"]["items"]["properties"]["value"]["enum"],
        json!(["2", "ACE", "JOKER", "KING"])
    );
    assert!(
        s["properties"]["cards"]["items"]["properties"]["value"]["x-enum-scope"]
            .as_str()
            .unwrap()
            .contains("4 distinct values")
    );
}

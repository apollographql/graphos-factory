use graphos_factory_core::yaml::{parse, stringify};
use serde_json::json;

#[test]
fn block_mappings_and_nesting() {
    let doc = parse(
        "\ncontract_version: 1\nservice: incident_io\nskill:\n  name: example\n  version: 0.1.0\n",
    )
    .unwrap();
    assert_eq!(
        doc,
        json!({"contract_version": 1, "service": "incident_io", "skill": {"name": "example", "version": "0.1.0"}})
    );
}

#[test]
fn scalars_resolve_to_json_types() {
    let doc =
        parse("a: 1\nb: 1.5\nc: true\nd: false\ne: null\nf: ~\ng:\nh: \"2\"\ni: 'x'\nj: 0x10")
            .unwrap();
    assert_eq!(
        doc,
        json!({"a": 1, "b": 1.5, "c": true, "d": false, "e": null, "f": null, "g": null, "h": "2", "i": "x", "j": 16})
    );
}

#[test]
fn a_version_like_plain_scalar_stays_a_string() {
    assert_eq!(
        parse("federation_version: \"2.12.0\"").unwrap(),
        json!({"federation_version": "2.12.0"})
    );
    assert_eq!(
        parse("version: 0.1.0").unwrap(),
        json!({"version": "0.1.0"})
    );
}

#[test]
fn block_sequences_of_scalars_and_maps() {
    let doc = parse("\nsources:\n  - kind: openapi\n    url: https://example.test/openapi.json\n    tags:\n      - a\n      - b\n  - kind: docs\n    url: https://example.test/docs\n").unwrap();
    assert_eq!(
        doc["sources"],
        json!([{"kind": "openapi", "url": "https://example.test/openapi.json", "tags": ["a", "b"]}, {"kind": "docs", "url": "https://example.test/docs"}])
    );
}

#[test]
fn nested_sequence_under_a_sequence_entry() {
    let doc = parse("\noperations:\n  - key: get:/a\n    params:\n      - name: after\n        in: query\n      - name: limit\n        in: query\n").unwrap();
    assert_eq!(doc["operations"][0]["params"].as_array().unwrap().len(), 2);
    assert_eq!(
        doc["operations"][0]["params"][1],
        json!({"name": "limit", "in": "query"})
    );
}

#[test]
fn flow_collections() {
    let doc =
        parse("graphql: { root: query, name: listIncidents }\ntags: [a, \"b c\", 3]").unwrap();
    assert_eq!(
        doc,
        json!({"graphql": {"root": "query", "name": "listIncidents"}, "tags": ["a", "b c", 3]})
    );
}

#[test]
fn nested_flow_collections() {
    let doc =
        parse("pagination: { expose: [page_size, after], next_cursor: \"meta.after\" }").unwrap();
    assert_eq!(
        doc["pagination"],
        json!({"expose": ["page_size", "after"], "next_cursor": "meta.after"})
    );
}

#[test]
fn comments_are_stripped_outside_quotes() {
    let doc = parse("# leading\nname: value # trailing\nurl: \"http://x/#frag\"\n").unwrap();
    assert_eq!(doc, json!({"name": "value", "url": "http://x/#frag"}));
}

#[test]
fn quoted_keys_with_colons() {
    let doc = parse("operations:\n  \"get:/v2/incidents\":\n    include: true\n").unwrap();
    assert_eq!(
        doc["operations"],
        json!({"get:/v2/incidents": {"include": true}})
    );
}

#[test]
fn plain_keys_containing_a_colon_slash_are_kept_whole() {
    let doc = parse("operations:\n  get:/v2/incidents:\n    include: true\n").unwrap();
    assert_eq!(
        doc["operations"],
        json!({"get:/v2/incidents": {"include": true}})
    );
}

#[test]
fn a_path_template_key_with_braces_is_a_key() {
    let doc = parse("\npaths:\n  /chat-files/{fileId}:\n    get:\n      operationId: getChatFile\n      responses:\n        \"200\":\n          description: ok\n").unwrap();
    let paths = doc["paths"].as_object().unwrap();
    assert_eq!(
        paths.keys().collect::<Vec<_>>(),
        vec!["/chat-files/{fileId}"]
    );
    assert_eq!(
        doc["paths"]["/chat-files/{fileId}"]["get"]["operationId"],
        "getChatFile"
    );
    assert_eq!(
        doc["paths"]["/chat-files/{fileId}"]["get"]["responses"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["200"]
    );
}

#[test]
fn literal_and_folded_block_scalars() {
    let doc = parse("\nliteral: |\n  line one\n  line two\nstripped: |-\n  only line\nfolded: >\n  a\n  b\n\n  c\n").unwrap();
    assert_eq!(doc["literal"], "line one\nline two\n");
    assert_eq!(doc["stripped"], "only line");
    assert_eq!(doc["folded"], "a b\nc\n");
}

#[test]
fn anchors_aliases_and_merge_keys() {
    let doc = parse("\ndefaults: &defaults\n  timeout: 30\n  retries: 2\na:\n  <<: *defaults\n  retries: 5\nb: *defaults\n").unwrap();
    assert_eq!(doc["a"], json!({"timeout": 30, "retries": 5}));
    assert_eq!(doc["b"], json!({"timeout": 30, "retries": 2}));
}

#[test]
fn leading_document_marker_is_accepted() {
    assert_eq!(
        parse("---\nopenapi: 3.0.0\n").unwrap(),
        json!({"openapi": "3.0.0"})
    );
}

#[test]
fn multiple_documents_are_refused() {
    let err = parse("a: 1\n---\nb: 2\n").unwrap_err();
    assert!(err.contains("multiple YAML documents"), "{}", err);
}

#[test]
fn an_unknown_alias_is_refused() {
    assert!(parse("a: *missing\n").is_err());
}

#[test]
fn empty_and_explicit_empty_collections() {
    assert_eq!(
        parse("a: []\nb: {}\nc:\n").unwrap(),
        json!({"a": [], "b": {}, "c": null})
    );
}

#[test]
fn stringify_round_trips_the_shapes_the_skill_writes() {
    let value = json!({
        "contract_version": 1,
        "service": "incident_io",
        "skill": {"name": "example", "version": "0.1.0"},
        "operations": {"get:/v2/incidents": {"include": true, "graphql": {"root": "query", "name": "listIncidents"}, "tags": ["a", "b"]}},
        "overrides": [{"key": "get:/v2/incidents/{id}", "reason": "ordered by hand", "assert": [{"contains": "severity # first"}]}],
        "empty_list": []
    });
    assert_eq!(parse(&stringify(&value, 0)).unwrap(), value);
}

#[test]
fn stringify_quotes_values_that_would_reparse_wrong() {
    let value = json!({"get:/a": {"name": "true", "n": 1}});
    assert_eq!(parse(&stringify(&value, 0)).unwrap(), value);
}

#[test]
fn sequences_of_maps_round_trip() {
    let value = json!({"sources": [{"kind": "openapi", "patches": [{"path": "#/x", "reason": "y"}]}, {"kind": "docs"}]});
    assert_eq!(parse(&stringify(&value, 0)).unwrap(), value);
}

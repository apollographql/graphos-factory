use graphos_factory_core::spans::{
    check_assertion, compare_with_lock, lock_document, sha256_hex, spans, squash,
};
use serde_json::json;

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key", "@tag"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

# ─── Types ───

"A widget."
type Widget_Co_Widget
  @key(fields: "id")
  @connect(
    source: "widget_co"
    http: { GET: "/widgets/{$this.id}" }
    selection: "$.widget { id name owner { id } }"
  ) {
  id: ID!
  name: String
  owner: Widget_Co_Owner
}

type Widget_Co_Owner { id: ID }

enum Widget_Co_Colour { RED type BLUE }

scalar Widget_Co_JSON @specifiedBy(url: "https://example.test")

union Widget_Co_Thing = Widget_Co_Widget | Widget_Co_Owner

type Query {
  """
  List widgets.
  """
  widget_co_listWidgets(limit: Int, offset: Int): [Widget_Co_Widget]
    @tag(name: "internal")
    @connect(source: "widget_co", http: { GET: "/widgets" }, selection: "$.widgets { id }")

  "Get one."
  widget_co_widget(id: ID!): Widget_Co_Widget
    @connect(source: "widget_co", http: { GET: "/widgets/{$args.id}" }, selection: "$.widget { id }")

  widget_co_widgetByName(name: String!): Widget_Co_Widget
    @connect(source: "widget_co", http: { GET: "/widgets/{$args.name}" }, selection: "$.widget { id }")

  widget_co_ghost: String
    @connect(source: "widget_co", http: { GET: "/nothing/here" }, selection: "$")

  widget_co_plain: String
}

type Mutation {
  widget_co_createWidget(name: String!): Widget_Co_Widget
    @tag(name: "beta")
    @connect(source: "widget_co", http: { POST: "/widgets", body: "name: $args.name" }, selection: "$.widget { id }")
}
"#;

fn inventory() -> serde_json::Value {
    let op =
        |key: &str, method: &str, path: &str| json!({"key": key, "method": method, "path": path});
    json!({"operations": [
        op("get:/widgets", "GET", "/widgets"),
        op("get:/widgets/{id}", "GET", "/widgets/{id}"),
        op("post:/widgets", "POST", "/widgets"),
    ]})
}

#[test]
fn every_top_level_declaration_and_root_field_gets_a_stable_key() {
    let all = spans(SDL, Some(&inventory()), &Default::default());
    let keys: Vec<&str> = all.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(
        keys,
        vec![
            "header",
            "type:Widget_Co_Widget",
            "type:Widget_Co_Owner",
            "type:Widget_Co_Colour",
            "type:Widget_Co_JSON",
            "type:Widget_Co_Thing",
            "get:/widgets",
            "get:/widgets/{id}",
            "Query.widget_co_ghost",
            "Query.widget_co_plain",
            "post:/widgets",
        ]
    );
    let kinds: Vec<&str> = all.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![
            "header",
            "type",
            "type",
            "type",
            "type",
            "type",
            "operation",
            "operation",
            "field",
            "field",
            "operation"
        ]
    );
}

#[test]
fn span_texts_carry_their_own_comments_and_stop_at_their_body() {
    let all = spans(SDL, Some(&inventory()), &Default::default());
    let by = |k: &str| all.iter().find(|s| s.key == k).unwrap();
    let header = by("header");
    assert!(header.text.starts_with("extend schema"));
    assert!(header.text.ends_with("{{BASE_URL}}\" })"));
    assert!(
        !header.text.contains("Types"),
        "the banner belongs to the first type"
    );
    let widget = by("type:Widget_Co_Widget");
    assert!(widget.text.contains("─── Types ───"));
    assert!(widget.text.contains("\"A widget.\""));
    assert!(widget
        .text
        .trim_end()
        .ends_with("owner: Widget_Co_Owner\n}"));
    assert!(
        !widget.text.contains("Widget_Co_Owner {"),
        "stops at its own closing brace despite braces inside @connect"
    );
    assert_eq!(widget.names, vec!["id", "name", "owner"]);
    let colour = by("type:Widget_Co_Colour");
    assert!(colour.text.trim_end().ends_with("BLUE }"));
    let scalar = by("type:Widget_Co_JSON");
    assert!(scalar.text.trim_end().ends_with("example.test\")"));
    let union = by("type:Widget_Co_Thing");
    assert!(union.text.trim_end().ends_with("Widget_Co_Owner"));
    let list = by("get:/widgets");
    assert!(list.text.contains("List widgets."));
    assert!(list.tags.contains(&"internal".to_string()));
    assert_eq!(list.names, vec!["limit", "offset"]);
    assert_eq!(list.line, 34);
}

#[test]
fn two_root_fields_on_one_operation_share_a_span() {
    let all = spans(SDL, Some(&inventory()), &Default::default());
    let one = all.iter().find(|s| s.key == "get:/widgets/{id}").unwrap();
    assert!(one.text.contains("widget_co_widget(id: ID!)"));
    assert!(one.text.contains("widget_co_widgetByName(name: String!)"));
    assert_eq!(one.names, vec!["id", "name"]);
    assert!(!all.iter().any(|s| s.key == "Query.widget_co_widgetByName"));
}

#[test]
fn without_an_inventory_root_fields_are_keyed_by_name() {
    let all = spans(SDL, None, &Default::default());
    let keys: Vec<&str> = all
        .iter()
        .filter(|s| s.kind == "field")
        .map(|s| s.key.as_str())
        .collect();
    assert_eq!(
        keys,
        vec![
            "Query.widget_co_listWidgets",
            "Query.widget_co_widget",
            "Query.widget_co_widgetByName",
            "Query.widget_co_ghost",
            "Query.widget_co_plain",
            "Mutation.widget_co_createWidget"
        ]
    );
}

#[test]
fn hashes_are_sha256_of_the_exact_text_and_the_lock_compares_them() {
    let all = spans(SDL, Some(&inventory()), &Default::default());
    for s in &all {
        assert_eq!(s.sha256, sha256_hex(&s.text));
        assert_eq!(s.sha256.len(), 64);
    }
    let lock = lock_document("widget-co.graphql", &all);
    assert_eq!(lock["contract_version"], 1);
    assert_eq!(lock["schema"], "widget-co.graphql");
    assert!(lock["written_by"]
        .as_str()
        .unwrap()
        .starts_with("graphos-factory-core "));
    assert_eq!(lock["spans"].as_object().unwrap().len(), all.len());
    assert!(compare_with_lock(&all, &lock).is_empty());

    let edited = SDL
        .replace(
            "type Widget_Co_Owner { id: ID }",
            "type Widget_Co_Owner { id: ID! }",
        )
        .replace("\n  widget_co_plain: String\n", "\n");
    let edits = compare_with_lock(
        &spans(&edited, Some(&inventory()), &Default::default()),
        &lock,
    );
    let summary: Vec<(String, &str)> = edits.iter().map(|e| (e.key.clone(), e.change)).collect();
    assert_eq!(
        summary,
        vec![
            ("type:Widget_Co_Owner".to_string(), "modified"),
            ("Query.widget_co_plain".to_string(), "removed"),
        ]
    );
    assert_eq!(edits[0].line, Some(22));
    assert_eq!(edits[1].line, None);

    let added = SDL.replace(
        "\n  widget_co_plain: String\n",
        "\n  widget_co_plain: String\n  widget_co_extra: Int\n",
    );
    let edits = compare_with_lock(
        &spans(&added, Some(&inventory()), &Default::default()),
        &lock,
    );
    assert_eq!(edits.len(), 1);
    assert_eq!(
        (edits[0].key.as_str(), edits[0].change),
        ("Query.widget_co_extra", "added")
    );
}

#[test]
fn a_whitespace_only_change_is_still_a_change() {
    let all = spans(SDL, Some(&inventory()), &Default::default());
    let lock = lock_document("widget-co.graphql", &all);
    let reindented = SDL.replace("  widget_co_plain: String", "    widget_co_plain: String");
    let edits = compare_with_lock(
        &spans(&reindented, Some(&inventory()), &Default::default()),
        &lock,
    );
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].key, "Query.widget_co_plain");
}

#[test]
fn assertions_read_the_span_the_way_the_engineer_does() {
    let all = spans(SDL, Some(&inventory()), &Default::default());
    let list = all.iter().find(|s| s.key == "get:/widgets").unwrap();
    let owner = all
        .iter()
        .find(|s| s.key == "type:Widget_Co_Owner")
        .unwrap();
    let ok = |k: &str, v: &str, s| check_assertion(k, v, s).unwrap();
    assert!(
        ok("contains", "List   widgets.", list),
        "whitespace runs compare equal"
    );
    assert!(ok("contains", "selection: \"$.widgets { id }\"", list));
    assert!(!ok("contains", "$.widgets { id name }", list));
    assert!(ok("not_contains", "offset: Int!", list));
    assert!(ok("matches", r#"GET:\s*"/widgets""#, list));
    assert!(ok("tag", "internal", list));
    assert!(!ok("tag", "beta", list));
    assert!(ok("arg", "limit", list));
    assert!(ok("no_arg", "cursor", list));
    assert!(ok("field", "id", owner));
    assert!(ok("no_field", "name", owner));
    assert!(check_assertion("matches", "(", list).is_err());
    assert!(check_assertion("bogus", "x", list).is_err());
    assert_eq!(squash("  a\n\t b  "), "a b");
}

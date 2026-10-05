use graphos_factory_core::op_match::OpHints;
use graphos_factory_core::reconcile::{
    expected_paths, field_connectors, field_spans, link_connectors, link_hosts, match_operation,
    parse_selection, read_links, reconcile, render_report, resolve_link_path, resolve_paths,
    root_properties, type_connectors, Inputs, Link,
};
use graphos_factory_core::sdl_index::SdlIndex;
use serde_json::{json, Value};

fn workspace() -> Value {
    json!({"contract_version": 1, "service": "widget_co", "directory": "widget-co", "type_prefix": "Widget_Co", "field_prefix": "widget_co", "source_kind": "rest", "intake": "spec"})
}

fn op(key: &str, method: &str, path: &str, semantics: &str, response: Value) -> Value {
    json!({"key": key, "operation_id": key, "method": method, "path": path, "semantics": semantics, "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null, "response": response, "errors": [], "support": "supported", "support_reason": null})
}

fn inventory() -> Value {
    json!({
        "contract_version": 1,
        "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test"]},
        "operations": [
            op("get:/widgets", "GET", "/widgets", "read", json!({"envelope": "widgets", "shape_ref": "#/shapes/WidgetList", "list": true})),
            op("get:/widgets/{id}", "GET", "/widgets/{id}", "read", json!({"envelope": "widget", "shape_ref": "#/shapes/GetWidget", "list": false})),
            op("get:/widgets/me", "GET", "/widgets/me", "read", json!({"envelope": "widget", "shape_ref": "#/shapes/GetWidget", "list": false})),
            op("post:/widgets", "POST", "/widgets", "unknown", json!({"envelope": "widget", "shape_ref": "#/shapes/GetWidget", "list": false})),
            op("delete:/widgets/{id}", "DELETE", "/widgets/{id}", "write", json!({"envelope": null, "shape_ref": null, "list": false}))
        ],
        "shapes": {
            "Widget": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"}, "created_at": {"type": "string"}, "secret_sauce": {"type": "string"},
                "owner": {"type": "object", "additionalProperties": true}, "tags": {"type": "array", "items": {"type": "string"}}
            }},
            "WidgetList": {"type": "object", "properties": {"widgets": {"type": "array", "items": {"$ref": "#/shapes/Widget"}}, "more": {"type": "boolean"}}},
            "GetWidget": {"type": "object", "properties": {"widget": {"$ref": "#/shapes/Widget"}}}
        },
        "unresolved": []
    })
}

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key", "@tag"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

"A widget."
type Widget_Co_Widget
  @key(fields: "id")
  @connect(
    source: "widget_co"
    http: { GET: "/widgets/{$this.id}" }
    selection: "$.widget { id name createdAt: created_at owner { id } tags }"
  ) {
  id: ID!
  name: String
  createdAt: String
  owner: Widget_Co_Owner
  tags: [String]
}

type Widget_Co_Owner { id: ID }

type Query {
  """
  List widgets. Hand-tuned: the engineer likes the limit first.
  """
  widget_co_listWidgets(limit: Int, offset: Int): [Widget_Co_Widget]
    @tag(name: "internal")
    @connect(
      source: "widget_co"
      http: { GET: "/widgets" }
      selection: """
      # engineer's note: created_at is renamed on purpose
      $.widgets {
        id
        name
        createdAt: created_at
        owner { id }
        tags
      }
      """
    )

  "Get one widget."
  widget_co_widget(id: ID!): Widget_Co_Widget
    @connect(source: "widget_co", http: { GET: "/widgets/{$args.id}" }, selection: "$.widget { id name created_at owner { id } tags secret_sauce }")

  widget_co_currentWidget: Widget_Co_Widget
    @connect(source: "widget_co", http: { GET: "/widgets/me" }, selection: "$.widget { id name createdAt: created_at owner { id } tags secret_sauce }")

  widget_co_ghost: String
    @connect(source: "widget_co", http: { GET: "/nothing/here" }, selection: "$")
}
"#;

fn selection() -> Value {
    json!({
        "contract_version": 1,
        "defaults": {"fields": "all", "max_depth": 4, "opaque_json_policy": "forbid"},
        "operations": {
            "get:/widgets": {"include": true, "graphql": {"root": "query", "name": "listWidgets"}, "fields": {"exclude": ["widgets[]>secret_sauce"], "rename": {"widgets[]>created_at": "createdAt"}}, "pagination": {"expose": ["limit", "offset", "cursor"]}},
            "get:/widgets/{id}": {"include": true, "graphql": {"root": "query", "name": "widget", "entity": true, "key": "id"}, "fields": {"exclude": ["widget>secret_sauce"], "rename": {"widget>created_at": "createdAt"}}},
            "get:/widgets/me": {"include": false, "reason": "not for the pilot"},
            "post:/widgets": {"include": true, "graphql": {"root": "mutation", "name": "createWidget"}, "tags": ["beta"]},
            "delete:/widgets/{id}": {"include": true, "graphql": {"root": "mutation"}}
        },
        "overrides": [{
            "key": "get:/widgets",
            "reason": "the engineer likes the limit first and renamed created_at on purpose",
            "decision": "D-0003",
            "assert": [{"contains": "engineer's note: created_at is renamed on purpose"}, {"tag": "internal"}]
        }]
    })
}

fn pinned() -> Value {
    let mut sel = selection();
    sel["overrides"] =
        json!([{"key": "get:/widgets", "reason": "frozen by the engineer", "decision": "D-0003"}]);
    sel
}

fn run(
    workspace: &Value,
    selection: &Value,
    inventory: &Value,
    sdl: &str,
    baseline: Option<&str>,
) -> Value {
    run_with(workspace, selection, inventory, sdl, baseline, None)
}

fn run_with(
    workspace: &Value,
    selection: &Value,
    inventory: &Value,
    sdl: &str,
    baseline: Option<&str>,
    lock: Option<&Value>,
) -> Value {
    reconcile(&Inputs {
        decisions: None,
        workspace,
        selection,
        inventory,
        sdl,
        baseline,
        schema_file: "widget-co.graphql",
        lock,
        today: "2026-09-09",
    })
}

fn keys(report: &Value, section: &str) -> Vec<String> {
    report[section]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["key"].as_str().unwrap().to_string())
        .collect()
}

fn ghostless() -> String {
    let start = SDL.find("\n  widget_co_ghost").unwrap();
    let end =
        SDL[start..].find("selection: \"$\")\n").unwrap() + start + "selection: \"$\")\n".len();
    format!("{}{}", &SDL[..start], &SDL[end..])
}

#[test]
fn field_spans_find_every_root_field_with_arguments_tags_and_connector() {
    let spans = field_spans(SDL, "Query");
    assert_eq!(
        spans.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        vec![
            "widget_co_listWidgets",
            "widget_co_widget",
            "widget_co_currentWidget",
            "widget_co_ghost"
        ]
    );
    let list = &spans[0];
    assert_eq!(list.args, vec!["limit", "offset"]);
    assert_eq!(list.tags, vec!["internal"]);
    let c = list.connect.as_ref().unwrap();
    assert_eq!(c.method.as_deref(), Some("GET"));
    assert_eq!(c.path.as_deref(), Some("/widgets"));
    assert!(c.selection.as_deref().unwrap().contains("engineer's note"));
    assert!(list.text.contains("Hand-tuned"));
    assert!(!list.text.contains("Get one widget"));
    assert_eq!(spans[1].args.len(), 1);
    assert_eq!(spans[2].args.len(), 0);
}

#[test]
fn type_connectors_see_through_the_braces_inside_connect() {
    let t = type_connectors(SDL);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].type_name, "Widget_Co_Widget");
    assert_eq!(t[0].connect.path.as_deref(), Some("/widgets/{$this.id}"));
    assert_eq!(t[0].keys, vec![Some("id".to_string())]);
}

#[test]
fn parse_selection_reads_aliases_envelopes_nesting_methods_and_comments() {
    let tree = parse_selection("\n    # a comment\n    limit\n    $.widgets {\n      id\n      createdAt: created_at\n      owner { id }\n      piles: $.piles->entries { name: key }\n      literal: $(\"x\")\n      arg: $args.foo\n    }\n  ");
    assert_eq!(tree[0].key.as_ref().unwrap()[0], "limit");
    let env = &tree[1];
    assert!(env.rooted);
    assert_eq!(env.key.as_ref().unwrap(), &vec!["widgets".to_string()]);
    let children = env.children.as_ref().unwrap();
    let names: Vec<String> = children
        .iter()
        .map(|c| {
            c.alias
                .clone()
                .or_else(|| c.key.as_ref().map(|k| k[0].clone()))
                .unwrap_or_else(|| "?".into())
        })
        .collect();
    assert_eq!(
        names,
        vec!["id", "createdAt", "owner", "piles", "literal", "arg"]
    );
    assert_eq!(
        children[1].key.as_ref().unwrap(),
        &vec!["created_at".to_string()]
    );
    assert_eq!(children[3].methods, vec!["entries"]);
    assert!(children[3].opaque);
    assert!(children[4].opaque);
    assert!(children[5].opaque);
}

/// The invariant `lint::walk_enums` and `lint::walk_int_overflow` lean on:
/// `parse_item` promotes any node carrying a method to `opaque` as its last
/// act, so a reader that skips a mapped leaf needs `opaque` alone and a
/// `!methods.is_empty()` clause beside it is unreachable (ADR 0030). The case
/// above pins it for a rooted subselection; this one pins the plain leaf the
/// two lint rules actually walk. The unmapped leaf is the control that makes
/// the `opaque` assertion load-bearing.
#[test]
fn a_leaf_carrying_a_method_is_opaque() {
    let mapped = parse_selection("state: status->match([\"pending\", \"retired\"], [@, @])");
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].methods, vec!["match"]);
    assert_eq!(mapped[0].key.as_ref().unwrap(), &vec!["status".to_string()]);
    assert!(mapped[0].opaque, "a mapped leaf must be opaque");
    let plain = parse_selection("state: status");
    assert!(plain[0].methods.is_empty());
    assert!(!plain[0].opaque, "an unmapped leaf must not be opaque");
}

/// `?` marks a path step optional (connect/v0.4 `PathTail ::= "?"? (PathStep
/// "?"?)*`); it never changes which key is read. Unparsed, it ended the path at
/// the `?` and left the rest to fall out as a sibling field: `owner?.name`
/// read all of `owner` plus a phantom `name` at the parent (ADR 0057).
#[test]
fn optional_chaining_reads_as_the_plain_path() {
    let keys = |text: &str| -> Vec<Vec<String>> {
        parse_selection(text)
            .iter()
            .map(|n| n.key.clone().unwrap_or_default())
            .collect()
    };
    let path = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        keys("ownerName: owner?.name"),
        vec![path(&["owner", "name"])]
    );
    assert_eq!(keys("$.owner?.name?"), vec![path(&["owner", "name"])]);
    assert_eq!(keys(r#"ctx: $?."@context""#), vec![path(&["@context"])]);

    let guarded = parse_selection("n: size?->jsonStringify");
    assert_eq!(guarded.len(), 1, "no phantom node for the method name");
    assert_eq!(guarded[0].key.as_ref().unwrap(), &path(&["size"]));
    assert_eq!(guarded[0].methods, vec!["jsonStringify"]);
    assert!(guarded[0].opaque);

    // A tail after a method reads into its result: consumed, not a sibling.
    let tail = parse_selection("m: $.errors?->first?.message\nid");
    assert_eq!(tail.len(), 2, "{:?}", tail);
    assert_eq!(tail[0].key.as_ref().unwrap(), &path(&["errors"]));
    assert_eq!(tail[0].methods, vec!["first"]);
    assert_eq!(tail[1].key.as_ref().unwrap(), &path(&["id"]));

    let group = parse_selection("a: owner? { id }");
    assert_eq!(group.len(), 1);
    assert_eq!(group[0].children.as_ref().unwrap().len(), 1);
}

/// `a: x ?? y` outputs one field. Its later operands are reads recorded on the
/// node, not sibling fields (`y` would otherwise be an unaliased output field,
/// and a literal operand a phantom opaque node). A chain flattens.
#[test]
fn a_fallback_chain_records_its_operands_not_sibling_fields() {
    let tree = parse_selection("a: x ?? y ?? \"d\"\nb: w ?! $.v\nc: p??q");
    assert_eq!(tree.len(), 3, "{:?}", tree);
    let aliases: Vec<_> = tree.iter().map(|n| n.alias.clone().unwrap()).collect();
    assert_eq!(aliases, vec!["a", "b", "c"]);
    assert_eq!(tree[0].key.as_ref().unwrap(), &vec!["x".to_string()]);
    assert!(!tree[0].opaque, "the first operand is still a plain read");
    assert_eq!(tree[0].fallbacks.len(), 2);
    assert_eq!(
        tree[0].fallbacks[0].key.as_ref().unwrap(),
        &vec!["y".to_string()]
    );
    assert!(tree[0].fallbacks[1].opaque && tree[0].fallbacks[1].key.is_none());
    assert!(tree[1].fallbacks[0].rooted);
    assert_eq!(
        tree[1].fallbacks[0].key.as_ref().unwrap(),
        &vec!["v".to_string()]
    );
    assert_eq!(
        tree[2].fallbacks[0].key.as_ref().unwrap(),
        &vec!["q".to_string()]
    );
}

/// A fallback operand maps its path, but never displaces the node of a field
/// that exposes the same path: the rename check reads that node's alias.
#[test]
fn resolve_paths_maps_a_fallback_read_without_claiming_its_path() {
    let tree =
        parse_selection("$.widgets { ident: id createdAt: created_at ?? id note: name ?? tags }");
    let inv = inventory();
    let r = resolve_paths(
        &tree,
        &json!({"$ref": "#/shapes/WidgetList"}),
        inv["shapes"].as_object().unwrap(),
    );
    assert!(r.has("widgets[]>created_at"));
    assert!(r.has("widgets[]>tags[]"), "a fallback-only read is mapped");
    assert_eq!(
        r.node("widgets[]>id").and_then(|n| n.alias.as_deref()),
        Some("ident")
    );
    assert!(r.unknown.is_empty(), "{:?}", r.unknown);
}

#[test]
fn resolve_paths_produces_ui_grammar_paths_and_flags_unknown_keys() {
    let tree = parse_selection("$.widgets { id createdAt: created_at owner { id } tags bogus }");
    let inv = inventory();
    let r = resolve_paths(
        &tree,
        &json!({"$ref": "#/shapes/WidgetList"}),
        inv["shapes"].as_object().unwrap(),
    );
    assert!(r.has("widgets[]"));
    assert!(r.has("widgets[]>created_at"));
    assert!(r.has("widgets[]>tags[]"));
    assert!(r.has("widgets[]>owner>id"));
    assert_eq!(r.unknown, vec!["widgets[]>bogus"]);
}

#[test]
fn expected_paths_lists_top_level_and_item_level_properties_only() {
    let inv = inventory();
    let paths = expected_paths(
        &inv["operations"][0],
        inv["shapes"].as_object().unwrap(),
        Some("widgets"),
    );
    assert_eq!(
        paths,
        vec![
            "widgets[]",
            "widgets[]>id",
            "widgets[]>name",
            "widgets[]>created_at",
            "widgets[]>secret_sauce",
            "widgets[]>owner",
            "widgets[]>tags",
            "more"
        ]
    );
}

#[test]
fn match_operation_prefers_the_literal_path_over_a_template() {
    let inv = inventory();
    assert_eq!(
        match_operation(&inv, Some("GET"), Some("/widgets/me"), &[])
            .unwrap()
            .unwrap()["key"],
        "get:/widgets/me"
    );
    assert_eq!(
        match_operation(&inv, Some("GET"), Some("/widgets/{$args.id}"), &[])
            .unwrap()
            .unwrap()["key"],
        "get:/widgets/{id}"
    );
    assert!(
        match_operation(&inv, Some("GET"), Some("/nothing/here"), &[])
            .unwrap()
            .is_none()
    );
}

#[test]
fn reconcile_reports_add_remove_change_unmatched_and_fences_off_overrides() {
    let report = run(&workspace(), &selection(), &inventory(), SDL, None);
    assert_eq!(
        report["selection_errors"],
        json!(["delete:/widgets/{id} is included without graphql.name"])
    );
    assert_eq!(report["add"].as_array().unwrap().len(), 1);
    assert_eq!(report["add"][0]["key"], "post:/widgets");
    assert_eq!(report["add"][0]["root"], "Mutation");
    assert_eq!(report["add"][0]["field"], "widget_co_createWidget");
    assert_eq!(report["remove"][0]["key"], "get:/widgets/me");
    assert_eq!(
        report["remove"][0]["fields"],
        json!(["Query.widget_co_currentWidget"])
    );
    assert_eq!(report["remove"][0]["reason"], "not for the pilot");
    assert_eq!(keys(&report, "change"), vec!["get:/widgets/{id}"]);
    let messages: Vec<String> = report["change"][0]["drift"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect();
    let joined = messages.join("\n");
    assert!(joined
        .contains("widget>secret_sauce is excluded by the selection but the connector maps it"));
    assert!(joined.contains(
        "widget>created_at should be exposed as createdAt; the connector exposes it as created_at"
    ));
    assert!(!joined.contains("entity"));
    assert_eq!(report["unmatched"][0]["field"], "widget_co_ghost");
    assert_eq!(report["overrides"].as_array().unwrap().len(), 1);
    let custom = &report["overrides"][0];
    assert_eq!(custom["key"], "get:/widgets");
    assert_eq!(custom["kind"], "operation");
    assert_eq!(custom["decision"], "D-0003");
    assert_eq!(custom["failed"], 0);
    assert_eq!(custom["pinned"], false);
    assert!(custom["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["ok"] == true));
    let drift: Vec<&str> = custom["drift"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(drift
        .iter()
        .any(|d| d.contains("@tag(name: \"internal\"), which the selection does not list")));
    assert!(drift
        .iter()
        .any(|d| d.contains("pagination exposes cursor")));
    assert_eq!(custom["preserved"], Value::Null);
    assert!(!keys(&report, "change").contains(&"get:/widgets".to_string()));
    assert_eq!(report["unchanged"].as_array().unwrap().len(), 0);
    assert_eq!(report["clean"], false);
}

#[test]
fn reconcile_is_clean_when_schema_matches_and_override_drift_does_not_spoil_it() {
    let mut sel = selection();
    sel["operations"]
        .as_object_mut()
        .unwrap()
        .remove("post:/widgets");
    sel["operations"]
        .as_object_mut()
        .unwrap()
        .remove("delete:/widgets/{id}");
    sel["operations"]["get:/widgets/me"] = json!({"include": true, "graphql": {"root": "query", "name": "currentWidget"}, "fields": {"rename": {"widget>created_at": "createdAt"}}});
    sel["operations"]["get:/widgets/{id}"]["fields"] = json!({"rename": {}});
    let report = run(&workspace(), &sel, &inventory(), &ghostless(), None);
    assert_eq!(report["add"], json!([]));
    assert_eq!(report["remove"], json!([]));
    assert_eq!(report["unmatched"], json!([]));
    assert_eq!(keys(&report, "change"), Vec::<String>::new());
    let mut unchanged = keys(&report, "unchanged");
    unchanged.sort();
    assert_eq!(unchanged, vec!["get:/widgets/me", "get:/widgets/{id}"]);
    assert!(!report["overrides"][0]["drift"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(report["clean"], true);
    assert_eq!(report["lock"]["present"], false);
    assert_eq!(report["modified_since_baseline"], Value::Null);
}

#[test]
fn a_baseline_proves_a_pinned_override_byte_for_byte_and_flags_a_modified_one() {
    let same = run(&workspace(), &pinned(), &inventory(), SDL, Some(SDL));
    assert_eq!(same["overrides"][0]["pinned"], true);
    assert_eq!(same["overrides"][0]["preserved"], true);
    assert_eq!(same["modified_since_baseline"], json!([]));
    let touched = SDL.replace(
        "the engineer likes the limit first",
        "the engineer liked the limit first",
    );
    let modified = run(&workspace(), &pinned(), &inventory(), &touched, Some(SDL));
    assert_eq!(modified["overrides"][0]["preserved"], false);
    assert_eq!(modified["clean"], false);
    assert_eq!(
        modified["modified_since_baseline"],
        json!([{"key": "get:/widgets", "change": "modified"}])
    );
    let neighbour = SDL.replace("\"Get one widget.\"", "\"Get exactly one widget.\"");
    let untouched = run(&workspace(), &pinned(), &inventory(), &neighbour, Some(SDL));
    assert_eq!(untouched["overrides"][0]["preserved"], true);
    assert_eq!(
        untouched["modified_since_baseline"],
        json!([{"key": "get:/widgets/{id}", "change": "modified"}])
    );
}

#[test]
fn an_asserted_override_may_change_as_long_as_its_assertions_hold() {
    let mut sel = selection();
    sel["operations"]
        .as_object_mut()
        .unwrap()
        .remove("post:/widgets");
    sel["operations"]
        .as_object_mut()
        .unwrap()
        .remove("delete:/widgets/{id}");
    sel["operations"]["get:/widgets/me"] = json!({"include": true, "graphql": {"root": "query", "name": "currentWidget"}, "fields": {"rename": {"widget>created_at": "createdAt"}}});
    sel["operations"]["get:/widgets/{id}"]["fields"] = json!({"rename": {}});
    let sdl = ghostless().replace(
        "the engineer likes the limit first",
        "the engineer liked the limit first",
    );
    let report = run(&workspace(), &sel, &inventory(), &sdl, Some(&ghostless()));
    let o = &report["overrides"][0];
    assert_eq!(o["preserved"], false, "the text moved");
    assert_eq!(o["failed"], 0, "but the intent still holds");
    assert_eq!(report["clean"], true);

    let broken = sdl.replace("@tag(name: \"internal\")", "");
    let report = run(&workspace(), &sel, &inventory(), &broken, None);
    let o = &report["overrides"][0];
    assert_eq!(o["failed"], 1);
    let failing: Vec<&Value> = o["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["ok"] != true)
        .collect();
    assert_eq!(failing[0]["assert"], "tag");
    assert_eq!(report["clean"], false);
}

#[test]
fn override_keys_cover_types_and_unexplained_root_fields_and_are_validated() {
    let mut sel = selection();
    sel["overrides"] = json!([
        {"key": "type:Widget_Co_Owner", "reason": "kept minimal", "assert": [{"field": "id"}, {"no_field": "name"}]},
        {"key": "Query.widget_co_ghost", "reason": "a probe field", "assert": [{"matches": "nothing/here"}]},
        {"key": "type:Nope", "reason": "typo"},
        {"key": "get:/nope", "reason": "typo"},
        {"key": "get:/widgets", "reason": "", "assert": [{"bogus": "x"}]},
    ]);
    let report = run(&workspace(), &sel, &inventory(), SDL, None);
    let errors = report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(errors.contains("type:Nope, which the schema has no span for"));
    assert!(errors.contains("get:/nope, which is not in inventory.json"));
    assert!(errors.contains("override get:/widgets has no reason"));
    assert!(errors.contains("unknown assertion kind \"bogus\""));
    let owner = report["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == "type:Widget_Co_Owner")
        .unwrap();
    assert_eq!(owner["kind"], "type");
    assert_eq!(owner["failed"], 0);
    let ghost = report["overrides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == "Query.widget_co_ghost")
        .unwrap();
    assert_eq!(ghost["kind"], "field");
    assert_eq!(ghost["failed"], 0);
}

#[test]
fn the_retired_customized_list_is_a_selection_error_and_expiry_spoils_clean() {
    let mut sel = selection();
    sel["customized"] = json!(["get:/widgets"]);
    let report = run(&workspace(), &sel, &inventory(), SDL, None);
    assert!(report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e.as_str().unwrap().contains("customized: is retired")));

    let mut sel = selection();
    sel["overrides"][0]["expires"] = json!("2026-01-01");
    sel["overrides"][0]["until"] = json!("the vendor sorts on-calls");
    let report = run(&workspace(), &sel, &inventory(), SDL, None);
    assert_eq!(report["overrides"][0]["expired"], true);
    assert_eq!(report["overrides"][0]["until"], "the vendor sorts on-calls");
    sel["overrides"][0]["expires"] = json!("2026-09-09");
    let report = run(&workspace(), &sel, &inventory(), SDL, None);
    assert_eq!(
        report["overrides"][0]["expired"], false,
        "expires on the day itself is still valid"
    );
}

#[test]
fn the_applied_lock_names_every_span_a_human_changed() {
    let spans = graphos_factory_core::spans::spans(SDL, Some(&inventory()), &Default::default());
    let lock = graphos_factory_core::spans::lock_document("widget-co.graphql", &spans);
    let clean = run_with(
        &workspace(),
        &selection(),
        &inventory(),
        SDL,
        None,
        Some(&lock),
    );
    assert_eq!(clean["lock"]["present"], true);
    assert_eq!(clean["lock"]["hand_edits"], json!([]));

    let edited = SDL
        .replace("\"Get one widget.\"", "\"Get exactly one widget.\"")
        .replace("type Widget_Co_Owner { id: ID }", "type Widget_Co_Owner { id: ID name: String }")
        .replace("\n  widget_co_ghost: String\n    @connect(source: \"widget_co\", http: { GET: \"/nothing/here\" }, selection: \"$\")\n", "\n");
    let report = run_with(
        &workspace(),
        &selection(),
        &inventory(),
        &edited,
        None,
        Some(&lock),
    );
    let edits: Vec<(String, String)> = report["lock"]["hand_edits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["key"].as_str().unwrap().to_string(),
                e["change"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        edits,
        vec![
            ("type:Widget_Co_Owner".to_string(), "modified".to_string()),
            ("get:/widgets/{id}".to_string(), "modified".to_string()),
            ("Query.widget_co_ghost".to_string(), "removed".to_string()),
        ]
    );
    assert_eq!(report["lock"]["hand_edits"][1]["override"], false);
    assert_eq!(report["lock"]["hand_edits"][1]["line"], 46);
}

#[test]
fn unknown_keys_are_notes_not_drift_when_the_oracle_is_inferred() {
    let mut ws = workspace();
    ws["intake"] = json!("discovered");
    let mut sel = selection();
    sel["overrides"] = json!([]);
    let sdl = SDL.replace(
        "$.widgets {\n        id",
        "$.widgets {\n        id\n        never_seen",
    );
    let report = run(&ws, &sel, &inventory(), &sdl, None);
    assert!(report["notes"].as_array().unwrap().iter().any(
        |n| n["key"] == "get:/widgets" && n["message"].as_str().unwrap().contains("never_seen")
    ));
    let widgets_change = report["change"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "get:/widgets");
    let has_drift = widgets_change
        .map(|c| {
            c["drift"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["message"].as_str().unwrap().contains("never_seen"))
        })
        .unwrap_or(false);
    assert!(!has_drift);
}

#[test]
fn resolve_paths_puts_a_bare_array_response_under_the_item_prefix() {
    // A response that is the array itself (no envelope) is selected per
    // element, so the mapped paths must use the `[]>field` grammar that
    // `expected_paths` and selection.yaml use for it.
    let shapes = json!({
        "Widget": {"type": "object", "properties": {"id": {"type": "string"}, "created_at": {"type": "string"}, "owner": {"type": "object", "properties": {"id": {"type": "string"}}}}},
        "WidgetArray": {"type": "array", "items": {"$ref": "#/shapes/Widget"}}
    });
    let tree = parse_selection("id createdAt: created_at owner { id } bogus");
    let r = resolve_paths(
        &tree,
        &json!({"$ref": "#/shapes/WidgetArray"}),
        shapes.as_object().unwrap(),
    );
    assert!(r.has("[]>id"));
    assert!(r.has("[]>created_at"));
    assert!(r.has("[]>owner>id"));
    assert_eq!(r.unknown, vec!["[]>bogus"]);
    let op = json!({"response": {"shape_ref": "#/shapes/WidgetArray", "root_is_array": true}});
    assert_eq!(
        expected_paths(&op, shapes.as_object().unwrap(), None),
        vec!["[]>id", "[]>created_at", "[]>owner"]
    );
}

// Codex review loop (loop-voo6kQVe), concern 1.2's own follow-up: the root
// cause of closure.rs's stub-reattachment workaround (fix 2.2) was never in
// closure.rs at all.

#[test]
fn parse_selection_treats_a_literal_object_value_as_one_opaque_leaf_not_spilled_siblings() {
    // The D-0031/R3 entity-reference-stub idiom (`alias: { id: x }`) used
    // to fail on the literal `{`: ident() cannot start there, so the
    // value-parsing branch returned None without consuming it, the
    // caller's own "stuck, force-advance by one" recovery parsed the
    // literal's own inner fields as flat siblings of whatever list held
    // it, and then consumed the literal's closing brace as if it were
    // that list's own -- promoting every field genuinely written after
    // it one level up, cascading a second time when the enclosing list's
    // own true closer then gets mis-consumed too (exactly what `d` below
    // exercises, one level past what `c` alone exercises).
    let tree = parse_selection("$.a { b { stub: { id: stubId } c } d }");
    assert_eq!(
        tree.len(),
        1,
        "no second-level cascade: 'd' must not spill out to a top-level sibling of $.a, got {:?}",
        tree
    );
    let a = &tree[0];
    assert!(a.rooted);
    let a_children = a.children.as_ref().unwrap();
    assert_eq!(
        a_children.len(),
        2,
        "a's own children are b and d, got {:?}",
        a_children
    );
    assert_eq!(a_children[0].key.as_ref().unwrap()[0], "b");
    assert_eq!(
        a_children[1].key.as_ref().unwrap()[0],
        "d",
        "'d' must stay a's own child, not spill further"
    );
    let b_children = a_children[0].children.as_ref().unwrap();
    assert_eq!(
        b_children.len(),
        2,
        "b's own children are the opaque stub and c, got {:?}",
        b_children
    );
    assert_eq!(b_children[0].alias.as_deref(), Some("stub"));
    // `alias: { … }` is a read (JSONSelection `Alias SubSelection`), not
    // a literal: its inner fields are its own children, resolved in b's
    // context, and never spill to b's siblings (PR 65 review, T3).
    assert!(!b_children[0].opaque);
    let stub_children = b_children[0].children.as_ref().unwrap();
    assert_eq!(stub_children.len(), 1);
    assert_eq!(stub_children[0].alias.as_deref(), Some("id"));
    assert_eq!(stub_children[0].key.as_ref().unwrap()[0], "stubId");
    assert_eq!(
        b_children[1].key.as_ref().unwrap()[0],
        "c",
        "'c' must stay b's own child, not spill to a"
    );
}

#[test]
fn parse_selection_captures_a_literal_objects_own_top_level_keys() {
    // A consumer that needs a real sub-selection, not just reachability
    // (crate::cmd::scaffold's render_selection), reconstructs a minimal
    // one from these instead of refusing every literal-object stub
    // outright (Codex review loop loop-X9zRKG7i, third pass, follow-up on
    // concern 3.4).
    let tree = parse_selection("$.a { stub: { id: stubId } }");
    let stub = &tree[0].children.as_ref().unwrap()[0];
    assert!(
        stub.children.is_some(),
        "an alias group is a read with children (T3)"
    );
    assert_eq!(stub.literal_keys, vec!["id".to_string()]);

    // Multiple top-level keys, a nested literal value, and a value that
    // itself contains a colon-like shape must not confuse the depth-1
    // scan or be mistaken for keys of their own.
    let tree = parse_selection("$.a { multi: { first: a second: { nested: b } third: \"a: b\" } }");
    let multi = &tree[0].children.as_ref().unwrap()[0];
    assert_eq!(
        multi.literal_keys,
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string()
        ],
        "only depth-1 keys, not 'nested' from inside second's own literal value"
    );

    // Every other opaque shape has no keys to offer.
    let tree = parse_selection("$.a { n: $args.foo s: \"x\" lit: $(\"y\") }");
    for child in tree[0].children.as_ref().unwrap() {
        assert!(child.opaque);
        assert!(
            child.literal_keys.is_empty(),
            "{:?} is not a literal object, got {:?}",
            child.alias,
            child.literal_keys
        );
    }
}

/// `manager: { id: manager_id }` is a read (JSONSelection `Alias
/// SubSelection`): `manager_id` is read beside it and the group builds an
/// object from it. Treating the group as an opaque literal reported
/// `manager_id` as not mapped under `fields: all` (PR 65 review, T3); read
/// in context, the operation reconciles clean. A `$({ … })` literal stays
/// opaque.
#[test]
fn an_alias_group_is_a_read_in_context_and_a_fields_all_response_reconciles_clean() {
    let inv = json!({
        "contract_version": 1,
        "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test"]},
        "operations": [op("get:/widgets/{id}", "GET", "/widgets/{id}", "read",
            json!({"shape_ref": "#/shapes/Widget", "list": false}))],
        "shapes": {"Widget": {"type": "object", "properties": {
            "id": {"type": "string"}, "manager_id": {"type": "string"}}}},
        "unresolved": []
    });
    let sel = json!({"contract_version": 1, "defaults": {"fields": "all"},
        "operations": {"get:/widgets/{id}": {"include": true,
            "response": {"envelope": null, "confirmed": true},
            "graphql": {"root": "query", "name": "widget"}}}});
    let sdl = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

type Widget_Co_Manager { id: ID }
type Widget_Co_Widget { id: ID manager: Widget_Co_Manager }

type Query {
  widget_co_widget(id: ID!): Widget_Co_Widget
    @connect(source: "widget_co", http: { GET: "/widgets/{$args.id}" }, selection: "id manager: { id: manager_id }")
}
"#;
    let report = run(&workspace(), &sel, &inv, sdl, None);
    assert!(
        report["change"].as_array().unwrap().is_empty(),
        "no drift expected: {}",
        graphos_factory_core::json::pretty(&report["change"])
    );
    let tree = parse_selection("id stub: $({ id: \"x\" })");
    assert!(tree[1].opaque, "a $({{…}}) literal stays opaque");
}

/// A root-level array property is spelled `_links[]` in the path grammar;
/// the bare `_links` matches none of its paths, so it would exclude nothing
/// and the fields stay "not mapped". Reconcile names the key and the
/// spelling instead of guessing.
#[test]
fn a_bare_name_excluding_a_root_array_is_reported_with_the_brackets_spelling() {
    let mut inv = inventory();
    inv["shapes"]["GetWidget"]["properties"]["_links"] = json!({"type": "array", "items": {"type": "object", "properties": {"href": {"type": "string"}}}});
    let mut sel = selection();
    sel["operations"]["get:/widgets/{id}"]["fields"]["exclude"] =
        json!(["widget>secret_sauce", "_links"]);
    let report = run(&workspace(), &sel, &inv, SDL, None);
    let messages = |report: &Value| drift_messages(report, "get:/widgets/{id}");
    let got = messages(&report);
    assert!(
        got.contains(&"fields.exclude _links leaves off an array's []: the path grammar spells it _links[], and as written it excludes nothing".to_string()),
        "{:#?}",
        got
    );
    // Spelled with the brackets, the exclusion takes effect: no spelling
    // error, and `_links` is no longer reported as unmapped.
    sel["operations"]["get:/widgets/{id}"]["fields"]["exclude"] =
        json!(["widget>secret_sauce", "_links[]"]);
    let got = messages(&run(&workspace(), &sel, &inv, SDL, None));
    assert!(
        !got.iter()
            .any(|m| m.contains("leaves off") || m.contains("_links")),
        "{:#?}",
        got
    );
}

fn drift_messages(report: &Value, key: &str) -> Vec<String> {
    report["change"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["key"] == key)
        .flat_map(|c| c["drift"].as_array().unwrap().clone())
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect()
}

/// The missing `[]` is found anywhere in the path, not only on a bare root
/// name: on a nested segment (`widgets>secret_sauce` for
/// `widgets[]>secret_sauce`) and as the leading `[]>` of a root-array
/// response. The entry is quoted as written, `>**` included, and the
/// correctly spelled fixture selection draws no such message.
#[test]
fn a_nested_or_root_array_path_missing_its_brackets_is_reported_as_written() {
    // Without the override, the operation's drift lands in `change`.
    let mut sel = selection();
    sel["overrides"] = json!([]);
    let clean = run(&workspace(), &sel, &inventory(), SDL, None);
    let got = drift_messages(&clean, "get:/widgets");
    assert!(!got.iter().any(|m| m.contains("leaves off")), "{:#?}", got);
    // Below the paths the inventory reaches nothing is guessed at.
    let mut deep = sel.clone();
    deep["operations"]["get:/widgets"]["fields"]["exclude"] = json!(["widgets[]>owner>id"]);
    let got = drift_messages(
        &run(&workspace(), &deep, &inventory(), SDL, None),
        "get:/widgets",
    );
    assert!(!got.iter().any(|m| m.contains("leaves off")), "{:#?}", got);

    sel["operations"]["get:/widgets"]["fields"]["exclude"] = json!(["widgets>secret_sauce>**"]);
    let got = drift_messages(
        &run(&workspace(), &sel, &inventory(), SDL, None),
        "get:/widgets",
    );
    assert!(
        got.contains(&"fields.exclude widgets>secret_sauce>** leaves off an array's []: the path grammar spells it widgets[]>secret_sauce>**, and as written it excludes nothing".to_string()),
        "{:#?}",
        got
    );

    let mut inv = inventory();
    inv["shapes"]["WidgetArray"] = json!({"type": "array", "items": {"$ref": "#/shapes/Widget"}});
    inv["operations"][0]["response"] =
        json!({"envelope": null, "shape_ref": "#/shapes/WidgetArray", "list": true});
    sel["operations"]["get:/widgets"]["fields"]["exclude"] = json!(["secret_sauce"]);
    let got = drift_messages(&run(&workspace(), &sel, &inv, SDL, None), "get:/widgets");
    assert!(
        got.contains(&"fields.exclude secret_sauce leaves off an array's []: the path grammar spells it []>secret_sauce, and as written it excludes nothing".to_string()),
        "{:#?}",
        got
    );
}

/// Git in `dir`, isolated from the developer's global and system config (a
/// `commit.gpgsign` or `core.hooksPath` there would fail or slow the commit).
fn git_in(dir: &std::path::Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.test")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.test")
        .output()
        .unwrap()
        .status
        .success()
}

/// A git repository holding a copy of the gitea pilot, with
/// everything committed except `uncommitted`.
fn pilot_repo(uncommitted: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("gitea");
    let pilot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    assert!(std::process::Command::new("cp")
        .args(["-R", pilot.to_str().unwrap(), ws.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    assert!(git_in(&ws, &["init", "-q"]));
    assert!(git_in(&ws, &["add", "-A"]));
    for path in uncommitted {
        assert!(git_in(&ws, &["rm", "-q", "--cached", path]));
    }
    assert!(git_in(&ws, &["commit", "-q", "-m", "baseline"]));
    (tmp, ws)
}

/// `reconcile <ws> --baseline <rev> --json`: exit code, report, stderr.
fn reconcile_against(ws: &std::path::Path, rev: &str) -> (Option<i32>, Value, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "reconcile",
            ws.to_str().unwrap(),
            "--baseline",
            rev,
            "--json",
        ])
        .output()
        .unwrap();
    let report = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    (out.status.code(), report, stderr)
}

/// On a first apply neither the schema nor the applied lock is in HEAD yet.
/// `--baseline HEAD` used to exit 2 ("exists on disk, but not in 'HEAD'"); it
/// now reads the baseline as having no spans, names the file it looked up,
/// and reports every span — the header included — as added. A revision that
/// is not a commit is still exit 2.
#[test]
fn a_first_apply_reads_a_missing_baseline_schema_as_no_spans() {
    let (_tmp, ws) = pilot_repo(&["gitea.graphql", ".factory/applied.lock.yaml"]);

    let (code, report, stderr) = reconcile_against(&ws, "HEAD");
    assert_eq!(code, Some(0), "a first apply against HEAD: {}", stderr);
    assert!(
        stderr.contains("first apply — gitea.graphql is not in HEAD"),
        "{}",
        stderr
    );
    let moved = report["modified_since_baseline"].as_array().unwrap();
    assert_eq!(moved.len() as u64, report["spans"].as_u64().unwrap());
    assert!(moved.iter().any(|m| m["key"] == "header"), "{:#?}", moved);
    assert!(moved.iter().all(|m| m["change"] == "added"), "{:#?}", moved);

    let (code, _, stderr) = reconcile_against(&ws, "no-such-rev");
    assert_eq!(code, Some(2), "an unknown revision is still an error");
    assert!(stderr.contains("not a commit"), "{}", stderr);
}

/// A commit that already holds `.factory/applied.lock.yaml` but not the
/// schema (removed from the index, renamed, `directory` changed) is not a
/// first apply: an empty baseline there would report every span as added and
/// hide the real delta. It stays exit 2, naming the file.
#[test]
fn a_commit_with_an_applied_lock_but_no_schema_is_not_a_first_apply() {
    let (_tmp, ws) = pilot_repo(&["gitea.graphql"]);

    let (code, _, stderr) = reconcile_against(&ws, "HEAD");
    assert_eq!(code, Some(2), "{}", stderr);
    assert!(!stderr.contains("first apply —"), "{}", stderr);
    assert!(
        stderr.contains("gitea.graphql is not in HEAD, but .factory/applied.lock.yaml is"),
        "{}",
        stderr
    );
}

/// The baseline is `workspace.yaml`'s `<directory>.graphql`, not whichever
/// `.graphql` the directory listing returns first: stray untracked schemas
/// beside the committed one neither turn the run into a first apply nor
/// change the delta. Listing order is the filesystem's, so there are several
/// strays on both sides of the real name.
#[test]
fn the_baseline_is_the_workspace_schema_not_a_stray_graphql_file() {
    let strays = [
        "a.graphql",
        "b.graphql",
        "c.graphql",
        "git.graphql",
        "m.graphql",
        "scratch.graphql",
        "x.graphql",
        "zz.graphql",
    ];
    let (_tmp, ws) = pilot_repo(&[]);
    for stray in strays {
        std::fs::write(ws.join(stray), "type Query { scratch: String }\n").unwrap();
    }

    let (code, report, stderr) = reconcile_against(&ws, "HEAD");
    assert_eq!(code, Some(0), "{}", stderr);
    assert!(!stderr.contains("first apply"), "{}", stderr);
    assert_eq!(report["schema"], "gitea.graphql");
    assert_eq!(report["modified_since_baseline"], json!([]));
}

/// Salesforce keys its SOQL lists by a literal query string
/// (`/query?q=SELECT+…+FROM+Account`): the connector's own literal query
/// stays part of its path and picks the operation that carries it. A
/// templated query string is stripped as before, and a literal one the
/// inventory does not key by still matches the bare path (ADR 0054).
#[test]
fn a_literal_query_string_is_part_of_the_connector_path() {
    let sdl = r#"type Query {
  sf_accounts: [Sf_Row]
    @connect(source: "sf", http: { GET: "/query?q=SELECT+Id+FROM+Account" }, selection: "id: Id")
  sf_contacts: [Sf_Row]
    @connect(source: "sf", http: { GET: "/query?q=SELECT+Id+FROM+Contact" }, selection: "id: Id")
  sf_account(id: ID!): Sf_Row
    @connect(source: "sf", http: { GET: "/sobjects/Account/{$args.id}?fields={$args.id}" }, selection: "id: Id")
  sf_version: Sf_Row
    @connect(source: "sf", http: { GET: "/version?pretty=1" }, selection: "id: Id")
}
"#;
    let paths: Vec<Option<String>> = field_spans(sdl, "Query")
        .into_iter()
        .map(|s| s.connect.and_then(|c| c.path))
        .collect();
    assert_eq!(
        paths,
        vec![
            Some("/query?q=SELECT+Id+FROM+Account".to_string()),
            Some("/query?q=SELECT+Id+FROM+Contact".to_string()),
            Some("/sobjects/Account/{$args.id}".to_string()),
            Some("/version?pretty=1".to_string()),
        ]
    );

    let inv = json!({"operations": [
        op("get:/query?q=SELECT+Id+FROM+Account", "GET", "/query?q=SELECT+Id+FROM+Account", "read", json!(null)),
        op("get:/query?q=SELECT+Id+FROM+Contact", "GET", "/query?q=SELECT+Id+FROM+Contact", "read", json!(null)),
        op("get:/sobjects/Account/{id}", "GET", "/sobjects/Account/{id}", "read", json!(null)),
        op("get:/version", "GET", "/version", "read", json!(null)),
    ]});
    let key = |path: &str| {
        match_operation(&inv, Some("GET"), Some(path), &[])
            .unwrap()
            .map(|o| o["key"].as_str().unwrap().to_string())
    };
    for (path, want) in [
        (
            "/query?q=SELECT+Id+FROM+Account",
            "get:/query?q=SELECT+Id+FROM+Account",
        ),
        (
            "/query?q=SELECT+Id+FROM+Contact",
            "get:/query?q=SELECT+Id+FROM+Contact",
        ),
        ("/sobjects/Account/{$args.id}", "get:/sobjects/Account/{id}"),
        ("/version?pretty=1", "get:/version"),
    ] {
        assert_eq!(key(path).as_deref(), Some(want), "{}", path);
    }
    assert_eq!(key("/query?q=SELECT+Id+FROM+Lead"), None);
}

#[test]
fn field_spans_keep_the_argument_after_a_string_default() {
    // blank() turns a string default into spaces, so the character before the
    // next argument's name used to read as the default's `=`, and that
    // argument was dropped: a pagination `expose` naming it then reported a
    // missing argument, and nothing else about it was checked.
    let sdl = r#"type Query {
  inline(status: String = "open", cursor: String, "Page size." limit: Int = 10, order: Order = ASC, after: ID): [Widget]
  stacked(
    status: String = "open"
    cursor: String
    label: String = "a, b: c"
    after: ID
  ): [Widget]
}
"#;
    let spans = field_spans(sdl, "Query");
    assert_eq!(
        spans[0].args,
        vec!["status", "cursor", "limit", "order", "after"]
    );
    assert_eq!(spans[1].args, vec!["status", "cursor", "label", "after"]);
}

// ADR 0069 — links: in selection.yaml. The entry references an inventory
// fact (shape, path, operation) and records a judgement (include, field,
// confirmed). Reconcile checks the references, notes a draft, and derives
// the host GraphQL type from the root fields that return the shape.

/// `inventory()` plus the owner by-id operation `Widget.owner_id` links to,
/// and a root-array read (`get:/widgets/all` → `WidgetArray`, whose items
/// carry `owner_id` inline) for the `[]>owner_id` host derivation.
fn linked_inventory() -> Value {
    let mut inv = inventory();
    // The fact `inventory build` records on the fk (ADR 0069): a confirmed
    // link with none is stale (ADR 0098).
    inv["shapes"]["Widget"]["properties"]["owner_id"] = json!({"type": "string",
        "candidate_entity_link": {"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}});
    inv["shapes"]["Owner"] = json!({"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}});
    inv["shapes"]["WidgetArray"] = json!({"type": "array", "items": {"type": "object", "properties": {
        "id": {"type": "string"}, "owner_id": {"type": "string"}}}});
    let ops = inv["operations"].as_array_mut().unwrap();
    ops.push(op(
        "get:/owners/{ownerId}",
        "GET",
        "/owners/{ownerId}",
        "read",
        json!({"envelope": null, "shape_ref": "#/shapes/Owner", "list": false}),
    ));
    ops.push(op(
        "get:/widgets/all",
        "GET",
        "/widgets/all",
        "read",
        json!({"envelope": null, "shape_ref": "#/shapes/WidgetArray", "list": true, "root_is_array": true}),
    ));
    inv
}

fn link(shape: &str, path: &str, operation: &str) -> Value {
    json!({"shape": shape, "path": path, "operation": operation, "parameter": "ownerId", "include": true, "confirmed": false})
}

/// The clean selection of `reconcile_is_clean_when_schema_matches_…`, plus
/// the owner operation and one drafted link.
fn linked_selection() -> Value {
    let mut sel = selection();
    let ops = sel["operations"].as_object_mut().unwrap();
    ops.remove("post:/widgets");
    ops.remove("delete:/widgets/{id}");
    sel["operations"]["get:/widgets/me"] = json!({"include": true, "graphql": {"root": "query", "name": "currentWidget"}, "fields": {"rename": {"widget>created_at": "createdAt"}}});
    sel["operations"]["get:/widgets/{id}"]["fields"] = json!({"rename": {}});
    sel["operations"]["get:/owners/{ownerId}"] =
        json!({"include": true, "graphql": {"root": "query", "name": "owner"}});
    sel["links"] = json!([link("Widget", "owner_id", "get:/owners/{ownerId}")]);
    sel
}

/// The ghostless schema plus the owner root field, the fk mapped as
/// `ownerId: owner_id` in every root-field selection that returns a Widget
/// (fields: all would otherwise report `widget>owner_id` unmapped), and
/// the fk declared on the widget type under that camelCased name.
fn linked_sdl() -> String {
    ghostless()
        .replace(
            "type Query {\n",
            "type Query {\n  widget_co_owner(ownerId: ID!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\" }, selection: \"id name\")\n\n",
        )
        .replace("        tags\n      }\n", "        tags\n        ownerId: owner_id\n      }\n")
        .replace("tags secret_sauce }", "tags secret_sauce ownerId: owner_id }")
        .replace("  tags: [String]\n}\n", "  tags: [String]\n  ownerId: ID\n}\n")
}

/// `linked_sdl()` with the link applied: the API embeds `owner { id }` and
/// the link resolves the full owner record through the by-id operation, so
/// the existing `owner` field gains the field-level `{$this.ownerId}`
/// connector. This is the schema a *confirmed* link is expected to have —
/// from Task 9 on, a confirmed link whose field is missing is `links.add`
/// (R9), so the confirmed case of every test below runs against this text.
fn linked_sdl_with_field() -> String {
    let with_field = linked_sdl().replace(
        "  owner: Widget_Co_Owner\n  tags: [String]\n  ownerId: ID\n}\n",
        "  owner: Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$this.ownerId}\" }, selection: \"id name\")\n  tags: [String]\n  ownerId: ID\n}\n",
    );
    assert!(
        with_field.contains("{$this.ownerId}"),
        "fixture drifted: {}",
        with_field
    );
    with_field
}

/// `linked_sdl()` plus a root field returning the bare array: the host of a
/// `WidgetArray > []>owner_id` link is that field's item type.
fn linked_sdl_with_array_root() -> String {
    linked_sdl().replace(
        "type Query {\n",
        "type Query {\n  widget_co_allWidgets: [Widget_Co_Widget]\n    @connect(source: \"widget_co\", http: { GET: \"/widgets/all\" }, selection: \"id ownerId: owner_id\")\n\n",
    )
}

/// The declared root fields `link_hosts` matches through (ADR 0044).
fn linked_hints(selection: &Value, sdl: &str) -> OpHints {
    OpHints::from_selection(Some(&workspace()), Some(selection), Some(sdl))
}

fn error_strings(report: &Value) -> Vec<String> {
    report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.as_str().map(str::to_string))
        .collect()
}

fn link_notes(report: &Value) -> Vec<String> {
    report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["message"].as_str())
        .filter(|m| m.starts_with("link "))
        .map(str::to_string)
        .collect()
}

#[test]
fn read_links_reads_every_key_and_defaults_the_judgements() {
    let sel = json!({"contract_version": 1, "operations": {}, "links": [
        {"shape": "Widget", "path": "owner_id", "operation": "get:/owners/{ownerId}", "include": true},
        {"shape": "WidgetList", "path": "widgets[]>owner_id", "operation": "get:/owners/{ownerId}",
         "parameter": "ownerId", "include": false, "field": "creator", "confirmed": false,
         "reason": "one request per widget is too many here", "decision": "D-0021"},
        {"path": "no shape, no entry", "operation": "get:/x/{id}", "include": true}
    ]});
    let links = read_links(&sel);
    assert_eq!(links.len(), 2, "an entry without a shape is not a link");
    let first = &links[0];
    assert_eq!(first.key(), "Widget > owner_id");
    assert_eq!(first.parameter, None);
    assert!(first.include);
    assert!(
        first.confirmed,
        "absent means confirmed — a hand-written entry is the user's word"
    );
    assert_eq!(first.field, None);
    assert_eq!(
        first.field_name(),
        "owner",
        "derived from the operation's last static segment"
    );
    let second = &links[1];
    assert_eq!(second.key(), "WidgetList > widgets[]>owner_id");
    assert_eq!(second.parameter.as_deref(), Some("ownerId"));
    assert!(!second.include);
    assert!(!second.confirmed);
    assert_eq!(
        second.field_name(),
        "creator",
        "an explicit field wins over the derivation"
    );
    assert_eq!(
        second.reason.as_deref(),
        Some("one request per widget is too many here")
    );
    assert_eq!(second.decision.as_deref(), Some("D-0021"));
}

#[test]
fn a_links_field_name_is_the_one_derivation_applied_to_the_operation_keys_path() {
    // `Link::field_name` hands the *path* half of the operation key
    // (`get:/albums/{album_id}` → `/albums/{album_id}`) to
    // `crate::cmd::inventory_links::link_field_name` (Task 4) — the one derivation, no copy
    // here (R17) — and an explicit `field:` wins over it.
    let sel = json!({"contract_version": 1, "operations": {}, "links": [
        {"shape": "Song", "path": "album_id", "operation": "get:/albums/{album_id}", "include": true},
        {"shape": "Song", "path": "card_id", "operation": "get:/spotify/payment_cards/{card_id}", "include": true},
        {"shape": "Song", "path": "artist_id", "operation": "get:/artists/{artist_id}", "include": true, "field": "performer"}
    ]});
    let links = read_links(&sel);
    assert_eq!(links[0].field_name(), "album");
    assert_eq!(
        links[0].field_name(),
        graphos_factory_core::cmd::inventory_links::link_field_name("/albums/{album_id}"),
        "the accessor and the shared derivation agree on the same path"
    );
    assert_eq!(
        links[1].field_name(),
        "paymentCard",
        "the last static segment, singular, camelCased"
    );
    assert_eq!(
        links[1].field_name(),
        graphos_factory_core::cmd::inventory_links::link_field_name(
            "/spotify/payment_cards/{card_id}"
        )
    );
    assert_eq!(
        links[2].field_name(),
        "performer",
        "an explicit field wins over the derivation"
    );
}

#[test]
fn an_unconfirmed_link_is_a_note_never_drift() {
    let report = run(
        &workspace(),
        &linked_selection(),
        &linked_inventory(),
        &linked_sdl(),
        None,
    );
    assert_eq!(error_strings(&report), Vec::<String>::new());
    assert_eq!(report["add"], json!([]));
    assert_eq!(report["remove"], json!([]));
    assert_eq!(report["unmatched"], json!([]));
    assert_eq!(keys(&report, "change"), Vec::<String>::new());
    assert_eq!(
        link_notes(&report),
        vec!["link Widget > owner_id is still the tool's draft (field owner via get:/owners/{ownerId}); agree it with the user, then set confirmed: true or drop the entry"]
    );
    let note = report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == "Widget > owner_id")
        .expect("the note is keyed by shape > path");
    assert!(note["message"].as_str().unwrap().starts_with("link "));
    // A draft never spoils `clean`: nothing is applied until it is confirmed.
    assert_eq!(report["clean"], true);

    // Confirmed: no note — and the field is applied in the schema, because
    // from Task 9 on a confirmed link with no field is `links.add` (R9).
    // Declined: no note either, and no field — include: false is an answer.
    for (key, value, sdl) in [
        ("confirmed", true, linked_sdl_with_field()),
        ("include", false, linked_sdl()),
    ] {
        let mut sel = linked_selection();
        sel["links"][0][key] = json!(value);
        let report = run(&workspace(), &sel, &linked_inventory(), &sdl, None);
        assert_eq!(
            link_notes(&report),
            Vec::<String>::new(),
            "{}: {}",
            key,
            value
        );
        assert_eq!(report["clean"], true, "{}: {}: {}", key, value, report);
    }
}

#[test]
fn a_link_whose_shape_operation_or_path_is_unknown_is_a_selection_error() {
    let mut sel = linked_selection();
    sel["links"] = json!([
        link("Nope", "owner_id", "get:/owners/{ownerId}"),
        link("Widget", "owner_id", "get:/nowhere/{id}"),
        link("Widget", "songs[]>owner_id", "get:/owners/{ownerId}"),
        link("WidgetList", "widgets[]>owner_id", "get:/owners/{ownerId}"),
        link("GetWidget", "widget>owner_id", "get:/owners/{ownerId}"),
    ]);
    let report = run(&workspace(), &sel, &linked_inventory(), &linked_sdl(), None);
    let errors = error_strings(&report);
    for expected in [
        "links[0] (Nope > owner_id): shape Nope is not in inventory.json",
        "links[1] (Widget > owner_id): operation get:/nowhere/{id} is not in inventory.json",
        "links[2] (Widget > songs[]>owner_id): path songs[]>owner_id does not resolve in shape Widget",
    ] {
        assert!(errors.iter().any(|e| e == expected), "expected {:?} in {:?}", expected, errors);
    }
    // The two paths that step through a list ($ref items) and through a
    // nested $ref object both resolve: no error names links[3] or links[4].
    assert!(
        !errors
            .iter()
            .any(|e| e.starts_with("links[3]") || e.starts_with("links[4]")),
        "{:?}",
        errors
    );
    assert_eq!(report["clean"], false);

    // The by-id operation is the field's provenance: excluding it breaks the link.
    let mut sel = linked_selection();
    sel["operations"]["get:/owners/{ownerId}"] =
        json!({"include": false, "reason": "not for the pilot"});
    let report = run(&workspace(), &sel, &linked_inventory(), &linked_sdl(), None);
    let expected = "links[0] (Widget > owner_id): operation get:/owners/{ownerId} is not included by the selection — the by-id operation is the field's provenance";
    let errors = error_strings(&report);
    assert!(errors.iter().any(|e| e == expected), "{:?}", errors);
    assert_eq!(report["clean"], false);
}

#[test]
fn link_hosts_derives_the_host_type_from_the_root_fields_that_return_the_shape() {
    let inv = linked_inventory();
    let sel = linked_selection();
    let sdl = linked_sdl();
    let hints = linked_hints(&sel, &sdl);
    let mut index = SdlIndex::new(&sdl);
    let links = read_links(&sel);
    // Three root fields hand back a Widget — the list through `$.widgets { }`,
    // the by-id and `me` through `$.widget { }` — so one host, not three; and
    // the fk is found under its camelCased SDL name.
    assert_eq!(
        link_hosts(&links[0], &inv, &sdl, &mut index, &hints),
        vec![("Widget_Co_Widget".to_string(), "ownerId".to_string())]
    );
    // The list wrapper is an envelope: `$.widgets { }` unwraps it, so the
    // root field's content shape is `Widget`, and no root field returns
    // `WidgetList` itself. A link written against the wrapper has no host.
    let through_wrapper = Link {
        shape: "WidgetList".to_string(),
        path: "widgets[]>owner_id".to_string(),
        ..links[0].clone()
    };
    assert_eq!(
        link_hosts(&through_wrapper, &inv, &sdl, &mut index, &hints),
        Vec::<(String, String)>::new(),
        "WidgetList is the envelope, never what a root field returns"
    );
    // A root-array shape: the fact sits on the items, the path is
    // `[]>owner_id`, and the host is the list root field's item type. This
    // is the `[]` segment being transparent — the assertion goes red when
    // `link_hosts` stops skipping it (Step 4).
    let through_list = Link {
        shape: "WidgetArray".to_string(),
        path: "[]>owner_id".to_string(),
        ..links[0].clone()
    };
    let array_sdl = linked_sdl_with_array_root();
    let array_hints = linked_hints(&sel, &array_sdl);
    let mut array_index = SdlIndex::new(&array_sdl);
    assert_eq!(
        link_hosts(
            &through_list,
            &inv,
            &array_sdl,
            &mut array_index,
            &array_hints
        ),
        vec![("Widget_Co_Widget".to_string(), "ownerId".to_string())],
        "the bare array's item type is the host, and `[]` names no field"
    );
    // A shape whose fk has no SDL field yet keeps the wire name, so the
    // caller can say which field is missing.
    let owner_id = Link {
        shape: "Owner".to_string(),
        path: "name".to_string(),
        ..links[0].clone()
    };
    assert_eq!(
        link_hosts(&owner_id, &inv, &sdl, &mut index, &hints),
        vec![("Widget_Co_Owner".to_string(), "name".to_string())]
    );
}

#[test]
fn resolve_link_path_steps_into_items_once_per_list_level_and_follows_refs() {
    // The walker's wire grammar (ADR 0069): an optional leading run of `[]`
    // (a root array's items, one per list level), then wire-name segments
    // joined by `>`, each carrying one `[]` per list level. A wire name need
    // not be a GraphQL identifier.
    let shapes = json!({
        "Grid": {"type": "array", "items": {"type": "array", "items": {"type": "object", "properties": {
            "owner_id": {"type": "string"}}}}},
        "Board": {"type": "object", "properties": {
            "matrix": {"type": "array", "items": {"type": "array", "items": {"$ref": "#/shapes/Cell"}}},
            "owner": {"type": "object", "properties": {"account.id": {"type": "string"}}},
            "@type": {"type": "string"}
        }},
        "Cell": {"type": "object", "properties": {"owner_id": {"type": "string"}}}
    });
    let shapes = shapes.as_object().unwrap();
    for (shape, path, resolves) in [
        ("Grid", "[][]>owner_id", true),
        ("Grid", "[]>owner_id", false),
        // A path ends at a property, never at the items themselves.
        ("Grid", "[][]", false),
        ("Board", "matrix[][]>owner_id", true),
        ("Board", "matrix[]>owner_id", false),
        ("Board", "owner>account.id", true),
        ("Board", "@type", true),
        ("Board", "[]>owner_id", false),
        ("Board", "owner[]>account.id", false),
        ("Cell", "owner_id", true),
    ] {
        assert_eq!(
            resolve_link_path(&shapes[shape], path, shapes),
            resolves,
            "{} > {}",
            shape,
            path
        );
    }
}

#[test]
fn link_hosts_reaches_a_shape_only_a_ref_reaches_and_names_the_fk_as_a_connector_selects_it() {
    // R31: `Maker` is no operation's response shape — only `Widget.maker`'s
    // `$ref` reaches it — so its host is found by walking from the root
    // fields that return a Widget through the `maker` field. The fk is
    // declared under the alias the `me` connector selects (`madeBy:
    // owner_id`), which no camelCasing of the wire name finds; the two root
    // fields that do not select `maker` only know the wire name, and the
    // declared name wins over it.
    let mut inv = linked_inventory();
    inv["shapes"]["Widget"]["properties"]["maker"] = json!({"$ref": "#/shapes/Maker"});
    inv["shapes"]["Maker"] = json!({"type": "object", "properties": {
        "id": {"type": "string"}, "owner_id": {"type": "string",
            "candidate_entity_link": {"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}}}});
    // A root array of arrays: the `[][]` run is the items of the items.
    inv["shapes"]["WidgetGrid"] = json!({"type": "array", "items": {"type": "array", "items": {
        "type": "object", "properties": {"id": {"type": "string"}, "owner_id": {"type": "string"}}}}});
    inv["operations"].as_array_mut().unwrap().push(op(
        "get:/widgets/grid",
        "GET",
        "/widgets/grid",
        "read",
        json!({"envelope": null, "shape_ref": "#/shapes/WidgetGrid", "list": true, "root_is_array": true}),
    ));
    let sel = linked_selection();
    let sdl = linked_sdl()
        .replace(
            "createdAt: created_at owner { id } tags secret_sauce ownerId: owner_id }",
            "createdAt: created_at owner { id } tags secret_sauce ownerId: owner_id maker { id madeBy: owner_id } }",
        )
        .replace(
            "  tags: [String]\n  ownerId: ID\n}\n",
            "  tags: [String]\n  ownerId: ID\n  maker: Widget_Co_Maker\n}\n\ntype Widget_Co_Maker {\n  id: ID\n  madeBy: ID\n}\n",
        )
        .replace(
            "type Query {\n",
            "type Query {\n  widget_co_widgetGrid: [[Widget_Co_Widget]]\n    @connect(source: \"widget_co\", http: { GET: \"/widgets/grid\" }, selection: \"id ownerId: owner_id\")\n\n",
        );
    assert!(
        sdl.contains("madeBy: owner_id") && sdl.contains("type Widget_Co_Maker"),
        "fixture drifted: {}",
        sdl
    );
    let hints = linked_hints(&sel, &sdl);
    let mut index = SdlIndex::new(&sdl);
    let links = read_links(&sel);
    let maker = Link {
        shape: "Maker".to_string(),
        path: "owner_id".to_string(),
        ..links[0].clone()
    };
    assert_eq!(
        link_hosts(&maker, &inv, &sdl, &mut index, &hints),
        vec![("Widget_Co_Maker".to_string(), "madeBy".to_string())],
        "the ref-only shape's GraphQL type is the host, and the fk is the field the connector declares for it"
    );
    let grid = Link {
        shape: "WidgetGrid".to_string(),
        path: "[][]>owner_id".to_string(),
        ..links[0].clone()
    };
    assert_eq!(
        link_hosts(&grid, &inv, &sdl, &mut index, &hints),
        vec![("Widget_Co_Widget".to_string(), "ownerId".to_string())],
        "every `[]` of the leading run is transparent in the schema"
    );
}

// ─── ADR 0069 — field-level relationship connectors ─────────────────────────

/// A by-id lookup of widgets returning `Widget` directly, and a by-id lookup
/// of owners; `Widget.owner_id` carries the fact.
fn link_inventory() -> Value {
    json!({
        "contract_version": 1,
        "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test"]},
        "operations": [
            op("get:/widgets/{id}", "GET", "/widgets/{id}", "read", json!({"shape_ref": "#/shapes/Widget"})),
            op("get:/owners/{ownerId}", "GET", "/owners/{ownerId}", "read", json!({"shape_ref": "#/shapes/Owner"}))
        ],
        "shapes": {
            "Widget": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"},
                "owner_id": {"type": "string", "candidate_entity_link":
                    {"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}}
            }},
            "Owner": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}}
        },
        "unresolved": []
    })
}

const LINK_SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key", "@tag"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

type Widget_Co_Owner {
  id: ID
  name: String
}

type Widget_Co_Widget {
  id: ID
  name: String
  owner_id: ID
  "The owner record owner_id names, fetched by the router."
  owner: Widget_Co_Owner
    @connect(source: "widget_co", http: { GET: "/owners/{$this.owner_id}" }, selection: "id name")
}

type Query {
  widget_co_widget(id: ID!): Widget_Co_Widget
    @connect(source: "widget_co", http: { GET: "/widgets/{$args.id}" }, selection: "id name owner_id")
  widget_co_owner(ownerId: ID!): Widget_Co_Owner
    @connect(source: "widget_co", http: { GET: "/owners/{$args.ownerId}" }, selection: "id name")
}
"#;

const OWNER_FIELD: &str = "  \"The owner record owner_id names, fetched by the router.\"\n  owner: Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$this.owner_id}\" }, selection: \"id name\")\n";

fn link_selection(links: Value) -> Value {
    let mut sel = json!({
        "contract_version": 1,
        "defaults": {"fields": "all"},
        "operations": {
            "get:/widgets/{id}": {"include": true, "graphql": {"root": "query", "name": "widget"}, "response": {"envelope": null}},
            "get:/owners/{ownerId}": {"include": true, "graphql": {"root": "query", "name": "owner"}, "response": {"envelope": null}}
        }
    });
    if !links.is_null() {
        sel["links"] = links;
    }
    sel
}

fn link_entry(include: bool, confirmed: bool) -> Value {
    json!({"shape": "Widget", "path": "owner_id", "operation": "get:/owners/{ownerId}",
           "parameter": "ownerId", "field": "owner", "include": include, "confirmed": confirmed})
}

fn link_run(selection: &Value, sdl: &str) -> Value {
    run(&workspace(), selection, &link_inventory(), sdl, None)
}

#[test]
fn link_connectors_find_a_this_field_inside_a_type_and_nothing_on_the_roots() {
    let found = link_connectors(LINK_SDL);
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(found[0].type_name, "Widget_Co_Widget");
    assert_eq!(found[0].field, "owner");
    assert_eq!(found[0].this_vars, vec!["owner_id"]);
    assert_eq!(
        found[0].span.connect.as_ref().unwrap().path.as_deref(),
        Some("/owners/{$this.owner_id}")
    );
    // The entity fixture has a type-level connector and root fields only:
    // neither is a field-level connector.
    assert!(link_connectors(SDL).is_empty());
}

#[test]
fn a_confirmed_link_whose_field_exists_reconciles_clean_and_is_not_a_duplicate_of_the_root_field() {
    let report = link_run(&link_selection(json!([link_entry(true, true)])), LINK_SDL);
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    // Two root connectors and the relationship field.
    assert_eq!(report["connectors"], 3, "{}", report);
    assert_eq!(
        report["links"]["unchanged"],
        json!([{"key": "Widget > owner_id", "type": "Widget_Co_Widget", "field": "owner",
                "operation": "get:/owners/{ownerId}", "line": 17}]),
        "{}",
        report
    );
    assert_eq!(report["links"]["add"], json!([]));
    assert_eq!(report["links"]["remove"], json!([]));
    assert_eq!(report["links"]["change"], json!([]));
    // The by-id root field is not read as one of two fields on the operation.
    assert!(
        drift_messages(&report, "get:/owners/{ownerId}")
            .iter()
            .all(|m| !m.contains("root fields connect")),
        "{}",
        report
    );
    assert!(keys(&report, "unchanged").contains(&"get:/owners/{ownerId}".to_string()));
    assert_eq!(report["clean"], true, "{}", report);
}

#[test]
fn a_this_connector_no_included_link_declares_is_a_removal() {
    let expected = json!([{"type": "Widget_Co_Widget", "field": "owner", "operation": "get:/owners/{ownerId}",
                           "line": 17, "reason": "no included links: entry declares it"}]);
    // No links: section at all.
    let report = link_run(&link_selection(Value::Null), LINK_SDL);
    assert_eq!(report["links"]["remove"], expected, "{}", report);
    assert_eq!(report["clean"], false);
    // An excluded link does not claim the field either.
    let report = link_run(&link_selection(json!([link_entry(false, true)])), LINK_SDL);
    assert_eq!(report["links"]["remove"], expected, "{}", report);
    assert_eq!(report["clean"], false);
}

#[test]
fn a_confirmed_link_without_its_field_is_an_addition_and_an_unconfirmed_one_is_only_a_note() {
    let sdl = LINK_SDL.replace(OWNER_FIELD, "");
    assert!(!sdl.contains("$this"), "{}", sdl);
    let report = link_run(&link_selection(json!([link_entry(true, true)])), &sdl);
    assert_eq!(report["connectors"], 2);
    let add = report["links"]["add"].as_array().unwrap();
    assert_eq!(add.len(), 1, "{}", report);
    assert_eq!(add[0]["key"], "Widget > owner_id");
    assert_eq!(add[0]["type"], "Widget_Co_Widget");
    assert_eq!(add[0]["field"], "owner");
    assert_eq!(add[0]["method"], "GET");
    assert_eq!(add[0]["path"], "/owners/{$this.owner_id}");
    assert_eq!(
        add[0]["message"],
        "type Widget_Co_Widget has no field owner @connect(GET /owners/{$this.owner_id}) for link Widget > owner_id"
    );
    assert_eq!(report["clean"], false);

    // Unconfirmed: the note Task 7 writes, and no drift of any kind.
    let report = link_run(&link_selection(json!([link_entry(true, false)])), &sdl);
    assert_eq!(report["links"]["add"], json!([]), "{}", report);
    assert!(
        report["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["message"]
                .as_str()
                .unwrap()
                .contains("still the tool's draft")),
        "{}",
        report
    );
    assert_eq!(report["clean"], true, "{}", report);
}

#[test]
fn a_link_field_reading_the_wrong_this_variable_is_a_change() {
    let sdl = LINK_SDL.replace("{$this.owner_id}", "{$this.id}");
    let report = link_run(&link_selection(json!([link_entry(true, true)])), &sdl);
    let change = report["links"]["change"].as_array().unwrap();
    assert_eq!(change.len(), 1, "{}", report);
    assert_eq!(change[0]["type"], "Widget_Co_Widget");
    assert_eq!(change[0]["field"], "owner");
    let drift: Vec<&str> = change[0]["drift"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert_eq!(
        drift,
        vec!["the path reads {$this.id}; the foreign key owner_id names on Widget_Co_Widget is owner_id"]
    );
    assert_eq!(report["links"]["unchanged"], json!([]));
    assert_eq!(report["clean"], false);
}

#[test]
fn link_hosts_keeps_the_declared_alias_when_one_walk_reaches_the_host_type_twice() {
    // R38: one root field reaches `Widget_Co_Maker` twice — first through
    // `designer`, whose sub-selection does not select the fk, then through
    // `maker`, whose sub-selection declares it as `madeBy: owner_id`. The
    // second visit revisits a (shape, type) pair the walk has entered; the
    // alias it declares is still the fk's name, not the wire name the first
    // visit could only fall back to.
    let mut inv = link_inventory();
    inv["shapes"]["Widget"]["properties"]["designer"] = json!({"$ref": "#/shapes/Maker"});
    inv["shapes"]["Widget"]["properties"]["maker"] = json!({"$ref": "#/shapes/Maker"});
    inv["shapes"]["Maker"] = json!({"type": "object", "properties": {
        "id": {"type": "string"}, "owner_id": {"type": "string",
            "candidate_entity_link": {"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}}}});
    let sdl = LINK_SDL
        .replace(
            OWNER_FIELD,
            "  designer: Widget_Co_Maker\n  maker: Widget_Co_Maker\n",
        )
        .replace(
            "type Widget_Co_Owner {\n",
            "type Widget_Co_Maker {\n  id: ID\n  madeBy: ID\n}\n\ntype Widget_Co_Owner {\n",
        )
        .replace(
            "selection: \"id name owner_id\")",
            "selection: \"id name owner_id designer { id } maker { id madeBy: owner_id }\")",
        );
    assert!(
        sdl.contains("designer { id } maker { id madeBy: owner_id }")
            && sdl.contains("type Widget_Co_Maker")
            && sdl.contains("  designer: Widget_Co_Maker\n"),
        "fixture drifted: {}",
        sdl
    );
    let sel = link_selection(Value::Null);
    let hints = OpHints::from_selection(Some(&workspace()), Some(&sel), Some(&sdl));
    let mut index = SdlIndex::new(&sdl);
    let maker = Link {
        shape: "Maker".to_string(),
        path: "owner_id".to_string(),
        operation: "get:/owners/{ownerId}".to_string(),
        parameter: Some("ownerId".to_string()),
        include: true,
        field: Some("owner".to_string()),
        confirmed: true,
        reason: None,
        decision: None,
    };
    assert_eq!(
        link_hosts(&maker, &inv, &sdl, &mut index, &hints),
        vec![("Widget_Co_Maker".to_string(), "madeBy".to_string())],
        "the revisit's declared alias is the fk"
    );
}

#[test]
fn a_link_with_a_reference_problem_derives_no_host_and_claims_no_field() {
    // R38: only a link whose references all resolve reaches `link_hosts`.
    // A path the shape does not have would otherwise derive the host by the
    // wire name, claim the `owner` field and report it as drift.
    let mut unknown_path = link_entry(true, true);
    unknown_path["path"] = json!("owner_ident");
    let report = link_run(&link_selection(json!([unknown_path])), LINK_SDL);
    let errors = error_strings(&report);
    assert!(
        errors.iter().any(|e| {
            e
            == "links[0] (Widget > owner_ident): path owner_ident does not resolve in shape Widget"
        }),
        "{:?}",
        errors
    );
    assert_eq!(report["links"]["change"], json!([]), "{}", report);
    assert_eq!(report["links"]["unchanged"], json!([]), "{}", report);
    let remove = report["links"]["remove"].as_array().unwrap();
    assert_eq!(remove.len(), 1, "{}", report);
    assert_eq!(remove[0]["field"], "owner");

    // A link through an excluded by-id operation is a selection error, and
    // not the field's claim either.
    let mut sel = link_selection(json!([link_entry(true, true)]));
    sel["operations"]["get:/owners/{ownerId}"] =
        json!({"include": false, "reason": "not in this slice"});
    let report = link_run(&sel, LINK_SDL);
    let errors = error_strings(&report);
    assert!(
        errors.iter().any(|e| e.starts_with(
            "links[0] (Widget > owner_id): operation get:/owners/{ownerId} is not included"
        )),
        "{:?}",
        errors
    );
    assert_eq!(report["links"]["unchanged"], json!([]), "{}", report);
    let remove = report["links"]["remove"].as_array().unwrap();
    assert_eq!(remove.len(), 1, "{}", report);
    assert_eq!(remove[0]["field"], "owner");
}

#[test]
fn a_link_field_on_another_operation_or_verb_drifts_and_a_link_with_no_host_is_a_note() {
    // The field reads the fk but reaches another operation's path: the
    // link's operation is the declared one, so the connector drifts from it.
    let sdl = LINK_SDL.replace("/owners/{$this.owner_id}", "/widgets/{$this.owner_id}");
    let report = link_run(&link_selection(json!([link_entry(true, true)])), &sdl);
    assert_eq!(
        report["links"]["change"][0]["drift"],
        json!(["the connector resolves get:/widgets/{id}; the link's operation is get:/owners/{ownerId}"]),
        "{}",
        report
    );
    assert_eq!(report["clean"], false);

    // The field is the GET-by-id read, but the link names a write on the
    // same path: the operation and the verb both drift from it.
    let mut inv = link_inventory();
    inv["operations"].as_array_mut().unwrap().push(op(
        "post:/owners/{ownerId}",
        "POST",
        "/owners/{ownerId}",
        "write",
        json!({"shape_ref": "#/shapes/Owner"}),
    ));
    let mut entry = link_entry(true, true);
    entry["operation"] = json!("post:/owners/{ownerId}");
    let mut sel = link_selection(json!([entry]));
    sel["operations"]["post:/owners/{ownerId}"] =
        json!({"include": true, "graphql": {"root": "mutation", "name": "claimOwner"}});
    let report = run(&workspace(), &sel, &inv, LINK_SDL, None);
    // No fact ever points at a write, so the entry is also stale (ADR
    // 0098): its reason and remedy lead, the connector's drift follows.
    let drift = report["links"]["change"][0]["drift"].as_array().unwrap();
    assert!(
        drift[0].as_str().unwrap().starts_with("inventory.json carries no candidate_entity_link fact for Widget > owner_id -> post:/owners/{ownerId}"),
        "{}",
        report
    );
    assert!(
        drift[1].as_str().unwrap().starts_with("pasted at line "),
        "{}",
        report
    );
    assert_eq!(
        json!(drift[2..]),
        json!([
            "the connector resolves get:/owners/{ownerId}; the link's operation is post:/owners/{ownerId}",
            "the connector's verb is GET; post:/owners/{ownerId} is a POST"
        ]),
        "{}",
        report
    );

    // A write on the parent's key is no link connector at all (R43): it is
    // listed outside links, and the confirmed link still lacks its field.
    let sdl = LINK_SDL.replace(
        "{ GET: \"/owners/{$this.owner_id}\" }",
        "{ POST: \"/owners/{$this.owner_id}\" }",
    );
    assert_ne!(sdl, LINK_SDL);
    let report = link_run(&link_selection(json!([link_entry(true, true)])), &sdl);
    assert_eq!(report["links"]["change"], json!([]), "{}", report);
    assert_eq!(
        report["field_connectors"][0]["field"], "owner",
        "{}",
        report
    );
    assert_eq!(report["field_connectors"][0]["method"], "POST");
    assert_eq!(report["links"]["add"][0]["field"], "owner", "{}", report);
    assert_eq!(report["unmatched"], json!([]));

    // A confirmed link on a shape no root field reaches has no host type to
    // add the field to: a links note, not an addition.
    let mut inv = link_inventory();
    inv["shapes"]["Maker"] = json!({"type": "object", "properties": {
        "id": {"type": "string"}, "owner_id": {"type": "string",
            "candidate_entity_link": {"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}}}});
    let mut entry = link_entry(true, true);
    entry["shape"] = json!("Maker");
    let report = run(
        &workspace(),
        &link_selection(json!([entry])),
        &inv,
        &LINK_SDL.replace(OWNER_FIELD, ""),
        None,
    );
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    assert_eq!(
        report["links"]["notes"],
        json!([{"key": "Maker > owner_id",
                "message": "no included operation's root field returns a type for shape Maker; the link has no host type in the schema yet"}]),
        "{}",
        report
    );
    assert_eq!(report["links"]["add"], json!([]));
}

/// LINK_SDL plus a type whose field-level connectors are no GET-by-id
/// template: a sub-resource read on two parent fields, a read past the
/// parent's id, a write, and a search with no `{$this.}` at all.
fn sdl_with_field_connectors() -> String {
    let sdl = LINK_SDL.replace(
        "type Query {\n",
        concat!(
            "type Widget_Co_Repo {\n  owner: String\n  name: String\n",
            "  issues(limit: Int): [Widget_Co_Owner]\n    @connect(source: \"widget_co\", http: { GET: \"/repos/{$this.owner}/{$this.name}/issues?limit={$args.limit}\" }, selection: \"id\")\n",
            "  receipt: Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/receipts/{$this.name}/receipt\" }, selection: \"id\")\n",
            "  claim: Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { POST: \"/owners/{$this.owner}\" }, selection: \"id\")\n",
            "  search(q: String): [Widget_Co_Owner]\n    @connect(source: \"widget_co\", http: { GET: \"/owners?q={$args.q}\" }, selection: \"id\")\n",
            "  pair: Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/pairs/{$this.owner}/{$this.name}\" }, selection: \"id\")\n",
            "}\n\ntype Query {\n"
        ),
    );
    assert!(sdl.contains("type Widget_Co_Repo"), "fixture drifted");
    sdl
}

fn line_of(sdl: &str, needle: &str) -> usize {
    sdl.lines().position(|l| l.starts_with(needle)).unwrap() + 1
}

#[test]
fn only_a_get_by_id_template_is_a_link_connector() {
    let sdl = sdl_with_field_connectors();
    let names = |found: Vec<(String, String)>| -> Vec<String> {
        found
            .into_iter()
            .map(|(t, f)| format!("{}.{}", t, f))
            .collect()
    };
    assert_eq!(
        names(
            link_connectors(&sdl)
                .into_iter()
                .map(|l| (l.type_name, l.field))
                .collect()
        ),
        vec!["Widget_Co_Widget.owner"]
    );
    assert_eq!(
        names(
            field_connectors(&sdl)
                .into_iter()
                .map(|f| (f.type_name, f.field))
                .collect()
        ),
        vec![
            "Widget_Co_Repo.issues",
            "Widget_Co_Repo.receipt",
            "Widget_Co_Repo.claim",
            "Widget_Co_Repo.search",
            "Widget_Co_Repo.pair"
        ]
    );

    // Reconcile counts and lists them, never as links and never against clean.
    let report = link_run(&link_selection(json!([link_entry(true, true)])), &sdl);
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    assert_eq!(report["links"]["remove"], json!([]), "{}", report);
    assert_eq!(report["links"]["unchanged"].as_array().unwrap().len(), 1);
    assert_eq!(report["connectors"], 8, "{}", report);
    let issues = line_of(&sdl, "  issues(");
    assert_eq!(
        report["field_connectors"],
        json!([
            {"type": "Widget_Co_Repo", "field": "issues", "method": "GET", "path": "/repos/{$this.owner}/{$this.name}/issues", "line": issues},
            {"type": "Widget_Co_Repo", "field": "receipt", "method": "GET", "path": "/receipts/{$this.name}/receipt", "line": line_of(&sdl, "  receipt:")},
            {"type": "Widget_Co_Repo", "field": "claim", "method": "POST", "path": "/owners/{$this.owner}", "line": line_of(&sdl, "  claim:")},
            {"type": "Widget_Co_Repo", "field": "search", "method": "GET", "path": "/owners", "line": line_of(&sdl, "  search(")},
            {"type": "Widget_Co_Repo", "field": "pair", "method": "GET", "path": "/pairs/{$this.owner}/{$this.name}", "line": line_of(&sdl, "  pair:")}
        ]),
        "{}",
        report
    );
    let note = report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["key"] == "field_connectors")
        .unwrap_or_else(|| panic!("{}", report));
    assert!(
        note["message"].as_str().unwrap().starts_with(&format!(
            "5 field-level connectors outside links: Widget_Co_Repo.issues (GET /repos/{{$this.owner}}/{{$this.name}}/issues, line {}), ",
            issues
        )),
        "{}",
        note
    );
    assert_eq!(report["clean"], true, "{}", report);

    // Without them: no note, and an empty list.
    let report = link_run(&link_selection(json!([link_entry(true, true)])), LINK_SDL);
    assert_eq!(report["field_connectors"], json!([]));
    assert!(!report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["key"] == "field_connectors"));
}

#[test]
fn render_report_prints_the_links_section() {
    let header = |counts: &str| {
        format!(
            "links ({}) — relationship fields inside types (from links:):\n",
            counts
        )
    };
    let confirmed = link_selection(json!([link_entry(true, true)]));
    // Added.
    let text = render_report(
        &link_run(&confirmed, &LINK_SDL.replace(OWNER_FIELD, "")),
        "ws",
        None,
    );
    assert!(
        text.contains(&format!(
            "{}  + Widget_Co_Widget.owner  GET /owners/{{$this.owner_id}}   (link Widget > owner_id)\n",
            header("+1 −0 ~0 =0")
        )),
        "{}",
        text
    );
    // Removed.
    let text = render_report(
        &link_run(&link_selection(Value::Null), LINK_SDL),
        "ws",
        None,
    );
    assert!(
        text.contains(&format!(
            "{}  - Widget_Co_Widget.owner   (line 17; no included links: entry declares it)\n",
            header("+0 −1 ~0 =0")
        )),
        "{}",
        text
    );
    // Changed, with its drift indented under it.
    let text = render_report(
        &link_run(
            &confirmed,
            &LINK_SDL.replace("{$this.owner_id}", "{$this.id}"),
        ),
        "ws",
        None,
    );
    assert!(
        text.contains(&format!(
            "{}  ~ Widget_Co_Widget.owner   (link Widget > owner_id)\n      the path reads {{$this.id}}; the foreign key owner_id names on Widget_Co_Widget is owner_id\n",
            header("+0 −0 ~1 =0")
        )),
        "{}",
        text
    );
    // In step: the count line alone.
    let text = render_report(&link_run(&confirmed, LINK_SDL), "ws", None);
    assert!(text.contains(&header("+0 −0 ~0 =1")), "{}", text);
    assert!(!text.contains("  + Widget_Co_Widget") && !text.contains("  - Widget_Co_Widget"));
    // A note on a link with no host type.
    let mut inv = link_inventory();
    inv["shapes"]["Maker"] = json!({"type": "object", "properties": {
        "id": {"type": "string"}, "owner_id": {"type": "string",
            "candidate_entity_link": {"operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}}}});
    let mut entry = link_entry(true, true);
    entry["shape"] = json!("Maker");
    let report = run(
        &workspace(),
        &link_selection(json!([entry])),
        &inv,
        &LINK_SDL.replace(OWNER_FIELD, ""),
        None,
    );
    let text = render_report(&report, "ws", None);
    assert!(
        text.contains(&format!(
            "{}  · Maker > owner_id   no included operation's root field returns a type for shape Maker; the link has no host type in the schema yet\n",
            header("+0 −0 ~0 =0")
        )),
        "{}",
        text
    );
    // Field-level connectors outside links get their own lines.
    let sdl = sdl_with_field_connectors();
    let text = render_report(&link_run(&confirmed, &sdl), "ws", None);
    assert!(
        text.contains(&format!(
            "field connectors (5) — field-level connectors outside links:, not reconciled:\n  · Widget_Co_Repo.issues  GET /repos/{{$this.owner}}/{{$this.name}}/issues   (line {})\n",
            line_of(&sdl, "  issues(")
        )),
        "{}",
        text
    );
    // …and only there: the one-line `field_connectors` note `--json`
    // carries is not printed again under notes.
    assert_eq!(text.matches("Widget_Co_Repo.issues").count(), 1, "{}", text);
    assert!(!text.contains("  · field_connectors: "), "{}", text);
    // Nothing field-level at all: neither section.
    let text = render_report(
        &run(&workspace(), &selection(), &inventory(), SDL, None),
        "ws",
        None,
    );
    assert!(
        !text.contains("links (") && !text.contains("field connectors ("),
        "{}",
        text
    );
}

/// A fixture with the owners by-id read written `/owners/{ownerId}<suffix>`
/// everywhere it appears (operation key, path, candidate link, selection,
/// links: entry), as deck-of-cards keys `get:/deck/{deck_id}/`.
fn owners_path_as(v: &Value, suffix: &str) -> Value {
    serde_json::from_str(&serde_json::to_string(v).unwrap().replace(
        "/owners/{ownerId}",
        &format!("/owners/{{ownerId}}{}", suffix),
    ))
    .unwrap()
}

/// LINK_SDL with both the root field and the relationship field reading
/// `/owners/{…}<suffix>`.
fn owners_sdl_as(suffix: &str) -> String {
    let sdl = LINK_SDL
        .replace(
            "\"/owners/{$this.owner_id}\"",
            &format!("\"/owners/{{$this.owner_id}}{}\"", suffix),
        )
        .replace(
            "\"/owners/{$args.ownerId}\"",
            &format!("\"/owners/{{$args.ownerId}}{}\"", suffix),
        );
    assert!(
        sdl.contains(&format!("GET: \"/owners/{{$this.owner_id}}{}\"", suffix)),
        "fixture drifted"
    );
    sdl
}

#[test]
fn a_by_id_path_with_a_trailing_slash_or_an_extension_is_a_link_template() {
    for suffix in ["/", ".json"] {
        let key = format!("get:/owners/{{ownerId}}{}", suffix);
        let inv = owners_path_as(&link_inventory(), suffix);
        let sel = owners_path_as(&link_selection(json!([link_entry(true, true)])), suffix);
        let sdl = owners_sdl_as(suffix);
        let found: Vec<String> = link_connectors(&sdl)
            .into_iter()
            .map(|l| format!("{}.{}", l.type_name, l.field))
            .collect();
        assert_eq!(found, vec!["Widget_Co_Widget.owner"], "{}", suffix);
        assert!(field_connectors(&sdl).is_empty(), "{}", suffix);

        // The field exists and matches: unchanged, and clean.
        let report = run(&workspace(), &sel, &inv, &sdl, None);
        assert_eq!(report["selection_errors"], json!([]), "{}", report);
        assert_eq!(
            report["links"]["unchanged"],
            json!([{"key": "Widget > owner_id", "type": "Widget_Co_Widget", "field": "owner",
                    "operation": key, "line": 17}]),
            "{}",
            report
        );
        assert_eq!(report["links"]["add"], json!([]), "{}", report);
        assert_eq!(report["field_connectors"], json!([]), "{}", report);
        assert_eq!(report["clean"], true, "{}", report);

        // The field missing: links.add proposes the path with its suffix,
        // with the parameter recorded and with it derived from the path.
        let owner_field = OWNER_FIELD.replace(
            "{$this.owner_id}\"",
            &format!("{{$this.owner_id}}{}\"", suffix),
        );
        let bare = sdl.replace(&owner_field, "");
        assert_ne!(bare, sdl, "the variant must drop the field");
        let want = format!("/owners/{{$this.owner_id}}{}", suffix);
        let report = run(&workspace(), &sel, &inv, &bare, None);
        assert_eq!(
            report["links"]["add"][0]["path"],
            want.as_str(),
            "{}",
            report
        );
        let mut entry = link_entry(true, true);
        entry.as_object_mut().unwrap().remove("parameter");
        let unparametered = owners_path_as(&link_selection(json!([entry])), suffix);
        let report = run(&workspace(), &unparametered, &inv, &bare, None);
        assert_eq!(
            report["links"]["add"][0]["path"],
            want.as_str(),
            "{}",
            report
        );
    }
    // Only one trailing slash, and only an extension, rides after the key.
    for path in [
        "/owners/{$this.owner_id}//",
        "/owners/{$this.owner_id}-raw",
        "/owners/{$this.owner_id}.",
    ] {
        let sdl = LINK_SDL.replace("/owners/{$this.owner_id}", path);
        assert!(link_connectors(&sdl).is_empty(), "{}", path);
        assert_eq!(field_connectors(&sdl).len(), 1, "{}", path);
    }
}

#[test]
fn a_quoted_alias_names_the_output_key() {
    // On the left of `:` a quoted string is a key at every connect version
    // (`"100x100": _100x100`); the parser used to read it as a string value
    // and lose both the alias and the field it reads.
    let tree = parse_selection(r#""100x100": _100x100 plain"#);
    assert_eq!(tree[0].alias.as_deref(), Some("100x100"));
    assert_eq!(tree[0].key, Some(vec!["_100x100".to_string()]));
    assert!(!tree[0].opaque);
    assert_eq!(tree[1].key, Some(vec!["plain".to_string()]));
    // Single quotes are the same key.
    let tree = parse_selection("'100x100': _100x100 plain");
    assert_eq!(tree[0].alias.as_deref(), Some("100x100"));
    assert_eq!(tree[0].key, Some(vec!["_100x100".to_string()]));
    assert!(!tree[0].opaque);
    assert_eq!(tree[1].key, Some(vec!["plain".to_string()]));
    // A single-quoted value, with no `:` after it, stays a literal.
    let tree = parse_selection("tag: 'x' plain");
    assert!(tree[0].opaque);
    assert_eq!(tree[1].key, Some(vec!["plain".to_string()]));
}

// ── ADR 0080: root_properties sees through a resolved success/error union ──

fn shape_map() -> graphos_factory_core::json::Object {
    json!({
        "Success": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}},
        "Failure": {"type": "object", "properties": {"message": {"type": "string"}}},
        "StatusUnion": {"oneOf": [{"$ref": "#/shapes/Success"}, {"$ref": "#/shapes/Failure"}]}
    })
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn a_raw_union_with_neither_fact_nor_judgement_has_no_root_properties() {
    let op = json!({"response": {"envelope": null, "shape_ref": "#/shapes/StatusUnion"}});
    assert_eq!(
        root_properties(&op, &shape_map(), None),
        Vec::<String>::new()
    );
}

#[test]
fn the_inventory_fact_makes_the_success_branchs_properties_visible() {
    let op = json!({"response": {"envelope": null, "shape_ref": "#/shapes/StatusUnion", "referenced_shape": "#/shapes/Success"}});
    let mut roots = root_properties(&op, &shape_map(), None);
    roots.sort();
    assert_eq!(roots, vec!["id".to_string(), "name".to_string()]);
}

#[test]
fn a_selection_judgement_makes_the_success_branchs_properties_visible_when_no_fact_exists() {
    let op = json!({"response": {"envelope": null, "shape_ref": "#/shapes/StatusUnion"}});
    let selection_response = json!({"envelope": null, "referenced_shape": "#/shapes/Success"});
    let mut roots = root_properties(&op, &shape_map(), Some(&selection_response));
    roots.sort();
    assert_eq!(roots, vec!["id".to_string(), "name".to_string()]);
}

#[test]
fn the_inventory_fact_wins_over_a_selection_judgement_when_both_are_present() {
    let op = json!({"response": {"envelope": null, "shape_ref": "#/shapes/StatusUnion", "referenced_shape": "#/shapes/Success"}});
    // A stale or disagreeing selection judgement never overrides the fact.
    let selection_response = json!({"envelope": null, "referenced_shape": "#/shapes/Failure"});
    let mut roots = root_properties(&op, &shape_map(), Some(&selection_response));
    roots.sort();
    assert_eq!(roots, vec!["id".to_string(), "name".to_string()]);
}

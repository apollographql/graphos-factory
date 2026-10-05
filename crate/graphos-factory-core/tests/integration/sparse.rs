//! Sparse fieldsets (ADR 0045): the derived `fields` default and the
//! `sparse-fieldsets` lint rule, on a small Graph-like spec — a node GET
//! (`/act_{ad_account_id}`) and a paged edge (`/act_{ad_account_id}/campaigns`),
//! both taking a string `fields` query parameter.

use graphos_factory_core::lint::{lint_workspace, LintOptions};
use graphos_factory_core::sparse::{
    declared_arg, derive, dropped_parts, expansion_groups, list_root, top_level_names, Derived,
};
use serde_json::{json, Value};
use std::path::Path;

fn spec() -> Value {
    let fields = json!({"name": "fields", "in": "query", "schema": {"type": "string"}, "description": "Comma-separated fields."});
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Graph", "version": "21.0"},
        "servers": [{"url": "https://graph.test/v21.0"}],
        "components": {"schemas": {
            "AdAccount": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"}, "account_status": {"type": "integer"},
                "business": {"$ref": "#/components/schemas/Business"}
            }},
            "Business": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"},
                "primary_page": {"$ref": "#/components/schemas/Page"}
            }},
            "Page": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}},
            "Campaign": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"}, "daily_budget": {"type": "string"}
            }},
            "CampaignPage": {"type": "object", "properties": {
                "data": {"type": "array", "items": {"$ref": "#/components/schemas/Campaign"}},
                "paging": {"type": "object", "properties": {
                    "cursors": {"type": "object", "properties": {"before": {"type": "string"}, "after": {"type": "string"}}},
                    "next": {"type": "string"}
                }}
            }}
        }},
        "paths": {
            "/act_{ad_account_id}": {"get": {
                "operationId": "getAdAccount",
                "parameters": [{"name": "ad_account_id", "in": "path", "required": true, "schema": {"type": "string"}}, fields],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/AdAccount"}}}}}
            }},
            "/act_{ad_account_id}/campaigns": {"get": {
                "operationId": "getAdAccountCampaigns",
                "parameters": [
                    {"name": "ad_account_id", "in": "path", "required": true, "schema": {"type": "string"}}, fields,
                    {"name": "after", "in": "query", "schema": {"type": "string"}},
                    {"name": "limit", "in": "query", "schema": {"type": "integer"}}
                ],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/CampaignPage"}}}}}
            }}
        }
    })
}

fn inventory() -> Value {
    graphos_factory_core::openapi::build_inventory(&spec())
        .unwrap()
        .inventory
}

fn op<'a>(inv: &'a Value, key: &str) -> &'a Value {
    inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == key)
        .unwrap()
}

fn shapes(inv: &Value) -> serde_json::Map<String, Value> {
    inv["shapes"].as_object().unwrap().clone()
}

const NODE: &str = "get:/act_{ad_account_id}";
const EDGE: &str = "get:/act_{ad_account_id}/campaigns";

/// The inventory with expansion facts on `business` and `primary_page`, as
/// the converter writes them (the inventory conversion that keeps them is
/// ADR 0047, #80, expansion boundaries).
/// A list default is verified the way the contract requires, with evidence.
fn annotated(business_default: Value, page_default: Value) -> Value {
    let mut inv = inventory();
    inv["shapes"]["AdAccount"]["properties"]["business"]["x-expansion"] =
        expansion_fact("Business", business_default);
    inv["shapes"]["Business"]["properties"]["primary_page"]["x-expansion"] =
        expansion_fact("Page", page_default);
    inv
}

fn expansion_fact(target: &str, default: Value) -> Value {
    let mut x = json!({"target": target, "mechanism": "fields", "default": default});
    if x["default"].is_array() {
        x["evidence"] = json!({"kind": "doc", "ref": "https://graph.test/docs"});
    }
    x
}

fn fields(d: &str) -> Derived {
    Derived::Fields(d.to_string())
}

#[test]
fn a_node_get_derives_its_sorted_wire_names_not_its_graphql_renames() {
    let inv = inventory();
    assert_eq!(
        derive(
            op(&inv, NODE),
            &shapes(&inv),
            "name id accountStatus: account_status"
        ),
        fields("account_status,id,name")
    );
}

#[test]
fn a_paged_edge_derives_its_item_fields_and_leaves_out_the_page_wrapper() {
    let inv = inventory();
    let edge = op(&inv, EDGE);
    assert_eq!(edge["pagination"]["response"], "paging.cursors.after");
    assert_eq!(
        derive(
            edge,
            &shapes(&inv),
            "data { name dailyBudget: daily_budget id } paging { cursors { before after } next }"
        ),
        fields("daily_budget,id,name")
    );
}

#[test]
fn a_page_selecting_a_key_beside_its_items_is_not_derivable() {
    let mut inv = inventory();
    inv["shapes"]["CampaignPage"]["properties"]["summary"] = json!({"type": "object"});
    let d = derive(op(&inv, EDGE), &shapes(&inv), "data { id } summary");
    assert!(
        matches!(&d, Derived::NotDerivable(why) if why.contains("`summary` beside the page items")),
        "{:?}",
        d
    );
}

#[test]
fn a_child_beyond_the_verified_default_is_an_expansion_group() {
    let inv = annotated(json!(["id"]), json!(["id"]));
    let s = shapes(&inv);
    let node = op(&inv, NODE);
    assert_eq!(
        derive(node, &s, "id business { name }"),
        fields("business{name},id")
    );
    // Inside the default projection: no group.
    assert_eq!(
        derive(node, &s, "id business { id }"),
        fields("business,id")
    );
    // Nested: primary_page is itself a boundary inside Business.
    assert_eq!(
        derive(node, &s, "business { primary_page { name } }"),
        fields("business{primary_page{name}}")
    );
}

#[test]
fn a_two_leaf_verified_default_needs_no_group_and_an_unverified_one_always_groups() {
    let two = annotated(json!(["id", "name"]), json!(["id"]));
    assert_eq!(
        derive(op(&two, NODE), &shapes(&two), "id business { name }"),
        fields("business,id")
    );
    let unverified = annotated(json!("unverified"), json!(["id"]));
    assert_eq!(
        derive(
            op(&unverified, NODE),
            &shapes(&unverified),
            "id business { id }"
        ),
        fields("business{id},id")
    );
}

/// A `??` operand that reads a path is requested too, or the fallback never
/// arrives; a literal one names nothing and no longer makes the default
/// underivable. An optional step is still a path (ADR 0057).
#[test]
fn a_fallback_operand_is_requested_and_an_optional_step_is_still_a_path() {
    let inv = inventory();
    assert_eq!(
        derive(
            op(&inv, NODE),
            &shapes(&inv),
            "id title: name ?? account_status"
        ),
        fields("account_status,id,name")
    );
    assert_eq!(
        derive(
            op(&inv, NODE),
            &shapes(&inv),
            "id title: name ?? \"untitled\""
        ),
        fields("id,name")
    );
    let d = derive(op(&inv, NODE), &shapes(&inv), "id owner: business?.name");
    assert!(
        matches!(&d, Derived::NotDerivable(why) if why.contains("`business.name`")),
        "{:?}",
        d
    );
}

#[test]
fn an_unannotated_embedded_object_is_requested_whole() {
    let inv = inventory();
    assert_eq!(
        derive(op(&inv, NODE), &shapes(&inv), "id business { name }"),
        fields("business,id")
    );
}

#[test]
fn a_boundary_on_the_items_of_a_named_list_shape_is_seen_through_the_ref() {
    // `labels: {$ref: LabelList}`, the annotation on `LabelList.items`: the
    // form the converter writes, and the one `spans obligations` walks.
    let mut inv = inventory();
    inv["shapes"]["AdAccount"]["properties"]["labels"] = json!({"$ref": "#/shapes/LabelList"});
    inv["shapes"]["LabelList"] = json!({"type": "array", "items": {
        "$ref": "#/shapes/Page", "x-expansion": expansion_fact("Page", json!(["id"]))}});
    assert_eq!(
        derive(op(&inv, NODE), &shapes(&inv), "id labels { name }"),
        fields("id,labels{name}")
    );
}

#[test]
fn declared_arg_reads_the_type_and_the_string_default() {
    let text = "meta_adAccount(\n    \"\"\"Fields, e.g. a,b\"\"\"\n    fields: String = \"id,name\"\n    adAccountId: ID!\n  ): X";
    let a = declared_arg(text, "fields").unwrap();
    assert_eq!(a.type_, "String");
    assert_eq!(a.default.as_deref(), Some("id,name"));
    assert_eq!(declared_arg(text, "adAccountId").unwrap().default, None);
    assert!(declared_arg(text, "limit").is_none());
    assert_eq!(
        top_level_names("business{name,primary_page{id}},id"),
        vec!["business", "id"]
    );
}

// ─── The lint rule ──────────────────────────────────────────────────────────

const WORKSPACE: &str = "contract_version: 1\nservice: meta\ndirectory: meta\ntype_prefix: Meta\nfield_prefix: meta\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-23T00:00:00Z\n";

const SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/act_{ad_account_id}\":\n    include: true\n    response: { envelope: null, confirmed: true }\n    graphql: { root: query, name: adAccount }\n  \"get:/act_{ad_account_id}/campaigns\":\n    include: true\n    response: { envelope: null, confirmed: true }\n    graphql: { root: query, name: adAccountCampaigns }\n";

fn sdl(node_args: &str, node_query: &str, edge_args: &str, edge_query: &str) -> String {
    format!(
        r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "meta", http: {{ baseURL: "{{{{BASE_URL}}}}" }})

type Meta_AdAccount {{
  id: ID
  name: String
  accountStatus: Int
}}

type Meta_Campaign {{
  id: ID
  name: String
}}

type Meta_Cursors {{
  after: String
}}

type Meta_Paging {{
  cursors: Meta_Cursors
}}

type Meta_CampaignPage {{
  data: [Meta_Campaign]
  paging: Meta_Paging
}}

type Query {{
  meta_adAccount(adAccountId: ID!{}): Meta_AdAccount
    @connect(
      source: "meta"
      http: {{
        GET: "/act_{{$args.adAccountId}}"
        queryParams: """
{}
        """
      }}
      selection: """
      id
      name
      accountStatus: account_status
      """
    )

  meta_adAccountCampaigns(adAccountId: ID!, after: String{}): Meta_CampaignPage
    @connect(
      source: "meta"
      http: {{
        GET: "/act_{{$args.adAccountId}}/campaigns"
        queryParams: """
{}
        after: $args.after
        """
      }}
      selection: """
      data {{ id name }}
      paging {{ cursors {{ after }} }}
      """
    )
}}
"#,
        node_args, node_query, edge_args, edge_query
    )
}

const FORWARD: &str = "        fields: $args.fields";

fn good_sdl() -> String {
    sdl(
        ", fields: String = \"account_status,id,name\"",
        FORWARD,
        ", fields: String = \"id,name\"",
        FORWARD,
    )
}

fn workspace(sdl: &str, inventory: &Value, decisions: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", SELECTION);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(inventory),
    );
    w("meta.graphql", sdl);
    if let Some(d) = decisions {
        w(".factory/decisions.json", d);
    }
    dir
}

/// `(severity, message)` of every sparse-fieldsets finding.
fn sparse_findings(dir: &Path) -> Vec<(String, String)> {
    lint_workspace(
        dir,
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings
    .into_iter()
    .filter(|f| f.rule == "sparse-fieldsets")
    .map(|f| (f.severity, f.message))
    .collect()
}

#[test]
fn lint_accepts_the_derived_default_forwarded_as_the_argument() {
    let ws = workspace(&good_sdl(), &inventory(), None);
    assert_eq!(sparse_findings(ws.path()), vec![]);
}

#[test]
fn lint_rejects_a_wrong_or_missing_default_a_missing_argument_and_a_literal_forward() {
    let cases = [
        (
            sdl(
                ", fields: String = \"id,name\"",
                FORWARD,
                ", fields: String = \"id,name\"",
                FORWARD,
            ),
            "the `fields` default \"id,name\" is not the list its connector maps, \"account_status,id,name\"",
        ),
        (
            sdl(", fields: String", FORWARD, ", fields: String = \"id,name\"", FORWARD),
            "`fields` has no default; declare `= \"account_status,id,name\"`",
        ),
        (
            sdl("", "", ", fields: String = \"id,name\"", FORWARD),
            "declares no `fields` argument",
        ),
        (
            sdl(
                ", fields: String = \"account_status,id,name\"",
                "        fields: $(\"account_status,id,name\")",
                ", fields: String = \"id,name\"",
                FORWARD,
            ),
            "queryParams must carry `fields: $args.fields` (it sends `$(\"account_status,id,name\")`)",
        ),
        (
            sdl(
                ", fields: [String] = \"id\"",
                FORWARD,
                ", fields: String = \"id,name\"",
                FORWARD,
            ),
            "`fields` is [String]; declare it `String`",
        ),
    ];
    for (text, expected) in cases {
        let ws = workspace(&text, &inventory(), None);
        let found = sparse_findings(ws.path());
        assert!(
            found.iter().any(|(sev, m)| sev == "error"
                && m.starts_with(&format!("{}: Query.meta_adAccount", NODE))
                && m.contains(expected)),
            "expected {:?} in {:?}",
            expected,
            found
        );
        assert!(
            found
                .iter()
                .all(|(_, m)| !m.contains("meta_adAccountCampaigns")),
            "{:?}",
            found
        );
    }
}

const MODIFIER: &str = "account_status,campaigns.limit(10){name},id,name";

fn modifier_sdl() -> String {
    sdl(
        &format!(", fields: String = \"{}\"", MODIFIER),
        FORWARD,
        ", fields: String = \"id,name\"",
        FORWARD,
    )
}

#[test]
fn a_paging_modifier_is_rejected_without_a_decision_and_accepted_with_one() {
    let ws = workspace(&modifier_sdl(), &inventory(), None);
    let found = sparse_findings(ws.path());
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(
        found[0].1.contains("no resolved decision naming"),
        "{:?}",
        found
    );

    let decision = json!({"contract_version": 1, "decisions": [{
        "id": "D-0001", "title": "Ad account reads its first ten campaign names inline", "status": "resolved", "date": "2026-09-23",
        "context": format!("{} requests a paging modifier the derivation cannot express.", NODE),
        "resolution": {"decision": format!("Send `{}` as the fields default.", MODIFIER)}
    }]});
    let ws = workspace(&modifier_sdl(), &inventory(), Some(&decision.to_string()));
    assert_eq!(sparse_findings(ws.path()), vec![]);

    // A decision that names another operation does not count.
    let other = decision
        .to_string()
        .replace("act_{ad_account_id} requests", "somewhere else requests");
    let ws = workspace(&modifier_sdl(), &inventory(), Some(&other));
    assert_eq!(sparse_findings(ws.path()).len(), 1);
}

/// One resolved decision with this resolution; an empty context or
/// `affects` is left out.
fn one_decision(context: &str, decision: &str, affects: &[&str]) -> String {
    let mut d = json!({
        "id": "D-0001", "title": "The fields default", "status": "resolved", "date": "2026-09-23",
        "resolution": {"decision": decision}
    });
    if !context.is_empty() {
        d["context"] = json!(context);
    }
    if !affects.is_empty() {
        d["affects"] = json!(affects);
    }
    json!({"contract_version": 1, "decisions": [d]}).to_string()
}

/// The node GET declaring `default`; the edge keeps its derived one.
fn node_default(default: &str) -> String {
    sdl(
        &format!(", fields: String = \"{}\"", default),
        FORWARD,
        ", fields: String = \"id,name\"",
        FORWARD,
    )
}

/// The node's sparse-fieldsets findings for `sdl` under `decisions`.
fn node_findings_in(sdl: &str, decisions: &str) -> Vec<(String, String)> {
    let ws = workspace(sdl, &inventory(), Some(decisions));
    sparse_findings(ws.path())
        .into_iter()
        .filter(|(_, m)| m.starts_with(&format!("{}: Query.meta_adAccount", NODE)))
        .collect()
}

/// The node's findings for a derivable `default` under `decisions`.
fn node_findings(default: &str, decisions: &str) -> Vec<(String, String)> {
    node_findings_in(&node_default(default), decisions)
}

/// The node's findings for `default` when its connector also reads a path
/// (`business.name`) no fields list can name, so only a decision can settle
/// the default.
fn undecidable_findings(default: &str, decisions: &str) -> Vec<(String, String)> {
    let text = node_default(default).replace(
        "      accountStatus: account_status\n",
        "      accountStatus: account_status\n      businessName: business.name\n",
    );
    node_findings_in(&text, decisions)
}

#[test]
fn a_decision_about_an_operation_whose_key_the_node_prefixes_does_not_excuse_the_node() {
    // NODE is a prefix of EDGE: a decision about the edge names only the edge.
    let about_edge = one_decision(
        &format!("{} reads fewer fields.", EDGE),
        &format!("Send `id,name` as the fields default for {}.", EDGE),
        &[],
    );
    let found = undecidable_findings("id,name", &about_edge);
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(found[0].1.contains("cannot be derived"), "{:?}", found);
    // The same decision naming the node, even sentence-final, does.
    let about_node = one_decision(
        "",
        &format!("Send `id,name` as the fields default for {}.", NODE),
        &[],
    );
    assert_eq!(undecidable_findings("id,name", &about_node), vec![]);
    // So does an exact `affects` entry.
    let affects = one_decision("", "Send `id,name` as the fields default.", &[NODE]);
    assert_eq!(undecidable_findings("id,name", &affects), vec![]);
}

#[test]
fn a_one_field_or_empty_default_is_carried_only_by_its_exact_backtick_span() {
    // "provide" and "valid" contain `id`; every string contains "".
    let prose = one_decision(
        &format!("{} needs a narrower list.", NODE),
        "Provide a valid default, `id,name`, and keep it short.",
        &[],
    );
    for default in ["id", ""] {
        let found = undecidable_findings(default, &prose);
        assert_eq!(found.len(), 1, "{:?}: {:?}", default, found);
        assert!(found[0].1.contains("cannot be derived"), "{:?}", found);
    }
    // An empty backtick span does not carry the empty default either.
    let empty_span = one_decision("", &format!("Send `` for {}.", NODE), &[]);
    assert_eq!(undecidable_findings("", &empty_span).len(), 1);
    // The exact span does carry the one-field default, inline or fenced.
    let exact = one_decision("", &format!("Send `id` for {}.", NODE), &[]);
    assert_eq!(undecidable_findings("id", &exact), vec![]);
    let fenced = one_decision("", &format!("For {} send\n\n```text\nid\n```\n", NODE), &[]);
    assert_eq!(undecidable_findings("id", &fenced), vec![]);
}

#[test]
fn a_decision_cannot_drop_a_field_the_connector_maps_from_a_derivable_default() {
    // The node maps id, name and account_status: a decision recording `id`
    // would leave two mapped fields null against the real source.
    for default in ["id", "id,name"] {
        let d = one_decision("", &format!("Send `{}` for {}.", default, NODE), &[]);
        let found = node_findings(default, &d);
        assert_eq!(found.len(), 1, "{:?}: {:?}", default, found);
        assert!(
            found[0].1.contains("drops `account_status`")
                && found[0].1.contains("never remove from it"),
            "{:?}",
            found
        );
    }
    // Adding to the derived list is what a decision is for.
    let d = one_decision("", &format!("Send `{}` for {}.", MODIFIER, NODE), &[]);
    assert_eq!(node_findings(MODIFIER, &d), vec![]);
}

#[test]
fn a_literal_keeps_the_derived_list_as_a_tree_not_as_text() {
    let none: Vec<String> = vec![];
    // A paging modifier on a derived group keeps it.
    assert_eq!(
        dropped_parts("id,campaigns.limit(5){name}", "campaigns{name},id"),
        none
    );
    // A wider group keeps a derived bare boundary and a derived group.
    assert_eq!(
        dropped_parts("business{id,name,extra},id", "business,id"),
        none
    );
    assert_eq!(
        dropped_parts("business{extra,name}", "business{name}"),
        none
    );
    // Order and whitespace do not matter, at any depth.
    assert_eq!(
        dropped_parts(
            "id , business{ primary_page{ name,id } , name }",
            "business{name,primary_page{id,name}},id"
        ),
        none
    );
    // A dropped child is named by its path; a bare name does not carry a
    // derived group's children.
    assert_eq!(
        dropped_parts(
            "business{primary_page{id}},id",
            "business{name,primary_page{id,name}},id"
        ),
        vec!["business.name", "business.primary_page.name"]
    );
    assert_eq!(
        dropped_parts("business,id", "business{name},id"),
        vec!["business.name"]
    );
}

#[test]
fn an_expansion_group_is_keyed_by_its_name_without_a_modifier() {
    let groups = expansion_groups("id,labels.limit(5){id,text},business{campaigns.limit(2){name}}");
    assert_eq!(groups["labels"], vec!["id", "text"]);
    assert_eq!(groups["business"], vec!["campaigns"]);
    assert_eq!(groups["business.campaigns"], vec!["name"]);
    assert_eq!(groups.len(), 3, "{:?}", groups);
}

#[test]
fn a_non_string_fields_parameter_is_an_info_not_an_error() {
    let mut inv = inventory();
    for o in inv["operations"].as_array_mut().unwrap() {
        if o["key"] == NODE {
            for p in o["parameters"].as_array_mut().unwrap() {
                if p["name"] == "fields" {
                    p["type"] = json!("array");
                }
            }
        }
    }
    let ws = workspace(
        &sdl("", "", ", fields: String = \"id,name\"", FORWARD),
        &inv,
        None,
    );
    let found = sparse_findings(ws.path());
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(found[0].0, "info");
    assert!(found[0].1.contains("is array, not a string"), "{:?}", found);
}

#[test]
fn the_rule_follows_the_workspace_param_name() {
    let renamed = WORKSPACE.to_string() + "sparse_fieldsets: { param: select }\n";
    let ws = workspace(&sdl("", "", "", ""), &inventory(), None);
    std::fs::write(ws.path().join(".factory/workspace.yaml"), renamed).unwrap();
    assert_eq!(sparse_findings(ws.path()), vec![]);
}

#[test]
fn a_workspace_can_turn_the_rule_off() {
    // Neither GET declares or forwards `fields`: two errors each when on.
    let ws = workspace(&sdl("", "", "", ""), &inventory(), None);
    assert_eq!(sparse_findings(ws.path()).len(), 4);
    let off = WORKSPACE.to_string() + "sparse_fieldsets: { enabled: false }\n";
    std::fs::write(ws.path().join(".factory/workspace.yaml"), off).unwrap();
    let report = lint_workspace(
        ws.path(),
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    );
    let rules: Vec<&str> = report.findings.iter().map(|f| f.rule.as_str()).collect();
    assert!(!rules.contains(&"sparse-fieldsets"), "{:?}", rules);
    // The key is in the workspace schema: no contract finding for it.
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.message.contains("sparse_fieldsets")),
        "{:?}",
        report.findings
    );
}

/// A Graph-like spec with list GETs and no pagination fact — a `data`
/// wrapper, a Shopify-style `products` wrapper beside a count, a `results`
/// wrapper beside a count, a `data` wrapper beside a `meta` object — and a
/// node with an array property among its scalars.
fn list_inventory() -> Value {
    let fields = json!({"name": "fields", "in": "query", "schema": {"type": "string"}});
    let item = json!({"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}});
    let get = |schema: Value| {
        json!({"get": {"parameters": [fields.clone()],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": schema}}}}}})
    };
    let spec = json!({
        "openapi": "3.0.3",
        "info": {"title": "Graph", "version": "1"},
        "servers": [{"url": "https://graph.test"}],
        "paths": {
            "/adsets": get(json!({"type": "object", "properties": {"data": {"type": "array", "items": item}}})),
            "/products": get(json!({"type": "object", "properties": {
                "products": {"type": "array", "items": item}, "count": {"type": "integer"}}})),
            "/results": get(json!({"type": "object", "properties": {
                "results": {"type": "array", "items": item}, "count": {"type": "integer"}}})),
            "/meta": get(json!({"type": "object", "properties": {
                "data": {"type": "array", "items": item},
                "meta": {"type": "object", "properties": {"total": {"type": "integer"}}}}})),
            "/account": get(json!({"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"},
                "tags": {"type": "array", "items": {"type": "string"}}}})),
            // A node the envelope rule reads as a list: two properties,
            // one of them an array of objects.
            "/order": get(json!({"type": "object", "properties": {
                "customer": {"type": "object", "properties": {"name": {"type": "string"}}},
                "line_items": {"type": "array", "items": {"type": "object", "properties": {
                    "sku": {"type": "string"}, "qty": {"type": "integer"}}}}}}))
        }
    });
    graphos_factory_core::openapi::build_inventory(&spec)
        .unwrap()
        .inventory
}

#[test]
fn a_list_response_with_no_pagination_fact_is_not_read_as_a_node() {
    let inv = list_inventory();
    for (key, selection, root) in [
        ("get:/adsets", "data { id name }", "data"),
        ("get:/products", "products { id name }", "products"),
        ("get:/results", "results { id name } count", "results"),
        ("get:/meta", "data { id } meta { total }", "data"),
    ] {
        let o = op(&inv, key);
        assert!(o.get("pagination").is_none(), "{}", o);
        assert_eq!(list_root(o).as_deref(), Some(root), "{}", key);
        let d = derive(o, &shapes(&inv), selection);
        assert!(
            matches!(&d, Derived::NotDerivable(why) if why.contains(&format!("is a list under `{}`", root))),
            "{}: {:?}",
            key,
            d
        );
    }
    // An entity with an array property among its scalars is still a node.
    let account = op(&inv, "get:/account");
    assert_eq!(list_root(account), None);
    assert_eq!(
        derive(account, &shapes(&inv), "id name tags"),
        fields("id,name,tags")
    );
}

const LIST_SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/adsets\":\n    include: true\n    response: { envelope: null, confirmed: true }\n    graphql: { root: query, name: adsets }\n";

const LIST_SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "meta", http: { baseURL: "{{BASE_URL}}" })

type Meta_AdSet {
  id: ID
  name: String
}

type Meta_AdSetList {
  data: [Meta_AdSet]
}

type Query {
  meta_adsets(fields: String = "id,name"): Meta_AdSetList
    @connect(
      source: "meta"
      http: {
        GET: "/adsets"
        queryParams: """
        fields: $args.fields
        """
      }
      selection: """
      data { id name }
      """
    )
}
"#;

#[test]
fn a_list_wrapper_stub_keeps_its_wrapper_and_narrows_the_items() {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", LIST_SELECTION);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&list_inventory()),
    );
    w("meta.graphql", LIST_SDL);
    w("template.yaml", TEMPLATE);
    let argv = vec![dir.path().to_string_lossy().to_string()];
    assert_eq!(graphos_factory_core::cmd::scaffold::main(&argv), 0);
    let item_keys = |case: &str| -> Vec<String> {
        let body = stub(dir.path(), case)["response"]["jsonBody"].clone();
        let mut keys: Vec<String> = body["data"][0]
            .as_object()
            .unwrap_or_else(|| panic!("{}: no data items in {}", case, body))
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(item_keys("adsets"), vec!["id", "name"]);
    assert_eq!(item_keys("adsets_fields_narrowed"), vec!["id"]);
}

const ORDER_SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/order\":\n    include: true\n    response: { envelope: null, confirmed: true }\n    graphql: { root: query, name: order }\n";

const ORDER_SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "meta", http: { baseURL: "{{BASE_URL}}" })

type Meta_Customer {
  name: String
}

type Meta_LineItem {
  sku: String
  qty: Int
}

type Meta_Order {
  customer: Meta_Customer
  line_items: [Meta_LineItem]
}

type Query {
  meta_order(fields: String = "customer,line_items"): Meta_Order
    @connect(
      source: "meta"
      http: {
        GET: "/order"
        queryParams: """
        fields: $args.fields
        """
      }
      selection: """
      customer { name }
      line_items { sku qty }
      """
    )
}
"#;

#[test]
fn a_node_the_envelope_rule_misreads_as_a_list_is_trimmed_at_its_top_level() {
    // `{customer, line_items: [...]}` reads as a list under `line_items`,
    // but the default names `line_items` itself: the source trims the top
    // level, and the items keep their fields.
    let inv = list_inventory();
    assert_eq!(
        list_root(op(&inv, "get:/order")).as_deref(),
        Some("line_items")
    );
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", ORDER_SELECTION);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inv),
    );
    w("meta.graphql", ORDER_SDL);
    w("template.yaml", TEMPLATE);
    let argv = vec![dir.path().to_string_lossy().to_string()];
    assert_eq!(graphos_factory_core::cmd::scaffold::main(&argv), 0);
    let main = stub(dir.path(), "order")["response"]["jsonBody"].clone();
    let mut item: Vec<&String> = main["line_items"][0].as_object().unwrap().keys().collect();
    item.sort();
    assert_eq!(item, vec!["qty", "sku"], "{}", main);
    // The narrowed case asks for `customer` alone and gets it.
    let narrowed = stub(dir.path(), "order_fields_narrowed")["response"]["jsonBody"].clone();
    assert_eq!(
        narrowed.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["customer"],
        "{}",
        narrowed
    );
}

#[test]
fn the_missing_unit_exemption_needs_the_operation_to_take_the_parameter() {
    // The node's inventory operation takes no `fields`; its comma default is
    // the author's own, so it still needs a unit entry.
    let mut inv = inventory();
    for o in inv["operations"].as_array_mut().unwrap() {
        if o["key"] == NODE {
            o["parameters"]
                .as_array_mut()
                .unwrap()
                .retain(|p| p["name"] != "fields");
        }
    }
    let ws = workspace(&good_sdl(), &inv, None);
    std::fs::create_dir_all(ws.path().join("tests")).unwrap();
    std::fs::write(
        ws.path().join("tests/meta.connector.yaml"),
        "config:\n  schema: meta.graphql\ntests: []\n",
    )
    .unwrap();
    let missing: Vec<String> = lint_workspace(
        ws.path(),
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings
    .into_iter()
    .filter(|f| f.rule == "missing-unit")
    .map(|f| f.message)
    .collect();
    assert_eq!(missing.len(), 1, "{:?}", missing);
    assert!(missing[0].starts_with(NODE), "{:?}", missing);
}

#[test]
fn the_pilot_has_no_sparse_fieldsets_findings() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos");
    for pilot in ["gitea"] {
        assert_eq!(sparse_findings(&root.join(pilot)), vec![], "{}", pilot);
    }
}

// ─── Scaffold ───────────────────────────────────────────────────────────────

const TEMPLATE: &str = "variables:\n  - name: BASE_URL\n    test_default: \"https://graph.test/v21.0\"\n  - name: AUTH_EXPR\n    test_default: \"{$env.META_TOKEN}\"\n";

fn scaffolded(sdl: &str) -> tempfile::TempDir {
    let ws = workspace(sdl, &inventory(), None);
    std::fs::write(ws.path().join("template.yaml"), TEMPLATE).unwrap();
    let argv = vec![ws.path().to_string_lossy().to_string()];
    assert_eq!(graphos_factory_core::cmd::scaffold::main(&argv), 0);
    ws
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("{}: {}", rel, e))
}

fn stub(dir: &Path, case: &str) -> Value {
    graphos_factory_core::json::parse(&read(
        dir,
        &format!("tests/fixtures/mappings/{}.json", case),
    ))
    .unwrap()
}

#[test]
fn scaffold_writes_an_omitted_and_a_narrowed_case_per_sparse_get() {
    let ws = scaffolded(&good_sdl());
    let d = ws.path();
    for (case, default, one_body) in [
        ("ad_account", "account_status,id,name", false),
        ("ad_account_campaigns", "id,name", true),
    ] {
        // The main case omits the argument; its stub demands the default.
        let doc = read(d, &format!("tests/cases/{}.graphql", case));
        assert!(!doc.contains("fields:"), "{}", doc);
        let main = stub(d, case);
        assert_eq!(
            main["request"]["queryParameters"]["fields"],
            json!({"equalTo": default})
        );
        // The narrowed case passes one field; its stub demands exactly it
        // and answers with it alone.
        let narrowed = format!("{}_fields_narrowed", case);
        let ndoc = read(d, &format!("tests/cases/{}.graphql", narrowed));
        assert!(ndoc.contains("fields: \"id\""), "{}", ndoc);
        assert!(!ndoc.contains("name"), "{}", ndoc);
        let n = stub(d, &narrowed);
        assert_eq!(
            n["request"]["queryParameters"]["fields"],
            json!({"equalTo": "id"})
        );
        let body = &n["response"]["jsonBody"];
        let item = if one_body { &body["data"][0] } else { body };
        assert_eq!(
            item.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["id"],
            "{}",
            body
        );
        if one_body {
            assert!(
                body.get("paging").is_some(),
                "the page wrapper stays: {}",
                body
            );
        }
    }
    // Both defaults carry a comma: no unit entry for either, so no suite.
    assert!(!d.join("tests/meta.connector.yaml").exists());
}

#[test]
fn a_single_field_default_keeps_its_unit_entry_and_lint_accepts_the_comma_gets_without_one() {
    let text = good_sdl()
        .replace(
            "fields: String = \"account_status,id,name\"",
            "fields: String = \"id\"",
        )
        .replace(
            "      id\n      name\n      accountStatus: account_status\n",
            "      id\n",
        );
    let ws = scaffolded(&text);
    let suite = read(ws.path(), "tests/meta.connector.yaml");
    assert!(
        suite.contains("target: \"Query.meta_adAccount\""),
        "{}",
        suite
    );
    assert!(suite.contains("?fields=id"), "{}", suite);
    assert!(
        !suite.contains("Query.meta_adAccountCampaigns"),
        "{}",
        suite
    );
    let missing_unit: Vec<String> = lint_workspace(
        ws.path(),
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings
    .into_iter()
    .filter(|f| f.rule == "missing-unit")
    .map(|f| f.message)
    .collect();
    assert_eq!(missing_unit, Vec::<String>::new());
}

/// The node GET selecting `id` and, when `business` is given, that
/// selection under `business`, with `default` declared.
fn boundary_sdl(business: Option<&str>, default: &str) -> String {
    let selection = match business {
        Some(b) => format!("      id\n      business {{ {} }}\n", b),
        None => "      id\n".to_string(),
    };
    good_sdl()
        .replace(
            "fields: String = \"account_status,id,name\"",
            &format!("fields: String = \"{}\"", default),
        )
        .replace(
            "      id\n      name\n      accountStatus: account_status\n",
            &selection,
        )
        .replace(
            "  accountStatus: Int\n}",
            "  accountStatus: Int\n  business: Meta_Business\n}\n\ntype Meta_Business {\n  id: ID\n  name: String\n}",
        )
}

fn scaffolded_with(sdl: &str, inv: &Value) -> tempfile::TempDir {
    let ws = workspace(sdl, inv, None);
    std::fs::write(ws.path().join("template.yaml"), TEMPLATE).unwrap();
    let argv = vec![ws.path().to_string_lossy().to_string()];
    assert_eq!(graphos_factory_core::cmd::scaffold::main(&argv), 0);
    ws
}

#[test]
fn an_unexpanded_boundary_carries_exactly_its_verified_default_leaves() {
    let inv = annotated(json!(["id", "name"]), json!(["id"]));
    // Selecting business.name needs no expansion under a two-leaf default,
    // and the stub must supply name or e2e passes on a misleading null.
    let ws = scaffolded_with(&boundary_sdl(Some("name"), "business,id"), &inv);
    let body = &stub(ws.path(), "ad_account")["response"]["jsonBody"];
    let mut keys: Vec<&String> = body["business"].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["id", "name"], "{}", body);
}

#[test]
fn an_expanded_boundary_carries_the_requested_children() {
    let inv = annotated(json!(["id"]), json!(["id"]));
    let ws = scaffolded_with(&boundary_sdl(Some("name"), "business{name},id"), &inv);
    let body = &stub(ws.path(), "ad_account")["response"]["jsonBody"];
    assert_eq!(
        body["business"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["name"],
        "{}",
        body
    );
}

#[test]
fn an_unverified_boundary_is_left_out_of_the_body_with_a_note() {
    let inv = annotated(json!("unverified"), json!(["id"]));
    // The default names `business`, so the boundary rule, not the cut to the
    // default's fields, is what leaves it out.
    let ws = scaffolded_with(&boundary_sdl(None, "business,id"), &inv);
    let body = &stub(ws.path(), "ad_account")["response"]["jsonBody"];
    assert!(body.get("business").is_none(), "{}", body);
    assert!(body.get("id").is_some(), "{}", body);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "scaffold",
            ws.path().to_str().unwrap(),
            "--dry-run",
            "--json",
            "--op",
            NODE,
            "--force",
        ])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        report["written"][0]["notes"]
            .to_string()
            .contains("`business` is an expansion boundary whose default projection is unverified"),
        "{}",
        report
    );
}

/// The notes `scaffold --dry-run --json --op NODE` prints for `ws`.
fn node_notes(ws: &Path) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "scaffold",
            ws.to_str().unwrap(),
            "--dry-run",
            "--json",
            "--op",
            NODE,
            "--force",
        ])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    report["written"][0]["notes"].to_string()
}

#[test]
fn the_main_stub_answers_with_exactly_the_default_fields() {
    // AdAccount also documents `business`; the source would not send it.
    let ws = scaffolded(&good_sdl());
    let body = &stub(ws.path(), "ad_account")["response"]["jsonBody"];
    let mut keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["account_status", "id", "name"], "{}", body);
    // A default that leaves out a mapped field: the stub leaves it out too,
    // so e2e sees the null the source would give, and the notes say why.
    let short = good_sdl().replace(
        "fields: String = \"account_status,id,name\"",
        "fields: String = \"id,name\"",
    );
    let ws = scaffolded(&short);
    let body = &stub(ws.path(), "ad_account")["response"]["jsonBody"];
    let mut keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, vec!["id", "name"], "{}", body);
    let notes = node_notes(ws.path());
    assert!(
        notes.contains("does not request `account_status`, which the connector maps"),
        "{}",
        notes
    );
}

#[test]
fn a_workspace_with_the_rule_off_gets_ordinary_scaffold_cases() {
    let ws = workspace(&good_sdl(), &inventory(), None);
    std::fs::write(ws.path().join("template.yaml"), TEMPLATE).unwrap();
    std::fs::write(
        ws.path().join(".factory/workspace.yaml"),
        WORKSPACE.to_string() + "sparse_fieldsets: { enabled: false }\n",
    )
    .unwrap();
    let argv = vec![ws.path().to_string_lossy().to_string()];
    assert_eq!(graphos_factory_core::cmd::scaffold::main(&argv), 0);
    assert!(!ws
        .path()
        .join("tests/cases/ad_account_fields_narrowed.graphql")
        .exists());
    // `fields` is an ordinary argument again: passed, not left to a default.
    let doc = read(ws.path(), "tests/cases/ad_account.graphql");
    assert!(doc.contains("fields:"), "{}", doc);
}

/// `sparse-fieldsets` content matching reads the union of resolved
/// decisions and current findings (ADR 0113 §2): a current finding whose
/// body carries the literal and names the operation settles an
/// underivable default as a decision does; a superseded one does not.
#[test]
fn a_current_finding_carries_an_underivable_default_and_a_superseded_one_does_not() {
    let text = node_default("id,name").replace(
        "      accountStatus: account_status\n",
        "      accountStatus: account_status\n      businessName: business.name\n",
    );
    for (status, expected) in [("current", 0), ("superseded", 1)] {
        let ws = workspace(&text, &inventory(), None);
        let finding = json!({"contract_version": 1, "findings": [{
            "id": "F-0001", "title": "The fields default", "date": "2026-10-01",
            "status": status, "source": "agent",
            "body": format!("{} sends `id,name`: business.name is read through the expansion.", NODE),
        }]});
        std::fs::write(
            ws.path().join(".factory/findings.json"),
            finding.to_string(),
        )
        .unwrap();
        let found: Vec<(String, String)> = sparse_findings(ws.path())
            .into_iter()
            .filter(|(_, m)| m.starts_with(&format!("{}: Query.meta_adAccount", NODE)))
            .collect();
        assert_eq!(found.len(), expected, "{}: {:?}", status, found);
    }
}

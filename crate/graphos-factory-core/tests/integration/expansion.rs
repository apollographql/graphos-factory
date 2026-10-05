//! Expansion boundaries (ADR 0047): a response property annotated
//! `x-expansion` is a relationship to another node. The obligations walk
//! stops there — a verified default offers its leaves, an unverified one
//! offers one blocking row — instead of unfolding the target's whole graph.

use graphos_factory_core::obligations::{build, Class, Report};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Workspace(PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

const WORKSPACE_YAML: &str =
    "contract_version: 1\nservice: graph\ndirectory: graph\ntype_prefix: Graph\nfield_prefix: graph\n";

/// A one-service workspace with the given inventory operations and shapes,
/// and `fields` as the root fields of `type Query` / `type Mutation`.
fn workspace(
    tag: &str,
    operations: Value,
    shapes: Value,
    query: &str,
    mutation: &str,
) -> Workspace {
    let dir = std::env::temp_dir().join(format!(
        "expansion-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write(&dir, ".factory/workspace.yaml", WORKSPACE_YAML);
    let mut sdl = String::from(
        "extend schema\n  @link(url: \"https://specs.apollo.dev/connect/v0.3\", import: [\"@source\", \"@connect\"])\n\n@source(name: \"graph\", http: { baseURL: \"{{BASE_URL}}\" })\n\n",
    );
    sdl.push_str(&format!("type Query {{\n{}\n}}\n", query));
    if !mutation.is_empty() {
        sdl.push_str(&format!("\ntype Mutation {{\n{}\n}}\n", mutation));
    }
    write(&dir, "graph.graphql", &sdl);
    write(
        &dir,
        ".factory/inventory.json",
        &json!({
            "contract_version": 1,
            "api": {"title": "Graph", "base_urls": ["https://graph.test/v21.0"]},
            "operations": operations,
            "shapes": shapes
        })
        .to_string(),
    );
    Workspace(dir)
}

fn verified(target: &str, leaves: &[&str]) -> Value {
    json!({"target": target, "mechanism": "fields", "default": leaves,
        "evidence": {"kind": "doc", "ref": "https://developers.example.test/graph/v21"}})
}

fn unverified(target: &str) -> Value {
    json!({"target": target, "mechanism": "fields", "default": "unverified"})
}

/// Account has a verified one-leaf relationship (`owner`), a verified
/// two-leaf one (`business`), an unverified list relationship (`labels`) and
/// an unannotated embedded value (`settings`) whose `Node` reference is
/// followed as today.
fn shapes() -> Value {
    json!({
        "Account": {"type": "object", "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"},
            "owner": {"$ref": "#/shapes/Node", "x-expansion": verified("Node", &["id"])},
            "business": {"$ref": "#/shapes/Node", "x-expansion": verified("Node", &["id", "name"])},
            "labels": {"type": "array", "items": {"$ref": "#/shapes/Label", "x-expansion": unverified("Label")}},
            "settings": {"$ref": "#/shapes/Settings"}
        }},
        "Node": {"type": "object", "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"},
            "kind": {"type": "string"},
            "parent": {"$ref": "#/shapes/Node", "x-expansion": unverified("Node")}
        }},
        "Label": {"type": "object", "properties": {"id": {"type": "string"}, "text": {"type": "string"}}},
        "Settings": {"type": "object", "properties": {
            "timezone": {"type": "string"},
            "contact": {"$ref": "#/shapes/Contact"}
        }},
        "Contact": {"type": "object", "properties": {"email": {"type": "string"}, "phone": {"type": "string"}}}
    })
}

const GET_ACCOUNT: &str = "get:/{account_id}";

fn get_account_op() -> Value {
    json!([{"key": GET_ACCOUNT, "operation_id": "getAccount", "method": "GET", "path": "/{account_id}",
        "response": {"status": "200", "shape_ref": "#/shapes/Account"}}])
}

fn account_field(selection: &str) -> String {
    format!(
        "  graph_account(id: ID!): Graph_Account\n    @connect(source: \"graph\", http: {{ GET: \"/{{$args.id}}\" }}, selection: \"{}\")",
        selection
    )
}

fn report_for(tag: &str, selection: &str) -> Report {
    let ws = workspace(
        tag,
        get_account_op(),
        shapes(),
        &account_field(selection),
        "",
    );
    build(&ws.0, GET_ACCOUNT).unwrap()
}

fn paths(report: &Report) -> Vec<&str> {
    report.response.iter().map(|r| r.path.as_str()).collect()
}

fn class(report: &Report, path: &str) -> String {
    report
        .response
        .iter()
        .find(|r| r.path == path)
        .map(|r| r.class.label())
        .unwrap_or_else(|| panic!("no response row {:?} in {:?}", path, paths(report)))
}

// ─── (2) a verified default ─────────────────────────────────────────────────

#[test]
fn a_verified_default_offers_exactly_its_leaves_in_the_source_shape() {
    let report = report_for("verified-leaves", "id");
    let offered = paths(&report);
    assert!(offered.contains(&"owner.id"));
    assert!(offered.contains(&"business.id"));
    assert!(offered.contains(&"business.name"));
    // Nothing beyond the default: not Node's other fields, not its nested
    // relationship.
    for p in ["owner.name", "owner.kind", "owner.parent", "business.kind"] {
        assert!(!offered.contains(&p), "{} offered: {:?}", p, offered);
    }
}

#[test]
fn a_verified_leaf_is_mapped_when_selected_and_unaccounted_when_absent() {
    let mapped = report_for("verified-mapped", "id owner { id }");
    assert_eq!(class(&mapped, "owner.id"), "mapped");
    let absent = report_for("verified-absent", "id");
    assert_eq!(class(&absent, "owner.id"), "unaccounted");
}

#[test]
fn a_structured_parent_omission_covers_the_default_leaves() {
    let ws = workspace(
        "verified-omit",
        get_account_op(),
        shapes(),
        &account_field("id"),
        "",
    );
    write(
        &ws.0,
        ".factory/decisions.json",
        &json!({"contract_version": 1, "decisions": [{
            "id": "D-0001", "title": "Business is not exposed", "status": "resolved", "date": "2026-09-24",
            "resolution": {"decision": "not needed by the customer"},
            "omits": [{"operation": GET_ACCOUNT, "direction": "response", "path": "business", "reason": "editorial"}]
        }]})
        .to_string(),
    );
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    assert_eq!(class(&report, "business.id"), "omitted-decided (editorial)");
    assert_eq!(
        class(&report, "business.name"),
        "omitted-decided (editorial)"
    );
}

// ─── (3) an unverified default ──────────────────────────────────────────────

#[test]
fn an_unverified_default_is_one_blocking_row_and_its_children_are_not_enumerated() {
    let report = report_for("unverified", "id");
    let rows: Vec<_> = report
        .response
        .iter()
        .filter(|r| matches!(r.class, Class::UnverifiedDefault { .. }))
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", paths(&report));
    assert_eq!(rows[0].path, "labels[]");
    assert_eq!(
        rows[0].class.label(),
        "unverified-default (Label: default projection not verified)"
    );
    assert!(!paths(&report).iter().any(|p| p.starts_with("labels[].")));
    assert_eq!(report.response_counts().unverified_default, 1);
    let why = report.check_failure().unwrap();
    assert!(why.contains("response unverified-default 1"), "{}", why);
}

#[test]
fn neither_a_mapping_nor_an_omission_clears_an_unverified_default() {
    let mapped = report_for("unverified-mapped", "id labels { id }");
    assert!(class(&mapped, "labels[]").starts_with("unverified-default"));

    let ws = workspace(
        "unverified-omit",
        get_account_op(),
        shapes(),
        &account_field("id"),
        "",
    );
    write(
        &ws.0,
        ".factory/decisions.json",
        &json!({"contract_version": 1, "decisions": [{
            "id": "D-0001", "title": "Labels are not exposed", "status": "resolved", "date": "2026-09-24",
            "resolution": {"decision": "not needed"},
            "omits": [{"operation": GET_ACCOUNT, "direction": "response", "path": "labels[]", "reason": "editorial"}]
        }]})
        .to_string(),
    );
    let omitted = build(&ws.0, GET_ACCOUNT).unwrap();
    assert!(class(&omitted, "labels[]").starts_with("unverified-default"));
}

#[test]
fn check_fails_on_an_unverified_default_with_everything_else_accounted() {
    let ws = workspace(
        "unverified-check",
        get_account_op(),
        shapes(),
        &account_field("id name owner { id } business { id name } settings { timezone contact { email phone } }"),
        "",
    );
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    let c = report.response_counts();
    assert_eq!(
        (c.unaccounted, c.unresolved, c.unverified_default),
        (0, 0, 1)
    );
    assert_eq!(
        report.check_failure().as_deref(),
        Some("response unverified-default 1")
    );
    let code = graphos_factory_core::cmd::source_coverage::main(&[
        ws.0.to_str().unwrap().into(),
        GET_ACCOUNT.into(),
        "--check".into(),
    ]);
    assert_eq!(code, 1);
}

// ─── (9) ordinary traversal is unchanged ────────────────────────────────────

#[test]
fn an_unannotated_embedded_value_is_still_walked_to_its_leaves() {
    let report = report_for("embedded", "id");
    let offered = paths(&report);
    assert!(offered.contains(&"settings.timezone"));
    assert!(offered.contains(&"settings.contact.email"));
    assert!(offered.contains(&"settings.contact.phone"));
}

#[test]
fn the_request_side_ignores_x_expansion() {
    let op = "post:/{account_id}";
    let ws = workspace(
        "request",
        json!([{"key": op, "operation_id": "updateAccount", "method": "POST", "path": "/{account_id}",
            "request_body": {"content_type": "application/json", "shape_ref": "#/shapes/Account"},
            "response": {"status": "200", "shape_ref": "#/shapes/Label"}}]),
        shapes(),
        "  graph_ping: String",
        "  graph_updateAccount(id: ID!): Graph_Label\n    @connect(source: \"graph\", http: { POST: \"/{$args.id}\" }, selection: \"id text\")",
    );
    let report = build(&ws.0, op).unwrap();
    let request: Vec<&str> = report.request.iter().map(|r| r.path.as_str()).collect();
    // The whole Node is a request obligation, not only its default leaves,
    // and no request row is an unverified-default.
    for p in [
        "owner.id",
        "owner.name",
        "owner.kind",
        "labels[].id",
        "labels[].text",
    ] {
        assert!(request.contains(&p), "{} missing from {:?}", p, request);
    }
    assert_eq!(report.request_counts().unverified_default, 0);
}

// ─── (8) representation invariance ──────────────────────────────────────────

#[test]
fn an_inline_and_a_named_ordinary_object_produce_identical_obligations() {
    let named = shapes();
    let mut inline = shapes();
    inline["Account"]["properties"]["settings"] = named["Settings"].clone();
    let rows = |shapes: Value, tag: &str| {
        let ws = workspace(
            tag,
            get_account_op(),
            shapes,
            &account_field("id settings { timezone }"),
            "",
        );
        build(&ws.0, GET_ACCOUNT)
            .unwrap()
            .response
            .iter()
            .map(|r| (r.path.clone(), r.class.label()))
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(named, "named"), rows(inline, "inline"));
}

// ─── (4) an expansion in the outgoing fields expression ────────────────────

/// A root field that forwards `fields` with `default` as its declared
/// default, and maps `selection`.
fn forwarding_field(name: &str, type_: &str, path: &str, default: &str, selection: &str) -> String {
    format!(
        "  graph_{}(id: ID!, fields: String = \"{}\"): {}\n    @connect(source: \"graph\", http: {{ GET: \"{}\", queryParams: \"fields: $args.fields\" }}, selection: \"{}\")",
        name, default, type_, path, selection
    )
}

fn expanded_report(tag: &str, default: &str, selection: &str) -> Report {
    let field = forwarding_field(
        "account",
        "Graph_Account",
        "/{$args.id}",
        default,
        selection,
    );
    let ws = workspace(tag, get_account_op(), shapes(), &field, "");
    build(&ws.0, GET_ACCOUNT).unwrap()
}

#[test]
fn an_expanded_child_is_offered_from_the_wire_expression_not_the_mapping() {
    let report = expanded_report("expanded", "id,owner{id,name}", "id owner { id }");
    assert_eq!(class(&report, "owner.id"), "mapped");
    // Requested on the wire, not mapped: the source sends it and the schema
    // drops it.
    assert_eq!(class(&report, "owner.name"), "unaccounted");
    // Not requested: not offered.
    assert!(!paths(&report).contains(&"owner.kind"));
}

#[test]
fn an_expansion_replaces_an_unverified_default() {
    let report = expanded_report(
        "expanded-unverified",
        "id,labels{text}",
        "id labels { text }",
    );
    assert_eq!(class(&report, "labels[].text"), "mapped");
    assert!(!paths(&report).contains(&"labels[]"));
    assert_eq!(report.response_counts().unverified_default, 0);
}

#[test]
fn a_nested_group_walks_exactly_that_path() {
    let report = expanded_report("nested", "id,owner{parent{name}}", "id");
    let under_owner: Vec<&str> = paths(&report)
        .into_iter()
        .filter(|p| p.starts_with("owner"))
        .collect();
    assert_eq!(under_owner, vec!["owner.parent.name"]);
}

#[test]
fn a_nested_boundary_the_expression_does_not_expand_stops_at_its_default() {
    let report = expanded_report("nested-default", "id,owner{id,parent}", "id");
    assert!(class(&report, "owner.parent").starts_with("unverified-default (Node"));
    assert_eq!(class(&report, "owner.id"), "unaccounted");
}

#[test]
fn a_requested_name_the_target_does_not_document_is_unresolved() {
    let report = expanded_report("undocumented", "id,owner{nickname}", "id");
    assert!(class(&report, "owner.nickname").starts_with("unresolved ("));
}

#[test]
fn a_literal_fields_expression_is_read_like_a_forwarded_default() {
    let field = "  graph_account(id: ID!): Graph_Account\n    @connect(source: \"graph\", http: { GET: \"/{$args.id}\", queryParams: \"\"\"fields: $(\"id,owner{id,name}\")\"\"\" }, selection: \"id owner { id }\")";
    let ws = workspace("literal", get_account_op(), shapes(), field, "");
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    assert_eq!(class(&report, "owner.name"), "unaccounted");
}

/// Ad -> adset (a boundary) -> targeting (an embedded value, no
/// annotation); Campaign -> source_campaign (a boundary back to Campaign).
fn ads_shapes() -> Value {
    json!({
        "Ad": {"type": "object", "properties": {
            "id": {"type": "string"},
            "adset": {"$ref": "#/shapes/AdSet", "x-expansion": verified("AdSet", &["id"])}
        }},
        "AdSet": {"type": "object", "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"},
            "targeting": {"$ref": "#/shapes/Targeting"}
        }},
        "Targeting": {"type": "object", "properties": {
            "age_min": {"type": "integer"},
            "age_max": {"type": "integer"},
            "geo": {"type": "object", "properties": {"countries": {"type": "array", "items": {"type": "string"}}}}
        }},
        "Campaign": {"type": "object", "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"},
            "source_campaign": {"$ref": "#/shapes/Campaign", "x-expansion": unverified("Campaign")}
        }}
    })
}

fn one_get(key: &str, path: &str, shape: &str) -> Value {
    json!([{"key": key, "operation_id": "get", "method": "GET", "path": path,
        "response": {"status": "200", "shape_ref": format!("#/shapes/{}", shape)}}])
}

#[test]
fn an_embedded_value_reached_through_an_expansion_is_walked_whole() {
    let key = "get:/{ad_id}";
    let field = forwarding_field(
        "ad",
        "Graph_Ad",
        "/{$args.id}",
        "adset{targeting},id",
        "id adset { targeting { age_min } }",
    );
    let ws = workspace(
        "embedded-expanded",
        one_get(key, "/{ad_id}", "Ad"),
        ads_shapes(),
        &field,
        "",
    );
    let report = build(&ws.0, key).unwrap();
    assert_eq!(class(&report, "adset.targeting.age_min"), "mapped");
    assert_eq!(class(&report, "adset.targeting.age_max"), "unaccounted");
    assert_eq!(
        class(&report, "adset.targeting.geo.countries[]"),
        "unaccounted"
    );
    assert!(!paths(&report).contains(&"adset.id"));
}

// ─── (5) recursion through a boundary ───────────────────────────────────────

fn campaign_report(tag: &str, default: Option<&str>) -> Report {
    let key = "get:/{campaign_id}";
    let field = match default {
        Some(d) => forwarding_field("campaign", "Graph_Campaign", "/{$args.id}", d, "id"),
        None => "  graph_campaign(id: ID!): Graph_Campaign\n    @connect(source: \"graph\", http: { GET: \"/{$args.id}\" }, selection: \"id\")".to_string(),
    };
    let ws = workspace(
        tag,
        one_get(key, "/{campaign_id}", "Campaign"),
        ads_shapes(),
        &field,
        "",
    );
    build(&ws.0, key).unwrap()
}

#[test]
fn an_unexpanded_self_relationship_is_its_default_row() {
    let report = campaign_report("recursion-default", None);
    assert!(class(&report, "source_campaign").starts_with("unverified-default (Campaign"));
}

#[test]
fn an_expanded_self_relationship_is_offered_despite_the_cycle_guard() {
    let report = campaign_report("recursion-expanded", Some("id,source_campaign{name}"));
    assert_eq!(class(&report, "source_campaign.name"), "unaccounted");
    assert!(!paths(&report).contains(&"source_campaign"));
}

#[test]
fn a_repeated_finite_expansion_terminates() {
    let report = campaign_report(
        "recursion-twice",
        Some("id,source_campaign{source_campaign{name}}"),
    );
    let under: Vec<&str> = paths(&report)
        .into_iter()
        .filter(|p| p.starts_with("source_campaign"))
        .collect();
    assert_eq!(under, vec!["source_campaign.source_campaign.name"]);
}

// ─── a paged edge: groups are relative to the `data[]` item ────────────────

#[test]
fn a_paged_edge_reads_groups_relative_to_its_items() {
    let key = "get:/{account_id}/accounts";
    let shapes = {
        let mut s = shapes();
        s["AccountPage"] = json!({"type": "object", "properties": {
            "data": {"type": "array", "items": {"$ref": "#/shapes/Account"}},
            "paging": {"type": "object", "properties": {"cursors": {"type": "object", "properties": {"after": {"type": "string"}}}}}
        }});
        s
    };
    let op = json!([{"key": key, "operation_id": "list", "method": "GET", "path": "/{account_id}/accounts",
        "pagination": {"style": "cursor", "request": "after", "response": "paging.cursors.after"},
        "response": {"status": "200", "shape_ref": "#/shapes/AccountPage", "array_root_properties": ["data"]}}]);
    let field = forwarding_field(
        "accounts",
        "Graph_AccountPage",
        "/{$args.id}/accounts",
        "id,owner{id,name}",
        "data { id owner { id } }",
    );
    let ws = workspace("paged", op, shapes, &field, "");
    let report = build(&ws.0, key).unwrap();
    assert_eq!(class(&report, "data[].owner.id"), "mapped");
    assert_eq!(class(&report, "data[].owner.name"), "unaccounted");
}

// ─── (6) a boundary child the request never asks for ───────────────────────

fn transport_rows(report: &Report) -> Vec<(&str, String)> {
    report
        .response
        .iter()
        .filter(|r| matches!(r.class, Class::TransportExpansionMissing { .. }))
        .map(|r| (r.path.as_str(), r.class.label()))
        .collect()
}

fn check_exit(ws: &Workspace, op: &str) -> i32 {
    graphos_factory_core::cmd::source_coverage::main(&[
        ws.0.to_str().unwrap().into(),
        op.into(),
        "--check".into(),
    ])
}

#[test]
fn a_get_whose_fields_default_lacks_the_group_is_transport_expansion_missing() {
    let field = forwarding_field(
        "account",
        "Graph_Account",
        "/{$args.id}",
        "id,owner",
        "id owner { id name }",
    );
    let ws = workspace("transport-get", get_account_op(), shapes(), &field, "");
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    assert_eq!(
        transport_rows(&report),
        vec![(
            "owner.name",
            "transport-expansion-missing (the request does not ask for this child of `owner`; the source sends id)".to_string()
        )]
    );
    // Not an offered path: the counts' offered total is unchanged by it.
    assert_eq!(report.response_counts().transport_expansion_missing, 1);
    assert!(!report
        .response
        .iter()
        .any(|r| r.path == "owner.name"
            && !matches!(r.class, Class::TransportExpansionMissing { .. })));
    assert!(report
        .check_failure()
        .unwrap()
        .contains("response transport-expansion-missing 1"));
    assert_eq!(check_exit(&ws, GET_ACCOUNT), 1);
}

#[test]
fn a_get_with_no_fields_parameter_cannot_ask_either() {
    let report = report_for("transport-no-fields", "id owner { id name }");
    assert_eq!(transport_rows(&report).len(), 1);
    assert_eq!(transport_rows(&report)[0].0, "owner.name");
}

#[test]
fn a_child_outside_the_expansion_group_is_not_asked_for() {
    let report = expanded_report(
        "transport-outside",
        "id,owner{id,name}",
        "id owner { id name kind }",
    );
    let rows: Vec<&str> = transport_rows(&report)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(rows, vec!["owner.kind"]);
}

#[test]
fn a_child_within_the_default_or_the_expansion_is_fine() {
    let within_default = report_for("transport-default", "id owner { id } business { id name }");
    assert!(transport_rows(&within_default).is_empty());
    let within_expansion = expanded_report(
        "transport-expanded",
        "id,owner{id,name}",
        "id owner { id name }",
    );
    assert!(transport_rows(&within_expansion).is_empty());
}

#[test]
fn an_unverified_boundary_blocks_on_its_own_row_not_on_transport() {
    let report = report_for("transport-unverified", "id labels { id text }");
    assert!(transport_rows(&report).is_empty());
    assert!(class(&report, "labels[]").starts_with("unverified-default"));
}

const POST_ACCOUNT: &str = "post:/act_{account_id}/accounts";

fn post_workspace(tag: &str) -> Workspace {
    workspace(
        tag,
        json!([{"key": POST_ACCOUNT, "operation_id": "createAccount", "method": "POST", "path": "/act_{account_id}/accounts",
            "request_body": {"content_type": "application/json", "shape_ref": "#/shapes/Label"},
            "response": {"status": "200", "shape_ref": "#/shapes/Account"}}]),
        shapes(),
        "  graph_ping: String",
        "  graph_createAccount(id: ID!, text: String): Graph_Account\n    @connect(source: \"graph\", http: { POST: \"/act_{$args.id}/accounts\", body: \"text: $args.text\" }, selection: \"id owner { id name }\")",
    )
}

#[test]
fn a_post_response_exposing_a_boundary_child_is_transport_expansion_missing() {
    let ws = post_workspace("transport-post");
    let report = build(&ws.0, POST_ACCOUNT).unwrap();
    let rows: Vec<&str> = transport_rows(&report)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(rows, vec!["owner.name"]);
    assert_eq!(check_exit(&ws, POST_ACCOUNT), 1);
}

#[test]
fn a_decision_recording_the_verified_wire_behaviour_clears_a_post_row() {
    let ws = post_workspace("transport-post-decided");
    write(
        &ws.0,
        ".factory/decisions.json",
        &json!({"contract_version": 1, "decisions": [{
            "id": "D-0001", "title": "createAccount returns the owner's name", "status": "resolved", "date": "2026-09-24",
            "affects": [POST_ACCOUNT],
            "resolution": {"decision": "probed 2026-09-24: the response carries `owner.name` without expansion"}
        }]})
        .to_string(),
    );
    let report = build(&ws.0, POST_ACCOUNT).unwrap();
    assert!(transport_rows(&report).is_empty());
}

/// `GET_ACCOUNT` taking a string `fields` query parameter: it can ask.
fn get_account_op_with_fields() -> Value {
    let mut ops = get_account_op();
    ops[0]["parameters"] = json!([{"name": "fields", "in": "query", "type": "string"}]);
    ops
}

#[test]
fn a_decision_does_not_clear_a_row_the_get_could_fix_in_its_fields_default() {
    let decision = json!({"contract_version": 1, "decisions": [{
        "id": "D-0001", "title": "The account returns its owner's name", "status": "resolved", "date": "2026-09-24",
        "affects": [GET_ACCOUNT],
        "resolution": {"decision": "probed: the response carries `owner.name` without expansion"}
    }]})
    .to_string();
    let field = forwarding_field(
        "account",
        "Graph_Account",
        "/{$args.id}",
        "id,owner",
        "id owner { id name }",
    );
    // The GET takes and forwards `fields`: the fix is `owner{id,name}` in the
    // default, so the row stands despite the decision.
    let ws = workspace(
        "transport-get-decided",
        get_account_op_with_fields(),
        shapes(),
        &field,
        "",
    );
    write(&ws.0, ".factory/decisions.json", &decision);
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    let rows: Vec<&str> = transport_rows(&report)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(rows, vec!["owner.name"]);
    // A GET with no fields parameter cannot ask, so the decision clears it.
    let ws = workspace(
        "transport-get-no-param-decided",
        get_account_op(),
        shapes(),
        &account_field("id owner { id name }"),
        "",
    );
    write(&ws.0, ".factory/decisions.json", &decision);
    assert!(transport_rows(&build(&ws.0, GET_ACCOUNT).unwrap()).is_empty());
}

// ─── groups relative to a list's item; modifiers ───────────────────────────

const LIST_ACCOUNTS: &str = "get:/accounts";

#[test]
fn an_unpaged_list_reads_its_groups_relative_to_the_item() {
    // `{results: [Account], count}` with no pagination fact: the facts
    // suggest the envelope `results`, so `owner{id,name}` is the item's.
    let mut s = shapes();
    s["AccountList"] = json!({"type": "object", "properties": {
        "results": {"type": "array", "items": {"$ref": "#/shapes/Account"}},
        "count": {"type": "integer"}}});
    let ops = json!([{"key": LIST_ACCOUNTS, "operation_id": "listAccounts", "method": "GET", "path": "/accounts",
        "parameters": [{"name": "fields", "in": "query", "type": "string"}],
        "response": {"status": "200", "shape_ref": "#/shapes/AccountList", "root_property_count": 2,
            "array_root_properties": ["results"], "total_items_property": "count"}}]);
    let field = forwarding_field(
        "accounts",
        "Graph_AccountList",
        "/accounts",
        "id,owner{id,name}",
        "results { id owner { id name } } count",
    );
    let ws = workspace("unpaged-list", ops, s, &field, "");
    let report = build(&ws.0, LIST_ACCOUNTS).unwrap();
    assert_eq!(class(&report, "results[].owner.name"), "mapped");
    assert!(transport_rows(&report).is_empty(), "{:?}", paths(&report));
}

#[test]
fn a_modifier_on_an_expanded_group_does_not_hide_the_group() {
    let report = expanded_report(
        "modifier",
        "id,labels.limit(5){id,text}",
        "id labels { id text }",
    );
    assert_eq!(class(&report, "labels[].text"), "mapped");
    assert_eq!(report.response_counts().unverified_default, 0);
}

#[test]
fn a_verified_default_s_leaves_are_trimmed() {
    let mut s = shapes();
    s["Account"]["properties"]["owner"]["x-expansion"] = verified("Node", &[" id "]);
    let ws = workspace(
        "trimmed-leaves",
        get_account_op(),
        s,
        &account_field("id owner { id }"),
        "",
    );
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    assert_eq!(class(&report, "owner.id"), "mapped");
}

// ─── a default is verified only with evidence ──────────────────────────────

#[test]
fn an_empty_malformed_or_unevidenced_default_is_unverified() {
    let evidence = json!({"kind": "doc", "ref": "https://developers.example.test/graph/v21"});
    let cases = [
        json!({"target": "Node", "mechanism": "fields", "default": [], "evidence": evidence}),
        json!({"target": "Node", "mechanism": "fields", "default": [1, 2], "evidence": evidence}),
        json!({"target": "Node", "mechanism": "fields", "default": ["id"]}),
        json!({"target": "Node", "mechanism": "fields", "default": ["id"],
            "evidence": {"kind": "guess", "ref": "https://developers.example.test"}}),
        json!({"target": "Node", "mechanism": "fields", "default": ["id"],
            "evidence": {"kind": "probe", "ref": " "}}),
    ];
    for (i, x) in cases.into_iter().enumerate() {
        let mut s = shapes();
        s["Account"]["properties"]["owner"]["x-expansion"] = x.clone();
        let ws = workspace(
            &format!("malformed-{}", i),
            get_account_op(),
            s,
            &account_field("id"),
            "",
        );
        let report = build(&ws.0, GET_ACCOUNT).unwrap();
        assert!(
            class(&report, "owner").starts_with("unverified-default"),
            "{}: {:?}",
            x,
            paths(&report)
        );
        assert!(report.check_failure().is_some(), "{}", x);
    }
}

// ─── a boundary beside a `$ref` to a named list shape ──────────────────────

/// `shapes()` with `labels` a `$ref` to a named array shape, annotated beside
/// the reference: the form the converter's `$ref` fast path keeps.
fn named_list_shapes(x: Value) -> Value {
    let mut s = shapes();
    s["Account"]["properties"]["labels"] = json!({"$ref": "#/shapes/LabelList", "x-expansion": x});
    s["LabelList"] = json!({"type": "array", "items": {"$ref": "#/shapes/Label"}});
    s
}

#[test]
fn a_verified_boundary_on_a_named_list_shape_offers_its_leaves_under_the_items() {
    let ws = workspace(
        "named-list-verified",
        get_account_op(),
        named_list_shapes(verified("Label", &["id"])),
        &account_field("id labels { id text }"),
        "",
    );
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    assert_eq!(class(&report, "labels[].id"), "mapped");
    assert!(!paths(&report).iter().any(|p| p.starts_with("labels.")));
    // The mapped child past the default is seen under the items too.
    let rows: Vec<&str> = transport_rows(&report)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    assert_eq!(rows, vec!["labels[].text"]);
}

#[test]
fn an_expanded_boundary_on_a_named_list_shape_offers_the_requested_children() {
    let field = forwarding_field(
        "account",
        "Graph_Account",
        "/{$args.id}",
        "id,labels{text}",
        "id labels { text }",
    );
    let ws = workspace(
        "named-list-expanded",
        get_account_op(),
        named_list_shapes(verified("Label", &["id"])),
        &field,
        "",
    );
    let report = build(&ws.0, GET_ACCOUNT).unwrap();
    assert_eq!(class(&report, "labels[].text"), "mapped");
    assert!(!paths(&report).iter().any(|p| p.starts_with("labels.")));
    assert!(transport_rows(&report).is_empty());
}

// ─── inventory describe ─────────────────────────────────────────────────────

#[test]
fn describe_keeps_the_relationship_beside_the_target_fields() {
    let shapes = shapes();
    let expanded = graphos_factory_core::inventory::expand_shape(
        &json!({"$ref": "#/shapes/Account"}),
        shapes.as_object().unwrap(),
        0,
        &Default::default(),
    );
    let owner = &expanded["properties"]["owner"];
    assert_eq!(owner["x-expansion"], verified("Node", &["id"]));
    // The target's fields are still listed under the relationship.
    for f in ["id", "name", "kind"] {
        assert!(owner["properties"][f].is_object(), "{} missing", f);
    }
    let labels = &expanded["properties"]["labels"]["items"];
    assert_eq!(labels["x-expansion"]["default"], "unverified");
    assert!(labels["properties"]["text"].is_object());
    // An unannotated reference expands exactly as before.
    assert!(expanded["properties"]["settings"]
        .get("x-expansion")
        .is_none());
}

// ─── (7) other consumers still see the whole target ────────────────────────

#[test]
fn conformance_still_validates_an_expanded_child() {
    use graphos_factory_core::conformance::conform;
    let shapes = shapes();
    let shapes = shapes.as_object().unwrap();
    let account = json!({"$ref": "#/shapes/Account"});
    let ok = json!({"id": "1", "owner": {"id": "2", "name": "Acme"}});
    assert!(conform(&ok, &account, shapes, "$", "response").is_empty());
    let bad = json!({"id": "1", "owner": {"id": "2", "name": 123}});
    let problems = conform(&bad, &account, shapes, "$", "response");
    assert_eq!(problems.len(), 1, "{:?}", problems);
    assert!(problems[0].contains("$.owner.name"), "{:?}", problems);
}

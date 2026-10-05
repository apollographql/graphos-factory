//! The zero-match guard (ADR 0074): when the schema has connectors and not
//! one pairs with an inventory operation, reconcile says so once, with the
//! URLs that show the base-URL split, and exits 2. The shapes are the ones
//! the 2026-09-29 survey found: Confluence (server `/wiki/api/v2`,
//! `BASE_URL` `/wiki`, connectors `/api/v2/...`) trips it; Calendar (the
//! prefix in `BASE_URL`) and Jira (no path) do not.

use serde_json::{json, Value};
use std::path::Path;

/// A one-operation workspace: the inventory's server URL and operation
/// paths, `BASE_URL`, and the connector paths the schema writes (none: a root field with no `@connect`).
fn workspace(
    server: &str,
    op_paths: &[&str],
    base_url: &str,
    connector_paths: &[&str],
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let p = dir.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    write(
        ".factory/workspace.yaml",
        "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\n",
    );
    let ops: Vec<Value> = op_paths
        .iter()
        .map(|p| {
            json!({ "key": format!("get:{}", p), "method": "GET", "path": p, "parameters": [],
                    "response": { "status": "200", "content_type": "application/json", "shape_ref": null } })
        })
        .collect();
    write(
        ".factory/inventory.json",
        &serde_json::to_string_pretty(&json!({
            "contract_version": 1,
            "api": { "title": "Widget Co", "base_urls": [server] },
            "operations": ops, "shapes": {}, "unresolved": []
        }))
        .unwrap(),
    );
    let mut selection = String::from("contract_version: 1\noperations:\n");
    for (i, p) in op_paths.iter().enumerate() {
        selection.push_str(&format!(
            "  \"get:{}\":\n    include: true\n    response:\n      envelope: null\n    graphql:\n      root: query\n      name: op{}\n",
            p, i
        ));
    }
    write(".factory/selection.yaml", &selection);
    let mut fields = String::new();
    for (i, p) in op_paths.iter().enumerate() {
        match connector_paths.get(i) {
            Some(c) => fields.push_str(&format!(
                "  widget_co_op{}: String\n    @connect(source: \"widget_co\", http: {{ GET: \"{}\" }}, selection: \"$\")\n",
                i, c
            )),
            None => fields.push_str(&format!("  widget_co_op{}: String\n", i)),
        }
    }
    write(
        "widget-co.graphql",
        &format!(
            "extend schema\n  @link(url: \"https://specs.apollo.dev/connect/v0.3\", import: [\"@source\", \"@connect\"])\n  @source(name: \"widget_co\", http: {{ baseURL: \"{{{{BASE_URL}}}}\" }})\n\ntype Query {{\n{}}}\n",
            fields
        ),
    );
    write(
        "template.yaml",
        &format!(
            "variables:\n  - name: BASE_URL\n    description: \"Base URL\"\n    test_default: \"{}\"\n",
            base_url
        ),
    );
    dir
}

fn report(dir: &Path) -> Value {
    graphos_factory_core::reconcile::reconcile_workspace(dir, None).unwrap()
}

fn exit(dir: &Path) -> i32 {
    graphos_factory_core::cmd::reconcile::main(&[dir.to_string_lossy().to_string()])
}

#[test]
fn a_confluence_shaped_split_trips_the_guard_with_the_urls_to_compare() {
    let ws = workspace(
        "https://no-default/wiki/api/v2",
        &["/spaces", "/pages"],
        "https://acme.atlassian.net/wiki",
        &["/api/v2/spaces", "/api/v2/pages"],
    );
    let r = report(ws.path());
    let z = r.get("zero_match").expect("the guard trips");
    assert_eq!(z["connectors"], 2);
    assert_eq!(
        z["inventory_server_urls"],
        json!(["https://no-default/wiki/api/v2"])
    );
    assert_eq!(z["base_url"], "https://acme.atlassian.net/wiki");
    // The core's own files and nothing else: no target file is a fact here.
    assert_eq!(
        z.as_object().unwrap().keys().collect::<Vec<_>>(),
        vec![
            "connectors",
            "inventory_server_urls",
            "base_url",
            "sample_connector_path"
        ]
    );
    assert_eq!(z["sample_connector_path"], "/api/v2/spaces");
    assert_eq!(r["clean"], false);
    let message = graphos_factory_core::reconcile::zero_match_message(z);
    assert!(
        message.contains("none of the 2 connectors pairs"),
        "{}",
        message
    );
    assert!(message.contains("A base URL with a path breaks every layer that compares paths"));
    assert_eq!(exit(ws.path()), 2);
}

#[test]
fn a_calendar_shaped_prefix_in_base_url_pairs_and_stays_clean() {
    let ws = workspace(
        "https://www.googleapis.com/calendar/v3",
        &["/colors"],
        "https://www.googleapis.com/calendar/v3",
        &["/colors"],
    );
    let r = report(ws.path());
    assert!(r.get("zero_match").is_none(), "{}", r);
    assert_eq!(exit(ws.path()), 0);
}

#[test]
fn a_jira_shaped_bare_host_pairs_and_stays_clean() {
    let ws = workspace(
        "https://acme.atlassian.net",
        &["/rest/api/3/myself"],
        "https://acme.atlassian.net",
        &["/rest/api/3/myself"],
    );
    assert!(report(ws.path()).get("zero_match").is_none());
    assert_eq!(exit(ws.path()), 0);
}

#[test]
fn a_workspace_with_no_connectors_is_not_affected() {
    let ws = workspace(
        "https://no-default/wiki/api/v2",
        &["/spaces"],
        "https://acme.atlassian.net/wiki",
        &[],
    );
    let r = report(ws.path());
    assert_eq!(r["connectors"], 0);
    assert!(r.get("zero_match").is_none());
    assert_ne!(exit(ws.path()), 2, "a fresh schema is a delta, not a split");
}

#[test]
fn one_pairing_connector_keeps_the_ordinary_unmatched_delta() {
    // Only one of two connectors is off: that is a delta to fix, exit 1,
    // not the split the guard describes.
    let ws = workspace(
        "https://acme.atlassian.net",
        &["/rest/api/3/myself", "/rest/api/3/field"],
        "https://acme.atlassian.net",
        &["/rest/api/3/myself", "/rest/api/2/field"],
    );
    let r = report(ws.path());
    assert!(r.get("zero_match").is_none());
    assert_eq!(r["unmatched"].as_array().unwrap().len(), 1);
    assert_eq!(exit(ws.path()), 1);
}

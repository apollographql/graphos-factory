//! `inventory diff` and `inventory describe` work on inventory.json itself,
//! whatever description format built it; the fixture here is an OpenAPI 3
//! document only because that is the shortest way to get an inventory.

use graphos_factory_core::inventory::{cap, diff_inventories, expand_shape, shape_name};
use graphos_factory_core::openapi::build_inventory;
use serde_json::{json, Value};
use std::collections::HashSet;

fn spec() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Widgets API", "version": "1.2.0"},
        "servers": [{"url": "https://api.widgets.test/{stage}", "variables": {"stage": {"default": "v1"}}}],
        "components": {
            "securitySchemes": {"bearerAuth": {"type": "http", "scheme": "bearer"}},
            "schemas": {
                "Widget": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "owner": {"$ref": "#/components/schemas/User"}}},
                "User": {"type": "object", "properties": {"id": {"type": "string"}, "widget": {"$ref": "#/components/schemas/Widget"}}},
                "WidgetList": {"type": "object", "properties": {
                    "widgets": {"type": "array", "items": {"$ref": "#/components/schemas/Widget"}},
                    "page_info": {"type": "object", "properties": {"next_cursor": {"type": "string"}}}
                }}
            }
        },
        "paths": {
            "/widgets": {
                "parameters": [{"name": "Accept", "in": "header", "schema": {"type": "string"}}],
                "get": {
                    "operationId": "listWidgets", "summary": "List widgets", "tags": ["Widgets"],
                    "parameters": [{"name": "cursor", "in": "query", "schema": {"type": "string"}}, {"name": "limit", "in": "query", "schema": {"type": "integer"}}],
                    "responses": {
                        "200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/WidgetList"}}}},
                        "404": {"description": "missing"}
                    }
                },
                "post": {
                    "operationId": "createWidget", "tags": ["Widgets"],
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}},
                    "responses": {"201": {"description": "created", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}}}
                }
            },
            "/widgets/{widgetId}": {
                "get": {
                    "operationId": "getWidget", "tags": ["Widgets"],
                    "parameters": [{"name": "widgetId", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}}}
                },
                "delete": {
                    "operationId": "deleteWidget",
                    "parameters": [{"name": "widgetId", "in": "path", "required": true, "schema": {"type": "string"}}],
                    "responses": {"204": {"description": "gone"}}
                }
            },
            "/widgets/export": {
                "get": {"operationId": "exportWidgets", "responses": {"200": {"description": "ok", "content": {"text/csv": {"schema": {"type": "string"}}}}}}
            }
        }
    })
}

fn build(s: &Value) -> Value {
    build_inventory(s).unwrap().inventory
}

#[test]
fn diff_reports_added_removed_and_changed_operations() {
    let before = build(&spec());
    let mut s = spec();
    s["paths"]
        .as_object_mut()
        .unwrap()
        .remove("/widgets/export");
    s["paths"]["/gadgets"] = json!({"get": {"operationId": "listGadgets", "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object"}}}}}}});
    s["paths"]["/widgets"]["get"]["summary"] = json!("List all widgets");
    let after = build(&s);
    let diff = diff_inventories(&before, &after);
    assert_eq!(diff["added"], json!(["get:/gadgets"]));
    assert_eq!(diff["removed"], json!(["get:/widgets/export"]));
    assert_eq!(
        diff["changed"],
        json!([{"key": "get:/widgets", "fields": ["summary"]}])
    );
}

#[test]
fn diff_notices_a_response_shape_change_behind_an_unchanged_ref() {
    let before = build(&spec());
    let mut s = spec();
    s["components"]["schemas"]["Widget"]["properties"]["color"] = json!({"type": "string"});
    let after = build(&s);
    let changed: Vec<Value> = diff_inventories(&before, &after)["changed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].clone())
        .collect();
    assert!(changed.contains(&json!("get:/widgets/{widgetId}")));
}

#[test]
fn expand_shape_inlines_refs_and_marks_recursion() {
    let inv = build(&spec());
    let expanded = expand_shape(
        &json!({"$ref": "#/shapes/Widget"}),
        inv["shapes"].as_object().unwrap(),
        0,
        &HashSet::new(),
    );
    assert_eq!(
        expanded["properties"]["owner"]["properties"]["widget"]["$recursive"],
        "Widget"
    );
}

#[test]
fn shape_names_and_synthesized_names_are_format_neutral() {
    assert_eq!(shape_name("#/shapes/Widget"), "Widget");
    assert_eq!(shape_name("#/components/schemas/Widget"), "Widget");
    assert_eq!(shape_name("#/definitions/Widget"), "Widget");
    assert_eq!(cap("get"), "Get");
    assert_eq!(cap("chat-files"), "ChatFiles");
    assert_eq!(cap("v2/incidents_log"), "V2IncidentsLog");
}

#[test]
fn a_changed_pagination_or_response_is_reported_once_each() {
    let mk = |envelope: &str, cursor: bool| -> serde_json::Value {
        let mut params = vec![
            serde_json::json!({"name": "limit", "in": "query", "schema": {"type": "integer"}}),
        ];
        if cursor {
            params.push(
                serde_json::json!({"name": "cursor", "in": "query", "schema": {"type": "string"}}),
            );
        }
        serde_json::json!({
            "openapi": "3.0.3", "info": {"title": "x", "version": "1"},
            "servers": [{"url": "https://api.example.test"}],
            "paths": {"/widgets": {"get": {"operationId": "listWidgets", "parameters": params,
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {
                    envelope: {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}}}}}}}}}}}
        })
    };
    let before = graphos_factory_core::openapi::build_inventory(&mk("items", false))
        .unwrap()
        .inventory;
    let after = graphos_factory_core::openapi::build_inventory(&mk("data", true))
        .unwrap()
        .inventory;
    let diff = graphos_factory_core::inventory::diff_inventories(&before, &after);
    let changed = diff["changed"].as_array().unwrap();
    assert_eq!(
        changed.len(),
        1,
        "{}",
        graphos_factory_core::json::pretty(&diff)
    );
    let fields: Vec<&str> = changed[0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert_eq!(
        fields,
        vec!["pagination", "parameters", "response"],
        "{:?}",
        fields
    );
}

/// The pagerduty pair: the same operations, and an API summary that moved
/// from the pooled cursor/cursor to the per-operation majority. No operation
/// changed, so `changed` is empty and the move is its own `api` entry.
fn api_pagination_pair() -> (Value, Value) {
    let mut before = build(&spec());
    before["api"]["pagination"] = json!({"style": "cursor", "request": "cursor", "size_param": "limit", "response": "page_info.next_cursor"});
    let mut after = before.clone();
    after["api"]["pagination"] = json!({"style": "offset", "request": "offset", "size_param": "limit", "response": null,
        "counts": {"cursor": 2, "offset": 16, "unknown": 2}});
    (before, after)
}

#[test]
fn diff_reports_an_api_pagination_change_on_its_own() {
    let (before, after) = api_pagination_pair();
    let diff = diff_inventories(&before, &after);
    assert_eq!(diff["added"], json!([]));
    assert_eq!(diff["removed"], json!([]));
    assert_eq!(diff["changed"], json!([]));
    assert_eq!(
        diff["api"],
        json!([{"field": "pagination", "fields": ["counts", "request", "response", "style"],
            "before": before["api"]["pagination"], "after": after["api"]["pagination"]}])
    );
    assert_eq!(diff_inventories(&before, &before)["api"], json!([]));
}

#[test]
fn inventory_diff_prints_the_api_pagination_line() {
    let (before, after) = api_pagination_pair();
    let dir = tempfile::tempdir().unwrap();
    let (b, a) = (
        dir.path().join("before.json"),
        dir.path().join("after.json"),
    );
    std::fs::write(&b, before.to_string()).unwrap();
    std::fs::write(&a, after.to_string()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "inventory",
            "diff",
            b.to_str().unwrap(),
            a.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("api.pagination: cursor -> offset (counts, request, response, style)"),
        "{}",
        stdout
    );
}

/// Run `inventory list --inventory <file>` with `extra` arguments.
fn list_inventory(file: &std::path::Path, extra: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["inventory", "list", "--inventory", file.to_str().unwrap()])
        .args(extra)
        .output()
        .unwrap()
}

fn written_inventory() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("inventory.json");
    std::fs::write(&file, build(&spec()).to_string()).unwrap();
    (dir, file)
}

#[test]
fn inventory_list_json_page_names_its_limit_and_next_offset() {
    // ADR 0091: a loop over `inventory list --json` that reads only
    // `operations` skipped every operation past the first 50. The page now
    // says how big it is and where the next one starts; following
    // `next_offset` until it is null visits every operation exactly once.
    let (_dir, file) = written_inventory();
    let mut offset = 0u64;
    let mut seen: Vec<String> = Vec::new();
    let mut pages = Vec::new();
    loop {
        let out = list_inventory(
            &file,
            &["--json", "--limit", "2", "--offset", &offset.to_string()],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let page: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(page["total"], 5, "{}", page);
        assert_eq!(page["offset"], offset, "{}", page);
        assert_eq!(page["limit"], 2, "{}", page);
        for op in page["operations"].as_array().unwrap() {
            seen.push(op["key"].as_str().unwrap().to_string());
        }
        pages.push(page["next_offset"].clone());
        match page["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => {
                assert!(page["next_offset"].is_null(), "{}", page);
                break;
            }
        }
        assert!(
            pages.len() < 10,
            "next_offset never reached null: {:?}",
            pages
        );
    }
    assert_eq!(pages, vec![json!(2), json!(4), Value::Null]);
    assert_eq!(seen.len(), 5, "{:?}", seen);
    assert_eq!(seen.iter().collect::<HashSet<_>>().len(), 5, "{:?}", seen);

    // One page holding everything: next_offset is null, and stderr is quiet.
    let out = list_inventory(&file, &["--json"]);
    let page: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(page["limit"], 50);
    assert!(page["next_offset"].is_null(), "{}", page);
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn inventory_list_json_says_on_stderr_when_operations_remain() {
    // A reader that pipes stdout into jq still sees, on the terminal, that
    // the page is not the whole inventory.
    let (_dir, file) = written_inventory();
    let out = list_inventory(&file, &["--json", "--limit", "2"]);
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("inventory list: operations 1-2 of 5; next page: --offset 2 --limit 2"),
        "{}",
        stderr
    );
    // The text page keeps its footer.
    let out = list_inventory(&file, &["--limit", "2"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\n1-2 of 5 operations"), "{}", stdout);
    assert!(
        stdout.contains("next page: --offset 2 --limit 2\n"),
        "{}",
        stdout
    );
}

#[test]
fn inventory_list_refuses_a_limit_or_offset_it_cannot_page_with() {
    // `--limit 0` would print a next_offset equal to the offset, so a loop
    // following it never ends; an unparsable value used to fall back to
    // the default silently.
    let (_dir, file) = written_inventory();
    for bad in [
        vec!["--limit", "0"],
        vec!["--limit", "ten"],
        vec!["--limit", "-1"],
        vec!["--offset", "x"],
        vec!["--offset", ""],
    ] {
        let mut args = bad.clone();
        args.push("--json");
        let out = list_inventory(&file, &args);
        assert_eq!(out.status.code(), Some(1), "{:?}", bad);
        assert!(out.stdout.is_empty(), "{:?}", bad);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.contains(&format!("bad {}", bad[0])),
            "{:?}: {}",
            bad,
            stderr
        );
    }
}

#[test]
fn inventory_list_refuses_a_bare_limit_or_offset() {
    // A bare flag is present with no value; it used to page from the
    // default, so `--offset $next` with `$next` empty returned page 1
    // forever. Before the flag, after it, and last on the line.
    let (_dir, file) = written_inventory();
    for (flag, argv) in [
        ("--offset", vec!["--json", "--offset"]),
        ("--offset", vec!["--offset", "--json"]),
        ("--limit", vec!["--limit", "--json"]),
        ("--limit", vec!["--json", "--limit"]),
        ("--offset", vec!["--offset"]),
    ] {
        let out = list_inventory(&file, &argv);
        assert_eq!(out.status.code(), Some(1), "{:?}", argv);
        assert!(out.stdout.is_empty(), "{:?}", argv);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(
            stderr.contains(&format!("bad {}", flag)),
            "{:?}: {}",
            argv,
            stderr
        );
    }
}

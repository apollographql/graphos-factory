//! The inference heuristics against a pilot spec — a real vendor document,
//! so a change to a heuristic is a diff against Gitea rather than a
//! synthetic fixture (ADR 0015). The PagerDuty corpus case lives
//! with that pilot, in the suite of the target that owns it.
use graphos_factory_core::openapi::{build_inventory, read_hint};
use serde_json::{json, Value};
use std::path::Path;

/// The envelope the tool proposes for an operation, from the response facts
/// the inventory records. The judgement itself lives in selection.yaml.
fn suggest(op: &Value) -> Option<String> {
    graphos_factory_core::envelope::suggest_envelope(op.get("response"))
}

fn pilot_spec(pilot: &str, file: &str) -> Option<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../pilots/graphos")
        .join(pilot)
        .join(file);
    // The pilots live in this repository: a missing spec is a broken
    // checkout, never a legitimate skip.
    assert!(
        path.exists(),
        "{} is not checked out next to the crate",
        path.display()
    );
    let text = std::fs::read_to_string(&path).unwrap();
    // The same reader `inventory build` uses: Swagger 2.0 is converted first.
    let spec = graphos_factory_core::spec::read(&text, path.to_str().unwrap())
        .unwrap()
        .document;
    Some(build_inventory(&spec).unwrap().inventory)
}

fn op<'a>(inv: &'a Value, key: &str) -> &'a Value {
    inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == key)
        .unwrap_or_else(|| panic!("no operation {}", key))
}

#[test]
fn read_hints_name_the_verb_and_never_change_the_semantics() {
    let hint = |id: &str, path: &str| read_hint(id, path);
    assert!(hint("renderMarkdown", "/markdown")
        .unwrap()
        .contains("carries `render`"));
    assert!(hint(
        "repoGetFileContentsPost",
        "/repos/{owner}/{repo}/file-contents"
    )
    .unwrap()
    .contains("carries `get`"));
    assert!(hint("x", "/schedules/preview")
        .unwrap()
        .contains("path segment `preview`"));
    assert!(hint("search_widgets", "/widgets/search").is_some());
    assert!(hint("issueCreateIssue", "/repos/{owner}/{repo}/issues").is_none());
    // An action verb that is also a noun for a reading stays out of the list.
    assert!(hint("repoApplyDiffPatch", "/repos/{owner}/{repo}/diffpatch").is_none());
    assert!(hint("exportReport", "/reports/export").is_none());
    // Word boundaries: `budget` is not `get`, `blacklist` is not `list`.
    assert!(hint("setBudget", "/budget").is_none());
    assert!(hint("addToBlacklist", "/blacklist").is_none());
    // A trailing path parameter is skipped for the segment check.
    assert!(hint("x", "/things/search/{id}").is_some());
    // An action verb in the operationId vetoes its read-word noun; the path
    // may still speak.
    assert!(hint("createSavedSearch", "/saved_searches").is_none());
    assert!(hint("addToList", "/lists/{id}/items").is_none());
    assert!(hint("uploadPreviewImage", "/images").is_none());
    assert!(hint("importList", "/imports").is_none());
    assert!(hint("createSchedulePreview", "/schedules/preview").is_some());
    // A Google-style custom method.
    assert!(hint("", "/v1/things:search").is_some());
}

#[test]
fn a_synthesized_operation_id_never_hints_a_read() {
    let spec = serde_json::json!({
        "openapi": "3.0.3", "info": {"title": "x", "version": "1"},
        "servers": [{"url": "https://api.example.test"}],
        "paths": {
            "/accounts/{list_id}/members": {"post": {
                "parameters": [{"name": "list_id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"201": {"description": "created", "content": {"application/json": {"schema": {"type": "object", "properties": {"id": {"type": "string"}}}}}}}
            }},
            "/things/{filter_id}/items": {"post": {
                "parameters": [{"name": "filter_id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"201": {"description": "created"}}
            }},
            "/widgets/search": {"post": {"responses": {"200": {"description": "ok"}}}}
        }
    });
    let inv = build_inventory(&spec).unwrap().inventory;
    for key in [
        "post:/accounts/{list_id}/members",
        "post:/things/{filter_id}/items",
        "post:/widgets/search",
    ] {
        assert_eq!(op(&inv, key)["semantics"], "write", "{}", key);
    }
    assert!(op(&inv, "post:/accounts/{list_id}/members")
        .get("read_hint")
        .is_none());
    assert!(op(&inv, "post:/things/{filter_id}/items")
        .get("read_hint")
        .is_none());
    assert!(op(&inv, "post:/widgets/search")["read_hint"].is_string());
}

/// The next-cursor key is reported only when it is a string; a HAL-style
/// `next: { href }` link is not a cursor.
#[test]
fn a_next_cursor_key_must_be_a_string_property() {
    let spec = serde_json::json!({
        "openapi": "3.0.3", "info": {"title": "x", "version": "1"},
        "servers": [{"url": "https://api.example.test"}],
        "paths": {
            "/cursored": {"get": {
                "parameters": [{"name": "cursor", "in": "query", "schema": {"type": "string"}}, {"name": "limit", "in": "query", "schema": {"type": "integer"}}],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {
                    "items": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}},
                    "next_cursor": {"type": ["string", "null"]}}}}}}}
            }},
            "/linked": {"get": {
                "parameters": [{"name": "cursor", "in": "query", "schema": {"type": "string"}}],
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"type": "object", "properties": {
                    "items": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}},
                    "next": {"type": "object", "properties": {"href": {"type": "string"}}}}}}}}}
            }},
            "/paged": {"get": {
                "parameters": [{"name": "page", "in": "query", "schema": {"type": "integer"}}],
                "responses": {"200": {"description": "ok"}}
            }}
        }
    });
    let inv = build_inventory(&spec).unwrap().inventory;
    let cursored = &op(&inv, "get:/cursored")["pagination"];
    assert_eq!(cursored["style"], "cursor");
    assert_eq!(cursored["request"], "cursor");
    assert_eq!(cursored["size_param"], "limit");
    assert_eq!(cursored["response"], "next_cursor");
    let linked = &op(&inv, "get:/linked")["pagination"];
    assert_eq!(linked["style"], "cursor");
    assert_eq!(
        linked["response"],
        Value::Null,
        "an object is a link, not a cursor"
    );
    let paged = &op(&inv, "get:/paged")["pagination"];
    assert_eq!(paged["style"], "page");
    assert_eq!(paged["size_param"], Value::Null);
    // The API summary takes the first string cursor any operation carries.
    assert_eq!(inv["api"]["pagination"]["response"], "next_cursor");
}

#[test]
fn gitea_corpus_semantics_envelopes_and_pagination() {
    let inv = match pilot_spec("gitea", "swagger.json") {
        Some(i) => i,
        None => return,
    };
    // Every POST is a write; the markdown renderers and the file-contents
    // lookup carry a read hint, and no other POST does — applying a diff
    // patch is a write the verb list must not mistake for a reading.
    let hinted: Vec<&str> = inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["method"] == "POST" && o.get("read_hint").is_some())
        .map(|o| o["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        hinted,
        vec![
            "post:/markdown",
            "post:/markdown/raw",
            "post:/markup",
            "post:/repos/{owner}/{repo}/file-contents",
        ]
    );
    assert!(inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["method"] == "POST")
        .all(|o| o["semantics"] == "write"));
    assert!(op(&inv, "post:/markdown")["read_hint"]
        .as_str()
        .unwrap()
        .contains("carries `render`"));
    assert!(op(&inv, "post:/repos/{owner}/{repo}/diffpatch")
        .get("read_hint")
        .is_none());
    assert_eq!(
        op(&inv, "delete:/repos/{owner}/{repo}")["semantics"],
        "write"
    );
    assert_eq!(op(&inv, "get:/version")["semantics"], "read");

    // Envelopes. The inventory records only facts (ADR 0018); the envelope
    // itself is the suggestion `selection draft` writes into selection.yaml
    // for the agent to confirm.
    let search = op(&inv, "get:/repos/search");
    assert_eq!(search["response"]["root_property_count"], 2);
    assert_eq!(search["response"]["array_root_properties"], json!(["data"]));
    assert!(search["response"].get("envelope").is_none());
    assert!(search["response"].get("list").is_none());
    // `{ ok, data[] }` — one array beside one status flag: a wrapper.
    assert_eq!(suggest(search), Some("data".to_string()));
    assert!(graphos_factory_core::envelope::is_list(
        search.get("response"),
        Some("data")
    ));
    // A bare array response has nothing to unwrap.
    let issues = op(&inv, "get:/repos/{owner}/{repo}/issues");
    assert_eq!(issues["response"]["root_is_array"], true);
    assert_eq!(suggest(issues), None);
    // One key, and it is not an array.
    let version = op(&inv, "get:/version");
    assert_eq!(version["response"]["sole_root_property"], "version");
    assert_eq!(suggest(version), Some("version".to_string()));
    assert!(!graphos_factory_core::envelope::is_list(
        version.get("response"),
        Some("version")
    ));
    // A resource that happens to embed a list is not a wrapper: a release
    // has sixteen root properties and one of them is `assets`. The detector
    // this replaced took that array as the envelope, which is the Mailchimp
    // failure (a 22-property audience reported `envelope: "modules"`).
    let release = op(&inv, "get:/repos/{owner}/{repo}/releases/{id}");
    assert_eq!(release["response"]["root_property_count"], 16);
    assert_eq!(
        release["response"]["array_root_properties"],
        json!(["assets"])
    );
    assert_eq!(suggest(release), None);

    // Pagination per operation: page/limit where the operation declares
    // them, no block where it does not; the API summary agrees here.
    assert_eq!(search["pagination"]["style"], "page");
    assert_eq!(search["pagination"]["request"], "page");
    assert_eq!(search["pagination"]["size_param"], "limit");
    assert_eq!(issues["pagination"]["style"], "page");
    assert!(
        op(&inv, "get:/version").get("pagination").is_none(),
        "an operation with no paging parameter carries no block"
    );
    assert_eq!(inv["api"]["pagination"]["style"], "page");
}

/// An `anyOf` nullable cursor (PagerDuty's idiom) and an untyped one still
/// count; an object does not.
#[test]
fn a_next_cursor_may_be_a_nullable_branch_union_or_untyped() {
    let page = |next: serde_json::Value| -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {
            "items": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}},
            "next_cursor": next}})
    };
    let get = |schema: serde_json::Value| -> serde_json::Value {
        serde_json::json!({"get": {
            "parameters": [{"name": "cursor", "in": "query", "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": schema}}}}}})
    };
    let spec = serde_json::json!({
        "openapi": "3.0.3", "info": {"title": "x", "version": "1"},
        "servers": [{"url": "https://api.example.test"}],
        "paths": {
            "/anyofnull": get(page(serde_json::json!({"anyOf": [{"type": "string"}, {"type": "null"}]}))),
            "/untyped": get(page(serde_json::json!({"description": "the cursor"}))),
            "/object": get(page(serde_json::json!({"type": "object", "properties": {"page": {"type": "integer"}}})))
        }
    });
    let inv = build_inventory(&spec).unwrap().inventory;
    assert_eq!(
        op(&inv, "get:/anyofnull")["pagination"]["response"],
        "next_cursor"
    );
    assert_eq!(
        op(&inv, "get:/untyped")["pagination"]["response"],
        "next_cursor"
    );
    assert_eq!(
        op(&inv, "get:/object")["pagination"]["response"],
        Value::Null
    );
}

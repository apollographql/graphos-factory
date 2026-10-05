//! `graphos-factory-core validate`: the conformance statuses and waivers.
use graphos_factory_core::cmd::validate::{summary_line, validate_workspace};
use graphos_factory_core::waivers::{normalize_where, read_waivers, Waiver};
use serde_json::{json, Value};
use std::path::Path;

fn inventory() -> Value {
    json!({"contract_version": 1, "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test/v2"]},
        "operations": [
            {"key": "get:/widgets", "operation_id": "listWidgets", "method": "GET", "path": "/widgets", "semantics": "read", "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null,
             "response": {"status": "200", "envelope": "widgets", "shape_ref": "#/shapes/WidgetList", "list": true}, "errors": [], "support": "supported", "support_reason": null},
            {"key": "post:/widgets", "operation_id": "createWidget", "method": "POST", "path": "/widgets", "semantics": "unknown", "provenance": "spec", "confidence": 1, "parameters": [],
             "request_body": {"shape_ref": "#/shapes/Widget", "content_type": "application/json"},
             "response": {"status": "201", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false}, "errors": [], "support": "supported", "support_reason": null}
        ],
        "shapes": {"Widget": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}},
                   "WidgetList": {"type": "object", "properties": {"widgets": {"type": "array", "items": {"$ref": "#/shapes/Widget"}}}}},
        "unresolved": []})
}

fn write(dir: &Path, rel: &str, text: &str) {
    let f = dir.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, text).unwrap();
}

/// A workspace with one body of every status: a conformant fixture, a
/// fixture whose path no operation has (unmatched), a 404 fixture for a
/// status the spec does not document (unchecked), a unit entry whose URL
/// matches nothing (unmatched) and one whose apiResponseBody is not JSON
/// (unmatched, but it names its operation).
fn workspace(selection: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(
        d,
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inventory()),
    );
    write(d, ".factory/selection.yaml", selection);
    write(
        d,
        "template.yaml",
        "variables:\n  - name: BASE_URL\n    test_default: \"https://api.widgets.test/v2\"\n",
    );
    write(d, "tests/fixtures/mappings/list_widgets.json",
        &json!({"request": {"method": "GET", "urlPath": "/widgets"}, "response": {"status": 200, "jsonBody": {"widgets": [{"id": "W1", "name": "a"}]}}}).to_string());
    write(d, "tests/fixtures/mappings/list_gadgets.json",
        &json!({"request": {"method": "GET", "urlPath": "/gadgets"}, "response": {"status": 200, "jsonBody": {"gadgets": []}}}).to_string());
    write(d, "tests/fixtures/mappings/widgets_not_found.json",
        &json!({"request": {"method": "GET", "urlPath": "/widgets"}, "response": {"status": 404, "jsonBody": {"message": "not found"}}}).to_string());
    write(d, "tests/widget-co.connector.yaml", concat!(
        "tests:\n",
        "  - name: \"list widgets\"\n    target: \"Query.widget_co_listWidgets\"\n    apiResponseBody: |\n      {\"widgets\": []}\n",
        "    expect:\n      connectorRequest:\n        method: GET\n        url: https://api.widgets.test/v2/widgets\n",
        "  - name: \"list gadgets\"\n    target: \"Query.widget_co_listGadgets\"\n    apiResponseBody: |\n      {\"gadgets\": []}\n",
        "    expect:\n      connectorRequest:\n        method: GET\n        url: https://api.widgets.test/v2/gadgets\n",
        "  - name: \"broken body\"\n    target: \"Mutation.widget_co_createWidget\"\n    apiResponseBody: |\n      not json\n",
        "    expect:\n      connectorRequest:\n        method: POST\n        url: https://api.widgets.test/v2/widgets\n",
    ));
    dir
}

const SELECTION: &str = "contract_version: 1\noperations:\n  \"get:/widgets\": { include: true, graphql: { root: query, name: listWidgets } }\n  \"post:/widgets\": { include: true, graphql: { root: mutation, name: createWidget } }\n";

fn statuses(report: &Value) -> Vec<(String, String)> {
    report["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["where"].as_str().unwrap().to_string(),
                r["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn every_cause_gets_its_own_status_and_only_unmatched_fails() {
    let ws = workspace(SELECTION);
    let report = validate_workspace(ws.path()).unwrap();
    let s = statuses(&report.json);
    assert!(s.contains(&(
        "tests/fixtures/mappings/list_widgets.json".into(),
        "pass".into()
    )));
    assert!(
        s.contains(&(
            "tests/fixtures/mappings/list_gadgets.json".into(),
            "unmatched".into()
        )),
        "{:?}",
        s
    );
    assert!(
        s.contains(&(
            "tests/fixtures/mappings/widgets_not_found.json".into(),
            "unchecked".into()
        )),
        "{:?}",
        s
    );
    assert!(
        s.contains(&(
            "tests/widget-co.connector.yaml › list widgets".into(),
            "pass".into()
        )),
        "{:?}",
        s
    );
    assert!(
        s.contains(&(
            "tests/widget-co.connector.yaml › list gadgets".into(),
            "unmatched".into()
        )),
        "{:?}",
        s
    );
    assert!(
        s.contains(&(
            "tests/widget-co.connector.yaml › broken body".into(),
            "unmatched".into()
        )),
        "{:?}",
        s
    );
    assert_eq!(
        report.json["counts"],
        json!({"fail": 0, "pass": 2, "unchecked": 1, "unmatched": 3, "waived": 0})
    );
    assert!(report.failing, "unmatched fails the layer");
    // Per operation: the unreadable body names its operation and is that
    // operation's failure; the 404 with no documented shape does not lower
    // get:/widgets below its pass.
    assert_eq!(report.json["operations"]["post:/widgets"], "fail");
    assert_eq!(report.json["operations"]["get:/widgets"], "pass");
    assert_eq!(
        summary_line(&report.json),
        "2 bodies conform, 0 do not, 1 unchecked, 3 unmatched"
    );
}

#[test]
fn unchecked_alone_does_not_fail_and_reads_as_before() {
    let ws = workspace(SELECTION);
    std::fs::remove_file(ws.path().join("tests/fixtures/mappings/list_gadgets.json")).unwrap();
    std::fs::write(
        ws.path().join("tests/widget-co.connector.yaml"),
        "tests: []\n",
    )
    .unwrap();
    let report = validate_workspace(ws.path()).unwrap();
    assert!(!report.failing);
    assert_eq!(
        summary_line(&report.json),
        "1 bodies conform, 0 do not, 1 unchecked"
    );
}

#[test]
fn waivers_turn_accepted_gaps_into_waived_and_report_the_unused_ones() {
    let selection = format!(
        "{}waivers:\n  - where: \"tests/fixtures/mappings/widgets_not_found.json\"\n    status: unchecked\n    reason: \"no 404 body is documented\"\n    decision: D-0001\n  - where: \"tests/widget-co.connector.yaml#list gadgets\"\n    status: unmatched\n    reason: \"gadgets are undocumented\"\n    decision: D-0002\n  - where: \"tests/fixtures/mappings/widgets_not_found.json\"\n    status: unmatched\n    reason: \"the wrong status: never matches\"\n  - operation: \"post:/widgets\"\n    status: unmatched\n    reason: \"the broken body, by operation\"\n    decision: D-0003\n",
        SELECTION
    );
    let ws = workspace(&selection);
    let report = validate_workspace(ws.path()).unwrap();
    let s = statuses(&report.json);
    assert!(
        s.contains(&(
            "tests/fixtures/mappings/widgets_not_found.json".into(),
            "waived".into()
        )),
        "{:?}",
        s
    );
    assert!(
        s.contains(&(
            "tests/widget-co.connector.yaml › list gadgets".into(),
            "waived".into()
        )),
        "the # spelling names the › body: {:?}",
        s
    );
    assert!(
        s.contains(&(
            "tests/widget-co.connector.yaml › broken body".into(),
            "waived".into()
        )),
        "{:?}",
        s
    );
    assert!(
        s.contains(&(
            "tests/fixtures/mappings/list_gadgets.json".into(),
            "unmatched".into()
        )),
        "not waived: {:?}",
        s
    );
    assert_eq!(
        report.json["counts"],
        json!({"fail": 0, "pass": 2, "unchecked": 0, "unmatched": 1, "waived": 3})
    );
    assert!(report.failing, "the unwaived unmatched fixture still fails");
    let waived = report.json["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["status"] == "waived"
                && r["where"] == "tests/fixtures/mappings/widgets_not_found.json"
        })
        .unwrap();
    assert_eq!(waived["waiver"]["decision"], "D-0001");
    assert_eq!(
        waived["reason"], "the spec documents no body shape for get:/widgets → 404",
        "the original cause stays on the result"
    );
    let unused: Vec<&str> = report.json["waivers"]["unused"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["target"].as_str().unwrap())
        .collect();
    assert_eq!(
        unused,
        vec!["tests/fixtures/mappings/widgets_not_found.json"],
        "the wrong-status waiver matched nothing"
    );
    assert_eq!(
        report.json["waivers"]["applied"].as_array().unwrap().len(),
        3
    );
    // Per operation, waived ranks below pass and above unchecked.
    assert_eq!(report.json["operations"]["post:/widgets"], "waived");
    assert_eq!(report.json["operations"]["get:/widgets"], "pass");
    assert_eq!(
        summary_line(&report.json),
        "2 bodies conform, 0 do not, 0 unchecked, 1 unmatched, 3 waived"
    );
}

#[test]
fn a_missing_inventory_is_an_error_not_a_pass() {
    let dir = tempfile::tempdir().unwrap();
    assert!(validate_workspace(dir.path())
        .unwrap_err()
        .contains("inventory build"));
}

#[test]
fn waiver_matching_and_where_normalisation() {
    assert_eq!(
        normalize_where("./tests/x.connector.yaml › list all"),
        "tests/x.connector.yaml#list all"
    );
    assert_eq!(
        normalize_where("tests/x.connector.yaml#list all"),
        "tests/x.connector.yaml#list all"
    );
    assert_eq!(
        normalize_where("tests/fixtures/mappings/a.json"),
        "tests/fixtures/mappings/a.json"
    );
    let w = Waiver {
        where_: Some("tests/x.connector.yaml#list all".into()),
        operation: None,
        status: "unchecked".into(),
        reason: None,
        decision: None,
        context: None,
        until: None,
        expires: None,
    };
    assert!(w.matches(
        "tests/x.connector.yaml › list all",
        Some("get:/x"),
        "unchecked"
    ));
    assert!(
        !w.matches(
            "tests/x.connector.yaml › list all",
            Some("get:/x"),
            "unmatched"
        ),
        "status is part of the match"
    );
    assert!(!w.matches(
        "tests/x.connector.yaml › list some",
        Some("get:/x"),
        "unchecked"
    ));
    let by_op = Waiver {
        where_: None,
        operation: Some("get:/x".into()),
        status: "unchecked".into(),
        reason: None,
        decision: None,
        context: None,
        until: None,
        expires: None,
    };
    assert!(by_op.matches("anything", Some("get:/x"), "unchecked"));
    assert!(
        !by_op.matches("anything", None, "unchecked"),
        "an unmatched body has no operation to match by"
    );
    let parsed = read_waivers(&graphos_factory_core::yaml::parse("waivers:\n  - operation: \"get:/x\"\n    status: unchecked\n    reason: r\n    decision: D-0001\n    expires: \"2026-01-01\"\n").unwrap());
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].decision.as_deref(), Some("D-0001"));
    assert!(graphos_factory_core::waivers::expired(
        &parsed[0],
        "2026-09-10"
    ));
    assert!(!graphos_factory_core::waivers::expired(
        &parsed[0],
        "2025-12-31"
    ));
}

/// A recorded request whose operation the inventory keys by a literal query
/// string (Salesforce SOQL lists, ADR 0054) resolves to that operation, in
/// both stub spellings: an inline `url` query and `urlPath` +
/// `queryParameters.equalTo`, and in a unit entry's URL. A query no
/// operation is keyed by still resolves by the path alone.
#[test]
fn a_request_resolves_to_the_operation_keyed_by_its_literal_query() {
    let op = |key: &str, path: &str, shape: &str| {
        json!({"key": key, "operation_id": key, "method": "GET", "path": path, "semantics": "read", "provenance": "spec", "confidence": 1,
               "parameters": [], "request_body": null,
               "response": {"status": "200", "envelope": null, "shape_ref": format!("#/shapes/{}", shape), "list": false},
               "errors": [], "support": "supported", "support_reason": null})
    };
    let inv = json!({"contract_version": 1, "api": {"title": "SF", "base_urls": ["https://sf.test"]},
        "operations": [
            op("get:/query?q=SELECT+Id+FROM+Account", "/query?q=SELECT+Id+FROM+Account", "AccountResult"),
            op("get:/query?q=SELECT+Id+FROM+Contact", "/query?q=SELECT+Id+FROM+Contact", "ContactResult"),
            op("get:/version", "/version", "Version")
        ],
        "shapes": {
            "AccountResult": {"type": "object", "properties": {"records": {"type": "array", "items": {"type": "object", "properties": {"Id": {"type": "string"}}}}}},
            "ContactResult": {"type": "object", "properties": {"records": {"type": "array", "items": {"type": "object", "properties": {"Id": {"type": "string"}}}}}},
            "Version": {"type": "object", "properties": {"version": {"type": "string"}}}
        },
        "unresolved": []});
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(
        d,
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inv),
    );
    write(
        d,
        ".factory/selection.yaml",
        "contract_version: 1\noperations: {}\n",
    );
    write(
        d,
        "template.yaml",
        "variables:\n  - name: BASE_URL\n    test_default: \"https://sf.test\"\n",
    );
    write(d, "tests/fixtures/mappings/accounts.json",
        &json!({"request": {"method": "GET", "urlPath": "/query", "queryParameters": {"q": {"equalTo": "SELECT Id FROM Account"}}},
                "response": {"status": 200, "jsonBody": {"records": [{"Id": "001"}]}}}).to_string());
    write(
        d,
        "tests/fixtures/mappings/contacts.json",
        &json!({"request": {"method": "GET", "url": "/query?q=SELECT+Id+FROM+Contact"},
                "response": {"status": 200, "jsonBody": {"records": [{"Id": "003"}]}}})
        .to_string(),
    );
    write(
        d,
        "tests/fixtures/mappings/version.json",
        &json!({"request": {"method": "GET", "url": "/version?pretty=1"},
                "response": {"status": 200, "jsonBody": {"version": "67.0"}}})
        .to_string(),
    );
    write(d, "tests/sf.connector.yaml", concat!(
        "tests:\n",
        "  - name: \"accounts\"\n    target: \"Query.sf_accounts\"\n    apiResponseBody: |\n      {\"records\": [{\"Id\": \"001\"}]}\n",
        "    expect:\n      connectorRequest:\n        method: GET\n        url: https://sf.test/query?q=SELECT+Id+FROM+Account\n",
    ));
    let report = validate_workspace(d).unwrap();
    let ops: Vec<(String, String, String)> = report.json["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["where"].as_str().unwrap().to_string(),
                r["status"].as_str().unwrap().to_string(),
                r["operation"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    for (where_, key) in [
        (
            "tests/fixtures/mappings/accounts.json",
            "get:/query?q=SELECT+Id+FROM+Account",
        ),
        (
            "tests/fixtures/mappings/contacts.json",
            "get:/query?q=SELECT+Id+FROM+Contact",
        ),
        ("tests/fixtures/mappings/version.json", "get:/version"),
        (
            "tests/sf.connector.yaml › accounts",
            "get:/query?q=SELECT+Id+FROM+Account",
        ),
    ] {
        assert!(
            ops.iter()
                .any(|(w, s, o)| w == where_ && s == "pass" && o == key),
            "{} should pass against {}: {:?}",
            where_,
            key,
            ops
        );
    }
    assert!(ops.iter().all(|(_, s, _)| s != "unmatched"), "{:?}", ops);
}

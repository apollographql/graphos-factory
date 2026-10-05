//! `sources refresh`: a newer vendor document replaces the upstream, the
//! recorded patches are replayed over it, and each is reported re-applied,
//! obsolete or in conflict; the working copy, the lock, the decision, the
//! applied lock and the inventory follow.

use graphos_factory_core::cmd;
use graphos_factory_core::jsonschema::validate;
use graphos_factory_core::lint::{lint_workspace, LintOptions};
use graphos_factory_core::refresh::{replay, Fate};
use graphos_factory_core::schemas;
use serde_json::{json, Value};
use std::path::Path;

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key", "@tag"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "widget_co", http: { baseURL: "{{BASE_URL}}" })

type Widget_Co_Widget {
  id: ID
  name: String
}

type Query {
  widget_co_listWidgets(limit: Int): [Widget_Co_Widget]
    @connect(source: "widget_co", http: { GET: "/widgets" }, selection: "$.widgets { id name }")
}
"#;

const SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/widgets\":\n    include: true\n    graphql: { root: query, name: listWidgets }\n";

const WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\n";

const DECISIONS: &str = r#"{"contract_version":1,"decisions":[
{"id":"D-0001","title":"Pilot scope","status":"resolved","date":"2026-09-08","context":"x.","resolution":{"decision":"y."}}
]}"#;

const SOURCES_LOCK: &str = "contract_version: 1\nsources:\n  - kind: docs\n    url: https://docs.widgets.test/reference\n    retrieved_at: 2026-09-08T14:05:40Z\n    used_for: [\"get:/widgets\"]\n    note: >-\n      Read for the pagination convention the spec does not\n      document; nothing else.\n";

/// The vendor's 1.0.0 document.
fn spec() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Widget Co", "version": "1.0.0"},
        "servers": [{"url": "https://api.widgets.test"}],
        "components": {"schemas": {
            "Widget": {"type": "object", "required": ["id", "name"], "properties": {"id": {"type": "string"}, "name": {"type": "string"}}},
            "WidgetList": {"type": "object", "properties": {"widgets": {"type": "array", "items": {"$ref": "#/components/schemas/Widget"}}}}
        }},
        "paths": {"/widgets": {"get": {
            "operationId": "listWidgets",
            "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/WidgetList"}}}}}
        }}}
    })
}

/// The engineer's two corrections: `name` is not required and is nullable.
fn corrected(mut doc: Value) -> Value {
    doc["components"]["schemas"]["Widget"]["required"] = json!(["id"]);
    doc["components"]["schemas"]["Widget"]["properties"]["name"]["nullable"] = json!(true);
    doc
}

fn write(dir: &Path, rel: &str, text: &str) {
    let f = dir.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, text).unwrap();
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

fn decisions_doc(dir: &Path) -> Value {
    graphos_factory_core::json::parse(&read(dir, ".factory/decisions.json")).unwrap()
}

/// The searchable prose of the record with this id: title, context, the
/// resolution's decision and note, and the affects list, all joined — so the
/// `.contains` assertions that scanned the old Markdown block still work.
fn prose(dir: &Path, id: &str) -> String {
    if id.starts_with("F-") {
        return finding_prose(dir, id);
    }
    let doc = decisions_doc(dir);
    let rec = graphos_factory_core::decisions::find(&doc, id)
        .cloned()
        .unwrap_or(Value::Null);
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let res = rec.get("resolution").cloned().unwrap_or(Value::Null);
    let affects = rec
        .get("affects")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    format!(
        "{} {} {} {} {}",
        s(&rec, "title"),
        s(&rec, "context"),
        s(&res, "decision"),
        s(&res, "note"),
        affects
    )
}

/// The same for a finding (`sources refresh` and `pin --force` write one,
/// ADR 0113 §3): title, body, cites and evidence.
fn finding_prose(dir: &Path, id: &str) -> String {
    let doc = findings_doc(dir);
    let rec = graphos_factory_core::findings::find(&doc, id)
        .cloned()
        .unwrap_or(Value::Null);
    let s = |k: &str| {
        rec.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string()
    };
    let evidence = graphos_factory_core::json::strings(rec.get("evidence")).join(" ");
    format!("{} {} {} {}", s("title"), s("body"), s("cites"), evidence)
}

/// The findings as every reader sees them: `findings.json`'s, then the ones
/// added since ADR 0118, one file each under `.factory/findings/`.
fn findings_doc(dir: &Path) -> Value {
    graphos_factory_core::findings::load(dir, None).unwrap()
}

/// The id of the one finding a refresh recorded: random (ADR 0118), so
/// found rather than assumed.
fn the_finding(dir: &Path) -> String {
    let ids = finding_ids(dir);
    assert_eq!(ids.len(), 1, "{:?}", ids);
    assert!(
        graphos_factory_core::record_log::is_random(&ids[0]),
        "{}",
        ids[0]
    );
    ids[0].clone()
}

fn finding_ids(dir: &Path) -> Vec<String> {
    graphos_factory_core::json::get_arr(&findings_doc(dir), "findings")
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("id").and_then(|x| x.as_str()).map(str::to_string))
        .collect()
}

fn all_prose(dir: &Path) -> String {
    decision_ids(dir)
        .iter()
        .chain(finding_ids(dir).iter())
        .map(|id| prose(dir, id))
        .collect::<Vec<_>>()
        .join("\n")
}

fn decision_ids(dir: &Path) -> Vec<String> {
    decisions_doc(dir)
        .get("decisions")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|d| d.get("id").and_then(|x| x.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn has_decision(dir: &Path, id: &str) -> bool {
    if id.starts_with("F-") {
        return finding_ids(dir).iter().any(|i| i == id);
    }
    decision_ids(dir).iter().any(|i| i == id)
}

/// The decision string of the record with this id (to assert it has no newline).
fn decision_str(dir: &Path, id: &str) -> String {
    if id.starts_with("F-") {
        let doc = findings_doc(dir);
        return graphos_factory_core::findings::find(&doc, id)
            .and_then(|r| r.get("body"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
    }
    let doc = decisions_doc(dir);
    graphos_factory_core::decisions::find(&doc, id)
        .and_then(|r| r.get("resolution"))
        .and_then(|r| r.get("decision"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn inventory(spec: &Value) -> Value {
    graphos_factory_core::openapi::build_inventory(spec)
        .unwrap()
        .inventory
}

/// A locked, pinned workspace whose working copy carries the two
/// corrections, codified as two patches citing the recorded decision
/// D-0001 (codify writes no decision of its own, ADR 0113 §3).
fn workspace() -> tempfile::TempDir {
    workspace_at("openapi.json", &|v| graphos_factory_core::json::pretty(v))
}

/// The same workspace with the working copy at `rel`, serialised by `text`.
fn workspace_at(rel: &str, text: &dyn Fn(&Value) -> String) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(d, "widget-co.graphql", SDL);
    write(d, ".factory/workspace.yaml", WORKSPACE);
    write(d, ".factory/selection.yaml", SELECTION);
    write(d, ".factory/decisions.json", DECISIONS);
    write(d, ".factory/sources.lock.yaml", SOURCES_LOCK);
    write(d, rel, &text(&spec()));
    write(
        d,
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inventory(&spec())),
    );
    let spans =
        graphos_factory_core::spans::spans(SDL, Some(&inventory(&spec())), &Default::default());
    let lock = graphos_factory_core::spans::lock_document("widget-co.graphql", &spans);
    write(
        d,
        ".factory/applied.lock.yaml",
        &graphos_factory_core::yaml::stringify(&lock, 0),
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            rel,
            "--url",
            "https://api.widgets.test/openapi.json",
        ])),
        0
    );
    write(d, rel, &text(&corrected(spec())));
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            rel,
            "--reason",
            "live API returns null name for archived widgets",
            "--decision",
            "D-0001",
        ])),
        0
    );
    assert_eq!(lock_check(d), 0);
    dir
}

fn lock_check(d: &Path) -> i32 {
    cmd::lock::main(&args(&[d.to_str().unwrap(), "--check"]))
}

fn source_rules(d: &Path) -> Vec<String> {
    lint_workspace(
        d,
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings
    .into_iter()
    .map(|f| f.rule)
    .filter(|r| r.starts_with("source-") || r == "unacknowledged-source-edit")
    .collect()
}

fn sources_lock(d: &Path) -> Value {
    graphos_factory_core::yaml::parse(&read(d, ".factory/sources.lock.yaml")).unwrap()
}

fn entry(d: &Path) -> Value {
    entry_at(d, "openapi.json")
}

fn entry_at(d: &Path, rel: &str) -> Value {
    sources_lock(d)["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["path"] == rel)
        .cloned()
        .unwrap_or_else(|| panic!("an entry for {}", rel))
}

fn refresh(d: &Path, from: &Path, extra: &[&str]) -> i32 {
    refresh_at(d, "openapi.json", from, extra)
}

/// `--reason` is required; a later `--reason` in `extra` wins.
fn refresh_at(d: &Path, rel: &str, from: &Path, extra: &[&str]) -> i32 {
    let mut a = vec![
        "refresh",
        d.to_str().unwrap(),
        "--path",
        rel,
        "--from",
        from.to_str().unwrap(),
        "--reason",
        "test refresh",
    ];
    a.extend_from_slice(extra);
    cmd::sources::main(&args(&a))
}

/// The binary itself, for the printed `--json` summary.
fn refresh_json(d: &Path, from: &Path, extra: &[&str]) -> (i32, Value) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "sources",
            "refresh",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--from",
            from.to_str().unwrap(),
            "--reason",
            "test refresh",
            "--json",
        ])
        .args(extra)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let json = graphos_factory_core::json::parse(&text).unwrap_or_else(|e| {
        panic!(
            "{}: {:?}\n{}",
            e,
            text,
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap(), json)
}

fn sha(bytes: &[u8]) -> String {
    graphos_factory_core::patch::bytes_sha256(bytes)
}

fn snapshot(d: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for rel in [
        "openapi.json",
        ".factory/sources/openapi.upstream.json",
        ".factory/sources.lock.yaml",
        ".factory/decisions.json",
        ".factory/applied.lock.yaml",
        ".factory/inventory.json",
    ] {
        out.push((rel.to_string(), std::fs::read(d.join(rel)).unwrap()));
    }
    out
}

#[test]
fn refresh_keeps_valid_writes_when_provenance_fails() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/context.yaml",
        &format!(
            "evidence:\n  - path: {}\n    sha256: recorded\n",
            d.join("widget-co.graphql").display()
        ),
    );

    let mut vendor = spec();
    vendor["info"]["version"] = json!("1.1.0");
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi.json");
    std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();

    let (code, summary) = refresh_json(d, &from, &[]);
    assert_eq!(code, 0);
    assert_eq!(summary["acknowledged_in_applied_lock"], true);
    assert_eq!(summary["provenance_recorded"], false);
    let applied =
        graphos_factory_core::yaml::parse(&read(d, ".factory/applied.lock.yaml")).unwrap();
    assert!(
        applied.get("provenance").is_none(),
        "refresh must remove stale provenance"
    );
    assert_eq!(lock_check(d), 0);
}

#[test]
fn a_refresh_with_one_obsolete_and_one_still_valid_patch_reports_exactly_that() {
    let ws = workspace();
    let d = ws.path();
    let mut applied =
        graphos_factory_core::yaml::parse(&read(d, ".factory/applied.lock.yaml")).unwrap();
    graphos_factory_core::provenance::refresh(d, &mut applied, None, None).unwrap();
    write(
        d,
        ".factory/applied.lock.yaml",
        &graphos_factory_core::yaml::stringify(&applied, 0),
    );
    let before = entry(d);
    assert_eq!(before["patches"].as_array().unwrap().len(), 2);
    let old_sha = before["upstream_sha256"].as_str().unwrap().to_string();

    // Vendor 1.1.0: adopted the `required` correction, still says `name`
    // is non-nullable, and added an operation.
    let mut vendor = spec();
    vendor["info"]["version"] = json!("1.1.0");
    vendor["components"]["schemas"]["Widget"]["required"] = json!(["id"]);
    vendor["paths"]["/gadgets"] = json!({"get": {
        "operationId": "listGadgets",
        "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/WidgetList"}}}}}
    }});
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi-1.1.0.json");
    let vendor_text = graphos_factory_core::json::compact(&vendor);
    std::fs::write(&from, &vendor_text).unwrap();

    assert_eq!(
        refresh(
            d,
            &from,
            &[
                "--reason",
                "vendor published 1.1.0",
                "--retrieved-at",
                "2026-09-11T10:00:00Z"
            ]
        ),
        0
    );

    // The upstream is the vendor's bytes; the working copy is those plus
    // the one patch that still applied.
    assert_eq!(
        std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap(),
        vendor_text.as_bytes()
    );
    let working: Value = graphos_factory_core::json::parse(&read(d, "openapi.json")).unwrap();
    assert_eq!(working["info"]["version"], "1.1.0");
    assert_eq!(
        working["components"]["schemas"]["Widget"]["required"],
        json!(["id"])
    );
    assert_eq!(
        working["components"]["schemas"]["Widget"]["properties"]["name"]["nullable"],
        json!(true)
    );
    assert!(working["paths"]["/gadgets"].is_object());

    // The entry: new hash, version and retrieved_at, one surviving patch
    // with its reason and decision intact.
    let e = entry(d);
    assert_eq!(e["upstream_sha256"], sha(vendor_text.as_bytes()));
    assert_ne!(e["upstream_sha256"], old_sha);
    assert_eq!(e["version"], "3.0.3");
    assert_eq!(e["kind"], "openapi");
    assert_eq!(e["retrieved_at"], "2026-09-11T10:00:00Z");
    assert_eq!(e["url"], "https://api.widgets.test/openapi.json", "kept");
    let patches = e["patches"].as_array().unwrap();
    assert_eq!(patches.len(), 1, "{}", e["patches"]);
    assert_eq!(patches[0]["op"], "add");
    assert_eq!(
        patches[0]["path"],
        "/components/schemas/Widget/properties/name/nullable"
    );
    assert_eq!(
        patches[0]["reason"],
        "live API returns null name for archived widgets"
    );
    assert_eq!(patches[0]["decision"], "D-0001");
    assert_eq!(
        validate(
            &sources_lock(d),
            &schemas::load("sources-lock.schema.json", None).unwrap()
        ),
        Vec::<String>::new()
    );
    assert!(
        read(d, ".factory/sources.lock.yaml").contains("note: >-"),
        "the docs entry is untouched"
    );

    // The decision names both hashes and each patch's fate, the dropped
    // patch's reason included.
    let decisions = prose(d, &the_finding(d));
    assert!(
        decisions.contains("Upstream refreshed: openapi.json"),
        "{}",
        decisions
    );
    assert!(decisions.contains(&old_sha), "{}", decisions);
    assert!(
        decisions.contains(&sha(vendor_text.as_bytes())),
        "{}",
        decisions
    );
    assert!(
        decisions.contains("vendor published 1.1.0"),
        "{}",
        decisions
    );
    assert!(
        decisions.contains("1 re-applied")
            && decisions.contains("1 obsolete")
            && decisions.contains("0 in conflict"),
        "{}",
        decisions
    );
    let line = |prefix: &str| -> String {
        decisions
            .lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no line starting {:?} in {}", prefix, decisions))
            .to_string()
    };
    let obsolete = line("- obsolete:");
    assert!(
        obsolete.contains("replace /components/schemas/Widget/required (was [\"id\",\"name\"])")
            && obsolete.contains("[D-0001: live API returns null name for archived widgets]"),
        "{}",
        obsolete
    );
    let reapplied = line("- reapplied:");
    assert!(
        reapplied.contains("add /components/schemas/Widget/properties/name/nullable")
            && reapplied.contains("[D-0001:"),
        "{}",
        reapplied
    );

    // Everything downstream agrees: no hand edit, no source finding, the
    // inventory rebuilt with the new operation and the patch mark.
    assert_eq!(lock_check(d), 0);
    assert_eq!(source_rules(d), Vec::<String>::new());
    let inv: Value =
        graphos_factory_core::json::parse(&read(d, ".factory/inventory.json")).unwrap();
    let keys: Vec<&str> = inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"get:/gadgets"), "{:?}", keys);
    let widgets = inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == "get:/widgets")
        .unwrap();
    assert!(
        widgets["patches"]
            .as_array()
            .map(|p| !p.is_empty())
            .unwrap_or(false),
        "the surviving patch still marks the operation: {}",
        widgets
    );
    let applied =
        graphos_factory_core::yaml::parse(&read(d, ".factory/applied.lock.yaml")).unwrap();
    assert_eq!(
        applied["sources"]["openapi.json"],
        graphos_factory_core::patch::canonical_sha256(&working)
    );
    let inventory_bytes = std::fs::read(d.join(".factory/inventory.json")).unwrap();
    assert_eq!(
        applied["provenance"]["inputs"][".factory/inventory.json"]["sha256"],
        graphos_factory_core::patch::bytes_sha256(&inventory_bytes),
        "provenance must describe the rebuilt inventory, not its pre-refresh bytes"
    );
}

#[test]
fn a_conflict_is_dropped_not_applied_recorded_and_exits_3() {
    let ws = workspace();
    let d = ws.path();

    // Vendor 2.0.0: `required` grew a third member — neither the value the
    // patch replaced nor the one it wrote.
    let mut vendor = spec();
    vendor["info"]["version"] = json!("2.0.0");
    vendor["components"]["schemas"]["Widget"]["required"] = json!(["id", "name", "sku"]);
    vendor["components"]["schemas"]["Widget"]["properties"]["sku"] = json!({"type": "string"});
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi.json");
    std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();

    let (code, summary) = refresh_json(d, &from, &[]);
    assert_eq!(code, 3);
    assert_eq!(summary["exit"], 3);
    assert_eq!(summary["dry_run"], false);
    assert_eq!(summary["finding"], the_finding(d).as_str());
    assert_eq!(summary["acknowledged_in_applied_lock"], true);
    assert_eq!(summary["patches"]["recorded"], 2);
    assert_eq!(summary["patches"]["reapplied"], 1);
    assert_eq!(summary["patches"]["obsolete"], 0);
    assert_eq!(summary["patches"]["conflicts"], 1);
    let outcomes = summary["patches"]["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(outcomes[0]["op"], "replace");
    assert_eq!(outcomes[0]["path"], "/components/schemas/Widget/required");
    assert_eq!(outcomes[0]["fate"], "conflict");
    assert_eq!(outcomes[0]["decision"], "D-0001");
    assert_eq!(outcomes[1]["fate"], "reapplied");
    assert_eq!(summary["inventory"]["added"], json!([]));
    assert_eq!(summary["inventory"]["removed"], json!([]));
    assert_eq!(
        summary["writes"],
        json!([
            ".factory/sources/openapi.upstream.json",
            "openapi.json",
            ".factory/sources.lock.yaml",
            ".factory/findings",
            ".factory/inventory.json",
            ".factory/applied.lock.yaml"
        ])
    );
    assert_eq!(
        summary["upstream_sha256"],
        sha(read(d, ".factory/sources/openapi.upstream.json").as_bytes())
    );

    let working: Value = graphos_factory_core::json::parse(&read(d, "openapi.json")).unwrap();
    assert_eq!(
        working["components"]["schemas"]["Widget"]["required"],
        json!(["id", "name", "sku"]),
        "the conflicting patch was not applied"
    );
    assert_eq!(
        working["components"]["schemas"]["Widget"]["properties"]["name"]["nullable"],
        json!(true),
        "the other patch was"
    );
    let e = entry(d);
    let patches = e["patches"].as_array().unwrap();
    assert_eq!(patches.len(), 1, "{}", e["patches"]);
    assert_eq!(patches[0]["op"], "add");
    let decisions = prose(d, &the_finding(d));
    assert!(decisions.contains("1 in conflict"), "{}", decisions);
    let conflict = decisions
        .lines()
        .find(|l| l.starts_with("- conflict:"))
        .unwrap_or_else(|| panic!("{}", decisions));
    assert!(
        conflict.contains("replace /components/schemas/Widget/required (was [\"id\",\"name\"])")
            && conflict.contains("[\"id\",\"name\",\"sku\"]")
            && conflict.contains("[D-0001: live API returns null name for archived widgets]"),
        "{}",
        conflict
    );
    // The workspace is consistent even so: the lock describes the working
    // copy, nothing is a hand edit, and the engineer's next step is codify.
    assert_eq!(lock_check(d), 0);
    assert_eq!(source_rules(d), Vec::<String>::new());
}

#[test]
fn refresh_refuses_what_it_would_lose_or_cannot_trust_and_writes_nothing() {
    let ws = workspace();
    let d = ws.path();
    let mut vendor = spec();
    vendor["info"]["version"] = json!("1.1.0");
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi.json");
    std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();

    // Usage: no --from; no --reason.
    assert_eq!(
        cmd::sources::main(&args(&[
            "refresh",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--reason",
            "x"
        ])),
        1
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "refresh",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--from",
            from.to_str().unwrap()
        ])),
        1
    );

    // An uncodified edit to the working copy would be lost.
    let clean = snapshot(d);
    let mut edited = corrected(spec());
    edited["info"]["description"] = json!("hand note");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(lock_check(d), 3);
    let edited_snapshot = snapshot(d);
    assert_eq!(refresh(d, &from, &[]), 2);
    assert_eq!(snapshot(d), edited_snapshot, "nothing written");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&corrected(spec())),
    );
    assert_eq!(snapshot(d), clean);

    // A modified upstream is restored, never refreshed over.
    let upstream = read(d, ".factory/sources/openapi.upstream.json");
    write(
        d,
        ".factory/sources/openapi.upstream.json",
        &upstream.replace("Widget Co", "Widget Corp"),
    );
    assert_eq!(refresh(d, &from, &[]), 2);
    write(d, ".factory/sources/openapi.upstream.json", &upstream);
    assert_eq!(snapshot(d), clean);

    // --from must be a description document, and never the working copy.
    let junk = fetched.path().join("junk.json");
    std::fs::write(&junk, "{\"hello\": 1}").unwrap();
    assert_eq!(refresh(d, &junk, &[]), 2);
    assert_eq!(refresh(d, &d.join("openapi.json"), &[]), 2);
    assert_eq!(snapshot(d), clean);

    // The same bytes again: nothing to refresh.
    assert_eq!(
        refresh(d, &d.join(".factory/sources/openapi.upstream.json"), &[]),
        0
    );
    assert_eq!(snapshot(d), clean);

    // A vendor document the reader cannot build an inventory from (a
    // dangling $ref) is refused before anything is written — in a dry run
    // and a real run alike.
    let mut broken = spec();
    broken["info"]["version"] = json!("1.2.0");
    broken["paths"]["/gadgets"] = json!({"get": {
        "operationId": "listGadgets",
        "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"allOf": [{"$ref": "#/components/schemas/Gone"}]}}}}}
    }});
    let broken_file = fetched.path().join("broken.json");
    std::fs::write(&broken_file, graphos_factory_core::json::pretty(&broken)).unwrap();
    assert_eq!(refresh(d, &broken_file, &["--dry-run"]), 2);
    assert_eq!(refresh(d, &broken_file, &[]), 2);
    assert_eq!(snapshot(d), clean);

    // An unpinned path.
    write(
        d,
        "other.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    assert_eq!(refresh_at(d, "other.json", &from, &[]), 2);
    assert_eq!(snapshot(d), clean);
}

#[test]
fn a_dry_run_reports_the_fates_and_the_inventory_diff_and_writes_nothing() {
    let ws = workspace();
    let d = ws.path();
    let mut vendor = spec();
    vendor["info"]["version"] = json!("1.1.0");
    vendor["components"]["schemas"]["Widget"]["required"] = json!(["id"]);
    vendor["paths"]["/gadgets"] = json!({"get": {
        "operationId": "listGadgets",
        "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/WidgetList"}}}}}
    }});
    vendor["paths"]["/widgets"]["get"]["deprecated"] = json!(true);
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi.json");
    std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();
    let clean = snapshot(d);
    assert_eq!(refresh(d, &from, &["--dry-run"]), 0);
    assert_eq!(snapshot(d), clean);

    // The same replay, read through the library the command uses.
    let entry_patches = entry(d)["patches"].as_array().unwrap().clone();
    let r = replay(&vendor, &entry_patches);
    assert_eq!(r.count(Fate::Obsolete), 1);
    assert_eq!(r.count(Fate::Reapplied), 1);
    assert_eq!(r.count(Fate::Conflict), 0);
    let before = inventory(&spec());
    let after = inventory(&r.document);
    let diff = graphos_factory_core::inventory::diff_inventories(&before, &after);
    assert_eq!(diff["added"], json!(["get:/gadgets"]));
    assert_eq!(diff["removed"], json!([]));
    let changed = diff["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1, "{}", diff);
    assert_eq!(changed[0]["key"], "get:/widgets");
    assert!(
        changed[0]["fields"]
            .as_array()
            .unwrap()
            .contains(&json!("deprecated")),
        "{}",
        changed[0]
    );
}

#[test]
fn when_no_patch_survives_the_working_copy_is_the_vendor_bytes_verbatim() {
    let ws = workspace();
    let d = ws.path();
    // Vendor adopted both corrections, and reformatted its document.
    let vendor = corrected(spec());
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi.json");
    let text = format!("{}\n\n", graphos_factory_core::json::compact(&vendor));
    std::fs::write(&from, &text).unwrap();
    assert_eq!(refresh(d, &from, &[]), 0);
    assert_eq!(read(d, "openapi.json"), text);
    assert_eq!(read(d, ".factory/sources/openapi.upstream.json"), text);
    let e = entry(d);
    assert_eq!(e["patches"], json!([]));
    assert_eq!(lock_check(d), 0);
    assert_eq!(source_rules(d), Vec::<String>::new());
    let decisions = prose(d, &the_finding(d));
    assert!(decisions.contains("2 obsolete"), "{}", decisions);
}

#[test]
fn a_swagger_document_replacing_an_openapi_one_records_the_new_kind() {
    let ws = workspace();
    let d = ws.path();
    let vendor = json!({
        "swagger": "2.0",
        "info": {"title": "Widget Co", "version": "3.0.0"},
        "host": "api.widgets.test",
        "schemes": ["https"],
        "definitions": {
            "Widget": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}, "name": {"type": "string", "x-nullable": true}}},
            "WidgetList": {"type": "object", "properties": {"widgets": {"type": "array", "items": {"$ref": "#/definitions/Widget"}}}}
        },
        "paths": {"/widgets": {"get": {
            "operationId": "listWidgets",
            "produces": ["application/json"],
            "parameters": [{"name": "limit", "in": "query", "type": "integer"}],
            "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/WidgetList"}}}
        }}}
    });
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("swagger.json");
    std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();
    // Both patches point under /components, which a Swagger document does
    // not have: the parent of the `add` is gone (conflict), the `replace`
    // target is gone (conflict).
    assert_eq!(refresh(d, &from, &[]), 3);
    let e = entry(d);
    assert_eq!(e["kind"], "swagger");
    assert_eq!(e["version"], "2.0");
    assert_eq!(e["patches"], json!([]));
    assert_eq!(lock_check(d), 0);
    assert_eq!(source_rules(d), Vec::<String>::new());
    let decisions = prose(d, &the_finding(d));
    assert!(
        decisions.contains("swagger 2.0, previously openapi"),
        "{}",
        decisions
    );
    assert!(decisions.contains("2 in conflict"), "{}", decisions);
    let inv: Value =
        graphos_factory_core::json::parse(&read(d, ".factory/inventory.json")).unwrap();
    assert_eq!(inv["operations"].as_array().unwrap().len(), 1);
}

#[test]
fn replay_classifies_every_op_kind_against_what_the_vendor_did() {
    let base = json!({"a": {"x": 1, "y": 2, "z": 3, "list": [1, 2]}, "b": "keep", "nul": "now meaningful", "nul2": null});
    let p = |op: &str, path: &str, value: Option<Value>, was: Option<Value>| -> Value {
        let mut m = json!({"op": op, "path": path, "reason": "r", "decision": "D-0001"});
        if let Some(v) = value {
            m["value"] = v;
        }
        if let Some(w) = was {
            m["was"] = w;
        }
        m
    };
    let patches = vec![
        // add: absent → re-applied
        p("add", "/a/w", Some(json!(9)), None),
        // add: present with the same value → obsolete
        p("add", "/b", Some(json!("keep")), None),
        // add: present with another value → conflict
        p("add", "/a/x", Some(json!(100)), None),
        // add: parent gone → conflict
        p("add", "/nope/q", Some(json!(1)), None),
        // add: into an array position, value absent → re-applied
        p("add", "/a/list/0", Some(json!(0)), None),
        // add: at the array's end, value already present → obsolete
        p("add", "/a/list/-", Some(json!(2)), None),
        // replace: `was` matches → re-applied
        p("replace", "/a/y", Some(json!(20)), Some(json!(2))),
        // replace: target already holds value → obsolete
        p("replace", "/a/z", Some(json!(3)), Some(json!(30))),
        // replace: vendor changed it elsewhere → conflict
        p("replace", "/a/x", Some(json!(10)), Some(json!(11))),
        // replace: target gone → conflict
        p("replace", "/gone", Some(json!(1)), Some(json!(0))),
        // replace: no `was` → re-applied unchecked
        p("replace", "/b", Some(json!("new")), None),
        // remove: `was` matches (after the replace above wrote 20) → re-applied
        p("remove", "/a/y", None, Some(json!(20))),
        // remove: target gone → obsolete
        p("remove", "/a/missing", None, Some(json!(1))),
        // remove: vendor changed the value → conflict
        p("remove", "/a/x", None, Some(json!(999))),
        // remove: no `was` → re-applied unchecked
        p("remove", "/a/z", None, None),
        // a malformed pointer → conflict
        p("add", "no-slash", Some(json!(1)), None),
        // an explicit null is a value: add null where absent → re-applied
        p("add", "/a/example", Some(Value::Null), None),
        // replace with `was: null` when the vendor moved off null → conflict
        p("replace", "/nul", Some(json!("patched")), Some(Value::Null)),
        // remove with `was: null` when the vendor moved off null → conflict
        p("remove", "/nul", None, Some(Value::Null)),
        // remove with `was: null` still null → re-applied
        p("remove", "/nul2", None, Some(Value::Null)),
    ];
    let r = replay(&base, &patches);
    let fates: Vec<&str> = r.outcomes.iter().map(|o| o.fate.as_str()).collect();
    assert_eq!(
        fates,
        vec![
            "reapplied", // 0 add /a/w
            "obsolete",  // 1 add /b same value
            "conflict",  // 2 add /a/x other value
            "conflict",  // 3 add /nope/q parent gone
            "reapplied", // 4 add /a/list/0 absent value
            "obsolete",  // 5 add /a/list/- present value
            "reapplied", // 6 replace /a/y was matches
            "obsolete",  // 7 replace /a/z already value
            "conflict",  // 8 replace /a/x vendor changed
            "conflict",  // 9 replace /gone
            "reapplied", // 10 replace /b no was
            "reapplied", // 11 remove /a/y was matches
            "obsolete",  // 12 remove /a/missing
            "conflict",  // 13 remove /a/x vendor changed
            "reapplied", // 14 remove /a/z no was
            "conflict",  // 15 malformed pointer
            "reapplied", // 16 add null
            "conflict",  // 17 replace was null, vendor moved
            "conflict",  // 18 remove was null, vendor moved
            "reapplied", // 19 remove was null, still null
        ]
    );
    assert_eq!(
        r.document,
        json!({"a": {"x": 1, "w": 9, "list": [0, 1, 2], "example": null}, "b": "new", "nul": "now meaningful"})
    );
    assert_eq!(r.kept().len(), 8);
    let whys: Vec<&str> = r.outcomes.iter().map(|o| o.why.as_str()).collect();
    assert!(
        whys[2].contains("as 1") && whys[2].contains("adds 100"),
        "{}",
        whys[2]
    );
    assert!(whys[3].contains("parent"), "{}", whys[3]);
    assert!(whys[5].contains("already contains"), "{}", whys[5]);
    assert!(
        whys[8].contains("to 1") && whys[8].contains("replaced 11 with 10"),
        "{}",
        whys[8]
    );
    assert!(whys[10].starts_with("no `was` recorded"), "{}", whys[10]);
    assert!(whys[13].contains("removed 999"), "{}", whys[13]);
    assert!(
        whys[17].contains("replaced null with \"patched\""),
        "an explicit null `was` is a value, not a missing key: {}",
        whys[17]
    );
    assert!(whys[18].contains("removed null"), "{}", whys[18]);
    // Report lines group by fate (re-applied, obsolete, conflict) and cite
    // the decision and reason, one line each.
    let lines = graphos_factory_core::refresh::report_lines(&r);
    assert_eq!(lines.len(), 20);
    assert!(lines.iter().all(|l| !l.contains('\n')));
    assert!(
        lines[0].starts_with("reapplied: add /a/w") && lines[0].ends_with("[D-0001: r]"),
        "{}",
        lines[0]
    );
    assert!(lines[8].starts_with("obsolete:  add /b"), "{}", lines[8]);
    assert!(
        lines[12].starts_with("conflict:  add /a/x"),
        "{}",
        lines[12]
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("conflict:  replace /nul (was null)")),
        "{:?}",
        lines
    );
}

#[test]
fn a_yaml_working_copy_is_rewritten_as_yaml_and_its_patches_still_reproduce_it() {
    let ws = workspace_at("openapi.yaml", &|v| {
        graphos_factory_core::yaml::stringify(v, 0)
    });
    let d = ws.path();
    assert_eq!(
        entry_at(d, "openapi.yaml")["patches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // Vendor 1.1.0 as YAML, one correction adopted.
    let mut vendor = spec();
    vendor["info"]["version"] = json!("1.1.0");
    vendor["components"]["schemas"]["Widget"]["required"] = json!(["id"]);
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi.yaml");
    std::fs::write(&from, graphos_factory_core::yaml::stringify(&vendor, 0)).unwrap();
    assert_eq!(refresh_at(d, "openapi.yaml", &from, &[]), 0);
    let text = read(d, "openapi.yaml");
    assert!(
        text.starts_with("openapi:"),
        "YAML, not JSON: {}",
        &text[..40]
    );
    let working = graphos_factory_core::yaml::parse(&text).unwrap();
    assert_eq!(working["info"]["version"], "1.1.0");
    assert_eq!(
        working["components"]["schemas"]["Widget"]["properties"]["name"]["nullable"],
        json!(true)
    );
    assert_eq!(
        entry_at(d, "openapi.yaml")["patches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(lock_check(d), 0);
    assert_eq!(source_rules(d), Vec::<String>::new());

    // A JSON working copy refreshed from a YAML vendor file with nothing
    // surviving is serialised as JSON, never handed the YAML bytes.
    let ws2 = workspace();
    let d2 = ws2.path();
    let from2 = fetched.path().join("all-adopted.yaml");
    std::fs::write(
        &from2,
        graphos_factory_core::yaml::stringify(&corrected(spec()), 0),
    )
    .unwrap();
    assert_eq!(refresh(d2, &from2, &[]), 0);
    let text2 = read(d2, "openapi.json");
    assert!(text2.trim_start().starts_with('{'), "{}", &text2[..40]);
    assert!(
        read(d2, ".factory/sources/openapi.upstream.json").starts_with("openapi:"),
        "the vendor copy keeps the vendor's bytes"
    );
    assert_eq!(entry(d2)["patches"], json!([]));
    assert_eq!(lock_check(d2), 0);
}

#[test]
fn a_second_refresh_replays_only_the_surviving_patches_and_keeps_the_url_unless_given() {
    let ws = workspace();
    let d = ws.path();
    let fetched = tempfile::tempdir().unwrap();
    // 1.1.0 adopts `required`; 1.2.0 adopts `nullable` too and moves the URL.
    let mut v1 = spec();
    v1["info"]["version"] = json!("1.1.0");
    v1["components"]["schemas"]["Widget"]["required"] = json!(["id"]);
    let f1 = fetched.path().join("v1.json");
    std::fs::write(&f1, graphos_factory_core::json::pretty(&v1)).unwrap();
    assert_eq!(refresh(d, &f1, &[]), 0);
    assert_eq!(entry(d)["url"], "https://api.widgets.test/openapi.json");
    assert_eq!(entry(d)["patches"].as_array().unwrap().len(), 1);

    let mut v2 = corrected(spec());
    v2["info"]["version"] = json!("1.2.0");
    let f2 = fetched.path().join("v2.json");
    std::fs::write(&f2, graphos_factory_core::json::pretty(&v2)).unwrap();
    assert_eq!(
        refresh(
            d,
            &f2,
            &["--url", "https://api.widgets.test/v2/openapi.json"]
        ),
        0
    );
    let e = entry(d);
    assert_eq!(e["url"], "https://api.widgets.test/v2/openapi.json");
    assert_eq!(e["patches"], json!([]));
    assert_eq!(
        e["upstream_sha256"],
        sha(read(d, ".factory/sources/openapi.upstream.json").as_bytes())
    );
    assert_eq!(finding_ids(d).len(), 2, "{}", all_prose(d));
    assert!(
        all_prose(d).contains("Of 1 recorded patch,"),
        "the second refresh saw one patch: {}",
        all_prose(d)
    );
    assert_eq!(lock_check(d), 0);
    assert_eq!(source_rules(d), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn a_write_that_fails_after_the_first_file_is_exit_4_and_names_what_was_written() {
    use std::os::unix::fs::PermissionsExt;
    let ws = workspace();
    let d = ws.path();
    let mut vendor = spec();
    vendor["info"]["version"] = json!("1.1.0");
    let fetched = tempfile::tempdir().unwrap();
    let from = fetched.path().join("openapi.json");
    std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();
    // The findings directory readable but not writable: `refresh` loads it
    // before the write phase (custody demands a real directory, ADR 0025)
    // and the fourth write, the new finding's own file (ADR 0118), fails. A
    // symlink here would be refused before anything is written, which is
    // the next test.
    let decisions = d.join(".factory/findings");
    std::fs::create_dir_all(&decisions).unwrap();
    let mut perms = std::fs::metadata(&decisions).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(&decisions, perms).unwrap();
    assert!(
        std::fs::write(decisions.join("probe.json"), "x").is_err(),
        "this test needs a write that really fails; running as root defeats the mode bit"
    );
    let (code, summary) = refresh_json(d, &from, &[]);
    assert_eq!(code, 4);
    assert_eq!(summary["exit"], 4);
    assert_eq!(
        summary["written"],
        json!([
            ".factory/sources/openapi.upstream.json",
            "openapi.json",
            ".factory/sources.lock.yaml"
        ])
    );
    assert!(
        summary["incomplete"]
            .as_str()
            .unwrap()
            .contains("the finding could not be written"),
        "{}",
        summary
    );
    // What it says was written, was; what it does not, was not.
    assert_eq!(
        read(d, ".factory/sources/openapi.upstream.json"),
        graphos_factory_core::json::pretty(&vendor)
    );
    assert_eq!(entry(d)["version"], "3.0.3");
    assert_eq!(
        entry(d)["upstream_sha256"],
        sha(graphos_factory_core::json::pretty(&vendor).as_bytes())
    );
    let inv: Value =
        graphos_factory_core::json::parse(&read(d, ".factory/inventory.json")).unwrap();
    assert_eq!(
        inv["api"]["version"], "1.0.0",
        "the inventory was not reached"
    );
}

/// Custody reads every `.factory` input before the first write, so a
/// `.factory` file that is not a regular file — a directory, or a symlink
/// out of the workspace — is exit 2 with nothing written, not a partial
/// refresh (ADR 0025).
///
/// The symlink case points at a *valid* findings file on purpose: if it
/// pointed at junk, the refusal would be indistinguishable from a parse
/// error and the case would pass with custody removed.
#[test]
fn a_factory_input_that_is_not_a_regular_file_refuses_before_any_write() {
    let elsewhere = tempfile::tempdir().unwrap();
    let valid_log = elsewhere.path().join("someone-elses-findings.json");
    std::fs::write(
        &valid_log,
        graphos_factory_core::json::pretty(&graphos_factory_core::findings::empty()),
    )
    .unwrap();

    let cases: Vec<(&str, Box<dyn Fn(&Path)>)> = vec![
        (
            "a directory",
            Box::new(|p: &Path| std::fs::create_dir(p).unwrap()),
        ),
        #[cfg(unix)]
        (
            "a symlink out of the workspace, to a log that reads perfectly",
            Box::new(move |p: &Path| std::os::unix::fs::symlink(&valid_log, p).unwrap()),
        ),
    ];
    for (what, make) in cases {
        let ws = workspace();
        let d = ws.path();
        let before = read(d, ".factory/sources/openapi.upstream.json");
        let mut vendor = spec();
        vendor["info"]["version"] = json!("1.1.0");
        let fetched = tempfile::tempdir().unwrap();
        let from = fetched.path().join("openapi.json");
        std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();
        // findings.json is the file refresh writes its record to (ADR 0113).
        let decisions = d.join(".factory/findings.json");
        make(&decisions);

        let code = cmd::sources::main(&args(&[
            "refresh",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--from",
            from.to_str().unwrap(),
            "--reason",
            "vendor published 1.1.0",
        ]));
        assert_eq!(code, 2, "{what}");
        assert_eq!(
            read(d, ".factory/sources/openapi.upstream.json"),
            before,
            "{what}: nothing is written"
        );
    }
}

#[test]
fn a_multi_line_reason_is_one_line_in_the_decision_and_an_unbuildable_document_is_refused_without_an_inventory(
) {
    let ws = workspace();
    let d = ws.path();
    let fetched = tempfile::tempdir().unwrap();
    // No inventory yet: the buildability gate still runs.
    std::fs::remove_file(d.join(".factory/inventory.json")).unwrap();
    let mut broken = spec();
    broken["info"]["version"] = json!("1.2.0");
    broken["paths"]["/gadgets"] = json!({"get": {
        "operationId": "listGadgets",
        "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"allOf": [{"$ref": "#/components/schemas/Gone"}]}}}}}
    }});
    let broken_file = fetched.path().join("broken.json");
    std::fs::write(&broken_file, graphos_factory_core::json::pretty(&broken)).unwrap();
    let before = read(d, "openapi.json");
    assert_eq!(refresh(d, &broken_file, &[]), 2);
    assert_eq!(read(d, "openapi.json"), before);
    assert!(!d.join(".factory/inventory.json").exists());

    let mut vendor = spec();
    vendor["info"]["version"] = json!("1.1.0");
    let from = fetched.path().join("openapi.json");
    std::fs::write(&from, graphos_factory_core::json::pretty(&vendor)).unwrap();
    assert_eq!(
        refresh(
            d,
            &from,
            &["--reason", "vendor published 1.1.0\n\nsee the changelog\n"]
        ),
        0
    );
    let fid = the_finding(d);
    assert!(
        decision_str(d, &fid).contains("vendor published 1.1.0 see the changelog"),
        "{}",
        prose(d, &fid)
    );
    assert!(
        decision_str(d, &fid)
            .lines()
            .any(|l| l == "Reason: vendor published 1.1.0 see the changelog"),
        "the reason is flattened to one line: {}",
        decision_str(d, &fid)
    );
    let doc = findings_doc(d);
    let rec = graphos_factory_core::findings::find(&doc, &fid).unwrap();
    assert_eq!(rec["source"], "sources");
    // No inventory to rebuild, so the evidence names only the document and
    // the command.
    assert_eq!(rec["evidence"][0], "openapi.json", "{}", rec);
    assert_eq!(rec["evidence"].as_array().unwrap().len(), 2, "{}", rec);
    assert!(
        rec["evidence"][1]
            .as_str()
            .unwrap()
            .starts_with("sources refresh --from "),
        "{}",
        rec
    );
}

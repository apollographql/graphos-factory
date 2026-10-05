//! The pinned-source workflow end to end: pin, detect a hand edit, codify it
//! into patches, lint and reconcile agree, a refresh-style check reproduces
//! the working copy from upstream + patches.

use graphos_factory_core::cmd;
use graphos_factory_core::jsonschema::validate;
use graphos_factory_core::lint::{lint_workspace, LintOptions};
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

/// The agent-authored lock before any pin: a docs entry with a folded note
/// that must survive every edit the tool makes to the file.
const SOURCES_LOCK: &str = "contract_version: 1\nsources:\n  - kind: docs\n    url: https://docs.widgets.test/reference\n    retrieved_at: 2026-09-08T14:05:40Z\n    used_for: [\"get:/widgets\"]\n    note: >-\n      Read for the pagination convention the spec does not\n      document; nothing else.\n";

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

fn inventory(spec: &Value) -> Value {
    graphos_factory_core::openapi::build_inventory(spec)
        .unwrap()
        .inventory
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
    graphos_factory_core::json::parse(
        &std::fs::read_to_string(dir.join(".factory/decisions.json")).unwrap(),
    )
    .unwrap()
}

/// title + context + resolution.decision + resolution.note + affects, joined — so
/// the `.contains` checks that scanned the old Markdown block still work.
fn prose(dir: &Path, id: &str) -> String {
    if id.starts_with("F-") {
        // A finding (`pin --force` and `refresh` write one, ADR 0113 §3).
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
        return format!(
            "{} {} {}",
            s("title"),
            s("body"),
            graphos_factory_core::json::strings(rec.get("evidence")).join(" ")
        );
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

/// The findings as every reader sees them: `findings.json`'s, then the ones
/// added since ADR 0118, one file each under `.factory/findings/`.
fn findings_doc(dir: &Path) -> Value {
    graphos_factory_core::findings::load(dir, None).unwrap()
}

fn finding_ids(dir: &Path) -> Vec<String> {
    graphos_factory_core::json::get_arr(&findings_doc(dir), "findings")
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("id").and_then(|x| x.as_str()).map(str::to_string))
        .collect()
}

/// The prose of every record joined — decisions and findings — for
/// assertions where the id isn't known.
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

/// A locked workspace with a pretty-printed spec as its working copy.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(d, "widget-co.graphql", SDL);
    write(d, ".factory/workspace.yaml", WORKSPACE);
    write(d, ".factory/selection.yaml", SELECTION);
    write(d, ".factory/decisions.json", DECISIONS);
    write(d, ".factory/sources.lock.yaml", SOURCES_LOCK);
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
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
    dir
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn pin(d: &Path) -> i32 {
    cmd::sources::main(&args(&[
        "pin",
        d.to_str().unwrap(),
        "--path",
        "openapi.json",
        "--url",
        "https://api.widgets.test/openapi.json",
    ]))
}

fn lock_check(d: &Path) -> i32 {
    cmd::lock::main(&args(&[d.to_str().unwrap(), "--check"]))
}

fn make_provenance_unreadable(d: &Path) {
    let mut applied =
        graphos_factory_core::yaml::parse(&read(d, ".factory/applied.lock.yaml")).unwrap();
    graphos_factory_core::provenance::refresh(d, &mut applied, None, None).unwrap();
    write(
        d,
        ".factory/applied.lock.yaml",
        &graphos_factory_core::yaml::stringify(&applied, 0),
    );
    write(
        d,
        ".factory/context.yaml",
        &format!(
            "evidence:\n  - path: {}\n    sha256: recorded\n",
            d.join("widget-co.graphql").display()
        ),
    );
}

fn lint(d: &Path) -> Vec<(String, String)> {
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
    .map(|f| (f.severity, f.rule))
    .collect()
}

fn rules(d: &Path) -> Vec<String> {
    lint(d).into_iter().map(|(_, r)| r).collect()
}

fn sources_lock(d: &Path) -> Value {
    graphos_factory_core::yaml::parse(&read(d, ".factory/sources.lock.yaml")).unwrap()
}

fn entry(d: &Path) -> Value {
    sources_lock(d)["sources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["path"] == "openapi.json")
        .cloned()
        .expect("an entry for openapi.json")
}

#[test]
fn pin_records_kind_version_and_upstream_without_touching_the_docs_entry() {
    let ws = workspace();
    let d = ws.path();
    assert!(!rules(d).contains(&"no-applied-lock".to_string()));
    assert_eq!(pin(d), 0);

    let text = read(d, ".factory/sources.lock.yaml");
    assert!(text.contains("    note: >-\n      Read for the pagination convention the spec does not\n      document; nothing else.\n"), "the folded note survives:\n{}", text);
    let e = entry(d);
    assert_eq!(e["kind"], "openapi");
    assert_eq!(e["version"], "3.0.3");
    assert_eq!(e["url"], "https://api.widgets.test/openapi.json");
    assert_eq!(e["upstream"], ".factory/sources/openapi.upstream.json");
    let upstream = std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    assert_eq!(
        upstream,
        std::fs::read(d.join("openapi.json")).unwrap(),
        "the upstream is the working copy's bytes at pin time"
    );
    assert_eq!(
        e["upstream_sha256"],
        graphos_factory_core::patch::bytes_sha256(&upstream)
    );
    assert_eq!(
        validate(
            &sources_lock(d),
            &schemas::load("sources-lock.schema.json", None).unwrap()
        ),
        Vec::<String>::new()
    );

    let applied =
        graphos_factory_core::yaml::parse(&read(d, ".factory/applied.lock.yaml")).unwrap();
    assert_eq!(
        applied["sources"]["openapi.json"],
        graphos_factory_core::patch::canonical_sha256(&spec())
    );
    assert_eq!(
        validate(
            &applied,
            &schemas::load("applied-lock.schema.json", None).unwrap()
        ),
        Vec::<String>::new()
    );
    assert_eq!(lock_check(d), 0);
    assert!(
        !rules(d)
            .iter()
            .any(|r| r.starts_with("source") || r == "unacknowledged-source-edit"),
        "{:?}",
        rules(d)
    );

    // Pinning again keeps the upstream (no --force) and stays in sync.
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::compact(&spec()),
    );
    assert_eq!(pin(d), 0);
    assert_eq!(
        std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap(),
        upstream
    );
}

#[test]
fn a_swagger_document_is_pinned_as_kind_swagger_with_a_quoted_version() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(
            &json!({"swagger": "2.0", "info": {"title": "L", "version": "1"}, "host": "api.l.test", "paths": {}}),
        ),
    );
    assert_eq!(pin(d), 0);
    let e = entry(d);
    assert_eq!(e["kind"], "swagger");
    assert_eq!(
        e["version"], "2.0",
        "a YAML number would have been read as 2"
    );
    assert!(read(d, ".factory/sources.lock.yaml").contains("version: \"2.0\""));
}

#[test]
fn formatting_is_not_a_hand_edit_but_a_content_change_is_until_codified() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);

    // Reformat only.
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::compact(&spec()),
    );
    assert_eq!(lock_check(d), 0, "formatting never counts");

    // A real edit: the vendor says `name` is required, the API sends null.
    let mut edited = spec();
    edited["components"]["schemas"]["Widget"]["required"] = json!(["id"]);
    edited["components"]["schemas"]["Widget"]["properties"]["name"]["nullable"] = json!(true);
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(lock_check(d), 3);
    let found = lint(d);
    assert!(
        found.contains(&(
            "error".to_string(),
            "unacknowledged-source-edit".to_string()
        )),
        "{:?}",
        found
    );
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["sources"][0]["hand_edit"], true);
    assert_eq!(report["clean"], false);

    // Codify.
    let code = cmd::codify::main(&args(&[
        d.to_str().unwrap(),
        "--source",
        "openapi.json",
        "--reason",
        "live API returns null name for archived widgets",
        "--context",
        "Observed on the archived widget the e2e fixture records.",
    ]));
    assert_eq!(code, 0);
    let e = entry(d);
    let patches = e["patches"].as_array().unwrap();
    assert_eq!(patches.len(), 2, "{}", e["patches"]);
    assert_eq!(patches[0]["op"], "replace");
    assert_eq!(patches[0]["path"], "/components/schemas/Widget/required");
    assert_eq!(patches[0]["was"], json!(["id", "name"]));
    assert_eq!(
        patches[0]["reason"],
        "live API returns null name for archived widgets"
    );
    // The patch carries the why (reason, context) and no decision: codify
    // writes no record (ADR 0113 §3).
    assert_eq!(patches[0]["decision"], Value::Null);
    assert_eq!(
        patches[0]["context"],
        "Observed on the archived widget the e2e fixture records."
    );
    assert_eq!(patches[1]["op"], "add");
    assert_eq!(
        patches[1]["path"],
        "/components/schemas/Widget/properties/name/nullable"
    );
    assert_eq!(decision_ids(d), vec!["D-0001"], "no decision was appended");
    assert!(!d.join(".factory/findings.json").exists());
    assert!(!d.join(".factory/findings").exists());
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

    // In sync again everywhere.
    assert_eq!(lock_check(d), 0);
    assert!(
        !rules(d).contains(&"unacknowledged-source-edit".to_string()),
        "{:?}",
        rules(d)
    );
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["sources"][0]["hand_edit"], false);
    assert_eq!(report["sources"][0]["patches"], 2);
    assert_eq!(report["sources"][0]["patches_ok"], true);

    // Upstream + patches reproduce the working copy (what a refresh relies on).
    let upstream =
        graphos_factory_core::sources::load_document(d, ".factory/sources/openapi.upstream.json")
            .unwrap();
    assert_eq!(
        graphos_factory_core::patch::apply(&upstream, patches).unwrap(),
        edited
    );

    // A second edit elsewhere keeps the first patches' reason and decision.
    let mut edited2 = edited.clone();
    edited2["paths"]["/widgets"]["get"]["parameters"][0]["schema"]["maximum"] = json!(100);
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited2),
    );
    assert_eq!(lock_check(d), 3);
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "the API caps limit at 100",
            "--decision",
            "D-0001"
        ])),
        0
    );
    let patches = entry(d)["patches"].as_array().unwrap().clone();
    assert_eq!(patches.len(), 3);
    assert_eq!(patches[0]["decision"], Value::Null);
    assert_eq!(
        patches[0]["context"], "Observed on the archived widget the e2e fixture records.",
        "a recorded patch keeps its context"
    );
    assert_eq!(
        patches[2]["path"],
        "/paths/~1widgets/get/parameters/0/schema/maximum"
    );
    assert_eq!(patches[2]["decision"], "D-0001");
    assert_eq!(patches[2]["context"], Value::Null);
    assert_eq!(decision_ids(d), vec!["D-0001"]);
    assert_eq!(lock_check(d), 0);

    // The inventory built from the working copy marks the touched operation.
    let out = d.join("inv.json");
    assert_eq!(
        cmd::inventory::main(&args(&[
            "build",
            d.join("openapi.json").to_str().unwrap(),
            "--out",
            out.to_str().unwrap()
        ])),
        0
    );
    let inv = graphos_factory_core::json::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let op = &inv["operations"][0];
    let marked: Vec<&str> = op["patches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert!(
        marked.contains(&"/paths/~1widgets/get/parameters/0/schema/maximum"),
        "{:?}",
        marked
    );
    // Widget is reached through WidgetList's items: the closure marks it too.
    assert!(
        marked.contains(&"/components/schemas/Widget/required"),
        "the closure reaches Widget through WidgetList: {:?}",
        marked
    );
    assert_eq!(marked.len(), 3, "{:?}", marked);
    assert_eq!(
        validate(&inv, &schemas::load("inventory.schema.json", None).unwrap()),
        Vec::<String>::new()
    );

    // Reverting the whole edit codifies to an empty patch list.
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "reverted"
        ])),
        0
    );
    assert_eq!(entry(d)["patches"], json!([]));
    assert_eq!(lock_check(d), 0);
}

#[test]
fn a_modified_upstream_is_an_error_everywhere_and_codify_refuses() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let mut tampered = spec();
    tampered["info"]["title"] = json!("Tampered");
    write(
        d,
        ".factory/sources/openapi.upstream.json",
        &graphos_factory_core::json::pretty(&tampered),
    );
    assert_eq!(lock_check(d), 3);
    assert!(lint(d).contains(&("error".to_string(), "source-upstream-modified".to_string())));
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["sources"][0]["upstream_ok"], false);
    assert_eq!(report["clean"], false);
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(
            &json!({"openapi": "3.0.3", "info": {"title": "X", "version": "1"}, "paths": {}}),
        ),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "x"
        ])),
        2
    );
}

#[test]
fn lint_reports_a_missing_upstream_a_kind_mismatch_and_stale_patches() {
    let ws = workspace();
    let d = ws.path();
    // Pinned by hand, no upstream copy, wrong kind.
    write(
        d,
        ".factory/sources.lock.yaml",
        "sources:\n  - kind: swagger\n    path: openapi.json\n",
    );
    let r = rules(d);
    assert!(
        r.contains(&"source-upstream-missing".to_string()),
        "{:?}",
        r
    );
    assert!(r.contains(&"source-kind-mismatch".to_string()), "{:?}", r);

    // Pinned properly, then the patch list is edited to something that does not fit.
    write(d, ".factory/sources.lock.yaml", SOURCES_LOCK);
    assert_eq!(pin(d), 0);
    let text = read(d, ".factory/sources.lock.yaml");
    let with_patch = format!("{}    patches:\n      - op: replace\n        path: /info/title\n        value: Other\n        reason: someone typed this\n", text);
    write(d, ".factory/sources.lock.yaml", &with_patch);
    let r = rules(d);
    assert!(r.contains(&"source-patches-stale".to_string()), "{:?}", r);
    assert_eq!(lock_check(d), 3);

    // The lock file itself is contract-checked.
    write(
        d,
        ".factory/sources.lock.yaml",
        "sources:\n  - kind: mystery\n    path: openapi.json\n",
    );
    let found = lint(d);
    assert!(
        found
            .iter()
            .any(|(sev, rule)| sev == "error" && rule == "contract"),
        "{:?}",
        found
    );
}

#[test]
fn sources_status_reports_and_pin_refuses_a_non_description_file() {
    let ws = workspace();
    let d = ws.path();
    write(d, "notes.json", "{\"hello\": 1}");
    assert_eq!(
        cmd::sources::main(&args(&["pin", d.to_str().unwrap(), "--path", "notes.json"])),
        2
    );
    assert_eq!(
        cmd::sources::main(&args(&["status", d.to_str().unwrap()])),
        0
    );
    assert_eq!(pin(d), 0);
    assert_eq!(
        cmd::sources::main(&args(&["status", d.to_str().unwrap(), "--json"])),
        0
    );
    assert_eq!(cmd::sources::main(&args(&["bogus"])), 1);
}

#[test]
fn re_editing_a_recorded_pointer_is_a_new_patch_with_the_new_reason() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let mut edited = spec();
    edited["info"]["title"] = json!("Widget Co (fixed)");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "first reason",
            "--decision",
            "D-0001"
        ])),
        0
    );
    assert_eq!(entry(d)["patches"][0]["decision"], "D-0001");

    // Same pointer, different value: a new patch, not the old one relabelled.
    edited["info"]["title"] = json!("Something else");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "second reason"
        ])),
        0
    );
    let patches = entry(d)["patches"].as_array().unwrap().clone();
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0]["value"], "Something else");
    assert_eq!(patches[0]["reason"], "second reason");
    // The new patch takes this run's flags: no --decision, so none.
    assert_eq!(patches[0]["decision"], Value::Null);
    assert_eq!(decision_ids(d), vec!["D-0001"]);
    assert_eq!(lock_check(d), 0);
}

#[test]
fn a_flush_left_sources_list_and_a_leading_patches_key_are_edited_in_place() {
    let ws = workspace();
    let d = ws.path();
    // Valid YAML the readers accept: items flush with `sources:`.
    write(
        d,
        ".factory/sources.lock.yaml",
        "sources:\n- kind: docs\n  url: https://docs.widgets.test/reference\n  note: >-\n    folded\n    note\n",
    );
    assert_eq!(pin(d), 0);
    let lock = sources_lock(d);
    assert_eq!(lock["sources"].as_array().unwrap().len(), 2);
    assert_eq!(lock["sources"][0]["note"], "folded note");
    assert_eq!(lock["sources"][1]["path"], "openapi.json");
    let text = read(d, ".factory/sources.lock.yaml");
    assert!(text.contains("\n- kind: openapi\n"), "{}", text);

    let mut edited = spec();
    edited["info"]["title"] = json!("Widget Co (fixed)");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "title"
        ])),
        0
    );
    assert_eq!(entry(d)["patches"].as_array().unwrap().len(), 1);
    assert_eq!(lock_check(d), 0);

    // `patches` as the entry's first key keeps its `- ` marker.
    let e = entry(d);
    let text = format!(
        "sources:\n- kind: docs\n  url: https://docs.widgets.test/reference\n  note: >-\n    folded\n    note\n- patches: []\n  kind: openapi\n  version: \"{}\"\n  path: openapi.json\n  upstream: {}\n  upstream_sha256: {}\n",
        e["version"].as_str().unwrap(),
        e["upstream"].as_str().unwrap(),
        e["upstream_sha256"].as_str().unwrap()
    );
    write(d, ".factory/sources.lock.yaml", &text);
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "title again"
        ])),
        0
    );
    let text = read(d, ".factory/sources.lock.yaml");
    assert!(
        text.contains("\n- patches:\n    - op: replace\n"),
        "{}",
        text
    );
    assert!(text.contains("\n  kind: openapi\n"), "{}", text);
    let e = entry(d);
    assert_eq!(e["kind"], "openapi");
    assert_eq!(e["patches"].as_array().unwrap().len(), 1);
    assert_eq!(e["patches"][0]["reason"], "title again");
}

#[test]
fn two_working_copies_never_share_one_upstream_and_pin_stays_inside_the_workspace() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        "vendor/a/openapi.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    write(
        d,
        "vendor/b/openapi.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    let pin_path = |p: &str| cmd::sources::main(&args(&["pin", d.to_str().unwrap(), "--path", p]));
    assert_eq!(pin_path("vendor/a/openapi.json"), 0);
    assert_eq!(pin_path("vendor/b/openapi.json"), 0);
    let lock = sources_lock(d);
    let upstreams: Vec<&str> = lock["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["upstream"].as_str())
        .collect();
    assert_eq!(
        upstreams,
        vec![
            ".factory/sources/vendor-a-openapi.upstream.json",
            ".factory/sources/vendor-b-openapi.upstream.json"
        ]
    );
    assert_eq!(
        graphos_factory_core::sources::default_upstream("openapi.json"),
        ".factory/sources/openapi.upstream.json",
        "a root-level document keeps the short name"
    );

    // A hand-written entry that claims another document's upstream is refused.
    let text = read(d, ".factory/sources.lock.yaml").replace(
        "upstream: .factory/sources/vendor-b-openapi.upstream.json",
        "upstream: .factory/sources/vendor-a-openapi.upstream.json",
    );
    write(d, ".factory/sources.lock.yaml", &text);
    assert_eq!(pin_path("vendor/b/openapi.json"), 2);

    // Outside the workspace: refused before anything is copied.
    write(
        d.parent().unwrap(),
        "outside.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    assert_eq!(pin_path("../outside.json"), 2);
    assert_eq!(pin_path("/etc/hostname"), 2);
    assert!(!d.join(".factory/sources/outside.upstream.json").exists());
}

#[test]
fn an_unreadable_working_copy_blocks_apply_and_is_named_as_such() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    assert_eq!(lock_check(d), 0);
    let mut broken = graphos_factory_core::json::pretty(&spec());
    broken.push_str("}} BROKEN");
    write(d, "openapi.json", &broken);
    assert_eq!(lock_check(d), 3);
    let found = lint(d);
    assert!(
        found.contains(&("error".to_string(), "source-unreadable".to_string())),
        "{:?}",
        found
    );
    assert!(
        !rules(d).contains(&"source-missing".to_string()),
        "the file exists; it is unreadable, not missing"
    );
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert!(report["sources"][0]["working_error"].is_string());
}

#[test]
fn a_missing_working_copy_or_upstream_blocks_apply_and_reconcile_says_which() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    assert_eq!(lock_check(d), 0);

    // The working copy is gone.
    let kept = read(d, "openapi.json");
    std::fs::remove_file(d.join("openapi.json")).unwrap();
    assert_eq!(lock_check(d), 3);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(report["sources"][0]["working_present"], false);
    assert!(rules(d).contains(&"source-missing".to_string()));
    write(d, "openapi.json", &kept);
    assert_eq!(lock_check(d), 0);

    // The upstream is gone: nothing about the spec can be checked any more.
    std::fs::remove_file(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    assert_eq!(lock_check(d), 3);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(report["sources"][0]["upstream_present"], false);
    assert!(rules(d).contains(&"source-upstream-missing".to_string()));
}

#[test]
fn patch_marks_find_components_whose_names_the_builder_sanitised() {
    let ws = workspace();
    let d = ws.path();
    // A component with a space in its name: the inventory calls it Widget_List.
    let mut s = spec();
    let list = s["components"]["schemas"]["WidgetList"].take();
    s["components"]["schemas"]
        .as_object_mut()
        .unwrap()
        .remove("WidgetList");
    s["components"]["schemas"]["Widget List"] = list;
    s["paths"]["/widgets"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] =
        json!({"$ref": "#/components/schemas/Widget List"});
    write(d, "openapi.json", &graphos_factory_core::json::pretty(&s));
    assert_eq!(pin(d), 0);

    let mut edited = s.clone();
    edited["components"]["schemas"]["Widget List"]["description"] = json!("the list envelope");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "documented"
        ])),
        0
    );
    assert_eq!(
        entry(d)["patches"][0]["path"],
        "/components/schemas/Widget List/description"
    );
    let out = d.join("inv.json");
    assert_eq!(
        cmd::inventory::main(&args(&[
            "build",
            d.join("openapi.json").to_str().unwrap(),
            "--out",
            out.to_str().unwrap()
        ])),
        0
    );
    let inv = graphos_factory_core::json::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert!(
        inv["shapes"]["Widget_List"].is_object(),
        "{}",
        inv["shapes"]
    );
    assert_eq!(
        inv["operations"][0]["patches"][0]["path"],
        "/components/schemas/Widget List/description"
    );
}

#[test]
fn an_empty_flow_list_is_pinned_into_and_a_non_empty_one_is_refused_with_the_fix() {
    let ws = workspace();
    let d = ws.path();
    // What an agent writes at init, before anything is pinned; no contract_version yet.
    write(d, ".factory/sources.lock.yaml", "sources: []\n");
    assert_eq!(pin(d), 0);
    let lock = sources_lock(d);
    assert_eq!(lock["contract_version"], 1, "added when missing");
    assert_eq!(lock["sources"][0]["path"], "openapi.json");
    assert!(read(d, ".factory/sources.lock.yaml")
        .starts_with("contract_version: 1\nsources:\n  - kind: openapi\n"));
    assert!(
        !rules(d).contains(&"contract".to_string()),
        "{:?}",
        rules(d)
    );

    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources: [{kind: docs, url: \"https://docs.widgets.test\"}]\n",
    );
    assert_eq!(pin(d), 2, "a non-empty flow list cannot be edited in place");
}

#[test]
fn pin_and_codify_source_keep_valid_writes_when_provenance_fails() {
    let ws = workspace();
    let d = ws.path();
    make_provenance_unreadable(d);

    assert_eq!(pin(d), 0);
    let applied =
        graphos_factory_core::yaml::parse(&read(d, ".factory/applied.lock.yaml")).unwrap();
    assert_eq!(
        applied["sources"]["openapi.json"],
        graphos_factory_core::patch::canonical_sha256(&spec())
    );
    assert!(
        applied.get("provenance").is_none(),
        "pin must remove stale provenance"
    );

    let mut edited = spec();
    edited["info"]["title"] = json!("A corrected title");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "correct the documented title"
        ])),
        0
    );
    let applied =
        graphos_factory_core::yaml::parse(&read(d, ".factory/applied.lock.yaml")).unwrap();
    assert_eq!(
        applied["sources"]["openapi.json"],
        graphos_factory_core::patch::canonical_sha256(&edited)
    );
    assert!(
        applied.get("provenance").is_none(),
        "codify --source must not leave stale provenance"
    );
    assert_eq!(lock_check(d), 0);
}

#[test]
fn force_replaces_the_baseline_only_with_a_reason_and_records_a_finding() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let vendor = std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    let mut edited = spec();
    edited["info"]["version"] = json!("2.0.0");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(lock_check(d), 3, "an uncodified edit");

    // Without a reason: refused, nothing changes.
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force"
        ])),
        1
    );
    assert_eq!(
        std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap(),
        vendor
    );
    assert_eq!(lock_check(d), 3);

    // A malformed --decision is a usage error.
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force",
            "--reason",
            "r",
            "--decision",
            "7"
        ])),
        1
    );
    // With a reason: the baseline moves and a finding says so (ADR 0113
    // §3), relating to the decision named; no decision is written.
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force",
            "--reason",
            "vendor published 2.0.0; re-fetched",
            "--decision",
            "D-0001"
        ])),
        0
    );
    assert_eq!(
        std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap(),
        std::fs::read(d.join("openapi.json")).unwrap()
    );
    assert_eq!(decision_ids(d), vec!["D-0001"]);
    let ids = finding_ids(d);
    assert_eq!(ids.len(), 1, "{}", all_prose(d));
    let fid = ids[0].clone();
    assert!(graphos_factory_core::record_log::is_random(&fid), "{}", fid);
    let f = graphos_factory_core::findings::find(&findings_doc(d), &fid)
        .cloned()
        .unwrap();
    assert_eq!(f["source"], "sources");
    assert_eq!(f["related"], json!(["D-0001"]));
    let decisions = prose(d, &fid);
    assert!(
        decisions.contains("Upstream replaced: openapi.json"),
        "{}",
        decisions
    );
    assert!(
        decisions.contains("vendor published 2.0.0; re-fetched"),
        "{}",
        decisions
    );
    assert_eq!(lock_check(d), 0);
    assert!(
        !rules(d).iter().any(|r| r.starts_with("source-")),
        "{:?}",
        rules(d)
    );
}

/// `pin --force` and `refresh` take `--decision` with a random id, a
/// decision added since ADR 0118, as they take a numbered one: the
/// decision is added, cited, and the finding each records relates to it.
#[test]
fn pin_force_and_refresh_cite_a_random_decision_id() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(
        cmd::decisions::main(&args(&[
            "add",
            d.to_str().unwrap(),
            "--title",
            "Follow the vendor's 2.0.0",
            "--question",
            "Move to 2.0.0?",
        ])),
        0
    );
    let log = graphos_factory_core::decisions::load(d, None).unwrap();
    let decision = graphos_factory_core::json::get_arr(&log, "decisions")
        .into_iter()
        .flatten()
        .filter_map(|r| r["id"].as_str())
        .find(|id| graphos_factory_core::record_log::is_random(id))
        .expect("the added decision has a random id")
        .to_string();
    assert_eq!(pin(d), 0);
    let mut edited = spec();
    edited["info"]["version"] = json!("2.0.0");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force",
            "--reason",
            "vendor published 2.0.0; re-fetched",
            "--decision",
            &decision,
        ])),
        0
    );
    let related = |d: &Path| -> Vec<Value> {
        findings_doc(d)["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["related"].clone())
            .collect()
    };
    assert_eq!(related(d), vec![json!([decision])]);

    edited["info"]["version"] = json!("2.1.0");
    write(
        d,
        "vendor-2.1.0.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "refresh",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--from",
            d.join("vendor-2.1.0.json").to_str().unwrap(),
            "--reason",
            "vendor published 2.1.0",
            "--decision",
            &decision,
        ])),
        0
    );
    assert_eq!(related(d), vec![json!([decision]), json!([decision])]);
}

#[test]
fn a_plain_re_pin_refuses_a_modified_upstream() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let vendor = std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    let recorded = entry(d)["upstream_sha256"].as_str().unwrap().to_string();

    // The "wrong fix": editing the vendor copy to match an edited working copy.
    let mut edited = spec();
    edited["info"]["title"] = json!("Laundered");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    write(
        d,
        ".factory/sources/openapi.upstream.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert!(rules(d).contains(&"source-upstream-modified".to_string()));
    assert_eq!(pin(d), 2, "re-pinning never re-blesses a modified upstream");
    assert_eq!(
        entry(d)["upstream_sha256"],
        recorded,
        "the record is untouched"
    );
    assert!(rules(d).contains(&"source-upstream-modified".to_string()));
    assert_eq!(lock_check(d), 3);

    // Restored, the plain re-pin works again.
    std::fs::write(d.join(".factory/sources/openapi.upstream.json"), &vendor).unwrap();
    assert_eq!(pin(d), 0);
    assert_eq!(entry(d)["upstream_sha256"], recorded);
}

#[test]
fn patch_marks_for_a_swagger_document_map_definitions_onto_shapes() {
    let ws = workspace();
    let d = ws.path();
    // Two definitions that sanitise to the same shape name: the builder calls
    // them Error_Object and Error_Object2, in first-use order.
    let swagger = json!({
        "swagger": "2.0",
        "info": {"title": "Errors", "version": "1"},
        "host": "api.errors.test",
        "schemes": ["https"],
        "definitions": {
            "Error Object": {"type": "object", "properties": {"a": {"type": "string"}}},
            "Error_Object": {"type": "object", "properties": {"b": {"type": "string"}}}
        },
        "paths": {
            "/a": {"get": {"operationId": "getA", "produces": ["application/json"],
                "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Error Object"}}}}},
            "/b": {"get": {"operationId": "getB", "produces": ["application/json"],
                "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Error_Object"}}}}}
        }
    });
    write(
        d,
        "swagger.json",
        &graphos_factory_core::json::pretty(&swagger),
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "swagger.json"
        ])),
        0
    );
    let mut edited = swagger.clone();
    edited["definitions"]["Error_Object"]["properties"]["z"] = json!({"type": "integer"});
    write(
        d,
        "swagger.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "swagger.json",
            "--reason",
            "the API also returns z"
        ])),
        0
    );
    let out = d.join("inv.json");
    assert_eq!(
        cmd::inventory::main(&args(&[
            "build",
            d.join("swagger.json").to_str().unwrap(),
            "--out",
            out.to_str().unwrap()
        ])),
        0
    );
    let inv = graphos_factory_core::json::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let op = |key: &str| -> Value {
        inv["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["key"] == key)
            .cloned()
            .unwrap()
    };
    assert_eq!(
        op("get:/a")["response"]["shape_ref"],
        "#/shapes/Error_Object"
    );
    assert_eq!(
        op("get:/b")["response"]["shape_ref"],
        "#/shapes/Error_Object2"
    );
    assert!(op("get:/a").get("patches").is_none(), "{}", op("get:/a"));
    assert_eq!(
        op("get:/b")["patches"][0]["path"],
        "/definitions/Error_Object/properties/z"
    );
}

#[test]
fn a_document_marker_comments_and_a_commented_sources_key_survive_pin() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/sources.lock.yaml",
        "---\n# what this service was built from\nsources:  # inputs\n  - kind: docs\n    url: https://docs.widgets.test/reference\n",
    );
    assert_eq!(pin(d), 0);
    let text = read(d, ".factory/sources.lock.yaml");
    assert!(
        text.starts_with("---\n# what this service was built from\ncontract_version: 1\nsources:  # inputs\n  - kind: docs\n"),
        "{}",
        text
    );
    let lock = sources_lock(d);
    assert_eq!(lock["contract_version"], 1);
    assert_eq!(lock["sources"].as_array().unwrap().len(), 2);
    assert_eq!(lock["sources"][1]["path"], "openapi.json");
    // And codify finds the entry.
    let mut edited = spec();
    edited["info"]["title"] = json!("x");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "t"
        ])),
        0
    );
    assert_eq!(entry(d)["patches"].as_array().unwrap().len(), 1);
}

#[test]
fn deleting_the_upstream_does_not_let_a_plain_pin_re_baseline() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let recorded = entry(d)["upstream_sha256"].as_str().unwrap().to_string();
    let mut edited = spec();
    edited["info"]["title"] = json!("Laundered");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    std::fs::remove_file(d.join(".factory/sources/openapi.upstream.json")).unwrap();

    assert_eq!(
        pin(d),
        2,
        "a recorded baseline is never re-created from the working copy"
    );
    assert!(!d.join(".factory/sources/openapi.upstream.json").exists());
    assert_eq!(entry(d)["upstream_sha256"], recorded);
    assert_eq!(lock_check(d), 3);
    assert!(rules(d).contains(&"source-upstream-missing".to_string()));

    // On purpose, with a reason: recreated and recorded as a decision.
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force",
            "--reason",
            "vendor copy lost; re-fetched"
        ])),
        0
    );
    assert!(d.join(".factory/sources/openapi.upstream.json").exists());
    assert_ne!(entry(d)["upstream_sha256"], recorded);
    assert!(all_prose(d).contains("Upstream replaced: openapi.json"));
    assert_eq!(lock_check(d), 0);
}

#[test]
fn an_entry_removed_from_the_lock_is_reported_as_unpinned_everywhere() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    assert_eq!(lock_check(d), 0);
    // Someone deletes the document entry; the applied lock still acknowledges the file.
    write(d, ".factory/sources.lock.yaml", SOURCES_LOCK);
    let mut edited = spec();
    edited["info"]["title"] = json!("unwatched");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert!(
        rules(d).contains(&"source-unpinned".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(lock_check(d), 3);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(report["sources_unpinned"], json!(["openapi.json"]));
    // Unpinning on purpose: `lock` rebuilds the acknowledgements from the entries that exist.
    assert_eq!(cmd::lock::main(&args(&[d.to_str().unwrap()])), 0);
    assert!(!rules(d).contains(&"source-unpinned".to_string()));
    assert_eq!(lock_check(d), 0);
}

#[test]
fn a_hand_written_dot_slash_path_is_the_same_entry() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    url: https://api.widgets.test/openapi.json\n    path: ./openapi.json\n",
    );
    assert_eq!(pin(d), 0);
    let lock = sources_lock(d);
    assert_eq!(lock["sources"].as_array().unwrap().len(), 1, "{}", lock);
    assert_eq!(lock["sources"][0]["path"], "openapi.json");
    assert_eq!(
        lock["sources"][0]["url"],
        "https://api.widgets.test/openapi.json"
    );
    assert_eq!(lock_check(d), 0);
}

#[test]
fn a_stray_definitions_section_in_an_openapi_3_document_marks_nothing() {
    let ws = workspace();
    let d = ws.path();
    let mut s = spec();
    s["definitions"] = json!({"Widget": {"type": "object"}});
    write(d, "openapi.json", &graphos_factory_core::json::pretty(&s));
    assert_eq!(pin(d), 0);
    let mut edited = s.clone();
    edited["definitions"]["Widget"]["properties"] = json!({"legacy": {"type": "string"}});
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "leftover section"
        ])),
        0
    );
    let out = d.join("inv.json");
    assert_eq!(
        cmd::inventory::main(&args(&[
            "build",
            d.join("openapi.json").to_str().unwrap(),
            "--out",
            out.to_str().unwrap()
        ])),
        0
    );
    let inv = graphos_factory_core::json::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    for op in inv["operations"].as_array().unwrap() {
        assert!(op.get("patches").is_none(), "{}", op);
    }
}

#[test]
fn a_refused_pin_leaves_the_vendor_copy_and_the_lock_untouched() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let vendor = std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    let lock_before = read(d, ".factory/sources.lock.yaml");
    // A lock the editor refuses (non-empty flow list), an edited working copy,
    // and a forced replacement with a reason: the refusal must happen before
    // any byte on disk changes.
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources: [{kind: openapi, path: openapi.json}]\n",
    );
    let mut edited = spec();
    edited["info"]["title"] = json!("EDITED");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force",
            "--reason",
            "re-fetch"
        ])),
        2
    );
    assert_eq!(
        std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap(),
        vendor,
        "the vendor copy survives a refused pin"
    );
    assert!(!all_prose(d).contains("Upstream replaced"));
    // And a first pin that is refused creates no upstream at all.
    std::fs::remove_file(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json"
        ])),
        2
    );
    assert!(!d.join(".factory/sources/openapi.upstream.json").exists());
    write(d, ".factory/sources.lock.yaml", &lock_before);
    std::fs::write(d.join(".factory/sources/openapi.upstream.json"), &vendor).unwrap();
}

#[test]
fn a_key_right_after_path_does_not_swallow_the_inserted_keys() {
    let ws = workspace();
    let d = ws.path();
    // The migration layout: no upstream keys yet, `version:` directly after `path:`.
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n    version: \"3.0.3\"\n",
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--url",
            "https://api.widgets.test/openapi.json"
        ])),
        0
    );
    let e = entry(d);
    assert_eq!(e["version"], "3.0.3");
    assert_eq!(e["url"], "https://api.widgets.test/openapi.json");
    assert_eq!(e["upstream"], ".factory/sources/openapi.upstream.json");
    assert!(e["upstream_sha256"].is_string());
    assert_eq!(sources_lock(d)["sources"].as_array().unwrap().len(), 1);
}

#[test]
fn deleting_the_whole_sources_lock_is_unpinned_for_lint_too() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    std::fs::remove_file(d.join(".factory/sources.lock.yaml")).unwrap();
    assert!(
        rules(d).contains(&"source-unpinned".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(lock_check(d), 3);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(
        cmd::sources::main(&args(&["status", d.to_str().unwrap()])),
        0
    );
}

#[test]
fn an_upstream_recorded_outside_the_workspace_is_refused_and_linted() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n    upstream: ../../escaped.upstream.json\n    upstream_sha256: 0000000000000000000000000000000000000000000000000000000000000000\n",
    );
    assert_eq!(pin(d), 2);
    assert!(!d
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("escaped.upstream.json")
        .exists());
    assert!(
        rules(d).contains(&"source-outside-workspace".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "x"
        ])),
        2
    );
    // The gate agrees with lint, and nothing is read or written through the entry.
    assert_eq!(lock_check(d), 3);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(report["sources"][0]["outside_workspace"], true);
    assert!(
        report["sources"][0]["upstream_ok"].is_null(),
        "nothing is read through the entry"
    );
    let statuses = graphos_factory_core::sources::statuses(d, None).unwrap();
    let lines = graphos_factory_core::cmd::sources::render_lines(&statuses);
    assert_eq!(lines.len(), 1, "{:?}", lines);
    assert!(
        lines[0].contains("OUTSIDE THE WORKSPACE") && !lines[0].contains("missing"),
        "{}",
        lines[0]
    );
    let text = graphos_factory_core::reconcile::render_report(&report, d.to_str().unwrap(), None);
    assert!(
        !text.contains("NO UPSTREAM COPY") && !text.contains("run `graphos-factory-core lock`"),
        "{}",
        text
    );
    assert_eq!(cmd::lock::main(&args(&[d.to_str().unwrap()])), 0);
    let applied = graphos_factory_core::spans::read_lock(d).unwrap().unwrap();
    assert!(
        applied["sources"]
            .as_object()
            .map(|m| m.is_empty())
            .unwrap_or(true),
        "{}",
        applied["sources"]
    );
    // An escaping --source is refused before anything is looked at.
    write(
        d.parent().unwrap(),
        "outside.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "../outside.json",
            "--reason",
            "x"
        ])),
        2
    );
}

#[test]
fn the_kind_mismatch_names_the_version_the_document_declares() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        "swagger.json",
        &graphos_factory_core::json::pretty(&json!({
            "swagger": "2.0", "info": {"title": "x", "version": "1"}, "host": "h", "paths": {}
        })),
    );
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    version: \"3.0.2\"\n    path: swagger.json\n",
    );
    let found = lint_workspace(
        d,
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings;
    let m = found
        .iter()
        .find(|f| f.rule == "source-kind-mismatch")
        .map(|f| f.message.clone())
        .expect("a kind mismatch");
    assert!(m.contains("declares swagger 2.0"), "{}", m);
    assert!(!m.contains("3.0.2"), "{}", m);
    let s = graphos_factory_core::sources::statuses(d, None).unwrap();
    assert_eq!(s[0].version, "3.0.2");
    assert_eq!(s[0].detected_version, "2.0");
}

#[test]
fn the_uncodified_edit_remedy_does_not_offer_lock_when_the_patches_are_stale() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let mut edited = spec();
    edited["info"]["title"] = json!("changed");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    let found = lint_workspace(
        d,
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings;
    let m = found
        .iter()
        .find(|f| f.rule == "unacknowledged-source-edit")
        .map(|f| f.message.clone())
        .expect("an uncodified edit");
    assert!(m.contains("codify --source openapi.json"), "{}", m);
    assert!(!m.contains("graphos-factory-core lock"), "{}", m);
}

#[test]
fn renderers_offer_only_remedies_the_tool_accepts() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    // A recorded baseline whose file is gone: restore, never re-pin.
    std::fs::remove_file(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    let statuses = graphos_factory_core::sources::statuses(d, None).unwrap();
    assert!(statuses[0].upstream_recorded);
    let line = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(line.contains("restore it (git checkout --"), "{}", line);
    assert!(
        !line.contains("run `graphos-factory-core sources pin --path openapi.json`"),
        "{}",
        line
    );
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let text = graphos_factory_core::reconcile::render_report(&report, d.to_str().unwrap(), None);
    assert!(text.contains("NO UPSTREAM COPY — restore it"), "{}", text);
    assert!(
        !text.contains("run `graphos-factory-core sources pin --path openapi.json`"),
        "{}",
        text
    );

    // No baseline recorded yet (a hand-written entry): pinning is the remedy.
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n",
    );
    let statuses = graphos_factory_core::sources::statuses(d, None).unwrap();
    assert!(!statuses[0].upstream_recorded);
    let line = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(
        line.contains("run `graphos-factory-core sources pin --path openapi.json`"),
        "{}",
        line
    );

    // A kind mismatch is said by reconcile too, not "in sync".
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: swagger\n    path: openapi.json\n",
    );
    assert_eq!(pin(d), 0, "pin fixes the record");
    write(
        d,
        ".factory/sources.lock.yaml",
        &read(d, ".factory/sources.lock.yaml").replace("kind: openapi", "kind: swagger"),
    );
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let text = graphos_factory_core::reconcile::render_report(&report, d.to_str().unwrap(), None);
    assert!(
        text.contains("declared swagger but the document is openapi"),
        "{}",
        text
    );
    assert!(
        !text.contains("openapi.json (swagger 3.0.3) — 0 patches; in sync"),
        "{}",
        text
    );
}

#[test]
fn a_codify_that_changes_nothing_writes_no_decision_and_says_so() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let mut edited = spec();
    edited["info"]["title"] = json!("once");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    let codify = |reason: &str| {
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            reason,
        ]))
    };
    assert_eq!(codify("first"), 0);
    let decisions = read(d, ".factory/decisions.json");
    assert_eq!(codify("second, unused"), 0);
    assert_eq!(
        read(d, ".factory/decisions.json"),
        decisions,
        "nothing appended"
    );
    let e = entry(d);
    assert_eq!(e["patches"][0]["reason"], "first");
    assert_eq!(e["patches"][0]["decision"], Value::Null);
    assert_eq!(decision_ids(d), vec!["D-0001"], "codify writes no decision");
    assert_eq!(lock_check(d), 0);

    // A --decision nothing takes is not written anywhere either.
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "third, unused",
            "--decision",
            "D-0007"
        ])),
        0
    );
    assert_eq!(read(d, ".factory/decisions.json"), decisions);
    assert_eq!(entry(d)["patches"][0]["decision"], Value::Null);
    assert!(!read(d, ".factory/sources.lock.yaml").contains("D-0007"));

    // codify refuses a missing recorded upstream (and names the accepted remedy).
    std::fs::remove_file(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    assert_eq!(codify("x"), 2);
}

#[test]
fn a_partial_revert_drops_the_patch_and_writes_no_record() {
    // codify writes no decision and no finding for a source patch (ADR 0113
    // §3): the entry's `patches[]` is the whole record, and a dropped patch
    // simply leaves it.
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let mut edited = spec();
    edited["info"]["title"] = json!("kept");
    edited["info"]["description"] = json!("dropped later");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    let codify = |extra: &[&str]| {
        let mut a = vec![
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "r",
        ];
        a.extend_from_slice(extra);
        cmd::codify::main(&args(&a))
    };
    let decisions = read(d, ".factory/decisions.json");
    assert_eq!(codify(&[]), 0);
    assert_eq!(entry(d)["patches"].as_array().unwrap().len(), 2);

    // Revert one of the two.
    edited["info"]
        .as_object_mut()
        .unwrap()
        .remove("description");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(codify(&[]), 0);
    assert_eq!(entry(d)["patches"].as_array().unwrap().len(), 1);
    assert_eq!(entry(d)["patches"][0]["path"], "/info/title");

    // An expiry has no place on a patch.
    assert_eq!(codify(&["--until", "vendor fixes it"]), 1);
    assert_eq!(codify(&["--expires", "2027-01-01"]), 1);

    // Dropping the last one: the copies match again.
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    assert_eq!(codify(&[]), 0);
    assert_eq!(entry(d)["patches"], json!([]));
    assert_eq!(lock_check(d), 0);
    assert_eq!(read(d, ".factory/decisions.json"), decisions);
    assert!(!d.join(".factory/findings.json").exists());
    assert!(!d.join(".factory/findings").exists());
}

#[test]
fn remedies_never_name_codify_while_the_upstream_is_modified() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let mut edited = spec();
    edited["info"]["title"] = json!("edited");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    write(
        d,
        ".factory/sources/openapi.upstream.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    let found = lint_workspace(
        d,
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings;
    let modified = found.iter().any(|f| f.rule == "source-upstream-modified");
    assert!(modified);
    for f in found
        .iter()
        .filter(|f| f.rule == "unacknowledged-source-edit" || f.rule == "source-patches-stale")
    {
        assert!(f.message.contains("restore"), "{}", f.message);
        assert!(
            !f.message.starts_with(&format!(
                "{} changed since applied.lock.yaml — codify it",
                "openapi.json"
            )),
            "{}",
            f.message
        );
    }
    let statuses = graphos_factory_core::sources::statuses(
        d,
        graphos_factory_core::spans::read_lock(d).unwrap().as_ref(),
    )
    .unwrap();
    let line = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(!line.contains("codify --source"), "{}", line);
    assert!(line.contains("UPSTREAM MODIFIED"), "{}", line);

    // The same with a missing (not modified) vendor copy.
    std::fs::remove_file(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    let applied = graphos_factory_core::spans::read_lock(d).unwrap();
    let statuses = graphos_factory_core::sources::statuses(d, applied.as_ref()).unwrap();
    assert!(statuses[0].hand_edit && !statuses[0].upstream_present);
    let line = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(!line.contains("codify --source"), "{}", line);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let text = graphos_factory_core::reconcile::render_report(&report, d.to_str().unwrap(), None);
    assert!(!text.contains("codify --source"), "{}", text);
    assert!(text.contains("restore the vendor copy first"), "{}", text);
}

#[test]
fn a_vendor_copy_lives_only_under_factory_sources_so_force_cannot_overwrite_the_schema() {
    let ws = workspace();
    let d = ws.path();
    let schema = read(d, "widget-co.graphql");
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n    upstream: widget-co.graphql\n",
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force",
            "--reason",
            "x"
        ])),
        2
    );
    assert_eq!(
        read(d, "widget-co.graphql"),
        schema,
        "the schema is untouched"
    );
    assert!(
        rules(d).contains(&"source-upstream-misplaced".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(lock_check(d), 3);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(report["sources"][0]["upstream_misplaced"], true);
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "x"
        ])),
        2
    );
    // A nested path under .factory/sources/ is refused too; the default is accepted.
    assert!(!graphos_factory_core::sources::valid_upstream(
        ".factory/sources/deep/x.json"
    ));
    assert!(!graphos_factory_core::sources::valid_upstream(
        ".factory/sources/"
    ));
    assert!(graphos_factory_core::sources::valid_upstream(
        ".factory/sources/openapi.upstream.json"
    ));
}

#[test]
fn an_unreadable_upstream_is_never_blessed_and_blocks_apply() {
    let ws = workspace();
    let d = ws.path();
    // Migration path: a hand-written entry and a stray, truncated file at the default upstream path.
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n",
    );
    write(
        d,
        ".factory/sources/openapi.upstream.json",
        "truncated {\"openapi\"\n",
    );
    assert_eq!(pin(d), 2, "a kept upstream must parse");

    // A pinned document whose upstream is later corrupted with a matching hash recorded.
    std::fs::remove_file(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    assert_eq!(pin(d), 0);
    write(
        d,
        ".factory/sources/openapi.upstream.json",
        "truncated {\"openapi\"\n",
    );
    let bytes = std::fs::read(d.join(".factory/sources/openapi.upstream.json")).unwrap();
    let sha = graphos_factory_core::patch::bytes_sha256(&bytes);
    let text = read(d, ".factory/sources.lock.yaml");
    let recorded = entry(d)["upstream_sha256"].as_str().unwrap().to_string();
    write(
        d,
        ".factory/sources.lock.yaml",
        &text.replace(&recorded, &sha),
    );
    assert!(
        rules(d).contains(&"source-upstream-unreadable".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(lock_check(d), 3);
    let statuses = graphos_factory_core::sources::statuses(d, None).unwrap();
    assert!(statuses[0].upstream_error.is_some());
    let line = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(line.contains("UPSTREAM UNREADABLE"), "{}", line);
    assert!(!line.contains("in sync"), "{}", line);
}

/// A symlinked vendor copy is the case ADR 0025 exists to refuse, and the
/// refusal has to survive the *status* read: `read_optional` returns the
/// `SymlinkRefused` error, and collapsing it into `None` used to report
/// "no upstream copy — restore it (git checkout -- …)" for a file that is
/// right there. "Not allowed" is never downgraded to "not there".
#[cfg(unix)]
#[test]
fn a_symlinked_upstream_is_reported_as_refused_not_as_missing() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n",
    );
    assert_eq!(pin(d), 0);
    // The link points at a spec that reads perfectly: only the shape of the
    // path is refused, so nothing here can pass by being unparseable.
    let outside = ws.path().parent().unwrap().join("elsewhere.json");
    std::fs::write(&outside, graphos_factory_core::json::pretty(&spec())).unwrap();
    let upstream = d.join(".factory/sources/openapi.upstream.json");
    std::fs::remove_file(&upstream).unwrap();
    std::os::unix::fs::symlink(&outside, &upstream).unwrap();

    let statuses = graphos_factory_core::sources::statuses(d, None).unwrap();
    let s = &statuses[0];
    assert!(
        s.upstream_present,
        "a refused file is present, not absent: {:?}",
        s
    );
    let e = s
        .upstream_error
        .as_deref()
        .expect("the refusal must reach the caller, not be swallowed by .ok()");
    assert!(
        e.contains(".factory/sources/openapi.upstream.json") && e.contains("refused"),
        "{}",
        e
    );
    // The link's target never appears in the report (ADR 0025).
    assert!(!e.contains("elsewhere.json"), "{}", e);

    let line = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(line.contains("UPSTREAM UNREADABLE"), "{}", line);
    assert!(
        !line.contains("no upstream copy"),
        "a refusal is not a missing file: {}",
        line
    );
    assert!(
        rules(d).contains(&"source-upstream-unreadable".to_string()),
        "{:?}",
        rules(d)
    );
    assert!(
        !rules(d).contains(&"source-upstream-missing".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(lock_check(d), 3);
    // `pin` refused already — but as "is missing", offering `git checkout --`
    // for a file that is right there. The exit code cannot tell the two
    // apart, so the message is the assertion: run the binary and read it.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "sources",
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("refused") && err.contains("symlink"),
        "pin must name the refusal, not call it missing: {}",
        err
    );
    assert!(
        !err.contains("is missing"),
        "a refused vendor copy is not a missing one: {}",
        err
    );
    assert!(!err.contains("elsewhere.json"), "{}", err);
    // And the link itself is untouched, as is what it points at.
    assert!(std::fs::symlink_metadata(&upstream).unwrap().is_symlink());
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        graphos_factory_core::json::pretty(&spec()),
        "the file outside the workspace is never written through"
    );
}

#[test]
fn a_hand_edit_with_no_upstream_yet_is_told_to_pin_not_restore() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n",
    );
    // Acknowledge, then edit: a hand edit against an entry that was never pinned.
    assert_eq!(cmd::lock::main(&args(&[d.to_str().unwrap()])), 0);
    let mut edited = spec();
    edited["info"]["title"] = json!("edited");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    let applied = graphos_factory_core::spans::read_lock(d).unwrap();
    let statuses = graphos_factory_core::sources::statuses(d, applied.as_ref()).unwrap();
    assert!(
        statuses[0].hand_edit && !statuses[0].upstream_present && !statuses[0].upstream_recorded
    );
    let line = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(line.contains("pin it first"), "{}", line);
    assert!(!line.contains("restore the vendor copy"), "{}", line);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let text = graphos_factory_core::reconcile::render_report(&report, d.to_str().unwrap(), None);
    assert!(text.contains("pin it first"), "{}", text);
    assert!(!text.contains("restore the vendor copy"), "{}", text);
    assert_eq!(pin(d), 0, "and pinning is accepted");
}

#[test]
fn factory_sources_is_reserved_for_vendor_copies() {
    let ws = workspace();
    let d = ws.path();
    // A working copy hand-placed under .factory/sources/ while another entry names it as upstream.
    write(
        d,
        ".factory/sources/vendor.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    let before = read(d, ".factory/sources/vendor.json");
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: .factory/sources/vendor.json\n  - kind: openapi\n    path: openapi.json\n    upstream: .factory/sources/vendor.json\n",
    );
    let mut edited = spec();
    edited["info"]["title"] = json!("clobber");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "openapi.json",
            "--force",
            "--reason",
            "x"
        ])),
        2
    );
    assert_eq!(
        read(d, ".factory/sources/vendor.json"),
        before,
        "the other working copy is untouched"
    );
    let r = rules(d);
    assert_eq!(
        r.iter()
            .filter(|x| *x == "source-upstream-misplaced")
            .count(),
        2,
        "both entries are misplaced: {:?}",
        r
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            ".factory/sources/vendor.json"
        ])),
        2,
        "a working copy is never pinned from the reserved directory"
    );
    assert_eq!(lock_check(d), 3);

    // `./` on upstream is the same file, not a misplacement.
    write(d, ".factory/sources.lock.yaml", SOURCES_LOCK);
    std::fs::remove_file(d.join(".factory/sources/vendor.json")).unwrap();
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    assert_eq!(pin(d), 0);
    let text = read(d, ".factory/sources.lock.yaml").replace(
        "upstream: .factory/sources/openapi.upstream.json",
        "upstream: ./.factory/sources/openapi.upstream.json",
    );
    write(d, ".factory/sources.lock.yaml", &text);
    assert!(
        !rules(d).contains(&"source-upstream-misplaced".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(lock_check(d), 0);
    assert_eq!(pin(d), 0);
    assert_eq!(
        entry(d)["upstream"],
        ".factory/sources/openapi.upstream.json",
        "normalised on write"
    );
}

#[test]
fn every_surface_refuses_an_entry_it_does_not_follow_and_says_why() {
    let ws = workspace();
    let d = ws.path();
    // Two working copies whose default vendor copies collide.
    write(
        d,
        "a/openapi.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    let mut other = spec();
    other["info"]["title"] = json!("Different document B");
    write(
        d,
        "a-openapi.json",
        &graphos_factory_core::json::pretty(&other),
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "a/openapi.json"
        ])),
        0
    );
    let vendor = std::fs::read(d.join(".factory/sources/a-openapi.upstream.json")).unwrap();
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "a-openapi.json"
        ])),
        2,
        "the default vendor copy is already a/openapi.json's"
    );
    assert_eq!(
        std::fs::read(d.join(".factory/sources/a-openapi.upstream.json")).unwrap(),
        vendor
    );
    // Hand-written: both entries without `upstream:` resolve to one file.
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: a/openapi.json\n  - kind: openapi\n    path: a-openapi.json\n",
    );
    let statuses = graphos_factory_core::sources::statuses(d, None).unwrap();
    assert!(
        statuses.iter().all(|s| s.upstream_misplaced),
        "{:?}",
        statuses
    );
    assert!(
        statuses[0]
            .misplaced_reason
            .as_deref()
            .unwrap()
            .contains("same vendor copy as a-openapi.json"),
        "{:?}",
        statuses[0].misplaced_reason
    );
    let r = rules(d);
    assert_eq!(
        r.iter()
            .filter(|x| *x == "source-upstream-misplaced")
            .count(),
        2,
        "{:?}",
        r
    );
    assert_eq!(
        cmd::sources::main(&args(&[
            "pin",
            d.to_str().unwrap(),
            "--path",
            "a-openapi.json",
            "--force",
            "--reason",
            "x"
        ])),
        2
    );
    assert_eq!(
        std::fs::read(d.join(".factory/sources/a-openapi.upstream.json")).unwrap(),
        vendor,
        "--force never wrote through the shared name"
    );

    // An alias: entry B's upstream is entry A's working copy. codify and lock refuse to follow it.
    write(
        d,
        ".factory/sources/vendor.json",
        &graphos_factory_core::json::pretty(&spec()),
    );
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: .factory/sources/vendor.json\n  - kind: openapi\n    path: openapi.json\n    upstream: .factory/sources/vendor.json\n",
    );
    let mut edited = spec();
    edited["info"]["title"] = json!("edited");
    write(
        d,
        "openapi.json",
        &graphos_factory_core::json::pretty(&edited),
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            "openapi.json",
            "--reason",
            "x"
        ])),
        2
    );
    assert_eq!(
        cmd::codify::main(&args(&[
            d.to_str().unwrap(),
            "--source",
            ".factory/sources/vendor.json",
            "--reason",
            "x"
        ])),
        2
    );
    assert!(!all_prose(d).contains("Source patch"));
    assert_eq!(cmd::lock::main(&args(&[d.to_str().unwrap()])), 0);
    let applied = graphos_factory_core::spans::read_lock(d).unwrap().unwrap();
    assert!(
        applied["sources"]
            .as_object()
            .map(|m| m.is_empty())
            .unwrap_or(true),
        "nothing acknowledged through a faulted entry: {}",
        applied["sources"]
    );
    let statuses = graphos_factory_core::sources::statuses(d, Some(&applied)).unwrap();
    let lines = graphos_factory_core::cmd::sources::render_lines(&statuses).join("\n");
    assert!(
        lines.contains("its `path` is under .factory/sources/"),
        "{}",
        lines
    );
    assert!(
        lines.contains("is the working copy of .factory/sources/vendor.json"),
        "{}",
        lines
    );
    assert!(!lines.contains("is not a file directly under"), "{}", lines);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    let text = graphos_factory_core::reconcile::render_report(&report, d.to_str().unwrap(), None);
    assert!(
        text.contains("is the working copy of .factory/sources/vendor.json"),
        "{}",
        text
    );
    assert!(
        !rules(d).contains(&"source-unpinned".to_string()),
        "{:?}",
        rules(d)
    );
}

#[test]
fn a_same_file_entry_and_a_duplicate_path_are_named_and_not_followed() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: .factory/sources/x.json\n    upstream: .factory/sources/x.json\n  - kind: openapi\n    path: openapi.json\n  - kind: openapi\n    path: openapi.json\n    upstream: .factory/sources/other.upstream.json\n",
    );
    let statuses = graphos_factory_core::sources::statuses(d, None).unwrap();
    assert!(statuses[0]
        .misplaced_reason
        .as_deref()
        .unwrap()
        .contains("name one file"));
    assert!(statuses[1]
        .misplaced_reason
        .as_deref()
        .unwrap()
        .contains("same `path`"));
    assert!(statuses[2]
        .misplaced_reason
        .as_deref()
        .unwrap()
        .contains("same `path`"));
    let r = rules(d);
    assert_eq!(
        r.iter().filter(|x| *x == "source-duplicate-path").count(),
        2,
        "{:?}",
        r
    );
    assert_eq!(
        r.iter()
            .filter(|x| *x == "source-upstream-misplaced")
            .count(),
        1,
        "{:?}",
        r
    );
    assert_eq!(lock_check(d), 3);
    // pin refuses too, and writes nothing.
    assert_eq!(pin(d), 2);
    assert!(!d.join(".factory/sources/openapi.upstream.json").exists());
    // inventory build marks nothing from an entry it does not follow.
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources:\n  - kind: openapi\n    path: openapi.json\n    upstream: .factory/sources/shared.upstream.json\n    patches:\n      - op: replace\n        path: /info/title\n        value: x\n        reason: r\n  - kind: openapi\n    path: other.json\n    upstream: .factory/sources/shared.upstream.json\n",
    );
    let out = d.join("inv.json");
    assert_eq!(
        cmd::inventory::main(&args(&[
            "build",
            d.join("openapi.json").to_str().unwrap(),
            "--out",
            out.to_str().unwrap()
        ])),
        0
    );
    let inv = graphos_factory_core::json::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
    for op in inv["operations"].as_array().unwrap() {
        assert!(op.get("patches").is_none(), "{}", op);
    }
}

#[test]
fn a_pinned_document_the_applied_lock_forgot_is_source_unlocked() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    assert_eq!(lock_check(d), 0);
    // Strip the `sources:` map from the applied lock: the document is pinned but unacknowledged.
    let mut applied = graphos_factory_core::spans::read_lock(d).unwrap().unwrap();
    applied.as_object_mut().unwrap().remove("sources");
    graphos_factory_core::spans::write_lock(d, &applied).unwrap();
    let statuses = graphos_factory_core::sources::statuses(d, Some(&applied)).unwrap();
    assert!(statuses[0].unlocked());
    assert!(
        rules(d).contains(&"source-unlocked".to_string()),
        "{:?}",
        rules(d)
    );
    assert_eq!(lock_check(d), 3);
    let report = graphos_factory_core::reconcile::reconcile_workspace(d, None).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(report["sources"][0]["locked"], false);
    // `lock` acknowledges it again.
    assert_eq!(cmd::lock::main(&args(&[d.to_str().unwrap()])), 0);
    assert!(!rules(d).contains(&"source-unlocked".to_string()));
    assert_eq!(lock_check(d), 0);
}

/// Run the binary's `sources status` on `dir` and return (exit, stdout).
fn status_output(dir: &Path, json: bool) -> (Option<i32>, String) {
    let mut argv = vec!["sources", "status", dir.to_str().unwrap()];
    if json {
        argv.push("--json");
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(&argv)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

/// A pinned document stays under `sources` with its integrity facts; every
/// other entry — docs, probe, a kind this binary does not model, and a
/// document entry with no `path` — is under `recorded`. Together the two
/// lists are every entry of the lock.
#[test]
fn status_json_splits_pinned_documents_from_recorded_entries() {
    let ws = workspace();
    let d = ws.path();
    assert_eq!(pin(d), 0);
    let mut lock = read(d, ".factory/sources.lock.yaml");
    lock.push_str("  - kind: probe\n    base_url: https://api.widgets.test\n    retrieved_at: 2026-09-09T10:00:00Z\n    operations: [\"get:/widgets\"]\n    credential: WIDGET_TOKEN\n  - kind: har\n    url: https://example.test/capture.har\n  - kind: openapi\n    url: https://api.widgets.test/openapi.json\n");
    write(d, ".factory/sources.lock.yaml", &lock);

    let (code, stdout) = status_output(d, true);
    assert_eq!(code, Some(0), "{}", stdout);
    let v: Value = serde_json::from_str(&stdout).unwrap();
    let pinned = v["sources"].as_array().unwrap();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0]["path"], "openapi.json");
    assert_eq!(pinned[0]["kind"], "openapi");

    let recorded = v["recorded"].as_array().unwrap();
    let kinds: Vec<&str> = recorded
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["docs", "probe", "har", "openapi"], "{}", stdout);
    // The docs entry's folded note survives as one string.
    assert_eq!(
        recorded[0]["note"],
        "Read for the pagination convention the spec does not document; nothing else."
    );
    assert_eq!(recorded[0]["operations"], json!(["get:/widgets"]));
    assert_eq!(recorded[1]["url"], "https://api.widgets.test");
    assert_eq!(recorded[1]["credential"], "WIDGET_TOKEN");
    assert_eq!(recorded[2]["operations"], json!([]));
    assert_eq!(recorded[3]["url"], "https://api.widgets.test/openapi.json");
}

/// A docs `url` is written by hand. What it may carry that must not leave the
/// lock — userinfo, a credential-named query value — is not reported; the
/// rest of the URL, and a URL with neither, is reported exactly as recorded.
#[test]
fn status_json_never_reports_a_credential_carried_in_a_recorded_url() {
    let ws = workspace();
    let d = ws.path();
    let mut lock = read(d, ".factory/sources.lock.yaml");
    lock.push_str("  - kind: docs\n    url: \"https://reader:hunter2@docs.widgets.test/ref?page=2&api_key=sk-live-123&Key=AIza456\"\n  - kind: docs\n    url: \"https://docs.widgets.test/ref?page=2&sort=name\"\n");
    write(d, ".factory/sources.lock.yaml", &lock);

    let (code, stdout) = status_output(d, true);
    assert_eq!(code, Some(0), "{}", stdout);
    for secret in ["reader", "hunter2", "sk-live-123", "AIza456"] {
        assert!(!stdout.contains(secret), "{} reported: {}", secret, stdout);
    }
    let v: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        v["recorded"][1]["url"],
        "https://docs.widgets.test/ref?page=2&api_key=REDACTED&Key=REDACTED"
    );
    assert_eq!(
        v["recorded"][2]["url"], "https://docs.widgets.test/ref?page=2&sort=name",
        "a URL with nothing to hide is reported as recorded"
    );
    let (code, text) = status_output(d, false);
    assert_eq!(code, Some(0));
    assert!(
        !text.contains("hunter2") && !text.contains("sk-live-123"),
        "{}",
        text
    );
}

/// An unreadable lock is a failure, never an empty report: a reader that got
/// `{"sources": [], "recorded": []}` would say the workspace has no sources.
#[test]
fn status_refuses_an_unreadable_lock_rather_than_reporting_no_sources() {
    let ws = workspace();
    let d = ws.path();
    write(
        d,
        ".factory/sources.lock.yaml",
        "contract_version: 1\nsources: [\n",
    );
    let (code, stdout) = status_output(d, true);
    assert_eq!(code, Some(2), "{}", stdout);
    assert!(stdout.is_empty(), "{}", stdout);
}

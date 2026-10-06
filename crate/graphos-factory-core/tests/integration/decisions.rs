use serde_json::json;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// Run `graphos-factory-core decisions <sub> <workspace> …` in-process and return the
/// exit code. The workspace is always the first positional after the subcommand.
fn decisions(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = Vec::with_capacity(args.len() + 1);
    argv.push(args[0].to_string());
    argv.push(dir.to_string_lossy().to_string());
    for a in &args[1..] {
        argv.push(a.to_string());
    }
    graphos_factory_core::cmd::decisions::main(&argv)
}

/// The log as every reader sees it: `decisions.json`'s records, then those
/// added since ADR 0118, one file each under `.factory/decisions/`.
fn read_doc(dir: &Path) -> Value {
    graphos_factory_core::decisions::load(dir, None).unwrap()
}

/// The id `add` gave the record with this title: a new record's id is
/// random (ADR 0118), so a test finds it by what it decided.
fn id_of(dir: &Path, title: &str) -> String {
    records(&read_doc(dir))
        .iter()
        .find(|r| r["title"] == title)
        .unwrap_or_else(|| panic!("no decision titled {:?}", title))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn by_title<'a>(doc: &'a Value, title: &str) -> &'a Value {
    records(doc)
        .iter()
        .find(|r| r["title"] == title)
        .unwrap_or_else(|| panic!("no decision titled {:?}", title))
}

/// A refused verb recorded nothing: no `decisions.json`, no record file.
fn assert_nothing_recorded(dir: &Path) {
    assert!(!dir.join(".factory/decisions.json").exists());
    assert!(!dir.join(".factory/decisions").exists());
}

/// Turn the records `add` wrote into an existing workspace's log: the same
/// records, in `titles` order, numbered D-0001… in `decisions.json`, as a
/// workspace written before ADR 0118 holds them. The record files go.
fn as_legacy(dir: &Path, titles: &[&str]) {
    let doc = read_doc(dir);
    let recs: Vec<Value> = titles
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut r = by_title(&doc, t).clone();
            let m = r.as_object_mut().unwrap();
            m.shift_remove("slug");
            m.insert("id".into(), Value::from(format!("D-{:04}", i + 1)));
            r
        })
        .collect();
    std::fs::remove_dir_all(dir.join(".factory/decisions")).unwrap();
    std::fs::write(
        dir.join(".factory/decisions.json"),
        graphos_factory_core::json::pretty(&json!({"contract_version": 1, "decisions": recs})),
    )
    .unwrap();
}

fn records(doc: &Value) -> &Vec<Value> {
    doc.get("decisions").unwrap().as_array().unwrap()
}

#[test]
fn add_appends_open_decisions_with_random_ids_and_choices() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--title",
                "Which OAuth scopes for the write operations?",
                "--question",
                "The vendor lists read and write scopes.",
                "--phase",
                "select",
                "--choice",
                "read-only:Read-only",
                "--choice",
                "read-write:Read + write",
                "--choice-detail",
                "read-write:Grants both scopes.",
                "--date",
                "2026-09-18",
            ],
        ),
        0
    );
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "Second one",
                "--date",
                "2026-09-18"
            ]
        ),
        0
    );

    let doc = read_doc(d);
    let recs = records(&doc);
    assert_eq!(recs.len(), 2);
    let first = by_title(&doc, "Which OAuth scopes for the write operations?");
    let second = by_title(&doc, "Second one");
    // New ids are random, distinct, and never a number (ADR 0118).
    for r in [first, second] {
        let id = r["id"].as_str().unwrap();
        assert!(graphos_factory_core::record_log::is_random(id), "{}", id);
    }
    assert_ne!(first["id"], second["id"]);
    assert!(
        !d.join(".factory/decisions.json").exists(),
        "a new record never goes into the single file"
    );
    assert_eq!(first.get("status").unwrap(), "open");
    assert_eq!(first.get("phase").unwrap(), "select");
    let choices = first.get("choices").unwrap().as_array().unwrap();
    assert_eq!(choices.len(), 2);
    assert_eq!(choices[1].get("id").unwrap(), "read-write");
    assert_eq!(choices[1].get("detail").unwrap(), "Grants both scopes.");
    // An open decision carries no resolution.
    assert!(first.get("resolution").is_none());
}

#[test]
fn add_records_structured_omits_and_round_trips_through_save_and_load() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "Envelope fields are structural",
                "--resolved",
                "--decision",
                "mark them omitted",
                "--omit",
                "post:/candidate.info|response|errors[]|consumed",
                "--omit",
                "post:/candidate.info|response|success|consumed",
                "--date",
                "2026-09-18",
            ],
        ),
        0
    );
    let doc = read_doc(d);
    let recs = records(&doc);
    let omits = recs[0].get("omits").unwrap().as_array().unwrap();
    assert_eq!(omits.len(), 2);
    assert_eq!(omits[0].get("operation").unwrap(), "post:/candidate.info");
    assert_eq!(omits[0].get("direction").unwrap(), "response");
    assert_eq!(omits[0].get("path").unwrap(), "errors[]");
    assert_eq!(omits[0].get("reason").unwrap(), "consumed");
    assert_eq!(omits[1].get("path").unwrap(), "success");
}

#[test]
fn add_refuses_an_omit_with_an_unknown_reason() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--omit",
                "get:/x|response|y|maybe",
            ],
        ),
        1
    );
    assert_nothing_recorded(d);
}

/// ADR 0073, the decisions-only rule: a JSON-scalar field's reason is a
/// `decisions.json` record, written only through this CLI, mirroring how
/// `--null-handling` (ADR 0070) records the other kind of per-item reason.
#[test]
fn add_records_structured_json_reasons_and_round_trips_through_save_and_load() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "Incident carries two free-form JSON fields",
                "--resolved",
                "--decision",
                "leave them typed JSON",
                "--json-reason",
                "Pagerduty_Incident.body|free-form-object",
                "--json-reason",
                "Pagerduty_Incident.conferenceBridge|free-form-object",
                "--date",
                "2026-09-29",
            ],
        ),
        0
    );
    let doc = read_doc(d);
    let recs = records(&doc);
    let reasons = recs[0].get("json_reasons").unwrap().as_array().unwrap();
    assert_eq!(reasons.len(), 2);
    assert_eq!(reasons[0].get("type").unwrap(), "Pagerduty_Incident");
    assert_eq!(reasons[0].get("field").unwrap(), "body");
    assert_eq!(reasons[0].get("reason").unwrap(), "free-form-object");
    assert_eq!(reasons[1].get("field").unwrap(), "conferenceBridge");
}

#[test]
fn add_refuses_a_json_reason_with_an_unknown_value() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--json-reason",
                "Widget.blob|because-i-said-so",
            ],
        ),
        1
    );
    assert_nothing_recorded(d);
}

#[test]
fn add_refuses_a_json_reason_missing_the_type_dot_field_key() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--json-reason",
                "Widget|free-form-object",
            ],
        ),
        1
    );
    assert_nothing_recorded(d);
}

/// A field is one level under its type: `Type.a.b` could never match an SDL
/// field, so it is refused rather than written as an inert entry.
#[test]
fn add_refuses_a_json_reason_key_nested_past_one_field() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--json-reason",
                "Widget.config.target|free-form-object",
            ],
        ),
        1
    );
    assert_nothing_recorded(d);
}

#[test]
fn add_records_null_handling_and_the_schema_accepts_it() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "An explicit null note clears it",
                "--resolved",
                "--decision",
                "send null for note, omit a null name",
                "--null-handling",
                "patch:/widgets/{id}|note|send_null",
                "--null-handling",
                "patch:/widgets/{id}|name|omit",
                "--date",
                "2026-09-29",
            ],
        ),
        0
    );
    let doc = read_doc(d);
    let nh = records(&doc)[0]
        .get("null_handling")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(
        nh,
        &vec![
            json!({"operation": "patch:/widgets/{id}", "argument": "note", "behavior": "send_null"}),
            json!({"operation": "patch:/widgets/{id}", "argument": "name", "behavior": "omit"}),
        ]
    );
    // `save` validated it against decisions.schema.json; a reload does too.
    assert!(graphos_factory_core::decisions::load(d, None).is_ok());
}

#[test]
fn add_refuses_a_null_handling_with_an_unknown_behavior() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--null-handling",
                "patch:/x|y|drop"
            ],
        ),
        1
    );
    assert_nothing_recorded(d);
}

#[test]
fn add_records_a_structured_secret_field_and_round_trips_through_save_and_load() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add", "--question", "What does this record decide?",
                "--title",
                "Webhook signing secret is deliberately exposed",
                "--resolved",
                "--decision",
                "The vendor's own webhook-setup docs require echoing it back once.",
                "--secret-field",
                "Granola_Webhook.signingSecret|expose|Only readable once at creation, matching the vendor's own console.",
                "--date",
                "2026-09-29",
            ],
        ),
        0
    );
    let doc = read_doc(d);
    let recs = records(&doc);
    let fields = recs[0].get("secret_fields").unwrap().as_array().unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].get("type").unwrap(), "Granola_Webhook");
    assert_eq!(fields[0].get("field").unwrap(), "signingSecret");
    assert_eq!(fields[0].get("disposition").unwrap(), "expose");
    assert_eq!(
        fields[0].get("reason").unwrap(),
        "Only readable once at creation, matching the vendor's own console."
    );
}

#[test]
fn add_refuses_a_secret_field_with_an_unknown_disposition() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--secret-field",
                "Foo.token|maybe|because",
            ],
        ),
        1
    );
    assert_nothing_recorded(d);
}

#[test]
fn add_refuses_a_secret_field_with_no_dot_in_the_type_field_pair() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--secret-field",
                "token|expose|because",
            ],
        ),
        1
    );
    assert_nothing_recorded(d);
}

#[test]
fn resolve_flips_status_and_records_the_choice() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "Scopes?",
                "--choice",
                "read-write:Read + write",
                "--date",
                "2026-09-18"
            ]
        ),
        0
    );
    assert_eq!(
        decisions(
            d,
            &[
                "resolve",
                "--id",
                &id_of(d, "Scopes?"),
                "--chosen",
                "read-write",
                "--note",
                "Writes are in scope.",
                "--by",
                "user",
                "--at",
                "2026-09-18"
            ]
        ),
        0
    );
    let doc = read_doc(d);
    let rec = &records(&doc)[0];
    assert_eq!(rec.get("status").unwrap(), "resolved");
    let res = rec.get("resolution").unwrap();
    assert_eq!(
        res.get("chosen").unwrap().as_array().unwrap()[0],
        "read-write"
    );
    assert_eq!(res.get("note").unwrap(), "Writes are in scope.");
    assert_eq!(res.get("by").unwrap(), "user");
}

#[test]
fn add_resolved_records_a_judgement_call_already_made() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "Hand edit codified: teams.list",
                "--context",
                "The engineer edited the span by hand.",
                "--resolved",
                "--decision",
                "Kept as an override with assertions.",
                "--by",
                "agent",
                "--date",
                "2026-09-18"
            ]
        ),
        0
    );
    let doc = read_doc(d);
    let rec = &records(&doc)[0];
    assert_eq!(rec.get("status").unwrap(), "resolved");
    assert_eq!(
        rec.get("context").unwrap(),
        "The engineer edited the span by hand."
    );
    assert_eq!(
        rec.get("resolution").unwrap().get("decision").unwrap(),
        "Kept as an override with assertions."
    );
}

#[test]
fn list_open_json_returns_only_open_records() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    decisions(
        d,
        &[
            "add",
            "--question",
            "What does this record decide?",
            "--title",
            "One",
            "--date",
            "2026-09-18",
        ],
    );
    decisions(
        d,
        &[
            "add",
            "--question",
            "What does this record decide?",
            "--title",
            "Two",
            "--date",
            "2026-09-18",
        ],
    );
    assert_eq!(
        decisions(d, &["resolve", "--id", &id_of(d, "One"), "--note", "done"]),
        0
    );
    // We only assert the exit code here; the JSON body goes to stdout. Re-read
    // the file and filter to confirm the open set is what --open would print.
    let doc = read_doc(d);
    let open: Vec<&Value> = records(&doc)
        .iter()
        .filter(|r| r.get("status").unwrap() == "open")
        .collect();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].get("id").unwrap(), id_of(d, "Two").as_str());
    assert_eq!(decisions(d, &["list", "--open", "--json"]), 0);
}

#[test]
fn resolve_rejects_an_unknown_id() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    decisions(
        d,
        &[
            "add",
            "--question",
            "What does this record decide?",
            "--title",
            "One",
            "--date",
            "2026-09-18",
        ],
    );
    assert_eq!(
        decisions(d, &["resolve", "--id", "D-0099", "--note", "x"]),
        1
    );
}

/// `--force` with only a `--note` replaced the whole resolution and erased
/// the recorded choice and decision; with only one of `--chosen` and
/// `--decision` it erased the other. Both are refused, and the record is kept.
#[test]
fn a_forced_re_resolve_without_a_choice_or_decision_keeps_the_resolution() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    decisions(
        d,
        &[
            "add",
            "--title",
            "One",
            "--date",
            "2026-09-18",
            "--choice",
            "a:A",
            "--choice",
            "b:B",
        ],
    );
    let id = id_of(d, "One");
    assert_eq!(
        decisions(
            d,
            &[
                "resolve",
                "--id",
                &id,
                "--chosen",
                "a",
                "--decision",
                "use A"
            ]
        ),
        0
    );
    assert_eq!(
        decisions(d, &["resolve", "--id", &id, "--note", "later", "--force"]),
        1
    );
    // Passing only one of the two would erase the other: refused as well.
    assert_eq!(
        decisions(
            d,
            &["resolve", "--id", &id, "--decision", "reworded", "--force"]
        ),
        1
    );
    assert_eq!(
        decisions(d, &["resolve", "--id", &id, "--chosen", "b", "--force"]),
        1
    );
    let resolution = records(&read_doc(d))[0].get("resolution").unwrap().clone();
    assert_eq!(resolution.get("decision").unwrap(), "use A");
    assert_eq!(resolution.get("chosen").unwrap(), &serde_json::json!(["a"]));
    assert_eq!(
        decisions(
            d,
            &[
                "resolve",
                "--id",
                &id,
                "--chosen",
                "b",
                "--decision",
                "use B",
                "--note",
                "later",
                "--force"
            ]
        ),
        0
    );
    let resolution = records(&read_doc(d))[0].get("resolution").unwrap().clone();
    assert_eq!(resolution.get("decision").unwrap(), "use B");
    assert_eq!(resolution.get("chosen").unwrap(), &serde_json::json!(["b"]));
}

#[test]
fn resolve_refuses_to_re_resolve_without_force() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    decisions(
        d,
        &[
            "add",
            "--question",
            "What does this record decide?",
            "--title",
            "One",
            "--date",
            "2026-09-18",
        ],
    );
    let id = id_of(d, "One");
    assert_eq!(
        decisions(d, &["resolve", "--id", &id, "--note", "first"]),
        0
    );
    assert_eq!(
        decisions(d, &["resolve", "--id", &id, "--note", "second"]),
        1
    );
    assert_eq!(
        decisions(d, &["resolve", "--id", &id, "--note", "second", "--force"]),
        0
    );
    let doc = read_doc(d);
    assert_eq!(
        records(&doc)[0]
            .get("resolution")
            .unwrap()
            .get("note")
            .unwrap(),
        "second"
    );
}

#[test]
fn add_requires_a_title() {
    let dir = TempDir::new().unwrap();
    assert_eq!(decisions(dir.path(), &["add", "--phase", "select"]), 1);
    assert_nothing_recorded(dir.path());
}

#[test]
fn load_refuses_a_present_but_invalid_file() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    // contract_version 2 violates the schema's const: 1.
    std::fs::write(
        d.join(".factory/decisions.json"),
        "{\"contract_version\": 2, \"decisions\": []}",
    )
    .unwrap();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "One"
            ]
        ),
        1
    );
    assert_eq!(decisions(d, &["list"]), 1);
}

const LEGACY_MD: &str = "# Decisions — test\n\nAppend-only. Newest last.\n\n## D-0001 · 2026-09-08 · First choice\nContext: because reasons.\nDecision: do the thing.\nAlternatives: not doing it (rejected).\nRequested by: user.\nAffects: get:/a, get:/b.\n\n## D-0002 · 2026-09-08 · Second\nContext: more.\nDecision: other thing.\nAffects: every operation.\n";

fn write_md(d: &Path, text: &str) {
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    std::fs::write(d.join(".factory/decisions.md"), text).unwrap();
}

#[test]
fn migrate_imports_legacy_markdown_preserving_ids_and_fields() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(d, LEGACY_MD);
    assert_eq!(decisions(d, &["migrate"]), 0);
    assert!(
        !d.join(".factory/decisions.md").exists(),
        "decisions.md is removed by default"
    );
    let doc = read_doc(d);
    let recs = records(&doc);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0].get("id").unwrap(), "D-0001");
    assert_eq!(recs[0].get("status").unwrap(), "resolved");
    assert_eq!(recs[0].get("title").unwrap(), "First choice");
    assert_eq!(recs[0].get("context").unwrap(), "because reasons.");
    assert_eq!(recs[0].get("requested_by").unwrap(), "user");
    assert_eq!(
        recs[0].get("affects").unwrap(),
        &json!(["get:/a", "get:/b"])
    );
    let res = recs[0].get("resolution").unwrap();
    assert_eq!(res.get("decision").unwrap(), "do the thing.");
    assert!(res
        .get("note")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("Alternatives: not doing it (rejected)."));
    assert_eq!(recs[1].get("id").unwrap(), "D-0002");
    assert_eq!(recs[1].get("affects").unwrap(), &json!(["every operation"]));
    // Id numbering continues after the migrated max.
    assert_eq!(
        graphos_factory_core::decisions::next_id(&read_doc(d)),
        "D-0003"
    );
}

const LEGACY_MD_WITH_OMITS: &str = "# Decisions — test\n\n## D-0001 · 2026-09-08 · Envelope fields are structural\nContext: every operation shares the same envelope.\n```\nOmits:\n  - operation: post:/candidate.info\n    direction: response\n    path: errors[]\n    reason: consumed\n  - operation: post:/candidate.info\n    direction: response\n    path: success\n    reason: consumed\n```\nDecision: mark them omitted.\nAffects: post:/candidate.info.\n";

#[test]
fn migrate_parses_a_fenced_omits_block_into_structured_records() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(d, LEGACY_MD_WITH_OMITS);
    assert_eq!(decisions(d, &["migrate"]), 0);
    let doc = read_doc(d);
    let recs = records(&doc);
    assert_eq!(recs.len(), 1);
    let omits = recs[0].get("omits").unwrap().as_array().unwrap();
    assert_eq!(omits.len(), 2);
    assert_eq!(omits[0].get("operation").unwrap(), "post:/candidate.info");
    assert_eq!(omits[0].get("direction").unwrap(), "response");
    assert_eq!(omits[0].get("path").unwrap(), "errors[]");
    assert_eq!(omits[0].get("reason").unwrap(), "consumed");
    assert_eq!(omits[1].get("path").unwrap(), "success");
    // The fenced block itself never lands in resolution.note as inert prose.
    let note = recs[0]
        .get("resolution")
        .and_then(|r| r.get("note"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(!note.contains("Omits:"));
}

#[test]
fn migrate_refuses_an_omits_entry_with_an_unknown_direction() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(
        d,
        "## D-0001 · 2026-09-08 · Bad\n```\nOmits:\n  - operation: post:/x\n    direction: sideways\n    path: y\n    reason: consumed\n```\nDecision: n/a.\n",
    );
    assert_eq!(decisions(d, &["migrate"]), 1);
    assert!(
        !d.join(".factory/decisions.json").exists(),
        "a decision the tool cannot express is refused, not written"
    );
    assert!(
        d.join(".factory/decisions.md").exists(),
        "the legacy file is untouched on a refused migration"
    );
}

#[test]
fn migrate_refuses_when_json_exists_unless_forced() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(d, LEGACY_MD);
    // Creating a decisions.json first makes migrate refuse without --force.
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "pre-existing",
                "--date",
                "2026-09-18"
            ]
        ),
        0
    );
    as_legacy(d, &["pre-existing"]);
    assert_eq!(decisions(d, &["migrate"]), 1);
    assert!(
        d.join(".factory/decisions.md").exists(),
        "the md is untouched on refusal"
    );
    assert_eq!(decisions(d, &["migrate", "--force"]), 0);
    let doc = read_doc(d);
    let recs = records(&doc);
    assert_eq!(recs.len(), 2, "--force overwrites from the md");
    assert_eq!(recs[0].get("title").unwrap(), "First choice");
}

#[test]
fn migrate_dry_run_writes_nothing_and_keep_md_keeps_it() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(d, LEGACY_MD);
    assert_eq!(decisions(d, &["migrate", "--dry-run"]), 0);
    assert_nothing_recorded(d);
    assert!(d.join(".factory/decisions.md").exists());
    assert_eq!(decisions(d, &["migrate", "--keep-md"]), 0);
    assert!(d.join(".factory/decisions.json").exists());
    assert!(
        d.join(".factory/decisions.md").exists(),
        "--keep-md keeps it"
    );
}

#[test]
fn migrate_refuses_a_file_with_no_decision_blocks() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(d, "# Decisions\n\nJust a preamble, no D-nnnn blocks.\n");
    assert_eq!(decisions(d, &["migrate"]), 1);
    assert_nothing_recorded(d);
    assert!(
        d.join(".factory/decisions.md").exists(),
        "nothing is deleted"
    );
}

#[test]
fn migrate_reports_already_done_when_only_json_exists() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "x",
                "--date",
                "2026-09-18"
            ]
        ),
        0
    );
    as_legacy(d, &["x"]);
    assert_eq!(decisions(d, &["migrate"]), 0);
}

#[test]
fn parse_markdown_warns_on_a_headerless_block() {
    let (recs, warns) =
        graphos_factory_core::decisions::parse_markdown("## D-0005\nContext: c.\n").unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].get("id").unwrap(), "D-0005");
    assert_eq!(recs[0].get("date").unwrap(), "unknown");
    assert_eq!(recs[0].get("title").unwrap(), "(untitled)");
    assert!(warns.iter().any(|w| w.contains("no date")));
    assert!(warns.iter().any(|w| w.contains("no title")));
}

/// `.factory/decisions.json` is custody's like every other `.factory` file
/// (ADR 0025): a symlink out of the workspace is refused on read and on
/// write, and the target is untouched. Without this, `decisions add` and
/// `resolve` would rewrite whatever the link points at, and `list` would
/// print it.
#[cfg(unix)]
#[test]
fn a_symlinked_decision_log_is_refused_and_its_target_untouched() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory")).unwrap();

    // A log that reads perfectly — the refusal must not depend on bad content.
    let target = outside.path().join("someone-elses-decisions.json");
    let valid = graphos_factory_core::json::pretty(&json!({
        "contract_version": 1,
        "decisions": [{
            "id": "D-0001", "date": "2026-09-08", "title": "Theirs",
            "status": "resolved", "phase": "select",
            "resolution": {"decision": "keep", "by": "user", "at": "2026-09-08"}
        }]
    }));
    std::fs::write(&target, &valid).unwrap();
    std::os::unix::fs::symlink(&target, d.join(".factory/decisions.json")).unwrap();

    for args in [
        vec!["list"],
        vec!["show", "D-0001"],
        vec![
            "add",
            "--question",
            "What does this record decide?",
            "--title",
            "New",
            "--phase",
            "select",
        ],
        vec!["resolve", "D-0001", "--decision", "clobbered"],
        vec!["reopen", "--id", "D-0001"],
    ] {
        let code = decisions(d, &args);
        assert_eq!(code, 1, "decisions {:?} must refuse a symlinked log", args);
    }
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        valid,
        "the file outside the workspace must be untouched"
    );
    assert!(
        std::fs::symlink_metadata(d.join(".factory/decisions.json"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link itself is left alone, not replaced by a regular file"
    );
}

/// The legacy log `migrate` reads is under `.factory/` too, so the same rule
/// holds: a symlinked `decisions.md` is neither imported nor unlinked.
#[cfg(unix)]
#[test]
fn migrate_refuses_a_symlinked_legacy_log() {
    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    let target = outside.path().join("decisions.md");
    std::fs::write(&target, LEGACY_MD).unwrap();
    std::os::unix::fs::symlink(&target, d.join(".factory/decisions.md")).unwrap();

    assert_eq!(decisions(d, &["migrate"]), 1);
    assert_nothing_recorded(d);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), LEGACY_MD);
    assert!(d.join(".factory/decisions.md").is_symlink());
}

const MIXED_FENCES_MD: &str = "# Decisions\n\n## D-0001 · 2026-09-01 · First, with a yaml example\n\nContext: an example block follows.\n\n```yaml\nkey: value\n```\n\nDecision: keep it.\n\n## D-0002 · 2026-09-02 · Second, plain\n\nDecision: plain.\n\n## D-0003 · 2026-09-03 · Third, with a json example\n\n```json\n{\"a\": 1}\n```\n\nDecision: done.\n\n## D-0004 · 2026-09-04 · Fourth, plain\n\nDecision: last.\n";

/// A ```yaml / ```json opener used to go unrecognised: its closing ``` was
/// taken as an opener and D-0002 and D-0003 were swallowed into D-0001's
/// note, silently, before decisions.md was removed. Every block now comes
/// through with its own decision.
#[test]
fn migrate_keeps_every_decision_across_fences_with_an_info_string() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(d, MIXED_FENCES_MD);
    assert_eq!(decisions(d, &["migrate"]), 0);
    let doc = read_doc(d);
    let recs = records(&doc);
    let ids: Vec<&str> = recs.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["D-0001", "D-0002", "D-0003", "D-0004"]);
    let decision = |i: usize| {
        recs[i]["resolution"]["decision"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(decision(0), "keep it.");
    assert_eq!(decision(2), "done.");
}

/// A fence that never balances before the next decision header is refused:
/// nothing is written and decisions.md stays.
#[test]
fn migrate_refuses_a_fence_that_runs_into_the_next_decision() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(
        d,
        "# Decisions\n\n## D-0001 · 2026-09-01 · Open fence\n\n```\nexample\n\nDecision: x.\n\n## D-0002 · 2026-09-02 · Next\n\n```\n\nDecision: y.\n",
    );
    assert_eq!(decisions(d, &["migrate"]), 1);
    assert_nothing_recorded(d);
    assert!(d.join(".factory/decisions.md").exists());
}

/// A `~~~` fence is a fence too. A `## D-0002` example inside one used to be
/// read as a real header, the real D-0002 was then skipped as a duplicate,
/// and migrate exited 0 and removed decisions.md (PR 65 review, T8). Now the
/// header inside the fence refuses the migration, and so does any duplicate
/// id: nothing is written and decisions.md stays.
#[test]
fn a_tilde_fence_holding_a_decision_header_never_replaces_the_real_decision() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(
        d,
        "# Decisions\n\n## D-0001 · 2026-09-01 · First\n\nContext: an example follows.\n\n~~~\n## D-0002 · 2026-09-02 · Example only\n\nDecision: fake.\n~~~\n\nDecision: keep it.\n\n## D-0002 · 2026-09-02 · The real second\n\nDecision: real.\n",
    );
    assert_eq!(decisions(d, &["migrate"]), 1);
    assert_nothing_recorded(d);
    assert!(d.join(".factory/decisions.md").exists());

    // A ~~~ block without a header inside it migrates like a ``` block.
    write_md(
        d,
        "# Decisions\n\n## D-0001 · 2026-09-01 · First\n\n~~~yaml\nkey: value\n~~~\n\nDecision: keep it.\n\n## D-0002 · 2026-09-02 · Second\n\nDecision: real.\n",
    );
    assert_eq!(decisions(d, &["migrate"]), 0);
    let doc = read_doc(d);
    let recs = records(&doc);
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0]["resolution"]["decision"], "keep it.");
    assert_eq!(recs[1]["resolution"]["decision"], "real.");
}

#[test]
fn a_duplicate_decision_id_refuses_the_migration() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    write_md(
        d,
        "# Decisions\n\n## D-0001 · 2026-09-01 · First\n\nDecision: a.\n\n## D-0001 · 2026-09-02 · Again\n\nDecision: b.\n",
    );
    assert_eq!(decisions(d, &["migrate"]), 1);
    assert!(d.join(".factory/decisions.md").exists());
}

// ─── reopen (ADR 0060) ──────────────────────────────────────────────────────

fn read_text(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".factory/decisions.json")).unwrap()
}

/// Run the real binary's `decisions reopen … --json`; (exit code, parsed stdout).
fn reopen_json(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["decisions", "reopen"])
        .arg(dir)
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let body = graphos_factory_core::json::parse(&text)
        .unwrap_or_else(|e| panic!("stdout not JSON ({}): {:?}", e, text));
    (out.status.code().unwrap(), body)
}

/// Three decisions in an existing workspace's `decisions.json`: D-0001
/// resolved with a choice, D-0002 open, D-0003 a resolved record carrying
/// every optional field, including omits. Built through `add` and `resolve`,
/// then numbered as a log written before ADR 0118 holds them, so reopen and
/// supersede are proven on the old records they will mostly meet.
fn three_decisions(d: &Path) {
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--title",
                "Scopes?",
                "--choice",
                "ro:Read only",
                "--choice",
                "rw:Read + write",
                "--date",
                "2026-09-18",
            ],
        ),
        0
    );
    assert_eq!(
        decisions(
            d,
            &[
                "resolve",
                "--id",
                &id_of(d, "Scopes?"),
                "--chosen",
                "rw",
                "--by",
                "user",
                "--at",
                "2026-09-18"
            ],
        ),
        0
    );
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "Still open",
                "--date",
                "2026-09-18"
            ]
        ),
        0
    );
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--title",
                "Drop the audit block",
                "--question",
                "Expose the audit block?",
                "--context",
                "Nobody reads it.",
                "--phase",
                "select",
                "--requested-by",
                "user",
                "--multiple",
                "--affects",
                "post:/candidate.info",
                "--choice",
                "drop:Drop it",
                "--choice",
                "keep:Keep it",
                "--choice-detail",
                "drop:Not mapped.",
                "--resolved",
                "--chosen",
                "drop",
                "--note",
                "Agreed in chat.",
                "--decision",
                "Omit results.audit.",
                "--by",
                "user",
                "--omit",
                "post:/candidate.info|response|results.audit|editorial",
                "--date",
                "2026-09-19",
            ],
        ),
        0
    );
    as_legacy(d, &["Scopes?", "Still open", "Drop the audit block"]);
}

#[test]
fn reopen_clears_the_answer_and_keeps_every_other_field() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    three_decisions(d);
    let before = records(&read_doc(d))[2].clone();
    assert!(before.get("resolution").is_some());

    assert_eq!(decisions(d, &["reopen", "--id", "D-0003"]), 0);

    let after = records(&read_doc(d))[2].clone();
    assert_eq!(after.get("status").unwrap(), "open");
    assert!(after.get("resolution").is_none());
    // Exactly the resolution went and the status flipped; everything else —
    // title, question, context, phase, requested_by, multiple, affects,
    // choices, omits — is the same value, in the same key order.
    let mut expected = before.clone();
    let m = expected.as_object_mut().unwrap();
    m.shift_remove("resolution");
    m.insert("status".into(), Value::from("open"));
    assert_eq!(
        graphos_factory_core::json::pretty(&after),
        graphos_factory_core::json::pretty(&expected)
    );
    assert_eq!(
        after.get("omits").unwrap(),
        &json!([{
            "operation": "post:/candidate.info", "direction": "response",
            "path": "results.audit", "reason": "editorial"
        }])
    );
}

#[test]
fn reopen_leaves_every_other_decision_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    three_decisions(d);
    let before_text = read_text(d);
    let mut expected = read_doc(d);

    assert_eq!(decisions(d, &["reopen", "--id", "D-0001"]), 0);

    // The file is exactly the old document with that one record changed:
    // the other records, their order and the whole layout are untouched.
    let rec = &mut expected["decisions"][0];
    rec.as_object_mut().unwrap().shift_remove("resolution");
    rec["status"] = Value::from("open");
    let after_text = read_text(d);
    assert_eq!(after_text, graphos_factory_core::json::pretty(&expected));
    let before_doc = graphos_factory_core::json::parse(&before_text).unwrap();
    let after_doc = read_doc(d);
    for i in [1, 2] {
        assert_eq!(
            graphos_factory_core::json::pretty(&records(&after_doc)[i]),
            graphos_factory_core::json::pretty(&records(&before_doc)[i]),
            "record {} must be untouched",
            i
        );
    }
    // D-0003's bytes appear verbatim in both files.
    let third = graphos_factory_core::json::pretty(&records(&before_doc)[2]);
    let third = third.trim_end();
    assert!(before_text.contains(&third.replace('\n', "\n    ")));
    assert!(after_text.contains(&third.replace('\n', "\n    ")));
}

#[test]
fn reopen_refuses_an_open_decision_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    three_decisions(d);
    let before = read_text(d);
    assert_eq!(decisions(d, &["reopen", "--id", "D-0002"]), 1);
    assert_eq!(read_text(d), before);
    let (code, body) = reopen_json(d, &["--id", "D-0002"]);
    assert_eq!(code, 1);
    assert_eq!(body["code"], "already-open");
    assert_eq!(body["exit"], 1);
    assert!(body["error"].as_str().unwrap().contains("D-0002"));
    assert_eq!(read_text(d), before);
}

#[test]
fn reopen_refuses_an_unknown_id_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    three_decisions(d);
    let before = read_text(d);
    assert_eq!(decisions(d, &["reopen", "--id", "D-0099"]), 1);
    let (code, body) = reopen_json(d, &["--id", "D-0099"]);
    assert_eq!(code, 1);
    assert_eq!(body["code"], "unknown-decision");
    assert_eq!(read_text(d), before);
    // --id is required.
    assert_eq!(decisions(d, &["reopen"]), 1);
    let (code, body) = reopen_json(d, &[]);
    assert_eq!(code, 1);
    assert_eq!(body["code"], "usage");
    assert_eq!(read_text(d), before);
}

/// `add` starts a log when there is none; `reopen` must not — there is no
/// decision to reopen, and it creates neither the file nor `.factory/`.
#[test]
fn reopen_refuses_a_missing_log_and_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(decisions(d, &["reopen", "--id", "D-0001"]), 1);
    let (code, body) = reopen_json(d, &["--id", "D-0001"]);
    assert_eq!(code, 1);
    assert_eq!(body["code"], "decisions-missing");
    assert!(!d.join(".factory").exists());
}

#[test]
fn reopen_refuses_an_invalid_log_and_leaves_it() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    for text in [
        "{\"contract_version\": 2, \"decisions\": []}",
        "{ not json",
        // A resolved record without its resolution violates the schema.
        "{\"contract_version\": 1, \"decisions\": [{\"id\": \"D-0001\", \"title\": \"t\", \"status\": \"resolved\", \"date\": \"d\"}]}",
    ] {
        std::fs::write(d.join(".factory/decisions.json"), text).unwrap();
        assert_eq!(decisions(d, &["reopen", "--id", "D-0001"]), 1);
        let (code, body) = reopen_json(d, &["--id", "D-0001"]);
        assert_eq!(code, 1, "{}", text);
        assert_eq!(body["code"], "decisions-invalid", "{}", text);
        assert_eq!(read_text(d), text);
    }
}

#[test]
fn reopen_reopens_a_superseded_decision() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    let doc = json!({
        "contract_version": 1,
        "decisions": [
            {"id": "D-0001", "title": "Old", "status": "superseded", "date": "2026-09-01",
             "resolution": {"decision": "use v1", "by": "agent", "at": "2026-09-01"}},
            {"id": "D-0002", "title": "Bare", "status": "superseded", "date": "2026-09-02"}
        ]
    });
    std::fs::write(
        d.join(".factory/decisions.json"),
        graphos_factory_core::json::pretty(&doc),
    )
    .unwrap();
    let (code, body) = reopen_json(d, &["--id", "D-0001"]);
    assert_eq!(code, 0);
    assert_eq!(
        body,
        json!({
            "id": "D-0001", "status": "open", "previous_status": "superseded",
            "cleared": {"decision": "use v1", "by": "agent", "at": "2026-09-01"},
            "exit": 0
        })
    );
    // A superseded record with no resolution reopens too; nothing to clear.
    let (code, body) = reopen_json(d, &["--id", "D-0002"]);
    assert_eq!(code, 0);
    assert_eq!(body["cleared"], Value::Null);
    let doc = read_doc(d);
    for rec in records(&doc) {
        assert_eq!(rec.get("status").unwrap(), "open");
        assert!(rec.get("resolution").is_none());
    }
}

#[test]
fn resolve_after_reopen_records_the_new_answer_without_force() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    three_decisions(d);
    let (code, body) = reopen_json(d, &["--id", "D-0001"]);
    assert_eq!(code, 0);
    assert_eq!(body["previous_status"], "resolved");
    assert_eq!(
        body["cleared"],
        json!({"chosen": ["rw"], "by": "user", "at": "2026-09-18"})
    );
    // An open record resolves without --force, and the new answer replaces
    // nothing: the old one is already gone.
    assert_eq!(
        decisions(
            d,
            &[
                "resolve",
                "--id",
                "D-0001",
                "--chosen",
                "ro",
                "--note",
                "Read only after all.",
                "--at",
                "2026-09-25"
            ],
        ),
        0
    );
    let rec = records(&read_doc(d))[0].clone();
    assert_eq!(rec.get("status").unwrap(), "resolved");
    assert_eq!(
        rec.get("resolution").unwrap(),
        // `--by` absent records the agent (ADR 0113 §1).
        &json!({"chosen": ["ro"], "note": "Read only after all.", "by": "agent", "at": "2026-09-25"})
    );
    // And it can be reopened again.
    assert_eq!(decisions(d, &["reopen", "--id", "D-0001"]), 0);
    assert_eq!(records(&read_doc(d))[0].get("status").unwrap(), "open");
}

// ─── ADR 0095: behaviour omits ──────────────────────────────────────────────

/// A behaviour waiver is recorded like any omit: the operation, `behaviour`,
/// the source it waives (`query:x`) and a reason.
#[test]
fn add_records_a_behaviour_omit() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "by default is about the server",
                "--resolved",
                "--decision",
                "the sentence is not about omitting the argument",
                "--omit",
                "get:/x|behaviour|query:sort|not-applicable",
                "--omit",
                "get:/x|behaviour|body:a.b|editorial",
            ],
        ),
        0
    );
    let doc = read_doc(d);
    let omits = records(&doc)[0].get("omits").unwrap().as_array().unwrap();
    assert_eq!(omits.len(), 2);
    assert_eq!(omits[0].get("direction").unwrap(), "behaviour");
    assert_eq!(omits[0].get("path").unwrap(), "query:sort");
    assert_eq!(omits[0].get("reason").unwrap(), "not-applicable");
    assert_eq!(omits[1].get("reason").unwrap(), "editorial");
}

/// Each direction has its own reasons: `consumed` is a wire fact and means
/// nothing for a behaviour omit, `not-applicable` means nothing for a wire
/// path. Neither reaches the file.
#[test]
fn add_refuses_a_reason_that_does_not_fit_the_direction() {
    for spec in [
        "get:/x|behaviour|query:sort|consumed",
        "get:/x|response|y|not-applicable",
        "get:/x|request|y|not-applicable",
        "get:/x|sideways|y|editorial",
    ] {
        let dir = TempDir::new().unwrap();
        let d = dir.path();
        assert_eq!(
            decisions(
                d,
                &[
                    "add",
                    "--question",
                    "What does this record decide?",
                    "--title",
                    "x",
                    "--omit",
                    spec
                ]
            ),
            1,
            "{}",
            spec
        );
        assert!(!d.join(".factory/decisions.json").exists(), "{}", spec);
    }
}

/// The schema enforces the same pairing on a hand-written file, so a
/// `behaviour` omit with `consumed` cannot be saved either.
#[test]
fn the_schema_refuses_a_reason_that_does_not_fit_the_direction() {
    let dir = TempDir::new().unwrap();
    let record = |direction: &str, reason: &str| {
        json!({"contract_version": 1, "decisions": [{
            "id": "D-0001", "title": "t", "status": "resolved", "date": "2026-09-29",
            "resolution": {"decision": "d"},
            "omits": [{"operation": "get:/x", "direction": direction, "path": "p", "reason": reason}]
        }]})
    };
    for (direction, reason, ok) in [
        ("behaviour", "not-applicable", true),
        ("behaviour", "editorial", true),
        ("behaviour", "consumed", false),
        ("response", "consumed", true),
        ("request", "not-applicable", false),
    ] {
        let saved =
            graphos_factory_core::decisions::save(dir.path(), &record(direction, reason), None);
        assert_eq!(saved.is_ok(), ok, "{} {}: {:?}", direction, reason, saved);
    }
}

// ─── supersede (ADR 0103) ───────────────────────────────────────────────────

/// Run the real binary's `decisions supersede … --json`; (exit code, parsed stdout).
fn supersede_json(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["decisions", "supersede"])
        .arg(dir)
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let body = graphos_factory_core::json::parse(&text)
        .unwrap_or_else(|e| panic!("stdout not JSON ({}): {:?}", e, text));
    (out.status.code().unwrap(), body)
}

#[test]
fn supersede_flips_only_the_status_and_keeps_the_answer_and_omits() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    three_decisions(d);
    let before = read_doc(d);
    let (code, body) = supersede_json(d, &["--id", "D-0003"]);
    assert_eq!(code, 0);
    assert_eq!(
        body,
        json!({"id": "D-0003", "status": "superseded", "previous_status": "resolved", "exit": 0})
    );
    let after = read_doc(d);
    // Every record but D-0003 is unchanged, and D-0003 differs only in its
    // status: the resolution and the omits stay, in the same key order.
    let mut expected = before.clone();
    expected["decisions"][2]["status"] = Value::from("superseded");
    assert_eq!(after, expected);
    assert_eq!(
        graphos_factory_core::json::pretty(&after),
        graphos_factory_core::json::pretty(&expected)
    );
    // Its omits no longer count.
    let omits = graphos_factory_core::obligations::omits_from_doc(&after);
    assert!(omits.iter().all(|e| e.status == "superseded"));
    // A superseded record reopens like any other.
    assert_eq!(decisions(d, &["reopen", "--id", "D-0003"]), 0);
}

#[test]
fn supersede_refuses_what_is_not_resolved_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    three_decisions(d);
    assert_eq!(decisions(d, &["supersede", "--id", "D-0001"]), 0);
    let before = read_text(d);
    for (id, code) in [
        ("D-0002", "not-resolved"),
        ("D-0001", "already-superseded"),
        ("D-0099", "unknown-decision"),
    ] {
        assert_eq!(decisions(d, &["supersede", "--id", id]), 1, "{}", id);
        let (exit, body) = supersede_json(d, &["--id", id]);
        assert_eq!(exit, 1, "{}", id);
        assert_eq!(body["code"], code, "{}", id);
        assert!(body["error"].as_str().unwrap().contains(id), "{}", body);
        assert_eq!(read_text(d), before, "{}", id);
    }
    let (exit, body) = supersede_json(d, &[]);
    assert_eq!(exit, 1);
    assert_eq!(body["code"], "usage");
    assert_eq!(read_text(d), before);
}

#[test]
fn supersede_refuses_a_missing_log_and_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let (code, body) = supersede_json(d, &["--id", "D-0001"]);
    assert_eq!(code, 1);
    assert_eq!(body["code"], "decisions-missing");
    assert!(!d.join(".factory").exists());
}

/// `depth-cap` (the fifth reason): accepted by the CLI and by
/// decisions.schema.json's enum, so the record loads back.
#[test]
fn add_accepts_the_depth_cap_json_reason() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--question",
                "What does this record decide?",
                "--title",
                "Deep config cut at max_depth",
                "--resolved",
                "--decision",
                "leave it JSON at the cut",
                "--json-reason",
                "Widget_Config.target|depth-cap",
                "--date",
                "2026-09-29",
            ],
        ),
        0
    );
    let doc = read_doc(d);
    let reasons = records(&doc)[0]
        .get("json_reasons")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(reasons[0].get("reason").unwrap(), "depth-cap");
    assert!(graphos_factory_core::decisions::load_present(d, None)
        .unwrap()
        .is_some());
}

// ─── ADR 0113: a decision carries its alternative ───────────────────────────

/// `decisions add` refuses a record with no `--question`, fewer than two
/// `--choice`s and no `editorial` omit (exit 1, `no-alternative`), and
/// writes nothing; one with any of the three is recorded. A `consumed`
/// omit is a fact and does not count as an alternative.
#[test]
fn add_refuses_a_record_that_names_no_alternative() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    for args in [
        vec![
            "add",
            "--title",
            "Enums keep wire casing",
            "--resolved",
            "--note",
            "naming.md",
        ],
        vec!["add", "--title", "One choice", "--choice", "a:A"],
        vec!["add", "--title", "Blank question", "--question", "  "],
        vec![
            "add",
            "--title",
            "Envelope flag consumed",
            "--resolved",
            "--decision",
            "isSuccess reads it",
            "--omit",
            "post:/candidate.info|response|success|consumed",
        ],
    ] {
        assert_eq!(decisions(d, &args), 1, "{:?}", args);
    }
    assert_nothing_recorded(d);

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "decisions",
            "add",
            d.to_str().unwrap(),
            "--title",
            "x",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let refusal: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(refusal["code"], "no-alternative");
    assert_eq!(refusal["exit"], 1);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("findings add"), "{}", stderr);
    assert_nothing_recorded(d);

    // A question, or two choices, is an alternative.
    assert_eq!(
        decisions(d, &["add", "--title", "Q", "--question", "Which scalar?"]),
        0
    );
    assert_eq!(
        decisions(
            d,
            &["add", "--title", "C", "--choice", "a:A", "--choice", "b:B"]
        ),
        0
    );
    // An editorial omit carries its alternative (expose the path): the
    // documented scope-exclusion shape (schema-authoring.md § Coverage)
    // records as a resolved decision.
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--title",
                "Drop the internal audit block from candidate reads",
                "--resolved",
                "--decision",
                "Internal-only; no caller has asked for it.",
                "--omit",
                "post:/candidate.info|response|results.auditTrail|editorial",
            ]
        ),
        0
    );
    let doc = read_doc(d);
    let last = by_title(&doc, "Drop the internal audit block from candidate reads");
    assert_eq!(last["status"], "resolved");
    assert_eq!(last["omits"][0]["reason"], "editorial");
    assert_eq!(records(&doc).len(), 3);
}

/// `add --resolved` and `resolve` record `by: agent` unless `--by` says
/// otherwise (ADR 0113 §1): the plugin passes `--by user` on every user path.
#[test]
fn resolution_by_defaults_to_agent() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--title",
                "Agent's call",
                "--question",
                "Int or String?",
                "--choice",
                "int:Int",
                "--choice",
                "string:String",
                "--resolved",
                "--chosen",
                "string",
                "--note",
                "int64",
            ]
        ),
        0
    );
    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--title",
                "User's call",
                "--question",
                "Expose it?",
                "--resolved",
                "--note",
                "no",
                "--by",
                "user",
            ]
        ),
        0
    );
    assert_eq!(
        decisions(d, &["add", "--title", "Open", "--question", "Which?"]),
        0
    );
    assert_eq!(
        decisions(
            d,
            &["resolve", "--id", &id_of(d, "Open"), "--note", "this one"]
        ),
        0
    );
    let doc = read_doc(d);
    let by: Vec<&str> = ["Agent's call", "User's call", "Open"]
        .iter()
        .map(|t| by_title(&doc, t)["resolution"]["by"].as_str().unwrap())
        .collect();
    assert_eq!(by, vec!["agent", "user", "agent"]);
}

/// The binary's `decisions` verb in `dir`: (exit code, stdout, stderr).
fn decisions_bin(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let mut argv: Vec<String> = vec![args[0].to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args[1..].iter().map(|a| a.to_string()));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("decisions")
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `--choice LABEL` numbers the choices 1, 2, 3 in flag order, `add` prints
/// them, `resolve --chosen 2` records "2", and the record passes the schema
/// and round-trips through `decisions list --json`.
#[test]
fn choices_without_an_id_are_numbered_and_resolved_by_number() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let (code, stdout, stderr) = decisions_bin(
        d,
        &[
            "add",
            "--title",
            "Pagination",
            "--question",
            "How are lists paged?",
            "--choice",
            "Paginate: cursor",
            "--choice",
            "Offset and limit",
            "--choice",
            "No pagination",
            "--choice-detail",
            "2:the API's own page/limit",
        ],
    );
    assert_eq!(code, Some(0), "{}{}", stdout, stderr);
    let id = id_of(d, "Pagination");
    assert_eq!(
        stdout,
        format!(
            "recorded {}\n  1  Paginate: cursor\n  2  Offset and limit\n  3  No pagination\n",
            id
        )
    );
    let rec = by_title(&read_doc(d), "Pagination").clone();
    assert_eq!(
        rec["choices"],
        json!([
            {"id": "1", "label": "Paginate: cursor"},
            {"id": "2", "label": "Offset and limit", "detail": "the API's own page/limit"},
            {"id": "3", "label": "No pagination"}
        ])
    );

    let (code, stdout, stderr) = decisions_bin(d, &["resolve", "--id", &id, "--chosen", "2"]);
    assert_eq!(code, Some(0), "{}{}", stdout, stderr);
    assert_eq!(stdout, format!("resolved {}\n  2  Offset and limit\n", id));

    // The schema accepts the numeric ids, and list --json carries them back.
    let schema = graphos_factory_core::schemas::load("decisions.schema.json", None).unwrap();
    let doc = read_doc(d);
    assert_eq!(
        graphos_factory_core::jsonschema::validate(&doc, &schema),
        Vec::<String>::new()
    );
    let (code, stdout, _) = decisions_bin(d, &["list", "--json"]);
    assert_eq!(code, Some(0));
    let listed: Value = serde_json::from_str(&stdout).unwrap();
    let listed = listed["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(listed["resolution"]["chosen"], json!(["2"]));
    assert_eq!(listed["choices"][1]["id"], "2");
}

/// `--chosen` takes the exact label of exactly one choice, on `resolve` and
/// on `add --resolved`; a value naming no choice is refused with the list.
#[test]
fn chosen_takes_an_exact_label_and_refuses_one_naming_no_choice() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        decisions(
            d,
            &["add", "--title", "Auth", "--choice", "Bearer", "--choice", "Basic",]
        ),
        0
    );
    let id = id_of(d, "Auth");
    let (code, _, stderr) = decisions_bin(d, &["resolve", "--id", &id, "--chosen", "Digest"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(
            "--chosen \"Digest\" names no choice id or label; the choices are: 1  Bearer; 2  Basic"
        ),
        "{}",
        stderr
    );
    assert_eq!(by_title(&read_doc(d), "Auth")["status"], "open");
    assert_eq!(
        decisions(d, &["resolve", "--id", &id, "--chosen", "Basic"]),
        0
    );
    assert_eq!(
        by_title(&read_doc(d), "Auth")["resolution"]["chosen"],
        json!(["2"])
    );

    assert_eq!(
        decisions(
            d,
            &[
                "add",
                "--title",
                "Paging",
                "--choice",
                "Cursor",
                "--choice",
                "Offset",
                "--resolved",
                "--chosen",
                "Cursor",
            ]
        ),
        0
    );
    assert_eq!(
        by_title(&read_doc(d), "Paging")["resolution"]["chosen"],
        json!(["1"])
    );
}

/// The labels a choice of one call records, in order, from a fresh log.
fn recorded_choices(args: &[&str]) -> Value {
    let dir = TempDir::new().unwrap();
    let mut argv = vec!["add", "--title", "T"];
    argv.extend_from_slice(args);
    let (code, stdout, stderr) = decisions_bin(dir.path(), &argv);
    assert_eq!(code, Some(0), "{}{}", stdout, stderr);
    by_title(&read_doc(dir.path()), "T")["choices"].clone()
}

/// Ids are kept only when every `--choice` of the call is `id:label`, the
/// older calling form; otherwise every value is a whole label, so a label
/// with a colon in it is never cut there.
#[test]
fn an_explicit_id_is_kept_only_when_every_choice_has_one() {
    // The older form, every value id:label: the ids are kept.
    assert_eq!(
        recorded_choices(&["--choice", "keep:Keep it", "--choice", "drop:Drop it"]),
        json!([{"id": "keep", "label": "Keep it"}, {"id": "drop", "label": "Drop it"}])
    );
    // A mixed call is all labels.
    assert_eq!(
        recorded_choices(&["--choice", "keep:Keep it", "--choice", "Drop it"]),
        json!([{"id": "1", "label": "keep:Keep it"}, {"id": "2", "label": "Drop it"}])
    );
    // Ordinary labels with a colon stay whole.
    assert_eq!(
        recorded_choices(&["--choice", "2:1 split", "--choice", "Even split"]),
        json!([{"id": "1", "label": "2:1 split"}, {"id": "2", "label": "Even split"}])
    );
    assert_eq!(
        recorded_choices(&["--choice", "v1:beta", "--choice", "v2"]),
        json!([{"id": "1", "label": "v1:beta"}, {"id": "2", "label": "v2"}])
    );
    assert_eq!(
        recorded_choices(&["--choice", "10:30 UTC daily", "--choice", "On demand"]),
        json!([{"id": "1", "label": "10:30 UTC daily"}, {"id": "2", "label": "On demand"}])
    );
    // A URL is never `id:label`, even when every value is one.
    assert_eq!(
        recorded_choices(&[
            "--choice",
            "https://api.example.test/v1",
            "--choice",
            "https://api.example.test/v2"
        ]),
        json!([
            {"id": "1", "label": "https://api.example.test/v1"},
            {"id": "2", "label": "https://api.example.test/v2"}
        ])
    );
}

/// A duplicate id in the older form is refused naming both flags; a call
/// whose every value looks like `id:label` but holds an id the pattern
/// refuses is refused at the flag, never with a JSON pointer; a call where
/// some value is a plain label records the shaped one whole. Nothing is
/// recorded on a refusal.
#[test]
fn a_duplicate_or_invalid_id_in_the_older_form_is_refused_at_the_flag() {
    let fresh = TempDir::new().unwrap();
    let f = fresh.path();
    let (code, _, stderr) = decisions_bin(
        f,
        &[
            "add",
            "--title",
            "Dup",
            "--choice",
            "two:Two",
            "--choice",
            "two:Second",
        ],
    );
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(
            "--choice \"two:Second\": the id \"two\" is already taken by the earlier --choice \"two:Two\""
        ),
        "{}",
        stderr
    );

    let (code, _, stderr) = decisions_bin(
        f,
        &[
            "add",
            "--title",
            "Bad",
            "--choice",
            "reads4_write1:Four reads, one write",
            "--choice",
            "reads_only:Reads only",
        ],
    );
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(
            "decisions: --choice \"reads4_write1:Four reads, one write\": every --choice in this call looks like id:label but \"reads4_write1\" does not match ^[a-z0-9][a-z0-9-]*$; drop the ids to have the choices numbered, or fix the id"
        ),
        "{}",
        stderr
    );
    assert!(!stderr.contains("/choices/"), "{}", stderr);
    assert_nothing_recorded(f);

    // One plain label in the call: every value is a label, the shaped one
    // whole, and nothing is refused.
    assert_eq!(
        recorded_choices(&[
            "--choice",
            "reads4_write1:Four reads, one write",
            "--choice",
            "Reads only"
        ]),
        json!([
            {"id": "1", "label": "reads4_write1:Four reads, one write"},
            {"id": "2", "label": "Reads only"}
        ])
    );
    // A space after the colon is a label's colon, not the older form.
    assert_eq!(
        recorded_choices(&["--choice", "Paginate: cursor", "--choice", "Offset: limit"]),
        json!([
            {"id": "1", "label": "Paginate: cursor"},
            {"id": "2", "label": "Offset: limit"}
        ])
    );
}

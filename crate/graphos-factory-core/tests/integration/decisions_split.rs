//! `decisions migrate --split` (ADR 0113 §5): one pass that leaves
//! decisions.json holding decisions only. Provenance moves onto the entry
//! it shadows (a hand-written context carried, the template dropped) or,
//! uncited, becomes a finding; an upstream record becomes a finding; a
//! record with a question, choices or an editorial omit stays, whatever its
//! title; everything else is sorted by hand through `--sorted`, and the
//! split refuses while one is unassigned.

use serde_json::{json, Value};
use std::path::Path;
use tempfile::TempDir;

const SELECTION: &str = "contract_version: 1
operations:
  \"get:/widgets\":
    include: true
    graphql: { root: query, name: listWidgets }

overrides:
  - key: \"get:/widgets\"
    reason: \"limit first\"
    decision: D-0002
    assert:
      - arg: limit
  - key: Query.widget_co_ping
    reason: \"ping is a health check\"
    decision: D-0003
    assert:
      - contains: ping
links:
  - shape: Widget
    path: owner_id
    operation: \"get:/owners/{id}\"
    include: true
    confirmed: true
    reason: \"the owner\"
    decision: D-0010
";

const SOURCES_LOCK: &str = "contract_version: 1
sources:
  - kind: openapi
    version: \"3.0.3\"
    path: openapi.json
    upstream: .factory/sources/openapi.upstream.json
    patches:
      - op: add
        path: \"/info/x-a\"
        value: true
        reason: \"live API says so\"
        decision: D-0004
      - op: add
        path: \"/info/x-b\"
        value: true
        reason: \"live API says so\"
        decision: D-0004
";

const MEMORY: &str = "# Memory\n\n## Vendor\n\n- the API rate-limits at 10/s\n";

fn log() -> Value {
    let resolved = |id: &str, title: &str, extra: Value| {
        let mut r = json!({"id": id, "title": title, "status": "resolved", "date": "2026-09-08",
            "resolution": {"decision": format!("{} decided.", id)}});
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
    };
    json!({"contract_version": 1, "decisions": [
        resolved("D-0001", "Pilot scope: one operation", json!({"context": "Scope prose."})),
        resolved("D-0002", "Hand edit codified: get:/widgets", json!({
            "context": "The engineer edited get:/widgets (root field at line 3) in widget-co.graphql by hand; `graphos-factory-core reconcile` reported it as a hand edit against applied.lock.yaml.",
            "affects": ["get:/widgets"]})),
        resolved("D-0003", "Hand edit codified: Query.widget_co_ping", json!({
            "context": "Ops asked for a ping that never touches the vendor.",
            "affects": ["Query.widget_co_ping"]})),
        resolved("D-0004", "Source patch: openapi.json", json!({
            "context": "Measured against the seeded instance on 2026-09-10.",
            "affects": ["add /info/x-a", "add /info/x-b"]})),
        resolved("D-0005", "Conformance waiver: tests/fixtures/mappings/gone.json", json!({
            "context": "`graphos-factory-core validate` reports 1 body at tests/fixtures/mappings/gone.json as unchecked.",
            "affects": ["get:/widgets"]})),
        resolved("D-0006", "Upstream refreshed: openapi.json", json!({
            "context": "`sources refresh` replaced the vendor's pinned copy (upstream_sha256 aaa) with openapi.json (upstream_sha256 bbb).",
            "affects": ["openapi.json"]})),
        resolved("D-0007", "Scalar for the int64 slots", json!({
            "question": "String or Int?",
            "choices": [{"id": "string", "label": "String"}, {"id": "int", "label": "Int"}]})),
        resolved("D-0008", "Hand edit codified: editorial omits an agent added", json!({
            "omits": [{"operation": "get:/widgets", "direction": "response", "path": "internal", "reason": "editorial"}]})),
        resolved("D-0009", "listWidgets(limit) drops an explicit null", json!({
            "null_handling": [{"operation": "get:/widgets", "argument": "limit", "behavior": "omit"}]})),
        resolved("D-0010", "The owner link stays", json!({})),
        resolved("D-0011", "errors[] is consumed by the error mapping", json!({
            "omits": [{"operation": "get:/widgets", "direction": "response", "path": "errors", "reason": "consumed"}]})),
        resolved("D-0012", "Moving to the newer pins failed", json!({})),
    ]})
}

const SORTED: &str = "D-0001:
  as: decision
  question: \"Which operations does the pilot cover?\"
  choices:
    - { id: one, label: \"The one list operation\" }
    - { id: all, label: \"Every operation\" }
  chosen: [one]
D-0007:
  as: decision
  chosen: [string]
D-0009:
  as: decision
  question: \"What does listWidgets send for an explicit null limit?\"
  choices:
    - { id: omit, label: \"Leave the key out\" }
    - { id: send-null, label: \"Send null\" }
  chosen: [omit]
  by: user
D-0010:
  as: drop
D-0011:
  as: finding
  cites: \"ADR 0103\"
D-0012:
  as: memory
  line: \"Moving to the newer pins fails on two v0.4-only constructs (was D-0012).\"
";

fn workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w(
        ".factory/decisions.json",
        &graphos_factory_core::json::pretty(&log()),
    );
    w(".factory/selection.yaml", SELECTION);
    w(".factory/sources.lock.yaml", SOURCES_LOCK);
    w(".factory/memory.md", MEMORY);
    w("sorted.yaml", SORTED);
    dir
}

fn sf(args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn read(d: &Path, rel: &str) -> String {
    std::fs::read_to_string(d.join(rel)).unwrap()
}

/// Every `.factory` file the split may write, for "nothing written" checks.
fn snapshot(d: &Path) -> Vec<Option<String>> {
    [
        ".factory/decisions.json",
        ".factory/findings.json",
        ".factory/selection.yaml",
        ".factory/sources.lock.yaml",
        ".factory/memory.md",
    ]
    .iter()
    .map(|r| std::fs::read_to_string(d.join(r)).ok())
    .collect()
}

#[test]
fn without_sorted_the_split_lists_what_only_a_person_can_sort_and_writes_nothing() {
    let ws = workspace();
    let d = ws.path();
    let before = snapshot(d);
    let (code, stdout, stderr) = sf(&[
        "decisions",
        "migrate",
        d.to_str().unwrap(),
        "--split",
        "--json",
    ]);
    assert_eq!(code, 1, "{}", stderr);
    let out: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(out["code"], "unsorted");
    let ids: Vec<&str> = out["unsorted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    // D-0008 is titled like provenance but carries an editorial omit: kept,
    // never listed. D-0009 and D-0011 carry payloads: never provenance.
    assert_eq!(ids, vec!["D-0001", "D-0009", "D-0010", "D-0011", "D-0012"]);
    assert!(
        stderr.contains("D-0012  Moving to the newer pins failed"),
        "{}",
        stderr
    );
    assert_eq!(snapshot(d), before);
}

#[test]
fn the_split_moves_provenance_records_findings_and_sorts_the_rest() {
    let ws = workspace();
    let d = ws.path();
    let sorted = d.join("sorted.yaml");

    // A dry run computes everything and writes nothing.
    let before = snapshot(d);
    let (code, stdout, stderr) = sf(&[
        "decisions",
        "migrate",
        d.to_str().unwrap(),
        "--split",
        "--sorted",
        sorted.to_str().unwrap(),
        "--dry-run",
        "--json",
    ]);
    assert_eq!(code, 0, "{}", stderr);
    let plan: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(plan["dry_run"], true);
    assert_eq!(
        plan["counts"],
        json!({"kept": 1, "decision": 3, "moved": 3, "finding": 3, "memory": 1, "dropped": 1})
    );
    assert_eq!(snapshot(d), before);

    let (code, stdout, stderr) = sf(&[
        "decisions",
        "migrate",
        d.to_str().unwrap(),
        "--split",
        "--sorted",
        sorted.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(code, 0, "{}", stderr);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    let outcome = |id: &str| -> Value {
        report["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(outcome("D-0002")["outcome"], "moved");
    assert_eq!(outcome("D-0005")["outcome"], "finding");
    assert_eq!(outcome("D-0005")["finding"], "F-0001");
    assert_eq!(outcome("D-0006")["finding"], "F-0002");
    assert_eq!(outcome("D-0011")["finding"], "F-0003");
    assert_eq!(outcome("D-0008")["outcome"], "kept");
    // The link's reference to the dropped record is removed, and said so.
    assert!(
        report["warnings"]
            .to_string()
            .contains("`decision: D-0010` named a record that is now dropped"),
        "{}",
        report["warnings"]
    );

    // decisions.json: decisions only, ids preserved, each with its alternative.
    let decisions: Value =
        graphos_factory_core::json::parse(&read(d, ".factory/decisions.json")).unwrap();
    let recs = decisions["decisions"].as_array().unwrap();
    let ids: Vec<&str> = recs.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["D-0001", "D-0007", "D-0008", "D-0009"]);
    assert_eq!(
        recs[0]["question"],
        "Which operations does the pilot cover?"
    );
    assert_eq!(recs[0]["resolution"]["chosen"], json!(["one"]));
    assert_eq!(
        recs[0]["resolution"]["by"], "agent",
        "absent by is the agent's"
    );
    assert_eq!(
        recs[0]["resolution"]["decision"], "D-0001 decided.",
        "prose kept"
    );
    assert_eq!(recs[1]["resolution"]["chosen"], json!(["string"]));
    assert_eq!(recs[3]["resolution"]["by"], "user");
    assert_eq!(
        recs[3]["null_handling"][0]["behavior"], "omit",
        "payload kept"
    );

    // findings.json: fresh ids, in record order, with their sources.
    let findings: Value =
        graphos_factory_core::json::parse(&read(d, ".factory/findings.json")).unwrap();
    let f = findings["findings"].as_array().unwrap();
    assert_eq!(f.len(), 3);
    assert_eq!(f[0]["source"], "codify");
    assert_eq!(
        f[0]["title"],
        "Conformance waiver: tests/fixtures/mappings/gone.json"
    );
    assert_eq!(f[0]["evidence"], json!(["get:/widgets"]));
    assert_eq!(f[1]["source"], "sources");
    assert!(f[1]["body"]
        .as_str()
        .unwrap()
        .contains("upstream_sha256 bbb"));
    assert_eq!(f[2]["source"], "agent");
    assert_eq!(f[2]["cites"], "ADR 0103");
    assert_eq!(
        f[2]["omits"],
        json!([{"operation": "get:/widgets", "direction": "response", "path": "errors", "reason": "consumed"}])
    );

    // selection.yaml: version 2; the template context is not carried, the
    // hand-written one is, and every reference to a moved or dropped record
    // is gone. Nothing else moves.
    let selection = read(d, ".factory/selection.yaml");
    let expected = SELECTION
        .replace("contract_version: 1", "contract_version: 2")
        .replace("    decision: D-0002\n", "")
        .replace(
            "    decision: D-0003\n",
            "    context: \"Ops asked for a ping that never touches the vendor.\"\n",
        )
        .replace("    decision: D-0010\n", "");
    assert_eq!(selection, expected);
    let lock = read(d, ".factory/sources.lock.yaml");
    assert_eq!(
        lock,
        SOURCES_LOCK.replace(
            "        decision: D-0004\n",
            "        context: \"Measured against the seeded instance on 2026-09-10.\"\n"
        )
    );
    // memory.md: the line under a new `## Tried and rejected`.
    assert_eq!(
        read(d, ".factory/memory.md"),
        format!(
            "{}\n## Tried and rejected\n\n- Moving to the newer pins fails on two v0.4-only constructs (was D-0012).\n",
            MEMORY
        )
    );

    // A second run finds nothing left to sort and changes nothing.
    let after = snapshot(d);
    let (code, stdout, stderr) = sf(&[
        "decisions",
        "migrate",
        d.to_str().unwrap(),
        "--split",
        "--json",
    ]);
    assert_eq!(code, 0, "{}", stderr);
    let again: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(again["counts"]["kept"], 4);
    assert_eq!(snapshot(d), after);
}

#[test]
fn a_sorted_entry_the_split_cannot_honour_is_refused_and_nothing_is_written() {
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "a finding cannot hold null_handling",
            SORTED.replace(
                "D-0009:\n  as: decision\n",
                "D-0009:\n  as: finding\n  x-unused: 1\n",
            ),
            "unknown key",
        ),
        (
            "a finding cannot hold null_handling",
            SORTED.replace("D-0009:\n  as: decision\n", "D-0009:\n  as: finding\n"),
            "carries null_handling, which only a decision holds",
        ),
        (
            "a decision with no alternative",
            SORTED.replace(
                "  question: \"Which operations does the pilot cover?\"\n  choices:\n    - { id: one, label: \"The one list operation\" }\n    - { id: all, label: \"Every operation\" }\n  chosen: [one]\n",
                "",
            ),
            "no-alternative",
        ),
        (
            "a chosen id that is not a choice",
            SORTED.replace("  chosen: [one]\n", "  chosen: [three]\n"),
            "is not one of its choices",
        ),
        (
            "a provenance record is the split's to sort",
            format!("{}D-0002:\n  as: drop\n", SORTED),
            "the split sorts this record itself (provenance)",
        ),
        (
            "an id the log does not record",
            format!("{}D-0099:\n  as: drop\n", SORTED),
            "which decisions.json does not record",
        ),
        (
            "memory needs a line",
            SORTED.replace(
                "  line: \"Moving to the newer pins fails on two v0.4-only constructs (was D-0012).\"\n",
                "",
            ),
            "needs `line`",
        ),
        (
            "an omit an instrument reads cannot be dropped",
            SORTED.replace("D-0011:\n  as: finding\n", "D-0011:\n  as: drop\n"),
            "carries omits an instrument reads",
        ),
    ];
    for (case, sorted, says) in cases {
        let ws = workspace();
        let d = ws.path();
        std::fs::write(d.join("sorted.yaml"), &sorted).unwrap();
        let before = snapshot(d);
        let (code, stdout, stderr) = sf(&[
            "decisions",
            "migrate",
            d.to_str().unwrap(),
            "--split",
            "--sorted",
            d.join("sorted.yaml").to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(code, 1, "{}: {}{}", case, stdout, stderr);
        assert!(stderr.contains(says), "{}: {}", case, stderr);
        let out: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(out["code"], "split-refused", "{}", case);
        assert_eq!(snapshot(d), before, "{}: nothing is written", case);
    }
}

#[test]
fn the_template_openings_are_the_binarys_own() {
    // The split drops a moved record's context when it opens with the
    // sentence codify used to write (ADR 0113 §5); a context that opens any
    // other way is the engineer's and is carried.
    use graphos_factory_core::split::{classify, Class, Provenance};
    let rec = |title: &str, extra: Value| {
        let mut r = json!({"id": "D-0001", "title": title, "status": "resolved", "date": "d", "resolution": {"decision": "x"}});
        for (k, v) in extra.as_object().unwrap() {
            r[k] = v.clone();
        }
        r
    };
    assert_eq!(
        classify(&rec("Hand edit codified: get:/x", json!({}))),
        Class::Provenance(Provenance::Override)
    );
    assert_eq!(
        classify(&rec("Source patch: openapi.json", json!({}))),
        Class::Provenance(Provenance::Patch)
    );
    assert_eq!(
        classify(&rec("Conformance waiver: t.json", json!({}))),
        Class::Provenance(Provenance::Waiver)
    );
    assert_eq!(
        classify(&rec("Upstream replaced: x", json!({}))),
        Class::Sources
    );
    assert_eq!(
        classify(&rec(
            "Hand edit codified: get:/x",
            json!({"question": "Q?"})
        )),
        Class::Kept
    );
    assert_eq!(
        classify(&rec(
            "Source patch: x",
            json!({"omits": [{"operation": "get:/x", "direction": "response", "path": "a", "reason": "consumed"}]})
        )),
        Class::Hand,
        "a payload is never provenance"
    );
    assert_eq!(
        classify(&rec("Conformance waivers: plural", json!({}))),
        Class::Hand
    );
}

#[test]
fn memory_lines_land_under_an_existing_tried_and_rejected_section() {
    let text = "# M\n\n## Tried and rejected\n\n- earlier\n\n## Vendor\n\n- quirk\n";
    let out =
        graphos_factory_core::split::append_tried_and_rejected(text, &["new one".to_string()]);
    assert_eq!(
        out,
        "# M\n\n## Tried and rejected\n\n- earlier\n- new one\n\n## Vendor\n\n- quirk\n"
    );
    let empty = graphos_factory_core::split::append_tried_and_rejected("", &["x".to_string()]);
    assert_eq!(empty, "## Tried and rejected\n\n- x\n");
}

#[test]
fn the_version_bump_keeps_quotes_spacing_and_comments() {
    use graphos_factory_core::split::bump_selection_version as bump;
    assert_eq!(
        bump("contract_version: 1\nx: 1\n"),
        "contract_version: 2\nx: 1\n"
    );
    assert_eq!(
        bump("contract_version: 1  # v\ncontract_version: 1\n"),
        "contract_version: 2  # v\ncontract_version: 1\n",
        "only the first, and the comment kept"
    );
    assert_eq!(bump("contract_version: \"1\"\n"), "contract_version: 2\n");
    assert_eq!(
        bump("contract_version:   '1'\t# q\r\n"),
        "contract_version:   2\t# q\r\n"
    );
    assert_eq!(bump("contract_version: 2\n"), "contract_version: 2\n");
    assert_eq!(
        bump("  contract_version: 1\n"),
        "  contract_version: 1\n",
        "column 0 only"
    );
    assert_eq!(bump("contract_version: 10\n"), "contract_version: 10\n");
}

/// Before ADR 0113 any resolved decision a `links:` entry named kept the
/// link; now only a resolved `keep` does. The split warns, on a dry run
/// too, for each included entry whose decision chooses neither.
#[test]
fn the_split_warns_when_a_links_decision_chooses_neither_keep_nor_drop() {
    let ws = workspace();
    let d = ws.path();
    // The link names D-0007, a kept decision whose answer is `string`.
    let selection = SELECTION.replace("    decision: D-0010\n", "    decision: D-0007\n");
    std::fs::write(d.join(".factory/selection.yaml"), &selection).unwrap();
    let sorted = d.join("sorted.yaml");
    let (code, stdout, stderr) = sf(&[
        "decisions",
        "migrate",
        d.to_str().unwrap(),
        "--split",
        "--sorted",
        sorted.to_str().unwrap(),
        "--dry-run",
        "--json",
    ]);
    assert_eq!(code, 0, "{}", stderr);
    let plan: Value = serde_json::from_str(&stdout).unwrap();
    let warnings = plan["warnings"].to_string();
    assert!(
        warnings.contains(
            "links entry Widget > owner_id: decision: D-0007 chooses neither keep nor drop"
        ),
        "{}",
        warnings
    );
    assert!(
        warnings.contains("decisions reopen . --id D-0007"),
        "{}",
        warnings
    );
    assert!(warnings.contains("--chosen keep"), "{}", warnings);
    assert!(
        stderr.contains("chooses neither keep nor drop"),
        "{}",
        stderr
    );

    // A `keep` answer draws nothing.
    let mut log = log();
    log["decisions"][6]["resolution"]["chosen"] = json!(["keep"]);
    log["decisions"][6]["choices"] =
        json!([{"id": "keep", "label": "Keep"}, {"id": "drop", "label": "Drop"}]);
    std::fs::write(
        d.join(".factory/decisions.json"),
        graphos_factory_core::json::pretty(&log),
    )
    .unwrap();
    let sorted_text = SORTED.replace("D-0007:\n  as: decision\n  chosen: [string]\n", "");
    std::fs::write(&sorted, sorted_text).unwrap();
    let (code, stdout, stderr) = sf(&[
        "decisions",
        "migrate",
        d.to_str().unwrap(),
        "--split",
        "--sorted",
        sorted.to_str().unwrap(),
        "--dry-run",
        "--json",
    ]);
    assert_eq!(code, 0, "{}", stderr);
    let plan: Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        !plan["warnings"].to_string().contains("chooses neither"),
        "{}",
        plan["warnings"]
    );
}

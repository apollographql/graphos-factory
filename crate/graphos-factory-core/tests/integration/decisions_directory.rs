//! ADR 0118: every decision or finding added is its own file with a random
//! id, beside the single `decisions.json` / `findings.json` an existing
//! workspace keeps, whose records are never migrated.

use graphos_factory_core::{decisions as log, findings, json, record_log};
use serde_json::{json, Value};
use std::path::Path;
use tempfile::TempDir;

fn decisions(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec![args[0].to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args[1..].iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::decisions::main(&argv)
}

fn findings_cmd(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec![args[0].to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args[1..].iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::findings::main(&argv)
}

fn add(dir: &Path, title: &str, extra: &[&str]) -> i32 {
    let mut args = vec![
        "add",
        "--title",
        title,
        "--question",
        "Which way?",
        "--date",
        "2026-10-05",
    ];
    args.extend_from_slice(extra);
    decisions(dir, &args)
}

fn ids(doc: &Value) -> Vec<String> {
    json::get_arr(doc, "decisions")
        .into_iter()
        .flatten()
        .map(|r| json::get_str(r, "id").unwrap().to_string())
        .collect()
}

fn files(dir: &Path, sub: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join(".factory").join(sub))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A workspace with a record directory and no records yet.
fn migrated() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".factory/decisions")).unwrap();
    dir
}

/// An existing workspace's log, written before ADR 0118: two numbered
/// records in `decisions.json`, the second resolved on `get:/a`.
const OLD_LOG: &str = r#"{
  "contract_version": 1,
  "decisions": [
    {
      "id": "D-0001",
      "title": "Old open question",
      "status": "open",
      "date": "2026-09-01",
      "question": "Which way?"
    },
    {
      "id": "D-0002",
      "title": "Old answer",
      "status": "resolved",
      "date": "2026-09-02",
      "question": "Which way?",
      "affects": [
        "get:/a"
      ],
      "resolution": {
        "note": "this way",
        "by": "agent"
      }
    }
  ]
}
"#;

fn existing() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join(".factory")).unwrap();
    std::fs::write(dir.path().join(".factory/decisions.json"), OLD_LOG).unwrap();
    dir
}

fn old_file(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".factory/decisions.json")).unwrap()
}

fn write_record(dir: &Path, name: &str, rec: Value) {
    let path = dir.join(".factory/decisions").join(name);
    let text = json::pretty(&json!({"contract_version": 2, "decision": rec}));
    std::fs::write(path, text).unwrap();
}

fn resolved(id: &str, date: &str, affects: &[&str], extra: Value) -> Value {
    let mut rec = json!({
        "id": id,
        "title": format!("decision {}", id),
        "status": "resolved",
        "date": date,
        "question": "Which way?",
        "affects": affects,
        "resolution": {"note": "this way", "by": "agent"}
    });
    if let Value::Object(m) = extra {
        for (k, v) in m {
            rec[k] = v;
        }
    }
    rec
}

#[test]
fn a_new_decision_beside_old_ones_is_its_own_file_and_the_old_file_is_untouched() {
    let dir = existing();
    let d = dir.path();
    assert_eq!(
        add(
            d,
            "New question",
            &["--after", "D-0002", "--affects", "get:/a"]
        ),
        0
    );
    assert_eq!(
        old_file(d),
        OLD_LOG,
        "adding a record never touches decisions.json"
    );
    let names = files(d, "decisions");
    assert_eq!(names.len(), 1, "{:?}", names);
    let doc = log::load(d, None).unwrap();
    let all = ids(&doc);
    assert_eq!(
        all[..2],
        ["D-0001", "D-0002"],
        "old records first, in file order"
    );
    assert!(record_log::is_random(&all[2]), "{}", all[2]);
    assert!(
        names[0].starts_with(&format!("{}-new-question", all[2])),
        "{:?}",
        names
    );
    // A new record may name an old one; the edge lives on the new record.
    let new = &json::get_arr(&doc, "decisions").unwrap()[2];
    assert_eq!(new["after"], json!(["D-0002"]));
}

#[test]
fn an_old_decision_changes_in_place_and_its_file_stays_version_1() {
    let dir = existing();
    let d = dir.path();
    assert_eq!(add(d, "New one", &[]), 0);
    let new_file = files(d, "decisions")[0].clone();
    let new_bytes = std::fs::read(d.join(".factory/decisions").join(&new_file)).unwrap();
    assert_eq!(
        decisions(
            d,
            &[
                "resolve",
                "--id",
                "D-0001",
                "--note",
                "settled",
                "--at",
                "2026-10-05"
            ]
        ),
        0
    );
    let after = json::parse(&old_file(d)).unwrap();
    assert_eq!(after["contract_version"], 1);
    let recs = json::get_arr(&after, "decisions").unwrap();
    assert_eq!(recs.len(), 2, "the new record never moves into the file");
    assert_eq!(recs[0]["status"], "resolved");
    let before = json::parse(OLD_LOG).unwrap();
    assert_eq!(
        recs[1], before["decisions"][1],
        "the other old record is untouched"
    );
    assert_eq!(
        std::fs::read(d.join(".factory/decisions").join(&new_file)).unwrap(),
        new_bytes,
        "nor is the new record's file"
    );
    assert_eq!(decisions(d, &["reopen", "--id", "D-0001"]), 0);
    assert_eq!(decisions(d, &["supersede", "--id", "D-0002"]), 0);
    let after = json::parse(&old_file(d)).unwrap();
    assert_eq!(after["decisions"][1]["status"], "superseded");
}

#[test]
fn an_old_decision_never_gains_a_field_only_a_new_one_carries() {
    let dir = existing();
    let d = dir.path();
    let mut doc = log::load(d, None).unwrap();
    doc["decisions"][1]["after"] = json!(["D-0001"]);
    let err = log::save(d, &doc, None).unwrap_err();
    assert!(
        err.contains("put a new edge or slug on a new record"),
        "{}",
        err
    );
    assert_eq!(old_file(d), OLD_LOG);
}

#[test]
fn a_workspace_with_no_log_starts_with_record_files() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(add(d, "First", &[]), 0);
    assert!(!d.join(".factory/decisions.json").exists());
    assert_eq!(files(d, "decisions").len(), 1);
}

#[test]
fn new_decisions_get_random_ids_in_one_file_each() {
    let dir = migrated();
    let d = dir.path();
    assert_eq!(add(d, "Paginate by cursor, not page", &[]), 0);
    assert_eq!(add(d, "Does createIssue need assignees?", &[]), 0);
    let names = files(d, "decisions");
    assert_eq!(names.len(), 2, "{:?}", names);
    let doc = log::load(d, None).unwrap();
    assert_eq!(doc["contract_version"], 2);
    for (id, name) in ids(&doc).iter().zip(&names) {
        assert!(record_log::is_random(id), "{}", id);
        assert_eq!(id.len(), 8, "{}", id);
        assert!(
            names.iter().any(|n| n.starts_with(&format!("{}-", id))),
            "{} in {:?}",
            id,
            names
        );
        let text = std::fs::read_to_string(d.join(".factory/decisions").join(name)).unwrap();
        let file = json::parse(&text).unwrap();
        assert_eq!(file["contract_version"], 2);
        assert!(file["decision"]["id"].is_string());
    }
    assert!(
        names
            .iter()
            .any(|n| n.ends_with("-paginate-by-cursor-not-page.json")),
        "{:?}",
        names
    );
    assert!(!d.join(".factory/decisions.json").exists());
}

#[test]
fn two_branches_that_each_add_a_decision_merge_as_two_files() {
    // Each branch is a copy of one migrated workspace with one record; the
    // merge is the union of the two directories, file by file.
    let base = migrated();
    assert_eq!(add(base.path(), "Base", &["--affects", "get:/a"]), 0);
    let branch = |title: &str, span: &str| {
        let b = TempDir::new().unwrap();
        std::fs::create_dir_all(b.path().join(".factory/decisions")).unwrap();
        for n in files(base.path(), "decisions") {
            std::fs::copy(
                base.path().join(".factory/decisions").join(&n),
                b.path().join(".factory/decisions").join(&n),
            )
            .unwrap();
        }
        assert_eq!(add(b.path(), title, &["--affects", span]), 0);
        b
    };
    let ours = branch("Ours", "get:/ours");
    let theirs = branch("Theirs", "get:/theirs");
    let merged = migrated();
    for b in [&ours, &theirs] {
        for n in files(b.path(), "decisions") {
            let to = merged.path().join(".factory/decisions").join(&n);
            let bytes = std::fs::read(b.path().join(".factory/decisions").join(&n)).unwrap();
            if to.exists() {
                assert_eq!(
                    std::fs::read(&to).unwrap(),
                    bytes,
                    "{} differs between the branches",
                    n
                );
            }
            std::fs::write(to, bytes).unwrap();
        }
    }
    let doc = log::load(merged.path(), None).unwrap();
    assert_eq!(ids(&doc).len(), 3);
    assert!(record_log::duplicate_ids(&doc, "decisions").is_empty());
    assert!(
        log::unordered_overlaps(&doc, true).is_empty(),
        "different spans do not overlap"
    );
}

#[test]
fn the_same_span_from_two_branches_is_an_overlap_until_one_names_the_other() {
    let dir = migrated();
    let d = dir.path();
    write_record(
        d,
        "D-a1b2c3-x.json",
        resolved("D-a1b2c3", "2026-10-01", &["post:/issues"], json!({})),
    );
    write_record(
        d,
        "D-z9y8x7-y.json",
        resolved("D-z9y8x7", "2026-10-02", &["post:/issues"], json!({})),
    );
    let doc = log::load(d, None).unwrap();
    assert_eq!(
        log::unordered_overlaps(&doc, true),
        [(
            "D-a1b2c3".to_string(),
            "D-z9y8x7".to_string(),
            "post:/issues".to_string()
        )]
    );
    write_record(
        d,
        "D-z9y8x7-y.json",
        resolved(
            "D-z9y8x7",
            "2026-10-02",
            &["post:/issues"],
            json!({"amends": ["D-a1b2c3"]}),
        ),
    );
    let doc = log::load(d, None).unwrap();
    assert!(log::unordered_overlaps(&doc, true).is_empty());
}

#[test]
fn two_numbered_records_are_never_an_overlap() {
    let doc = json!({"contract_version": 1, "decisions": [
        resolved("D-0001", "2026-01-01", &["get:/a"], json!({})),
        resolved("D-0002", "2026-01-02", &["get:/a"], json!({})),
    ]});
    assert!(log::unordered_overlaps(&doc, true).is_empty());
    assert_eq!(
        log::unordered_overlaps(&doc, false).len(),
        1,
        "the exemption is what keeps it quiet"
    );
}

/// Of a numbered and a random record, the random one is always the newer,
/// whatever the dates say (a record file's `date` is whatever `--date` or
/// a hand said): so lint's fix, `link --id <newer> --after <older>`, puts
/// the edge on the record that may carry one, and it runs.
#[test]
fn a_random_record_is_newer_than_a_numbered_one_whatever_its_date() {
    let dir = existing();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory/decisions")).unwrap();
    write_record(
        d,
        "D-mrqsct-early.json",
        resolved("D-mrqsct", "2026-08-01", &["get:/a"], json!({})),
    );
    let doc = log::load(d, None).unwrap();
    assert_eq!(
        log::unordered_overlaps(&doc, true),
        [(
            "D-0002".to_string(),
            "D-mrqsct".to_string(),
            "get:/a".to_string()
        )]
    );
    assert_eq!(
        decisions(d, &["link", "--id", "D-mrqsct", "--after", "D-0002"]),
        0
    );
    assert!(log::unordered_overlaps(&log::load(d, None).unwrap(), true).is_empty());
    assert_eq!(old_file(d), OLD_LOG);
}

#[test]
fn a_hand_merged_duplicate_id_is_read_but_never_written() {
    let dir = migrated();
    let d = dir.path();
    write_record(
        d,
        "D-a1b2c3-one.json",
        resolved("D-a1b2c3", "2026-10-01", &[], json!({})),
    );
    write_record(
        d,
        "D-a1b2c3-two.json",
        resolved("D-a1b2c3", "2026-10-02", &[], json!({})),
    );
    let doc = log::load(d, None).unwrap();
    assert_eq!(record_log::duplicate_ids(&doc, "decisions"), ["D-a1b2c3"]);
    let err = log::save(d, &doc, None).unwrap_err();
    assert!(err.contains("D-a1b2c3 twice"), "{}", err);
    assert_eq!(
        add(d, "Anything", &[]),
        1,
        "every write refuses the log until one is renamed"
    );
}

#[test]
fn the_single_file_writer_refuses_a_duplicate_too() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    let doc = json!({"contract_version": 1, "decisions": [
        resolved("D-0001", "2026-01-01", &[], json!({})),
        resolved("D-0001", "2026-01-02", &[], json!({})),
    ]});
    let err = log::save(d, &doc, None).unwrap_err();
    assert!(err.contains("D-0001 twice"), "{}", err);
    assert!(!d.join(".factory/decisions.json").exists());
}

#[test]
fn a_record_file_named_for_another_id_is_refused() {
    let dir = migrated();
    let d = dir.path();
    write_record(
        d,
        "D-a1b2c3-x.json",
        resolved("D-z9y8x7", "2026-10-01", &[], json!({})),
    );
    let err = log::load(d, None).unwrap_err();
    assert!(err.contains("does not begin with that id"), "{}", err);
}

#[test]
fn a_stray_file_in_the_directory_is_refused_and_a_dot_file_ignored() {
    let dir = migrated();
    let d = dir.path();
    std::fs::write(d.join(".factory/decisions/.DS_Store"), "x").unwrap();
    assert!(log::load(d, None).is_ok());
    std::fs::write(d.join(".factory/decisions/notes.txt"), "x").unwrap();
    let err = log::load(d, None).unwrap_err();
    assert!(err.contains("notes.txt"), "{}", err);
}

#[test]
fn a_dangling_or_circular_edge_is_refused() {
    let dir = migrated();
    let d = dir.path();
    assert_eq!(add(d, "Names a ghost", &["--after", "D-gh0st1"]), 1);
    assert!(files(d, "decisions").is_empty());
    write_record(
        d,
        "D-a1b2c3-a.json",
        resolved(
            "D-a1b2c3",
            "2026-10-01",
            &[],
            json!({"after": ["D-z9y8x7"]}),
        ),
    );
    write_record(
        d,
        "D-z9y8x7-b.json",
        resolved(
            "D-z9y8x7",
            "2026-10-02",
            &[],
            json!({"after": ["D-a1b2c3"]}),
        ),
    );
    let doc = log::load(d, None).unwrap();
    assert_eq!(
        log::link_cycle(&doc).unwrap(),
        ["D-a1b2c3", "D-z9y8x7", "D-a1b2c3"]
    );
    let err = log::save(d, &doc, None).unwrap_err();
    assert!(err.contains("cycle"), "{}", err);
    assert_eq!(decisions(d, &["list", "--causal"]), 1);
}

#[test]
fn causal_order_follows_edges_then_shared_spans() {
    let dir = migrated();
    let d = dir.path();
    // Dated in the order c, a, b; b presumes c explicitly, and a and b share
    // a span, so the order is c, a, b with b placed by both.
    write_record(
        d,
        "D-cccccc-c.json",
        resolved("D-cccccc", "2026-10-01", &["get:/c"], json!({})),
    );
    write_record(
        d,
        "D-aaaaaa-a.json",
        resolved("D-aaaaaa", "2026-10-02", &["get:/x"], json!({})),
    );
    write_record(
        d,
        "D-bbbbbb-b.json",
        resolved(
            "D-bbbbbb",
            "2026-10-03",
            &["get:/x"],
            json!({"after": ["D-cccccc"]}),
        ),
    );
    let doc = log::load(d, None).unwrap();
    let order = log::causal_order(&doc).unwrap();
    let got: Vec<&str> = order
        .iter()
        .map(|(r, _)| json::get_str(r, "id").unwrap())
        .collect();
    assert_eq!(got, ["D-cccccc", "D-aaaaaa", "D-bbbbbb"]);
    let vias: Vec<&str> = order[2].1.iter().map(|e| e.via.as_str()).collect();
    assert_eq!(vias, ["after", "affects get:/x"]);
    assert_eq!(decisions(d, &["list", "--causal"]), 0);
}

#[test]
fn a_new_finding_is_its_own_file_too() {
    let dir = existing();
    let d = dir.path();
    let old_findings = r#"{
  "contract_version": 1,
  "findings": [
    {
      "id": "F-0001",
      "title": "An old fact",
      "date": "2026-09-01",
      "status": "current",
      "body": "It held.",
      "source": "agent"
    }
  ]
}
"#;
    std::fs::write(d.join(".factory/findings.json"), old_findings).unwrap();
    assert_eq!(
        findings_cmd(
            d,
            &[
                "add",
                "--title",
                "A fact",
                "--body",
                "It holds.",
                "--cites",
                "ADR 0032",
                "--date",
                "2026-10-05"
            ]
        ),
        0
    );
    let names = files(d, "findings");
    assert_eq!(names.len(), 1, "{:?}", names);
    assert!(
        names[0].starts_with("F-") && names[0].ends_with("-a-fact.json"),
        "{:?}",
        names
    );
    assert_eq!(
        std::fs::read_to_string(d.join(".factory/findings.json")).unwrap(),
        old_findings,
        "the old findings file is untouched"
    );
    let all = findings::load(d, None).unwrap();
    assert_eq!(all["findings"][0]["id"], "F-0001");
    assert_eq!(json::get_arr(&all, "findings").unwrap().len(), 2);
}

#[test]
fn split_refuses_a_workspace_that_already_has_record_files() {
    // `--split` (ADR 0113) rewrites the single files whole; run after a new
    // record was added, it would fold that record into decisions.json.
    let dir = existing();
    let d = dir.path();
    assert_eq!(add(d, "New", &[]), 0);
    assert_eq!(decisions(d, &["migrate", "--split", "--dry-run"]), 1);
    assert_eq!(old_file(d), OLD_LOG);
}

/// What the lock and `selection review` hash: the tests that run them are
/// `provenance::a_decision_record_added_after_the_lock_is_provenance_drift`
/// and `selection_review::a_decision_record_file_binds_the_review`.
#[test]
fn record_paths_lists_every_record_file() {
    let dir = migrated();
    let d = dir.path();
    assert_eq!(add(d, "Bound", &[]), 0);
    let paths = record_log::record_paths(d).unwrap();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].starts_with(".factory/decisions/D-"), "{:?}", paths);
}

#[test]
fn link_adds_an_edge_to_a_new_record_and_refuses_an_old_one() {
    let dir = existing();
    let d = dir.path();
    assert_eq!(add(d, "Also on get:/a", &["--affects", "get:/a"]), 0);
    let new = ids(&log::load(d, None).unwrap())[2].clone();
    assert_eq!(
        decisions(d, &["link", "--id", &new, "--amends", "D-0002"]),
        0
    );
    let doc = log::load(d, None).unwrap();
    assert_eq!(doc["decisions"][2]["amends"], json!(["D-0002"]));
    // An old record never gains a field, so the file stays as committed.
    assert_eq!(
        decisions(d, &["link", "--id", "D-0001", "--after", "D-0002"]),
        1
    );
    assert_eq!(old_file(d), OLD_LOG);
    // A dangling edge and an unknown id are refused, and nothing changes.
    let before = std::fs::read_dir(d.join(".factory/decisions"))
        .unwrap()
        .flatten()
        .map(|e| std::fs::read(e.path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        decisions(d, &["link", "--id", &new, "--after", "D-gh0st1"]),
        1
    );
    assert_eq!(
        decisions(d, &["link", "--id", "D-zzzzz9", "--after", "D-0001"]),
        1
    );
    assert_eq!(
        decisions(d, &["link", "--id", &new]),
        1,
        "an edge is required"
    );
    let after = std::fs::read_dir(d.join(".factory/decisions"))
        .unwrap()
        .flatten()
        .map(|e| std::fs::read(e.path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(before, after);
}

/// On the public pilot's real log, `decision-overlap` is quiet only because
/// two numbered records are exempt: without the exemption it would name
/// every pair of resolved decisions that share a span (measured, ADR 0118).
/// The target's own pilots are measured in its suite.
#[test]
fn the_numbered_exemption_is_what_keeps_the_public_pilot_quiet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let doc = log::load(&root.join("pilots/graphos/gitea"), None).unwrap();
    assert!(log::unordered_overlaps(&doc, true).is_empty());
    assert_eq!(log::unordered_overlaps(&doc, false).len(), 108);
}

/// `add --json` names the record file it wrote, so a caller that stages or
/// copies it (the Desktop plugin's proposal path) does not glob for it.
#[test]
fn add_json_prints_the_record_file_it_wrote() {
    let dir = existing();
    let d = dir.path();
    let ws = d.to_str().unwrap();
    let bin = env!("CARGO_BIN_EXE_graphos-factory-bare");
    for (verb, title, log) in [
        ("decisions", "Paginate by cursor", ".factory/decisions/"),
        ("findings", "The cursor is opaque", ".factory/findings/"),
    ] {
        let mut args = vec![verb, "add", ws, "--title", title, "--json"];
        if verb == "decisions" {
            args.extend(["--question", "Which way?", "--date", "2026-10-05"]);
        } else {
            args.extend(["--body", "Seen in the spec.", "--cites", "spec"]);
        }
        let out = std::process::Command::new(bin)
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        let id = report["id"].as_str().unwrap();
        let path = report["path"].as_str().unwrap();
        assert!(path.starts_with(&format!("{}{}-", log, id)), "{}", path);
        assert!(d.join(path).is_file(), "{}", path);
        assert!(record_log::record_paths(d)
            .unwrap()
            .contains(&path.to_string()));
    }
}

/// ADR 0118 §4: a file is rewritten only when one of its records changed.
/// A log file that is valid but not in the binary's own layout (re-indented,
/// compacted, reformatted on save) is left byte for byte, so a verb on one
/// record changes exactly one file; the Desktop plugin's proposal path
/// refuses a run that changes two (found on review of its PR).
#[test]
fn a_verb_on_one_record_leaves_a_reformatted_sibling_file_alone() {
    let dir = existing();
    let d = dir.path();
    assert_eq!(add(d, "New one", &[]), 0);
    let new_file = d.join(".factory/decisions").join(&files(d, "decisions")[0]);
    let new_id = ids(&log::load(d, None).unwrap())[2].clone();

    // decisions.json re-indented to four spaces; resolve the new record.
    let reindented = serde_json::to_string_pretty(&json::parse(&old_file(d)).unwrap())
        .unwrap()
        .replace("\n  ", "\n    ");
    std::fs::write(d.join(".factory/decisions.json"), &reindented).unwrap();
    assert_eq!(
        decisions(
            d,
            &[
                "resolve",
                "--id",
                &new_id,
                "--note",
                "settled",
                "--at",
                "2026-10-05"
            ]
        ),
        0
    );
    assert_eq!(old_file(d), reindented, "decisions.json is not rewritten");
    assert_eq!(
        json::parse(&std::fs::read_to_string(&new_file).unwrap()).unwrap()["decision"]["status"],
        "resolved"
    );

    // The record file compacted onto one line; reopen an old record.
    let compact =
        serde_json::to_string(&json::parse(&std::fs::read_to_string(&new_file).unwrap()).unwrap())
            .unwrap();
    std::fs::write(&new_file, &compact).unwrap();
    assert_eq!(decisions(d, &["reopen", "--id", "D-0002"]), 0);
    assert_eq!(
        std::fs::read_to_string(&new_file).unwrap(),
        compact,
        "the record file is not rewritten"
    );
    assert_eq!(
        json::parse(&old_file(d)).unwrap()["decisions"][1]["status"],
        "open"
    );
}

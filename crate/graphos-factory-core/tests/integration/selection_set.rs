//! `selection set` — flip an operation's include flag in place (ADR 0018).

use std::path::Path;

/// A selection whose first entry puts `response:` before `include:` (as the
/// gitea pilot does), carries comments the edit must not disturb, and includes
/// one already-excluded operation.
const SELECTION: &str = r#"contract_version: 1
defaults:
  fields: all
operations:
  # keep this comment exactly where it is
  "get:/version":
    response:
      envelope: "version"
      confirmed: true
    include: true
    graphql: { root: query, name: version }   # a trailing comment
  "get:/widgets":
    include: true
    graphql: { root: query, name: listWidgets }
  "delete:/widgets/{id}":
    include: false
    reason: "read-only first release"
"#;

fn workspace(selection: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join(".factory/selection.yaml");
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, selection).unwrap();
    dir
}

fn read(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".factory/selection.yaml")).unwrap()
}

fn set(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec!["set".to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::selection::main(&argv)
}

/// The binary itself, so a `--json` refusal is observed exactly as a machine
/// caller (the Desktop panel) sees it on stdout.
fn set_json(dir: &Path, args: &[&str]) -> (i32, serde_json::Value) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("selection")
        .arg("set")
        .arg(dir.to_string_lossy().to_string())
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let json = graphos_factory_core::json::parse(&text).unwrap_or_else(|e| {
        let stderr = String::from_utf8_lossy(&out.stderr);
        panic!("stdout not JSON ({}): {:?}\nstderr: {}", e, text, stderr)
    });
    (out.status.code().unwrap(), json)
}

#[test]
fn set_flips_one_include_and_leaves_everything_else_exactly_as_it_was() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    assert_eq!(set(d, &["--op", "get:/version", "--include", "false"]), 0);
    let after = read(d);
    // Comments and neighbours survive.
    assert!(
        after.contains("  # keep this comment exactly where it is\n"),
        "{}",
        after
    );
    assert!(
        after.contains("name: version }   # a trailing comment"),
        "{}",
        after
    );
    let parsed = graphos_factory_core::yaml::parse(&after).unwrap();
    assert_eq!(
        parsed["operations"]["get:/version"]["include"],
        serde_json::json!(false)
    );
    // The response block that precedes `include:` is untouched.
    assert_eq!(
        parsed["operations"]["get:/version"]["response"],
        serde_json::json!({"envelope": "version", "confirmed": true})
    );
    assert_eq!(
        parsed["operations"]["get:/version"]["graphql"],
        serde_json::json!({"root": "query", "name": "version"})
    );
    // Its neighbours keep their flags.
    assert_eq!(
        parsed["operations"]["get:/widgets"]["include"],
        serde_json::json!(true)
    );
    assert_eq!(
        parsed["operations"]["delete:/widgets/{id}"]["include"],
        serde_json::json!(false)
    );
}

#[test]
fn set_can_enable_an_excluded_operation_and_keeps_its_reason() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    assert_eq!(
        set(d, &["--op", "delete:/widgets/{id}", "--include", "true"]),
        0
    );
    let parsed = graphos_factory_core::yaml::parse(&read(d)).unwrap();
    assert_eq!(
        parsed["operations"]["delete:/widgets/{id}"]["include"],
        serde_json::json!(true)
    );
    assert_eq!(
        parsed["operations"]["delete:/widgets/{id}"]["reason"],
        serde_json::json!("read-only first release")
    );
}

#[test]
fn setting_an_operation_to_the_value_it_already_has_is_nothing_to_do() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    let before = read(d);
    assert_eq!(set(d, &["--op", "get:/widgets", "--include", "true"]), 2);
    assert_eq!(read(d), before);
}

#[test]
fn several_ops_toggle_in_one_call() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    assert_eq!(
        set(
            d,
            &[
                "--op",
                "get:/version",
                "--op",
                "get:/widgets",
                "--include",
                "false"
            ]
        ),
        0
    );
    let parsed = graphos_factory_core::yaml::parse(&read(d)).unwrap();
    assert_eq!(
        parsed["operations"]["get:/version"]["include"],
        serde_json::json!(false)
    );
    assert_eq!(
        parsed["operations"]["get:/widgets"]["include"],
        serde_json::json!(false)
    );
}

#[test]
fn an_operation_the_selection_does_not_list_is_refused_and_writes_nothing() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    let before = read(d);
    assert_eq!(set(d, &["--op", "get:/nope", "--include", "false"]), 1);
    assert_eq!(read(d), before);
}

#[test]
fn an_unknown_op_refusal_still_emits_the_json_envelope() {
    // A machine caller passing --json must get the same report() envelope on the
    // unknown-op refusal as on the structural-error refusal — not a bare stderr
    // line it cannot parse (e.g. a stale key removed since the panel loaded).
    let dir = workspace(SELECTION);
    let d = dir.path();
    let before = read(d);
    let (code, json) = set_json(d, &["--op", "get:/nope", "--include", "false", "--json"]);
    assert_eq!(code, 1);
    let errors = json["errors"].as_array().expect("errors array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["key"], serde_json::json!("get:/nope"));
    assert_eq!(json["changed"].as_array().unwrap().len(), 0);
    assert_eq!(read(d), before);
}

#[test]
fn a_non_boolean_include_is_a_usage_error() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    let before = read(d);
    assert_eq!(set(d, &["--op", "get:/version", "--include", "maybe"]), 1);
    assert_eq!(set(d, &["--op", "get:/version"]), 1);
    assert_eq!(read(d), before);
}

#[test]
fn dry_run_reports_the_change_but_writes_nothing() {
    let dir = workspace(SELECTION);
    let d = dir.path();
    let before = read(d);
    assert_eq!(
        set(
            d,
            &["--op", "get:/version", "--include", "false", "--dry-run"]
        ),
        0
    );
    assert_eq!(read(d), before);
}

/// An operation entry that is a block mapping but carries no `include:` line —
/// a structural problem the toggle must refuse, not fold into nothing-to-do.
const NO_INCLUDE_LINE: &str = r#"contract_version: 1
defaults:
  fields: all
operations:
  "get:/a":
    include: false
    graphql: { root: query, name: a }
  "get:/b":
    graphql: { root: query, name: b }
"#;

#[test]
fn a_target_with_no_include_line_is_refused_with_exit_1_and_writes_nothing() {
    let dir = workspace(NO_INCLUDE_LINE);
    let d = dir.path();
    let before = read(d);
    assert_eq!(set(d, &["--op", "get:/b", "--include", "true"]), 1);
    assert_eq!(read(d), before);
}

#[test]
fn a_structural_failure_on_one_target_refuses_the_whole_call_atomically() {
    let dir = workspace(NO_INCLUDE_LINE);
    let d = dir.path();
    let before = read(d);
    // get:/a would flip false->true, but get:/b has no include line: neither is written.
    assert_eq!(
        set(
            d,
            &["--op", "get:/a", "--op", "get:/b", "--include", "true"]
        ),
        1
    );
    assert_eq!(read(d), before);
}

#[test]
fn a_target_with_no_include_line_is_refused_in_the_false_direction_too() {
    // A missing `include:` line parses as `false`, so `--include false` must not
    // be swallowed as "already false" (exit 2): the structural refusal (exit 1)
    // applies in either direction.
    let dir = workspace(NO_INCLUDE_LINE);
    let d = dir.path();
    let before = read(d);
    assert_eq!(set(d, &["--op", "get:/b", "--include", "false"]), 1);
    assert_eq!(read(d), before);
}

#[test]
fn a_false_direction_structural_failure_refuses_even_beside_a_benign_skip() {
    // get:/a is already `false`, so `--include false` on it is a benign skip;
    // get:/b has no include line. The structural failure must still refuse the
    // whole call with exit 1 rather than reporting nothing-to-do (exit 2).
    let dir = workspace(NO_INCLUDE_LINE);
    let d = dir.path();
    let before = read(d);
    assert_eq!(
        set(
            d,
            &["--op", "get:/a", "--op", "get:/b", "--include", "false"]
        ),
        1
    );
    assert_eq!(read(d), before);
}

// ── Flow-style entries (ADR 0027) ────────────────────────────────────────────
//
// The pilots write most excluded operations as a single-line flow mapping —
// 459 of gitea's 467 entries and 22 of pagerduty's 35 — because that is the
// form the agent authors in bulk. The toggle must edit that form in place,
// changing only the bytes of the `include` value.

/// Every awkward flow shape in one file: a plain flow entry, one whose reason
/// carries braces and the literal text `include:` inside a quoted string, one
/// with a nested flow value after the flag, one single-quoted, one with no
/// flag at all, and a block entry beside them that must keep working.
const FLOW: &str = r#"contract_version: 1
defaults:
  fields: all
operations:
  "get:/version":
    include: true
    graphql: { root: query, name: version }
  # ── excluded in bulk (D-0001) ───────────────────────────────────────────
  "get:/admin/orgs": { include: false, reason: "pilot scope (D-0001)" }
  "post:/admin/users": {include: false,reason: "pilot scope (D-0001)"}
  "get:/tricky": { include: false, reason: "braces { } and include: true inside a string" }
  "get:/nested": { include: false, graphql: { root: query, name: nested }, reason: "later" }
  'get:/single': { include: false, reason: 'single quoted' }
"#;

/// A flow entry with no `include` key at all. The schema requires one, so
/// this file is invalid as a whole — the point is that the toggle refuses on
/// structure, before it ever writes or validates.
const FLOW_NO_INCLUDE: &str = r#"contract_version: 1
defaults:
  fields: all
operations:
  "get:/noflag": { reason: "no include at all" }
"#;

fn flow_line(text: &str, key: &str) -> String {
    text.lines()
        .find(|l| {
            l.trim_start().starts_with(&format!("\"{}\"", key))
                || l.trim_start().starts_with(&format!("'{}'", key))
        })
        .unwrap_or_else(|| panic!("no line for {}", key))
        .to_string()
}

#[test]
fn a_flow_style_entry_is_flipped_in_place_and_changes_only_the_value_bytes() {
    let dir = workspace(FLOW);
    let d = dir.path();
    let before = read(d);
    assert_eq!(set(d, &["--op", "get:/admin/orgs", "--include", "true"]), 0);
    let after = read(d);
    assert_eq!(
        flow_line(&after, "get:/admin/orgs"),
        r#"  "get:/admin/orgs": { include: true, reason: "pilot scope (D-0001)" }"#
    );
    // Only that one value moved: the rest of the file is byte-identical.
    assert_eq!(
        after.replace(
            &flow_line(&after, "get:/admin/orgs"),
            &flow_line(&before, "get:/admin/orgs")
        ),
        before
    );
    let parsed = graphos_factory_core::yaml::parse(&after).unwrap();
    assert_eq!(
        parsed["operations"]["get:/admin/orgs"]["include"],
        serde_json::json!(true)
    );
    assert_eq!(
        parsed["operations"]["get:/admin/orgs"]["reason"],
        serde_json::json!("pilot scope (D-0001)")
    );
}

#[test]
fn flipping_a_flow_entry_back_restores_the_file_byte_for_byte() {
    let dir = workspace(FLOW);
    let d = dir.path();
    let before = read(d);
    assert_eq!(set(d, &["--op", "get:/admin/orgs", "--include", "true"]), 0);
    assert_ne!(read(d), before);
    assert_eq!(
        set(d, &["--op", "get:/admin/orgs", "--include", "false"]),
        0
    );
    assert_eq!(read(d), before);
}

#[test]
fn a_flow_entry_written_without_spaces_keeps_its_own_spacing() {
    let dir = workspace(FLOW);
    let d = dir.path();
    assert_eq!(
        set(d, &["--op", "post:/admin/users", "--include", "true"]),
        0
    );
    assert_eq!(
        flow_line(&read(d), "post:/admin/users"),
        r#"  "post:/admin/users": {include: true,reason: "pilot scope (D-0001)"}"#
    );
}

#[test]
fn a_reason_holding_braces_and_the_text_include_is_not_mistaken_for_the_flag() {
    // The scan is quote-aware: `include: true` inside the reason string must
    // not be the thing that gets rewritten, and the braces in it must not
    // unbalance the depth count.
    let dir = workspace(FLOW);
    let d = dir.path();
    assert_eq!(set(d, &["--op", "get:/tricky", "--include", "true"]), 0);
    assert_eq!(
        flow_line(&read(d), "get:/tricky"),
        r#"  "get:/tricky": { include: true, reason: "braces { } and include: true inside a string" }"#
    );
}

#[test]
fn a_nested_flow_value_inside_the_entry_survives_the_edit() {
    let dir = workspace(FLOW);
    let d = dir.path();
    assert_eq!(set(d, &["--op", "get:/nested", "--include", "true"]), 0);
    assert_eq!(
        flow_line(&read(d), "get:/nested"),
        r#"  "get:/nested": { include: true, graphql: { root: query, name: nested }, reason: "later" }"#
    );
    let parsed = graphos_factory_core::yaml::parse(&read(d)).unwrap();
    assert_eq!(
        parsed["operations"]["get:/nested"]["graphql"],
        serde_json::json!({"root": "query", "name": "nested"})
    );
}

#[test]
fn a_single_quoted_flow_key_is_found_too() {
    let dir = workspace(FLOW);
    let d = dir.path();
    assert_eq!(set(d, &["--op", "get:/single", "--include", "true"]), 0);
    assert_eq!(
        flow_line(&read(d), "get:/single"),
        r#"  'get:/single': { include: true, reason: 'single quoted' }"#
    );
}

#[test]
fn a_flow_entry_with_no_include_is_refused_and_says_what_is_wrong() {
    let dir = workspace(FLOW_NO_INCLUDE);
    let d = dir.path();
    let before = read(d);
    for want in ["true", "false"] {
        let (code, json) = set_json(d, &["--op", "get:/noflag", "--include", want, "--json"]);
        assert_eq!(code, 1);
        // Not "not a block mapping": the entry *is* a flow mapping the toggle
        // can read; what it lacks is the flag.
        assert_eq!(
            json["errors"][0]["reason"],
            serde_json::json!("its flow mapping has no include: value to set")
        );
    }
    assert_eq!(read(d), before);
}

#[test]
fn the_block_entry_beside_flow_entries_still_toggles() {
    let dir = workspace(FLOW);
    let d = dir.path();
    assert_eq!(set(d, &["--op", "get:/version", "--include", "false"]), 0);
    let after = read(d);
    assert!(after.contains("\n    include: false\n"), "{}", after);
    assert!(
        after
            .contains(r#"  "get:/admin/orgs": { include: false, reason: "pilot scope (D-0001)" }"#),
        "{}",
        after
    );
}

#[test]
fn a_flow_and_a_block_entry_toggle_together_in_one_call() {
    let dir = workspace(FLOW);
    let d = dir.path();
    // get:/version starts true; take it down so the mixed call has two real
    // changes, one block and one flow, in a single write.
    assert_eq!(set(d, &["--op", "get:/version", "--include", "false"]), 0);
    assert_eq!(
        set(
            d,
            &[
                "--op",
                "get:/version",
                "--op",
                "get:/admin/orgs",
                "--include",
                "true"
            ]
        ),
        0
    );
    let parsed = graphos_factory_core::yaml::parse(&read(d)).unwrap();
    assert_eq!(
        parsed["operations"]["get:/admin/orgs"]["include"],
        serde_json::json!(true)
    );
    assert_eq!(
        parsed["operations"]["get:/version"]["include"],
        serde_json::json!(true)
    );
    // Each kept its own style.
    assert!(read(d).contains("\n    include: true\n"), "{}", read(d));
    assert_eq!(
        flow_line(&read(d), "get:/admin/orgs"),
        r#"  "get:/admin/orgs": { include: true, reason: "pilot scope (D-0001)" }"#
    );
}

/// A flow mapping the author broke across two lines: legal YAML, but not a
/// span this edit can locate, so it is refused with the fix in the message.
const MULTILINE_FLOW: &str = r#"contract_version: 1
defaults:
  fields: all
operations:
  "get:/a": { include: false,
              reason: "wrapped across two lines" }
"#;

#[test]
fn a_multi_line_flow_entry_is_refused_with_an_actionable_message() {
    let dir = workspace(MULTILINE_FLOW);
    let d = dir.path();
    let before = read(d);
    let (code, json) = set_json(d, &["--op", "get:/a", "--include", "true", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(read(d), before);
    let why = json["errors"][0]["reason"].as_str().unwrap().to_string();
    assert!(
        why.contains("does not close on one line")
            && why.contains("one line")
            && why.contains("block mapping"),
        "the refusal must say how to fix it: {:?}",
        why
    );
    assert_eq!(json["changed"].as_array().unwrap().len(), 0);
}

#[test]
fn a_flow_dry_run_reports_the_same_change_shape_as_a_block_dry_run() {
    let dir = workspace(FLOW);
    let d = dir.path();
    let before = read(d);
    let (flow_code, flow_json) = set_json(
        d,
        &[
            "--op",
            "get:/admin/orgs",
            "--include",
            "true",
            "--dry-run",
            "--json",
        ],
    );
    let (block_code, block_json) = set_json(
        d,
        &[
            "--op",
            "get:/version",
            "--include",
            "false",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!((flow_code, block_code), (0, 0));
    assert_eq!(read(d), before, "a dry run writes nothing");
    for (json, key, from, to) in [
        (&flow_json, "get:/admin/orgs", false, true),
        (&block_json, "get:/version", true, false),
    ] {
        assert_eq!(json["dry_run"], serde_json::json!(true));
        assert_eq!(json["file"], serde_json::json!(".factory/selection.yaml"));
        let changed = json["changed"].as_array().unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(
            changed[0],
            serde_json::json!({"key": key, "field": "include", "from": from, "to": to})
        );
        assert_eq!(json["errors"].as_array().unwrap().len(), 0);
    }
}

#[test]
fn an_operation_declared_twice_is_refused_rather_than_half_edited() {
    // The YAML parse gate catches this before any entry is located, in flow
    // style as in block style — the flow path must not route around it.
    for entry in [
        "  \"get:/a\": { include: false, reason: \"first\" }\n  \"get:/a\": { include: false, reason: \"second\" }\n",
        "  \"get:/a\":\n    include: false\n  \"get:/a\":\n    include: false\n",
    ] {
        let dir = workspace(&format!(
            "contract_version: 1\ndefaults:\n  fields: all\noperations:\n{}",
            entry
        ));
        let d = dir.path();
        let before = read(d);
        assert_eq!(set(d, &["--op", "get:/a", "--include", "true"]), 1);
        assert_eq!(read(d), before);
    }
}

#[test]
fn the_gitea_pilot_flow_entry_dry_runs_as_exactly_one_change() {
    // The defect was found on this exact key. A dry run writes nothing, so the
    // pilot's judgements are read here, never edited.
    let pilot = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    if !pilot.join(".factory/selection.yaml").exists() {
        eprintln!("skipping: pilots/graphos/gitea is not checked out next to the crate");
        return;
    }
    let before = std::fs::read_to_string(pilot.join(".factory/selection.yaml")).unwrap();
    let (code, json) = set_json(
        &pilot,
        &[
            "--op",
            "get:/admin/orgs",
            "--include",
            "true",
            "--dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{}", graphos_factory_core::json::pretty(&json));
    let changed = json["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(
        changed[0],
        serde_json::json!({"key": "get:/admin/orgs", "field": "include", "from": false, "to": true})
    );
    assert_eq!(
        std::fs::read_to_string(pilot.join(".factory/selection.yaml")).unwrap(),
        before,
        "a dry run must not touch the pilot"
    );
}

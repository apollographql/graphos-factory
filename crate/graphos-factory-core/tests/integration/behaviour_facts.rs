//! `source-coverage` classifies what the source says about omitting an
//! optional argument (ADR 0095): documented, waived by a resolved decision,
//! or unaccounted, and `--check` fails on unaccounted.

use graphos_factory_core::obligations::{build, BehaviourClass, Report};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const VENMO: &str =
    "ID of the payment card to use for the transaction. If not passed, Venmo balance will be used.";
const OP: &str = "get:/pay";

struct Workspace(PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// A read whose optional `payment_card_id` query parameter carries the Venmo
/// sentence, exposed as `paymentCardId` with `card_arg` as its declaration
/// (a doc comment, and the type).
fn workspace(tag: &str, card_arg: &str, decisions: Option<Value>) -> Workspace {
    workspace_selecting(tag, card_arg, decisions, "id extra")
}

fn workspace_selecting(
    tag: &str,
    card_arg: &str,
    decisions: Option<Value>,
    selection: &str,
) -> Workspace {
    let dir = std::env::temp_dir().join(format!(
        "behaviour-facts-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write(
        &dir,
        ".factory/workspace.yaml",
        "contract_version: 1\nservice: shop\ndirectory: shop\ntype_prefix: Shop\nfield_prefix: shop\n",
    );
    write(
        &dir,
        "shop.graphql",
        &format!(
            "extend schema\n  @link(url: \"https://specs.apollo.dev/connect/v0.3\", import: [\"@source\", \"@connect\"])\n\n@source(name: \"shop\", http: {{ baseURL: \"{{{{BASE_URL}}}}\" }})\n\ntype Shop_Ack {{ id: ID }}\ntype Query {{\n  shop_pay(\n    amount: Float!\n    {}\n  ): Shop_Ack\n    @connect(source: \"shop\", http: {{ GET: \"/pay\", queryParams: \"amount: $args.amount payment_card_id: $args.paymentCardId\" }}, selection: \"{}\")\n}}\n",
            card_arg, selection
        ),
    );
    write(
        &dir,
        ".factory/inventory.json",
        &json!({
            "contract_version": 1,
            "api": {"title": "Shop", "base_urls": ["https://shop.test"]},
            "operations": [{
                "key": OP, "operation_id": "pay", "method": "GET", "path": "/pay",
                "parameters": [
                    {"name": "amount", "in": "query", "required": true, "type": "number",
                     "description": "Amount to pay."},
                    {"name": "payment_card_id", "in": "query", "required": false,
                     "type": "integer", "description": VENMO}
                ],
                "response": {"status": "200", "shape_ref": "#/shapes/Ack"}
            }],
            "shapes": {"Ack": {"type": "object", "properties": {"id": {"type": "string"}, "extra": {"type": "string"}}}}
        })
        .to_string(),
    );
    if let Some(d) = decisions {
        write(&dir, ".factory/decisions.json", &d.to_string());
    }
    Workspace(dir)
}

fn report(ws: &Workspace) -> Report {
    build(&ws.0, OP).unwrap()
}

fn check(ws: &Workspace) -> i32 {
    graphos_factory_core::cmd::source_coverage::main(&[
        ws.0.to_str().unwrap().into(),
        OP.into(),
        "--check".into(),
    ])
}

fn decision(status: &str, direction: &str, path: &str, reason: &str) -> Value {
    let mut rec = json!({
        "id": "D-0001", "title": "the card sentence", "status": status, "date": "2026-09-29",
        "omits": [{"operation": OP, "direction": direction, "path": path, "reason": reason}]
    });
    if status == "resolved" {
        rec["resolution"] = json!({"decision": "not about omitting the card"});
    }
    json!({"contract_version": 1, "decisions": [rec]})
}

fn only_row(r: &Report) -> &graphos_factory_core::obligations::BehaviourRow {
    assert_eq!(r.behaviour.len(), 1, "{:?}", r.behaviour);
    &r.behaviour[0]
}

/// The Venmo case: the doc comment (or none) leaves the sentence out, so
/// nothing in the schema tells a caller what no card means. Unaccounted, and
/// `--check` fails on it.
#[test]
fn an_omitted_sentence_is_unaccounted_and_fails_the_check() {
    for (tag, arg) in [
        ("none", "paymentCardId: Int"),
        ("other", "\"ID of the card.\"\n    paymentCardId: Int"),
    ] {
        let ws = workspace(tag, arg, None);
        let r = report(&ws);
        let row = only_row(&r);
        assert_eq!(row.class, BehaviourClass::Unaccounted, "{}", tag);
        assert_eq!(row.path, "query:payment_card_id");
        assert_eq!(row.sentence, "If not passed, Venmo balance will be used.");
        assert_eq!(row.carrier, "Query.shop_pay(paymentCardId)");
        assert_eq!(
            r.check_failure().as_deref(),
            Some("behaviour unaccounted 1"),
            "{}",
            tag
        );
        assert_eq!(check(&ws), 1, "{}", tag);
    }
}

/// A doc comment carrying the sentence keeps the check green.
#[test]
fn a_carried_sentence_is_documented_and_passes_the_check() {
    let ws = workspace(
        "carried",
        "\"ID of the card. If not passed, Venmo balance will be used.\"\n    paymentCardId: Int",
        None,
    );
    let r = report(&ws);
    assert_eq!(only_row(&r).class, BehaviourClass::Documented);
    assert_eq!(r.check_failure(), None);
    assert_eq!(check(&ws), 0);
}

/// A resolved decision's behaviour waiver is the recorded way out: the row
/// reads waived, naming the decision and the reason, and the check passes.
#[test]
fn a_resolved_waiver_clears_the_row() {
    let ws = workspace(
        "waived",
        "paymentCardId: Int",
        Some(decision(
            "resolved",
            "behaviour",
            "query:payment_card_id",
            "not-applicable",
        )),
    );
    let r = report(&ws);
    assert_eq!(
        only_row(&r).class,
        BehaviourClass::Waived {
            reason: "not-applicable".into(),
            decision: "D-0001".into()
        }
    );
    assert_eq!(r.check_failure(), None);
    assert_eq!(check(&ws), 0);
}

/// Only a resolved decision counts, as for a wire omit; the row says why the
/// waiver on an open one does not.
#[test]
fn an_open_waiver_does_not_count_and_the_row_says_so() {
    let ws = workspace(
        "open",
        "paymentCardId: Int",
        Some(decision(
            "open",
            "behaviour",
            "query:payment_card_id",
            "editorial",
        )),
    );
    let r = report(&ws);
    let row = only_row(&r);
    assert_eq!(row.class, BehaviourClass::Unaccounted);
    assert!(
        row.note
            .as_deref()
            .is_some_and(|n| n.contains("D-0001") && n.contains("open")),
        "{:?}",
        row.note
    );
    assert_eq!(check(&ws), 1);
}

/// A waiver names one source by operation and `location:name`. A waiver for
/// another parameter, or another operation, does not clear this row.
#[test]
fn a_waiver_for_another_source_does_not_clear_the_row() {
    let ws = workspace(
        "other-source",
        "paymentCardId: Int",
        Some(decision(
            "resolved",
            "behaviour",
            "query:amount",
            "editorial",
        )),
    );
    assert_eq!(only_row(&report(&ws)).class, BehaviourClass::Unaccounted);
}

/// A behaviour waiver is not a wire omission: it covers no request or
/// response path, even one that shares its spelling.
#[test]
fn a_behaviour_waiver_covers_no_wire_path() {
    let ws = workspace_selecting(
        "not-wire",
        "paymentCardId: Int",
        Some(decision("resolved", "behaviour", "extra", "editorial")),
        "id",
    );
    let r = report(&ws);
    let extra = r.response.iter().find(|row| row.path == "extra").unwrap();
    assert_eq!(extra.class.label(), "unaccounted");
    // Nor is it a wire omit gone stale (ADR 0103): the stale list reads wire
    // omits only.
    assert!(r.stale.is_empty(), "{:?}", r.stale.len());
}

/// A required argument cannot be omitted, so its sentence is no fact about
/// omission; an operation with no such sentence has no behaviour section, and
/// `--json` carries no key for it.
#[test]
fn a_required_argument_and_a_silent_source_have_no_behaviour_rows() {
    let ws = workspace("required", "paymentCardId: Int!", None);
    assert!(report(&ws).behaviour.is_empty());
    assert_eq!(check(&ws), 0);
}

/// With no OP-KEY (ADR 0101) the same rows decide the bar: an operation whose
/// only gap is an unaccounted sentence fails `--check`, and `--json` carries
/// the behaviour counts in the operation's object and in the totals.
#[test]
fn every_selected_operation_counts_behaviour_rows() {
    let selection = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/pay\":\n    include: true\n    graphql:\n      root: query\n      name: pay\n";
    for (tag, arg, fails) in [
        ("all-missing", "paymentCardId: Int", true),
        (
            "all-carried",
            "\"If not passed, Venmo balance will be used.\"\n    paymentCardId: Int",
            false,
        ),
    ] {
        let ws = workspace(tag, arg, None);
        write(&ws.0, ".factory/selection.yaml", selection);
        let dir = ws.0.to_str().unwrap().to_string();
        let code =
            graphos_factory_core::cmd::source_coverage::main(&[dir.clone(), "--check".into()]);
        assert_eq!(code, if fails { 1 } else { 0 }, "{}", tag);
        let report = graphos_factory_core::obligations::build(&ws.0, OP).unwrap();
        let counts = report.behaviour_counts();
        assert_eq!(
            (counts.offered, counts.unaccounted),
            (1, if fails { 1 } else { 0 }),
            "{}",
            tag
        );
    }
}

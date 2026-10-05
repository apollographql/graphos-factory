//! Bare `/{id}` paths: two operations whose templates differ only in the
//! parameter name. Attribution must follow what the selection or the test
//! declares, never the inventory's order (ADR 0044). `get:/{ad_id}` is
//! listed first on purpose: a matcher that takes the first candidate lands
//! on it.

use graphos_factory_core::op_match::{disambiguate, OpHints};
use graphos_factory_core::reconcile::{match_operation, read_links, reconcile, Inputs, Link};
use serde_json::{json, Value};

fn op(key: &str, path: &str, shape: &str) -> Value {
    json!({"key": key, "operation_id": key, "method": "GET", "path": path, "semantics": "read", "provenance": "spec", "confidence": 1,
        "parameters": [], "request_body": null,
        "response": {"status": "200", "envelope": null, "shape_ref": format!("#/shapes/{}", shape), "list": false},
        "errors": [], "support": "supported", "support_reason": null})
}

fn inventory() -> Value {
    json!({
        "contract_version": 1,
        "api": {"title": "Meta", "base_urls": ["https://graph.test/v21.0"]},
        "operations": [
            op("get:/{ad_id}", "/{ad_id}", "Ad"),
            op("get:/{campaign_id}", "/{campaign_id}", "Campaign")
        ],
        "shapes": {
            "Ad": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}}},
            "Campaign": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"}, "daily_budget": {"type": "string"}
            }}
        },
        "unresolved": []
    })
}

const WORKSPACE: &str = "contract_version: 1\nservice: meta\ndirectory: meta\ntype_prefix: Meta\nfield_prefix: meta\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-23T00:00:00Z\n";

const SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/{ad_id}\":\n    include: false\n    reason: not in this slice\n  \"get:/{campaign_id}\":\n    include: true\n    graphql: { root: query, name: campaign }\n";

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "meta", http: { baseURL: "{{BASE_URL}}" })

type Meta_Campaign {
  id: ID!
  name: String
  daily_budget: String
}

type Query {
  meta_campaign(campaignId: ID!): Meta_Campaign
    @connect(source: "meta", http: { GET: "/{$args.campaignId}" }, selection: "id name daily_budget")
}
"#;

fn yaml(text: &str) -> Value {
    graphos_factory_core::yaml::parse(text).unwrap()
}

fn report(selection: &Value) -> Value {
    reconcile(&Inputs {
        decisions: None,
        workspace: &yaml(WORKSPACE),
        selection,
        inventory: &inventory(),
        sdl: SDL,
        baseline: None,
        schema_file: "meta.graphql",
        lock: None,
        today: "2026-09-23",
    })
}

fn keys(report: &Value, section: &str) -> Vec<String> {
    report[section]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["key"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn reconcile_attributes_a_bare_id_connector_to_the_operation_the_selection_declares() {
    let report = report(&yaml(SELECTION));
    assert_eq!(report["selection_errors"], json!([]));
    assert_eq!(keys(&report, "add"), Vec::<String>::new());
    assert_eq!(keys(&report, "remove"), Vec::<String>::new());
    assert!(keys(&report, "unchanged").contains(&"get:/{campaign_id}".to_string()));
    assert_eq!(report["clean"], true);
}

/// The selection names no field for either operation.
const UNHINTED_SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/{ad_id}\":\n    include: false\n    reason: not in this slice\n";

fn lock_workspace(selection: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w("meta.graphql", SDL);
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", selection);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inventory()),
    );
    dir
}

#[test]
fn lock_writes_the_declared_key_for_a_bare_id_connector() {
    let dir = lock_workspace(SELECTION);
    let argv = vec![dir.path().to_string_lossy().to_string()];
    assert_eq!(graphos_factory_core::cmd::lock::main(&argv), 0);
    let lock = std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap();
    assert!(lock.contains("\"get:/{campaign_id}\""), "{}", lock);
    assert!(!lock.contains("{ad_id}"), "{}", lock);
}

#[test]
fn lock_refuses_an_unsettled_bare_id_tie_and_writes_nothing() {
    // Recording `Query.meta_campaign` would key the span on a name that
    // moves to an operation key as soon as the selection names the field.
    let dir = lock_workspace(UNHINTED_SELECTION);
    let argv = vec![dir.path().to_string_lossy().to_string()];
    assert_eq!(graphos_factory_core::cmd::lock::main(&argv), 2);
    assert!(!dir.path().join(".factory/applied.lock.yaml").exists());
}

#[test]
fn an_unsettled_bare_id_tie_is_an_error_naming_both_operations_in_reconcile() {
    let selection = yaml(UNHINTED_SELECTION);
    let report = report(&selection);
    let errors: Vec<&str> = report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert!(
        errors.iter().any(
            |e| e.starts_with("Query.meta_campaign: GET /{$args.campaignId}")
                && e.contains("get:/{ad_id}, get:/{campaign_id}")
        ),
        "{:?}",
        errors
    );
    assert_eq!(keys(&report, "remove"), Vec::<String>::new());
    assert_eq!(report["clean"], false);

    let spans = graphos_factory_core::spans::spans(SDL, Some(&inventory()), &OpHints::default());
    let field = spans
        .iter()
        .find(|s| s.key == "Query.meta_campaign")
        .unwrap();
    assert!(field
        .unattributed
        .as_deref()
        .unwrap()
        .contains("get:/{ad_id}, get:/{campaign_id}"));
}

#[test]
fn match_operation_errors_on_a_tie_and_follows_the_hint() {
    let inv = inventory();
    let err = match_operation(&inv, Some("GET"), Some("/{$args.id}"), &[]).unwrap_err();
    assert_eq!(
        err,
        "GET /{$args.id} matches 2 operations equally (get:/{ad_id}, get:/{campaign_id}) and nothing declares which"
    );
    let hinted = match_operation(
        &inv,
        Some("GET"),
        Some("/{$args.id}"),
        &["get:/{campaign_id}"],
    );
    assert_eq!(hinted.unwrap().unwrap()["key"], "get:/{campaign_id}");
    let wrong = match_operation(&inv, Some("GET"), Some("/{$args.id}"), &["get:/{nope}"]);
    assert!(wrong
        .unwrap_err()
        .contains("the declared operation get:/{nope} is not one of them"));
}

#[test]
fn disambiguate_takes_a_sole_candidate_whatever_the_hint() {
    let inv = inventory();
    let ops = inv["operations"].as_array().unwrap();
    assert_eq!(disambiguate(vec![], &[]).unwrap(), None);
    assert_eq!(
        disambiguate(vec![&ops[1]], &["get:/{ad_id}"])
            .unwrap()
            .unwrap()["key"],
        "get:/{campaign_id}"
    );
}

#[test]
fn hints_come_from_graphql_root_and_name() {
    let hints = OpHints::from_selection(Some(&yaml(WORKSPACE)), Some(&yaml(SELECTION)), None);
    assert_eq!(
        hints.for_target("Query.meta_campaign"),
        vec!["get:/{campaign_id}"]
    );
    assert_eq!(hints.for_case("campaign"), vec!["get:/{campaign_id}"]);
    assert_eq!(
        hints.for_case("campaign_minimal"),
        vec!["get:/{campaign_id}"]
    );
    assert!(hints.for_case("campaign_invalid").is_empty());
}

#[test]
fn a_fields_narrowed_stub_on_a_tied_path_is_judged_against_its_cases_operation() {
    // Sparse fieldsets (ADR 0045) writes `<case>_<param>_narrowed` beside
    // the case; with no case document of its own name, only the stem
    // declares it.
    let hints = OpHints::from_selection(Some(&yaml(WORKSPACE)), Some(&yaml(SELECTION)), None);
    assert_eq!(
        hints.for_case("campaign_fields_narrowed"),
        vec!["get:/{campaign_id}"]
    );
    assert!(hints.for_case("mystery_fields_narrowed").is_empty());
    // Only the workspace's own parameter: `status` is not it.
    assert!(hints.for_case("campaign_status_narrowed").is_empty());
    // A parameter with an underscore is stripped whole, and `fields` is then
    // no longer the suffix.
    let masked = WORKSPACE.to_string() + "sparse_fieldsets: { param: field_mask }\n";
    let hints = OpHints::from_selection(Some(&yaml(&masked)), Some(&yaml(SELECTION)), None);
    assert_eq!(
        hints.for_case("campaign_field_mask_narrowed"),
        vec!["get:/{campaign_id}"]
    );
    assert!(hints.for_case("campaign_fields_narrowed").is_empty());
    let ws = validate_workspace("campaign_fields_narrowed", "Query.meta_campaign");
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    let r = result(
        &report,
        "tests/fixtures/mappings/campaign_fields_narrowed.json",
    );
    assert_eq!(r["operation"], "get:/{campaign_id}", "{}", r);
    assert_eq!(r["status"], "fail", "{}", r);
    assert!(r["problems"].to_string().contains("daily_budget"), "{}", r);
}

/// Both operations carry `graphql: { root: query, name: campaign }`.
fn both_named_campaign(ad_included: bool) -> Value {
    yaml(&format!("contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/{{ad_id}}\":\n    include: {}\n    reason: not in this slice\n    graphql: {{ root: query, name: campaign }}\n  \"get:/{{campaign_id}}\":\n    include: true\n    graphql: {{ root: query, name: campaign }}\n", ad_included))
}

#[test]
fn an_excluded_entry_never_cancels_an_included_hint() {
    // An excluded entry that kept its graphql block used to cancel the
    // included one's hint: "nothing declares which", a spurious add, and
    // lock refusing to write.
    let report = report(&both_named_campaign(false));
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    assert_eq!(keys(&report, "add"), Vec::<String>::new());
    assert_eq!(report["clean"], true, "{}", report);
}

/// The user drops `get:/{campaign_id}`: its entry is excluded but keeps the
/// graphql block, and the schema still has the field until apply removes it.
const DROPPED_SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/{ad_id}\":\n    include: false\n    reason: not in this slice\n  \"get:/{campaign_id}\":\n    include: false\n    reason: dropped by the user\n    graphql: { root: query, name: campaign }\n";

fn lint_is_clean_of_ties(dir: &std::path::Path) {
    let rules = lint_rules(dir);
    assert!(
        !rules
            .iter()
            .any(|(r, _)| r == "unacknowledged-edit" || r == "unattributed-span"),
        "{:?}",
        rules
    );
}

#[test]
fn excluding_an_entry_on_a_bare_id_tie_reports_its_removal() {
    // The field must stay attributed to the excluded entry, or reconcile
    // cannot say what to remove and lock --check sees a fake hand edit.
    let report = report(&yaml(DROPPED_SELECTION));
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    assert_eq!(keys(&report, "remove"), vec!["get:/{campaign_id}"]);

    let dir = lock_workspace(SELECTION);
    assert_eq!(lock(dir.path(), &[]), 0);
    write(dir.path(), ".factory/selection.yaml", DROPPED_SELECTION);
    assert_eq!(lock(dir.path(), &["--check"]), 0);
    lint_is_clean_of_ties(dir.path());
}

#[test]
fn excluding_an_entity_entry_on_a_bare_id_tie_reports_its_removal() {
    let dropped = ENTITY_SELECTION.replace(
        "    include: true\n    graphql: { root: query, name: campaign, entity: true",
        "    include: false\n    reason: dropped by the user\n    graphql: { root: query, name: campaign, entity: true",
    );
    let sdl = entity_sdl();
    let report = reconcile(&Inputs {
        decisions: None,
        workspace: &yaml(WORKSPACE),
        selection: &yaml(&dropped),
        inventory: &inventory(),
        sdl: &sdl,
        baseline: None,
        schema_file: "meta.graphql",
        lock: None,
        today: "2026-09-23",
    });
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    let remove = &report["remove"][0];
    assert_eq!(remove["key"], "get:/{campaign_id}", "{}", report);
    assert_eq!(remove["entity"], "Meta_Campaign", "{}", report);

    let dir = lock_workspace(ENTITY_SELECTION);
    write(dir.path(), "meta.graphql", &sdl);
    assert_eq!(lock(dir.path(), &[]), 0);
    write(dir.path(), ".factory/selection.yaml", &dropped);
    assert_eq!(lock(dir.path(), &["--check"]), 0);
    lint_is_clean_of_ties(dir.path());
}

#[test]
fn a_field_two_included_entries_claim_is_an_error_naming_both() {
    let report = report(&both_named_campaign(true));
    let errors = report["selection_errors"].to_string();
    assert!(
        errors.contains("Query.meta_campaign: GET /{$args.campaignId} matches 2 operations equally (get:/{ad_id}, get:/{campaign_id}) and it is claimed by both get:/{ad_id} and get:/{campaign_id}"),
        "{}",
        errors
    );
}

// ─── Conformance: a concrete request URL, the same two operations ───────────

const CAMPAIGN_URL: &str = "https://graph.test/v21.0/120210000000000001";

/// A Campaign body whose `daily_budget` is a number where Campaign declares
/// a string. Ad has no `daily_budget`, so judged against Ad it conforms.
fn campaign_body() -> Value {
    json!({"id": "120210000000000001", "name": "Spring", "daily_budget": 123})
}

fn validate_workspace(stub: &str, target: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", SELECTION);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inventory()),
    );
    w(
        "template.yaml",
        "variables:\n  - name: BASE_URL\n    test_default: \"https://graph.test/v21.0\"\n",
    );
    w(
        &format!("tests/fixtures/mappings/{}.json", stub),
        &json!({"request": {"method": "GET", "urlPath": "/120210000000000001"},
                "response": {"status": 200, "jsonBody": campaign_body()}})
        .to_string(),
    );
    w(
        "tests/meta.connector.yaml",
        &format!(
            "tests:\n  - name: \"campaign\"\n    target: \"{}\"\n    apiResponseBody: |\n      {}\n    expect:\n      connectorRequest:\n        method: GET\n        url: {}\n",
            target,
            campaign_body(),
            CAMPAIGN_URL
        ),
    );
    dir
}

fn result<'a>(report: &'a Value, where_: &str) -> &'a Value {
    report["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["where"] == where_)
        .unwrap_or_else(|| panic!("no result for {}: {}", where_, report))
}

#[test]
fn validate_judges_a_bare_id_body_against_the_operation_its_stub_and_entry_declare() {
    let ws = validate_workspace("campaign", "Query.meta_campaign");
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    for where_ in [
        "tests/fixtures/mappings/campaign.json",
        "tests/meta.connector.yaml › campaign",
    ] {
        let r = result(&report, where_);
        assert_eq!(r["operation"], "get:/{campaign_id}", "{}", r);
        assert_eq!(r["status"], "fail", "{}", r);
        assert!(r["problems"].to_string().contains("daily_budget"), "{}", r);
    }
}

#[test]
fn an_unsettled_bare_id_tie_is_an_error_naming_both_operations_in_validate() {
    // Neither the stub's name nor the entry's target is declared anywhere.
    let ws = validate_workspace("mystery", "Query.meta_mystery");
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    for where_ in [
        "tests/fixtures/mappings/mystery.json",
        "tests/meta.connector.yaml › campaign",
    ] {
        let r = result(&report, where_);
        assert_eq!(r["status"], "unmatched", "{}", r);
        assert_eq!(r["operation"], Value::Null, "{}", r);
        let reason = r["reason"].as_str().unwrap();
        assert!(
            reason.starts_with("GET /120210000000000001 matches 2 operations equally (get:/{ad_id}, get:/{campaign_id})"),
            "{}",
            reason
        );
    }
}

#[test]
fn find_operation_keeps_the_most_specific_template_before_the_hint() {
    use graphos_factory_core::conformance::find_operation;
    let inv = json!({"operations": [
        {"key": "get:/{ad_id}", "method": "GET", "path": "/{ad_id}"},
        {"key": "get:/{campaign_id}", "method": "GET", "path": "/{campaign_id}"},
        {"key": "get:/me", "method": "GET", "path": "/me"}
    ]});
    // A literal path outranks every template, whatever the hint says.
    assert_eq!(
        find_operation(&inv, "GET", "/me", &["get:/{ad_id}"])
            .unwrap()
            .unwrap()["key"],
        "get:/me"
    );
    assert_eq!(
        find_operation(&inv, "GET", "/1", &["get:/{campaign_id}"])
            .unwrap()
            .unwrap()["key"],
        "get:/{campaign_id}"
    );
    assert!(find_operation(&inv, "GET", "/1", &[]).is_err());
}

#[test]
fn an_unreachable_stub_on_a_bare_id_tie_is_told_to_waive_and_the_waiver_settles_it() {
    // `fixtures` names a stub no case issues `unreachable_<sub>_<n>.json` and
    // rewrites it under that name, so renaming it is not a way out.
    let ws = validate_workspace("unreachable_graph_1", "Query.meta_campaign");
    let where_ = "tests/fixtures/mappings/unreachable_graph_1.json";
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    let r = result(&report, where_);
    assert_eq!(r["status"], "unmatched", "{}", r);
    let reason = r["reason"].as_str().unwrap();
    let waive = format!(
        "graphos-factory-core codify --waive {} --status unmatched --reason R",
        where_
    );
    assert!(reason.ends_with(&waive), "{}", reason);

    let argv: Vec<String> = [
        ws.path().to_str().unwrap(),
        "--waive",
        where_,
        "--status",
        "unmatched",
        "--reason",
        "recorded; no case issues this request",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(graphos_factory_core::cmd::codify::main(&argv), 0);
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    assert_eq!(result(&report, where_)["status"], "waived");
}

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let f = dir.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, text).unwrap();
}

/// A case named for itself, not after the entry: scaffold's `<case>` is
/// `campaign`; this is the kind of variant every pilot carries
/// (`repo_not_found`, `list_incidents_filtered`).
const VARIANT_CASE: &str =
    "query {\n  meta_campaign(campaignId: \"120210000000000001\") {\n    id\n    daily_budget\n  }\n}\n";

#[test]
fn a_variant_stub_is_judged_against_the_operation_its_case_document_selects() {
    let ws = validate_workspace("campaign_archived", "Query.meta_campaign");
    write(
        ws.path(),
        "tests/cases/campaign_archived.graphql",
        VARIANT_CASE,
    );
    // Named after its first case and shared with another (`also`), as
    // `fixtures` writes it: `x-cases` lists its own name first.
    write(
        ws.path(),
        "tests/cases/campaign_paused.graphql",
        VARIANT_CASE,
    );
    write(
        ws.path(),
        "tests/fixtures/mappings/campaign_paused.json",
        &json!({"request": {"method": "GET", "urlPath": "/120210000000000001"},
                "response": {"status": 200, "jsonBody": campaign_body()},
                "metadata": {"x-cases": ["campaign_paused", "campaign_paused_again"]}})
        .to_string(),
    );
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    for where_ in [
        "tests/fixtures/mappings/campaign_archived.json",
        "tests/fixtures/mappings/campaign_paused.json",
    ] {
        let r = result(&report, where_);
        assert_eq!(r["operation"], "get:/{campaign_id}", "{}", r);
        // Caught, not waived: Campaign declares daily_budget a string.
        assert_eq!(r["status"], "fail", "{}", r);
        assert!(r["problems"].to_string().contains("daily_budget"), "{}", r);
    }
}

#[test]
fn a_nested_stub_tagged_with_its_case_declares_nothing() {
    // Case `ad` calls meta_ad at the root and Campaign beneath it; the stub
    // answering the nested Campaign fetch is tagged `x-cases: ["ad"]`, as
    // testing.md prescribes. Read through the case, it would be judged
    // against Ad — which has no daily_budget — and its defect would pass.
    let ws = validate_workspace("campaign", "Query.meta_campaign");
    write(ws.path(), ".factory/selection.yaml", "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/{ad_id}\":\n    include: true\n    graphql: { root: query, name: ad }\n  \"get:/{campaign_id}\":\n    include: true\n    graphql: { root: query, name: campaign }\n");
    write(
        ws.path(),
        "tests/cases/ad.graphql",
        "query { meta_ad(adId: \"2\") { id name campaign { id name daily_budget } } }\n",
    );
    write(
        ws.path(),
        "tests/fixtures/mappings/ad_campaign.json",
        &json!({"request": {"method": "GET", "urlPath": "/120210000000000001"},
                "response": {"status": 200, "jsonBody": campaign_body()},
                "metadata": {"x-cases": ["ad"]}})
        .to_string(),
    );
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    let r = result(&report, "tests/fixtures/mappings/ad_campaign.json");
    assert_eq!(r["status"], "unmatched", "{}", r);
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .ends_with("graphos-factory-core codify --waive tests/fixtures/mappings/ad_campaign.json --status unmatched --reason R"),
        "{}",
        r
    );
}

// ─── A type-level connector on a bare-id path: the entity entry declares it ──

const ENTITY_SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/{ad_id}\":\n    include: false\n    reason: not in this slice\n  \"get:/{campaign_id}\":\n    include: true\n    graphql: { root: query, name: campaign, entity: true, key: id }\n";

fn entity_sdl() -> String {
    SDL.replace(
        "type Meta_Campaign {",
        "type Meta_Campaign\n  @key(fields: \"id\")\n  @connect(source: \"meta\", http: { GET: \"/{$this.id}\" }, selection: \"id name daily_budget\")\n{",
    )
}

#[test]
fn a_type_level_connector_is_attributed_to_the_entity_entry_whose_field_returns_its_type() {
    let sdl = entity_sdl();
    let report = reconcile(&Inputs {
        decisions: None,
        workspace: &yaml(WORKSPACE),
        selection: &yaml(ENTITY_SELECTION),
        inventory: &inventory(),
        sdl: &sdl,
        baseline: None,
        schema_file: "meta.graphql",
        lock: None,
        today: "2026-09-23",
    });
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    assert_eq!(report["clean"], true, "{}", report);

    // The entity's unit entry targets the type, as scaffold writes it.
    let ws = validate_workspace("campaign", "Meta_Campaign");
    write(ws.path(), ".factory/selection.yaml", ENTITY_SELECTION);
    write(ws.path(), "meta.graphql", &sdl);
    let report = graphos_factory_core::cmd::validate::validate_workspace(ws.path())
        .unwrap()
        .json;
    let r = result(&report, "tests/meta.connector.yaml › campaign");
    assert_eq!(r["operation"], "get:/{campaign_id}", "{}", r);
    assert_eq!(r["status"], "fail", "{}", r);
}

// ─── An unattributed span is a tie to settle, never an edit to codify ────────

/// A second bare-id root field no selection entry names.
fn sdl_with_meta_ad() -> String {
    SDL.replace(
        "type Query {\n",
        "type Query {\n  meta_ad(adId: ID!): Meta_Campaign\n    @connect(source: \"meta\", http: { GET: \"/{$args.adId}\" }, selection: \"id name\")\n",
    )
}

fn codify(dir: &std::path::Path, args: &[&str]) -> i32 {
    let mut argv = vec![dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::codify::main(&argv)
}

fn lock(dir: &std::path::Path, args: &[&str]) -> i32 {
    let mut argv = vec![dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::lock::main(&argv)
}

fn lint_rules(dir: &std::path::Path) -> Vec<(String, String)> {
    graphos_factory_core::lint::lint_workspace(
        dir,
        &graphos_factory_core::lint::LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings
    .into_iter()
    .map(|f| (f.rule, f.message))
    .collect()
}

#[test]
fn codify_refuses_a_span_lock_refuses() {
    let dir = lock_workspace(SELECTION);
    assert_eq!(lock(dir.path(), &[]), 0);
    write(dir.path(), "meta.graphql", &sdl_with_meta_ad());
    assert_eq!(lock(dir.path(), &[]), 2);
    let code = codify(
        dir.path(),
        &["--key", "Query.meta_ad", "--reason", "hand edit", "--pin"],
    );
    assert_eq!(code, 2);
    let applied = std::fs::read_to_string(dir.path().join(".factory/applied.lock.yaml")).unwrap();
    assert!(!applied.contains("Query.meta_ad"), "{}", applied);
    let selection = std::fs::read_to_string(dir.path().join(".factory/selection.yaml")).unwrap();
    assert_eq!(selection, SELECTION);
}

#[test]
fn lock_check_and_lint_report_an_unattributed_span_as_a_tie_not_an_edit() {
    let dir = lock_workspace(SELECTION);
    assert_eq!(lock(dir.path(), &[]), 0);
    write(dir.path(), "meta.graphql", &sdl_with_meta_ad());
    let tie_not_edit = |rules: &[(String, String)]| {
        assert!(
            rules.iter().any(|(r, m)| r == "unattributed-span"
                && m.starts_with("Query.meta_ad: GET /{$args.adId} matches 2 operations equally")),
            "{:?}",
            rules
        );
        assert!(
            !rules.iter().any(|(r, _)| r == "unacknowledged-edit"),
            "{:?}",
            rules
        );
    };
    assert_eq!(lock(dir.path(), &["--check"]), 3);
    tie_not_edit(&lint_rules(dir.path()));
    // Reported at the field's line in the schema, not in selection.yaml.
    let finding = graphos_factory_core::lint::lint_workspace(
        dir.path(),
        &graphos_factory_core::lint::LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    )
    .findings
    .into_iter()
    .find(|f| f.rule == "unattributed-span")
    .unwrap();
    assert_eq!(finding.file.as_deref(), Some("meta.graphql"));
    assert_eq!(finding.line, Some(14));

    // What the old codify left behind: a lock that records `Query.meta_ad`,
    // after which `lock --check` read green.
    let hints = OpHints::from_selection(Some(&yaml(WORKSPACE)), Some(&yaml(SELECTION)), None);
    let current =
        graphos_factory_core::spans::spans(&sdl_with_meta_ad(), Some(&inventory()), &hints);
    graphos_factory_core::spans::write_lock(
        dir.path(),
        &graphos_factory_core::spans::lock_document("meta.graphql", &current),
    )
    .unwrap();
    assert_eq!(lock(dir.path(), &["--check"]), 3);
    tie_not_edit(&lint_rules(dir.path()));
}

// ─── Every hint wiring has a test that fails without it ─────────────────────

#[test]
fn lint_finds_the_hinted_workspace_in_sync_with_its_lock() {
    let dir = lock_workspace(SELECTION);
    assert_eq!(lock(dir.path(), &[]), 0);
    let rules = lint_rules(dir.path());
    assert!(
        !rules
            .iter()
            .any(|(r, _)| r == "unacknowledged-edit" || r == "unattributed-span"),
        "{:?}",
        rules
    );
}

#[test]
fn obligations_read_the_field_the_selection_declares_for_a_bare_id_operation() {
    let dir = lock_workspace(SELECTION);
    let report =
        graphos_factory_core::obligations::build(dir.path(), "get:/{campaign_id}").unwrap();
    assert_eq!(report.response_counts().unaccounted, 0);
    assert!(!report.response.is_empty());
}

#[test]
fn codify_finds_a_bare_id_span_by_the_operation_key_the_selection_declares() {
    let dir = lock_workspace(SELECTION);
    assert_eq!(lock(dir.path(), &[]), 0);
    write(
        dir.path(),
        "meta.graphql",
        &SDL.replace(
            "selection: \"id name daily_budget\")",
            "selection: \"id name\")",
        ),
    );
    let code = codify(
        dir.path(),
        &[
            "--key",
            "get:/{campaign_id}",
            "--reason",
            "hand edit",
            "--pin",
        ],
    );
    assert_eq!(code, 0);
}

#[test]
fn an_unattributed_span_withholds_the_relock_advice() {
    // The tie is not a hand edit (no span is reported changed), but the
    // schema is a recorded provenance output, so it drifts. A relock would
    // refuse to write (exit 2) until the selection names the operation, so
    // the drift listing must not advise one.
    let dir = lock_workspace(SELECTION);
    assert_eq!(lock(dir.path(), &[]), 0);
    write(dir.path(), "meta.graphql", &sdl_with_meta_ad());
    let bin = env!("CARGO_BIN_EXE_graphos-factory-bare");
    let ws = dir.path().to_str().unwrap();
    let text = std::process::Command::new(bin)
        .args(["lock", ws, "--check"])
        .output()
        .unwrap();
    assert_eq!(text.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("~ meta.graphql   (outputs)"), "{}", stdout);
    assert!(
        !stdout.contains("record the workspace as it stands"),
        "{}",
        stdout
    );
    assert!(stdout.contains("do not relock yet"), "{}", stdout);
    let json = std::process::Command::new(bin)
        .args(["lock", ws, "--check", "--json"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["hand_edits"], json!([]), "{}", report);
    assert_eq!(report["unattributed"].as_array().unwrap().len(), 1);
    assert_eq!(report["relock_advisable"], false);
}

// ─── A field-level connector on a bare-id path: the links entry declares it ─

/// `Campaign.ad_id` carries the fact for `get:/{ad_id}`.
fn link_inventory() -> Value {
    let mut inv = inventory();
    inv["shapes"]["Campaign"]["properties"]["ad_id"] = json!({"type": "string", "candidate_entity_link":
        {"operation": "get:/{ad_id}", "parameter": "ad_id", "list_context": false}});
    inv
}

const LINK_SELECTION: &str = "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/{ad_id}\":\n    include: true\n    graphql: { root: query, name: ad }\n  \"get:/{campaign_id}\":\n    include: true\n    graphql: { root: query, name: campaign }\nlinks:\n  - shape: Campaign\n    path: ad_id\n    operation: \"get:/{ad_id}\"\n    parameter: ad_id\n    field: ad\n    include: true\n    confirmed: true\n";

/// The same selection with no `links:` section.
fn unlinked_selection() -> String {
    LINK_SELECTION[..LINK_SELECTION.find("links:").unwrap()].to_string()
}

fn link_sdl() -> String {
    SDL.replace(
        "type Meta_Campaign {\n  id: ID!\n  name: String\n  daily_budget: String\n}\n",
        concat!(
            "type Meta_Ad {\n  id: ID!\n  name: String\n}\n\n",
            "type Meta_Campaign {\n  id: ID!\n  name: String\n  daily_budget: String\n  ad_id: ID\n",
            "  ad: Meta_Ad\n    @connect(source: \"meta\", http: { GET: \"/{$this.ad_id}\" }, selection: \"id name\")\n}\n"
        ),
    )
    .replace(
        "selection: \"id name daily_budget\")",
        "selection: \"id name daily_budget ad_id\")",
    )
    .replace(
        "type Query {\n",
        "type Query {\n  meta_ad(adId: ID!): Meta_Ad\n    @connect(source: \"meta\", http: { GET: \"/{$args.adId}\" }, selection: \"id name\")\n",
    )
}

fn link_report(selection: &Value) -> Value {
    reconcile(&Inputs {
        decisions: None,
        workspace: &yaml(WORKSPACE),
        selection,
        inventory: &link_inventory(),
        sdl: &link_sdl(),
        baseline: None,
        schema_file: "meta.graphql",
        lock: None,
        today: "2026-09-28",
    })
}

#[test]
fn a_field_level_connector_on_a_bare_id_path_is_settled_by_its_links_entry() {
    // Without the entry, `/{$this.ad_id}` matches both bare-id operations and
    // nothing declares which: an error naming the candidates, never the
    // inventory's order (ADR 0044).
    let report = link_report(&yaml(&unlinked_selection()));
    let errors: Vec<&str> = report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        errors.iter().any(|e| e.starts_with(
            "Meta_Campaign.ad: GET /{$this.ad_id} matches 2 operations equally (get:/{ad_id}, get:/{campaign_id}) and nothing declares which"
        )),
        "{:?}",
        errors
    );
    assert_eq!(report["clean"], false);

    // With it, the link's operation settles the tie and the field is in step.
    let report = link_report(&yaml(LINK_SELECTION));
    assert_eq!(report["selection_errors"], json!([]), "{}", report);
    assert_eq!(
        report["links"]["unchanged"].as_array().unwrap().len(),
        1,
        "{}",
        report
    );
    assert_eq!(report["links"]["unchanged"][0]["operation"], "get:/{ad_id}");
    assert_eq!(report["links"]["remove"], json!([]));
    assert_eq!(report["clean"], true, "{}", report);
}

#[test]
fn a_links_entry_with_a_reference_problem_settles_no_tie() {
    // R38: a link whose operation or path does not resolve never reaches
    // `link_hosts`, so `with_links` declares nothing for any field — even
    // on hints a caller builds from every entry.
    let sdl = link_sdl();
    let inv = link_inventory();
    let sel = yaml(LINK_SELECTION);
    let links = read_links(&sel);
    let declared = |links: &[Link]| {
        OpHints::from_selection(Some(&yaml(WORKSPACE)), Some(&sel), Some(&sdl))
            .with_links(links, &inv, &sdl)
            .for_link("Meta_Campaign", "ad")
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    assert_eq!(declared(&links), vec!["get:/{ad_id}"]);
    let unknown_operation = Link {
        operation: "get:/nowhere/{id}".to_string(),
        ..links[0].clone()
    };
    assert_eq!(declared(&[unknown_operation]), Vec::<String>::new());
    let unknown_path = Link {
        path: "ad_ident".to_string(),
        ..links[0].clone()
    };
    assert_eq!(declared(&[unknown_path]), Vec::<String>::new());

    // Reconcile hands `with_links` only the entries with no reference
    // problem at all: a link through an excluded by-id operation is a
    // selection error, and the tie it would have settled stays one.
    let excluded = LINK_SELECTION.replace(
        "  \"get:/{ad_id}\":\n    include: true\n",
        "  \"get:/{ad_id}\":\n    include: false\n    reason: not in this slice\n",
    );
    assert_ne!(excluded, LINK_SELECTION);
    let report = link_report(&yaml(&excluded));
    let errors: Vec<&str> = report["selection_errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        errors.iter().any(|e| e
            .starts_with("links[0] (Campaign > ad_id): operation get:/{ad_id} is not included")),
        "{:?}",
        errors
    );
    assert!(
        errors.iter().any(|e| e.starts_with(
            "Meta_Campaign.ad: GET /{$this.ad_id} matches 2 operations equally (get:/{ad_id}, get:/{campaign_id}) and nothing declares which"
        )),
        "{:?}",
        errors
    );
    assert_eq!(report["links"]["unchanged"], json!([]), "{}", report);
}

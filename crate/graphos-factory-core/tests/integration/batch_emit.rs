//! Step 2 of `$batch` (ADR 0071): the selection's `graphql.batch`
//! judgement, `batch find`'s paste-ready connector draft (no instrument
//! writes the schema, plan.md §3.1), lint's `batchable-entity-unbatched`,
//! and scaffold's `$batch` case, whose lookup stub demands the exact key
//! list. The e2e proof of these files is in the ADR; here each piece is
//! pinned on `tests/fixtures/stay-listings/` (artifacts main 22b3de9a),
//! with Amenity keyed through `GET /amenities?ids=`.

use graphos_factory_core::batch::find;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stay-listings")
}

const SCHEMA: &str = "stay-listings.graphql";
const AMENITIES_GRAPHQL: &str =
    "    graphql:\n      root: query\n      name: amenities\n      description: null\n";

/// A copy of the fixture with Amenity keyed by `get:/amenities`, its
/// `graphql.batch` set to `batch` (None leaves it out), and `inventory`
/// applied to the inventory.
fn workspace(batch: Option<bool>, inventory: impl Fn(&mut Value)) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for rel in [
        SCHEMA,
        "template.yaml",
        ".factory/workspace.yaml",
        ".factory/inventory.json",
        ".factory/selection.yaml",
    ] {
        let to = dir.path().join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(fixture().join(rel), to).unwrap();
    }
    let sel_path = dir.path().join(".factory/selection.yaml");
    let sel = std::fs::read_to_string(&sel_path).unwrap();
    assert_eq!(sel.matches(AMENITIES_GRAPHQL).count(), 1);
    let judgement = batch
        .map(|b| format!("      batch: {}\n", b))
        .unwrap_or_default();
    std::fs::write(
        &sel_path,
        sel.replace(
            AMENITIES_GRAPHQL,
            &format!(
                "{}      entity: true\n      key: id\n{}",
                AMENITIES_GRAPHQL, judgement
            ),
        ),
    )
    .unwrap();
    let inv_path = dir.path().join(".factory/inventory.json");
    let mut inv: Value =
        serde_json::from_str(&std::fs::read_to_string(&inv_path).unwrap()).unwrap();
    inventory(&mut inv);
    std::fs::write(&inv_path, serde_json::to_string_pretty(&inv).unwrap()).unwrap();
    dir
}

fn load(dir: &Path) -> (Value, Value, String) {
    let inv = serde_json::from_str(
        &std::fs::read_to_string(dir.join(".factory/inventory.json")).unwrap(),
    )
    .unwrap();
    let sel = graphos_factory_core::yaml::parse(
        &std::fs::read_to_string(dir.join(".factory/selection.yaml")).unwrap(),
    )
    .unwrap();
    (inv, sel, std::fs::read_to_string(dir.join(SCHEMA)).unwrap())
}

fn amenity_draft(dir: &Path) -> Option<String> {
    let (inv, sel, sdl) = load(dir);
    find(&inv, Some(&sel), &sdl, false)
        .unwrap()
        .into_iter()
        .find(|r| r.type_name == "Amenity")
        .unwrap()
        .draft
}

/// Paste the draft over Amenity's type header, as the agent does.
fn paste(dir: &Path) {
    let draft = amenity_draft(dir).expect("a draft");
    let sdl = std::fs::read_to_string(dir.join(SCHEMA)).unwrap();
    let header = "type Stay_Listings_Amenity {";
    assert_eq!(sdl.matches(header).count(), 1);
    std::fs::write(
        dir.join(SCHEMA),
        sdl.replace(header, &format!("{} {{", draft)),
    )
    .unwrap();
}

fn op_mut<'a>(inv: &'a mut Value, key: &str) -> &'a mut Value {
    inv["operations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|o| o["key"] == key)
        .unwrap()
}

/// `GET /amenities` takes `ids` as an exploded array: the repeated form.
fn repeated(inv: &mut Value) {
    op_mut(inv, "get:/amenities")["parameters"] = serde_json::json!([{ "name": "ids", "in": "query", "required": true, "type": "array", "explode": true }]);
}

/// The lookup is `POST /amenities` with `{"ids": [...]}` (max 25).
fn body(inv: &mut Value) {
    op_mut(inv, "get:/amenities")["parameters"] = serde_json::json!([]);
    let mut post = op_mut(inv, "get:/amenities").clone();
    post["key"] = "post:/amenities".into();
    post["method"] = "POST".into();
    post["request_body"] = serde_json::json!({ "content_type": "application/json", "required": true, "shape_ref": "#/shapes/AmenityIds" });
    inv["operations"].as_array_mut().unwrap().push(post);
    inv["shapes"]["AmenityIds"] = serde_json::json!({
        "type": "object",
        "properties": { "ids": { "type": "array", "items": { "type": "string" }, "maxItems": 25 } }
    });
}

#[test]
fn an_array_returning_entity_operation_keys_its_item_type() {
    let ws = workspace(Some(true), |_| {});
    let (inv, sel, sdl) = load(ws.path());
    let amenity = find(&inv, Some(&sel), &sdl, false)
        .unwrap()
        .into_iter()
        .find(|r| r.type_name == "Amenity")
        .expect("Amenity is keyed");
    assert_eq!(amenity.keyed_by.as_deref(), Some("get:/amenities"));
    assert_eq!(amenity.batch, Some(true));
    assert_eq!(amenity.sdl_type.as_deref(), Some("Stay_Listings_Amenity"));
}

#[test]
fn the_draft_is_the_connector_proven_at_e2e() {
    let ws = workspace(Some(true), |_| {});
    assert_eq!(
        amenity_draft(ws.path()).unwrap(),
        "type Stay_Listings_Amenity\n  @key(fields: \"id\")\n  @connect(\n    source: \"stay_listings\"\n    http: { GET: \"/amenities\", queryParams: \"ids: $batch.id->joinNotNull(',')\" }\n    selection: \"id category name\"\n  )"
    );
}

#[test]
fn the_repeated_and_body_drafts_use_their_own_forms() {
    let rep = workspace(Some(true), repeated);
    assert!(amenity_draft(rep.path())
        .unwrap()
        .contains("http: { GET: \"/amenities\", queryParams: \"ids: $batch.id\" }"));
    let b = workspace(Some(true), body);
    let draft = amenity_draft(b.path()).unwrap();
    assert!(
        draft.contains("http: { POST: \"/amenities\", body: \"ids: $batch.id\" }"),
        "{}",
        draft
    );
    assert!(draft.contains("batch: { maxSize: 25 }"), "{}", draft);
}

#[test]
fn no_draft_when_the_lookup_needs_another_path_parameter() {
    let ws = workspace(Some(true), |inv| {
        let op = op_mut(inv, "get:/amenities");
        op["path"] = "/sites/{siteId}/amenities".into();
        op["parameters"].as_array_mut().unwrap().push(
            serde_json::json!({ "name": "siteId", "in": "path", "required": true, "type": "string" }),
        );
    });
    let (inv, sel, sdl) = load(ws.path());
    let amenity = find(&inv, Some(&sel), &sdl, false)
        .unwrap()
        .into_iter()
        .find(|r| r.type_name == "Amenity")
        .unwrap();
    assert!(amenity.draft.is_none());
    assert!(amenity.draft_note.unwrap().contains("siteId"));
}

fn lint_rules(dir: &Path) -> Vec<String> {
    let result = graphos_factory_core::lint::lint_workspace(
        dir,
        &graphos_factory_core::lint::LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    );
    result
        .findings
        .iter()
        .filter(|f| f.severity == "error")
        .map(|f| f.rule.clone())
        .collect()
}

#[test]
fn lint_errors_until_the_connector_is_pasted_or_batching_is_declined() {
    let undecided = workspace(None, |_| {});
    assert!(lint_rules(undecided.path()).contains(&"batchable-entity-unbatched".to_string()));
    let batch = workspace(Some(true), |_| {});
    assert!(lint_rules(batch.path()).contains(&"batchable-entity-unbatched".to_string()));
    paste(batch.path());
    assert!(!lint_rules(batch.path()).contains(&"batchable-entity-unbatched".to_string()));
    let declined = workspace(Some(false), |_| {});
    assert!(!lint_rules(declined.path()).contains(&"batchable-entity-unbatched".to_string()));
    // The fixture itself keys only Listing, which has no bulk lookup.
    assert!(!lint_rules(&fixture()).contains(&"batchable-entity-unbatched".to_string()));
}

fn scaffold(dir: &Path, extra: &[&str]) -> i32 {
    let mut argv = vec![dir.to_string_lossy().to_string()];
    argv.extend(extra.iter().map(|s| s.to_string()));
    graphos_factory_core::cmd::scaffold::main(&argv)
}

fn mapping(dir: &Path, stem: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(dir.join(format!("tests/fixtures/mappings/{}.json", stem)))
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn scaffold_writes_a_case_whose_lookup_stub_demands_the_comma_separated_keys() {
    let ws = workspace(Some(true), |_| {});
    paste(ws.path());
    assert_eq!(scaffold(ws.path(), &["--op", "batch:Amenity"]), 0);
    let case =
        std::fs::read_to_string(ws.path().join("tests/cases/amenity_batch.graphql")).unwrap();
    assert!(case.contains("stay_listings_listings {"), "{}", case);
    assert!(
        case.contains("amenities {\n      id\n      category\n      name\n"),
        "{}",
        case
    );

    let root = mapping(ws.path(), "amenity_batch");
    assert_eq!(root["request"]["urlPath"], "/listings");
    for k in ["page", "limit", "sortBy", "numOfBeds"] {
        assert_eq!(
            root["request"]["queryParameters"][k]["absent"], true,
            "{}",
            k
        );
    }
    let items = root["response"]["jsonBody"].as_array().unwrap();
    assert_eq!(
        items[0]["amenities"],
        serde_json::json!([{ "id": "id-1" }, { "id": "id-2" }])
    );
    assert_eq!(
        items[1]["amenities"],
        serde_json::json!([{ "id": "id-2" }, { "id": "id-3" }])
    );

    let lookup = mapping(ws.path(), "amenity_batch_lookup");
    assert_eq!(lookup["request"]["urlPath"], "/amenities");
    assert_eq!(
        lookup["request"]["queryParameters"]["ids"]["equalTo"],
        "id-1,id-2,id-3"
    );
    assert_eq!(
        lookup["metadata"]["x-cases"],
        serde_json::json!(["amenity_batch"])
    );
    // e2e fails the case unless the router called this stub.
    assert_eq!(lookup["metadata"]["x-required"], true);
    let ids: Vec<&str> = lookup["response"]["jsonBody"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["id-1", "id-2", "id-3"]);
}

#[test]
fn scaffold_demands_repeated_keys_with_has_exactly_and_a_body_array_with_equal_to_json() {
    let rep = workspace(Some(true), repeated);
    paste(rep.path());
    assert_eq!(scaffold(rep.path(), &["--op", "batch:Amenity"]), 0);
    assert_eq!(
        mapping(rep.path(), "amenity_batch_lookup")["request"]["queryParameters"]["ids"]
            ["hasExactly"],
        serde_json::json!([{ "equalTo": "id-1" }, { "equalTo": "id-2" }, { "equalTo": "id-3" }])
    );
    let b = workspace(Some(true), body);
    paste(b.path());
    assert_eq!(scaffold(b.path(), &["--op", "batch:Amenity"]), 0);
    let lookup = mapping(b.path(), "amenity_batch_lookup");
    assert_eq!(lookup["request"]["method"], "POST");
    assert_eq!(
        lookup["request"]["bodyPatterns"],
        serde_json::json!([{ "equalToJson": { "ids": ["id-1", "id-2", "id-3"] } }])
    );
}

#[test]
fn scaffold_writes_no_batch_case_without_a_batch_connector_and_refuses_an_unknown_type() {
    let ws = workspace(Some(true), |_| {});
    // The fixture has no tests yet: every operation gets its case, and no
    // batch case is written before the connector is pasted.
    assert_eq!(scaffold(ws.path(), &[]), 0);
    assert!(ws.path().join("tests/cases/listings.graphql").exists());
    assert!(!ws.path().join("tests/cases/amenity_batch.graphql").exists());
    assert_eq!(scaffold(ws.path(), &["--op", "batch:Amenity"]), 1);
    assert_eq!(scaffold(ws.path(), &["--op", "batch:Nope"]), 1);
}

#[test]
fn scaffold_skips_a_batch_case_when_no_field_references_the_entity() {
    // Retype the only field that reaches Amenity from a Query root.
    let ws = workspace(Some(true), |_| {});
    paste(ws.path());
    let sdl = std::fs::read_to_string(ws.path().join(SCHEMA)).unwrap();
    let field = "  amenities: [Stay_Listings_Amenity!]\n}";
    assert_eq!(sdl.matches(field).count(), 1);
    std::fs::write(
        ws.path().join(SCHEMA),
        sdl.replace(field, "  amenities: [String!]\n}"),
    )
    .unwrap();
    let code = scaffold(ws.path(), &["--op", "batch:Amenity", "--json"]);
    assert_eq!(code, 2, "a skip only: nothing written");
    assert!(!ws.path().join("tests/cases/amenity_batch.graphql").exists());
}

fn amenity_note(dir: &Path) -> String {
    let (inv, sel, sdl) = load(dir);
    let amenity = find(&inv, Some(&sel), &sdl, false)
        .unwrap()
        .into_iter()
        .find(|r| r.type_name == "Amenity")
        .unwrap();
    assert!(
        amenity.draft.is_none(),
        "no draft for {:?}",
        amenity.verdict
    );
    amenity.draft_note.unwrap()
}

#[test]
fn no_draft_for_a_partial_shape_names_the_missing_fields() {
    // get:/amenities stays the entity operation (Amenity) without a key
    // list; the only key-list lookup answers AmenitySummary, which lacks
    // `name`.
    let ws = workspace(Some(true), |inv| {
        let mut search = op_mut(inv, "get:/amenities").clone();
        op_mut(inv, "get:/amenities")["parameters"] = serde_json::json!([]);
        search["key"] = "get:/amenities/search".into();
        search["path"] = "/amenities/search".into();
        search["response"]["shape_ref"] = "#/shapes/AmenitySummaries".into();
        inv["operations"].as_array_mut().unwrap().push(search);
        inv["shapes"]["AmenitySummary"] = serde_json::json!({
            "type": "object", "properties": { "id": { "type": "string" }, "category": { "type": "string" } }
        });
        inv["shapes"]["AmenitySummaries"] =
            serde_json::json!({ "type": "array", "items": { "$ref": "#/shapes/AmenitySummary" } });
    });
    let note = amenity_note(ws.path());
    assert!(
        note.starts_with("partial-shape: AmenitySummary lacks name of Amenity"),
        "{}",
        note
    );
}

#[test]
fn no_draft_when_the_lookup_needs_scope_names_the_unbound_parameters() {
    let ws = workspace(Some(true), |inv| {
        op_mut(inv, "get:/amenities")["parameters"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "name": "siteId", "in": "query", "required": true, "type": "string" }));
    });
    let note = amenity_note(ws.path());
    assert!(
        note.starts_with("needs-scope:") && note.contains("siteId (query)"),
        "{}",
        note
    );
}

#[test]
fn no_draft_for_a_style_conflict_names_both_forms() {
    let ws = workspace(Some(true), |inv| {
        op_mut(inv, "get:/amenities")["parameters"] = serde_json::json!([{
            "name": "ids", "in": "query", "required": true, "type": "array",
            "description": "Comma-separated amenity ids."
        }]);
    });
    let note = amenity_note(ws.path());
    assert!(
        note.starts_with("style-conflict:")
            && note.contains("repeated")
            && note.contains("comma-separated"),
        "{}",
        note
    );
}

#[test]
fn no_draft_for_a_list_without_a_key_filter() {
    let ws = workspace(Some(true), |inv| {
        op_mut(inv, "get:/amenities")["parameters"] = serde_json::json!([]);
    });
    let note = amenity_note(ws.path());
    assert_eq!(
        note,
        "list-no-key-filter: no operation that lists Amenity takes a list of `id`"
    );
}

#[test]
fn scaffold_refuses_a_batch_case_for_a_type_that_is_not_batchable() {
    // The connector was pasted when the lookup was clean; the inventory
    // now says a required companion is unbound: nothing to prove against.
    let ws = workspace(Some(true), |_| {});
    paste(ws.path());
    let inv_path = ws.path().join(".factory/inventory.json");
    let mut inv: Value =
        serde_json::from_str(&std::fs::read_to_string(&inv_path).unwrap()).unwrap();
    op_mut(&mut inv, "get:/amenities")["parameters"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "name": "siteId", "in": "query", "required": true, "type": "string" }));
    std::fs::write(&inv_path, serde_json::to_string_pretty(&inv).unwrap()).unwrap();
    assert_eq!(
        scaffold(ws.path(), &["--op", "batch:Amenity"]),
        2,
        "skipped, nothing written"
    );
    assert!(!ws.path().join("tests/cases/amenity_batch.graphql").exists());
}

/// Amenity's `category` on the wire is `amenity_category`, and the schema's
/// field is `graphql` (the name the other connectors alias it to).
fn rename_category(dir: &Path, graphql: &str) {
    let inv_path = dir.join(".factory/inventory.json");
    let mut inv: Value =
        serde_json::from_str(&std::fs::read_to_string(&inv_path).unwrap()).unwrap();
    let amenity = &mut inv["shapes"]["Amenity"];
    let props = amenity["properties"].as_object_mut().unwrap();
    let category = props.remove("category").unwrap();
    props.insert("amenity_category".into(), category);
    amenity["required"] = serde_json::json!(["id", "amenity_category", "name"]);
    std::fs::write(&inv_path, serde_json::to_string_pretty(&inv).unwrap()).unwrap();
    let sdl = std::fs::read_to_string(dir.join(SCHEMA)).unwrap();
    let field = "type Stay_Listings_Amenity {\n  id: ID!\n  category: String\n";
    assert_eq!(sdl.matches(field).count(), 1);
    std::fs::write(
        dir.join(SCHEMA),
        sdl.replace(
            field,
            &format!(
                "type Stay_Listings_Amenity {{\n  id: ID!\n  {}: String\n",
                graphql
            ),
        ),
    )
    .unwrap();
}

#[test]
fn the_draft_selects_the_wire_name_of_a_field_the_schema_renames() {
    let ws = workspace(Some(true), |_| {});
    rename_category(ws.path(), "amenityCategory");
    let draft = amenity_draft(ws.path()).unwrap();
    assert!(
        draft.contains("selection: \"id amenityCategory: amenity_category name\""),
        "{}",
        draft
    );
}

#[test]
fn no_draft_when_a_field_has_no_wire_property_it_maps_to() {
    // `kind` is no generator's name for `amenity_category`: the draft
    // would select a property the response lacks, so there is none.
    let ws = workspace(Some(true), |_| {});
    rename_category(ws.path(), "kind");
    let note = amenity_note(ws.path());
    assert!(
        note.contains("`kind`") && note.contains("amenity_category"),
        "{}",
        note
    );
}

#[test]
fn the_draft_leaves_out_a_field_with_its_own_connector() {
    let ws = workspace(Some(true), |_| {});
    let sdl = std::fs::read_to_string(ws.path().join(SCHEMA)).unwrap();
    let field = "  name: String\n}\n\n\"A listing's geographic coordinates.\"";
    assert_eq!(sdl.matches(field).count(), 1);
    std::fs::write(
        ws.path().join(SCHEMA),
        sdl.replace(
            field,
            "  name: String\n  usage(since: String): Int\n    @connect(\n      source: \"stay_listings\"\n      http: { GET: \"/amenities/{$this.id}/usage\" }\n      selection: \"$.count\"\n    )\n}\n\n\"A listing's geographic coordinates.\"",
        ),
    )
    .unwrap();
    let draft = amenity_draft(ws.path()).unwrap();
    assert!(
        draft.contains("selection: \"id category name\""),
        "{}",
        draft
    );
}

/// Map Amenity's other fields in root field `field`'s own selection.
fn select_amenity_fields_in(dir: &Path, after: &str) {
    let sdl = std::fs::read_to_string(dir.join(SCHEMA)).unwrap();
    let at = sdl.find(after).expect("root field");
    let key_only = "      amenities { id }\n";
    let rel = sdl[at..].find(key_only).expect("its amenities");
    let mut out = sdl.clone();
    out.replace_range(
        at + rel..at + rel + key_only.len(),
        "      amenities { id category name }\n",
    );
    std::fs::write(dir.join(SCHEMA), out).unwrap();
}

#[test]
fn the_batch_case_goes_through_a_root_whose_selection_maps_the_key_alone() {
    // `listings` now maps every Amenity field itself: the planner resolves
    // them there and never calls the lookup, so the case must go through
    // `featuredListings`, which still maps `amenities { id }`.
    let ws = workspace(Some(true), |_| {});
    paste(ws.path());
    select_amenity_fields_in(ws.path(), "stay_listings_listings(");
    assert_eq!(scaffold(ws.path(), &["--op", "batch:Amenity"]), 0);
    let case =
        std::fs::read_to_string(ws.path().join("tests/cases/amenity_batch.graphql")).unwrap();
    assert!(
        case.contains("stay_listings_featuredListings {"),
        "{}",
        case
    );
    assert_eq!(
        mapping(ws.path(), "amenity_batch")["request"]["urlPath"],
        "/featured-listings"
    );
}

#[test]
fn scaffold_refuses_a_batch_case_when_every_root_maps_the_entitys_fields() {
    let ws = workspace(Some(true), |_| {});
    paste(ws.path());
    select_amenity_fields_in(ws.path(), "stay_listings_listings(");
    select_amenity_fields_in(ws.path(), "stay_listings_featuredListings(");
    assert_eq!(
        scaffold(ws.path(), &["--op", "batch:Amenity"]),
        2,
        "skipped, nothing written"
    );
    assert!(!ws.path().join("tests/cases/amenity_batch.graphql").exists());
}

#[test]
fn the_json_report_gives_no_draft_note_once_the_connector_is_pasted() {
    // Pasted, then the lookup gained a required companion: the verdict is
    // needs-scope and find records why it would not draft, but the type
    // has its connector, so neither the draft nor its note is shown.
    let ws = workspace(Some(true), |_| {});
    paste(ws.path());
    let inv_path = ws.path().join(".factory/inventory.json");
    let mut inv: Value =
        serde_json::from_str(&std::fs::read_to_string(&inv_path).unwrap()).unwrap();
    op_mut(&mut inv, "get:/amenities")["parameters"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "name": "siteId", "in": "query", "required": true, "type": "string" }));
    std::fs::write(&inv_path, serde_json::to_string_pretty(&inv).unwrap()).unwrap();
    let (inv, sel, sdl) = load(ws.path());
    let amenity = find(&inv, Some(&sel), &sdl, false)
        .unwrap()
        .into_iter()
        .find(|r| r.type_name == "Amenity")
        .unwrap();
    assert!(amenity.has_batch_connector && amenity.draft_note.is_some());
    let json = graphos_factory_core::cmd::batch::report_json(&amenity);
    assert_eq!(json["draft"], Value::Null);
    assert_eq!(json["draft_note"], Value::Null, "{}", json);
}

fn lint_message(dir: &Path) -> String {
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
    .find(|f| f.rule == "batchable-entity-unbatched")
    .expect("the finding")
    .message
}

#[test]
fn the_lint_message_names_the_step_left() {
    let undecided = lint_message(workspace(None, |_| {}).path());
    assert!(
        undecided.contains("set graphql.batch: true and paste"),
        "{}",
        undecided
    );
    let decided = lint_message(workspace(Some(true), |_| {}).path());
    assert!(
        !decided.contains("set graphql.batch: true")
            && decided.contains("graphql.batch is true; paste"),
        "{}",
        decided
    );
    let undraftable = workspace(Some(true), |_| {});
    rename_category(undraftable.path(), "kind");
    let m = lint_message(undraftable.path());
    assert!(
        m.contains("has no draft (") && m.contains("`kind`") && m.contains("by hand"),
        "{}",
        m
    );
}

#[test]
fn the_draft_finds_its_schema_type_by_parse_and_shortest_suffix() {
    // Review of #110: the schema type came from a line-start text scan in
    // document order, so `P_Order_Item` won over `P_Item` for shape `Item`,
    // a `type P_Item` inside a description counted, and `extend type` did not.
    let inv = serde_json::json!({
        "contract_version": 1, "api": {},
        "operations": [
            { "key": "get:/items/{id}", "method": "GET", "path": "/items/{id}",
              "parameters": [{ "name": "id", "in": "path", "required": true, "type": "string" }],
              "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Item" } }
        ],
        "shapes": { "Item": { "type": "object", "properties": { "id": { "type": "string" } } } },
        "unresolved": []
    });
    let sel = serde_json::json!({ "contract_version": 1, "operations": {
        "get:/items/{id}": { "include": true, "graphql": { "root": "query", "name": "item", "entity": true, "key": "id" } }
    } });
    let sdl_type = |sdl: &str| {
        find(&inv, Some(&sel), sdl, false).unwrap()[0]
            .sdl_type
            .clone()
    };
    assert_eq!(
        sdl_type("type P_Order_Item { id: ID! }\ntype P_Item { id: ID! }\n").as_deref(),
        Some("P_Item")
    );
    assert_eq!(
        sdl_type("\"\"\"\ntype P_Item is described here\n\"\"\"\ntype Q { a: Int }\n"),
        None
    );
    assert_eq!(
        sdl_type("extend type P_Item @key(fields: \"id\") { id: ID! }\n").as_deref(),
        Some("P_Item")
    );
}

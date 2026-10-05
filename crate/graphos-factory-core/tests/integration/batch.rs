//! `batch find`: which keyed types a bulk-by-keys endpoint could resolve
//! with a `$batch` connector. Each verdict and each way a list of keys is
//! passed is pinned on a small inline inventory; the real-data cases are
//! `tests/fixtures/stay-listings/` (`GET
//! /amenities?ids=` returns `[Amenity]`, `GET /amenities/listings?ids=`
//! returns arrays with no listing id), `tests/fixtures/petstore-batch/`
//! (gen 8df53f4: `GET /user/findByNames`), and the gitea pilot, which has
//! nothing to batch (pagerduty, which has nothing either, is pinned in its
//! target's suite).

use graphos_factory_core::batch::{find, missing_batch, TypeReport, Verdict, INLINE};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn sel(ops: Value) -> Value {
    json!({ "contract_version": 1, "operations": ops })
}

fn entity(key: &str) -> Value {
    json!({ "include": true, "graphql": { "root": "query", "name": "thing", "entity": true, "key": key } })
}

fn report<'a>(reports: &'a [TypeReport], name: &str) -> &'a TypeReport {
    reports
        .iter()
        .find(|r| r.type_name == name)
        .unwrap_or_else(|| panic!("no report for {}: {:?}", name, reports))
}

/// An inventory with `Widget {id, name}`, its by-id lookup, and `list` as
/// the one operation returning `[Widget]`.
fn widgets(list: Value) -> Value {
    json!({
        "contract_version": 1,
        "api": {},
        "operations": [
            {
                "key": "get:/widgets/{id}", "method": "GET", "path": "/widgets/{id}",
                "parameters": [{ "name": "id", "in": "path", "required": true, "type": "string" }],
                "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Widget" }
            },
            list
        ],
        "shapes": {
            "Widget": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } },
            "WidgetList": { "type": "array", "items": { "$ref": "#/shapes/Widget" } },
            "WidgetPage": { "type": "object", "properties": { "items": { "type": "array", "items": { "$ref": "#/shapes/Widget" } }, "next": { "type": "string" } } },
            "WidgetIds": { "type": "object", "properties": { "widgetIds": { "type": "array", "items": { "type": "string" }, "maxItems": 100 } } }
        },
        "unresolved": []
    })
}

fn list_op(parameters: Value) -> Value {
    json!({
        "key": "get:/widgets", "method": "GET", "path": "/widgets",
        "parameters": parameters,
        "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetList", "root_is_array": true }
    })
}

fn widget_report(list: Value) -> TypeReport {
    let inv = widgets(list);
    let reports = find(
        &inv,
        Some(&sel(json!({ "get:/widgets/{id}": entity("id") }))),
        "",
        false,
    )
    .unwrap();
    assert_eq!(reports.len(), 1, "{:?}", reports);
    reports[0].clone()
}

#[test]
fn an_array_query_parameter_named_for_the_key_is_batchable_and_repeated() {
    let r = widget_report(list_op(json!([
        { "name": "ids", "in": "query", "required": false, "type": "array", "explode": true }
    ])));
    assert_eq!(r.verdict, Verdict::Batchable);
    assert_eq!(r.keyed_by.as_deref(), Some("get:/widgets/{id}"));
    let k = r.candidates[0].key_param.as_ref().unwrap();
    assert_eq!(
        (k.name.as_str(), k.location.as_str(), k.passing.as_str()),
        ("ids", "query", "repeated")
    );
    assert_eq!(k.max_size, None);
}

#[test]
fn form_style_without_explode_is_comma_separated() {
    let r = widget_report(list_op(json!([
        { "name": "widget_ids", "in": "query", "required": false, "type": "array", "style": "form", "explode": false }
    ])));
    assert_eq!(r.verdict, Verdict::Batchable);
    assert_eq!(
        r.candidates[0].key_param.as_ref().unwrap().passing,
        "comma-separated"
    );
}

#[test]
fn space_and_pipe_delimited_styles_name_their_delimiter() {
    for (style, passing) in [
        ("spaceDelimited", "space-delimited"),
        ("pipeDelimited", "pipe-delimited"),
    ] {
        let r = widget_report(list_op(json!([
            { "name": "ids", "in": "query", "required": false, "type": "array", "style": style }
        ])));
        assert_eq!(r.verdict, Verdict::Batchable, "{style}");
        assert_eq!(
            r.candidates[0].key_param.as_ref().unwrap().passing,
            passing,
            "{style}"
        );
    }
}

#[test]
fn a_deep_object_array_is_not_a_key_list() {
    let r = widget_report(list_op(json!([
        { "name": "ids", "in": "query", "required": false, "type": "array", "style": "deepObject" }
    ])));
    assert_eq!(r.verdict, Verdict::ListNoKeyFilter);
    assert!(r.candidates[0].key_param.is_none());
}

#[test]
fn a_string_parameter_documented_as_comma_separated_counts_with_its_stated_maximum() {
    let r = widget_report(list_op(json!([
        { "name": "id", "in": "query", "required": true, "type": "string",
          "description": "Comma-separated widget ids, up to 50 per request." }
    ])));
    assert_eq!(r.verdict, Verdict::Batchable);
    let k = r.candidates[0].key_param.as_ref().unwrap();
    assert_eq!(k.passing, "comma-separated");
    assert_eq!(k.max_size, Some(50));
}

#[test]
fn a_plain_scalar_key_parameter_is_one_lookup_not_a_batch() {
    let r = widget_report(list_op(json!([
        { "name": "id", "in": "query", "required": false, "type": "string", "description": "One widget id." }
    ])));
    assert_eq!(r.verdict, Verdict::ListNoKeyFilter);
    assert!(r.candidates[0].key_param.is_none());
}

#[test]
fn an_array_filter_on_another_field_is_a_list_without_a_key_filter() {
    // Petstore's findByTags: an array parameter, but of tags, not keys.
    let r = widget_report(list_op(json!([
        { "name": "tags", "in": "query", "required": false, "type": "array", "explode": true }
    ])));
    assert_eq!(r.verdict, Verdict::ListNoKeyFilter);
    assert_eq!(r.candidates.len(), 1);
}

#[test]
fn an_array_body_property_is_batchable_with_its_max_items() {
    let r = widget_report(json!({
        "key": "post:/widgets.lookup", "method": "POST", "path": "/widgets.lookup", "semantics": "read",
        "parameters": [],
        "request_body": { "content_type": "application/json", "required": true, "shape_ref": "#/shapes/WidgetIds" },
        "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetPage",
                      "array_root_properties": ["items"] }
    }));
    assert_eq!(r.verdict, Verdict::Batchable);
    let c = &r.candidates[0];
    assert_eq!(c.via, "envelope:items");
    let k = c.key_param.as_ref().unwrap();
    assert_eq!(
        (
            k.name.as_str(),
            k.location.as_str(),
            k.passing.as_str(),
            k.max_size
        ),
        ("widgetIds", "body", "array", Some(100))
    );
}

#[test]
fn a_path_parameter_documented_as_a_list_is_batchable() {
    let r = widget_report(json!({
        "key": "get:/widgets/batch/{ids}", "method": "GET", "path": "/widgets/batch/{ids}",
        "parameters": [{ "name": "ids", "in": "path", "required": true, "type": "array" }],
        "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetList", "root_is_array": true }
    }));
    assert_eq!(r.verdict, Verdict::Batchable);
    let k = r.candidates[0].key_param.as_ref().unwrap();
    assert_eq!(
        (k.location.as_str(), k.passing.as_str()),
        ("path", "comma-separated")
    );
}

#[test]
fn nothing_returning_an_array_of_the_type_is_none() {
    let r = widget_report(json!({
        "key": "get:/widgets/count", "method": "GET", "path": "/widgets/count",
        "parameters": [], "response": { "status": "200", "content_type": "application/json", "shape_ref": null }
    }));
    assert_eq!(r.verdict, Verdict::None);
    assert!(r.candidates.is_empty());
}

#[test]
fn a_write_method_is_never_a_candidate() {
    let r = widget_report(json!({
        "key": "put:/widgets", "method": "PUT", "path": "/widgets",
        "parameters": [{ "name": "ids", "in": "query", "required": true, "type": "array" }],
        "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetList", "root_is_array": true }
    }));
    assert_eq!(r.verdict, Verdict::None);
}

#[test]
fn a_compound_key_is_reported_with_a_note_and_no_candidates() {
    let inv = widgets(list_op(
        json!([{ "name": "ids", "in": "query", "required": true, "type": "array" }]),
    ));
    let reports = find(
        &inv,
        Some(&sel(json!({ "get:/widgets/{id}": entity("id name") }))),
        "",
        false,
    )
    .unwrap();
    assert_eq!(reports[0].verdict, Verdict::None);
    assert!(reports[0].note.as_deref().unwrap().contains("compound key"));
}

#[test]
fn an_excluded_entity_operation_keys_nothing() {
    let inv = widgets(list_op(json!([])));
    let mut e = entity("id");
    e["include"] = json!(false);
    assert!(find(
        &inv,
        Some(&sel(json!({ "get:/widgets/{id}": e }))),
        "",
        false
    )
    .unwrap()
    .is_empty());
    assert!(
        find(&inv, None, "", false).unwrap().is_empty(),
        "no selection keys nothing"
    );
}

#[test]
fn another_shape_of_the_same_entity_is_a_partial_shape_unless_it_carries_every_field() {
    // Jira (artifacts main 22b3de9a): `GET /group/bulk?groupId=` answers
    // GroupDetails {groupId, name}, which lacks Group's expand, self and
    // users; `GET /filter/search?id=` answers FilterDetails, which lacks
    // Filter's sharedUsers. A batch there would resolve partial records.
    // A shape carrying every field, extras allowed, is the full record.
    let get = |key: &str, path: &str, param: &str, shape: &str| {
        json!({ "key": key, "method": "GET", "path": path,
                "parameters": [{ "name": param, "in": "query", "required": false, "type": "array", "explode": true }],
                "response": { "status": "200", "content_type": "application/json", "shape_ref": shape, "sole_root_property": "values" } })
    };
    let one = |key: &str, path: &str, shape: &str| {
        json!({ "key": key, "method": "GET", "path": path, "parameters": [],
                "response": { "status": "200", "content_type": "application/json", "shape_ref": shape } })
    };
    let page = |item: &str| json!({ "type": "object", "properties": { "values": { "type": "array", "items": { "$ref": format!("#/shapes/{}", item) } } } });
    let obj = |props: &[&str]| {
        let mut p = serde_json::Map::new();
        for n in props {
            p.insert(n.to_string(), json!({ "type": "string" }));
        }
        json!({ "type": "object", "properties": p })
    };
    let inv = json!({
        "contract_version": 1, "api": {},
        "operations": [
            one("get:/group", "/group", "#/shapes/Group"),
            get("get:/group/bulk", "/group/bulk", "groupId", "#/shapes/PageBeanGroupDetails"),
            one("get:/filter/{id}", "/filter/{id}", "#/shapes/Filter"),
            get("get:/filter/search", "/filter/search", "id", "#/shapes/PageBeanFilterDetails"),
            one("get:/project/{id}", "/project/{id}", "#/shapes/Project"),
            get("get:/project/search", "/project/search", "id", "#/shapes/PageBeanProjectDetails"),
        ],
        "shapes": {
            "Group": obj(&["expand", "groupId", "name", "self", "users"]),
            "GroupDetails": obj(&["groupId", "name"]),
            "PageBeanGroupDetails": page("GroupDetails"),
            "Filter": obj(&["id", "jql", "name", "sharedUsers"]),
            "FilterDetails": obj(&["expand", "id", "jql", "name"]),
            "PageBeanFilterDetails": page("FilterDetails"),
            "Project": obj(&["id", "key"]),
            "ProjectDetails": obj(&["id", "key", "self"]),
            "PageBeanProjectDetails": page("ProjectDetails"),
        },
        "unresolved": []
    });
    let reports = find(
        &inv,
        Some(&sel(json!({
            "get:/group": { "include": true, "graphql": { "root": "query", "name": "group", "entity": true, "key": "groupId" } },
            "get:/filter/{id}": entity("id"),
            "get:/project/{id}": entity("id"),
        }))),
        "",
        false,
    ).unwrap();
    let group = report(&reports, "Group");
    assert_eq!(group.verdict, Verdict::PartialShape);
    assert_eq!(group.candidates[0].item_shape, "GroupDetails");
    assert_eq!(
        group.candidates[0].problems[0].detail,
        "GroupDetails lacks expand, self, users of Group"
    );
    let filter = report(&reports, "Filter");
    assert_eq!(filter.verdict, Verdict::PartialShape);
    assert!(filter.candidates[0].problems[0]
        .detail
        .contains("lacks sharedUsers"));
    let missing: Vec<&str> = missing_batch(&reports)
        .iter()
        .map(|r| r.type_name.as_str())
        .collect();
    assert_eq!(
        missing,
        ["Project"],
        "a partial shape is not batchable, so --check names only Project"
    );
    let project = report(&reports, "Project");
    assert_eq!(
        project.verdict,
        Verdict::Batchable,
        "a superset shape is the full record"
    );
}

#[test]
fn a_success_or_error_wrapper_is_unwrapped_and_inline_items_match_by_path() {
    // An envelope API: every response is oneOf [Success, ErrorResponse] with no
    // recorded envelope; interviewerPool.list declares its items inline.
    let inv = json!({
        "contract_version": 1, "api": {},
        "operations": [
            { "key": "post:/interviewerPool.info", "method": "POST", "path": "/interviewerPool.info", "parameters": [],
              "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/InfoResponse" } },
            { "key": "post:/interviewerPool.list", "method": "POST", "path": "/interviewerPool.list", "parameters": [],
              "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/ListResponse" } },
            { "key": "post:/user.list", "method": "POST", "path": "/user.list", "parameters": [],
              "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/UserListResponse" } }
        ],
        "shapes": {
            "ErrorResponse": { "type": "object", "properties": { "success": { "type": "boolean" }, "errors": { "type": "array", "items": { "$ref": "#/shapes/ErrorDetail" } } } },
            "ErrorDetail": { "type": "object", "properties": { "id": { "type": "string" } } },
            "InterviewerPool": { "type": "object", "properties": { "id": { "type": "string" }, "title": { "type": "string" } } },
            "InfoSuccess": { "type": "object", "properties": { "success": { "type": "boolean" }, "results": { "$ref": "#/shapes/InterviewerPool" } } },
            "InfoResponse": { "oneOf": [{ "$ref": "#/shapes/InfoSuccess" }, { "$ref": "#/shapes/ErrorResponse" }] },
            "ListSuccess": { "type": "object", "properties": { "success": { "type": "boolean" },
                "results": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" } } } } } },
            "ListResponse": { "oneOf": [{ "$ref": "#/shapes/ListSuccess" }, { "$ref": "#/shapes/ErrorResponse" }] },
            "UserListSuccess": { "type": "object", "properties": {
                "results": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" } } } } } },
            "UserListResponse": { "oneOf": [{ "$ref": "#/shapes/UserListSuccess" }, { "$ref": "#/shapes/ErrorResponse" }] }
        },
        "unresolved": []
    });
    // Its lists are POSTs the inventory classifies as writes; the
    // selection's `root: query` is the confirmation that they read.
    let read = json!({ "include": true, "graphql": { "root": "query", "name": "list" } });
    let reports = find(
        &inv,
        Some(&sel(json!({
            "post:/interviewerPool.info": entity("id"),
            "post:/interviewerPool.list": read.clone(),
            "post:/user.list": read,
        }))),
        "",
        false,
    )
    .unwrap();
    let pool = report(&reports, "InterviewerPool");
    assert_eq!(pool.verdict, Verdict::ListNoKeyFilter);
    assert_eq!(
        pool.candidates.len(),
        1,
        "user.list is another entity: {:?}",
        pool.candidates
    );
    assert_eq!(pool.candidates[0].operation, "post:/interviewerPool.list");
    assert_eq!(pool.candidates[0].item_shape, INLINE);
    assert_eq!(pool.candidates[0].via, "inferred:results");
}

const BATCH_SDL: &str = r#"
type Query {
  widget(id: ID!): Acme_Widget
}

type Acme_Widget
  @key(fields: "id")
  @connect(
    source: "api"
    http: { GET: "/widgets", queryParams: "ids: $batch.id" }
    selection: "id name"
  )
{
  id: ID!
  name: String
}
"#;

#[test]
fn check_fails_only_for_a_keyed_batchable_type_without_a_batch_connector() {
    let inv = widgets(list_op(
        json!([{ "name": "ids", "in": "query", "required": true, "type": "array" }]),
    ));
    let selection = sel(json!({ "get:/widgets/{id}": entity("id") }));
    let without = find(
        &inv,
        Some(&selection),
        "type Query { widget(id: ID!): Acme_Widget }",
        false,
    )
    .unwrap();
    assert_eq!(missing_batch(&without).len(), 1);
    let with = find(&inv, Some(&selection), BATCH_SDL, false).unwrap();
    assert!(with[0].has_batch_connector);
    assert!(missing_batch(&with).is_empty());
    // A $batch only inside a Query field's @connect is not a type-level one.
    let root_only = "type Query {\n  widgets(ids: [ID!]!): [Acme_Widget] @connect(source: \"api\", http: { GET: \"/w\", queryParams: \"ids: $batch.id\" }, selection: \"id\")\n}\n";
    assert_eq!(
        missing_batch(&find(&inv, Some(&selection), root_only, false).unwrap()).len(),
        1
    );
}

#[test]
fn all_types_adds_unkeyed_response_types_once_per_entity() {
    let inv = widgets(list_op(
        json!([{ "name": "ids", "in": "query", "required": true, "type": "array" }]),
    ));
    let reports = find(&inv, None, "", true).unwrap();
    assert_eq!(reports.len(), 1, "{:?}", reports);
    assert_eq!(reports[0].type_name, "Widget");
    assert_eq!(reports[0].keyed_by, None);
    assert_eq!(reports[0].verdict, Verdict::Batchable);
    assert!(
        missing_batch(&reports).is_empty(),
        "--check is about keyed types only"
    );
}

/// A copy of a fixture directory in a temp dir, `.factory/inventory.json`
/// built from `spec` when given.
fn workspace(fixture: &str, spec: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let from = manifest().join("tests/fixtures").join(fixture);
    for rel in [".factory/inventory.json", ".factory/selection.yaml"] {
        if from.join(rel).exists() {
            let to = dir.path().join(rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(from.join(rel), to).unwrap();
        }
    }
    if let Some(spec) = spec {
        std::fs::create_dir_all(dir.path().join(".factory")).unwrap();
        let code = graphos_factory_core::cmd::inventory::main(&[
            "build".to_string(),
            from.join(spec).to_string_lossy().to_string(),
            "--out".to_string(),
            dir.path()
                .join(".factory/inventory.json")
                .to_string_lossy()
                .to_string(),
        ]);
        assert_eq!(code, 0, "inventory build");
    }
    dir
}

fn load(dir: &Path) -> (Value, Option<Value>) {
    let inv = serde_json::from_str(
        &std::fs::read_to_string(dir.join(".factory/inventory.json")).unwrap(),
    )
    .unwrap();
    let sel = std::fs::read_to_string(dir.join(".factory/selection.yaml"))
        .ok()
        .map(|t| graphos_factory_core::yaml::parse(&t).unwrap());
    (inv, sel)
}

fn batch(dir: &Path, flags: &[&str]) -> i32 {
    let mut argv = vec!["find".to_string(), dir.to_string_lossy().to_string()];
    argv.extend(flags.iter().map(|f| f.to_string()));
    graphos_factory_core::cmd::batch::main(&argv)
}

#[test]
fn stay_amenities_are_batchable_by_comma_separated_ids_and_listings_are_not() {
    let ws = workspace("stay-listings", None);
    let (inv, selection) = load(ws.path());

    let keyed = find(&inv, selection.as_ref(), "", false).unwrap();
    assert_eq!(keyed.len(), 1);
    let listing = report(&keyed, "Listing");
    assert_eq!(
        listing.keyed_by.as_deref(),
        Some("get:/listings/{listingId}")
    );
    assert_eq!(listing.verdict, Verdict::ListNoKeyFilter);
    let ops: Vec<&str> = listing
        .candidates
        .iter()
        .map(|c| c.operation.as_str())
        .collect();
    assert_eq!(
        ops,
        [
            "get:/listings",
            "get:/featured-listings",
            "get:/user/{userId}/listings"
        ]
    );
    // /amenities/listings takes listing ids but answers with amenity arrays
    // that carry no listing id: never a candidate for Listing.
    assert!(!ops.contains(&"get:/amenities/listings"));

    let all = find(&inv, selection.as_ref(), "", true).unwrap();
    let amenity = report(&all, "Amenity");
    assert_eq!(amenity.verdict, Verdict::Batchable);
    let c = &amenity.candidates[0];
    assert_eq!(c.operation, "get:/amenities");
    let k = c.key_param.as_ref().unwrap();
    assert_eq!(
        (k.name.as_str(), k.location.as_str(), k.passing.as_str()),
        ("ids", "query", "comma-separated")
    );
    assert!(amenity
        .candidates
        .iter()
        .all(|c| c.operation != "get:/amenities/listings"));
    assert_eq!(
        all.iter()
            .filter(|r| r.verdict == Verdict::Batchable)
            .count(),
        1
    );

    assert_eq!(batch(ws.path(), &["--check"]), 0, "Amenity is not keyed");
}

#[test]
fn petstore_users_are_batchable_by_find_by_names_and_pets_are_not() {
    let ws = workspace("petstore-batch", Some("petstore-batch.yaml"));
    let (inv, selection) = load(ws.path());
    let reports = find(&inv, selection.as_ref(), "", false).unwrap();
    assert_eq!(reports.len(), 2);

    let user = report(&reports, "User");
    assert_eq!(user.key, "username");
    assert_eq!(user.verdict, Verdict::Batchable);
    let c = &user.candidates[0];
    assert_eq!(c.operation, "get:/user/findByNames");
    let k = c.key_param.as_ref().unwrap();
    assert_eq!(
        (k.name.as_str(), k.location.as_str(), k.passing.as_str()),
        ("username", "query", "repeated")
    );

    let pet = report(&reports, "Pet");
    assert_eq!(
        pet.verdict,
        Verdict::ListNoKeyFilter,
        "findByStatus/findByTags filter, they do not key"
    );

    assert_eq!(
        batch(ws.path(), &["--check"]),
        1,
        "User is batchable and has no $batch connector"
    );
    assert_eq!(batch(ws.path(), &["--json"]), 0);

    std::fs::write(
        ws.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: petstore\ndirectory: petstore\n",
    )
    .unwrap();
    std::fs::write(
        ws.path().join("petstore.graphql"),
        "type Petstore_User\n  @key(fields: \"username\")\n  @connect(source: \"api\", http: { GET: \"/user/findByNames\", queryParams: \"username: $batch.username\" }, selection: \"username\")\n{\n  username: String!\n}\n",
    )
    .unwrap();
    assert_eq!(
        batch(ws.path(), &["--check"]),
        0,
        "the $batch connector is there"
    );
}

#[test]
fn gitea_has_nothing_to_batch() {
    for pilot in ["gitea"] {
        let dir = manifest().join("../../pilots/graphos").join(pilot);
        let (inv, selection) = load(&dir);
        let all = find(&inv, selection.as_ref(), "", true).unwrap();
        assert!(!all.is_empty(), "{} has response types", pilot);
        assert_eq!(
            all.iter()
                .filter(|r| r.verdict == Verdict::Batchable)
                .count(),
            0,
            "{}: {:?}",
            pilot,
            all
        );
        assert_eq!(batch(&dir, &["--check"]), 0, "{}", pilot);
    }
}

#[test]
fn an_unreadable_workspace_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(batch(dir.path(), &[]), 2);
    assert_eq!(
        graphos_factory_core::cmd::batch::main(&["show".to_string()]),
        2
    );
}

fn widget_list_op(parameters: Value, path: &str) -> Value {
    json!({
        "key": format!("get:{}", path), "method": "GET", "path": path,
        "parameters": parameters,
        "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetList", "root_is_array": true }
    })
}

#[test]
fn a_post_the_source_classifies_as_a_write_is_never_a_candidate() {
    // A bulk delete answers with the deleted widgets and takes their ids:
    // it has every mark of a lookup except that it deletes.
    let r = widget_report(json!({
        "key": "post:/widgets/delete", "method": "POST", "path": "/widgets/delete", "semantics": "write",
        "parameters": [],
        "request_body": { "content_type": "application/json", "required": true, "shape_ref": "#/shapes/WidgetIds" },
        "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetPage", "array_root_properties": ["items"] }
    }));
    assert_eq!(r.verdict, Verdict::None);
    assert!(r.candidates.is_empty());
}

#[test]
fn a_lookup_that_needs_a_path_segment_or_a_required_filter_needs_scope() {
    // CM360: every list sits under /userprofiles/{profileId}; Confluence's
    // /custom-content requires `type`; CM360's floodlight groups say in prose
    // "Must specify either advertiserId or floodlightConfigurationId".
    let ids = json!({ "name": "ids", "in": "query", "required": false, "type": "array", "explode": true });
    let path_scoped = widget_report(widget_list_op(
        json!([ids.clone(), { "name": "profileId", "in": "path", "required": true, "type": "string" }]),
        "/userprofiles/{profileId}/widgets",
    ));
    assert_eq!(path_scoped.verdict, Verdict::NeedsScope);
    assert!(
        path_scoped.candidates[0].problems[0]
            .detail
            .contains("profileId (path)"),
        "{:?}",
        path_scoped.candidates
    );
    let filtered = widget_report(widget_list_op(
        json!([ids.clone(), { "name": "type", "in": "query", "required": true, "type": "string" }]),
        "/widgets",
    ));
    assert_eq!(filtered.verdict, Verdict::NeedsScope);
    assert!(
        filtered.candidates[0]
            .key_param
            .as_ref()
            .unwrap()
            .companions
            == ["type (query)"]
    );
    let prose = widget_report(widget_list_op(
        json!([ids.clone(), { "name": "advertiserId", "in": "query", "required": false, "type": "string",
                "description": "Must specify either advertiserId or floodlightConfigurationId for a non-empty result." }]),
        "/widgets",
    ));
    assert_eq!(prose.verdict, Verdict::NeedsScope);
    assert!(prose.candidates[0].problems[0]
        .detail
        .contains("advertiserId (query, documented as required)"));
    let optional = widget_report(widget_list_op(
        json!([ids, { "name": "limit", "in": "query", "required": false, "type": "integer", "description": "Not required." }]),
        "/widgets",
    ));
    assert_eq!(
        optional.verdict,
        Verdict::Batchable,
        "an optional companion is fine"
    );
}

#[test]
fn an_exploded_array_documented_as_comma_separated_is_a_style_conflict() {
    // Confluence /pages: `id` is an array with no style or explode (form,
    // exploded: repeated) and its description says comma-separated.
    let conflict = widget_report(list_op(json!([
        { "name": "id", "in": "query", "required": false, "type": "array",
          "description": "Multiple ids can be specified as a comma-separated list." }
    ])));
    assert_eq!(conflict.verdict, Verdict::StyleConflict);
    assert!(conflict.candidates[0].problems[0]
        .detail
        .contains("says comma-separated"));
    // Explode off agrees with the prose: no conflict.
    let agreed = widget_report(list_op(json!([
        { "name": "id", "in": "query", "required": false, "type": "array", "style": "form", "explode": false,
          "description": "Multiple ids can be specified as a comma-separated list." }
    ])));
    assert_eq!(agreed.verdict, Verdict::Batchable);
    assert_eq!(
        agreed.candidates[0].key_param.as_ref().unwrap().passing,
        "comma-separated"
    );
}

#[test]
fn known_miss_a_wrapper_with_two_array_properties() {
    // KNOWN MISS (ADR 0068): Jira's POST /issue/bulkfetch answers
    // {issues: [...], issueErrors: [...]}. With two array properties and no
    // selection envelope the detector cannot tell which one holds the
    // records, so the bulk lookup is not found. Pinned, not fixed.
    let inv = json!({
        "contract_version": 1, "api": {},
        "operations": [
            { "key": "get:/issue/{id}", "method": "GET", "path": "/issue/{id}", "parameters": [],
              "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Issue" } },
            { "key": "post:/issue/bulkfetch", "method": "POST", "path": "/issue/bulkfetch", "semantics": "read", "parameters": [],
              "request_body": { "content_type": "application/json", "required": true, "shape_ref": "#/shapes/BulkFetch" },
              "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/BulkIssues",
                            "array_root_properties": ["issueErrors", "issues"] } }
        ],
        "shapes": {
            "Issue": { "type": "object", "properties": { "id": { "type": "string" }, "key": { "type": "string" } } },
            "IssueError": { "type": "object", "properties": { "id": { "type": "string" } } },
            "BulkFetch": { "type": "object", "properties": { "issueIdsOrKeys": { "type": "array", "items": { "type": "string" } } } },
            "BulkIssues": { "type": "object", "properties": {
                "issues": { "type": "array", "items": { "$ref": "#/shapes/Issue" } },
                "issueErrors": { "type": "array", "items": { "$ref": "#/shapes/IssueError" } } } }
        },
        "unresolved": []
    });
    let reports = find(
        &inv,
        Some(&sel(json!({ "get:/issue/{id}": entity("id") }))),
        "",
        false,
    )
    .unwrap();
    assert_eq!(
        report(&reports, "Issue").verdict,
        Verdict::None,
        "known miss: wrapper with two arrays"
    );
}

#[test]
fn known_miss_a_type_keyed_by_account_id_under_all_types() {
    // KNOWN MISS (ADR 0068): Jira's GET /user/bulk?accountId= answers
    // users keyed by accountId. `--all-types` recognises id, uuid and
    // <type>Id as keys, so User (keyed by accountId) is never reported.
    // Pinned, not fixed.
    let inv = json!({
        "contract_version": 1, "api": {},
        "operations": [
            { "key": "get:/user/bulk", "method": "GET", "path": "/user/bulk",
              "parameters": [{ "name": "accountId", "in": "query", "required": true, "type": "array", "explode": true }],
              "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/PageBeanUser", "sole_root_property": "values" } }
        ],
        "shapes": {
            "User": { "type": "object", "properties": { "accountId": { "type": "string" }, "displayName": { "type": "string" } } },
            "PageBeanUser": { "type": "object", "properties": { "values": { "type": "array", "items": { "$ref": "#/shapes/User" } } } }
        },
        "unresolved": []
    });
    let reports = find(&inv, None, "", true).unwrap();
    assert!(
        reports.iter().all(|r| r.type_name != "User"),
        "known miss: {:?}",
        reports
    );
}

#[test]
fn a_paginated_key_list_lookup_is_paginated_not_batchable() {
    // Review of #109: a key list on a paged list (`GET /pets?ids=` with
    // page/limit) answers at most one page, so a batch bigger than the page
    // silently drops entities. The operation's own `pagination` fact says so.
    let mut op = list_op(json!([
        { "name": "ids", "in": "query", "required": false, "type": "array", "explode": true },
        { "name": "limit", "in": "query", "required": false, "type": "integer" }
    ]));
    op["pagination"] =
        json!({ "style": "page", "request": "page", "size_param": "limit", "response": null });
    let r = widget_report(op.clone());
    assert_eq!(r.verdict, Verdict::Paginated);
    assert_eq!(r.verdict.label(), "paginated");
    let p = &r.candidates[0].problems;
    assert_eq!(p.len(), 1, "{:?}", p);
    assert_eq!(p[0].kind, "paginated");
    assert!(
        p[0].detail.contains("style page, size param limit"),
        "{}",
        p[0].detail
    );
    assert!(
        missing_batch(&[r]).is_empty(),
        "--check does not ask for it"
    );
    // A recorded `none` is no pagination.
    op["pagination"] =
        json!({ "style": "none", "request": null, "size_param": null, "response": null });
    assert_eq!(widget_report(op).verdict, Verdict::Batchable);
}

#[test]
fn a_line_item_or_a_child_is_not_its_parents_batch_lookup() {
    // Review of #109: `stem` stripped `Item`, so `GET /order-items?ids=`
    // answering [OrderItem] became Order's batch lookup. A child that names
    // its parent (`OrderDetail.orderId`) is not the parent either. A
    // representation of the same entity (`OrderSummary`) still is.
    let list = |key: &str, path: &str, shape: &str| {
        json!({ "key": key, "method": "GET", "path": path,
                "parameters": [{ "name": "ids", "in": "query", "required": false, "type": "array", "explode": true }],
                "response": { "status": "200", "content_type": "application/json", "shape_ref": shape, "root_is_array": true } })
    };
    let arr =
        |item: &str| json!({ "type": "array", "items": { "$ref": format!("#/shapes/{}", item) } });
    let inv = |extra: Value, shape_name: &str, shape: Value| {
        let mut shapes = json!({
            "Order": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } },
        });
        shapes[shape_name] = shape;
        shapes[format!("{}List", shape_name)] = arr(shape_name);
        json!({
            "contract_version": 1, "api": {},
            "operations": [
                { "key": "get:/orders/{id}", "method": "GET", "path": "/orders/{id}",
                  "parameters": [{ "name": "id", "in": "path", "required": true, "type": "string" }],
                  "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Order" } },
                extra
            ],
            "shapes": shapes, "unresolved": []
        })
    };
    let selection = sel(json!({ "get:/orders/{id}": entity("id") }));
    let order = |i: &Value| {
        let reports = find(i, Some(&selection), "", false).unwrap();
        report(&reports, "Order").clone()
    };
    let item = inv(
        list("get:/order-items", "/order-items", "#/shapes/OrderItemList"),
        "OrderItem",
        json!({ "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" }, "quantity": { "type": "integer" } } }),
    );
    let r = order(&item);
    assert_eq!(r.verdict, Verdict::None, "{:?}", r.candidates);
    let detail = inv(
        list(
            "get:/order-details",
            "/order-details",
            "#/shapes/OrderDetailList",
        ),
        "OrderDetail",
        json!({ "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" }, "orderId": { "type": "string" } } }),
    );
    let r = order(&detail);
    assert_eq!(r.verdict, Verdict::None, "{:?}", r.candidates);
    let summary = inv(
        list("get:/orders", "/orders", "#/shapes/OrderSummaryList"),
        "OrderSummary",
        json!({ "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } }),
    );
    let r = order(&summary);
    assert_eq!(r.verdict, Verdict::Batchable, "{:?}", r.candidates);
    assert_eq!(r.candidates[0].item_shape, "OrderSummary");
}

#[test]
fn max_size_is_read_from_the_key_parameter_only_and_never_a_page_size() {
    // Review of #109: "up to 1,000" read as 1; the operation's "Returns up
    // to 100 results per page" read as the batch size.
    let max = |param_description: &str, op_description: &str| {
        let mut op = list_op(json!([
            { "name": "ids", "in": "query", "required": false, "type": "string", "description": param_description }
        ]));
        op["description"] = json!(op_description);
        let r = widget_report(op);
        assert_eq!(r.verdict, Verdict::Batchable, "{:?}", r);
        r.candidates[0].key_param.as_ref().unwrap().max_size
    };
    assert_eq!(max("Comma-separated ids, up to 1,000.", ""), Some(1000));
    assert_eq!(
        max("Comma-separated ids, maximum of 250 ids.", ""),
        Some(250)
    );
    assert_eq!(
        max(
            "Comma-separated ids.",
            "Returns up to 100 results per page."
        ),
        None,
        "the operation's prose is about the response"
    );
    assert_eq!(
        max(
            "Comma-separated ids.",
            "Rate limited to 10 requests a second."
        ),
        None,
        "the operation's prose is never the key list's"
    );
    assert_eq!(
        max(
            "Comma-separated ids; returns up to 100 results per page.",
            ""
        ),
        None,
        "a page size in the parameter's prose is not a list size"
    );
    assert_eq!(
        max(
            "Comma-separated ids, at most 50. Up to 20 results per page.",
            ""
        ),
        Some(50)
    );
}

#[test]
fn the_schema_is_parsed_for_type_level_batch_connectors() {
    // Review of #109: the text scanner missed `extend type` and an indented
    // `type`, and counted `$batch`/`@connect` inside a description or a
    // comment, so `--check` went green with no connector at all.
    let inv = widgets(list_op(
        json!([{ "name": "ids", "in": "query", "required": true, "type": "array" }]),
    ));
    let selection = sel(json!({ "get:/widgets/{id}": entity("id") }));
    let has = |sdl: &str| find(&inv, Some(&selection), sdl, false).unwrap()[0].has_batch_connector;
    let connect = r#"@connect(source: "api", http: { GET: "/widgets", queryParams: "ids: $batch.id" }, selection: "id name")"#;
    assert!(has(&format!(
        "type Acme_Widget {{ id: ID! }}\nextend type Acme_Widget {}\n",
        connect
    )));
    assert!(has(&format!(
        "schema {{ query: Query }}\n  type Acme_Widget @key(fields: \"id\") {} {{ id: ID! }}\n",
        connect
    )));
    assert!(has(&format!(
        "type Acme_Widget @connect__connect(source: \"api\", http: {{ GET: \"/w\" }}, selection: \"$batch.id\") {{ id: ID! }}"
    )));
    assert!(!has(
        "\"\"\"\nOne day: @connect(http: { GET: \"/w?ids={$batch.id}\" })\n\"\"\"\ntype Acme_Widget @key(fields: \"id\") { id: ID! }\n"
    ));
    assert!(!has(&format!(
        "# type Acme_Widget {}\ntype Acme_Widget @key(fields: \"id\") {{ id: ID! }}\n",
        connect
    )));
    assert!(!has(
        "type Acme_Widget @connect(source: \"api\", http: { GET: \"/widgets/{$this.id}\" }, selection: \"id\") { id: ID! }"
    ));
    let err = find(&inv, Some(&selection), "type Acme_Widget @connect(", false).unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn a_multibyte_character_after_a_backslash_in_a_directive_string_does_not_panic() {
    // Review of #109: the scanner stepped two bytes past a backslash inside
    // a string and sliced mid-codepoint on `é`.
    let inv = widgets(list_op(
        json!([{ "name": "ids", "in": "query", "required": true, "type": "array" }]),
    ));
    let selection = sel(json!({ "get:/widgets/{id}": entity("id") }));
    let sdl = "type Acme_Widget\n  @connect(source: \"api\", http: { GET: \"/w\", queryParams: \"ids: $batch.id\" }, selection: \"\"\"\n  caf\\é\n  id\n  \"\"\")\n{\n  id: ID!\n}\n";
    let reports = find(&inv, Some(&selection), sdl, false).unwrap();
    assert!(reports[0].has_batch_connector);
}

#[test]
fn a_schema_that_exists_but_cannot_be_read_or_parsed_exits_2() {
    // Review of #109: `unwrap_or_default()` made an unreadable schema read
    // as "no connector" and `--check` exit 1. A missing file is still "no
    // connector yet".
    let ws = workspace("petstore-batch", Some("petstore-batch.yaml"));
    std::fs::write(
        ws.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: petstore\ndirectory: petstore\n",
    )
    .unwrap();
    assert_eq!(
        batch(ws.path(), &["--check"]),
        1,
        "missing schema: no connector"
    );
    std::fs::write(
        ws.path().join("petstore.graphql"),
        "type Petstore_User @connect(",
    )
    .unwrap();
    assert_eq!(batch(ws.path(), &["--check"]), 2, "unparsable schema");
    std::fs::write(ws.path().join("petstore.graphql"), [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(batch(ws.path(), &["--check"]), 2, "not UTF-8");
    std::fs::remove_file(ws.path().join("petstore.graphql")).unwrap();
    std::fs::create_dir(ws.path().join("petstore.graphql")).unwrap();
    assert_eq!(batch(ws.path(), &[]), 2, "a directory, not a file");
}

#[test]
fn a_polymorphic_response_shape_is_expanded_once() {
    // Review of #117: `inventory build` records `WidgetOut: oneOf [CatWidget]`,
    // `CatWidget: allOf [WidgetOut, {...}]` as a cycle, and `branches`
    // recursed through it until the stack overflowed (`lint` reaches it
    // through `entity::check`).
    let mut inv = widgets(json!({
        "key": "get:/widgets", "method": "GET", "path": "/widgets",
        "parameters": [{ "name": "ids", "in": "query", "required": true, "type": "array", "items": { "type": "string" } }],
        "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/WidgetOutList", "root_is_array": true }
    }));
    let shapes = inv["shapes"].as_object_mut().unwrap();
    shapes.insert(
        "WidgetOutList".into(),
        json!({ "type": "array", "items": { "$ref": "#/shapes/WidgetOut" } }),
    );
    shapes.insert(
        "WidgetOut".into(),
        json!({ "oneOf": [{ "$ref": "#/shapes/CatWidget" }] }),
    );
    shapes.insert(
        "CatWidget".into(),
        json!({ "allOf": [
            { "$ref": "#/shapes/WidgetOut" },
            { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } }
        ] }),
    );
    let selection = sel(json!({ "get:/widgets/{id}": entity("id") }));
    for all_types in [false, true] {
        let reports = find(&inv, Some(&selection), "", all_types).unwrap();
        report(&reports, "Widget");
    }
}

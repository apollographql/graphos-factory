//! `graphos-factory-core scaffold`: a case, a stub and a unit entry per selected
//! operation, built from the schema's connector and the inventory shapes.
use serde_json::{json, Value};
use std::path::Path;

const WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\n";

const SELECTION: &str = concat!(
    "contract_version: 1\n",
    "operations:\n",
    "  \"get:/widgets\":\n    include: true\n    graphql: { root: query, name: listWidgets }\n",
    "  \"post:/widgets\":\n    include: true\n    graphql: { root: mutation, name: createWidget }\n",
    "  \"get:/version\":\n    include: true\n    graphql: { root: query, name: version }\n",
    "  \"delete:/widgets/{id}\": { include: false, reason: \"scope\" }\n",
);

const TEMPLATE: &str = "variables:\n  - name: BASE_URL\n    test_default: \"https://api.widgets.test/v2\"\n  - name: AUTH_EXPR\n    test_default: \"{$env.WIDGET_TOKEN}\"\n";

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(
  name: "widget_co"
  http: {
    baseURL: "{{BASE_URL}}"
    headers: [
      { name: "Authorization", value: "Bearer {{AUTH_EXPR}}" }
    ]
  }
)

enum Widget_Co_Color {
  RED
  BLUE
}

type Widget_Co_Widget {
  id: ID!
  name: String
  color: Widget_Co_Color
  weight: Float
  owner: Widget_Co_Owner
}

type Widget_Co_Owner {
  login: String
  isAdmin: Boolean
}

type Query {
  "Every widget."
  widget_co_listWidgets(
    limit: Int
    "Widget ids."
    ids: [ID!]
    since: String
    color: Widget_Co_Color
  ): [Widget_Co_Widget]
    @connect(
      source: "widget_co"
      http: {
        GET: "/widgets"
        queryParams: """
        limit: $args.limit
        ids[]: $args.ids
        since: $args.since
        color: $args.color
        """
      }
      selection: """
      $.widgets {
        id
        name
        color
        weight
        owner {
          login
          isAdmin: is_admin
        }
      }
      """
    )
  widget_co_version: String
    @connect(
      source: "widget_co"
      http: { GET: "/version" }
      selection: "$.version"
    )
}

type Mutation {
  widget_co_createWidget(
    name: String!
    count: Int
    tags: [String!]
    color: Widget_Co_Color
  ): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        POST: "/widgets"
        body: """
        name: $args.name
        count: $args.count
        tags: $args.tags
        color: $args.color
        """
      }
      selection: """
      id
      name
      """
    )
}
"#;

fn inventory() -> Value {
    json!({"contract_version": 1, "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test/v2"]},
        "operations": [
            {"key": "get:/widgets", "operation_id": "listWidgets", "method": "GET", "path": "/widgets", "semantics": "read", "provenance": "spec", "confidence": 1,
             "parameters": [
                {"name": "limit", "in": "query", "required": false, "schema": {"type": "integer"}},
                {"name": "ids", "in": "query", "required": false, "schema": {"type": "array", "items": {"type": "integer"}}},
                {"name": "since", "in": "query", "required": false, "schema": {"type": "string", "format": "date-time"}},
                {"name": "color", "in": "query", "required": false, "schema": {"type": "string", "enum": ["RED", "BLUE"]}}
             ],
             "request_body": null,
             "response": {"status": "200", "envelope": "widgets", "shape_ref": "#/shapes/WidgetList", "list": true}, "errors": [], "support": "supported", "support_reason": null},
            {"key": "get:/version", "operation_id": "version", "method": "GET", "path": "/version", "semantics": "read", "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null,
             "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Version", "list": false}, "errors": [], "support": "supported", "support_reason": null},
            {"key": "post:/widgets", "operation_id": "createWidget", "method": "POST", "path": "/widgets", "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
             "request_body": {"shape_ref": "#/shapes/CreateWidget", "content_type": "application/json"},
             "response": {"status": "201", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false}, "errors": [], "support": "supported", "support_reason": null},
            {"key": "delete:/widgets/{id}", "operation_id": "deleteWidget", "method": "DELETE", "path": "/widgets/{id}", "semantics": "delete", "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null,
             "response": {"status": "204", "envelope": null, "shape_ref": null, "list": false}, "errors": [], "support": "supported", "support_reason": null}
        ],
        "shapes": {
            "Widget": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"},
                "color": {"type": "string", "enum": ["RED", "BLUE"]},
                "weight": {"type": "number"},
                "owner": {"$ref": "#/shapes/Owner"},
                "parent": {"$ref": "#/shapes/Widget"}}},
            "Owner": {"type": "object", "properties": {"login": {"type": "string"}, "is_admin": {"type": "boolean"}}},
            "WidgetList": {"type": "object", "properties": {"widgets": {"type": "array", "items": {"$ref": "#/shapes/Widget"}}}},
            "Version": {"type": "object", "properties": {"version": {"type": "string"}}},
            "CreateWidget": {"type": "object", "required": ["name"], "properties": {
                "name": {"type": "string"}, "count": {"type": "integer"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "color": {"type": "string", "enum": ["RED", "BLUE"]}}}
        },
        "unresolved": []})
}

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w("widget-co.graphql", SDL);
    w("template.yaml", TEMPLATE);
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", SELECTION);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&inventory()),
    );
    dir
}

fn regex_is_match(pattern: &str, text: &str) -> bool {
    regex::Regex::new(pattern).unwrap().is_match(text)
}

fn scaffold(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec![dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::scaffold::main(&argv)
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("{}: {}", rel, e))
}

fn read_json(dir: &Path, rel: &str) -> Value {
    serde_json::from_str(&read(dir, rel)).unwrap_or_else(|e| panic!("{}: {}", rel, e))
}

/// The unit suite as parsed YAML, and the entry targeting `target`.
fn unit_entry(dir: &Path, target: &str) -> Value {
    let suite = graphos_factory_core::yaml::parse(&read(dir, "tests/widget-co.connector.yaml"))
        .expect("the suite is valid YAML");
    suite["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["target"].as_str() == Some(target))
        .cloned()
        .unwrap_or_else(|| panic!("no unit entry targets {}", target))
}

#[test]
fn a_read_gets_a_document_with_every_argument_a_stub_and_a_unit_entry() {
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);

    // The document: every argument passed, the list with two elements, the
    // enum as an enum literal, the connector's whole selection tree.
    let doc = read(d, "tests/cases/list_widgets.graphql");
    assert!(doc.starts_with("# scaffold:"), "{}", doc);
    assert!(doc.contains("query {\n  widget_co_listWidgets("), "{}", doc);
    // ID arguments take the spec's integer type, two of them.
    assert!(regex_is_match(r#"ids: \[\d+, \d+\]"#, &doc), "{}", doc);
    assert!(doc.contains("limit: 1"), "{}", doc);
    assert!(
        regex_is_match(r#"since: "2026-01-0\dT00:00:00Z""#, &doc),
        "{}",
        doc
    );
    assert!(doc.contains("color: BLUE"), "{}", doc);
    for f in [
        "id", "name", "color", "weight", "owner {", "login", "isAdmin",
    ] {
        assert!(doc.contains(f), "selection lacks {}:\n{}", f, doc);
    }

    // The stub: method, path, the credential header, one matcher per query
    // key (hasExactly for the repeated one), a response body of the shape.
    let stub = read_json(d, "tests/fixtures/mappings/list_widgets.json");
    assert_eq!(stub["request"]["method"], "GET");
    assert_eq!(stub["request"]["urlPath"], "/widgets");
    assert_eq!(
        stub["request"]["headers"]["authorization"]["equalTo"],
        "Bearer test-token"
    );
    let qp = &stub["request"]["queryParameters"];
    assert_eq!(qp["limit"]["equalTo"], "1");
    assert!(qp["since"]["equalTo"]
        .as_str()
        .unwrap()
        .starts_with("2026-01-0"));
    assert_eq!(qp["color"]["equalTo"], "BLUE"); // what the router sends: the enum value itself
    let ids = qp["ids[]"]["hasExactly"].as_array().unwrap();
    assert_eq!(ids.len(), 2);
    assert!(ids
        .iter()
        .all(|m| m["equalTo"].as_str().unwrap().parse::<u64>().is_ok()));
    let body = &stub["response"]["jsonBody"];
    assert_eq!(stub["response"]["status"], 200);
    let first = &body["widgets"][0];
    assert!(first["id"].is_string(), "{}", body);
    assert!(first["weight"].is_number(), "{}", body);
    assert!(first["owner"]["is_admin"].is_boolean(), "{}", body);
    assert!(
        first["color"] == "RED" || first["color"] == "BLUE",
        "an enum sample is one of the schema's values: {}",
        body
    );
    // The selection does not descend into `parent`: the cycle is cut there.
    assert_eq!(first["parent"], json!({}), "{}", body);
    assert!(stub["metadata"]["x-scaffold"]
        .as_str()
        .unwrap()
        .contains("audit"));

    // The unit entry: scalar $args only (no list, no colon-bearing query
    // value), the credential header, the URL with the remaining query
    // string, and the mapped response.
    let entry = unit_entry(d, "Query.widget_co_listWidgets");
    let args = &entry["variables"]["$args"];
    assert_eq!(args["limit"], 1);
    assert_eq!(args["color"], "BLUE");
    assert!(args.get("ids").is_none(), "{}", args);
    assert!(args.get("since").is_none(), "{}", args);
    let req = &entry["expect"]["connectorRequest"];
    assert_eq!(req["method"], "GET");
    assert_eq!(
        req["url"],
        "https://api.widgets.test/v2/widgets?limit=1&color=BLUE"
    );
    assert_eq!(req["headers"]["authorization"], "Bearer test-token");
    let mapped: Value =
        serde_json::from_str(entry["expect"]["connectorResponse"].as_str().unwrap()).unwrap();
    assert_eq!(mapped[0]["id"], first["id"]);
    assert_eq!(mapped[0]["owner"]["isAdmin"], first["owner"]["is_admin"]);
    let api: Value = serde_json::from_str(entry["apiResponseBody"].as_str().unwrap()).unwrap();
    assert_eq!(&api, body, "the unit entry's API body is the stub's body");
}

#[test]
fn a_scalar_root_selection_gets_no_selection_set_and_a_scalar_response() {
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let doc = read(d, "tests/cases/version.graphql");
    assert!(doc.contains("query {\n  widget_co_version\n}"), "{}", doc);
    let stub = read_json(d, "tests/fixtures/mappings/version.json");
    assert!(stub["response"]["jsonBody"]["version"].is_string());
    let entry = unit_entry(d, "Query.widget_co_version");
    assert_eq!(
        entry["expect"]["connectorResponse"]
            .as_str()
            .unwrap()
            .trim(),
        format!("{}", stub["response"]["jsonBody"]["version"])
    );
    assert!(entry.get("variables").is_none());
}

#[test]
fn a_write_gets_a_full_case_a_minimal_case_and_a_string_bodied_unit_entry() {
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);

    // The full case: every argument, the stub asserting the typed body exactly.
    let doc = read(d, "tests/cases/create_widget.graphql");
    assert!(
        doc.contains("mutation {\n  widget_co_createWidget("),
        "{}",
        doc
    );
    assert!(
        regex_is_match(r#"tags: \["tags-\d+", "tags-\d+"\]"#, &doc),
        "{}",
        doc
    );
    let stub = read_json(d, "tests/fixtures/mappings/create_widget.json");
    assert_eq!(stub["request"]["method"], "POST");
    let expected = &stub["request"]["bodyPatterns"][0]["equalToJson"];
    assert_eq!(expected["name"], "name-1");
    assert_eq!(
        expected["count"], 2,
        "the stub asserts the integer the router sends"
    );
    assert_eq!(expected["tags"].as_array().unwrap().len(), 2);
    assert_eq!(expected["color"], "BLUE");
    assert_eq!(stub["response"]["status"], 201);

    // The minimal case: required arguments only, the body exactly those keys.
    let min = read(d, "tests/cases/create_widget_minimal.graphql");
    assert!(min.contains("name: \"name-1\""), "{}", min);
    assert!(!min.contains("count:") && !min.contains("tags:"), "{}", min);
    let mstub = read_json(d, "tests/fixtures/mappings/create_widget_minimal.json");
    assert_eq!(
        mstub["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"name": "name-1"})
    );
    // Both stubs are case-scoped by name, so e2e loads each for its own case.
    assert!(mstub["metadata"]["x-scaffold"].is_string());

    // The unit entry: the list and the optional integer are left out, the
    // body rover builds is asserted as the strings it sends.
    let entry = unit_entry(d, "Mutation.widget_co_createWidget");
    let args = &entry["variables"]["$args"];
    assert_eq!(args["name"], "name-1");
    assert_eq!(args["color"], "BLUE");
    assert!(args.get("count").is_none(), "{}", args);
    assert!(args.get("tags").is_none(), "{}", args);
    let req = &entry["expect"]["connectorRequest"];
    assert_eq!(req["method"], "POST");
    assert_eq!(req["url"], "https://api.widgets.test/v2/widgets");
    let body: Value = serde_json::from_str(req["body"].as_str().unwrap()).unwrap();
    assert_eq!(body, json!({"name": "name-1", "color": "BLUE"}));
    let mapped: Value =
        serde_json::from_str(entry["expect"]["connectorResponse"].as_str().unwrap()).unwrap();
    assert_eq!(mapped["name"], stub["response"]["jsonBody"]["name"]);
    assert!(
        mapped.get("color").is_none(),
        "only the selected fields map back"
    );

    // The minimal case has no unit entry of its own: rover's stringified body
    // would duplicate the full entry's proof.
    let suite = read(d, "tests/widget-co.connector.yaml");
    assert_eq!(suite.matches("Mutation.widget_co_createWidget").count(), 1);
}

#[test]
fn existing_cases_are_skipped_force_regenerates_one_and_dry_run_writes_nothing() {
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--dry-run"]), 0);
    assert!(!d.join("tests").exists(), "dry run wrote files");

    assert_eq!(scaffold(d, &[]), 0);
    let before = read(d, "tests/widget-co.connector.yaml");
    // Nothing left to do: exit 2, the suite untouched.
    assert_eq!(scaffold(d, &[]), 2);
    assert_eq!(read(d, "tests/widget-co.connector.yaml"), before);

    // A hand-audited document survives a second run (the case exists) …
    std::fs::write(
        d.join("tests/cases/version.graphql"),
        "query { widget_co_version }\n",
    )
    .unwrap();
    assert_eq!(scaffold(d, &["--op", "get:/widgets"]), 2);
    assert_eq!(
        read(d, "tests/cases/version.graphql"),
        "query { widget_co_version }\n"
    );
    // … until --force names it.
    assert_eq!(scaffold(d, &["--op", "get:/version", "--force"]), 0);
    assert!(read(d, "tests/cases/version.graphql").starts_with("# scaffold:"));
    // --force on one operation replaces its stale scaffold entry: one entry
    // per target, the rest of the suite untouched.
    let after = read(d, "tests/widget-co.connector.yaml");
    assert_eq!(after.matches("Query.widget_co_version").count(), 1);
    assert_eq!(after.matches("Query.widget_co_listWidgets").count(), 1);
    assert_eq!(after.matches("Mutation.widget_co_createWidget").count(), 1);
    graphos_factory_core::yaml::parse(&after).expect("the suite is still valid YAML");
}

#[test]
fn json_output_lists_every_plan_with_its_files_and_notes() {
    let dir = workspace();
    let d = dir.path();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "scaffold",
            d.to_str().unwrap(),
            "--dry-run",
            "--json",
            "--op",
            "post:/widgets",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["suite"], "tests/widget-co.connector.yaml");
    let written = report["written"].as_array().unwrap();
    let cases: Vec<&str> = written
        .iter()
        .map(|w| w["case"].as_str().unwrap())
        .collect();
    assert_eq!(cases, vec!["create_widget", "create_widget_minimal"]);
    assert_eq!(written[0]["document"], "tests/cases/create_widget.graphql");
    assert_eq!(
        written[0]["mapping"],
        "tests/fixtures/mappings/create_widget.json"
    );
    let notes = written[0]["notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("list arguments")),
        "{:?}",
        notes
    );
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap().contains("integer/number/boolean body")),
        "{:?}",
        notes
    );
    assert!(report["skipped"].as_array().unwrap().is_empty());
}

#[test]
fn a_selection_in_the_mapping_language_gets_no_connector_response() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "      $.widgets {\n        id\n        name\n",
        "      $.widgets {\n        id: id->slice(0, 3)\n        name\n",
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    assert_eq!(scaffold(d, &["--op", "get:/widgets"]), 0);
    let entry = unit_entry(d, "Query.widget_co_listWidgets");
    assert!(
        entry["expect"].get("connectorResponse").is_none(),
        "{}",
        entry
    );
    assert!(!entry["name"].as_str().unwrap().contains("mapping"));
}

#[test]
fn the_gitea_pilot_stripped_of_its_tests_scaffolds_every_operation_and_conforms() {
    let pilot = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    if !pilot.join(".factory/inventory.json").exists() {
        eprintln!("skipping: pilots/graphos/gitea is not checked out next to the crate");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let name = e.file_name();
            let n = name.to_string_lossy();
            if n == "tests" || n == "evidence" {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to.join(&name));
            } else {
                std::fs::copy(e.path(), to.join(&name)).unwrap();
            }
        }
    }
    copy(&pilot, d);
    assert_eq!(scaffold(d, &[]), 0);
    let cases: Vec<String> = std::fs::read_dir(d.join("tests/cases"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(cases.len(), 9, "{:?}", cases); // 8 operations + create_issue_minimal
    for c in &cases {
        let stem = c.trim_end_matches(".graphql");
        read_json(d, &format!("tests/fixtures/mappings/{}.json", stem));
    }
    let suite = graphos_factory_core::yaml::parse(&read(d, "tests/gitea.connector.yaml")).unwrap();
    assert_eq!(suite["tests"].as_array().unwrap().len(), 8);
    // Every scaffolded body conforms to the swagger oracle, request bodies included.
    let report = graphos_factory_core::cmd::validate::validate_workspace(d).unwrap();
    assert!(
        !report.failing,
        "{}",
        graphos_factory_core::json::pretty(&report.json)
    );
    let results = report.json["results"].as_array().unwrap();
    assert!(
        results.iter().all(|r| r["status"] == "pass"),
        "{}",
        graphos_factory_core::json::pretty(&report.json)
    );
}

// ── The cases the first review found: what the scaffold must refuse to
// assert, and the suite it must not corrupt. ────────────────────────────

const HARD_SELECTION: &str = concat!(
    "contract_version: 1\n",
    "operations:\n",
    "  \"post:/things/{id}\":\n    include: true\n    graphql: { root: mutation, name: touchThing }\n",
    "  \"post:/nested\":\n    include: true\n    graphql: { root: mutation, name: nested }\n",
    "  \"post:/flat\":\n    include: true\n    graphql: { root: mutation, name: flat }\n",
    "  \"get:/oneline\":\n    include: true\n    graphql: { root: query, name: oneline }\n",
    "  \"delete:/widgets/{id}\":\n    include: true\n    graphql: { root: mutation, name: deleteWidget }\n",
    "  \"get:/cycle\":\n    include: true\n    graphql: { root: query, name: cycle }\n",
    "  \"get:/ghost\":\n    include: true\n    graphql: { root: query, name: ghost }\n",
    "  \"get:/dated/{when}\":\n    include: true\n    graphql: { root: query, name: dated }\n",
    "  \"get:/literal\":\n    include: true\n    graphql: { root: query, name: literal }\n",
    "  \"get:/order\":\n    include: true\n    graphql: { root: query, name: order }\n",
    "  \"get:/joined\":\n    include: true\n    graphql: { root: query, name: joined }\n",
);

const HARD_SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(
  name: "widget_co"
  http: {
    baseURL: "{{BASE_URL}}"
    headers: [
      { name: "Authorization", value: "Bearer {{AUTH_EXPR}}" }
    ]
  }
)

type Widget_Co_Widget {
  id: ID!
  name: String
  parent: Widget_Co_Widget
}

type Query {
  widget_co_cycle: Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: { GET: "/cycle" }
      selection: """
      id
      name
      parent {
        id
        name
        parent {
          id
          name
        }
      }
      """
    )
  widget_co_ghost: Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: { GET: "/ghost" }
      selection: """
      id
      name: ghost_name
      """
    )
  widget_co_dated(when: String!): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: { GET: "/dated/{$args.when}" }
      selection: "id"
    )
  widget_co_literal(x: String): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        GET: "/literal?fixed=1&mode=a%20b"
        queryParams: """
        x: $args.x
        """
      }
      selection: "id"
    )
  widget_co_order(alpha: String, beta: String): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        GET: "/order"
        queryParams: """
        beta: $args.beta
        alpha: $args.alpha
        """
      }
      selection: "id"
    )
  widget_co_oneline(alpha: String, beta: String): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: { GET: "/oneline", queryParams: "alpha: $args.alpha beta: $args.beta" }
      selection: "id"
    )
  widget_co_joined(tags: [String!]): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        GET: "/joined"
        queryParams: """
        tags: $args.tags
        """
      }
      selection: "id"
    )
}

type Mutation {
  widget_co_touchThing(id: ID!, note: String): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        POST: "/things/{$args.id}"
        queryParams: """
        note: $args.note
        """
      }
      selection: "id"
    )
  widget_co_nested(name: String!, kind: String): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: {
        POST: "/nested"
        body: """
        thing: {
          name: $args.name
        }
        kind: $args.kind
        """
      }
      selection: "id"
    )
  widget_co_flat(name: String!, kind: String): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: { POST: "/flat", body: "name: $args.name kind: $args.kind" }
      selection: "id"
    )
  widget_co_deleteWidget(id: ID!): Widget_Co_Widget
    @connect(
      source: "widget_co"
      http: { DELETE: "/widgets/{$args.id}" }
      selection: "id"
    )
}
"#;

fn hard_inventory() -> Value {
    let widget_response =
        json!({"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false});
    let op = |key: &str,
              id: &str,
              method: &str,
              path: &str,
              params: Value,
              request_body: Value,
              response: Value| {
        json!({"key": key, "operation_id": id, "method": method, "path": path, "semantics": "read", "provenance": "spec", "confidence": 1,
               "parameters": params, "request_body": request_body, "response": response, "errors": [], "support": "supported", "support_reason": null})
    };
    json!({"contract_version": 1, "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test/v2"]},
        "operations": [
            op("post:/things/{id}", "touchThing", "POST", "/things/{id}",
               json!([{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}, {"name": "note", "in": "query", "required": false, "schema": {"type": "string"}}]),
               Value::Null, widget_response.clone()),
            op("post:/nested", "nested", "POST", "/nested", json!([]),
               json!({"shape_ref": "#/shapes/NestedReq", "content_type": "application/json"}), widget_response.clone()),
            op("post:/flat", "flat", "POST", "/flat", json!([]),
               json!({"shape_ref": "#/shapes/FlatReq", "content_type": "application/json"}), widget_response.clone()),
            op("delete:/widgets/{id}", "deleteWidget", "DELETE", "/widgets/{id}",
               json!([{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}]),
               Value::Null, json!({"status": "204", "content_type": null, "envelope": null, "shape_ref": null, "list": false})),
            op("get:/cycle", "cycle", "GET", "/cycle", json!([]), Value::Null, widget_response.clone()),
            op("get:/ghost", "ghost", "GET", "/ghost", json!([]), Value::Null, widget_response.clone()),
            op("get:/dated/{when}", "dated", "GET", "/dated/{when}",
               json!([{"name": "when", "in": "path", "required": true, "schema": {"type": "string", "format": "date-time"}}]),
               Value::Null, widget_response.clone()),
            op("get:/literal", "literal", "GET", "/literal", json!([]), Value::Null, widget_response.clone()),
            op("get:/order", "order", "GET", "/order", json!([]), Value::Null, widget_response.clone()),
            op("get:/oneline", "oneline", "GET", "/oneline", json!([]), Value::Null, widget_response.clone()),
            op("get:/joined", "joined", "GET", "/joined",
               json!([{"name": "tags", "in": "query", "required": false, "schema": {"type": "array", "items": {"type": "string"}}}]),
               Value::Null, widget_response.clone()),
        ],
        "shapes": {
            "Widget": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "parent": {"$ref": "#/shapes/Widget"}}},
            "NestedReq": {"type": "object", "properties": {"thing": {"type": "object", "properties": {"name": {"type": "string"}}}, "kind": {"type": "string"}}},
            "FlatReq": {"type": "object", "properties": {"name": {"type": "string"}, "kind": {"type": "string"}}}
        },
        "unresolved": []})
}

fn hard_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w("widget-co.graphql", HARD_SDL);
    w("template.yaml", TEMPLATE);
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", HARD_SELECTION);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&hard_inventory()),
    );
    dir
}

fn entry_for(dir: &Path, target: &str) -> Option<Value> {
    let suite =
        graphos_factory_core::yaml::parse(&read(dir, "tests/widget-co.connector.yaml")).unwrap();
    suite["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["target"].as_str() == Some(target))
        .cloned()
}

fn notes_for(dir: &Path, key: &str) -> Vec<String> {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "scaffold",
            dir.to_str().unwrap(),
            "--dry-run",
            "--json",
            "--op",
            key,
            "--force",
        ])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    report["written"][0]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_write_without_a_body_mapping_is_loose_by_design_and_still_gets_its_minimal_case() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/touch_thing.json");
    assert!(stub["request"].get("bodyPatterns").is_none());
    assert!(stub["metadata"]["x-loose-body"]
        .as_str()
        .unwrap()
        .contains("no body mapping"));
    // lint mutation-cases wants the required-only case for any POST with
    // optional arguments, body or not.
    let min = read_json(d, "tests/fixtures/mappings/touch_thing_minimal.json");
    assert!(min["metadata"]["x-loose-body"].is_string());
    // The full stub demands `note`; the minimal one demands it absent, or it
    // would also match the full case's request and WireMock's tie-break
    // (the order stubs load in, which `e2e.sh`'s locale used to decide)
    // would pick which of the two answers it (ADR 0115).
    assert_eq!(
        stub["request"]["queryParameters"],
        json!({"note": {"equalTo": "note-2"}})
    );
    assert_eq!(
        min["request"]["queryParameters"],
        json!({"note": {"absent": true}})
    );
    assert!(read(d, "tests/cases/touch_thing_minimal.graphql").contains("id: \"id-1\""));
    // rover accepts a write entry without `body` when the connector declares
    // none, and fails one asserting `{}` — so no body key.
    let entry = entry_for(d, "Mutation.widget_co_touchThing").unwrap();
    assert!(
        entry["expect"]["connectorRequest"].get("body").is_none(),
        "{}",
        entry
    );
    assert_eq!(
        entry["expect"]["connectorRequest"]["url"],
        "https://api.widgets.test/v2/things/id-1?note=note-2"
    );
}

#[test]
fn a_body_mapping_that_is_not_flat_gets_no_body_assertion_and_no_unit_entry() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/nested.json");
    assert!(stub["request"].get("bodyPatterns").is_none(), "{}", stub);
    // Not explained away: lint loose-write-body must still ask for the body.
    assert!(stub["metadata"].get("x-loose-body").is_none());
    assert!(entry_for(d, "Mutation.widget_co_nested").is_none());
    let notes = notes_for(d, "post:/nested");
    assert!(
        notes.iter().any(|n| n.contains("not a flat")),
        "{:?}",
        notes
    );
    // The minimal case still exists (the document is right even when the
    // body cannot be asserted) and says its stub has no body either.
    assert!(d.join("tests/cases/nested_minimal.graphql").exists());
    assert!(
        notes
            .iter()
            .any(|n| n.contains("minimal case's stub asserts no body")),
        "{:?}",
        notes
    );
}

/// The AppWorld LLM arm writes `body` on one line with several pairs; until
/// ADR 0042 `body_mapping` read that as not flat, so the stub asserted no
/// body and no unit entry was written (the note above, wrongly).
#[test]
fn a_one_line_body_with_two_pairs_is_flat_and_gets_a_body_assertion_and_a_unit_entry() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/flat.json");
    let expected = &stub["request"]["bodyPatterns"][0]["equalToJson"];
    assert!(
        expected.get("name").is_some() && expected.get("kind").is_some(),
        "{}",
        stub
    );
    let entry = entry_for(d, "Mutation.widget_co_flat").expect("a unit entry for the flat write");
    let body: Value = serde_json::from_str(
        entry["expect"]["connectorRequest"]["body"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(
        body.get("name").is_some() && body.get("kind").is_some(),
        "{}",
        entry
    );
    let notes = notes_for(d, "post:/flat");
    assert!(
        !notes.iter().any(|n| n.contains("not a flat")),
        "{:?}",
        notes
    );
}

/// The same style on `queryParams`: until ADR 0042 `wiring()` read the first
/// pair of a one-line block only, so the stub's `queryParameters` and the unit
/// URL carried `alpha` and silently dropped `beta`.
#[test]
fn a_one_line_query_params_with_two_pairs_puts_both_keys_in_the_stub_and_the_unit_url() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/oneline.json");
    assert_eq!(stub["request"]["urlPath"], "/oneline");
    assert_eq!(
        stub["request"]["queryParameters"]["alpha"]["equalTo"],
        "alpha-1"
    );
    assert_eq!(
        stub["request"]["queryParameters"]["beta"]["equalTo"],
        "beta-2"
    );
    let entry =
        entry_for(d, "Query.widget_co_oneline").expect("a unit entry for the one-line read");
    assert_eq!(
        entry["expect"]["connectorRequest"]["url"],
        "https://api.widgets.test/v2/oneline?alpha=alpha-1&beta=beta-2"
    );
}

#[test]
fn an_operation_with_no_documented_response_body_gets_a_bodiless_stub_and_no_unit_entry() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/delete_widget.json");
    assert_eq!(stub["response"]["status"], 204);
    assert!(stub["response"].get("jsonBody").is_none(), "{}", stub);
    assert!(stub["response"].get("headers").is_none(), "{}", stub);
    assert!(entry_for(d, "Mutation.widget_co_deleteWidget").is_none());
    let notes = notes_for(d, "delete:/widgets/{id}");
    assert!(
        notes.iter().any(|n| n.contains("no response body")),
        "{:?}",
        notes
    );
}

#[test]
fn a_self_referential_shape_is_expanded_as_deep_as_the_selection_descends() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let body = read_json(d, "tests/fixtures/mappings/cycle.json")["response"]["jsonBody"].clone();
    assert!(body["parent"]["parent"]["id"].is_string(), "{}", body);
    assert_eq!(
        body["parent"]["parent"]["parent"],
        json!({}),
        "cut past the selection"
    );
    let entry = entry_for(d, "Query.widget_co_cycle").unwrap();
    let mapped: Value =
        serde_json::from_str(entry["expect"]["connectorResponse"].as_str().unwrap()).unwrap();
    assert_eq!(
        mapped["parent"]["parent"]["id"],
        body["parent"]["parent"]["id"]
    );
    assert!(mapped["parent"]["parent"].get("parent").is_none());
}

#[test]
fn a_selected_key_the_body_cannot_carry_drops_the_connector_response_instead_of_asserting_absence()
{
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let entry = entry_for(d, "Query.widget_co_ghost").unwrap();
    assert!(
        entry["expect"].get("connectorResponse").is_none(),
        "{}",
        entry
    );
    let notes = notes_for(d, "get:/ghost");
    assert!(
        notes.iter().any(|n| n.contains("`ghost_name`")),
        "{:?}",
        notes
    );
}

#[test]
fn path_arguments_stay_punctuation_free_and_are_percent_encoded() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let doc = read(d, "tests/cases/dated.graphql");
    assert!(
        doc.contains("when: \"when-1\""),
        "no date-time in a path slot:\n{}",
        doc
    );
    let stub = read_json(d, "tests/fixtures/mappings/dated.json");
    assert_eq!(stub["request"]["urlPath"], "/dated/when-1");
    let entry = entry_for(d, "Query.widget_co_dated").unwrap();
    assert_eq!(entry["variables"]["$args"]["when"], "when-1");
    assert_eq!(
        entry["expect"]["connectorRequest"]["url"],
        "https://api.widgets.test/v2/dated/when-1"
    );
}

#[test]
fn a_literal_query_string_in_the_path_is_asserted_everywhere() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/literal.json");
    assert_eq!(stub["request"]["urlPath"], "/literal");
    assert_eq!(stub["request"]["queryParameters"]["fixed"]["equalTo"], "1");
    assert_eq!(stub["request"]["queryParameters"]["mode"]["equalTo"], "a b");
    assert_eq!(stub["request"]["queryParameters"]["x"]["equalTo"], "x-1");
    let entry = entry_for(d, "Query.widget_co_literal").unwrap();
    assert_eq!(
        entry["expect"]["connectorRequest"]["url"],
        "https://api.widgets.test/v2/literal?fixed=1&mode=a%20b&x=x-1"
    );
}

#[test]
fn the_unit_url_follows_query_params_order_not_argument_order() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let entry = entry_for(d, "Query.widget_co_order").unwrap();
    assert_eq!(
        entry["expect"]["connectorRequest"]["url"],
        "https://api.widgets.test/v2/order?beta=beta-2&alpha=alpha-1"
    );
}

/// ADR 0051 lever 6. A list on a query key without `[]` was asserted as a
/// comma-joined value, with a note calling it a guess about the router's
/// serialisation. The first Databricks e2e runs answered it: on a plain
/// `key: $args.x` line the router repeats the key once per element
/// (`order_by=a&order_by=b`), so the stub now demands exactly that. A line in
/// the mapping language (`->joinNotNull(",")`) is still asserted as the
/// joined value, with the note.
#[test]
fn a_list_on_a_plain_query_key_is_asserted_as_the_key_repeated_per_element() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/joined.json");
    assert_eq!(
        stub["request"]["queryParameters"]["tags"],
        json!({"hasExactly": [{"equalTo": "tags-1"}, {"equalTo": "tags-2"}]}),
        "{}",
        stub
    );
    let notes = notes_for(d, "get:/joined");
    assert!(
        !notes.iter().any(|n| n.contains("comma-joined")),
        "{:?}",
        notes
    );

    // Through the mapping language, the connector joins it itself.
    let sdl = read(d, "widget-co.graphql").replace(
        "        tags: $args.tags\n",
        "        tags: $args.tags->joinNotNull(\",\")\n",
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    assert_eq!(scaffold(d, &["--op", "get:/joined", "--force"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/joined.json");
    assert_eq!(
        stub["request"]["queryParameters"]["tags"]["equalTo"], "tags-1,tags-2",
        "{}",
        stub
    );
    let notes = notes_for(d, "get:/joined");
    assert!(
        notes.iter().any(|n| n.contains("comma-joined")),
        "{:?}",
        notes
    );
}

/// The same on a one-line `queryParams`, the form ADR 0042 widened
/// `wiring()` to read (every AppWorld block is one): the plain-key reader
/// saw triple-quoted blocks with one pair per line only, so this list kept
/// the comma-joined assertion and gained a note blaming the mapping language
/// the line does not use.
#[test]
fn a_list_on_a_one_line_query_params_is_asserted_as_the_key_repeated_too() {
    let dir = hard_workspace();
    let d = dir.path();
    let sdl = read(d, "widget-co.graphql").replace(
        "        queryParams: \"\"\"\n        tags: $args.tags\n        \"\"\"\n",
        "        queryParams: \"tags: $args.tags\"\n",
    );
    assert!(sdl.contains("queryParams: \"tags: $args.tags\""));
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    assert_eq!(scaffold(d, &["--op", "get:/joined"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/joined.json");
    assert_eq!(
        stub["request"]["queryParameters"]["tags"],
        json!({"hasExactly": [{"equalTo": "tags-1"}, {"equalTo": "tags-2"}]}),
        "{}",
        stub
    );
    let notes = notes_for(d, "get:/joined");
    assert!(
        !notes.iter().any(|n| n.contains("comma-joined")),
        "{:?}",
        notes
    );
}

#[test]
fn force_needs_op_and_a_typoed_op_is_an_error_not_nothing_to_do() {
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    std::fs::write(d.join("tests/cases/version.graphql"), "HAND AUDITED\n").unwrap();
    assert_eq!(scaffold(d, &["--force"]), 1);
    assert_eq!(read(d, "tests/cases/version.graphql"), "HAND AUDITED\n");
    assert_eq!(scaffold(d, &["--op", "get:/nope"]), 1);
    assert_eq!(
        scaffold(d, &["--op", "get:/nope", "--op", "get:/version", "--force"]),
        1
    );
    assert_eq!(read(d, "tests/cases/version.graphql"), "HAND AUDITED\n");
    // An excluded operation is not an included one.
    assert_eq!(scaffold(d, &["--op", "delete:/widgets/{id}"]), 1);
}

#[test]
fn json_output_has_the_same_shape_when_there_is_nothing_to_do() {
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["scaffold", d.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["dry_run"], false);
    assert_eq!(report["suite"], "tests/widget-co.connector.yaml");
    assert!(report["written"].as_array().unwrap().is_empty());
    assert_eq!(report["skipped"].as_array().unwrap().len(), 3);
    assert_eq!(
        report["skipped"][0]["reason"],
        "tests/cases/list_widgets.graphql exists"
    );
}

#[test]
fn generated_files_end_with_one_newline_and_no_whitespace_only_lines() {
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &[]), 0);
    let stub = read(d, "tests/fixtures/mappings/version.json");
    assert!(
        stub.ends_with("}\n") && !stub.ends_with("\n\n"),
        "{:?}",
        &stub[stub.len() - 4..]
    );
    let suite = read(d, "tests/widget-co.connector.yaml");
    for (i, line) in suite.lines().enumerate() {
        assert!(
            line.is_empty() || !line.trim().is_empty(),
            "whitespace-only line {} in the suite",
            i + 1
        );
    }
}

/// A suite the scaffold did not write: items at column 0, a hand-written
/// entry for an operation the scaffold will also cover.
const HAND_SUITE: &str = "config:\n  schema: widget-co.graphql\n  common:\n    variables:\n      $config:\n        WIDGET_TOKEN: test-token\ntests:\n- name: \"hand-written version\"\n  target: \"Query.widget_co_version\"\n  apiResponseBody: |\n    {\"version\": \"9\"}\n  expect:\n    connectorRequest:\n      method: GET\n      url: https://api.widgets.test/v2/version\n";

#[test]
fn appending_to_a_hand_written_suite_keeps_it_valid_at_its_own_indentation() {
    let dir = workspace();
    let d = dir.path();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("tests/widget-co.connector.yaml"), HAND_SUITE).unwrap();
    assert_eq!(scaffold(d, &[]), 0);
    let text = read(d, "tests/widget-co.connector.yaml");
    let suite = graphos_factory_core::yaml::parse(&text).expect("still valid YAML");
    let tests = suite["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 4, "{}", text); // the hand entry + three scaffolded
    assert_eq!(tests[0]["name"], "hand-written version");
    assert!(
        text.contains("\n- name: \"widget_co_listWidgets"),
        "items at column 0:\n{}",
        text
    );
    assert!(text.starts_with("config:\n  schema: widget-co.graphql\n"));

    // --force on the operation the hand entry covers: the scaffold entry is
    // replaced, the hand entry stays, and the notes say so.
    assert_eq!(scaffold(d, &["--op", "get:/version", "--force"]), 0);
    let text = read(d, "tests/widget-co.connector.yaml");
    let suite = graphos_factory_core::yaml::parse(&text).unwrap();
    let versions: Vec<&Value> = suite["tests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["target"] == "Query.widget_co_version")
        .collect();
    assert_eq!(versions.len(), 2, "{}", text);
    assert_eq!(versions[0]["name"], "hand-written version");
    let notes = notes_for(d, "get:/version");
    assert!(
        notes.iter().any(|n| n.contains("hand-written entry")),
        "{:?}",
        notes
    );
}

#[test]
fn a_suite_without_a_tests_key_gets_one_and_an_invalid_suite_stops_everything() {
    let dir = workspace();
    let d = dir.path();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(
        d.join("tests/widget-co.connector.yaml"),
        "config:\n  schema: widget-co.graphql\n",
    )
    .unwrap();
    assert_eq!(scaffold(d, &[]), 0);
    let suite =
        graphos_factory_core::yaml::parse(&read(d, "tests/widget-co.connector.yaml")).unwrap();
    assert_eq!(suite["config"]["schema"], "widget-co.graphql");
    assert_eq!(suite["tests"].as_array().unwrap().len(), 3);

    // `tests: []` is opened.
    let dir2 = workspace();
    let d2 = dir2.path();
    std::fs::create_dir_all(d2.join("tests")).unwrap();
    std::fs::write(
        d2.join("tests/widget-co.connector.yaml"),
        "config:\n  schema: widget-co.graphql\ntests: []\n",
    )
    .unwrap();
    assert_eq!(scaffold(d2, &[]), 0);
    let suite =
        graphos_factory_core::yaml::parse(&read(d2, "tests/widget-co.connector.yaml")).unwrap();
    assert_eq!(suite["tests"].as_array().unwrap().len(), 3);

    // An unparseable suite: exit 1, and no case or stub is written either.
    let dir3 = workspace();
    let d3 = dir3.path();
    std::fs::create_dir_all(d3.join("tests")).unwrap();
    std::fs::write(
        d3.join("tests/widget-co.connector.yaml"),
        "tests:\n  - name: broken\n   target: [\n",
    )
    .unwrap();
    assert_eq!(scaffold(d3, &[]), 1);
    assert!(!d3.join("tests/cases").exists());
    assert_eq!(
        read(d3, "tests/widget-co.connector.yaml"),
        "tests:\n  - name: broken\n   target: [\n"
    );
}

#[test]
fn a_run_that_writes_no_unit_entry_creates_no_suite() {
    let dir = hard_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "delete:/widgets/{id}"]), 0);
    assert!(d.join("tests/cases/delete_widget.graphql").exists());
    assert!(
        !d.join("tests/widget-co.connector.yaml").exists(),
        "an empty suite would make lint's missing-unit fire for every operation"
    );
}

/// `--force` opens an existing suite even when the plan writes no entry, to
/// remove the scaffold's stale one (ADR 0051 lever 8). With nothing to
/// remove, the suite stays byte for byte as it was: opening it had rewritten
/// `tests: []` as `tests:`.
#[test]
fn force_leaves_the_suite_alone_when_no_entry_goes_in_or_comes_out() {
    let dir = hard_workspace();
    let d = dir.path();
    let before = "config:\n  schema: widget-co.graphql\ntests: []\n";
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("tests/widget-co.connector.yaml"), before).unwrap();
    assert_eq!(scaffold(d, &["--op", "delete:/widgets/{id}", "--force"]), 0);
    assert!(d.join("tests/cases/delete_widget.graphql").exists());
    assert_eq!(read(d, "tests/widget-co.connector.yaml"), before);

    // It is still parsed: an unparseable suite stops the run.
    let broken = "tests:\n  - name: broken\n   target: [\n";
    std::fs::write(d.join("tests/widget-co.connector.yaml"), broken).unwrap();
    assert_eq!(scaffold(d, &["--op", "delete:/widgets/{id}", "--force"]), 1);
    assert_eq!(read(d, "tests/widget-co.connector.yaml"), broken);
}

/// A write with optional arguments gets a `_minimal` case too, planned with
/// the same target and no unit entry of its own. Lever 8's removal took that
/// for "the new plan writes none" and deleted the entry the full case had
/// just appended: `--force` on a fresh workspace left a suite of `tests:`
/// alone, and on an existing one the note said "replaced" then "removed".
#[test]
fn force_on_a_write_with_a_minimal_case_keeps_the_entry_it_just_wrote() {
    let target = "Mutation.widget_co_createWidget";
    // A fresh workspace: the suite is created with the entry in it.
    let dir = workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "post:/widgets", "--force"]), 0);
    assert!(d.join("tests/cases/create_widget_minimal.graphql").exists());
    let entry = entry_for(d, target).unwrap_or_else(|| {
        panic!(
            "the entry survives: {}",
            read(d, "tests/widget-co.connector.yaml")
        )
    });
    assert_eq!(entry["expect"]["connectorRequest"]["method"], "POST");

    // Regenerated over the suite it wrote: replaced, still exactly one.
    assert_eq!(scaffold(d, &["--op", "post:/widgets", "--force"]), 0);
    let suite =
        graphos_factory_core::yaml::parse(&read(d, "tests/widget-co.connector.yaml")).unwrap();
    let n = suite["tests"]
        .as_array()
        .map(|a| a.iter().filter(|t| t["target"] == target).count())
        .unwrap_or(0);
    assert_eq!(n, 1, "{}", read(d, "tests/widget-co.connector.yaml"));
    let notes = notes_for(d, "post:/widgets");
    assert!(!notes.iter().any(|n| n.contains("removed")), "{:?}", notes);
}

#[test]
fn a_key_after_tests_is_refused_with_advice_and_nothing_written() {
    let dir = workspace();
    let d = dir.path();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    let text = "tests:\n- name: \"hand\"\n  target: Query.widget_co_version\n  apiResponseBody: |\n    {}\n  expect:\n    connectorRequest:\n      method: GET\n      url: https://api.widgets.test/v2/version\nconfig:\n  schema: widget-co.graphql\n";
    std::fs::write(d.join("tests/widget-co.connector.yaml"), text).unwrap();
    assert_eq!(scaffold(d, &[]), 1);
    assert!(!d.join("tests/cases").exists());
    assert_eq!(read(d, "tests/widget-co.connector.yaml"), text);
}

#[test]
fn an_unquoted_hand_written_target_is_still_recognised_by_force() {
    let dir = workspace();
    let d = dir.path();
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(
        d.join("tests/widget-co.connector.yaml"),
        "config:\n  schema: widget-co.graphql\ntests:\n  - name: hand\n    target: Query.widget_co_version\n    apiResponseBody: |\n      {}\n    expect:\n      connectorRequest:\n        method: GET\n        url: https://api.widgets.test/v2/version\n",
    )
    .unwrap();
    assert_eq!(scaffold(d, &[]), 0);
    let notes = notes_for(d, "get:/version");
    assert!(
        notes.iter().any(|n| n.contains("hand-written entry")),
        "{:?}",
        notes
    );
}

#[test]
fn a_non_json_response_body_gets_a_bodiless_stub_and_no_unit_entry() {
    let dir = workspace();
    let d = dir.path();
    let mut inv = inventory();
    inv["operations"][1]["response"]["content_type"] = json!("text/csv");
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();
    assert_eq!(scaffold(d, &["--op", "get:/version"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/version.json");
    assert!(stub["response"].get("jsonBody").is_none(), "{}", stub);
    assert!(stub["response"].get("headers").is_none(), "{}", stub);
    assert!(!d.join("tests/widget-co.connector.yaml").exists());
    let notes = notes_for(d, "get:/version");
    assert!(notes.iter().any(|n| n.contains("text/csv")), "{:?}", notes);
}

// ── Codex review loop, third pass, concern 3.4: the parser fix's fourth
// caller (render_selection) renders an invalid GraphQL selection for an
// opaque, childless node aliasing an object-typed field. ───────────────

fn copy_pilot_without_tests(from: &Path, to: &Path) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let name = e.file_name();
            let n = name.to_string_lossy();
            if n == "tests" || n == "evidence" {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to.join(&name));
            } else {
                std::fs::copy(e.path(), to.join(&name)).unwrap();
            }
        }
    }
    copy(from, to);
}

/// Runs `scaffold --json` and parses stdout, capturing it the way the
/// existing `scaffold()` helper (which only returns the exit code) does
/// not -- needed here to read the `invalid` list.
fn scaffold_json(dir: &Path, args: &[&str]) -> Value {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let bin = env!("CARGO_BIN_EXE_graphos-factory-bare");
    let mut argv: Vec<String> = vec!["scaffold".to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let mut child = Command::new(bin)
        .args(&argv)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child.wait().unwrap();
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{}: {:?}", e, out))
}

#[test]
fn every_pilot_scaffolds_only_valid_graphql_or_explicitly_refuses_the_rest() {
    // A generic renderer defect (any opaque node aliasing a
    // composite-returning field triggers it, not only the originally
    // reported vendor case, proven separately against a vendored fixture
    // subset of that workspace above) -- swept across a real pilot here
    // and a target's own pilots in its suite, so a fix that only special-cases the one reported field would
    // still be caught, and any other pre-existing occurrence would surface
    // too.
    for name in ["gitea"] {
        let pilot = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../pilots/graphos")
            .join(name);
        if !pilot.join(".factory/inventory.json").exists() {
            eprintln!("skipping {}: not checked out next to the crate", name);
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        copy_pilot_without_tests(&pilot, d);
        let code = scaffold(d, &[]);
        assert!(
            code == 0 || code == 3,
            "{}: unexpected exit {} (0 = all written, 3 = some refused, nothing else is a clean outcome here)",
            name,
            code
        );

        // Every case file that *was* written must itself validate against
        // the pilot's own schema -- not merely trust the exit code, in
        // case a future change reintroduces the write-before-validate
        // ordering this fix closes.
        let schema_files: Vec<_> = std::fs::read_dir(d)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                e.path()
                    .extension()
                    .map(|x| x == "graphql")
                    .unwrap_or(false)
            })
            .map(|e| e.path())
            .into_iter()
            .collect();
        let sdl = std::fs::read_to_string(&schema_files[0]).unwrap();
        let schema = apollo_compiler::validation::Valid::assume_valid(
            apollo_compiler::Schema::parse(&sdl, "schema.graphql").unwrap(),
        );
        let cases_dir = d.join("tests/cases");
        if cases_dir.exists() {
            for entry in std::fs::read_dir(&cases_dir).unwrap() {
                let path = entry.unwrap().path();
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    apollo_compiler::ExecutableDocument::parse_and_validate(
                        &schema,
                        &text,
                        "generated.graphql"
                    )
                    .is_ok(),
                    "{}: {} was written but does not validate:\n{}",
                    name,
                    path.display(),
                    text
                );
            }
        }
    }
}

/// ADR 0051 lever 1. A required input-object argument used to get no
/// placeholder ("pass it by hand"), so the document omitted it and the router
/// rejected the case. It is now sampled from the schema: every required field
/// (a nested required input included), one representative optional one, and a
/// custom scalar valued as the JSON type of the wire slot it feeds. The stub
/// then demands that body exactly. rover cannot pass an object-valued $args,
/// so a required input object gets no unit entry, and says why.
#[test]
fn a_required_input_object_argument_is_sampled_from_the_schema_and_its_body_asserted() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "scalar Widget_Co_Int64\n\n",
            "input Widget_Co_OwnerInput {\n  login: String!\n  isAdmin: Boolean\n}\n\n",
            "input Widget_Co_WidgetImportInput {\n  name: String!\n  owner: Widget_Co_OwnerInput!\n  weight: Widget_Co_Int64!\n  color: Widget_Co_Color\n  note: String\n}\n\n",
            "type Mutation {\n  widget_co_importWidget(input: Widget_Co_WidgetImportInput!, dryRun: Boolean): Widget_Co_Widget\n    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/widgets/import\"\n        body: \"\"\"\n        widget: $args.input\n        dry_run: $args.dryRun\n        \"\"\"\n      }\n      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/widgets/import\":\n    include: true\n    graphql: {{ root: mutation, name: importWidget }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "post:/widgets/import", "operation_id": "importWidget", "method": "POST", "path": "/widgets/import",
        "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/ImportWidget", "content_type": "application/json"},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["ImportWidget"] = json!({"type": "object", "required": ["widget"], "properties": {
        "widget": {"type": "object", "required": ["name", "owner", "weight"], "properties": {
            "name": {"type": "string"},
            "owner": {"type": "object", "required": ["login"], "properties": {"login": {"type": "string"}, "isAdmin": {"type": "boolean"}}},
            "weight": {"type": "integer", "format": "int64"},
            "color": {"type": "string"},
            "note": {"type": "string"}}},
        "dry_run": {"type": "boolean"}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "post:/widgets/import"]), 0);
    let case = read(d, "tests/cases/import_widget.graphql");
    // Every required field, the nested required input, the scalar as an
    // integer literal, and one optional field per object (the first in
    // order); an enum value cycles with the field's position.
    assert!(
        case.contains(r#"input: { name: "name-1", owner: { login: "login-1", isAdmin: true }, weight: 3, color: BLUE }"#)
            && case.contains("dryRun: true"),
        "{}",
        case
    );
    let stub = read_json(d, "tests/fixtures/mappings/import_widget.json");
    assert_eq!(
        stub["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"widget": {"name": "name-1", "owner": {"login": "login-1", "isAdmin": true}, "weight": 3, "color": "BLUE"}, "dry_run": true}),
        "{}",
        stub
    );
    // The minimal case: required arguments only, and inside the input object
    // its required fields only.
    let minimal = read_json(d, "tests/fixtures/mappings/import_widget_minimal.json");
    assert_eq!(
        minimal["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"widget": {"name": "name-1", "owner": {"login": "login-1"}, "weight": 3}}),
        "{}",
        minimal
    );
    assert!(
        !d.join("tests/widget-co.connector.yaml").exists()
            || entry_for(d, "Mutation.widget_co_importWidget").is_none()
    );
    let out = scaffold_json(
        d,
        &[
            "--op",
            "post:/widgets/import",
            "--force",
            "--dry-run",
            "--json",
        ],
    );
    assert!(
        out["invalid"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "{}",
        out
    );
    let text = graphos_factory_core::json::compact(&out);
    assert!(!text.contains("pass it by hand"), "{}", out);
    assert!(text.contains("rejects object-valued $args"), "{}", out);
}

/// The exemption above is for the root field's own omitted argument only. A
/// nested field whose required argument shares the name (`input`) is a
/// document scaffold rendered wrong, so the operation is still refused.
#[test]
fn a_nested_required_argument_sharing_the_omitted_name_still_refuses() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL
        .replace(
            "type Widget_Co_Widget {\n  id: ID!\n",
            "type Widget_Co_Widget {\n  id: ID!\n  label(input: String!): String\n",
        )
        .replace(
            "type Mutation {\n",
            "input Widget_Co_WidgetImportInput {\n  name: String!\n}\n\ntype Mutation {\n  widget_co_importWidget(input: Widget_Co_WidgetImportInput!): Widget_Co_Widget\n    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/widgets/import\"\n        body: \"\"\"\n        widget: $args.input\n        \"\"\"\n      }\n      selection: \"\"\"\n      id\n      name\n      label\n      \"\"\"\n    )\n",
        );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/widgets/import\":\n    include: true\n    graphql: {{ root: mutation, name: importWidget }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "post:/widgets/import", "operation_id": "importWidget", "method": "POST", "path": "/widgets/import",
        "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/ImportWidget", "content_type": "application/json"},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["ImportWidget"] = json!({"type": "object", "required": ["widget"], "properties": {
        "widget": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "post:/widgets/import"]), 3);
    assert!(!d.join("tests/cases/import_widget.graphql").exists());
    let out = scaffold_json(d, &["--op", "post:/widgets/import", "--dry-run", "--json"]);
    let invalid = graphos_factory_core::json::compact(&out["invalid"]);
    assert!(invalid.contains("label(input:)"), "{}", out);
}

/// Builds the workspace for the lever 2 tests: two writes whose body maps
/// input objects back to wire names with a sub-selection.
fn nested_body_workspace() -> tempfile::TempDir {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "input Widget_Co_AddressInput {\n  postalCode: String!\n  city: String\n}\n\n",
            "input Widget_Co_OrderInput {\n  customerId: String!\n  shippingAddress: Widget_Co_AddressInput!\n}\n\n",
            "input Widget_Co_LineInput {\n  sku: String!\n  quantity: Int!\n}\n\n",
            "input Widget_Co_MetaInput {\n  sourceApp: String!\n}\n\n",
            "type Mutation {\n",
            "  widget_co_createOrder(order: Widget_Co_OrderInput!, lines: [Widget_Co_LineInput!], note: String): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/orders\"\n        body: \"\"\"\n",
            "        order: $args.order {\n          customer_id: customerId\n          shipping: shippingAddress {\n            postal_code: postalCode\n            city\n          }\n        }\n",
            "        line_items: $args.lines {\n          sku\n          qty: quantity\n        }\n        note: $args.note\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
            "  widget_co_updateNote(id: ID!, note: String!, meta: Widget_Co_MetaInput): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/notes/{$args.id}\"\n        body: \"\"\"\n",
            "        note_text: $args.note\n        meta: $args.meta {\n          source_app: sourceApp\n        }\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/orders\":\n    include: true\n    graphql: {{ root: mutation, name: createOrder }}\n  \"post:/notes/{{id}}\":\n    include: true\n    graphql: {{ root: mutation, name: updateNote }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    let op = |key: &str, id: &str, path: &str, params: Value, shape: &str| {
        json!({"key": key, "operation_id": id, "method": "POST", "path": path,
            "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": params,
            "request_body": {"shape_ref": format!("#/shapes/{}", shape), "content_type": "application/json"},
            "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
            "errors": [], "support": "supported", "support_reason": null})
    };
    let ops = inv["operations"].as_array_mut().unwrap();
    ops.push(op(
        "post:/orders",
        "createOrder",
        "/orders",
        json!([]),
        "CreateOrder",
    ));
    ops.push(op(
        "post:/notes/{id}",
        "updateNote",
        "/notes/{id}",
        json!([{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}]),
        "UpdateNote",
    ));
    inv["shapes"]["CreateOrder"] = json!({"type": "object", "required": ["order"], "properties": {
        "order": {"type": "object", "properties": {
            "customer_id": {"type": "string"},
            "shipping": {"type": "object", "properties": {"postal_code": {"type": "string"}, "city": {"type": "string"}}}}},
        "line_items": {"type": "array", "items": {"type": "object", "properties": {"sku": {"type": "string"}, "qty": {"type": "integer"}}}},
        "note": {"type": "string"}}});
    inv["shapes"]["UpdateNote"] = json!({"type": "object", "properties": {
        "note_text": {"type": "string"},
        "meta": {"type": "object", "properties": {"source_app": {"type": "string"}}}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();
    dir
}

/// ADR 0051 lever 2. A body that maps an input object back to wire names
/// with a sub-selection (`order: $args.order { customer_id: customerId … }`)
/// was "not flat": the stub asserted no body, so a full case and its
/// `_minimal` sibling both matched any POST to the path. The mapping is now
/// evaluated against the case's own values, so the stub demands exactly the
/// wire names, renames and list elements included.
#[test]
fn a_nested_body_mapping_is_evaluated_into_an_exact_stub_body_with_wire_names() {
    let dir = nested_body_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "post:/orders"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/create_order.json");
    let pattern = &stub["request"]["bodyPatterns"][0];
    assert_eq!(
        pattern["equalToJson"],
        json!({
            "order": {"customer_id": "customerId-1", "shipping": {"postal_code": "postalCode-1", "city": "city-2"}},
            "line_items": [{"sku": "sku-1", "qty": 2}, {"sku": "sku-1", "qty": 2}],
            "note": "note-3"
        }),
        "{}",
        stub
    );
    // Exact: nothing may be ignored, or the matcher proves nothing.
    assert!(pattern.get("ignoreExtraElements").is_none(), "{}", stub);
    let minimal = read_json(d, "tests/fixtures/mappings/create_order_minimal.json");
    assert_eq!(
        minimal["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"order": {"customer_id": "customerId-1", "shipping": {"postal_code": "postalCode-1"}}}),
        "{}",
        minimal
    );
    let notes = notes_for(d, "post:/orders");
    assert!(
        !notes.iter().any(|n| n.contains("not a flat")),
        "{:?}",
        notes
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("minimal case's stub asserts no body")),
        "{:?}",
        notes
    );
}

/// An input object read field by field (`customer_id: $args.order.customerId`
/// then `address: $args.order.address { … }`). The argument's wire shape was
/// taken from the first top-level key naming it, so `order` got the shape
/// of the integer `customer_id`: no field found its wire property, both
/// custom scalars were sampled as `{}`, and the stub demanded a body the
/// spec rejects. A one-line body was read no further than its first key, or
/// not at all when single-quoted. The shape is now walked from the request
/// shape along every entry of the parsed mapping, in each form.
#[test]
fn an_input_object_read_field_by_field_is_sampled_from_each_fields_wire_slot() {
    let multi = "        body: \"\"\"\n        customer_id: $args.order.customerId\n        address: $args.order.address {\n          street_name: streetName\n          moved_at: movedAt\n        }\n        \"\"\"\n";
    let one_line = "        body: \"\"\"customer_id: $args.order.customerId address: $args.order.address { street_name: streetName moved_at: movedAt }\"\"\"\n";
    let single = "        body: \"customer_id: $args.order.customerId address: $args.order.address { street_name: streetName moved_at: movedAt }\"\n";
    for body in [multi, one_line, single] {
        let dir = workspace();
        let d = dir.path();
        let sdl = SDL.replace(
            "type Mutation {\n",
            &format!(
                concat!(
                    "scalar Widget_Co_Int64\n\nscalar Widget_Co_DateTime\n\n",
                    "input Widget_Co_ShipAddressInput {{\n  streetName: String!\n  movedAt: Widget_Co_DateTime!\n}}\n\n",
                    "input Widget_Co_ShipOrderInput {{\n  customerId: Widget_Co_Int64!\n  address: Widget_Co_ShipAddressInput!\n}}\n\n",
                    "type Mutation {{\n",
                    "  widget_co_createShipment(order: Widget_Co_ShipOrderInput!): Widget_Co_Widget\n",
                    "    @connect(\n      source: \"widget_co\"\n      http: {{\n        POST: \"/shipments\"\n{}      }}\n",
                    "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
                ),
                body
            ),
        );
        std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
        std::fs::write(
            d.join(".factory/selection.yaml"),
            format!(
                "{}  \"post:/shipments\":\n    include: true\n    graphql: {{ root: mutation, name: createShipment }}\n",
                SELECTION
            ),
        )
        .unwrap();
        let mut inv = inventory();
        inv["operations"].as_array_mut().unwrap().push(json!({
            "key": "post:/shipments", "operation_id": "createShipment", "method": "POST", "path": "/shipments",
            "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
            "request_body": {"shape_ref": "#/shapes/CreateShipment", "content_type": "application/json"},
            "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
            "errors": [], "support": "supported", "support_reason": null}));
        inv["shapes"]["CreateShipment"] = json!({"type": "object", "properties": {
            "customer_id": {"type": "integer", "format": "int64"},
            "address": {"type": "object", "properties": {
                "street_name": {"type": "string"},
                "moved_at": {"type": "string", "format": "date-time"}}}}});
        std::fs::write(
            d.join(".factory/inventory.json"),
            graphos_factory_core::json::pretty(&inv),
        )
        .unwrap();

        assert_eq!(scaffold(d, &["--op", "post:/shipments"]), 0, "{}", body);
        let stub = read_json(d, "tests/fixtures/mappings/create_shipment.json");
        assert_eq!(
            stub["request"]["bodyPatterns"][0]["equalToJson"],
            json!({"customer_id": 1, "address": {"street_name": "streetName-1", "moved_at": "2026-01-02T00:00:00Z"}}),
            "{}\n{}",
            body,
            stub
        );
    }
}

/// A body path that crosses a list (`skus: $args.order.lines.sku`) is mapped
/// over it: Router 2.17 sends `"skus": ["a", "b"]` for two lines, at the top
/// level and inside a sub-selection alike (observed against a local echo
/// upstream). The evaluation walked the list as an object, found no `sku`,
/// and dropped the key, so the stub asserted a body without it.
#[test]
fn a_body_path_through_a_list_is_mapped_over_the_list() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "input Widget_Co_SkuLineInput {\n  sku: String!\n}\n\n",
            "input Widget_Co_SkuOrderInput {\n  customerId: String!\n  lines: [Widget_Co_SkuLineInput!]!\n}\n\n",
            "type Mutation {\n",
            "  widget_co_createSkuOrder(order: Widget_Co_SkuOrderInput!): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/sku-orders\"\n        body: \"\"\"\n",
            "        customer_id: $args.order.customerId\n        skus: $args.order.lines.sku\n",
            "        wrapped: $args.order { first_skus: lines.sku }\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/sku-orders\":\n    include: true\n    graphql: {{ root: mutation, name: createSkuOrder }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "post:/sku-orders", "operation_id": "createSkuOrder", "method": "POST", "path": "/sku-orders",
        "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/SkuOrder", "content_type": "application/json"},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["SkuOrder"] = json!({"type": "object", "properties": {
        "customer_id": {"type": "string"},
        "skus": {"type": "array", "items": {"type": "string"}},
        "wrapped": {"type": "object", "properties": {
            "first_skus": {"type": "array", "items": {"type": "string"}}}}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "post:/sku-orders"]), 0);
    let case = read(d, "tests/cases/create_sku_order.graphql");
    assert!(
        case.contains("lines: [{ sku: \"sku-1\" }, { sku: \"sku-1\" }]"),
        "{}",
        case
    );
    let stub = read_json(d, "tests/fixtures/mappings/create_sku_order.json");
    assert_eq!(
        stub["request"]["bodyPatterns"][0]["equalToJson"],
        json!({
            "customer_id": "customerId-1",
            "skus": ["sku-1", "sku-1"],
            "wrapped": {"first_skus": ["sku-1", "sku-1"]}
        }),
        "{}",
        stub
    );
}

/// A `String` argument on an integer wire slot (the non-identifying int64
/// lessons.md types as `String`) was sampled as the integer literal `1`,
/// which GraphQL rejects for a `String`: the whole operation was refused
/// (exit 3). It is now the numeric string; an `ID` there stays an integer,
/// which it accepts.
#[test]
fn a_string_argument_on_an_integer_slot_is_sampled_as_a_numeric_string() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "type Mutation {\n",
            "  widget_co_linkWidget(externalId: String!, ownerId: ID!): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/links\"\n",
            "        body: \"\"\"\n        external_id: $args.externalId\n        owner_id: $args.ownerId\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/links\":\n    include: true\n    graphql: {{ root: mutation, name: linkWidget }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "post:/links", "operation_id": "linkWidget", "method": "POST", "path": "/links",
        "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/Link", "content_type": "application/json"},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["Link"] = json!({"type": "object", "required": ["external_id", "owner_id"], "properties": {
        "external_id": {"type": "integer", "format": "int64"},
        "owner_id": {"type": "integer", "format": "int64"}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "post:/links"]), 0);
    let case = read(d, "tests/cases/link_widget.graphql");
    assert!(case.contains("externalId: \"1\""), "{}", case);
    assert!(case.contains("ownerId: 2"), "{}", case);
    let stub = read_json(d, "tests/fixtures/mappings/link_widget.json");
    assert_eq!(
        stub["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"external_id": "1", "owner_id": 2}),
        "{}",
        stub
    );
}

/// The same mapping with the input object optional: the unit entry leaves the
/// object out (rover rejects object-valued $args) and asserts the body rover
/// builds from what is left, through the same evaluation.
#[test]
fn a_nested_body_with_an_optional_input_object_gets_a_unit_entry_asserting_the_rest() {
    let dir = nested_body_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "post:/notes/{id}"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/update_note.json");
    assert_eq!(
        stub["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"note_text": "note-2", "meta": {"source_app": "sourceApp-1"}}),
        "{}",
        stub
    );
    let entry = entry_for(d, "Mutation.widget_co_updateNote").expect("a unit entry");
    let body: Value = serde_json::from_str(
        entry["expect"]["connectorRequest"]["body"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body, json!({"note_text": "note-2"}), "{}", entry);
    assert!(
        entry["variables"]["$args"].get("meta").is_none(),
        "{}",
        entry
    );
}

/// ADR 0051 lever 2b. An input-object query argument goes on the wire as
/// dotted keys (`"filter_by.min_weight": $args.filterBy.minWeight`).
/// `wiring()` keyed the argument by the whole dotted key, which names no
/// parameter, so its custom-scalar leaf was sampled as `{}` (the case never reached
/// the upstream) and the stub asserted none of the keys. The parameter is
/// now the key's first segment, and the stub asserts every leaf.
#[test]
fn an_input_object_on_dotted_query_keys_is_sampled_from_its_parameter_and_asserted() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Query {\n",
        concat!(
            "scalar Widget_Co_Int64\n\n",
            "input Widget_Co_FilterInput {\n  minWeight: Widget_Co_Int64\n  label: String\n}\n\n",
            "type Query {\n",
            "  widget_co_searchWidgets(filterBy: Widget_Co_FilterInput!, pageToken: String): [Widget_Co_Widget]\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        GET: \"/widgets/search\"\n        queryParams: \"\"\"\n",
            "        \"filter_by.min_weight\": $args.filterBy.minWeight\n        \"filter_by.label\": $args.filterBy.label\n        page_token: $args.pageToken\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"get:/widgets/search\":\n    include: true\n    graphql: {{ root: query, name: searchWidgets }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "get:/widgets/search", "operation_id": "searchWidgets", "method": "GET", "path": "/widgets/search",
        "semantics": "read", "provenance": "spec", "confidence": 1,
        "parameters": [
            {"name": "filter_by", "in": "query", "required": true, "type": "object", "shape_ref": "#/shapes/Filter"},
            {"name": "page_token", "in": "query", "required": false, "type": "string"}],
        "request_body": null,
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/WidgetList", "list": true},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["Filter"] = json!({"type": "object", "properties": {
        "min_weight": {"type": "integer", "format": "int64"}, "label": {"type": "string"}}});
    inv["shapes"]["WidgetList"] = json!({"type": "array", "items": {"$ref": "#/shapes/Widget"}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "get:/widgets/search"]), 0);
    let case = read(d, "tests/cases/search_widgets.graphql");
    assert!(case.contains("filterBy: { minWeight: 1 }"), "{}", case);
    let stub = read_json(d, "tests/fixtures/mappings/search_widgets.json");
    let qp = &stub["request"]["queryParameters"];
    assert_eq!(qp["filter_by.min_weight"]["equalTo"], "1", "{}", stub);
    assert_eq!(qp["page_token"]["equalTo"], "pageToken-2", "{}", stub);
    // A leaf the case does not pass is not asserted at all, rather than as
    // the whole object's JSON.
    assert!(qp.get("filter_by.label").is_none(), "{}", stub);

    // The same entries on one line (ADR 0042): the same stub.
    let sdl = read(d, "widget-co.graphql").replace(
        "        queryParams: \"\"\"\n        \"filter_by.min_weight\": $args.filterBy.minWeight\n        \"filter_by.label\": $args.filterBy.label\n        page_token: $args.pageToken\n        \"\"\"\n",
        "        queryParams: \"\"\" \"filter_by.min_weight\": $args.filterBy.minWeight \"filter_by.label\": $args.filterBy.label page_token: $args.pageToken \"\"\"\n",
    );
    assert!(sdl.contains("queryParams: \"\"\" \"filter_by"), "{}", sdl);
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    assert_eq!(scaffold(d, &["--op", "get:/widgets/search", "--force"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/search_widgets.json");
    let qp = &stub["request"]["queryParameters"];
    assert_eq!(qp["filter_by.min_weight"]["equalTo"], "1", "{}", stub);
    assert_eq!(qp["page_token"]["equalTo"], "pageToken-2", "{}", stub);
    assert!(qp.get("filter_by.label").is_none(), "{}", stub);
}

/// ADR 0051 lever 3. The example body is built to a depth cap that counts
/// array hops, while the selection's depth does not: three arrays on one
/// selected path (Databricks `results.@.widgets.@.visualization.query.options
/// .parameters.@.multiValuesOptions.prefix`) cut the branch to `{}` before
/// its leaf, so the unit entry's apiResponseBody lacked a selected key and
/// rover failed "Property .prefix not found in object". Every selected path
/// is now in the body.
#[test]
fn every_selected_path_is_in_the_response_body_however_many_arrays_it_crosses() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Query {\n",
        concat!(
            "type Widget_Co_G { prefix: String }\n",
            "type Widget_Co_F { options: Widget_Co_G }\n",
            "type Widget_Co_E { f: Widget_Co_F }\n",
            "type Widget_Co_D { es: [Widget_Co_E] }\n",
            "type Widget_Co_C { ds: [Widget_Co_D] }\n",
            "type Widget_Co_B { c: Widget_Co_C }\n",
            "type Widget_Co_Page { bs: [Widget_Co_B] }\n\n",
            "type Query {\n",
            "  widget_co_deep: Widget_Co_Page\n",
            "    @connect(\n      source: \"widget_co\"\n      http: { GET: \"/deep\" }\n",
            "      selection: \"\"\"\n      bs {\n        c {\n          ds {\n            es {\n              f {\n                options {\n                  prefix\n                }\n              }\n            }\n          }\n        }\n      }\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"get:/deep\":\n    include: true\n    graphql: {{ root: query, name: deep }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "get:/deep", "operation_id": "deep", "method": "GET", "path": "/deep",
        "semantics": "read", "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null,
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Page", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    let o = |props: Value| json!({"type": "object", "properties": props});
    inv["shapes"]["G"] = o(json!({"prefix": {"type": "string"}, "suffix": {"type": "string"}}));
    inv["shapes"]["F"] = o(json!({"options": {"$ref": "#/shapes/G"}}));
    inv["shapes"]["E"] = o(json!({"f": {"$ref": "#/shapes/F"}}));
    inv["shapes"]["D"] = o(json!({"es": {"type": "array", "items": {"$ref": "#/shapes/E"}}}));
    inv["shapes"]["C"] = o(json!({"ds": {"type": "array", "items": {"$ref": "#/shapes/D"}}}));
    inv["shapes"]["B"] = o(json!({"c": {"$ref": "#/shapes/C"}}));
    inv["shapes"]["Page"] = o(json!({"bs": {"type": "array", "items": {"$ref": "#/shapes/B"}}}));
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "get:/deep"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/deep.json");
    assert_eq!(
        stub["response"]["jsonBody"]["bs"][0]["c"]["ds"][0]["es"][0]["f"]["options"]["prefix"],
        "prefix-1",
        "{}",
        stub
    );
    let entry = entry_for(d, "Query.widget_co_deep").expect("a unit entry");
    assert!(
        entry["expect"].get("connectorResponse").is_some(),
        "the mapped response is asserted: {}",
        entry
    );
    let notes = notes_for(d, "get:/deep");
    assert!(
        !notes.iter().any(|n| n.contains("does not carry")),
        "{:?}",
        notes
    );
}

/// ADR 0051 lever 5. The depth cap cuts an unselected branch to its type's
/// bare leaf, `{}` for an object, even when the shape requires properties
/// there, so the stub's response body failed `validate` (Databricks
/// `…external_model_config.target: missing required property "model"`). A
/// cut object now keeps every property its shape requires, each as its own
/// smallest value.
#[test]
fn a_branch_cut_by_the_depth_cap_keeps_the_properties_its_shape_requires() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Query {\n",
        concat!(
            "type Widget_Co_Shallow { name: String }\n\n",
            "type Query {\n",
            "  widget_co_shallow: Widget_Co_Shallow\n",
            "    @connect(\n      source: \"widget_co\"\n      http: { GET: \"/shallow\" }\n",
            "      selection: \"\"\"\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"get:/shallow\":\n    include: true\n    graphql: {{ root: query, name: shallow }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "get:/shallow", "operation_id": "shallow", "method": "GET", "path": "/shallow",
        "semantics": "read", "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null,
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Top", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    let o = |props: Value| json!({"type": "object", "properties": props});
    inv["shapes"]["Top"] = o(json!({"name": {"type": "string"}, "a": {"$ref": "#/shapes/A"}}));
    inv["shapes"]["A"] = o(json!({"b": {"$ref": "#/shapes/B"}}));
    inv["shapes"]["B"] = o(json!({"c": {"$ref": "#/shapes/C"}}));
    inv["shapes"]["C"] = o(json!({"d": {"$ref": "#/shapes/D"}}));
    inv["shapes"]["D"] = o(json!({"target": {"$ref": "#/shapes/Target"}}));
    inv["shapes"]["Target"] = json!({"type": "object", "required": ["model", "spec"], "properties": {
        "model": {"type": "string"}, "spec": {"$ref": "#/shapes/Spec"}, "extra": {"type": "string"}}});
    inv["shapes"]["Spec"] = json!({"type": "object", "required": ["kind"], "properties": {
        "kind": {"type": "string", "enum": ["LLM", "EMBEDDING"]}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "get:/shallow"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/shallow.json");
    let target = &stub["response"]["jsonBody"]["a"]["b"]["c"]["d"]["target"];
    assert_eq!(target["model"], "model-1", "{}", stub);
    assert_eq!(target["spec"]["kind"], "LLM", "{}", stub);
    // Only what the shape requires, not the optional properties.
    assert!(target.get("extra").is_none(), "{}", stub);
}

/// Lever 5 on a recursive shape: the cut lands where `ForEach` requires
/// `task`, a `Task` already on the stack. The cycle guard used to give it a
/// bare `{}`, which dropped `task_key`, the string `Task` itself requires
/// (Databricks jobs `…for_each_task.task: missing required property
/// "task_key"`). Only required properties are followed, so the revisit ends.
#[test]
fn a_cut_on_a_recursive_shape_keeps_the_required_fields_of_the_repeated_type() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Query {\n",
        concat!(
            "type Widget_Co_Job { name: String }\n\n",
            "type Query {\n",
            "  widget_co_job: Widget_Co_Job\n",
            "    @connect(\n      source: \"widget_co\"\n      http: { GET: \"/job\" }\n",
            "      selection: \"\"\"\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"get:/job\":\n    include: true\n    graphql: {{ root: query, name: job }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "get:/job", "operation_id": "job", "method": "GET", "path": "/job",
        "semantics": "read", "provenance": "spec", "confidence": 1, "parameters": [], "request_body": null,
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Job", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["Job"] = json!({"type": "object", "properties": {
        "name": {"type": "string"}, "task": {"$ref": "#/shapes/Task"}}});
    // The depth cap lands on ForEach (depth 5), with Task already on the
    // stack from depth 1: its required `task` closes the cycle.
    inv["shapes"]["Task"] = json!({"type": "object", "required": ["task_key"], "properties": {
        "task_key": {"type": "string"}, "w1": {"$ref": "#/shapes/W1"}}});
    inv["shapes"]["W1"] = json!({"type": "object", "properties": {"w2": {"$ref": "#/shapes/W2"}}});
    inv["shapes"]["W2"] = json!({"type": "object", "properties": {"w3": {"$ref": "#/shapes/W3"}}});
    inv["shapes"]["W3"] =
        json!({"type": "object", "properties": {"for_each": {"$ref": "#/shapes/ForEach"}}});
    inv["shapes"]["ForEach"] = json!({"type": "object", "required": ["task"], "properties": {
        "task": {"$ref": "#/shapes/Task"}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "get:/job"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/job.json");
    let inner = &stub["response"]["jsonBody"]["task"]["w1"]["w2"]["w3"]["for_each"]["task"];
    assert!(inner.is_object(), "{}", stub);
    assert_eq!(inner["task_key"], "task_key-1", "{}", stub);
}

/// ADR 0051 lever 7. A closed vocabulary whose values are not GraphQL enum
/// names (SCIM `urn:ietf:params:scim:schemas:core:2.0:Group`) is kept as a
/// String, and scaffold sampled it `schemas-1`, a body the spec rejects
/// (`"schemas-1" is not one of …`). A String placeholder now takes the wire
/// slot's first `enum` value, a list's elements included.
#[test]
fn a_string_argument_on_a_closed_vocabulary_is_sampled_from_the_vocabulary() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "type Mutation {\n",
            "  widget_co_createGroup(displayName: String!, schemas: [String!], provider: String): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/groups\"\n        body: \"\"\"\n",
            "        displayName: $args.displayName\n        schemas: $args.schemas\n        provider: $args.provider\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/groups\":\n    include: true\n    graphql: {{ root: mutation, name: createGroup }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "post:/groups", "operation_id": "createGroup", "method": "POST", "path": "/groups",
        "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/Group", "content_type": "application/json"},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["Group"] = json!({"type": "object", "required": ["displayName"], "properties": {
        "displayName": {"type": "string"},
        "schemas": {"type": "array", "items": {"type": "string", "enum": ["urn:ietf:params:scim:schemas:core:2.0:Group"]}},
        "provider": {"type": "string", "enum": ["amazon-bedrock", "google-cloud-vertex-ai"]}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "post:/groups"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/create_group.json");
    let body = &stub["request"]["bodyPatterns"][0]["equalToJson"];
    assert_eq!(
        body["schemas"],
        json!([
            "urn:ietf:params:scim:schemas:core:2.0:Group",
            "urn:ietf:params:scim:schemas:core:2.0:Group"
        ]),
        "{}",
        stub
    );
    assert_eq!(body["provider"], "amazon-bedrock", "{}", stub);
    // An open string keeps its ordinary placeholder.
    assert_eq!(body["displayName"], "displayName-1", "{}", stub);
}

/// ADR 0051 lever 2c. A body that is one argument passed whole
/// (`body: "$args.body"`, Databricks workspace-conf's string map) was "not
/// flat": its stub asserted nothing, so it and its `_minimal` sibling
/// matched the same request (lint fixture-collision). The argument's value
/// is now the asserted body; a whole-body object gets no unit entry, since
/// rover cannot pass it and `{}` would be false.
#[test]
fn a_whole_body_argument_is_asserted_as_the_body() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "scalar Widget_Co_JSON\n\n",
            "type Mutation {\n",
            "  widget_co_setConf(body: Widget_Co_JSON!): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        PATCH: \"/conf\"\n        body: \"\"\"\n",
            "        $args.body\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"patch:/conf\":\n    include: true\n    graphql: {{ root: mutation, name: setConf }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "patch:/conf", "operation_id": "setConf", "method": "PATCH", "path": "/conf",
        "semantics": "update", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/Conf", "content_type": "application/json", "required": true},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["Conf"] = json!({"type": "object", "additionalProperties": {"type": "string"}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "patch:/conf"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/set_conf.json");
    assert_eq!(
        stub["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"key": "body-1"}),
        "{}",
        stub
    );
    let notes = notes_for(d, "patch:/conf");
    assert!(
        !notes.iter().any(|n| n.contains("not a flat")),
        "{:?}",
        notes
    );
    assert!(
        !d.join("tests/widget-co.connector.yaml").exists()
            || entry_for(d, "Mutation.widget_co_setConf").is_none()
    );
    let required_note = |notes: &[String]| {
        notes
            .iter()
            .any(|n| n.starts_with("a required input-object argument"))
    };
    assert!(required_note(&notes), "{:?}", notes);

    // Optional, it is still the whole body: no unit entry, and the note does
    // not call it a required argument.
    let sdl =
        read(d, "widget-co.graphql").replace("(body: Widget_Co_JSON!)", "(body: Widget_Co_JSON)");
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    let notes = notes_for(d, "patch:/conf");
    assert!(!required_note(&notes), "{:?}", notes);
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("the whole-body argument is an object")),
        "{:?}",
        notes
    );
}

/// `body: "$args.input { wire: field … }"` is the whole body with each field
/// mapped to its wire name. Its `ID` fields are sampled from the wire
/// properties they read: an `ID` over an `integer` is a JSON number in the
/// document and in the stub's body (the router passes the literal through),
/// an `ID` over a string, a string. The mapping used to be "not flat", so the
/// fields had no wire slot and every `ID` was a string the spec rejects.
#[test]
fn an_id_over_an_integer_in_a_whole_body_sub_selection_is_a_number_everywhere() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "input Widget_Co_ItemInput {\n  id: ID\n  label: String!\n  ownerRef: ID!\n  sku: ID!\n}\n\n",
            "type Mutation {\n",
            "  widget_co_addItem(input: Widget_Co_ItemInput!): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        POST: \"/items\"\n        body: \"\"\"\n",
            "        $args.input {\n          id\n          label\n          owner_ref: ownerRef\n          sku\n        }\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/items\":\n    include: true\n    graphql: {{ root: mutation, name: addItem }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "post:/items", "operation_id": "addItem", "method": "POST", "path": "/items",
        "semantics": "create", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/Item", "content_type": "application/json", "required": true},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["Item"] = json!({"type": "object", "required": ["label", "owner_ref", "sku"], "properties": {
        "id": {"type": "integer", "format": "int64"},
        "label": {"type": "string"},
        "owner_ref": {"type": "integer", "format": "int64"},
        "sku": {"type": "string", "format": "uuid"}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    assert_eq!(scaffold(d, &["--op", "post:/items"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/add_item.json");
    let body = &stub["request"]["bodyPatterns"][0]["equalToJson"];
    assert!(body["id"].is_i64(), "an integer id is a number: {}", stub);
    assert!(body["owner_ref"].is_i64(), "{}", stub);
    assert!(
        body["sku"].is_string(),
        "a uuid id stays a string: {}",
        stub
    );
    assert!(body["label"].is_string(), "{}", stub);
    // The document and the stub agree: the same literal on both sides.
    let case = read(d, "tests/cases/add_item.graphql");
    for (gql, wire) in [("id", "id"), ("ownerRef", "owner_ref"), ("sku", "sku")] {
        assert!(
            case.contains(&format!("{}: {}", gql, body[wire])),
            "{} in the document must be the value the stub demands: {}",
            gql,
            case
        );
    }
    let notes = notes_for(d, "post:/items");
    assert!(
        !notes.iter().any(|n| n.contains("not a flat")),
        "{:?}",
        notes
    );
}

/// A DELETE that sends a body is not a write to `body_cannot_conform`, so a
/// required integer body argument stays in its unit entry as the string
/// rover sends. The note had lost its warning that `validate` fails that
/// body (and a comment called the branch unreachable).
#[test]
fn a_required_typed_body_argument_kept_in_a_unit_entry_warns_that_validate_fails() {
    let dir = workspace();
    let d = dir.path();
    let sdl = SDL.replace(
        "type Mutation {\n",
        concat!(
            "type Mutation {\n",
            "  widget_co_retireWidget(id: ID!, reasonCode: Int!): Widget_Co_Widget\n",
            "    @connect(\n      source: \"widget_co\"\n      http: {\n        DELETE: \"/retired/{$args.id}\"\n",
            "        body: \"\"\"\n        reason_code: $args.reasonCode\n        \"\"\"\n      }\n",
            "      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
        ),
    );
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"delete:/retired/{{id}}\":\n    include: true\n    graphql: {{ root: mutation, name: retireWidget }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "delete:/retired/{id}", "operation_id": "retireWidget", "method": "DELETE", "path": "/retired/{id}",
        "semantics": "delete", "provenance": "spec", "confidence": 1,
        "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
        "request_body": {"shape_ref": "#/shapes/Retire", "content_type": "application/json"},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["Retire"] = json!({"type": "object", "required": ["reason_code"], "properties": {
        "reason_code": {"type": "integer"}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    let notes = notes_for(d, "delete:/retired/{id}");
    assert!(
        notes.iter().any(|n| n.contains("stays in the unit entry")
            && n.contains("`graphos-factory-core validate` will fail that request body")),
        "{:?}",
        notes
    );
    assert!(!notes.iter().any(|n| n.contains("waive it")), "{:?}", notes);
}

/// ADR 0051 lever 8. rover drops a list $args and stringifies a scalar, so a
/// unit entry for a write with a required list or integer/number/boolean
/// body argument asserts a request body that `validate` fails. ADR 0014 said
/// to waive it, but a waiver covers only a body the oracle could not judge,
/// never one that fails. No unit entry is written for it now (e2e proves the
/// request), and `--force` removes the scaffold's own stale entry when the
/// new plan writes none.
#[test]
fn a_unit_entry_whose_body_would_fail_validate_is_not_written_and_force_removes_a_stale_one() {
    let dir = workspace();
    let d = dir.path();
    let sdl = |count: &str| {
        SDL.replace(
            "type Mutation {\n",
            &format!(
                "type Mutation {{\n  widget_co_setCount(name: String!, count: {}): Widget_Co_Widget\n    @connect(\n      source: \"widget_co\"\n      http: {{\n        POST: \"/counts\"\n        body: \"\"\"\n        name: $args.name\n        count: $args.count\n        \"\"\"\n      }}\n      selection: \"\"\"\n      id\n      name\n      \"\"\"\n    )\n",
                count
            ),
        )
    };
    std::fs::write(
        d.join(".factory/selection.yaml"),
        format!(
            "{}  \"post:/counts\":\n    include: true\n    graphql: {{ root: mutation, name: setCount }}\n",
            SELECTION
        ),
    )
    .unwrap();
    let mut inv = inventory();
    inv["operations"].as_array_mut().unwrap().push(json!({
        "key": "post:/counts", "operation_id": "setCount", "method": "POST", "path": "/counts",
        "semantics": "update", "provenance": "spec", "confidence": 1, "parameters": [],
        "request_body": {"shape_ref": "#/shapes/SetCount", "content_type": "application/json"},
        "response": {"status": "200", "envelope": null, "shape_ref": "#/shapes/Widget", "list": false},
        "errors": [], "support": "supported", "support_reason": null}));
    inv["shapes"]["SetCount"] = json!({"type": "object", "required": ["name", "count"], "properties": {
        "name": {"type": "string"}, "count": {"type": "integer"}}});
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();

    // Optional count: it is left out of the unit entry, which is written.
    std::fs::write(d.join("widget-co.graphql"), sdl("Int")).unwrap();
    assert_eq!(scaffold(d, &["--op", "post:/counts"]), 0);
    assert!(entry_for(d, "Mutation.widget_co_setCount").is_some());

    // Required count: the entry would assert "count": "2" and fail
    // validate, so none is written, and the stale one goes.
    std::fs::write(d.join("widget-co.graphql"), sdl("Int!")).unwrap();
    let notes = notes_for(d, "post:/counts");
    assert!(
        notes
            .iter()
            .any(|n| n.contains("would fail `graphos-factory-core validate`")),
        "{:?}",
        notes
    );
    assert!(!notes.iter().any(|n| n.contains("waive it")), "{:?}", notes);
    assert_eq!(scaffold(d, &["--op", "post:/counts", "--force"]), 0);
    let suite =
        graphos_factory_core::yaml::parse(&read(d, "tests/widget-co.connector.yaml")).unwrap();
    let left = suite["tests"]
        .as_array()
        .map(|a| {
            a.iter()
                .any(|t| t["target"] == "Mutation.widget_co_setCount")
        })
        .unwrap_or(false);
    assert!(
        !left,
        "the stale scaffold entry is removed: {}",
        read(d, "tests/widget-co.connector.yaml")
    );
    // The stub still asserts the body exactly, typed.
    let stub = read_json(d, "tests/fixtures/mappings/set_count.json");
    assert_eq!(
        stub["request"]["bodyPatterns"][0]["equalToJson"],
        json!({"name": "name-1", "count": 2}),
        "{}",
        stub
    );
}

/// The widget workspace with documented error statuses (ADR 0077): list
/// documents a 404 with a body and a 429 without; version a 500.
fn error_workspace() -> tempfile::TempDir {
    let dir = workspace();
    let mut inv = inventory();
    for op in inv["operations"].as_array_mut().unwrap() {
        match op["key"].as_str().unwrap() {
            "get:/widgets" => {
                op["errors"] = json!([
                    {"status": "404", "shape_ref": "#/shapes/Error"},
                    {"status": "429"}
                ])
            }
            "get:/version" => op["errors"] = json!([{"status": "500"}]),
            _ => {}
        }
    }
    inv["shapes"]["Error"] =
        json!({"type": "object", "properties": {"message": {"type": "string"}}});
    std::fs::write(
        dir.path().join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();
    dir
}

// ── ADR 0080: the success-shape picker (item 6) ─────────────────────────

const SHAPE_SELECTION_BASE: &str = concat!(
    "contract_version: 1\n",
    "operations:\n",
    "  \"get:/status\":\n    include: true\n    graphql: { root: query, name: status }\n",
    "  \"get:/bulk\":\n    include: true\n    graphql: { root: query, name: bulk }\n",
);

const SHAPE_SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(
  name: "widget_co"
  http: { baseURL: "{{BASE_URL}}" }
)

type Widget_Co_Status {
  id: ID
  name: String
}

type Widget_Co_Bulk {
  created: [String]
}

type Query {
  widget_co_status: Widget_Co_Status
    @connect(
      source: "widget_co"
      http: { GET: "/status" }
      selection: """
      id
      name
      """
    )
  widget_co_bulk: Widget_Co_Bulk
    @connect(
      source: "widget_co"
      http: { GET: "/bulk" }
      selection: "created"
    )
}
"#;

/// `get:/status` -- a two-branch success/error union discriminated by a
/// required boolean `ok`, unambiguous by ADR 0080's own rule.
/// `get:/bulk` -- the exact false positive the retired heuristic produced:
/// a legitimate 2xx-only bulk response with an `errors` field, alongside a
/// real error variant, with no status-code or discriminator evidence.
/// `referenced_shape_for_status` controls whether `get:/status`'s response
/// carries the inventory fact (as `inventory build` would write it when
/// unambiguous).
fn shape_inventory(referenced_shape_for_status: bool) -> Value {
    let mut status_response = json!({
        "status": "200", "envelope": null, "content_type": "application/json",
        "shape_ref": "#/shapes/StatusUnion", "list": false
    });
    if referenced_shape_for_status {
        status_response["referenced_shape"] = Value::from("#/shapes/Success");
    }
    json!({"contract_version": 1, "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test/v2"]},
        "operations": [
            {"key": "get:/status", "operation_id": "status", "method": "GET", "path": "/status", "semantics": "read", "provenance": "spec", "confidence": 1,
             "parameters": [], "request_body": null, "response": status_response, "errors": [], "support": "supported", "support_reason": null},
            {"key": "get:/bulk", "operation_id": "bulk", "method": "GET", "path": "/bulk", "semantics": "read", "provenance": "spec", "confidence": 1,
             "parameters": [], "request_body": null,
             "response": {"status": "200", "envelope": null, "content_type": "application/json", "shape_ref": "#/shapes/BulkUnion", "list": false},
             "errors": [], "support": "supported", "support_reason": null}
        ],
        "shapes": {
            "Success": {"type": "object", "required": ["ok", "id"], "properties": {
                "ok": {"type": "boolean", "const": true},
                "id": {"type": "string"}, "name": {"type": "string"}
            }},
            "Failure": {"type": "object", "required": ["ok", "message"], "properties": {
                "ok": {"type": "boolean", "const": false},
                "message": {"type": "string"}
            }},
            "StatusUnion": {"oneOf": [{"$ref": "#/shapes/Success"}, {"$ref": "#/shapes/Failure"}]},
            "BulkResult": {"type": "object", "required": ["created", "errors"], "properties": {
                "created": {"type": "array", "items": {"type": "string"}},
                "errors": {"type": "array", "items": {"type": "object"}}
            }},
            "ErrorEnvelope": {"type": "object", "required": ["message"], "properties": {
                "message": {"type": "string"}
            }},
            "BulkUnion": {"oneOf": [{"$ref": "#/shapes/BulkResult"}, {"$ref": "#/shapes/ErrorEnvelope"}]}
        },
        "unresolved": []})
}

fn shape_workspace(referenced_shape_for_status: bool, selection: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w("widget-co.graphql", SHAPE_SDL);
    w("template.yaml", TEMPLATE);
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", selection);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(&shape_inventory(referenced_shape_for_status)),
    );
    dir
}

#[test]
fn a_status_writes_an_error_case_with_its_directive_and_a_stub_answering_it() {
    let dir = error_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "get:/widgets"]), 0);
    let suite = read(d, "tests/widget-co.connector.yaml");
    assert_eq!(scaffold(d, &["--op", "get:/widgets", "--status", "404"]), 0);

    // The directive is the case's first line, on its own.
    let doc = read(d, "tests/cases/list_widgets_404.graphql");
    assert_eq!(
        doc.lines().next(),
        Some("# expect-upstream-status: 404"),
        "{}",
        doc
    );
    assert!(doc.contains("with the documented error body"), "{}", doc);
    assert!(doc.contains("widget_co_listWidgets("), "{}", doc);

    // The stub answers the status with the documented body.
    let stub = read_json(d, "tests/fixtures/mappings/list_widgets_404.json");
    assert_eq!(stub["response"]["status"], 404);
    assert!(
        stub["response"]["jsonBody"]["message"].is_string(),
        "{}",
        stub
    );
    // Its own argument values, so the success stub cannot answer it.
    let ok = read_json(d, "tests/fixtures/mappings/list_widgets.json");
    assert_ne!(
        stub["request"]["queryParameters"], ok["request"]["queryParameters"],
        "the error case reuses the success case's values"
    );
    // No unit entry: rover cannot set a response status.
    assert_eq!(read(d, "tests/widget-co.connector.yaml"), suite);
}

#[test]
fn all_missing_writes_one_case_per_open_status_then_has_nothing_to_do() {
    let dir = error_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "get:/widgets"]), 0);
    assert_eq!(
        scaffold(d, &["--op", "get:/widgets", "--status", "all-missing"]),
        0
    );
    let s404 = read_json(d, "tests/fixtures/mappings/list_widgets_404.json");
    let s429 = read_json(d, "tests/fixtures/mappings/list_widgets_429.json");
    assert_eq!(s429["response"]["status"], 429);
    assert!(s429["response"].get("jsonBody").is_none(), "{}", s429);
    let doc = read(d, "tests/cases/list_widgets_429.graphql");
    assert!(
        doc.starts_with("# expect-upstream-status: 429\n"),
        "{}",
        doc
    );
    assert!(doc.contains("with no body (none documented)"), "{}", doc);
    // Each status its own values.
    assert_ne!(
        s404["request"]["queryParameters"],
        s429["request"]["queryParameters"]
    );
    // Every documented status now has a case.
    assert_eq!(
        scaffold(d, &["--op", "get:/widgets", "--status", "all-missing"]),
        2
    );
    // An operation that documents none has nothing to do either.
    assert_eq!(
        scaffold(d, &["--op", "post:/widgets", "--status", "all-missing"]),
        2
    );
}

#[test]
fn status_names_exactly_one_operation() {
    let dir = error_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--status", "404"]), 1);
    assert_eq!(
        scaffold(
            d,
            &[
                "--op",
                "get:/widgets",
                "--op",
                "get:/version",
                "--status",
                "404"
            ]
        ),
        1
    );
    assert_eq!(
        scaffold(d, &["--op", "get:/nope", "--status", "all-missing"]),
        1
    );
    assert!(!d.join("tests").exists());
}

#[test]
fn an_error_stub_matching_what_an_existing_stub_matches_is_refused() {
    let dir = error_workspace();
    let d = dir.path();
    // version takes no argument: its error stub could only repeat the
    // success stub's matcher, and WireMock would answer with either.
    assert_eq!(scaffold(d, &["--op", "get:/version"]), 0);
    assert_eq!(scaffold(d, &["--op", "get:/version", "--status", "500"]), 3);
    assert!(!d.join("tests/cases/version_500.graphql").exists());
    assert!(!d.join("tests/fixtures/mappings/version_500.json").exists());
}

fn error_statuses_lines(d: &Path) -> Vec<String> {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["error-statuses", d.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

/// Four tab-separated columns, `-` for an empty one: root field, documented
/// statuses, statuses a recorded run executed, operation key. An operation
/// that documents none is listed too, so e2e.sh can attribute its requests.
#[test]
fn error_statuses_lists_each_operation_with_documented_and_executed_statuses() {
    let dir = error_workspace();
    let d = dir.path();
    assert_eq!(
        error_statuses_lines(d),
        vec![
            "widget_co_createWidget\t-\t-\tpost:/widgets".to_string(),
            "widget_co_listWidgets\t404,429\t-\tget:/widgets".to_string(),
            "widget_co_version\t500\t-\tget:/version".to_string()
        ]
    );
    // The third column is what the last evidence run recorded as executed.
    std::fs::create_dir_all(d.join(".factory/evidence")).unwrap();
    std::fs::write(
        d.join(".factory/evidence/latest.json"),
        json!({"layers": {"wiremock_e2e": {"status": "pass", "error_coverage": {
            "get:/widgets": {"documented": ["404", "429"], "executed": ["404"], "not_run": ["429"]}
        }}}})
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        error_statuses_lines(d)[1],
        "widget_co_listWidgets\t404,429\t404\tget:/widgets"
    );
}

/// A mapping owned by `case` (its `x-cases`), for a request to `path` that
/// answers `status`.
fn own_stub(d: &Path, file: &str, case: &str, path: &str, status: Value) {
    let mut m = read_json(d, "tests/fixtures/mappings/list_widgets.json");
    m["request"] = json!({"method": "GET", "urlPath": path});
    m["response"] = json!({"status": status});
    m["metadata"] = json!({"x-cases": [case]});
    std::fs::write(
        d.join("tests/fixtures/mappings").join(file),
        graphos_factory_core::json::pretty(&m),
    )
    .unwrap();
}

/// ADR 0077: a status counts for an operation only when its own request was
/// answered with it. A nested lookup's 404 in the same case is not the
/// operation's, so the 404 is still owed. (It was credited before.)
#[test]
fn a_status_served_to_a_nested_request_is_not_credited() {
    let dir = error_workspace();
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "get:/widgets"]), 0);
    own_stub(
        d,
        "nested_owner.json",
        "list_widgets",
        "/owners/1",
        json!(404),
    );
    assert_eq!(
        scaffold(d, &["--op", "get:/widgets", "--status", "all-missing"]),
        0
    );
    assert!(
        d.join("tests/cases/list_widgets_404.graphql").exists(),
        "the nested 404 was credited to get:/widgets"
    );
}

/// The positive control: the operation's own request answering 404 does
/// count, whether the stub says `urlPath`, carries a base path in front of
/// the template, or writes the status as a number; only the 429 is owed.
#[test]
fn a_status_served_to_the_operations_own_request_is_credited() {
    for (path, status) in [("/widgets", json!(404)), ("/v2/widgets", json!("404"))] {
        let dir = error_workspace();
        let d = dir.path();
        assert_eq!(scaffold(d, &["--op", "get:/widgets"]), 0);
        own_stub(d, "own_404.json", "list_widgets", path, status);
        assert_eq!(
            scaffold(d, &["--op", "get:/widgets", "--status", "all-missing"]),
            0
        );
        assert!(
            !d.join("tests/cases/list_widgets_404.graphql").exists(),
            "{}: the own 404 was not credited",
            path
        );
        assert!(d.join("tests/cases/list_widgets_429.graphql").exists());
    }
}

/// A status is a string or an unsigned integer; anything else answers
/// nothing and is never stringified into a match.
#[test]
fn a_status_that_is_not_a_string_or_an_unsigned_integer_is_not_credited() {
    for bad in [json!(-1), json!(true), json!({"code": 404})] {
        let dir = error_workspace();
        let d = dir.path();
        assert_eq!(scaffold(d, &["--op", "get:/widgets"]), 0);
        own_stub(d, "odd.json", "list_widgets", "/widgets", bad.clone());
        assert_eq!(
            scaffold(d, &["--op", "get:/widgets", "--status", "all-missing"]),
            0
        );
        assert!(
            d.join("tests/cases/list_widgets_404.graphql").exists(),
            "{} was credited",
            bad
        );
    }
}

/// Two error cases planned in one run whose requests cannot differ would be
/// written side by side, and WireMock would answer both with either. An
/// argumentless call is the plain case; the second is refused (exit 3).
#[test]
fn two_planned_error_cases_with_identical_requests_are_not_both_written() {
    let dir = error_workspace();
    let d = dir.path();
    let mut inv: Value =
        graphos_factory_core::json::parse(&read(d, ".factory/inventory.json")).unwrap();
    for op in inv["operations"].as_array_mut().unwrap() {
        if op["key"] == "get:/version" {
            op["errors"] = json!([{"status": "500"}, {"status": "503"}]);
        }
    }
    std::fs::write(
        d.join(".factory/inventory.json"),
        graphos_factory_core::json::pretty(&inv),
    )
    .unwrap();
    assert_eq!(
        scaffold(d, &["--op", "get:/version", "--status", "all-missing"]),
        3
    );
    let cases = ["version_500", "version_503"];
    let written = cases
        .iter()
        .filter(|c| d.join(format!("tests/cases/{}.graphql", c)).exists())
        .count();
    assert_eq!(written, 1, "both identical requests were written");
}

/// e2e.sh attributes a served status by the journal's method and path: the
/// jq it runs is exercised here on a journal whose nested `/users/` lookup
/// answered 404 beside the operation's own 200 (ADR 0077).
#[test]
fn e2e_sh_credits_only_the_operations_own_request() {
    let script =
        std::fs::read_to_string(Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR")).join("e2e.sh"))
            .unwrap();
    let program = regex::Regex::new(r"(?s)OWN_STATUSES_JQ='(.*?)'\n")
        .unwrap()
        .captures(&script)
        .expect("e2e.sh defines OWN_STATUSES_JQ")[1]
        .to_string();
    let journal = json!({"requests": [
        {"wasMatched": true, "request": {"method": "GET", "url": "/repos/o/r/issues?state=closed"}, "response": {"status": 200}},
        {"wasMatched": true, "request": {"method": "GET", "url": "/users/"}, "response": {"status": 404}},
        {"wasMatched": true, "request": {"method": "GET", "url": "/api/v1/repos/o/r"}, "response": {"status": 404}},
        {"wasMatched": false, "request": {"method": "GET", "url": "/repos/o/r/issues"}, "response": {"status": 500}},
        {"wasMatched": true, "request": {"method": "POST", "url": "/deck/new/"}, "response": {"status": 500}}
    ]});
    let own = |method: &str, template: &str| -> String {
        use std::io::Write;
        let mut child = std::process::Command::new("jq")
            .args(["-r", "--arg", "m", method, "--arg", "t", template, &program])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("jq is required");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(journal.to_string().as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_string()
    };
    // The root operation: its own 200, not the nested 404 (nor an unmatched 500).
    assert_eq!(own("get", "/repos/{owner}/{repo}/issues"), "200");
    // `/users/{username}` needs a non-empty segment: `/users/` is not its request.
    assert_eq!(own("get", "/users/{username}"), "");
    // A base path in front of the template still matches.
    assert_eq!(own("get", "/repos/{owner}/{repo}"), "404");
    // The method is compared ignoring case, and it must be the operation's.
    assert_eq!(own("POST", "/deck/new/"), "500");
    assert_eq!(own("get", "/deck/new/"), "");
}

#[test]
fn an_unambiguous_success_error_union_types_the_success_branch_via_the_inventory_fact() {
    let dir = shape_workspace(true, SHAPE_SELECTION_BASE);
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "get:/status"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/status.json");
    let body = &stub["response"]["jsonBody"];
    assert!(body.get("id").is_some(), "{}", body);
    // The error branch's own field never leaks into the example.
    assert!(body.get("message").is_none(), "{}", body);
    let notes = notes_for(d, "get:/status");
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("two-branch success/error union")),
        "{:?}",
        notes
    );
}

#[test]
fn deepen_reaches_a_field_the_base_example_cuts_inside_the_resolved_success_branch() {
    // A success/error envelope: the response is the two-branch union and the
    // success branch nests arrays deeper than the base example's depth cap,
    // so only deepen can put the selected leaf in the body. It must walk the
    // picked branch, not the union wrapper, which has no properties for
    // prop_shape to look a name up in.
    let dir = shape_workspace(true, SHAPE_SELECTION_BASE);
    let d = dir.path();
    let inv_path = d.join(".factory/inventory.json");
    let mut inv =
        graphos_factory_core::json::parse(&std::fs::read_to_string(&inv_path).unwrap()).unwrap();
    inv["shapes"]["Success"]["properties"]["a"] = json!({"type": "array", "items": {"type": "object", "properties": {
        "b": {"type": "array", "items": {"type": "object", "properties": {
            "c": {"type": "array", "items": {"type": "object", "properties": {
                "leaf": {"type": "string"}}}}}}}}}});
    std::fs::write(&inv_path, graphos_factory_core::json::pretty(&inv)).unwrap();
    let sdl = SHAPE_SDL
        .replace(
            "      id\n      name\n",
            "      id\n      name\n      a { b { c { leaf } } }\n",
        )
        .replace(
            "type Widget_Co_Status {\n  id: ID\n  name: String\n",
            "type Widget_Co_Status {\n  id: ID\n  name: String\n  a: [A]\n",
        )
        + "\ntype A { b: [B] }\ntype B { c: [C] }\ntype C { leaf: String }\n";
    std::fs::write(d.join("widget-co.graphql"), sdl).unwrap();
    assert_eq!(scaffold(d, &["--op", "get:/status"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/status.json");
    let body = &stub["response"]["jsonBody"];
    assert!(body["a"][0]["b"][0]["c"][0]["leaf"].is_string(), "{}", body);
    assert!(body.get("message").is_none(), "{}", body);
}

#[test]
fn an_ambiguous_union_with_no_judgement_falls_back_to_json_and_the_finding_is_open() {
    let dir = shape_workspace(false, SHAPE_SELECTION_BASE);
    let d = dir.path();
    assert_eq!(scaffold(d, &["--op", "get:/bulk"]), 0);
    let stub = read_json(d, "tests/fixtures/mappings/bulk.json");
    assert_eq!(stub["response"]["jsonBody"], json!({}), "{}", stub);
    let notes = notes_for(d, "get:/bulk");
    assert!(
        notes
            .iter()
            .any(|n| n.contains("two-branch success/error union") && n.contains("referenced_shape")),
        "{:?}",
        notes
    );
}

/// The three-part proof (plan item 6, "Done when"): (a) covered above --
/// `inventory build` writes no fact for the ambiguous union; (b) and (c)
/// here -- with no judgement, scaffold falls back to JSON and the finding
/// is open; once a selection judgement picks the branch, scaffold types
/// it, the finding is resolved, and inventory.json never changed at any
/// point -- the branch choice moved downstream handling without moving the
/// source-derived facts.
#[test]
fn a_selection_judgement_resolves_the_ambiguous_union_without_ever_touching_inventory_json() {
    let dir = shape_workspace(false, SHAPE_SELECTION_BASE);
    let d = dir.path();

    assert_eq!(scaffold(d, &["--op", "get:/bulk"]), 0);
    let before_inventory = read(d, ".factory/inventory.json");
    let before_stub = read_json(d, "tests/fixtures/mappings/bulk.json");
    assert_eq!(before_stub["response"]["jsonBody"], json!({}));
    let before_notes = notes_for(d, "get:/bulk");
    assert!(before_notes
        .iter()
        .any(|n| n.contains("two-branch success/error union")));

    // The reviewed judgement: recorded in selection.yaml only.
    let judged_selection = concat!(
        "contract_version: 1\n",
        "operations:\n",
        "  \"get:/status\":\n    include: true\n    graphql: { root: query, name: status }\n",
        "  \"get:/bulk\":\n    include: true\n    graphql: { root: query, name: bulk }\n    response: { envelope: null, referenced_shape: \"#/shapes/BulkResult\" }\n",
    );
    std::fs::write(d.join(".factory/selection.yaml"), judged_selection).unwrap();

    assert_eq!(scaffold(d, &["--op", "get:/bulk", "--force"]), 0);
    let after_inventory = read(d, ".factory/inventory.json");
    assert_eq!(
        before_inventory, after_inventory,
        "a selection judgement must never change inventory.json"
    );
    let after_stub = read_json(d, "tests/fixtures/mappings/bulk.json");
    assert!(
        after_stub["response"]["jsonBody"].get("created").is_some(),
        "{}",
        after_stub
    );
    let after_notes = notes_for(d, "get:/bulk");
    assert!(
        !after_notes
            .iter()
            .any(|n| n.contains("two-branch success/error union")),
        "{:?}",
        after_notes
    );
}

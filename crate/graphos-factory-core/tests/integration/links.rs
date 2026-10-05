//! `links apply --dry-run`: the field-level relationship connector a
//! confirmed `links:` entry asks for, printed and never written (ADR 0069).

use graphos_factory_core::cmd::links::{render_link_field, LinkRefusal};
use graphos_factory_core::reconcile::read_links;
use serde_json::{json, Value};
use std::path::Path;

const WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\n";

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(
  name: "widget_co"
  http: { baseURL: "{{BASE_URL}}", headers: [{ name: "Authorization", value: "Bearer {{AUTH_EXPR}}" }] }
)

type Widget_Co_Widget {
  id: ID!
  name: String
  owner_id: ID
}

type Widget_Co_Owner {
  id: ID!
  login: String
}

type Query {
  widget_co_widget(id: ID!): Widget_Co_Widget
    @connect(source: "widget_co", http: { GET: "/widgets/{$args.id}" }, selection: "id name owner_id")
  widget_co_owner(ownerId: ID!): Widget_Co_Owner
    @connect(source: "widget_co", http: { GET: "/owners/{$args.ownerId}" }, selection: "id login")
}
"#;

/// Two by-id reads: the host (`Widget`, which carries `owner_id`) and the
/// target (`Owner`). The fact on `Widget.owner_id` is what (a)'s pass
/// records for this spec.
fn inventory() -> Value {
    json!({
        "contract_version": 1,
        "api": {"title": "Widget Co", "base_urls": ["https://api.widgets.test"]},
        "operations": [
            {"key": "get:/widgets/{id}", "operation_id": "getWidget", "method": "GET", "path": "/widgets/{id}",
             "semantics": "read", "provenance": "spec", "confidence": 1,
             "parameters": [{"name": "id", "in": "path", "required": true, "type": "string"}],
             "response": {"status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Widget",
                          "root_property_count": 3},
             "support": "supported", "support_reason": null},
            {"key": "get:/owners/{ownerId}", "operation_id": "getOwner", "method": "GET", "path": "/owners/{ownerId}",
             "semantics": "read", "provenance": "spec", "confidence": 1,
             "parameters": [{"name": "ownerId", "in": "path", "required": true, "type": "string"}],
             "response": {"status": "200", "content_type": "application/json", "shape_ref": "#/shapes/Owner",
                          "root_property_count": 2},
             "support": "supported", "support_reason": null}
        ],
        "shapes": {
            "Widget": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"},
                "owner_id": {"type": "string", "candidate_entity_link": {
                    "operation": "get:/owners/{ownerId}", "parameter": "ownerId", "list_context": false}}}},
            "Owner": {"type": "object", "properties": {"id": {"type": "string"}, "login": {"type": "string"}}}
        },
        "unresolved": []
    })
}

const SELECTION: &str = "contract_version: 1\noperations:\n  \"get:/widgets/{id}\":\n    include: true\n    graphql: { root: query, name: widget }\n  \"get:/owners/{ownerId}\":\n    include: true\n    graphql: { root: query, name: owner }\nlinks:\n  - shape: Widget\n    path: owner_id\n    operation: \"get:/owners/{ownerId}\"\n    parameter: ownerId\n    field: owner\n    include: true\n    confirmed: true\n";

/// The Case 2 schema (AppWorld's form, ADR 0069 R1): no credential on the
/// @source; the by-id root field takes `access_token: String!` and sends it
/// as a Bearer header.
const SOURCE_HEADER: &str =
    ", headers: [{ name: \"Authorization\", value: \"Bearer {{AUTH_EXPR}}\" }] }";
const OWNER_ROOT: &str = "  widget_co_owner(ownerId: ID!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\" }";

fn per_call_sdl() -> String {
    for needle in [SOURCE_HEADER, OWNER_ROOT] {
        assert!(SDL.contains(needle), "fixture drifted: {:?}", needle);
    }
    SDL.replacen(SOURCE_HEADER, " }", 1).replacen(
        OWNER_ROOT,
        "  widget_co_owner(ownerId: ID!, access_token: String!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\", headers: [{ name: \"Authorization\", value: \"Bearer {$args.access_token}\" }] }",
        1,
    )
}

/// Case 2 sent as a query parameter, beside a static pair the by-id read
/// needs (kept) and an optional non-credential argument (not the link's).
fn per_call_query_sdl() -> String {
    let per_call = per_call_sdl();
    let query = per_call.replacen(
        "widget_co_owner(ownerId: ID!, access_token: String!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\", headers: [{ name: \"Authorization\", value: \"Bearer {$args.access_token}\" }] }",
        "widget_co_owner(ownerId: ID!, access_token: String!, verbose: Boolean): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}?format=full&access_token={$args.access_token}&verbose={$args.verbose}\" }",
        1,
    );
    assert_ne!(
        query, per_call,
        "the root field's credential must move for this case to mean anything"
    );
    query
}

/// An optional credential (Spotify's form in AppWorld): the by-id root
/// field takes `access_token: String` and sends it through a transform that
/// drops the header when the argument is null. The value is an expression,
/// not a bare `{$args.<a>}`, and is still the credential.
const TRANSFORM_VALUE: &str =
    "{$args.access_token->match([null, null], [@, $(['Bearer', @])->joinNotNull(' ')])}";

fn per_call_transform_sdl() -> String {
    let per_call = per_call_sdl();
    let root = "widget_co_owner(ownerId: ID!, access_token: String!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\", headers: [{ name: \"Authorization\", value: \"Bearer {$args.access_token}\" }] }";
    assert_eq!(per_call.matches(root).count(), 1, "fixture drifted");
    per_call.replacen(
        root,
        &format!(
            "widget_co_owner(ownerId: ID!, access_token: String): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: {{ GET: \"/owners/{{$args.ownerId}}\", headers: [{{ name: \"Authorization\", value: \"{}\" }}] }}",
            TRANSFORM_VALUE
        ),
        1,
    )
}

/// The static part of a by-id read (R51): a header that reads no `$args`
/// and a `queryParams` entry that reads none.
const STATIC_HEADER: &str = "{ name: \"Accept\", value: \"application/vnd.widget.v2+json\" }";
const STATIC_QUERY: &str = "queryParams: \"\"\"format: $(\"full\")\"\"\"";

/// Case 1 with the static settings on the owner root connector.
fn static_settings_sdl() -> String {
    let root = "http: { GET: \"/owners/{$args.ownerId}\" }";
    assert_eq!(SDL.matches(root).count(), 1, "fixture drifted");
    SDL.replacen(
        root,
        &format!(
            "http: {{ GET: \"/owners/{{$args.ownerId}}\", headers: [{}], {} }}",
            STATIC_HEADER, STATIC_QUERY
        ),
        1,
    )
}

/// Case 2 (a per-call Bearer header) with the static settings beside it.
fn per_call_static_sdl() -> String {
    let per_call = per_call_sdl();
    let headers =
        "headers: [{ name: \"Authorization\", value: \"Bearer {$args.access_token}\" }] }";
    assert_eq!(per_call.matches(headers).count(), 1, "fixture drifted");
    per_call.replacen(
        headers,
        &format!(
            "headers: [{}, {{ name: \"Authorization\", value: \"Bearer {{$args.access_token}}\" }}], {} }}",
            STATIC_HEADER, STATIC_QUERY
        ),
        1,
    )
}

/// The owner by-id selection as the printed field carries it: `owner_id: ID`
/// is nullable, so the field gets the null guard (ADR 0084), in the
/// subselection form connect/v0.3 composes.
const GUARDED_ID_LOGIN: &str = "selection: \"\"\"\n      $($this.owner_id ?! 0)->match([null, null], [@, $]) {\n        id login\n      }\n      \"\"\"";
const GUARD_IS_SUCCESS: &str = "isSuccess: \"$($this.owner_id ?! 0)->match([null, true], [@, $status->gte(200)->and($status->lt(300))])\"";

fn workspace(sdl: &str, selection: &str) -> tempfile::TempDir {
    workspace_with(sdl, selection, &inventory())
}

fn workspace_with(sdl: &str, selection: &str, inventory: &Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let w = |rel: &str, text: &str| {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    };
    w("widget-co.graphql", sdl);
    w(".factory/workspace.yaml", WORKSPACE);
    w(".factory/selection.yaml", selection);
    w(
        ".factory/inventory.json",
        &graphos_factory_core::json::pretty(inventory),
    );
    dir
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap()
}

fn apply(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec!["apply".to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::links::main(&argv)
}

/// The binary itself: exit code, stdout, stderr.
fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("links")
        .arg("apply")
        .arg(dir.to_string_lossy().to_string())
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// The binary with `--json`, so the printed text is observed exactly as a
/// machine caller sees it on stdout.
fn apply_json(dir: &Path, args: &[&str]) -> (i32, Value) {
    let mut all: Vec<&str> = args.to_vec();
    all.push("--json");
    let (code, stdout, stderr) = run(dir, &all);
    let json = graphos_factory_core::json::parse(&stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON ({}): {:?}\nstderr: {}", e, stdout, stderr));
    (code, json)
}

fn op(inv: &Value, key: &str) -> Value {
    inv["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == key)
        .cloned()
        .unwrap()
}

fn owner_op(inv: &Value) -> Value {
    op(inv, "get:/owners/{ownerId}")
}

fn first_link(selection: &str) -> graphos_factory_core::reconcile::Link {
    read_links(&graphos_factory_core::yaml::parse(selection).unwrap()).remove(0)
}

/// `text` pasted as the last field of `type <host> { … }`, as the agent
/// pastes it.
fn paste(sdl: &str, host: &str, text: &str) -> String {
    let open = sdl
        .find(&format!("type {} {{", host))
        .unwrap_or_else(|| panic!("no type {}", host));
    let close = open + sdl[open..].find("\n}").unwrap() + 1;
    format!("{}{}{}", &sdl[..close], text, &sdl[close..])
}

#[test]
fn apply_dry_run_prints_a_this_connector_with_no_key_header_or_second_source() {
    let dir = workspace(SDL, SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["file"], "widget-co.graphql");
    let links = report["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{}", report);
    assert_eq!(links[0]["link"], "Widget > owner_id");
    assert_eq!(links[0]["shape"], "Widget");
    assert_eq!(links[0]["path"], "owner_id");
    assert_eq!(links[0]["host"], "Widget_Co_Widget");
    assert_eq!(links[0]["fk_field"], "owner_id");
    assert_eq!(links[0]["field"], "owner");
    assert_eq!(links[0]["operation"], "get:/owners/{ownerId}");
    let text = links[0]["text"].as_str().unwrap();
    assert!(text.contains("  owner: Widget_Co_Owner\n"), "{}", text);
    assert!(text.contains("source: \"widget_co\""), "{}", text);
    assert!(
        text.contains("http: { GET: \"/owners/{$this.owner_id}\" }"),
        "{{$args.ownerId}} becomes {{$this.owner_id}}: {}",
        text
    );
    assert!(
        text.contains(GUARDED_ID_LOGIN),
        "the by-id root field's selection is copied verbatim, under the null guard: {}",
        text
    );
    assert!(text.contains(GUARD_IS_SUCCESS), "{}", text);
    assert!(
        text.contains("through get:/owners/{ownerId}, one request per Widget_Co_Widget that selects it; select this instead of calling Query.widget_co_owner per item"),
        "{}",
        text
    );
    // Case 1 — the by-id root field authenticates through the @source: no
    // entity, no credential of its own, no argument, no second source.
    for forbidden in [
        "@key",
        "headers",
        "Authorization",
        "access_token",
        "$args",
        "@source(",
    ] {
        assert!(
            !text.contains(forbidden),
            "{} must not appear: {}",
            forbidden,
            text
        );
    }
    assert_eq!(report["skipped"], json!([]));
    assert_eq!(report["refused"], json!([]));
    assert_eq!(
        read(dir.path(), "widget-co.graphql"),
        SDL,
        "the schema is never written"
    );
}

/// `Owner.widgets: [Widget_Co_Widget]`, plain or resolved by its own
/// sub-resource connector, with the owner by-id selection as given.
fn owner_widgets_sdl(own_connector: bool, owner_selection: &str) -> String {
    let widgets = if own_connector {
        "  widgets: [Widget_Co_Widget]\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$this.id}/widgets\" }, selection: \"id name owner_id\")\n"
    } else {
        "  widgets: [Widget_Co_Widget]\n"
    };
    let sdl = SDL
        .replacen(
            "  login: String\n}",
            &format!("  login: String\n{}}}", widgets),
            1,
        )
        .replacen(
            "selection: \"id login\"",
            &format!("selection: \"{}\"", owner_selection),
            1,
        );
    assert!(sdl.contains(widgets), "fixture drifted");
    assert!(
        sdl.contains(&format!("selection: \"{}\"", owner_selection)),
        "fixture drifted"
    );
    sdl
}

#[test]
fn a_by_id_selection_that_selects_back_into_the_host_is_refused_as_circular() {
    // The Spotify shape (R52): the owner by-id selection selects `widgets
    // { … }` back into Widget, so the copy on Widget re-enters its own host
    // and composition fails with CIRCULAR_REFERENCE. Measured with rover on
    // a scratch copy: `type Widget_Co_Widget appears more than once in
    // Widget_Co_Widget.owner.widgets` — and the same when `Owner.widgets`
    // carries its own connector: a field the selection selects is walked.
    let inv = inventory();
    for own_connector in [false, true] {
        let sdl = owner_widgets_sdl(own_connector, "id login widgets { id name owner_id }");
        let err = render_link_field(
            &first_link(SELECTION),
            "Widget_Co_Widget",
            "owner_id",
            &owner_op(&inv),
            &sdl,
        )
        .unwrap_err();
        assert_eq!(
            err,
            LinkRefusal::Circular {
                via: vec![
                    "Widget_Co_Owner".to_string(),
                    "Widget_Co_Widget".to_string()
                ]
            },
            "own connector: {}",
            own_connector
        );
        assert_eq!(err.kind(), "circular");
        let dir = workspace(&sdl, SELECTION);
        let (code, report) = apply_json(dir.path(), &["--dry-run"]);
        assert_eq!(code, 1, "{}", report);
        assert_eq!(report["links"], json!([]));
        assert_eq!(report["refused"][0]["link"], "Widget > owner_id");
        assert_eq!(report["refused"][0]["kind"], "circular");
        let detail = report["refused"][0]["detail"].as_str().unwrap();
        assert!(
            detail.contains(
                "selects back into Widget_Co_Widget (Widget_Co_Owner -> Widget_Co_Widget)"
            ),
            "{}",
            detail
        );
        assert!(
            detail.contains("Exclude the field that selects back from the by-id operation (`fields.exclude`, then apply it) or decline the link (`include: false` with a `reason`)"),
            "the advice is the fix the selection walk honours: {}",
            detail
        );
    }
}

#[test]
fn an_existing_field_a_self_link_and_a_missing_or_non_get_root_field_are_refused() {
    let inv = inventory();
    let link = first_link(SELECTION);
    // The host already declares `owner`, as a plain field.
    let sdl = SDL.replace(
        "  owner_id: ID\n}",
        "  owner_id: ID\n  owner: Widget_Co_Owner\n}",
    );
    assert_ne!(sdl, SDL);
    let err = render_link_field(&link, "Widget_Co_Widget", "owner_id", &owner_op(&inv), &sdl)
        .unwrap_err();
    assert_eq!(err, LinkRefusal::FieldExists("owner".to_string()));
    assert_eq!(err.kind(), "field-exists");
    // Widget.id -> get:/widgets/{id}: the by-id operation returns the host.
    let self_selection = SELECTION
        .replace("path: owner_id", "path: id")
        .replace(
            "operation: \"get:/owners/{ownerId}\"",
            "operation: \"get:/widgets/{id}\"",
        )
        .replace("parameter: ownerId", "parameter: id")
        .replace("field: owner", "field: itself");
    let err = render_link_field(
        &first_link(&self_selection),
        "Widget_Co_Widget",
        "id",
        &op(&inv, "get:/widgets/{id}"),
        SDL,
    )
    .unwrap_err();
    assert_eq!(err, LinkRefusal::SelfLink);
    assert_eq!(err.kind(), "self");
    // No root field carries the by-id connector: apply the operation first.
    let without_owner = SDL.replace(
        "  widget_co_owner(ownerId: ID!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\" }, selection: \"id login\")\n",
        "",
    );
    assert_ne!(
        without_owner, SDL,
        "the owner root field must be removed for this case to mean anything"
    );
    let err = render_link_field(
        &link,
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &without_owner,
    )
    .unwrap_err();
    assert_eq!(err.kind(), "no-root-field");
    assert!(
        matches!(err, LinkRefusal::NoRootField(ref d) if d.contains("no Query field carries @connect(GET \"/owners/{ownerId}\")")),
        "{:?}",
        err
    );
    // A non-GET target is refused even when a root field matches its path.
    let mut post_op = owner_op(&inv);
    post_op["method"] = json!("POST");
    post_op["key"] = json!("post:/owners/{ownerId}");
    let err = render_link_field(&link, "Widget_Co_Widget", "owner_id", &post_op, SDL).unwrap_err();
    assert!(
        matches!(err, LinkRefusal::NoRootField(ref d) if d.contains("is not a GET")),
        "{:?}",
        err
    );
    // A by-id root field the field cannot copy: one that sends a credential
    // argument it never declares, one whose path reads no `$args`, and a
    // schema with no @source to reuse.
    let refusal = |sdl: &str| {
        render_link_field(&link, "Widget_Co_Widget", "owner_id", &owner_op(&inv), sdl).unwrap_err()
    };
    let undeclared = SDL.replace(
        "http: { GET: \"/owners/{$args.ownerId}\" }",
        "http: { GET: \"/owners/{$args.ownerId}\", headers: [{ name: \"Authorization\", value: \"Bearer {$args.token}\" }] }",
    );
    assert_ne!(undeclared, SDL);
    assert_eq!(
        refusal(&undeclared),
        LinkRefusal::NoRootField(
            "Query.widget_co_owner sends {$args.token} but declares no argument token; fix the root field first"
                .to_string()
        )
    );
    let no_args = SDL.replace(
        "\"/owners/{$args.ownerId}\"",
        "\"/owners/{$config.ownerId}\"",
    );
    assert_ne!(no_args, SDL);
    assert_eq!(
        refusal(&no_args),
        LinkRefusal::NoRootField(
            "Query.widget_co_owner interpolates 0 $args in its path; a by-id field interpolates exactly one"
                .to_string()
        )
    );
    let no_source = SDL.replace(
        "@source(\n  name: \"widget_co\"\n  http: { baseURL: \"{{BASE_URL}}\", headers: [{ name: \"Authorization\", value: \"Bearer {{AUTH_EXPR}}\" }] }\n)\n",
        "",
    );
    assert_ne!(no_source, SDL);
    assert_eq!(
        refusal(&no_source),
        LinkRefusal::NoRootField("the schema declares no @source to reuse".to_string())
    );
}

#[test]
fn an_unconfirmed_link_is_skipped_and_apply_without_dry_run_prints_nothing() {
    let draft = SELECTION.replace("confirmed: true", "confirmed: false");
    let dir = workspace(SDL, &draft);
    assert_eq!(
        apply(dir.path(), &[]),
        1,
        "without --dry-run the command refuses: it never writes the schema"
    );
    let (code, stdout, stderr) = run(dir.path(), &[]);
    assert_eq!(code, 1);
    assert_eq!(stdout, "", "nothing is printed without --dry-run");
    assert!(stderr.contains("pass --dry-run"), "{}", stderr);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 2, "nothing to do: {}", report);
    assert_eq!(report["links"], json!([]));
    assert_eq!(report["refused"], json!([]));
    assert_eq!(report["skipped"][0]["link"], "Widget > owner_id");
    assert!(
        report["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("confirmed: false"),
        "{}",
        report
    );
    // A declined entry is skipped the same way.
    let declined = SELECTION.replace(
        "include: true\n    confirmed",
        "include: false\n    confirmed",
    );
    assert_ne!(declined, SELECTION);
    let dir2 = workspace(SDL, &declined);
    let (code, report) = apply_json(dir2.path(), &["--dry-run"]);
    assert_eq!(code, 2, "{}", report);
    assert_eq!(report["skipped"][0]["reason"], "declined (include: false)");
    assert_eq!(read(dir.path(), "widget-co.graphql"), SDL);
    // The verb itself: no subcommand is a usage error, --help is not.
    assert_eq!(graphos_factory_core::cmd::links::main(&[]), 1);
    assert_eq!(
        graphos_factory_core::cmd::links::main(&["draft".to_string()]),
        1
    );
    assert_eq!(
        graphos_factory_core::cmd::links::main(&["--help".to_string()]),
        0
    );
    assert_eq!(
        graphos_factory_core::cmd::links::main(&["apply".to_string(), "--help".to_string()]),
        0
    );
}

#[test]
fn the_link_flag_narrows_to_one_entry_and_an_unknown_key_is_refused() {
    // A second entry that is refused; `--link` keeps it out. No fact backs
    // a self-link (the builder never proposes one), so since ADR 0100 it is
    // refused `target-refused`, as lint's `link-target-refused` flags it,
    // before the schema-level `self` check is reached; `self` itself is
    // covered by `render_link_field` above.
    let two = format!(
        "{}  - shape: Widget\n    path: id\n    operation: \"get:/widgets/{{id}}\"\n    parameter: id\n    field: itself\n    include: true\n    confirmed: true\n",
        SELECTION
    );
    let dir = workspace(SDL, &two);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1, "both visited, the self link refused: {}", report);
    assert_eq!(report["links"].as_array().unwrap().len(), 1);
    assert_eq!(report["refused"][0]["link"], "Widget > id");
    assert_eq!(report["refused"][0]["kind"], "target-refused");
    let (code, report) = apply_json(dir.path(), &["--dry-run", "--link", "Widget > owner_id"]);
    assert_eq!(code, 0, "{}", report);
    assert_eq!(report["links"].as_array().unwrap().len(), 1);
    assert_eq!(report["links"][0]["link"], "Widget > owner_id");
    assert_eq!(report["refused"], json!([]));
    let (code, report) = apply_json(dir.path(), &["--dry-run", "--link", "Widget > id"]);
    assert_eq!(code, 1, "{}", report);
    assert_eq!(report["links"], json!([]));
    assert_eq!(report["refused"][0]["kind"], "target-refused");
    assert_eq!(
        apply(dir.path(), &["--dry-run", "--link", "Widget > nope"]),
        1
    );
    assert_eq!(apply(dir.path(), &["--dry-run", "--link", "Widget"]), 1);
}

#[test]
fn a_per_call_credential_on_the_by_id_root_field_is_mirrored_onto_the_printed_field() {
    // Case 2 (ADR 0069 R1): the printed field declares the same argument and
    // sends it the same way — without it every link fails authentication at
    // runtime.
    let per_call = per_call_sdl();
    let dir = workspace(&per_call, SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    let text = report["links"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("  owner(access_token: String!): Widget_Co_Owner\n"),
        "same argument, same type, same nullability: {}",
        text
    );
    assert!(
        text.contains("http: { GET: \"/owners/{$this.owner_id}\", headers: [{ name: \"Authorization\", value: \"Bearer {$args.access_token}\" }] }"),
        "same header, same value template: {}",
        text
    );
    assert!(
        !text.contains("{$args.ownerId}"),
        "the by-id key becomes $this; only the credential is mirrored: {}",
        text
    );
    assert!(!text.contains("@source("), "{}", text);
    assert_eq!(
        read(dir.path(), "widget-co.graphql"),
        per_call,
        "the schema is never written"
    );

    // Sent as a query parameter instead, the credential is mirrored there;
    // the static pair stays and the optional non-credential argument does
    // not come along.
    let query = per_call_query_sdl();
    let inv = inventory();
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &query,
    )
    .unwrap();
    assert!(
        text.contains("  owner(access_token: String!): Widget_Co_Owner\n"),
        "{}",
        text
    );
    assert!(
        text.contains(
            "http: { GET: \"/owners/{$this.owner_id}?format=full&access_token={$args.access_token}\" }"
        ),
        "{}",
        text
    );
    assert!(!text.contains("headers"), "{}", text);
    assert!(!text.contains("verbose"), "{}", text);

    // The same with the non-credential argument sent through an expression:
    // it reads an argument, so it is the root field's own, not a static pair.
    let expression = query.replacen(
        "&verbose={$args.verbose}",
        "&verbose={$args.verbose->match([null, 'false'], [@, 'true'])}",
        1,
    );
    assert_ne!(expression, query, "fixture drifted");
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &expression,
    )
    .unwrap();
    assert!(!text.contains("verbose"), "{}", text);
}

#[test]
fn an_optional_credential_sent_through_a_transform_is_mirrored_verbatim() {
    // The root sends `{$args.access_token->match(…)}`, not a bare
    // `{$args.access_token}`: the printed field still declares the argument
    // as the root does (nullable) and sends the same expression. Printed
    // without it, a private record answers through the link as a
    // token-less read.
    let sdl = per_call_transform_sdl();
    let dir = workspace(&sdl, SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    let text = report["links"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("  owner(access_token: String): Widget_Co_Owner\n"),
        "same argument, same nullability: {}",
        text
    );
    assert!(
        text.contains(&format!(
            "http: {{ GET: \"/owners/{{$this.owner_id}}\", headers: [{{ name: \"Authorization\", value: \"{}\" }}] }}",
            TRANSFORM_VALUE
        )),
        "same header, same expression: {}",
        text
    );
}

#[test]
fn a_query_credential_links_apply_cannot_copy_exactly_is_refused() {
    // The printer rebuilds a queryParams credential as `param={$args.<a>}`
    // and copies a URI credential pair as written. Either would change the
    // request here, so it refuses rather than print a field that differs
    // from the root on the wire or sends an argument it does not declare.
    let per_call = per_call_sdl();
    let root = "widget_co_owner(ownerId: ID!, access_token: String!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\", headers: [{ name: \"Authorization\", value: \"Bearer {$args.access_token}\" }] }";
    assert_eq!(per_call.matches(root).count(), 1, "fixture drifted");
    let inv = inventory();
    for (case, replacement, says) in [
        (
            "queryParams expression",
            "widget_co_owner(ownerId: ID!, access_token: String): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\", queryParams: \"access_token: $args.access_token->match([null, null], [@, @])\" }",
            "through a queryParams expression",
        ),
        (
            "queryParams deeper path",
            "widget_co_owner(ownerId: ID!, access_token: String!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\", queryParams: \"access_token: $args.access_token.value\" }",
            "through a queryParams expression",
        ),
        (
            "URI pair reading a second argument",
            "widget_co_owner(ownerId: ID!, api_token: String!, region: String): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}?sig={$(['x', $args.api_token, $args.region])->joinNotNull('-')}\" }",
            "together with {$args.region}, which is no credential",
        ),
    ] {
        let sdl = per_call.replacen(root, replacement, 1);
        match render_link_field(
            &first_link(SELECTION),
            "Widget_Co_Widget",
            "owner_id",
            &owner_op(&inv),
            &sdl,
        ) {
            Err(LinkRefusal::NoRootField(detail)) => {
                assert!(detail.contains(says), "{}: {}", case, detail)
            }
            other => panic!("{}: {:?}", case, other),
        }
    }
    // A queryParams credential that is exactly `$args.<a>` still prints.
    let bare = per_call.replacen(
        root,
        "widget_co_owner(ownerId: ID!, access_token: String!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\", queryParams: \"access_token: $args.access_token\" }",
        1,
    );
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &bare,
    )
    .unwrap();
    assert!(
        text.contains("GET: \"/owners/{$this.owner_id}?access_token={$args.access_token}\""),
        "{}",
        text
    );
}

#[test]
fn the_printed_field_is_what_lint_and_reconcile_accept() {
    // Pasted as printed, each form draws no link-credential and reconcile
    // counts the link as applied: the printer and the rules agree (R1).
    for (case, sdl) in [
        ("source-level", SDL.to_string()),
        ("per-call header", per_call_sdl()),
        ("per-call query", per_call_query_sdl()),
        ("static settings", static_settings_sdl()),
        ("per-call header + static settings", per_call_static_sdl()),
        ("per-call optional transform", per_call_transform_sdl()),
    ] {
        let dir = workspace(&sdl, SELECTION);
        let (code, report) = apply_json(dir.path(), &["--dry-run"]);
        assert_eq!(code, 0, "{}: {}", case, report);
        let text = report["links"][0]["text"].as_str().unwrap();
        let pasted = paste(&sdl, "Widget_Co_Widget", text);
        std::fs::write(dir.path().join("widget-co.graphql"), &pasted).unwrap();
        let lint = graphos_factory_core::lint::lint_workspace(
            dir.path(),
            &graphos_factory_core::lint::LintOptions {
                schemas_dir: None,
                skip_evidence: true,
                target: &graphos_factory_core::target::BARE,
            },
        );
        // `link-untested` is about the workspace's tests, which this one
        // has none of, not about the printed text (its own test covers it).
        let link_rules: Vec<_> = lint
            .findings
            .iter()
            .filter(|f| f.rule.starts_with("link-") && f.rule != "link-untested")
            .collect();
        assert!(
            link_rules.is_empty(),
            "{}: {:?}\n{}",
            case,
            link_rules,
            pasted
        );
        let reconciled =
            graphos_factory_core::reconcile::reconcile_workspace(dir.path(), None).unwrap();
        let links = &reconciled["links"];
        assert_eq!(links["add"], json!([]), "{}: {}", case, links);
        assert_eq!(links["remove"], json!([]), "{}: {}", case, links);
        assert_eq!(links["change"], json!([]), "{}: {}", case, links);
        assert_eq!(
            links["unchanged"].as_array().map(Vec::len),
            Some(1),
            "{}: {}",
            case,
            links
        );
        assert_eq!(reconciled["selection_errors"], json!([]), "{}", case);
        // Run again on the pasted schema: already applied, nothing to do.
        let (code, report) = apply_json(dir.path(), &["--dry-run"]);
        assert_eq!(code, 2, "{}: {}", case, report);
        assert_eq!(report["links"], json!([]), "{}", case);
        assert_eq!(report["refused"], json!([]), "{}", case);
        assert!(
            report["skipped"][0]["reason"]
                .as_str()
                .unwrap()
                .starts_with("already applied: Widget_Co_Widget.owner"),
            "{}: {}",
            case,
            report
        );
    }
}

#[test]
fn a_link_with_no_host_or_a_broken_reference_is_refused_with_its_kind() {
    // No root field returns Widget, so no host type can be derived (R42).
    let no_widget_root = SDL.replace(
        "  widget_co_widget(id: ID!): Widget_Co_Widget\n    @connect(source: \"widget_co\", http: { GET: \"/widgets/{$args.id}\" }, selection: \"id name owner_id\")\n",
        "",
    );
    assert_ne!(no_widget_root, SDL);
    let dir = workspace(&no_widget_root, SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1, "{}", report);
    assert_eq!(report["links"], json!([]));
    assert_eq!(report["skipped"], json!([]));
    assert_eq!(report["refused"][0]["kind"], "no-host", "{}", report);
    assert!(
        report["refused"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("no included operation's root field returns a type for shape Widget"),
        "{}",
        report
    );
    // Each reference problem `link_reference_problems` finds is a refusal
    // with the kind it amounts to, never a silent skip.
    let cases: [(String, &str, &str); 4] = [
        (
            SELECTION.replace("shape: Widget", "shape: Gadget"),
            "no-host",
            "shape Gadget is not in inventory.json",
        ),
        (
            SELECTION.replace("path: owner_id", "path: maker_id"),
            "no-host",
            "path maker_id does not resolve in shape Widget",
        ),
        (
            SELECTION.replace(
                "operation: \"get:/owners/{ownerId}\"",
                "operation: \"get:/makers/{ownerId}\"",
            ),
            "no-root-field",
            "get:/makers/{ownerId} is not in inventory.json",
        ),
        (
            SELECTION.replace(
                "\"get:/owners/{ownerId}\":\n    include: true",
                "\"get:/owners/{ownerId}\":\n    include: false",
            ),
            "no-root-field",
            "get:/owners/{ownerId} is not included by the selection",
        ),
    ];
    for (selection, kind, detail) in cases {
        assert_ne!(selection, SELECTION, "{}", detail);
        let dir = workspace(SDL, &selection);
        let (code, report) = apply_json(dir.path(), &["--dry-run"]);
        assert_eq!(code, 1, "{}: {}", detail, report);
        assert_eq!(report["links"], json!([]), "{}", detail);
        assert_eq!(report["refused"][0]["kind"], kind, "{}: {}", detail, report);
        assert!(
            report["refused"][0]["detail"]
                .as_str()
                .unwrap()
                .contains(detail),
            "{}: {}",
            detail,
            report
        );
    }
}

#[test]
fn a_tie_between_by_id_root_fields_is_settled_by_the_selection_or_refused() {
    // Two Query fields reach GET /owners/{…}; the first declared returns a
    // different type. The selection's graphql name for the link's operation
    // (ADR 0044) says which one the link copies.
    let profile = "  widget_co_ownerProfile(ownerId: ID!): Widget_Co_Profile\n    @connect(source: \"widget_co\", http: { GET: \"/owners/{$args.ownerId}\" }, selection: \"id bio\")\n";
    let sdl = SDL.replace(
        "type Query {\n",
        &format!(
            "type Widget_Co_Profile {{\n  id: ID!\n  bio: String\n}}\n\ntype Query {{\n{}",
            profile
        ),
    );
    assert!(sdl.contains(profile));
    let inv = inventory();
    let err = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &sdl,
    )
    .unwrap_err();
    assert_eq!(err.kind(), "no-root-field");
    assert!(
        matches!(err, LinkRefusal::NoRootField(ref d)
            if d.contains("2 Query fields reach GET /owners/{ownerId} (widget_co_ownerProfile, widget_co_owner)")),
        "{:?}",
        err
    );
    let dir = workspace(&sdl, SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    let text = report["links"][0]["text"].as_str().unwrap();
    assert!(text.contains("  owner: Widget_Co_Owner\n"), "{}", text);
    assert!(text.contains(GUARDED_ID_LOGIN), "{}", text);
    assert!(
        text.contains("calling Query.widget_co_owner per item"),
        "{}",
        text
    );
}

#[test]
fn the_text_report_prints_the_field_on_stdout_and_every_skip_on_stderr() {
    let dir = workspace(SDL, SELECTION);
    let (code, stdout, stderr) = run(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", stderr);
    assert!(
        stdout.starts_with("# Widget_Co_Widget.owner — paste into `type Widget_Co_Widget { … }` in widget-co.graphql; links entry \"Widget > owner_id\" via get:/owners/{ownerId}\n  \"\"\"\n"),
        "{}",
        stdout
    );
    assert!(stdout.contains("  owner: Widget_Co_Owner\n"), "{}", stdout);
    assert_eq!(stderr, "", "a clean print says nothing on stderr");
    let (_, report) = apply_json(dir.path(), &["--dry-run"]);
    assert!(
        stdout.contains(report["links"][0]["text"].as_str().unwrap()),
        "the text and --json print the same field"
    );
    let draft = SELECTION.replace("confirmed: true", "confirmed: false");
    let dir = workspace(SDL, &draft);
    let (code, stdout, stderr) = run(dir.path(), &["--dry-run"]);
    assert_eq!(code, 2);
    assert_eq!(stdout, "");
    assert!(
        stderr.contains(
            "links apply: skipped Widget > owner_id: still the tool's draft (confirmed: false)"
        ),
        "{}",
        stderr
    );
    assert!(
        stderr.contains("links apply: nothing to do — no confirmed, included links entry"),
        "{}",
        stderr
    );
    let refused = SDL.replace(
        "  owner_id: ID\n}",
        "  owner_id: ID\n  owner: Widget_Co_Owner\n}",
    );
    let dir = workspace(&refused, SELECTION);
    let (code, stdout, stderr) = run(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1);
    assert_eq!(stdout, "");
    assert!(
        stderr.contains("links apply: refused Widget > owner_id: field-exists — the host type already declares `owner`"),
        "{}",
        stderr
    );
}

#[test]
fn two_links_that_land_on_one_host_field_print_it_once() {
    // `Featured` is a second shape the schema gives the same GraphQL type
    // (`get:/featured` returns it as Widget_Co_Widget), and it carries the
    // same foreign key: both links resolve to Widget_Co_Widget.owner.
    let mut inv = inventory();
    let ops = inv["operations"].as_array_mut().unwrap();
    let mut featured = ops[0].clone();
    featured["key"] = json!("get:/featured");
    featured["operation_id"] = json!("getFeatured");
    featured["path"] = json!("/featured");
    featured["parameters"] = json!([]);
    featured["response"]["shape_ref"] = json!("#/shapes/Featured");
    let mut user = ops[1].clone();
    user["key"] = json!("get:/users/{userId}");
    user["operation_id"] = json!("getUser");
    user["path"] = json!("/users/{userId}");
    user["parameters"][0]["name"] = json!("userId");
    ops.push(featured);
    ops.push(user);
    inv["shapes"]["Featured"] = inv["shapes"]["Widget"].clone();
    let sdl = SDL.replace(
        "type Query {\n",
        "type Query {\n  widget_co_featured: Widget_Co_Widget\n    @connect(source: \"widget_co\", http: { GET: \"/featured\" }, selection: \"id name owner_id\")\n  widget_co_user(userId: ID!): Widget_Co_Owner\n    @connect(source: \"widget_co\", http: { GET: \"/users/{$args.userId}\" }, selection: \"id login\")\n",
    );
    assert_ne!(sdl, SDL);
    let operations = SELECTION.replace(
        "links:\n",
        "  \"get:/featured\":\n    include: true\n    graphql: { root: query, name: featured }\n  \"get:/users/{userId}\":\n    include: true\n    graphql: { root: query, name: user }\nlinks:\n",
    );
    let second = |operation: &str, parameter: &str| {
        format!(
            "{}  - shape: Featured\n    path: owner_id\n    operation: \"{}\"\n    parameter: {}\n    field: owner\n    include: true\n    confirmed: true\n",
            operations, operation, parameter
        )
    };
    let dir = workspace_with(&sdl, &second("get:/owners/{ownerId}", "ownerId"), &inv);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    let links = report["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "one field, printed once: {}", report);
    assert_eq!(links[0]["link"], "Widget > owner_id");
    assert_eq!(links[0]["host"], "Widget_Co_Widget");
    assert_eq!(report["skipped"][0]["link"], "Featured > owner_id");
    assert_eq!(
        report["skipped"][0]["reason"],
        "printed once: Widget_Co_Widget.owner is the field links entry \"Widget > owner_id\" prints, and it serves this entry too"
    );
    assert_eq!(report["refused"], json!([]));
    // The same field through another operation is a collision to settle
    // with `field:`, not a second copy. The fact on `Featured.owner_id`
    // points at that operation, so the entry is not stale (ADR 0100).
    let mut inv = inv;
    inv["shapes"]["Featured"]["properties"]["owner_id"]["candidate_entity_link"] = json!({
        "operation": "get:/users/{userId}", "parameter": "userId", "list_context": false});
    let dir = workspace_with(&sdl, &second("get:/users/{userId}", "userId"), &inv);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1, "{}", report);
    assert_eq!(report["links"].as_array().unwrap().len(), 1, "{}", report);
    assert_eq!(report["refused"][0]["link"], "Featured > owner_id");
    assert_eq!(report["refused"][0]["kind"], "field-exists");
    assert!(
        report["refused"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("links entry \"Widget > owner_id\" already prints `owner` on Widget_Co_Widget through get:/owners/{ownerId}"),
        "{}",
        report
    );
}

#[test]
fn a_host_that_declares_no_foreign_key_field_is_refused_and_nothing_is_printed() {
    // R50: the widget root field's selection leaves `owner_id` out, so the
    // host type never declares it; a printed `{$this.owner_id}` would read a
    // field that is not there, and only compose would say so.
    let sdl = SDL
        .replace("  owner_id: ID\n}", "}")
        .replace("selection: \"id name owner_id\"", "selection: \"id name\"");
    assert!(
        !sdl.contains("owner_id"),
        "the fixture must drop every owner_id"
    );
    let inv = inventory();
    let err = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &sdl,
    )
    .unwrap_err();
    assert_eq!(
        err,
        LinkRefusal::NoFkField {
            host: "Widget_Co_Widget".to_string(),
            link: "Widget > owner_id".to_string()
        }
    );
    assert_eq!(err.kind(), "no-fk-field");
    let dir = workspace(&sdl, SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1, "{}", report);
    assert_eq!(report["links"], json!([]), "nothing is printed: {}", report);
    assert_eq!(report["skipped"], json!([]));
    assert_eq!(report["refused"][0]["link"], "Widget > owner_id");
    assert_eq!(report["refused"][0]["kind"], "no-fk-field");
    assert_eq!(
        report["refused"][0]["detail"],
        "no-fk-field — Widget_Co_Widget declares no field for Widget > owner_id; include the property in the selection and apply it before the link"
    );
    let (code, stdout, stderr) = run(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1);
    assert_eq!(stdout, "", "nothing is printed");
    assert!(
        stderr.contains("links apply: refused Widget > owner_id: no-fk-field — "),
        "{}",
        stderr
    );
}

#[test]
fn one_field_name_for_two_foreign_keys_on_one_host_is_refused_naming_both() {
    // `Featured` is a second shape the schema gives Widget_Co_Widget
    // (get:/featured). Its `owner_id` is Widget's relationship again — one
    // field, printed once — but its `maker_id` is another relationship the
    // entry also names `owner`: folded into the first, it would vanish.
    let mut inv = inventory();
    let ops = inv["operations"].as_array_mut().unwrap();
    let mut featured = ops[0].clone();
    featured["key"] = json!("get:/featured");
    featured["operation_id"] = json!("getFeatured");
    featured["path"] = json!("/featured");
    featured["parameters"] = json!([]);
    featured["response"]["shape_ref"] = json!("#/shapes/Featured");
    ops.push(featured);
    let fact = inv["shapes"]["Widget"]["properties"]["owner_id"].clone();
    inv["shapes"]["Featured"] = inv["shapes"]["Widget"].clone();
    inv["shapes"]["Featured"]["properties"]["maker_id"] = fact;
    let sdl = SDL
        .replacen("  owner_id: ID\n}", "  owner_id: ID\n  maker_id: ID\n}", 1)
        .replacen(
            "selection: \"id name owner_id\"",
            "selection: \"id name owner_id maker_id\"",
            1,
        )
        .replacen(
            "type Query {\n",
            "type Query {\n  widget_co_featured: Widget_Co_Widget\n    @connect(source: \"widget_co\", http: { GET: \"/featured\" }, selection: \"id name owner_id maker_id\")\n",
            1,
        );
    assert!(sdl.contains("widget_co_featured") && sdl.contains("  maker_id: ID\n"));
    let entry = |path: &str, field: &str| {
        format!(
            "  - shape: Featured\n    path: {}\n    operation: \"get:/owners/{{ownerId}}\"\n    parameter: ownerId\n    field: {}\n    include: true\n    confirmed: true\n",
            path, field
        )
    };
    let selection = |maker_field: &str| {
        format!(
            "{}{}{}",
            SELECTION.replace(
                "links:\n",
                "  \"get:/featured\":\n    include: true\n    graphql: { root: query, name: featured }\nlinks:\n",
            ),
            entry("owner_id", "owner"),
            entry("maker_id", maker_field)
        )
    };
    let dir = workspace_with(&sdl, &selection("owner"), &inv);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1, "{}", report);
    let links = report["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{}", report);
    assert_eq!(links[0]["link"], "Widget > owner_id");
    assert_eq!(links[0]["fk_field"], "owner_id");
    // The same relationship (one operation, one fk) is printed once.
    assert_eq!(
        report["skipped"],
        json!([{"link": "Featured > owner_id", "reason": "printed once: Widget_Co_Widget.owner is the field links entry \"Widget > owner_id\" prints, and it serves this entry too"}])
    );
    // Another fk under the same name is refused, naming both fks.
    assert_eq!(report["refused"].as_array().unwrap().len(), 1, "{}", report);
    assert_eq!(report["refused"][0]["link"], "Featured > maker_id");
    assert_eq!(report["refused"][0]["kind"], "field-exists");
    assert_eq!(
        report["refused"][0]["detail"],
        "field-exists — links entry \"Widget > owner_id\" already prints `owner` on Widget_Co_Widget through get:/owners/{ownerId} keyed by `owner_id`, and this entry would print it through get:/owners/{ownerId} keyed by `maker_id`; set a distinct field: on one of the two entries (another camelCase name)"
    );
    // The fix the refusal names: a distinct `field:` prints both.
    let dir = workspace_with(&sdl, &selection("maker"), &inv);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    let links = report["links"].as_array().unwrap();
    assert_eq!(links.len(), 2, "{}", report);
    assert_eq!(links[1]["link"], "Featured > maker_id");
    assert_eq!(links[1]["field"], "maker");
    assert!(
        links[1]["text"]
            .as_str()
            .unwrap()
            .contains("http: { GET: \"/owners/{$this.maker_id}\" }"),
        "{}",
        links[1]["text"]
    );
}

#[test]
fn the_cycle_check_walks_the_by_id_selection_so_no_paste_changes_the_verdict() {
    // R52. Two links in opposite directions — Widget.owner and
    // Owner.favoriteWidget — are not circular: neither by-id selection
    // selects the other's field. Whichever is pasted first, the other still
    // prints (measured with rover: both pasted, compose passes).
    let mut inv = inventory();
    inv["shapes"]["Owner"]["properties"]["favorite_widget_id"] = json!({
        "type": "string",
        "candidate_entity_link": {"operation": "get:/widgets/{id}", "parameter": "id", "list_context": false}});
    let sdl = SDL
        .replacen(
            "  login: String\n}",
            "  login: String\n  favorite_widget_id: ID\n}",
            1,
        )
        .replacen(
            "selection: \"id login\"",
            "selection: \"id login favorite_widget_id\"",
            1,
        );
    assert!(sdl.contains("selection: \"id login favorite_widget_id\""));
    let selection = format!(
        "{}  - shape: Owner\n    path: favorite_widget_id\n    operation: \"get:/widgets/{{id}}\"\n    parameter: id\n    field: favoriteWidget\n    include: true\n    confirmed: true\n",
        SELECTION
    );
    let dir = workspace_with(&sdl, &selection, &inv);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    let links = report["links"].as_array().unwrap();
    assert_eq!(links.len(), 2, "{}", report);
    assert_eq!(links[0]["link"], "Widget > owner_id");
    assert_eq!(links[1]["link"], "Owner > favorite_widget_id");
    assert_eq!(links[1]["host"], "Widget_Co_Owner");
    let owner = links[0]["text"].as_str().unwrap().to_string();
    let favorite = links[1]["text"].as_str().unwrap().to_string();
    for (host, text, other) in [
        ("Widget_Co_Widget", &owner, "Owner > favorite_widget_id"),
        ("Widget_Co_Owner", &favorite, "Widget > owner_id"),
    ] {
        let dir = workspace_with(&paste(&sdl, host, text), &selection, &inv);
        let (code, report) = apply_json(dir.path(), &["--dry-run"]);
        assert_eq!(code, 0, "{} pasted first: {}", host, report);
        assert_eq!(report["refused"], json!([]), "{} pasted first", host);
        assert_eq!(
            report["links"].as_array().unwrap().len(),
            1,
            "{}: {}",
            host,
            report
        );
        assert_eq!(report["links"][0]["link"], other);
        assert!(
            report["skipped"][0]["reason"]
                .as_str()
                .unwrap()
                .starts_with("already applied: "),
            "{}",
            report
        );
    }
    // A back-reference the by-id selection does not select never makes a
    // link circular — plain, or resolved by its own sub-resource connector
    // (rover composes the latter pasted).
    let inv = inventory();
    for own_connector in [false, true] {
        let sdl = owner_widgets_sdl(own_connector, "id login");
        let text = render_link_field(
            &first_link(SELECTION),
            "Widget_Co_Widget",
            "owner_id",
            &owner_op(&inv),
            &sdl,
        )
        .unwrap_or_else(|e| panic!("own connector {}: {}", own_connector, e));
        assert!(text.contains(GUARDED_ID_LOGIN), "{}", text);
    }
}

#[test]
fn the_root_connectors_static_headers_and_query_params_come_along() {
    // R51: both reads must fetch the same representation, so the printed
    // field sends the by-id root connector's static settings too; the
    // credential is mirrored as before, after them.
    let inv = inventory();
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &static_settings_sdl(),
    )
    .unwrap();
    assert!(
        text.contains(&format!(
            "http: {{ GET: \"/owners/{{$this.owner_id}}\", headers: [{}], {} }}",
            STATIC_HEADER, STATIC_QUERY
        )),
        "{}",
        text
    );
    assert!(!text.contains("Authorization"), "Case 1 still: {}", text);
    assert!(!text.contains("$args"), "{}", text);
    assert!(text.contains("  owner: Widget_Co_Owner\n"), "{}", text);
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &per_call_static_sdl(),
    )
    .unwrap();
    assert!(
        text.contains(&format!(
            "http: {{ GET: \"/owners/{{$this.owner_id}}\", headers: [{}, {{ name: \"Authorization\", value: \"Bearer {{$args.access_token}}\" }}], {} }}",
            STATIC_HEADER, STATIC_QUERY
        )),
        "{}",
        text
    );
    assert!(
        text.contains("  owner(access_token: String!): Widget_Co_Owner\n"),
        "{}",
        text
    );
}

#[test]
fn the_source_name_is_the_directives_own_argument_and_reference_refusals_name_their_fix() {
    use graphos_factory_core::cmd::links::source_name;
    // `http:` first: its header's `name: "Authorization"` precedes the
    // directive's own `name:`.
    let source = "@source(\n  name: \"widget_co\"\n  http: { baseURL: \"{{BASE_URL}}\", headers: [{ name: \"Authorization\", value: \"Bearer {{AUTH_EXPR}}\" }] }\n)";
    let reordered = "@source(\n  http: { baseURL: \"{{BASE_URL}}\", headers: [{ name: \"Authorization\", value: \"Bearer {{AUTH_EXPR}}\" }] }\n  name: \"widget_co\"\n)";
    assert!(SDL.contains(source), "fixture drifted");
    let sdl = SDL.replacen(source, reordered, 1);
    assert_eq!(source_name(SDL).as_deref(), Some("widget_co"));
    assert_eq!(source_name(&sdl).as_deref(), Some("widget_co"));
    let inv = inventory();
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inv),
        &sdl,
    )
    .unwrap();
    assert!(text.contains("source: \"widget_co\""), "{}", text);
    // `self` and `no-host` say what to change.
    assert_eq!(
        LinkRefusal::SelfLink.to_string(),
        "self — the by-id operation returns the host type itself, so the field would re-read the record it sits on: decline it (`include: false`), or give the summary its own type when the detail carries fields the summary lacks"
    );
    for (selection, detail) in [
        (
            SELECTION.replace("shape: Widget", "shape: Gadget"),
            "no-host — shape Gadget is not in inventory.json; fix the entry's `shape`",
        ),
        (
            SELECTION.replace("path: owner_id", "path: maker_id"),
            "no-host — path maker_id does not resolve in shape Widget; fix the entry's `path`",
        ),
    ] {
        let dir = workspace(SDL, &selection);
        let (code, report) = apply_json(dir.path(), &["--dry-run"]);
        assert_eq!(code, 1, "{}", report);
        assert_eq!(report["refused"][0]["detail"], detail, "{}", report);
    }
}

/// The fixture schema moved to another connect version.
fn with_connect(sdl: &str, version: &str) -> String {
    let link = "https://specs.apollo.dev/connect/v0.3";
    assert_eq!(sdl.matches(link).count(), 1, "fixture drifted");
    sdl.replacen(
        link,
        &format!("https://specs.apollo.dev/connect/{}", version),
        1,
    )
}

fn lint_rules(dir: &Path, rule: &str) -> Vec<graphos_factory_core::lint::Finding> {
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
    .filter(|f| f.rule == rule)
    .collect()
}

#[test]
fn a_nullable_foreign_key_gets_the_null_guard_and_a_non_null_one_does_not() {
    // ADR 0084. The router cannot skip a field-level connector: for a parent
    // whose fk is null it sends `GET /owners/` (an empty final segment), and
    // AppWorld answers 307 — CONNECTOR_FETCH on every null parent (measured
    // on spotify Spotify_QueueSong.album, 6 of 7 parents). The guard makes
    // any answer a success and maps it to null when `$this.owner_id` is null.
    let inv = inventory();
    let render = |sdl: &str| {
        render_link_field(
            &first_link(SELECTION),
            "Widget_Co_Widget",
            "owner_id",
            &owner_op(&inv),
            sdl,
        )
    };
    // connect/v0.3: the subselection form.
    let text = render(SDL).unwrap();
    assert!(text.contains(GUARD_IS_SUCCESS), "{}", text);
    assert!(text.contains(GUARDED_ID_LOGIN), "{}", text);
    // connect/v0.4: the arm form — composition rejects the subselection
    // form's nested objects there.
    let v04 = with_connect(SDL, "v0.4");
    let text = render(&v04).unwrap();
    assert!(text.contains(GUARD_IS_SUCCESS), "{}", text);
    assert!(
        text.contains("selection: \"\"\"\n      $($this.owner_id ?! 0)->match([null, null], [@, $ {\n        id login\n      }])\n      \"\"\""),
        "{}",
        text
    );
    // A multi-line selection keeps its own nesting, two columns in.
    let nested = v04.replacen(
        "selection: \"id login\"",
        "selection: \"\"\"\n    id\n    login\n    team {\n      name\n    }\n    \"\"\"",
        1,
    );
    let nested = nested.replacen(
        "  login: String\n}",
        "  login: String\n  team: Widget_Co_Team\n}\n\ntype Widget_Co_Team {\n  name: String\n}",
        1,
    );
    let text = render(&nested).unwrap();
    assert!(
        text.contains("[@, $ {\n        id\n        login\n        team {\n          name\n        }\n      }])"),
        "{}",
        text
    );
    // `owner_id: ID!`: every parent carries one, so nothing changes.
    let non_null = SDL.replacen("  owner_id: ID\n", "  owner_id: ID!\n", 1);
    assert_ne!(non_null, SDL, "fixture drifted");
    let text = render(&non_null).unwrap();
    assert!(text.contains("selection: \"\"\"id login\"\"\""), "{}", text);
    assert!(!text.contains("isSuccess"), "{}", text);
    assert!(!text.contains("?!"), "{}", text);
}

#[test]
fn the_guard_keeps_the_sources_is_success_for_a_parent_whose_fk_is_set() {
    let source = "@source(\n  name: \"widget_co\"\n";
    assert!(SDL.contains(source), "fixture drifted");
    let sdl = SDL.replacen(
        source,
        "@source(\n  name: \"widget_co\"\n  isSuccess: \"$status->eq(200)\"\n",
        1,
    );
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inventory()),
        &sdl,
    )
    .unwrap();
    assert!(
        text.contains(
            "isSuccess: \"$($this.owner_id ?! 0)->match([null, true], [@, $status->eq(200)])\""
        ),
        "{}",
        text
    );
    // A block string is folded onto the guard's one line, not read as `""`.
    let block = SDL.replacen(
        source,
        "@source(\n  name: \"widget_co\"\n  isSuccess: \"\"\"\n    $status->eq(200)\n  \"\"\"\n",
        1,
    );
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inventory()),
        &block,
    )
    .unwrap();
    assert!(
        text.contains(
            "isSuccess: \"$($this.owner_id ?! 0)->match([null, true], [@, $status->eq(200)])\""
        ),
        "{}",
        text
    );
}

#[test]
fn a_nullable_foreign_key_the_guard_cannot_hold_for_is_refused_with_the_failure() {
    let inv = inventory();
    let render = |sdl: &str| {
        render_link_field(
            &first_link(SELECTION),
            "Widget_Co_Widget",
            "owner_id",
            &owner_op(&inv),
            sdl,
        )
    };
    // connect/v0.2 parses no `?!`.
    let err = render(&with_connect(SDL, "v0.2")).unwrap_err();
    assert_eq!(err.kind(), "nullable-fk");
    let detail = err.to_string();
    assert!(
        detail.starts_with("nullable-fk — Widget_Co_Widget.owner_id is nullable, so for a parent whose owner_id is null the router sends GET /owners/ and the field fails (CONNECTOR_FETCH) or maps that answer"),
        "{}",
        detail
    );
    assert!(
        detail.contains("connect/v0.2, which has no `?!`"),
        "{}",
        detail
    );
    // connect/v0.3 with a value that reads no body: under the subselection
    // form it would survive a null parent as a record of nothing.
    let constant = SDL.replacen(
        "selection: \"id login\"",
        "selection: \"\"\"id login kind: $(\"owner\")\"\"\"",
        1,
    );
    assert_ne!(constant, SDL, "fixture drifted");
    let err = render(&constant).unwrap_err();
    assert_eq!(err.kind(), "nullable-fk");
    assert!(err.to_string().contains("holds `$(`"), "{}", err);
    // connect/v0.4 prints it: the arm form maps a null parent to null
    // whatever the selection holds.
    let text = render(&with_connect(&constant, "v0.4")).unwrap();
    assert!(
        text.contains("[@, $ {\n        id login kind: $(\"owner\")\n      }])"),
        "{}",
        text
    );
    // The CLI refuses it with exit 1 and names the kind.
    let dir = workspace(&with_connect(SDL, "v0.2"), SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1, "{}", report);
    assert_eq!(report["refused"][0]["kind"], "nullable-fk", "{}", report);
    assert_eq!(report["links"], json!([]), "{}", report);
}

#[test]
fn lint_warns_on_a_relationship_field_whose_nullable_fk_has_no_null_guard() {
    // The field as it was printed before ADR 0084, pasted by hand.
    let unguarded = "  owner: Widget_Co_Owner\n    @connect(\n      source: \"widget_co\"\n      http: { GET: \"/owners/{$this.owner_id}\" }\n      selection: \"\"\"id login\"\"\"\n    )\n";
    let dir = workspace(&paste(SDL, "Widget_Co_Widget", unguarded), SELECTION);
    let found = lint_rules(dir.path(), "link-null-guard");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(found[0].severity, "warn");
    assert!(
        found[0].message.starts_with("Widget_Co_Widget.owner reads {$this.owner_id} and owner_id is nullable (ID), but the connector carries no isSuccess null guard and no selection null guard; for a parent whose owner_id is null the router still sends GET /owners/ and the field fails with CONNECTOR_FETCH"),
        "{}",
        found[0].message
    );
    assert!(
        found[0].message.contains("null-owner_id parent"),
        "{}",
        found[0].message
    );
    // Half a guard names the missing half.
    let half = unguarded.replacen(
        "http: { GET: \"/owners/{$this.owner_id}\" }\n",
        &format!(
            "http: {{ GET: \"/owners/{{$this.owner_id}}\" }}\n      {}\n",
            GUARD_IS_SUCCESS
        ),
        1,
    );
    let dir = workspace(&paste(SDL, "Widget_Co_Widget", &half), SELECTION);
    let found = lint_rules(dir.path(), "link-null-guard");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(
        found[0]
            .message
            .contains("carries no selection null guard;"),
        "{}",
        found[0].message
    );
    // The printed field, in either form, draws nothing.
    for version in ["v0.3", "v0.4"] {
        let sdl = with_connect(SDL, version);
        let text = render_link_field(
            &first_link(SELECTION),
            "Widget_Co_Widget",
            "owner_id",
            &owner_op(&inventory()),
            &sdl,
        )
        .unwrap();
        let dir = workspace(&paste(&sdl, "Widget_Co_Widget", &text), SELECTION);
        assert_eq!(
            lint_rules(dir.path(), "link-null-guard").len(),
            0,
            "{}",
            version
        );
    }
    // A non-null fk needs no guard.
    let non_null = SDL.replacen("  owner_id: ID\n", "  owner_id: ID!\n", 1);
    let dir = workspace(&paste(&non_null, "Widget_Co_Widget", unguarded), SELECTION);
    assert_eq!(lint_rules(dir.path(), "link-null-guard").len(), 0);
}

/// A workspace with the printed `owner` field pasted, `files` written on
/// top, for lint's `link-untested` rule (ADR 0094).
fn untested_workspace(files: &[(&str, &str)]) -> tempfile::TempDir {
    let text = render_link_field(
        &first_link(SELECTION),
        "Widget_Co_Widget",
        "owner_id",
        &owner_op(&inventory()),
        SDL,
    )
    .unwrap();
    let dir = workspace(&paste(SDL, "Widget_Co_Widget", &text), SELECTION);
    for (rel, body) in files {
        let f = dir.path().join(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, body).unwrap();
    }
    dir
}

const ROOT_UNIT: &str = "tests:\n  - name: \"widget\"\n    target: \"Query.widget_co_widget\"\n    variables:\n      $args:\n        id: \"w-1\"\n    apiResponseBody: |\n      {\"id\": \"w-1\"}\n    expect:\n      connectorRequest:\n        method: GET\n        url: https://api.widgets.test/widgets/w-1\n      connectorResponse: |\n        {\"id\": \"w-1\"}\n";
const LINK_UNIT: &str = "  - name: \"owner relationship field\"\n    target: \"Widget_Co_Widget.owner\"\n    variables:\n      $this:\n        owner_id: \"o-7\"\n    apiResponseBody: |\n      {\"id\": \"o-7\", \"login\": \"octo\"}\n    expect:\n      connectorRequest:\n        method: GET\n        url: https://api.widgets.test/owners/o-7\n      connectorResponse: |\n        {\"id\": \"o-7\", \"login\": \"octo\"}\n";
const ROOT_CASE: &str = "# owner { id } is only mentioned here, in a comment.\nquery { widget_co_widget(id: \"w-1\") { id name owner_id } }\n";

#[test]
fn lint_warns_on_a_relationship_field_no_unit_entry_or_e2e_case_exercises() {
    let unit_path = "tests/widget-co.connector.yaml";
    // Root tests only: the field has neither, and the comment's
    // `owner { id }` is not a selection.
    let dir = untested_workspace(&[
        (unit_path, ROOT_UNIT),
        ("tests/cases/widget.graphql", ROOT_CASE),
    ]);
    let found = lint_rules(dir.path(), "link-untested");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(found[0].severity, "warn");
    assert_eq!(found[0].file.as_deref(), Some("widget-co.graphql"));
    assert!(found[0].line.is_some(), "{:?}", found[0]);
    assert!(
        found[0].message.starts_with("Widget_Co_Widget.owner is a relationship field with no unit entry (target: \"Widget_Co_Widget.owner\" in tests/*.connector.yaml) and no e2e case (a tests/cases/*.graphql selecting owner on Widget_Co_Widget); with neither it is not validated, whatever evidence/latest.json says"),
        "{}",
        found[0].message
    );
    // A unit entry leaves only the e2e case missing.
    let both_units = format!("{}{}", ROOT_UNIT, LINK_UNIT);
    let dir = untested_workspace(&[
        (unit_path, &both_units),
        ("tests/cases/widget.graphql", ROOT_CASE),
    ]);
    let found = lint_rules(dir.path(), "link-untested");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(
        found[0].message.starts_with("Widget_Co_Widget.owner is a relationship field with no e2e case (a tests/cases/*.graphql selecting owner on Widget_Co_Widget); the field is half tested — write it by hand"),
        "{}",
        found[0].message
    );
    // A case selecting the field — directly, aliased, through a named
    // fragment or an inline fragment — leaves only the unit entry missing.
    for case in [
        "query { widget_co_widget(id: \"w-1\") { id owner { id login } } }\n",
        "query { widget_co_widget(id: \"w-1\") { id boss: owner { id } } }\n",
        "query { widget_co_widget(id: \"w-1\") { ...W } }\nfragment W on Widget_Co_Widget { owner_id owner { login } }\n",
        "query { widget_co_widget(id: \"w-1\") { ... on Widget_Co_Widget { owner { id } } } }\n",
    ] {
        let dir = untested_workspace(&[
            (unit_path, ROOT_UNIT),
            ("tests/cases/widget.graphql", ROOT_CASE),
            ("tests/cases/widget_owner.graphql", case),
        ]);
        let found = lint_rules(dir.path(), "link-untested");
        assert_eq!(found.len(), 1, "{}: {:?}", case, found);
        assert!(
            found[0].message.starts_with("Widget_Co_Widget.owner is a relationship field with no unit entry (target: \"Widget_Co_Widget.owner\" in tests/*.connector.yaml);"),
            "{}: {}",
            case,
            found[0].message
        );
        // Both together: quiet.
        let dir = untested_workspace(&[
            (unit_path, &both_units),
            ("tests/cases/widget_owner.graphql", case),
        ]);
        assert_eq!(lint_rules(dir.path(), "link-untested").len(), 0, "{}", case);
    }
    // A field of the same name on another type is not this field: the
    // owner type gets a plain `owner` of its own, and a case selects that.
    let dir = untested_workspace(&[(unit_path, &both_units)]);
    let schema_path = dir.path().join("widget-co.graphql");
    let sdl = std::fs::read_to_string(&schema_path).unwrap().replacen(
        "type Widget_Co_Owner {\n  id: ID!\n  login: String\n}",
        "type Widget_Co_Owner {\n  id: ID!\n  login: String\n  owner: String\n}",
        1,
    );
    assert!(sdl.contains("  owner: String\n"), "fixture drifted");
    std::fs::write(&schema_path, sdl).unwrap();
    std::fs::create_dir_all(dir.path().join("tests/cases")).unwrap();
    std::fs::write(
        dir.path().join("tests/cases/owner.graphql"),
        "query { widget_co_owner(ownerId: \"o-7\") { id login owner } }\n",
    )
    .unwrap();
    let found = lint_rules(dir.path(), "link-untested");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(
        found[0]
            .message
            .starts_with("Widget_Co_Widget.owner is a relationship field with no e2e case"),
        "{}",
        found[0].message
    );
    // What an operation does not execute is not coverage: a fragment no
    // operation spreads, and a field, a spread or an inline fragment under
    // a literal @skip(if: true) or @include(if: false).
    for case in [
        "query { widget_co_widget(id: \"w-1\") { id } }\nfragment Unused on Widget_Co_Widget { owner { id } }\n",
        "query { widget_co_widget(id: \"w-1\") { id owner @skip(if: true) { id } } }\n",
        "query { widget_co_widget(id: \"w-1\") { id owner @include(if: false) { id } } }\n",
        "query { widget_co_widget(id: \"w-1\") { id ...W @skip(if: true) } }\nfragment W on Widget_Co_Widget { owner { id } }\n",
        "query { widget_co_widget(id: \"w-1\") { id ... on Widget_Co_Widget @include(if: false) { owner { id } } } }\n",
    ] {
        let dir = untested_workspace(&[
            (unit_path, &both_units),
            ("tests/cases/widget_owner.graphql", case),
        ]);
        let found = lint_rules(dir.path(), "link-untested");
        assert_eq!(found.len(), 1, "{}: {:?}", case, found);
        assert!(
            found[0]
                .message
                .starts_with("Widget_Co_Widget.owner is a relationship field with no e2e case"),
            "{}: {}",
            case,
            found[0].message
        );
    }
    // A condition on a variable, a literal @skip(if: false), and a spread
    // fragment that spreads another all count.
    for case in [
        "query Q($s: Boolean!) { widget_co_widget(id: \"w-1\") { id owner @skip(if: $s) { id } } }\n",
        "query { widget_co_widget(id: \"w-1\") { id owner @skip(if: false) { id } } }\n",
        "query { widget_co_widget(id: \"w-1\") { ...A } }\nfragment A on Widget_Co_Widget { id ...B }\nfragment B on Widget_Co_Widget { owner { id } }\n",
    ] {
        let dir = untested_workspace(&[
            (unit_path, &both_units),
            ("tests/cases/widget_owner.graphql", case),
        ]);
        assert_eq!(lint_rules(dir.path(), "link-untested").len(), 0, "{}", case);
    }
    // The unit half reads the suite as YAML: a commented-out target line
    // and a block scalar that mentions the target are not an entry.
    let commented = format!(
        "{}  # - name: \"owner\"\n  #   target: \"Widget_Co_Widget.owner\"\n  - name: \"note\"\n    target: \"Query.widget_co_owner\"\n    apiResponseBody: |\n      target: \"Widget_Co_Widget.owner\"\n",
        ROOT_UNIT
    );
    let dir = untested_workspace(&[
        (unit_path, &commented),
        (
            "tests/cases/widget_owner.graphql",
            "query { widget_co_widget(id: \"w-1\") { id owner { id } } }\n",
        ),
    ]);
    let found = lint_rules(dir.path(), "link-untested");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(
        found[0].message.starts_with(
            "Widget_Co_Widget.owner is a relationship field with no unit entry (target:"
        ),
        "{}",
        found[0].message
    );
    // No suite and no case: the field has neither, and says so.
    let dir = untested_workspace(&[]);
    let found = lint_rules(dir.path(), "link-untested");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(
        found[0].message.contains(" and no e2e case ")
            && found[0]
                .message
                .contains("with neither it is not validated"),
        "{}",
        found[0].message
    );
    // No suite, a case: the unit entry is still missing.
    let dir = untested_workspace(&[(
        "tests/cases/widget_owner.graphql",
        "query { widget_co_widget(id: \"w-1\") { id owner { id } } }\n",
    )]);
    let found = lint_rules(dir.path(), "link-untested");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert!(
        found[0].message.starts_with(
            "Widget_Co_Widget.owner is a relationship field with no unit entry (target: \"Widget_Co_Widget.owner\" in tests/*.connector.yaml); the field is half tested"
        ),
        "{}",
        found[0].message
    );
    // A schema with no relationship field draws nothing.
    let dir = workspace(SDL, SELECTION);
    assert_eq!(lint_rules(dir.path(), "link-untested").len(), 0);
}

// ---- link-null-untested and link-live-unaccounted (ADR 0106) ----

const OWNER_CASE: &str = "query { widget_co_widget(id: \"w-1\") { id owner_id owner { id } } }\n";

/// A WireMock mapping answering `request` (a JSON object's body) and
/// carrying `metadata` (`""` for none).
fn mapping(request: &str, metadata: &str) -> String {
    if metadata.is_empty() {
        format!(
            "{{\"request\": {{{}}}, \"response\": {{\"status\": 307}}}}\n",
            request
        )
    } else {
        format!(
            "{{\"request\": {{{}}}, \"response\": {{\"status\": 307}}, \"metadata\": {{{}}}}}\n",
            request, metadata
        )
    }
}

#[test]
fn lint_warns_on_a_nullable_fk_no_e2e_case_proves_the_null_parent_for() {
    let unit = format!("{}{}", ROOT_UNIT, LINK_UNIT);
    let unit_path = "tests/widget-co.connector.yaml";
    let empty = "tests/fixtures/mappings/owner_empty_segment.json";
    let base: Vec<(&str, &str)> = vec![
        (unit_path, &unit),
        ("tests/cases/widget.graphql", ROOT_CASE),
        ("tests/cases/widget_owner.graphql", OWNER_CASE),
    ];
    // A unit entry and an e2e case, no mapping for GET /owners/.
    let dir = untested_workspace(&base);
    assert_eq!(lint_rules(dir.path(), "link-untested").len(), 0);
    let found = lint_rules(dir.path(), "link-null-untested");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(found[0].severity, "warn");
    assert_eq!(found[0].file.as_deref(), Some("widget-co.graphql"));
    assert!(found[0].line.is_some(), "{:?}", found[0]);
    assert!(
        found[0].message.starts_with("Widget_Co_Widget.owner reads {$this.owner_id} and owner_id is nullable, but no e2e case answers the empty-segment GET /owners/ for a null-owner_id parent; the field is not validated"),
        "{}",
        found[0].message
    );
    // A mapping that answers it and serves a case selecting the field
    // clears it: through x-cases, x-shared, the file name, and `url` with
    // or without a query string, method GET or ANY.
    let answered: Vec<(&str, String)> = vec![
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"urlPath\": \"/owners/\"",
                "\"x-cases\": [\"widget_owner\"]",
            ),
        ),
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"urlPath\": \"/owners/\"",
                "\"x-cases\": [\"widget-owner\"]",
            ),
        ),
        (
            empty,
            mapping(
                "\"method\": \"ANY\", \"urlPath\": \"/owners/\"",
                "\"x-shared\": true",
            ),
        ),
        (
            "tests/fixtures/mappings/widget_owner.json",
            mapping("\"method\": \"GET\", \"urlPath\": \"/owners/\"", ""),
        ),
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"url\": \"/owners/\"",
                "\"x-cases\": [\"widget_owner\"]",
            ),
        ),
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"url\": \"/owners/?expand=1\"",
                "\"x-cases\": [\"widget_owner\"]",
            ),
        ),
    ];
    for (path, body) in &answered {
        let mut files = base.clone();
        files.push((path, body));
        let dir = untested_workspace(&files);
        assert_eq!(
            lint_rules(dir.path(), "link-null-untested").len(),
            0,
            "{}: {}",
            path,
            body
        );
    }
    // Not an answer: the served case does not select the field, the file
    // name serves another case, the path is not the empty segment, the
    // method is not GET, and a url that only begins with the path.
    let unanswered: Vec<(&str, String)> = vec![
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"urlPath\": \"/owners/\"",
                "\"x-cases\": [\"widget\"]",
            ),
        ),
        (
            empty,
            mapping("\"method\": \"GET\", \"urlPath\": \"/owners/\"", ""),
        ),
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"urlPath\": \"/owners\"",
                "\"x-cases\": [\"widget_owner\"]",
            ),
        ),
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"urlPath\": \"/owners/o-7\"",
                "\"x-cases\": [\"widget_owner\"]",
            ),
        ),
        (
            empty,
            mapping(
                "\"method\": \"POST\", \"urlPath\": \"/owners/\"",
                "\"x-cases\": [\"widget_owner\"]",
            ),
        ),
        (
            empty,
            mapping(
                "\"method\": \"GET\", \"url\": \"/owners/o-7?x=1\"",
                "\"x-cases\": [\"widget_owner\"]",
            ),
        ),
    ];
    for (path, body) in &unanswered {
        let mut files = base.clone();
        files.push((path, body));
        let dir = untested_workspace(&files);
        assert_eq!(
            lint_rules(dir.path(), "link-null-untested").len(),
            1,
            "{}: {}",
            path,
            body
        );
    }
    // A field with no e2e case at all is link-untested's, not reported twice.
    let dir = untested_workspace(&[
        (unit_path, &unit),
        ("tests/cases/widget.graphql", ROOT_CASE),
    ]);
    assert_eq!(lint_rules(dir.path(), "link-untested").len(), 1);
    assert_eq!(lint_rules(dir.path(), "link-null-untested").len(), 0);
    // A non-null fk has no null parent to prove.
    let dir = untested_workspace(&base);
    let schema_path = dir.path().join("widget-co.graphql");
    let sdl = std::fs::read_to_string(&schema_path).unwrap();
    let non_null = sdl.replacen("  owner_id: ID\n", "  owner_id: ID!\n", 1);
    assert_ne!(sdl, non_null, "fixture drifted");
    std::fs::write(&schema_path, non_null).unwrap();
    assert_eq!(lint_rules(dir.path(), "link-null-untested").len(), 0);
}

#[test]
fn lint_warns_on_a_relationship_field_no_live_case_selects_and_no_field_exclusion_names() {
    let live = "tests/live.yaml";
    let doc = "tests/live/widget_owner.graphql";
    let rules = |files: &[(&str, &str)]| {
        let dir = untested_workspace(files);
        (
            lint_rules(dir.path(), "link-live-unaccounted"),
            lint_rules(dir.path(), "live-exclusion-unknown"),
            lint_rules(dir.path(), "live-exclusion-unreasoned"),
            lint_rules(dir.path(), "live-exclusion-malformed"),
        )
    };
    // No tests/live.yaml: the rule has nothing to read.
    assert_eq!(rules(&[]).0.len(), 0);
    // A live.yaml with no case and no exclusion.
    let (found, ..) = rules(&[(live, "cases: []\nexclusions: []\n")]);
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(found[0].severity, "warn");
    assert_eq!(found[0].file.as_deref(), Some("live.yaml"));
    assert!(
        found[0].message.starts_with("Widget_Co_Widget.owner is a relationship field no live case selects and no exclusion names — add a tests/live case that selects owner on Widget_Co_Widget, or an `exclusions:` entry `field: \"Widget_Co_Widget.owner\"`"),
        "{}",
        found[0].message
    );
    // A listed live case whose document selects the field clears it.
    let listed = "cases:\n  - name: widget_owner\n";
    assert_eq!(rules(&[(live, listed), (doc, OWNER_CASE)]).0.len(), 0);
    // A listed case that does not select it, a document no case lists, and
    // a selection only in a comment do not.
    assert_eq!(rules(&[(live, listed), (doc, ROOT_CASE)]).0.len(), 1);
    assert_eq!(
        rules(&[(live, "cases: []\n"), (doc, OWNER_CASE)]).0.len(),
        1
    );
    // A `field:` exclusion naming it, with a reason, clears it.
    let (found, unknown, unreasoned, malformed) = rules(&[(
        live,
        "cases: []\nexclusions:\n  - field: \"Widget_Co_Widget.owner\"\n    reason: its only parent is a write\n",
    )]);
    assert_eq!(found.len(), 0, "{:?}", found);
    assert_eq!(unknown.len() + unreasoned.len() + malformed.len(), 0);
    // One naming no relationship field is unknown, and clears nothing.
    let (found, unknown, ..) = rules(&[(
        live,
        "cases: []\nexclusions:\n  - field: \"Widget_Co_Widget.name\"\n    reason: a write\n",
    )]);
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(unknown.len(), 1, "{:?}", unknown);
    assert!(
        unknown[0]
            .message
            .starts_with("live exclusion field Widget_Co_Widget.name is not a relationship field"),
        "{}",
        unknown[0].message
    );
    // One with no reason is unreasoned.
    let (_, _, unreasoned, _) = rules(&[(
        live,
        "cases: []\nexclusions:\n  - field: \"Widget_Co_Widget.owner\"\n",
    )]);
    assert_eq!(unreasoned.len(), 1, "{:?}", unreasoned);
    assert!(
        unreasoned[0].message.starts_with(
            "live exclusion Widget_Co_Widget.owner gives no reason — say why the field cannot"
        ),
        "{}",
        unreasoned[0].message
    );
    // An operation exclusion whose reason names the field is prose, not an
    // exclusion of the field.
    let (found, unknown, ..) = rules(&[(
        live,
        "cases: []\nexclusions:\n  - operation: \"get:/owners/{ownerId}\"\n    reason: \"link Widget_Co_Widget.owner: its only parent is a write\"\n",
    )]);
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(unknown.len(), 0, "{:?}", unknown);
    // Both keys, or neither, is malformed (live.sh fails on it).
    for body in [
        "cases: []\nexclusions:\n  - operation: \"get:/owners/{ownerId}\"\n    field: \"Widget_Co_Widget.owner\"\n    reason: r\n",
        "cases: []\nexclusions:\n  - reason: r\n",
    ] {
        let (_, _, _, malformed) = rules(&[(live, body)]);
        assert_eq!(malformed.len(), 1, "{}: {:?}", body, malformed);
        assert_eq!(malformed[0].severity, "error");
    }
    // A schema with no relationship field draws nothing.
    let dir = workspace(SDL, SELECTION);
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::write(dir.path().join(live), "cases: []\n").unwrap();
    assert_eq!(lint_rules(dir.path(), "link-live-unaccounted").len(), 0);
}

// ---- link-target-refused (ADR 0098) ----

/// The fixture inventory with `Owner` no longer carrying `id`: ADR 0085's
/// rule (`link_target_refusal`) now refuses `get:/owners/{ownerId}`, while
/// the recorded fact on `Widget.owner_id` is still there, as in an
/// inventory built before the rule (AppWorld splitwise).
fn refused_inventory() -> Value {
    let mut inv = inventory();
    let owner = inv["shapes"]["Owner"]["properties"]
        .as_object_mut()
        .unwrap();
    assert!(owner.remove("id").is_some(), "fixture drifted");
    inv
}

/// The fixture inventory with the fact on `Widget.owner_id` gone and the
/// target still qualifying: the builder's other rules no longer propose it.
fn fact_gone_inventory() -> Value {
    let mut inv = inventory();
    let fk = inv["shapes"]["Widget"]["properties"]["owner_id"]
        .as_object_mut()
        .unwrap();
    assert!(
        fk.remove("candidate_entity_link").is_some(),
        "fixture drifted"
    );
    inv
}

/// `SDL` with the link field pasted exactly as `links apply --dry-run`
/// prints it against the sound inventory.
fn pasted_sdl() -> String {
    let dir = workspace(SDL, SELECTION);
    let (code, report) = apply_json(dir.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);
    paste(
        SDL,
        "Widget_Co_Widget",
        report["links"][0]["text"].as_str().unwrap(),
    )
}

fn reconciled_links(dir: &Path) -> Value {
    graphos_factory_core::reconcile::reconcile_workspace(dir, None).unwrap()
}

#[test]
fn a_pasted_link_whose_target_is_refused_is_a_lint_error_and_reconcile_drift() {
    // Sound first: the pasted field draws nothing and reconciles in sync,
    // so what follows is the refusal's doing and nothing else.
    let sdl = pasted_sdl();
    let sound = workspace(&sdl, SELECTION);
    assert!(lint_rules(sound.path(), "link-target-refused").is_empty());
    assert_eq!(reconciled_links(sound.path())["links"]["change"], json!([]));

    let dir = workspace_with(&sdl, SELECTION, &refused_inventory());
    let found = lint_rules(dir.path(), "link-target-refused");
    assert_eq!(found.len(), 1, "{:?}", found);
    let f = &found[0];
    assert_eq!(f.severity, "error", "{:?}", f);
    for needle in [
        "link Widget > owner_id",
        "pasted as Widget_Co_Widget.owner (line",
        "get:/owners/{ownerId} is a refused link target: its response carries no `ownerId` nor an `id`",
        "never regenerate",
        "include: false",
    ] {
        assert!(f.message.contains(needle), "{:?} not in {}", needle, f.message);
    }
    let report = reconciled_links(dir.path());
    let links = &report["links"];
    assert_eq!(links["add"], json!([]), "{}", links);
    assert_eq!(links["remove"], json!([]), "{}", links);
    assert_eq!(links["unchanged"], json!([]), "{}", links);
    let change = links["change"].as_array().unwrap();
    assert_eq!(change.len(), 1, "{}", links);
    assert_eq!(change[0]["type"], "Widget_Co_Widget");
    assert_eq!(change[0]["field"], "owner");
    assert_eq!(change[0]["stale"], true);
    assert!(change[0]["line"].as_u64().is_some(), "{}", change[0]);
    let drift = change[0]["drift"].to_string();
    assert!(drift.contains("is a refused link target"), "{}", drift);
    assert!(drift.contains("pasted at line"), "{}", drift);
    assert_eq!(report["clean"], false);
}

#[test]
fn an_unpasted_link_whose_target_is_refused_warns_and_is_never_asked_for() {
    // Sound, the unpasted link is an add: reconcile asks for the field.
    let sound = workspace(SDL, SELECTION);
    assert_eq!(
        reconciled_links(sound.path())["links"]["add"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    let dir = workspace_with(SDL, SELECTION, &refused_inventory());
    let found = lint_rules(dir.path(), "link-target-refused");
    assert_eq!(found.len(), 1, "{:?}", found);
    assert_eq!(found[0].severity, "warn", "{:?}", found[0]);
    assert!(
        found[0].message.contains("do not paste it"),
        "{}",
        found[0].message
    );
    // The remedy is to decline it, not to give it a host.
    assert!(lint_rules(dir.path(), "link-no-host").is_empty());
    let links = &reconciled_links(dir.path())["links"];
    assert_eq!(
        links["add"],
        json!([]),
        "a refused link is never asked for: {}",
        links
    );
    let change = links["change"].as_array().unwrap();
    assert_eq!(change.len(), 1, "{}", links);
    assert!(change[0]["line"].is_null(), "{}", change[0]);
    assert!(
        change[0]["drift"]
            .to_string()
            .contains("not pasted, and not to be pasted"),
        "{}",
        change[0]
    );
}

#[test]
fn a_link_whose_fact_is_gone_is_flagged_like_a_refused_one() {
    for (case, sdl, severity) in [
        ("not pasted", SDL.to_string(), "warn"),
        ("pasted", pasted_sdl(), "error"),
    ] {
        let dir = workspace_with(&sdl, SELECTION, &fact_gone_inventory());
        let found = lint_rules(dir.path(), "link-target-refused");
        assert_eq!(found.len(), 1, "{}: {:?}", case, found);
        assert_eq!(found[0].severity, severity, "{}", case);
        assert!(
            found[0]
                .message
                .contains("inventory.json carries no candidate_entity_link fact for Widget > owner_id -> get:/owners/{ownerId}"),
            "{}: {}",
            case,
            found[0].message
        );
        let links = &reconciled_links(dir.path())["links"];
        assert_eq!(links["add"], json!([]), "{}: {}", case, links);
        assert_eq!(
            links["change"].as_array().map(Vec::len),
            Some(1),
            "{}: {}",
            case,
            links
        );
    }
}

#[test]
fn a_stale_link_that_names_a_resolved_decision_is_kept() {
    // ADR 0113 §4: only a resolved `keep` (`keep` or `keep-…`) exempts a
    // stale link; an open decision, a `drop`, a resolution that chose
    // neither, an unrecorded id and no decision at all leave it stale, each
    // with its own remedy.
    let decisions = |status: &str, chosen: &str| {
        format!(
            "{{\"contract_version\": 1, \"decisions\": [{{\"id\": \"D-0001\", \"title\": \"keep the owner link\", \"status\": \"{}\", \"date\": \"2026-09-30\", \"question\": \"Keep it?\", \"choices\": [{{\"id\": \"keep\", \"label\": \"Keep\"}}, {{\"id\": \"keep-guarded\", \"label\": \"Keep, guarded\"}}, {{\"id\": \"drop\", \"label\": \"Drop\"}}, {{\"id\": \"other\", \"label\": \"Other\"}}]{}}}]}}\n",
            status,
            if status == "resolved" {
                format!(", \"resolution\": {{\"chosen\": [{}], \"note\": \"n\", \"by\": \"user\"}}", chosen)
            } else {
                String::new()
            }
        )
    };
    let cited = SELECTION.replacen(
        "    confirmed: true\n",
        "    confirmed: true\n    decision: D-0001\n",
        1,
    );
    assert_ne!(cited, SELECTION, "fixture drifted");
    let sdl = pasted_sdl();
    // (case, selection, decisions.json, findings expected, remedy text)
    for (case, selection, log, expected, says) in [
        (
            "resolved keep named",
            cited.as_str(),
            Some(decisions("resolved", "\"keep\"")),
            0,
            "",
        ),
        (
            "resolved keep-guarded named (gitea's D-0018)",
            cited.as_str(),
            Some(decisions("resolved", "\"keep-guarded\"")),
            0,
            "",
        ),
        (
            "resolved drop named",
            cited.as_str(),
            Some(decisions("resolved", "\"drop\"")),
            1,
            "D-0001 chose drop: remove only that field",
        ),
        (
            "resolved, neither keep nor drop",
            cited.as_str(),
            Some(decisions("resolved", "\"other\"")),
            1,
            "D-0001 neither keeps nor drops it",
        ),
        (
            "open decision named",
            cited.as_str(),
            Some(decisions("open", "")),
            1,
            "D-0001 is open: answer it (`graphos-factory-core decisions resolve . --id D-0001 --chosen keep`",
        ),
        (
            "decision named but no log",
            cited.as_str(),
            None,
            1,
            "decision: D-0001 is not recorded in decisions.json; raise the decision",
        ),
        (
            "no decision named",
            SELECTION,
            Some(decisions("resolved", "\"keep\"")),
            1,
            "raise the decision for it, run: graphos-factory-core decisions add . --title 'Stale link: Widget > owner_id' --question",
        ),
    ] {
        let dir = workspace_with(&sdl, selection, &refused_inventory());
        if let Some(text) = &log {
            std::fs::write(dir.path().join(".factory/decisions.json"), text).unwrap();
        }
        let found = lint_rules(dir.path(), "link-target-refused");
        assert_eq!(found.len(), expected, "{}: {:?}", case, found);
        if expected > 0 {
            assert!(found[0].message.contains(says), "{}: {}", case, found[0].message);
        }
        let links = &reconciled_links(dir.path())["links"];
        if expected == 0 {
            assert_eq!(links["change"], json!([]), "{}: {}", case, links);
            assert_eq!(
                links["unchanged"].as_array().map(Vec::len),
                Some(1),
                "{}",
                case
            );
        } else {
            assert_eq!(
                links["change"].as_array().map(Vec::len),
                Some(1),
                "{}: {}",
                case,
                links
            );
            assert!(links["change"][0]["drift"].to_string().contains(says), "{}: {}", case, links);
        }
    }
}

#[test]
fn a_stale_link_with_no_decision_prints_the_exact_decisions_add_that_raises_it() {
    // The binary writes decisions only through the verb (ADR 0113 §4):
    // reconcile, lint and links apply print the command; running it, then
    // naming the id and resolving keep, exempts the link.
    let dir = workspace_with(&pasted_sdl(), SELECTION, &refused_inventory());
    let found = lint_rules(dir.path(), "link-target-refused");
    assert_eq!(found.len(), 1, "{:?}", found);
    let message = &found[0].message;
    let start = message
        .find("graphos-factory-core decisions add .")
        .unwrap();
    let end = start + message[start..].find(" — and name its id").unwrap();
    let command = &message[start..end];
    assert!(command.contains("--choice 'keep:"), "{}", command);
    assert!(command.contains("--choice 'drop:"), "{}", command);
    assert!(
        command.contains("--affects 'Widget_Co_Widget.owner'"),
        "{}",
        command
    );
    // Run the printed command through the verb itself.
    let words = shell_words(command);
    assert_eq!(&words[..3], ["graphos-factory-core", "decisions", "add"]);
    let mut argv: Vec<String> = vec!["add".into(), dir.path().to_string_lossy().to_string()];
    argv.extend(words[4..].iter().cloned());
    assert_eq!(graphos_factory_core::cmd::decisions::main(&argv), 0);
    let log: Value = graphos_factory_core::json::parse(
        &std::fs::read_to_string(dir.path().join(".factory/decisions.json")).unwrap(),
    )
    .unwrap();
    let rec = &log["decisions"][0];
    assert_eq!(rec["status"], "open");
    assert_eq!(rec["affects"], json!(["Widget_Co_Widget.owner"]));
    assert!(rec["context"]
        .as_str()
        .unwrap()
        .contains("is a refused link target"));
    // Named but open: still stale.
    let cited = SELECTION.replacen(
        "    confirmed: true\n",
        "    confirmed: true\n    decision: D-0001\n",
        1,
    );
    std::fs::write(dir.path().join(".factory/selection.yaml"), &cited).unwrap();
    assert_eq!(lint_rules(dir.path(), "link-target-refused").len(), 1);
    // Resolved keep: exempt.
    assert_eq!(
        graphos_factory_core::cmd::decisions::main(&[
            "resolve".to_string(),
            dir.path().to_string_lossy().to_string(),
            "--id".into(),
            "D-0001".into(),
            "--chosen".into(),
            "keep".into(),
        ]),
        0
    );
    assert_eq!(lint_rules(dir.path(), "link-target-refused").len(), 0);
}

/// Split a POSIX-shell command line of plain words and single-quoted words
/// (with `'\''` for an embedded quote), as `stale_link_decision_command`
/// prints it.
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '\\' => {
                in_word = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            ' ' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            other => {
                in_word = true;
                cur.push(other);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

#[test]
fn the_text_report_marks_a_stale_link_as_drift() {
    let dir = workspace_with(&pasted_sdl(), SELECTION, &refused_inventory());
    let text =
        graphos_factory_core::reconcile::render_report(&reconciled_links(dir.path()), ".", None);
    assert!(text.contains("links (+0 −0 ~1 =0)"), "{}", text);
    assert!(
        text.contains("  ~ Widget_Co_Widget.owner   (link Widget > owner_id)"),
        "{}",
        text
    );
    assert!(text.contains("is a refused link target"), "{}", text);
}

// ---- links apply refuses a stale entry (ADR 0100) ----

/// The one refusal `links apply --dry-run --json` reports, asserted to be
/// the only outcome: nothing printed, nothing skipped, exit 1.
fn only_refusal(dir: &Path) -> Value {
    let (code, report) = apply_json(dir, &["--dry-run"]);
    assert_eq!(code, 1, "{}", report);
    assert_eq!(report["links"], json!([]), "{}", report);
    let refused = report["refused"].as_array().unwrap();
    assert_eq!(refused.len(), 1, "{}", report);
    assert_eq!(refused[0]["link"], "Widget > owner_id");
    refused[0].clone()
}

#[test]
fn links_apply_refuses_a_link_whose_target_is_refused_with_lints_reason() {
    // Sound, the entry prints: the refusal below is the stale target's.
    let sound = workspace(SDL, SELECTION);
    let (code, report) = apply_json(sound.path(), &["--dry-run"]);
    assert_eq!(code, 0, "{}", report);

    let dir = workspace_with(SDL, SELECTION, &refused_inventory());
    let r = only_refusal(dir.path());
    assert_eq!(r["kind"], "target-refused", "{}", r);
    let detail = r["detail"].as_str().unwrap();
    // The reason is lint's, word for word.
    let lint = lint_rules(dir.path(), "link-target-refused");
    assert_eq!(lint.len(), 1, "{:?}", lint);
    let staleness = graphos_factory_core::reconcile::LinkStaleness::new(&refused_inventory());
    let reason = staleness.reason(&first_link(SELECTION)).unwrap();
    assert!(lint[0].message.contains(&reason), "{}", lint[0].message);
    // The remedy is reconcile's and lint's too: with no decision named, the
    // exact `decisions add` that raises one (ADR 0113 §4).
    let remedy = graphos_factory_core::reconcile::stale_link_remedy(
        &first_link(SELECTION),
        &graphos_factory_core::reconcile::LinkDecision::None,
        &reason,
        &["Widget_Co_Widget".to_string()],
        false,
    );
    assert!(remedy.contains("do not paste it"), "{}", remedy);
    assert!(
        remedy.contains(
            "run: graphos-factory-core decisions add . --title 'Stale link: Widget > owner_id'"
        ),
        "{}",
        remedy
    );
    assert!(lint[0].message.contains(&remedy), "{}", lint[0].message);
    assert_eq!(detail, format!("target-refused — {}; {}", reason, remedy));
    assert!(
        reason.starts_with("get:/owners/{ownerId} is a refused link target: its response carries no `ownerId` nor an `id`"),
        "{}",
        reason
    );
    // The text report says the same on stderr and prints nothing.
    let (code, stdout, stderr) = run(dir.path(), &["--dry-run"]);
    assert_eq!(code, 1);
    assert_eq!(stdout, "");
    assert!(
        stderr.contains(&format!(
            "links apply: refused Widget > owner_id: {}",
            detail
        )),
        "{}",
        stderr
    );
}

#[test]
fn links_apply_refuses_a_pasted_stale_link_naming_the_field_to_remove() {
    // Sound and pasted, it is skipped as applied, never refused.
    let sdl = pasted_sdl();
    let sound = workspace(&sdl, SELECTION);
    let (code, report) = apply_json(sound.path(), &["--dry-run"]);
    assert_eq!(code, 2, "{}", report);
    assert_eq!(report["refused"], json!([]));

    let dir = workspace_with(&sdl, SELECTION, &refused_inventory());
    let r = only_refusal(dir.path());
    assert_eq!(r["kind"], "target-refused", "{}", r);
    let detail = r["detail"].as_str().unwrap();
    for needle in [
        "is a refused link target",
        "it is pasted as Widget_Co_Widget.owner (line ",
        "remove only that field",
        "never regenerate",
        "include: false",
        "decision:",
    ] {
        assert!(detail.contains(needle), "{:?} not in {}", needle, detail);
    }
}

#[test]
fn links_apply_refuses_a_link_whose_fact_is_gone() {
    for (case, sdl) in [("not pasted", SDL.to_string()), ("pasted", pasted_sdl())] {
        let dir = workspace_with(&sdl, SELECTION, &fact_gone_inventory());
        let r = only_refusal(dir.path());
        assert_eq!(r["kind"], "target-refused", "{}: {}", case, r);
        assert!(
            r["detail"].as_str().unwrap().contains(
                "inventory.json carries no candidate_entity_link fact for Widget > owner_id -> get:/owners/{ownerId}"
            ),
            "{}: {}",
            case,
            r
        );
    }
}

#[test]
fn links_apply_prints_a_stale_link_kept_by_a_resolved_decision_exactly_as_a_sound_one() {
    let decisions = |status: &str| {
        format!(
            "{{\"contract_version\": 1, \"decisions\": [{{\"id\": \"D-0001\", \"title\": \"keep the owner link\", \"status\": \"{}\", \"date\": \"2026-09-30\", \"choices\": [{{\"id\": \"keep\", \"label\": \"Keep\"}}, {{\"id\": \"drop\", \"label\": \"Drop\"}}]{}}}]}}\n",
            status,
            if status == "resolved" {
                ", \"resolution\": {\"chosen\": [\"keep\"], \"note\": \"kept\", \"by\": \"user\"}"
            } else {
                ""
            }
        )
    };
    let cited = SELECTION.replacen(
        "    confirmed: true\n",
        "    confirmed: true\n    decision: \" D-0001 \"\n",
        1,
    );
    assert_ne!(cited, SELECTION, "fixture drifted");
    let sound = workspace(SDL, SELECTION);
    let (_, expected) = apply_json(sound.path(), &["--dry-run"]);
    let expected = expected["links"][0]["text"].clone();
    assert!(expected.is_string());
    for inventory in [refused_inventory(), fact_gone_inventory()] {
        // (case, selection, decisions.json, prints)
        for (case, selection, log, prints) in [
            (
                "resolved decision named",
                cited.as_str(),
                Some(decisions("resolved")),
                true,
            ),
            (
                "open decision named",
                cited.as_str(),
                Some(decisions("open")),
                false,
            ),
            ("decision named but no log", cited.as_str(), None, false),
            (
                "no decision named",
                SELECTION,
                Some(decisions("resolved")),
                false,
            ),
        ] {
            let dir = workspace_with(SDL, selection, &inventory);
            if let Some(text) = &log {
                std::fs::write(dir.path().join(".factory/decisions.json"), text).unwrap();
            }
            let (code, report) = apply_json(dir.path(), &["--dry-run"]);
            if prints {
                assert_eq!(code, 0, "{}: {}", case, report);
                assert_eq!(report["refused"], json!([]), "{}", case);
                assert_eq!(report["links"][0]["text"], expected, "{}", case);
                // Lint agrees: the kept entry draws nothing.
                assert!(
                    lint_rules(dir.path(), "link-target-refused").is_empty(),
                    "{}",
                    case
                );
            } else {
                assert_eq!(code, 1, "{}: {}", case, report);
                assert_eq!(report["refused"][0]["kind"], "target-refused", "{}", case);
                assert_eq!(
                    lint_rules(dir.path(), "link-target-refused").len(),
                    1,
                    "{}",
                    case
                );
            }
        }
    }
}

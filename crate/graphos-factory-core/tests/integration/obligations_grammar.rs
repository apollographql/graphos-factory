//! The obligations classifier reads the expression forms the skill itself
//! recommends (ADR 0050): `->entries` over a dictionary and `->match` value
//! translation in a selection, nested object literals and argument
//! sub-selections in a request body. Anything else stays `unresolved`, with
//! the construct named.

use graphos_factory_core::obligations::{build, Report};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

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

/// A workspace whose one operation `op` is served by the root fields in
/// `sdl_types` (full SDL after the `@source`).
fn workspace(tag: &str, op: Value, shapes: Value, sdl_types: &str) -> Workspace {
    let dir = std::env::temp_dir().join(format!(
        "obligations-grammar-{}-{}-{}",
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
            "extend schema\n  @link(url: \"https://specs.apollo.dev/connect/v0.3\", import: [\"@source\", \"@connect\"])\n\n@source(name: \"shop\", http: {{ baseURL: \"{{{{BASE_URL}}}}\" }})\n\nscalar Shop_JSON\n\n{}\n",
            sdl_types
        ),
    );
    write(
        &dir,
        ".factory/inventory.json",
        &json!({
            "contract_version": 1,
            "api": {"title": "Shop", "base_urls": ["https://shop.test"]},
            "operations": [op],
            "shapes": shapes
        })
        .to_string(),
    );
    Workspace(dir)
}

fn response_class(report: &Report, path: &str) -> String {
    report
        .response
        .iter()
        .find(|r| r.path == path)
        .map(|r| r.class.label())
        .unwrap_or_else(|| {
            panic!(
                "no response row {:?} in {:?}",
                path,
                report.response.iter().map(|r| &r.path).collect::<Vec<_>>()
            )
        })
}

// ─── Lever 1: `->entries` over a dictionary ─────────────────────────────────

const GET_STORE: &str = "get:/stores/{id}";

fn get_store() -> Value {
    json!({"key": GET_STORE, "operation_id": "getStore", "method": "GET", "path": "/stores/{id}",
        "response": {"status": "200", "shape_ref": "#/shapes/Store"}})
}

fn store_shapes() -> Value {
    json!({
        "Store": {"type": "object", "properties": {
            "id": {"type": "string"},
            // A dictionary: its keys are data (deck-of-cards `piles`).
            "piles": {"type": "object", "additionalProperties": {"$ref": "#/shapes/Pile"}},
            // A nullable dictionary through anyOf (a vendor's `extraData`).
            "extra": {"anyOf": [{"type": "object", "additionalProperties": {"type": "string"}}, {"type": "null"}]},
            // An object with fixed properties.
            "address": {"type": "object", "properties": {"city": {"type": "string"}, "zip": {"type": "string"}}}
        }},
        "Pile": {"type": "object", "properties": {"remaining": {"type": "integer"}}}
    })
}

fn store_field(selection: &str) -> String {
    format!(
        "type Query {{\n  shop_store(id: ID!): Shop_Store\n    @connect(source: \"shop\", http: {{ GET: \"/stores/{{$args.id}}\" }}, selection: \"\"\"\n{}\n\"\"\")\n}}",
        selection
    )
}

fn store_report(tag: &str, selection: &str) -> Report {
    let ws = workspace(tag, get_store(), store_shapes(), &store_field(selection));
    build(&ws.0, GET_STORE).unwrap()
}

#[test]
fn entries_over_a_dictionary_with_a_sub_selection_maps_its_row() {
    let r = store_report(
        "entries-children",
        "id\npiles: piles->entries { name: key remaining: value.remaining }",
    );
    assert_eq!(response_class(&r, "piles"), "mapped");
}

#[test]
fn entries_over_a_nullable_dictionary_as_a_leaf_maps_its_row() {
    let r = store_report("entries-leaf", "id\nextra: extra->entries");
    assert_eq!(response_class(&r, "extra"), "mapped");
}

#[test]
fn entries_over_an_object_with_fixed_properties_stays_unresolved_and_says_why() {
    let r = store_report("entries-object", "id\naddress: address->entries");
    assert_eq!(
        response_class(&r, "address.city"),
        "unresolved (selection method not parsed: ->entries)"
    );
}

// ─── Optional chaining and fallbacks (ADR 0057) ─────────────────────────────

/// `address?.city` reads the one leaf. Unparsed, the `?` ended the path at
/// `address`, so every `address.*` row read `mapped (json)` — a silent pass
/// for `zip`, which nothing maps.
#[test]
fn an_optional_step_maps_its_leaf_not_the_whole_parent() {
    let r = store_report("optional-step", "id\ncity: address?.city");
    assert_eq!(response_class(&r, "address.city"), "mapped");
    assert_eq!(response_class(&r, "address.zip"), "unaccounted");
}

#[test]
fn a_guarded_method_is_read_like_the_unguarded_one() {
    let r = store_report(
        "optional-match",
        "id\nzip: address?.zip?->match([null, null], [@, @])",
    );
    assert_eq!(response_class(&r, "address.zip"), "mapped");
    assert_eq!(response_class(&r, "address.city"), "unaccounted");
    let r = store_report("optional-method", "id\nzip: address?.zip?->jsonStringify");
    assert_eq!(
        response_class(&r, "address.zip"),
        "unresolved (selection method not parsed: ->jsonStringify)"
    );
    assert_eq!(response_class(&r, "address.city"), "unaccounted");
}

/// A `??` operand is read in the same context as the field it falls back
/// from, so it is mapped; a literal operand reads nothing.
#[test]
fn a_fallback_operand_is_mapped_and_a_literal_one_maps_nothing() {
    let r = store_report("fallback-path", "id\ncity: address.city ?? address.zip");
    assert_eq!(response_class(&r, "address.city"), "mapped");
    assert_eq!(response_class(&r, "address.zip"), "mapped");
    let r = store_report("fallback-literal", "id\ncity: address?.city ?! \"none\"");
    assert_eq!(response_class(&r, "address.city"), "mapped");
    assert_eq!(response_class(&r, "address.zip"), "unaccounted");
}

#[test]
fn a_method_chained_after_entries_stays_unresolved_and_names_the_chain() {
    let r = store_report("entries-chain", "id\npiles: piles->entries->first");
    assert_eq!(
        response_class(&r, "piles"),
        "unresolved (selection method not parsed: ->entries->first)"
    );
}

// ─── Lever 2: `->match` value translation ───────────────────────────────────

fn counts_shapes() -> Value {
    json!({
        "Store": {"type": "object", "properties": {
            "id": {"type": "string"},
            "size": {"type": "integer", "format": "int64"},
            "status": {"type": "string", "enum": ["open", "closed"]},
            "owner": {"type": "object", "properties": {"followers": {"type": "integer", "format": "int64"}, "name": {"type": "string"}}},
            "tags": {"type": "array", "items": {"type": "string"}}
        }}
    })
}

fn counts_report(tag: &str, selection: &str) -> Report {
    let ws = workspace(tag, get_store(), counts_shapes(), &store_field(selection));
    build(&ws.0, GET_STORE).unwrap()
}

/// gitea's null-preserving stringification: the leaf is `size`, mapped.
#[test]
fn match_on_a_leaf_maps_the_field_it_translates() {
    let r = counts_report(
        "match-leaf",
        "id\nsize: size->match([null, null], [@, @->jsonStringify])\nstatus: status->match([\"open\", \"OPEN\"], [\"closed\", \"CLOSED\"])",
    );
    assert_eq!(response_class(&r, "size"), "mapped");
    assert_eq!(response_class(&r, "status"), "mapped");
    // Coverage is the field's own: an unselected sibling stays unaccounted.
    assert_eq!(response_class(&r, "owner.name"), "unaccounted");
}

#[test]
fn match_inside_a_nested_selection_maps_the_nested_leaf() {
    let r = counts_report(
        "match-nested",
        "id\nowner { name followers: followers->match([null, null], [@, @->jsonStringify]) }",
    );
    assert_eq!(response_class(&r, "owner.followers"), "mapped");
    assert_eq!(response_class(&r, "owner.name"), "mapped");
}

#[test]
fn a_method_chained_after_match_stays_unresolved_and_names_the_chain() {
    let r = counts_report(
        "match-chain",
        "id\ntags: tags->match([null, []], [@, @])->first",
    );
    assert_eq!(
        response_class(&r, "tags[]"),
        "unresolved (selection method not parsed: ->match->first)"
    );
}

/// Over an object, an arm may send any part of it (`@.name` sends only
/// `name`), so `->match` is not value translation there.
#[test]
fn match_on_an_object_stays_unresolved() {
    let r = counts_report(
        "match-object",
        "id\nowner: owner->match([null, null], [@, @.name])",
    );
    for path in ["owner.name", "owner.followers"] {
        assert_eq!(
            response_class(&r, path),
            "unresolved (selection method not parsed: ->match on a non-scalar value)"
        );
    }
}

/// Granola's `events`: an array of enum strings, each element translated
/// with `->map(@->match(…))`. The connector reads exactly the array, so its
/// element row is mapped, as `->match` on a scalar leaf is.
#[test]
fn map_of_an_element_match_over_an_array_of_scalars_maps_its_elements() {
    // Coverage is the field's own, exactly as if the method were absent.
    let plain = response_class(&counts_report("map-plain", "id\ntags"), "tags[]");
    assert!(plain.starts_with("mapped"), "{}", plain);
    let r = counts_report(
        "map-element-match",
        "id\ntags: tags->map(@->match([\"a\", \"A\"], [\"b\", \"B\"]))",
    );
    assert_eq!(response_class(&r, "tags[]"), plain);
    let r = counts_report(
        "map-element-match-guarded",
        "id\ntags: tags?->map(@?->match([\"a\", \"A\"]))",
    );
    assert_eq!(response_class(&r, "tags[]"), plain);
}

/// Anything else inside the map still reads arbitrary parts of the element,
/// or chains after it: unresolved, the construct named.
#[test]
fn map_of_anything_but_one_element_match_stays_unresolved() {
    for (tag, selection, why) in [
        (
            "map-chain-inside",
            "id\ntags: tags->map(@->match([\"a\", \"A\"])->first)",
            "->map",
        ),
        (
            "map-then-first",
            "id\ntags: tags->map(@->match([\"a\", \"A\"]))->first",
            "->map->first",
        ),
        (
            "map-object-literal",
            "id\ntags: tags->map({ v: @ })",
            "->map",
        ),
    ] {
        let r = counts_report(tag, selection);
        assert_eq!(
            response_class(&r, "tags[]"),
            format!("unresolved (selection method not parsed: {})", why),
            "{}",
            tag
        );
    }
    // Over an array of objects the element-wise match stays unresolved too.
    let r = counts_report(
        "map-over-object",
        "id\nowner: owner->map(@->match([null, null], [@, @.name]))",
    );
    assert!(
        response_class(&r, "owner.name").starts_with("unresolved"),
        "{}",
        response_class(&r, "owner.name")
    );
}

#[test]
fn match_on_an_array_stays_unresolved() {
    let r = counts_report("match-array", "id\ntags: tags->match([[], null], [@, @])");
    assert_eq!(
        response_class(&r, "tags[]"),
        "unresolved (selection method not parsed: ->match on a non-scalar value)"
    );
}

/// A translated node's sub-selection is walked, not taken as one opaque
/// value: an unselected child stays unaccounted instead of `mapped (json)`.
#[test]
fn match_with_a_sub_selection_walks_the_children() {
    let r = counts_report(
        "match-children",
        "id\nowner: owner->match([null, null], [@, @]) { name }",
    );
    assert_eq!(response_class(&r, "owner.name"), "mapped");
    assert_eq!(response_class(&r, "owner.followers"), "unaccounted");
}

/// `--check` still fails closed on a row the walk leaves unresolved.
#[test]
fn check_fails_closed_on_a_construct_the_walk_does_not_read() {
    let ws = workspace(
        "check-unresolved",
        get_store(),
        counts_shapes(),
        &store_field(
            "id size status owner { name followers }\ntags: tags->match([null, []], [@, @])->first",
        ),
    );
    let report = build(&ws.0, GET_STORE).unwrap();
    assert_eq!(report.response_counts().unaccounted, 0);
    assert_eq!(
        report.check_failure().as_deref(),
        Some("response unresolved 1")
    );
    let code = graphos_factory_core::cmd::source_coverage::main(&[
        ws.0.to_str().unwrap().into(),
        GET_STORE.into(),
        "--check".into(),
    ]);
    assert_eq!(code, 1);
}

// ─── Lever 3: nested request bodies ─────────────────────────────────────────

const POST_STORE: &str = "post:/stores";

fn write_shapes() -> Value {
    json!({
        "Ack": {"type": "object", "properties": {"id": {"type": "string"}}},
        "Update": {"type": "object", "properties": {
            "incident": {"type": "object", "properties": {
                "type": {"type": "string"},
                "title": {"type": "string"},
                "urgency": {"type": "string"},
                "service": {"type": "object", "properties": {"id": {"type": "string"}, "type": {"type": "string"}}},
                "body": {"type": "object", "properties": {"details": {"type": "string"}}}
            }},
            "settings": {"type": "object", "properties": {
                "from_name": {"type": "string"},
                "title": {"type": "string"},
                "reply_to": {"type": "string"},
                "inner": {"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}}}
            }},
            "items": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "qty": {"type": "integer"}}}},
            "team": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "qty": {"type": "integer"}}}},
            "blob": {"type": "object", "additionalProperties": {"type": "string"}},
            "tree": {"$ref": "#/shapes/Tree"},
            "value": {"$ref": "#/shapes/Value"}
        }},
        // Recursive arrays: directly, and through a union's array variant.
        "Tree": {"type": "array", "items": {"$ref": "#/shapes/Tree"}},
        "Value": {"anyOf": [{"type": "string"}, {"type": "array", "items": {"$ref": "#/shapes/Value"}}]}
    })
}

fn write_report(tag: &str, body: &str) -> Report {
    let op = json!({"key": POST_STORE, "operation_id": "update", "method": "POST", "path": "/stores",
        "request_body": {"content_type": "application/json", "shape_ref": "#/shapes/Update"},
        "response": {"status": "200", "shape_ref": "#/shapes/Ack"}});
    let sdl = format!(
        "input Shop_SettingsInput {{ fromName: String title: String inner: Shop_InnerInput }}\ninput Shop_InnerInput {{ a: String b: String }}\ninput Shop_ItemInput {{ name: String qty: Int }}\ninput Shop_JobInput {{ team: [Shop_ItemInput] }}\ntype Shop_Ack {{ id: ID }}\ntype Query {{ shop_ping: String }}\ntype Mutation {{\n  shop_update(title: String, serviceId: ID, details: String, settings: Shop_SettingsInput, items: [Shop_ItemInput], job: Shop_JobInput, blob: Shop_JSON): Shop_Ack\n    @connect(source: \"shop\", http: {{ POST: \"/stores\", body: \"\"\"\n{}\n\"\"\" }}, selection: \"id\")\n}}",
        body
    );
    let ws = workspace(tag, op, write_shapes(), &sdl);
    build(&ws.0, POST_STORE).unwrap()
}

fn request_class(report: &Report, path: &str) -> String {
    report
        .request
        .iter()
        .find(|r| r.path == path)
        .map(|r| r.class.label())
        .unwrap_or_else(|| {
            panic!(
                "no request row {:?} in {:?}",
                path,
                report.request.iter().map(|r| &r.path).collect::<Vec<_>>()
            )
        })
}

/// pagerduty's `$({ incident: { … } })` body.
#[test]
fn a_nested_object_literal_maps_each_leaf_it_sends() {
    let r = write_report(
        "literal-nested",
        "$({\n  incident: {\n    type: \"incident\",\n    title: $args.title,\n    service: { id: $args.serviceId, type: \"service_reference\" },\n    body: $args.details->map({ details: @ })->first\n  }\n})",
    );
    assert_eq!(request_class(&r, "incident.type"), "mapped");
    assert_eq!(request_class(&r, "incident.title"), "mapped");
    assert_eq!(request_class(&r, "incident.service.id"), "mapped");
    assert_eq!(request_class(&r, "incident.service.type"), "mapped");
    // Not sent: honestly unaccounted, no longer hidden as unresolved.
    assert_eq!(request_class(&r, "incident.urgency"), "unaccounted");
    // A construct outside the grammar is unresolved at its own path only.
    assert_eq!(
        request_class(&r, "incident.body.details"),
        "unresolved (request expression not parsed: ->map->first)"
    );
}

#[test]
fn an_object_literal_on_a_position_offered_as_one_row_maps_that_row() {
    let r = write_report("literal-leaf", "$({ blob: { a: \"x\" } })");
    assert_eq!(request_class(&r, "blob"), "mapped");
}

/// Mailchimp's and FullStory's `key: $args.x { wire: field }` form.
#[test]
fn an_argument_sub_selection_maps_each_wire_key_it_reads() {
    let r = write_report(
        "subselection",
        "settings: $args.settings {\n  from_name: fromName\n  title\n  inner { a }\n}",
    );
    assert_eq!(request_class(&r, "settings.from_name"), "mapped");
    assert_eq!(request_class(&r, "settings.title"), "mapped");
    assert_eq!(request_class(&r, "settings.inner.a"), "mapped");
    assert_eq!(request_class(&r, "settings.inner.b"), "unaccounted");
    assert_eq!(request_class(&r, "settings.reply_to"), "unaccounted");
}

/// A body that is one argument sub-selection (Salesforce's
/// `$args.input { Name: name … }`), not a list of `key: expr` pairs: each wire
/// key is read at the request root. It used to yield no pairs, so every
/// offered path was unaccounted.
#[test]
fn a_whole_body_argument_sub_selection_maps_each_wire_key_at_the_root() {
    let r = write_report("body-subselection", "$args.job {\n  team { name }\n}");
    assert_eq!(request_class(&r, "team[].name"), "mapped");
    assert_eq!(request_class(&r, "team[].qty"), "unaccounted");
}

/// A body that forwards one whole argument (`body: "$args.job"`): the
/// argument's input type is matched against the request root.
#[test]
fn a_whole_body_argument_forward_maps_what_its_input_type_declares() {
    let r = write_report("body-forward", "$args.job");
    assert_eq!(request_class(&r, "team[].name"), "mapped");
    assert_eq!(request_class(&r, "team[].qty"), "mapped");
    assert_eq!(request_class(&r, "settings.title"), "unaccounted");
}

/// A sub-selection on a deeper argument path (Meta's
/// `targeting: $args.input.targeting { … }`), bare and inside an object
/// literal: the path is followed down the input type, then each wire key is
/// read. It used to be unresolved ("argument sub-selection").
#[test]
fn a_sub_selection_on_a_deeper_argument_path_reads_the_field_it_names() {
    for (tag, body) in [
        ("deep-bare", "team: $args.job.team { name }"),
        ("deep-literal", "$({ team: $args.job.team { name } })"),
    ] {
        let r = write_report(tag, body);
        assert_eq!(request_class(&r, "team[].name"), "mapped", "{}", tag);
        assert_eq!(request_class(&r, "team[].qty"), "unaccounted", "{}", tag);
    }
}

/// The deeper sub-selection walks the path with the same helper as the plain
/// `$args.a.b` forward (`arg_path_type`), so it stops at the same two
/// places. A misspelt segment is unresolved and names it, the typo test of
/// the plain path below in sub-selection form.
#[test]
fn a_sub_selection_on_a_deeper_path_naming_no_input_field_is_unresolved_and_says_so() {
    let r = write_report("deep-sub-typo", "settings: $args.settings.titel { a }");
    assert_eq!(
        request_class(&r, "settings.title"),
        "unresolved (request expression not parsed: `titel` is not a field of Shop_SettingsInput)"
    );
}

/// Past the JSON scalar the deeper sub-selection sends an opaque value, the
/// JSON test of the plain path below in sub-selection form.
#[test]
fn a_sub_selection_on_a_deeper_path_into_the_json_scalar_maps_as_json() {
    let r = write_report("deep-sub-json", "blob: $args.blob.a { x }");
    assert_eq!(request_class(&r, "blob"), "mapped (json)");
}

/// A sub-selection whose argument the field does not declare is unresolved
/// and names it, on a one-segment and a deeper head alike.
#[test]
fn a_sub_selection_on_an_undeclared_argument_is_unresolved_and_says_so() {
    for (tag, body) in [
        ("sub-noarg", "settings: $args.setings { title }"),
        ("deep-sub-noarg", "settings: $args.setings.inner { a }"),
    ] {
        let r = write_report(tag, body);
        assert_eq!(
            request_class(&r, "settings.title"),
            "unresolved (request expression not parsed: `setings` is not an argument of the field)",
            "{}",
            tag
        );
    }
}

/// A quoted alias in a sub-selection (Meta's `"100x100": _100x100`) sends
/// that wire key. The parser used to read the quoted key as a string value,
/// so the path it sends read unaccounted.
#[test]
fn a_quoted_alias_in_a_sub_selection_sends_its_wire_key() {
    let r = write_report(
        "quoted-alias",
        "settings: $args.settings {\n  \"from_name\": fromName\n  title\n}",
    );
    assert_eq!(request_class(&r, "settings.from_name"), "mapped");
    assert_eq!(request_class(&r, "settings.title"), "mapped");
    assert_eq!(request_class(&r, "settings.reply_to"), "unaccounted");
}

#[test]
fn a_sub_selection_on_a_list_argument_reads_the_items() {
    let r = write_report("subselection-list", "items: $args.items { name }");
    assert_eq!(request_class(&r, "items[].name"), "mapped");
    assert_eq!(request_class(&r, "items[].qty"), "unaccounted");
}

#[test]
fn a_sub_selection_entry_naming_no_input_field_is_unresolved_and_says_so() {
    let r = write_report(
        "subselection-bad",
        "settings: $args.settings { title: headline }",
    );
    assert_eq!(
        request_class(&r, "settings.title"),
        "unresolved (request expression not parsed: `headline` is not a field of Shop_SettingsInput)"
    );
}

/// A vendor's `hiringTeam: $args.job.hiringTeam`: a deeper argument path
/// forwards the input field it names, children included.
#[test]
fn a_deeper_argument_path_forwards_the_field_it_names() {
    let r = write_report("deep-path", "$({ team: $args.job.team })");
    assert_eq!(request_class(&r, "team[].name"), "mapped");
    assert_eq!(request_class(&r, "team[].qty"), "mapped");
}

/// A misspelt segment is the same mistake the sub-selection branch reports.
#[test]
fn a_deeper_argument_path_naming_no_input_field_is_unresolved_and_says_so() {
    let r = write_report(
        "deep-path-typo",
        "$({ settings: { title: $args.settings.titel } })",
    );
    assert_eq!(
        request_class(&r, "settings.title"),
        "unresolved (request expression not parsed: `titel` is not a field of Shop_SettingsInput)"
    );
}

/// Past the JSON scalar there is no declared structure to check against.
#[test]
fn a_deeper_argument_path_into_the_json_scalar_maps_as_json() {
    let r = write_report("deep-path-json", "$({ blob: $args.blob.a })");
    assert_eq!(request_class(&r, "blob"), "mapped (json)");
}

/// v0.3: a nested `{ }` in a bare body is a sub-selection, whose entries
/// only whitespace separates, on separate lines or on one.
#[test]
fn a_whitespace_separated_nested_literal_in_a_bare_body_is_read_entry_by_entry() {
    for (tag, body) in [
        (
            "ws-nested-lines",
            "title: $args.title\nsettings: {\n  from_name: $args.title\n  reply_to: $args.title\n}",
        ),
        (
            "ws-nested-line",
            "title: $args.title\nsettings: { from_name: $args.title reply_to: $args.title }",
        ),
    ] {
        let r = write_report(tag, body);
        assert_eq!(request_class(&r, "settings.from_name"), "mapped", "{}", tag);
        assert_eq!(request_class(&r, "settings.reply_to"), "mapped", "{}", tag);
    }
}

/// v0.4: a bare braced body may leave its commas out.
#[test]
fn a_braced_body_without_commas_is_read_entry_by_entry() {
    let r = write_report(
        "ws-braced",
        "{\n  settings: $args.settings\n  items: $args.items\n}",
    );
    assert_eq!(request_class(&r, "settings.title"), "mapped");
    assert_eq!(request_class(&r, "items[].name"), "mapped");
}

/// Commas on some lines and not on others: each line is still one entry.
#[test]
fn a_body_mixing_commas_and_newlines_is_read_entry_by_entry() {
    let r = write_report(
        "mixed-separators",
        "$({\n  settings: { title: $args.title, from_name: $args.title\n    reply_to: $args.title },\n  items: $args.items\n})",
    );
    for path in [
        "settings.title",
        "settings.from_name",
        "settings.reply_to",
        "items[].name",
    ] {
        assert_eq!(request_class(&r, path), "mapped", "{}", path);
    }
}

/// A quoted key is read without its quotes, and still starts an entry when
/// only whitespace separates it from the one before.
#[test]
fn a_quoted_key_starts_an_entry() {
    let r = write_report(
        "quoted-key",
        "$({ settings: { \"from_name\": $args.title\n  'title': $args.title } })",
    );
    assert_eq!(request_class(&r, "settings.from_name"), "mapped");
    assert_eq!(request_class(&r, "settings.title"), "mapped");
}

/// `$("…")` is a literal at every connect version; `$("a") ?? $("b")` is
/// two groups and a fallback, not one literal.
#[test]
fn a_dollar_wrapped_literal_maps_and_a_fallback_between_two_does_not() {
    let r = write_report(
        "dollar-literal",
        "$({ settings: { from_name: $(\"Shop\"), title: $(\"a\") ?? $(\"b\") } })",
    );
    assert_eq!(request_class(&r, "settings.from_name"), "mapped");
    assert_eq!(
        request_class(&r, "settings.title"),
        "unresolved (request expression not parsed: `??` fallback)"
    );
}

/// An apostrophe in a `#` comment must not open a string that swallows the
/// pairs after it.
#[test]
fn a_comment_in_a_body_is_ignored() {
    let r = write_report(
        "comment",
        "$({\n  # don't forget: settings\n  settings: { title: $args.title },\n  items: $args.items\n})",
    );
    assert_eq!(request_class(&r, "settings.title"), "mapped");
    assert_eq!(request_class(&r, "items[].name"), "mapped");
}

/// A `"` inside a single-quoted string opens nothing: the commented-out
/// entry after it stays a comment, not a mapped path.
#[test]
fn a_double_quote_inside_a_single_quoted_string_does_not_hide_a_comment() {
    let r = write_report(
        "comment-after-quote",
        "settings: {\n  title: '5\" wide'\n  # from_name: $args.title\n}",
    );
    assert_eq!(request_class(&r, "settings.title"), "mapped");
    assert_eq!(request_class(&r, "settings.from_name"), "unaccounted");
}

/// A `#` inside a single-quoted string is not a comment.
#[test]
fn a_hash_inside_a_single_quoted_string_is_not_a_comment() {
    let r = write_report(
        "hash-in-string",
        "$({ settings: { title: $('Issue #1') }, items: $args.items })",
    );
    assert_eq!(request_class(&r, "settings.title"), "mapped");
    assert_eq!(request_class(&r, "items[].name"), "mapped");
}

#[test]
fn a_sub_selection_entry_with_a_method_and_no_alias_is_unresolved_and_names_it() {
    let r = write_report(
        "subselection-method",
        "settings: $args.settings { title->trim }",
    );
    assert_eq!(
        request_class(&r, "settings.title"),
        "unresolved (request expression not parsed: ->trim)"
    );
}

#[test]
fn a_fallback_to_an_object_literal_is_named_as_a_fallback() {
    let r = write_report(
        "fallback-object",
        "$({ settings: $args.settings ?? { title: \"x\" } })",
    );
    assert_eq!(
        request_class(&r, "settings.title"),
        "unresolved (request expression not parsed: `??` fallback)"
    );
}

/// Mailchimp's `conditions` sub-selection over an item the request shape
/// offers as one row: the sub-selection sends that row.
#[test]
fn a_sub_selection_on_a_position_offered_as_one_row_maps_that_row() {
    let r = write_report("subselection-leaf", "blob: $args.settings { title }");
    assert_eq!(request_class(&r, "blob"), "mapped");
}

/// A sub-selection over a recursive array shape (`Tree = [Tree]`) steps into
/// the items once, meets the shape it is already inside, and maps the row the
/// walk offers there, rather than recursing until the stack overflows.
#[test]
fn a_sub_selection_on_a_recursive_array_shape_terminates() {
    let r = write_report("subselection-tree", "tree: $args.settings { title }");
    assert_eq!(request_class(&r, "tree[]"), "mapped");
}

/// The same cycle through a union: a JSON-value shape whose only structured
/// variant is an array of itself.
#[test]
fn a_sub_selection_on_a_recursive_array_union_terminates() {
    let r = write_report("subselection-json", "value: $args.settings { title }");
    assert_eq!(request_class(&r, "value[]"), "mapped");
}

// ─── Recursion through a nullable reference ─────────────────────────────────

/// Omni's `JsonValue`: every self-reference is wrapped as
/// `oneOf: [{$ref}, {type: null}]`, so it carries no bare `$ref` for a cycle
/// guard keyed on the reference name to see.
fn json_value_shapes() -> Value {
    json!({
        "Doc": {"type": "object", "properties": {
            "id": {"type": "string"},
            "value": {"$ref": "#/shapes/JsonValue"}
        }},
        "JsonValue": {"anyOf": [
            {"type": "string"},
            {"type": "null"},
            {"type": "object", "additionalProperties": {"oneOf": [{"$ref": "#/shapes/JsonValue"}, {"type": "null"}]}},
            {"type": "array", "items": {"oneOf": [{"$ref": "#/shapes/JsonValue"}, {"type": "null"}]}}
        ]}
    })
}

#[test]
fn a_response_shape_recursing_through_a_nullable_reference_terminates() {
    let op = json!({"key": GET_STORE, "operation_id": "getStore", "method": "GET", "path": "/stores/{id}",
        "response": {"status": "200", "shape_ref": "#/shapes/Doc"}});
    let sdl = "type Shop_Doc { id: ID value: Shop_JSON }\ntype Query {\n  shop_doc(id: ID!): Shop_Doc\n    @connect(source: \"shop\", http: { GET: \"/stores/{$args.id}\" }, selection: \"id value\")\n}";
    let ws = workspace("nullable-ref-response", op, json_value_shapes(), sdl);
    let r = build(&ws.0, GET_STORE).unwrap();
    assert_eq!(response_class(&r, "id"), "mapped");
    // `value` is selected whole, so the row the walk stops at is JSON.
    assert_eq!(response_class(&r, "value[]"), "mapped (json)");
}

#[test]
fn a_sub_selection_over_a_nullable_recursive_reference_terminates() {
    let op = json!({"key": POST_STORE, "operation_id": "update", "method": "POST", "path": "/stores",
        "request_body": {"content_type": "application/json", "shape_ref": "#/shapes/Doc"},
        "response": {"status": "200", "shape_ref": "#/shapes/Doc"}});
    let sdl = "input Shop_SettingsInput { title: String }\ntype Shop_Doc { id: ID }\ntype Query { shop_ping: String }\ntype Mutation {\n  shop_update(settings: Shop_SettingsInput): Shop_Doc\n    @connect(source: \"shop\", http: { POST: \"/stores\", body: \"\"\"\nvalue: $args.settings { title }\n\"\"\" }, selection: \"id\")\n}";
    let ws = workspace("nullable-ref-request", op, json_value_shapes(), sdl);
    let r = build(&ws.0, POST_STORE).unwrap();
    assert_eq!(request_class(&r, "value[]"), "mapped");
}

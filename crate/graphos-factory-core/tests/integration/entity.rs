//! Entity rules (ADR 0076): `entity-without-lookup`,
//! `entity-key-not-embedded`, `entity-without-consumer`,
//! `entity-field-unresolved`. Pinned on the petstore-batch spec (gen
//! 8df53f4, three entities: Pet, Order, User) and the stay-listings fixture
//! (Listing, and Amenity keyed through its
//! `$batch` lookup), with one negative per rule. The review tests (ADR
//! 0076 § Review) use small inline inventories: a `$ref` cycle, a
//! `resolvable: false` stub, embeddings read through the selection, two
//! keys in either order, and lookups that are not by-id.

use graphos_factory_core::entity::check;
use serde_json::{json, Value};
use std::path::Path;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn petstore_inventory() -> Value {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("inventory.json");
    let code = graphos_factory_core::cmd::inventory::main(&[
        "build".to_string(),
        fixture("petstore-batch/petstore-batch.yaml")
            .to_string_lossy()
            .to_string(),
        "--out".to_string(),
        out.to_string_lossy().to_string(),
    ]);
    assert_eq!(code, 0);
    serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap()
}

const PETSTORE_WORKSPACE: &str =
    "contract_version: 1\nservice: petstore\ndirectory: petstore\ntype_prefix: Petstore\nfield_prefix: petstore\n";

fn petstore_selection(pet_included: bool) -> Value {
    graphos_factory_core::yaml::parse(&format!(
        "contract_version: 1\noperations:\n  \"get:/pet/{{petId}}\":\n    include: {}\n    graphql: {{ root: query, name: pet, entity: true, key: id }}\n  \"get:/store/order/{{orderId}}\":\n    include: true\n    graphql: {{ root: query, name: order, entity: true, key: id }}\n  \"get:/user/{{username}}\":\n    include: true\n    graphql: {{ root: query, name: user, entity: true, key: username }}\n  \"get:/pet/findByStatus\":\n    include: true\n    graphql: {{ root: query, name: petsByStatus }}\n",
        pet_included
    ))
    .unwrap()
}

/// Three entities; `@PET_FIELDS@`, `@ORDER_PET@` and `@PET_ROOT@` let a
/// test change one thing.
const PETSTORE_SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])
  @source(name: "petstore", http: { baseURL: "{{BASE_URL}}" })

type Petstore_Pet @key(fields: "id") {
  id: ID!
  name: String!
  status: String@PET_FIELDS@
}

type Petstore_Order @key(fields: "id") {
  id: ID!
  quantity: Int@ORDER_PET@
}

type Petstore_User
  @key(fields: "username")
  @connect(
    source: "petstore"
    http: { GET: "/user/findByNames", queryParams: "username: $batch.username" }
    selection: "username email"
  ) {
  username: String!
  email: String
}

type Query {
@PET_ROOT@  petstore_petsByStatus(status: String): [Petstore_Pet]
    @connect(source: "petstore", http: { GET: "/pet/findByStatus", queryParams: "status: $args.status" }, selection: "id name status")
  petstore_order(orderId: ID!): Petstore_Order
    @connect(source: "petstore", http: { GET: "/store/order/{$args.orderId}" }, selection: "id quantity@ORDER_SEL@")
  petstore_user(username: String!): Petstore_User
    @connect(source: "petstore", http: { GET: "/user/{$args.username}" }, selection: "username email")
}
"#;

const PET_ROOT: &str = "  petstore_pet(petId: ID!): Petstore_Pet\n    @connect(source: \"petstore\", http: { GET: \"/pet/{$args.petId}\" }, selection: \"id name status\")\n";

fn sdl(pet_fields: &str, order_pet: bool, pet_root: bool) -> String {
    PETSTORE_SDL
        .replace("@PET_FIELDS@", pet_fields)
        .replace(
            "@ORDER_PET@",
            if order_pet {
                "\n  pet: Petstore_Pet"
            } else {
                ""
            },
        )
        .replace(
            "@ORDER_SEL@",
            if order_pet { " pet: { id: petId }" } else { "" },
        )
        .replace("@PET_ROOT@", if pet_root { PET_ROOT } else { "" })
}

fn rules(findings: &[graphos_factory_core::entity::EntityFinding]) -> Vec<(&'static str, String)> {
    findings.iter().map(|(_, r, m)| (*r, m.clone())).collect()
}

fn workspace() -> Value {
    graphos_factory_core::yaml::parse(PETSTORE_WORKSPACE).unwrap()
}

#[test]
fn the_three_petstore_entities_pass() {
    let inv = petstore_inventory();
    let f = check(
        &sdl("", true, true),
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    );
    assert!(f.is_empty(), "{:?}", f);
}

#[test]
fn an_entity_no_selected_operation_returns_by_key_has_no_lookup() {
    // get:/pet/{petId} is excluded; findByStatus lists pets but does not
    // take their ids, so batch find rates Pet list-no-key-filter.
    let inv = petstore_inventory();
    let f = rules(&check(
        &sdl("", true, false),
        &workspace(),
        Some(&petstore_selection(false)),
        Some(&inv),
    ));
    let lookup: Vec<&String> = f
        .iter()
        .filter(|(r, _)| *r == "entity-without-lookup")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(lookup.len(), 1, "{:?}", f);
    assert!(
        lookup[0].starts_with("Petstore_Pet carries @key(fields: \"id\")"),
        "{}",
        lookup[0]
    );
}

#[test]
fn an_embedding_without_the_key_is_an_error_and_with_it_is_not() {
    // Order now embeds `pet`, and its connector reads it as `pet { id name }`;
    // the embedded shape carries only `name`.
    let mut inv = petstore_inventory();
    inv["shapes"]["PetRef"] =
        json!({ "type": "object", "properties": { "name": { "type": "string" } } });
    inv["shapes"]["Order"]["properties"]["pet"] = json!({ "$ref": "#/shapes/PetRef" });
    let reads_pet = sdl("", true, true).replace(" pet: { id: petId }", " pet { id name }");
    // The literal `pet: { id: petId }` builds the reference from Order's own
    // `petId`, not from the embedded shape: no finding.
    let f = check(
        &sdl("", true, true),
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    );
    assert!(f.is_empty(), "{:?}", f);
    let f = rules(&check(
        &reads_pet,
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    ));
    let embedded: Vec<&String> = f
        .iter()
        .filter(|(r, _)| *r == "entity-key-not-embedded")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(embedded.len(), 1, "{:?}", f);
    assert!(
        embedded[0].starts_with("get:/store/order/{orderId} embeds Petstore_Pet at Order.pet")
            && embedded[0].ends_with(
                "lacks the key id: the router cannot turn that embedding into a reference"
            ),
        "{}",
        embedded[0]
    );
    inv["shapes"]["PetRef"]["properties"]["id"] = json!({ "type": "integer" });
    let f = check(
        &reads_pet,
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    );
    assert!(f.is_empty(), "{:?}", f);
}

#[test]
fn an_entity_nothing_returns_has_no_consumer() {
    // No root field and no other type returns Pet; the by-id lookup stays.
    let inv = petstore_inventory();
    let full = sdl("", false, false);
    let line = full
        .lines()
        .find(|l| l.contains("petstore_petsByStatus"))
        .unwrap()
        .to_string();
    let s = full.replace(&format!("{}\n", line), "");
    assert!(!s.contains("petstore_petsByStatus"));
    let s = s.replace("    @connect(source: \"petstore\", http: { GET: \"/pet/findByStatus\", queryParams: \"status: $args.status\" }, selection: \"id name status\")\n", "");
    let f = rules(&check(
        &s,
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    ));
    let consumer: Vec<&String> = f
        .iter()
        .filter(|(r, _)| *r == "entity-without-consumer")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(consumer.len(), 1, "{:?}", f);
    assert!(
        consumer[0].starts_with("Petstore_Pet carries @key but nothing in this schema returns it")
    );
}

#[test]
fn a_field_no_connector_maps_is_unresolved_unless_it_has_its_own() {
    let inv = petstore_inventory();
    let f = rules(&check(
        &sdl("\n  nickname: String", true, true),
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    ));
    let unresolved: Vec<&String> = f
        .iter()
        .filter(|(r, _)| *r == "entity-field-unresolved")
        .map(|(_, m)| m)
        .collect();
    assert_eq!(unresolved.len(), 1, "{:?}", f);
    assert!(
        unresolved[0].starts_with("Petstore_Pet: nickname is mapped by no connector"),
        "{}",
        unresolved[0]
    );
    // A field-level connector of its own resolves it.
    let own = "\n  nickname: String\n    @connect(source: \"petstore\", http: { GET: \"/pet/{$this.id}/nickname\" }, selection: \"$\")";
    let f = check(
        &sdl(own, true, true),
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    );
    assert!(f.is_empty(), "{:?}", f);
}

fn stay_listings() -> (Value, Value, Value, String) {
    let dir = fixture("stay-listings");
    let read = |rel: &str| std::fs::read_to_string(dir.join(rel)).unwrap();
    (
        graphos_factory_core::yaml::parse(&read(".factory/workspace.yaml")).unwrap(),
        graphos_factory_core::yaml::parse(&read(".factory/selection.yaml")).unwrap(),
        serde_json::from_str(&read(".factory/inventory.json")).unwrap(),
        read("stay-listings.graphql"),
    )
}

#[test]
fn a_listing_passes_and_so_does_amenity_once_keyed_and_batched() {
    let (ws, sel, inv, sdl) = stay_listings();
    let f = check(&sdl, &ws, Some(&sel), Some(&inv));
    assert!(f.is_empty(), "Listing: {:?}", f);
    // Amenity keyed with the $batch connector ADR 0071 drafts: its lookup is
    // batch find's `GET /amenities?ids=` (batchable), the listings embed it
    // with its id, and the connector maps all three fields.
    let header = "type Stay_Listings_Amenity {";
    assert_eq!(sdl.matches(header).count(), 1);
    let keyed = sdl.replace(
        header,
        "type Stay_Listings_Amenity\n  @key(fields: \"id\")\n  @connect(\n    source: \"stay_listings\"\n    http: { GET: \"/amenities\", queryParams: \"ids: $batch.id->joinNotNull(',')\" }\n    selection: \"id category name\"\n  ) {",
    );
    let f = check(&keyed, &ws, Some(&sel), Some(&inv));
    assert!(f.is_empty(), "Amenity: {:?}", f);
}

#[test]
fn a_body_property_named_for_the_key_is_a_lookup() {
    // An RPC-style API: `POST /candidate.info` with `{id}` in the body, answering a
    // oneOf of success and error; the lookup is real though no path or
    // query parameter carries the key.
    let inv = json!({
        "contract_version": 1, "api": {},
        "operations": [{
            "key": "post:/candidate.info", "method": "POST", "path": "/candidate.info", "parameters": [],
            "request_body": { "content_type": "application/json", "required": true, "shape_ref": "#/shapes/InfoRequest" },
            "response": { "status": "200", "content_type": "application/json", "shape_ref": "#/shapes/InfoResponse" }
        }],
        "shapes": {
            "InfoRequest": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] },
            "Candidate": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } },
            "InfoSuccess": { "type": "object", "properties": { "success": { "type": "boolean" }, "results": { "$ref": "#/shapes/Candidate" } } },
            "ErrorResponse": { "type": "object", "properties": { "errors": { "type": "array", "items": { "type": "string" } } } },
            "InfoResponse": { "oneOf": [{ "$ref": "#/shapes/InfoSuccess" }, { "$ref": "#/shapes/ErrorResponse" }] }
        },
        "unresolved": []
    });
    let sel = graphos_factory_core::yaml::parse(
        "contract_version: 1\noperations:\n  \"post:/candidate.info\":\n    include: true\n    graphql: { root: query, name: candidate, entity: true, key: id }\n",
    )
    .unwrap();
    let ws = graphos_factory_core::yaml::parse("contract_version: 1\nservice: hire_co\ndirectory: hire-co\ntype_prefix: Hire_Co\nfield_prefix: hire_co\n").unwrap();
    let sdl = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])
  @source(name: "hire_co", http: { baseURL: "{{BASE_URL}}" })
type Hire_Co_Candidate @key(fields: "id") {
  id: ID!
  name: String
}
type Query {
  hire_co_candidate(id: ID!): Hire_Co_Candidate
    @connect(source: "hire_co", http: { POST: "/candidate.info", body: "id: $args.id" }, selection: "$.results { id name }")
}
"#;
    let f = check(sdl, &ws, Some(&sel), Some(&inv));
    assert!(f.is_empty(), "{:?}", f);
    // Without the body property, nothing carries the key.
    let mut bare = inv.clone();
    bare["shapes"]["InfoRequest"]["properties"] = json!({ "limit": { "type": "integer" } });
    let f = check(sdl, &ws, Some(&sel), Some(&bare));
    assert!(
        f.iter().any(|(_, r, _)| *r == "entity-without-lookup"),
        "{:?}",
        f
    );
}

// ─── Review fixes (ADR 0076, Consequences § Review) ─────────────────────────

const CYC_WORKSPACE: &str =
    "contract_version: 1\nservice: cyc\ndirectory: cyc\ntype_prefix: Cyc\nfield_prefix: cyc\n";

fn op(key: &str, params: Value, body: Option<&str>, response: &str) -> Value {
    let (method, path) = key.split_once(':').unwrap();
    json!({
        "key": key, "method": method.to_ascii_uppercase(), "path": path, "parameters": params,
        "request_body": body.map(|b| json!({ "content_type": "application/json", "required": true, "shape_ref": format!("#/shapes/{}", b) })),
        "response": { "status": "200", "content_type": "application/json", "shape_ref": format!("#/shapes/{}", response) }
    })
}

fn path_param(name: &str) -> Value {
    json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } })
}

fn cyc_inventory(ops: Vec<Value>, shapes: Value) -> Value {
    json!({ "contract_version": 1, "api": {}, "operations": ops, "shapes": shapes, "unresolved": [] })
}

fn cyc_selection(entries: &[(&str, &str, &str)]) -> Value {
    let mut y = "contract_version: 1\noperations:\n".to_string();
    for (key, root, name) in entries {
        y.push_str(&format!(
            "  \"{}\":\n    include: true\n    graphql: {{ root: {}, name: {} }}\n",
            key, root, name
        ));
    }
    graphos_factory_core::yaml::parse(&y).unwrap()
}

const CYC_HEAD: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])
  @source(name: "cyc", http: { baseURL: "{{BASE_URL}}" })
"#;

#[test]
fn a_polymorphic_request_body_is_walked_once() {
    // `PetInput: oneOf [CatInput]`, `CatInput: allOf [PetInput, …]`: the
    // standard polymorphism inventory build emits as a `$ref` cycle. The
    // body walk ran forever on it; bounded here so a regression fails
    // instead of hanging the suite.
    let inv = cyc_inventory(
        vec![
            op("post:/pets.search", json!([]), Some("PetInput"), "Pet"),
            op(
                "get:/pets/{petId}",
                json!([path_param("petId")]),
                None,
                "Pet",
            ),
        ],
        json!({
            "PetInput": { "oneOf": [{ "$ref": "#/shapes/CatInput" }] },
            "CatInput": { "allOf": [{ "$ref": "#/shapes/PetInput" }, { "type": "object", "properties": { "meow": { "type": "string" } } }] },
            "Pet": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } }
        }),
    );
    // Only the search is selected, as a read: its body is what gets walked.
    let sel = cyc_selection(&[("post:/pets.search", "query", "searchPets")]);
    let sdl = format!(
        "{}type Cyc_Pet @key(fields: \"id\") {{\n  id: ID!\n  name: String\n}}\ntype Query {{\n  cyc_searchPets(meow: String): [Cyc_Pet]\n    @connect(source: \"cyc\", http: {{ POST: \"/pets.search\", body: \"meow: $args.meow\" }}, selection: \"id name\")\n}}\n",
        CYC_HEAD
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ws = graphos_factory_core::yaml::parse(CYC_WORKSPACE).unwrap();
        tx.send(check(&sdl, &ws, Some(&sel), Some(&inv))).unwrap();
    });
    let f = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("entity::check did not return within 20s on a $ref cycle");
    // It terminates, and the search (no key in the cyclic body) is no lookup.
    let f = rules(&f);
    assert_eq!(
        f.iter()
            .filter(|(r, _)| *r == "entity-without-lookup")
            .count(),
        1,
        "{:?}",
        f
    );
}

fn tag_sdl(pet_has_tags: bool) -> String {
    let sdl = sdl("", true, true)
        .replace(
            "type Petstore_Order",
            "type Petstore_Tag @key(fields: \"id\", resolvable: false) {\n  id: ID!\n}\n\ntype Petstore_Order",
        );
    if pet_has_tags {
        sdl.replace(
            "  status: String\n",
            "  status: String\n  tags: [Petstore_Tag]\n",
        )
        .replace(
            "selection: \"id name status\"",
            "selection: \"id name status tags { id }\"",
        )
    } else {
        sdl
    }
}

#[test]
fn a_resolvable_false_stub_needs_no_lookup_but_still_a_consumer() {
    // Tag is another subgraph's entity: `@key(fields: "id", resolvable:
    // false)` lets Pet reference it by id. No operation here returns a tag
    // by id, and none has to.
    let inv = petstore_inventory();
    let f = check(
        &tag_sdl(true),
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    );
    assert!(f.is_empty(), "{:?}", f);
    // A stub nothing references is still dead weight: the consumer warning
    // stays, and no lookup error joins it.
    let f = rules(&check(
        &tag_sdl(false),
        &workspace(),
        Some(&petstore_selection(true)),
        Some(&inv),
    ));
    assert_eq!(
        f.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        vec!["entity-without-consumer"],
        "{:?}",
        f
    );
    assert!(f[0].1.starts_with("Petstore_Tag carries @key"), "{:?}", f);
}

fn visit_case(visit_embeds: &str, pet_ref: Value, selection: &str) -> Vec<(&'static str, String)> {
    let inv = cyc_inventory(
        vec![
            op(
                "get:/pets/{petId}",
                json!([path_param("petId")]),
                None,
                "Pet",
            ),
            op(
                "get:/visits/{visitId}",
                json!([path_param("visitId")]),
                None,
                "Visit",
            ),
        ],
        json!({
            "Pet": { "type": "object", "properties": { "pet_id": { "type": "string" }, "name": { "type": "string" } } },
            "PetRef": { "type": "object", "properties": pet_ref },
            "Visit": { "type": "object", "properties": { "id": { "type": "string" }, visit_embeds: { "$ref": "#/shapes/PetRef" } } }
        }),
    );
    let sel = cyc_selection(&[
        ("get:/pets/{petId}", "query", "pet"),
        ("get:/visits/{visitId}", "query", "visit"),
    ]);
    let sdl = format!(
        "{}type Cyc_Pet @key(fields: \"id\") {{\n  id: ID!\n  name: String\n}}\ntype Cyc_Visit {{\n  id: ID!\n  pet: Cyc_Pet\n}}\ntype Query {{\n  cyc_pet(petId: ID!): Cyc_Pet\n    @connect(source: \"cyc\", http: {{ GET: \"/pets/{{$args.petId}}\" }}, selection: \"id: pet_id name\")\n  cyc_visit(visitId: ID!): Cyc_Visit\n    @connect(source: \"cyc\", http: {{ GET: \"/visits/{{$args.visitId}}\" }}, selection: \"{}\")\n}}\n",
        CYC_HEAD, selection
    );
    let ws = graphos_factory_core::yaml::parse(CYC_WORKSPACE).unwrap();
    rules(&check(&sdl, &ws, Some(&sel), Some(&inv)))
}

#[test]
fn the_embedded_key_is_read_through_the_connectors_selection() {
    let full = json!({ "pet_id": { "type": "string" }, "name": { "type": "string" } });
    let bare = json!({ "name": { "type": "string" } });
    // The wire says `pet_id`; the selection maps it to the key `id`.
    let f = visit_case("pet", full.clone(), "id pet { id: pet_id name }");
    assert!(f.is_empty(), "{:?}", f);
    // The GraphQL field `pet` reads the wire property `animal`: the
    // embedding is found through the alias, and the key checked there.
    let f = visit_case("animal", full, "id pet: animal { id: pet_id name }");
    assert!(f.is_empty(), "{:?}", f);
    let f = visit_case("animal", bare.clone(), "id pet: animal { id: pet_id name }");
    assert_eq!(f.len(), 1, "{:?}", f);
    assert_eq!(f[0].0, "entity-key-not-embedded");
    assert!(
        f[0].1
            .starts_with("get:/visits/{visitId} embeds Cyc_Pet at Visit.pet (Cyc_Visit), but the embedded shape lacks the key id"),
        "{}",
        f[0].1
    );
    // A selection that does not produce the key at all lacks it too.
    let f = visit_case("pet", bare.clone(), "id pet { name }");
    assert_eq!(
        f.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        vec!["entity-key-not-embedded"],
        "{:?}",
        f
    );
    // A key the reader cannot judge (`$this`, a literal) is not a finding.
    let f = visit_case("pet", bare, "id pet { id: $this.id name }");
    assert!(f.is_empty(), "{:?}", f);
}

#[test]
fn every_key_is_read_whatever_the_directive_order() {
    // Pet gains a second key, `name`, with no lookup of its own; Order's
    // `pet { id }` embedding carries `id` only, which is one full key.
    let two = |first: &str, second: &str| {
        let s = sdl("", true, true)
            .replace(
                "type Petstore_Pet @key(fields: \"id\")",
                &format!(
                    "type Petstore_Pet @key(fields: \"{}\") @key(fields: \"{}\")",
                    first, second
                ),
            )
            .replace(" pet: { id: petId }", " pet { id }");
        let mut inv = petstore_inventory();
        inv["shapes"]["PetRef"] =
            json!({ "type": "object", "properties": { "id": { "type": "integer" } } });
        inv["shapes"]["Order"]["properties"]["pet"] = json!({ "$ref": "#/shapes/PetRef" });
        rules(&check(
            &s,
            &workspace(),
            Some(&petstore_selection(true)),
            Some(&inv),
        ))
    };
    let a = two("id", "name");
    let b = two("name", "id");
    assert_eq!(a, b);
    assert_eq!(a.len(), 1, "{:?}", a);
    assert_eq!(a[0].0, "entity-without-lookup");
    assert!(
        a[0].1
            .starts_with("Petstore_Pet carries @key(fields: \"name\")"),
        "{}",
        a[0].1
    );
}

#[test]
fn a_scope_parameter_or_a_create_body_is_no_lookup() {
    let shapes = json!({
        "Pet": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } },
        "PetInput": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } },
        "StatusUpdate": { "type": "object", "properties": { "gid": { "type": "string" } } }
    });
    let ws = graphos_factory_core::yaml::parse(CYC_WORKSPACE).unwrap();
    let pet_sdl = |root: &str, field: &str, http: &str| {
        format!(
            "{}type Cyc_Pet @key(fields: \"id\") {{\n  id: ID!\n  name: String\n}}\ntype {} {{\n  {}: [Cyc_Pet]\n    @connect(source: \"cyc\", http: {{ {} }}, selection: \"id name\")\n}}\n",
            CYC_HEAD, root, field, http
        )
    };
    let lookup_errors = |f: Vec<graphos_factory_core::entity::EntityFinding>| {
        f.iter()
            .filter(|(_, r, _)| *r == "entity-without-lookup")
            .count()
    };
    // `org_id` ends with `id`, but it scopes the list; it names no pet.
    let inv = cyc_inventory(
        vec![op(
            "get:/orgs/{org_id}/pets",
            json!([path_param("org_id")]),
            None,
            "Pet",
        )],
        shapes.clone(),
    );
    let sel = cyc_selection(&[("get:/orgs/{org_id}/pets", "query", "orgPets")]);
    let s = pet_sdl(
        "Query",
        "cyc_orgPets(orgId: ID!)",
        "GET: \"/orgs/{$args.orgId}/pets\"",
    );
    assert_eq!(lookup_errors(check(&s, &ws, Some(&sel), Some(&inv))), 1);
    // A create's body carries `id`, but a write resolves no reference.
    let inv = cyc_inventory(
        vec![op("post:/pets", json!([]), Some("PetInput"), "Pet")],
        shapes.clone(),
    );
    let sel = cyc_selection(&[("post:/pets", "mutation", "createPet")]);
    let s = pet_sdl(
        "Mutation",
        "cyc_createPet(id: ID!)",
        "POST: \"/pets\", body: \"id: $args.id\"",
    );
    assert_eq!(lookup_errors(check(&s, &ws, Some(&sel), Some(&inv))), 1);
    // The same body on a confirmed read is an RPC-style lookup, and a by-id path
    // whose parameter ends with the key is Asana's `status_gid`.
    let inv = cyc_inventory(
        vec![
            op("post:/pets.info", json!([]), Some("PetInput"), "Pet"),
            op(
                "get:/status_updates/{status_gid}",
                json!([path_param("status_gid")]),
                None,
                "StatusUpdate",
            ),
        ],
        shapes,
    );
    let sel = cyc_selection(&[
        ("post:/pets.info", "query", "pet"),
        ("get:/status_updates/{status_gid}", "query", "statusUpdate"),
    ]);
    let s = format!(
        "{}type Cyc_Pet @key(fields: \"id\") {{\n  id: ID!\n  name: String\n}}\ntype Cyc_StatusUpdate @key(fields: \"gid\") {{\n  gid: ID!\n}}\ntype Query {{\n  cyc_pet(id: ID!): Cyc_Pet\n    @connect(source: \"cyc\", http: {{ POST: \"/pets.info\", body: \"id: $args.id\" }}, selection: \"id name\")\n  cyc_statusUpdate(gid: ID!): Cyc_StatusUpdate\n    @connect(source: \"cyc\", http: {{ GET: \"/status_updates/{{$args.gid}}\" }}, selection: \"gid\")\n}}\n",
        CYC_HEAD
    );
    let f = check(&s, &ws, Some(&sel), Some(&inv));
    assert!(f.is_empty(), "{:?}", f);
}

#[test]
fn an_embedding_under_an_envelope_is_read_where_the_selection_reads_it() {
    // An envelope API's shape: `{success, results: Visit}`, the connector reads
    // `$.results { … pet: { id: petId } }`. The literal reads `petId` from
    // `results`, not from the wrapper, which has no `petId` of its own.
    let case = |visit_props: Value| {
        let inv = cyc_inventory(
            vec![
                op(
                    "get:/pets/{petId}",
                    json!([path_param("petId")]),
                    None,
                    "Pet",
                ),
                op("post:/visit.info", json!([]), None, "VisitInfo"),
            ],
            json!({
                "Pet": { "type": "object", "properties": { "id": { "type": "string" }, "name": { "type": "string" } } },
                "Visit": { "type": "object", "properties": visit_props },
                "VisitInfo": { "type": "object", "properties": { "success": { "type": "boolean" }, "results": { "$ref": "#/shapes/Visit" } } }
            }),
        );
        let sel = cyc_selection(&[
            ("get:/pets/{petId}", "query", "pet"),
            ("post:/visit.info", "query", "visit"),
        ]);
        let sdl = format!(
            "{}type Cyc_Pet @key(fields: \"id\") {{\n  id: ID!\n  name: String\n}}\ntype Cyc_Visit {{\n  id: ID!\n  pet: Cyc_Pet\n}}\ntype Query {{\n  cyc_pet(petId: ID!): Cyc_Pet\n    @connect(source: \"cyc\", http: {{ GET: \"/pets/{{$args.petId}}\" }}, selection: \"id name\")\n  cyc_visit(id: ID!): Cyc_Visit\n    @connect(source: \"cyc\", http: {{ POST: \"/visit.info\", body: \"id: $args.id\" }}, selection: \"$.results {{ id pet: {{ id: petId }} }}\")\n}}\n",
            CYC_HEAD
        );
        let ws = graphos_factory_core::yaml::parse(CYC_WORKSPACE).unwrap();
        rules(&check(&sdl, &ws, Some(&sel), Some(&inv)))
    };
    let f = case(json!({ "id": { "type": "string" }, "petId": { "type": "string" } }));
    assert!(f.is_empty(), "{:?}", f);
    let f = case(json!({ "id": { "type": "string" } }));
    assert_eq!(f.len(), 1, "{:?}", f);
    assert!(
        f[0].1.starts_with(
            "post:/visit.info embeds Cyc_Pet at Visit.pet (Cyc_Visit), but the embedded shape lacks the key id"
        ),
        "{:?}",
        f
    );
}

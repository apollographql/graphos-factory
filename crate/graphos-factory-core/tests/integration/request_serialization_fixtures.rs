//! Cross-spec / no-name-coupling proof for `request_serialization`,
//! run against real, checked-in fixture directories rather than tempdirs
//! (task PROOF section: "Build compact fixtures from two different specs,
//! with one renamed variant to detect name coupling").

use graphos_factory_core::request_serialization::{obligations, ObligationStatus};
use std::path::Path;

fn assert_all_pass(dir: &Path, label: &str) {
    let obs = obligations(dir);
    assert_eq!(obs.len(), 4, "{}: {:#?}", label, obs);
    for o in &obs {
        assert!(
            matches!(o.status, ObligationStatus::Pass),
            "{}: obligation {} did not pass: {:#?}",
            label,
            o.id,
            o
        );
    }
}

#[test]
fn serialization_widgets_fixture_passes_every_obligation() {
    assert_all_pass(
        Path::new("tests/fixtures/serialization-widgets"),
        "serialization-widgets",
    );
}

#[test]
fn serialization_crm_fixture_passes_every_obligation() {
    assert_all_pass(
        Path::new("tests/fixtures/serialization-crm"),
        "serialization-crm",
    );
}

#[test]
fn renamed_widgets_fixture_produces_the_identical_obligation_set() {
    // Same shapes, same structure, every name mechanically substituted
    // (rename.py). If `request_serialization` were secretly keyed off a
    // literal string like "widget" anywhere, this fixture would diverge --
    // it must not.
    let original = obligations(Path::new("tests/fixtures/serialization-widgets"));
    let renamed = obligations(Path::new("tests/fixtures/serialization-widgets-renamed"));
    assert_eq!(original.len(), renamed.len());
    let mut o_sorted: Vec<(&str, String, Option<(usize, usize)>)> = original
        .iter()
        .map(|o| (o.id.as_str(), o.status.as_str().to_string(), o.denominator))
        .collect();
    let mut r_sorted: Vec<(&str, String, Option<(usize, usize)>)> = renamed
        .iter()
        .map(|o| (o.id.as_str(), o.status.as_str().to_string(), o.denominator))
        .collect();
    o_sorted.sort();
    r_sorted.sort();
    assert_eq!(
        o_sorted, r_sorted,
        "renamed fixture's (id, status, denominator) tuples diverged from the original -- \
         a sign of accidental name coupling in request_serialization"
    );
    // Both must actually be the fully-proven baseline, not two fixtures
    // that happen to fail identically.
    for o in &original {
        assert!(matches!(o.status, ObligationStatus::Pass), "{:#?}", o);
    }
}

// ── Members of a list of input objects ──────────────────────────────────────
//
// `addPet(input: { tags: [{ id, name }] })`, from a fresh walkthrough on the
// Swagger Petstore: the proof built `input.tags.id`'s pointer as `/tags/id`,
// with no array index, so no stub could ever demand it there. A member
// under a list is now proven at each element's own index (`/tags/0/id`,
// `/tags/1/id`): every element the case passes with the member must be
// demanded with that value at its index, as every element of a scalar list
// is.

mod list_members {
    use graphos_factory_core::request_serialization::{report, GapKind, ObligationStatus, Report};
    use serde_json::{json, Value};
    use std::path::Path;

    const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.14", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.4", import: ["@source", "@connect"])

@source(name: "pets", http: { baseURL: "{{BASE_URL}}" })

input Pets_TagInput {
  id: Int!
  name: String!
}

input Pets_PetInput {
  name: String!
  photoUrls: [String!]!
  tags: [Pets_TagInput!]!
}

input Pets_OptionInput {
  code: String!
}

input Pets_LineInput {
  sku: String!
  options: [Pets_OptionInput!]!
}

input Pets_OrderInput {
  lines: [Pets_LineInput!]!
}

type Pets_Pet {
  name: String
}

type Mutation {
  "Add a pet."
  pets_addPet(input: Pets_PetInput!): Pets_Pet
    @connect(
      source: "pets"
      http: {
        POST: "/pets"
        body: """
        $args.input {
          name
          photoUrls
          tags { id name }
        }
        """
      }
      selection: "name"
    )
  "Place an order."
  pets_addOrder(input: Pets_OrderInput!): Pets_Pet
    @connect(
      source: "pets"
      http: {
        POST: "/orders"
        body: """
        $args.input {
          lines { sku options { code } }
        }
        """
      }
      selection: "name"
    )
  "Add a tag set."
  pets_addTagSet(tags: [Pets_TagInput!]!): Pets_Pet
    @connect(
      source: "pets"
      http: {
        POST: "/tagsets"
        body: """
        tags: $args.tags { id name }
        """
      }
      selection: "name"
    )
}
"#;

    const SELECTION: &str = r#"operations:
  "post:/pets":
    include: true
    graphql: { root: mutation, name: addPet }
  "post:/orders":
    include: true
    graphql: { root: mutation, name: addOrder }
  "post:/tagsets":
    include: true
    graphql: { root: mutation, name: addTagSet }
"#;

    /// The source document the inventory is built from, as `init` builds it.
    fn spec() -> Value {
        let post = |schema: &str| {
            json!({"post": {
                "operationId": schema,
                "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", schema)}}}},
                "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Pet"}}}}}
            }})
        };
        json!({
            "openapi": "3.0.3",
            "info": {"title": "Pets", "version": "1"},
            "servers": [{"url": "https://pets.example.test"}],
            "paths": {"/pets": post("Pet"), "/orders": post("Order"), "/tagsets": post("TagSet")},
            "components": {"schemas": {
                "Tag": {"type": "object", "required": ["id", "name"], "properties": {
                    "id": {"type": "integer", "format": "int64"}, "name": {"type": "string"}}},
                "Pet": {"type": "object", "required": ["name", "photoUrls", "tags"], "properties": {
                    "name": {"type": "string"},
                    "photoUrls": {"type": "array", "items": {"type": "string"}},
                    "tags": {"type": "array", "items": {"$ref": "#/components/schemas/Tag"}}}},
                "Option": {"type": "object", "required": ["code"], "properties": {"code": {"type": "string"}}},
                "Line": {"type": "object", "required": ["sku", "options"], "properties": {
                    "sku": {"type": "string"},
                    "options": {"type": "array", "items": {"$ref": "#/components/schemas/Option"}}}},
                "Order": {"type": "object", "required": ["lines"], "properties": {
                    "lines": {"type": "array", "items": {"$ref": "#/components/schemas/Line"}}}},
                "TagSet": {"type": "object", "required": ["tags"], "properties": {
                    "tags": {"type": "array", "items": {"$ref": "#/components/schemas/Tag"}}}}
            }}
        })
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A workspace whose every case is recorded as executed and passing at
    /// e2e, each behind a stub named for it demanding `body` exactly.
    fn workspace(cases: &[(&str, &str, &str, Value)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let inventory = graphos_factory_core::openapi::build_inventory(&spec())
            .unwrap()
            .inventory;
        write(d, "pets.graphql", SDL);
        write(
            d,
            ".factory/workspace.yaml",
            "directory: pets\nfield_prefix: pets\n",
        );
        write(d, ".factory/selection.yaml", SELECTION);
        write(
            d,
            ".factory/inventory.json",
            &graphos_factory_core::json::pretty(&inventory),
        );
        write(
            d,
            ".factory/evidence/latest.json",
            &json!({"layers": {"wiremock_e2e": {"status": "pass", "log": ".factory/evidence/runs/r/e2e.log"}}})
                .to_string(),
        );
        let mut log = String::new();
        for (name, path, call, body) in cases {
            log.push_str(&format!("PASS: {}\n", name));
            write(
                d,
                &format!("tests/cases/{}.graphql", name),
                &format!("mutation {{ {} {{ name }} }}\n", call),
            );
            write(
                d,
                &format!("tests/fixtures/mappings/{}.json", name),
                &json!({
                    "request": {"method": "POST", "urlPath": path,
                        "bodyPatterns": [{"equalToJson": body.to_string()}]},
                    "response": {"status": 200, "jsonBody": {"name": "x"}}
                })
                .to_string(),
            );
        }
        write(d, ".factory/evidence/runs/r/e2e.log", &log);
        dir
    }

    fn unasserted<'r>(r: &'r Report, op: &str) -> Vec<&'r str> {
        r.writes
            .iter()
            .find(|w| w.operation == op)
            .unwrap_or_else(|| panic!("{} reported: {:#?}", op, r.writes))
            .gaps
            .iter()
            .filter(|g| g.kind == GapKind::ArgumentUnasserted)
            .map(|g| g.message.as_str())
            .collect()
    }

    const ADD_PET: &str = r#"pets_addPet(input: { name: "doggie", photoUrls: ["https://example.test/a.png"], tags: [{ id: 701, name: "friendly" }, { id: 702, name: "calm" }] })"#;

    fn pet_body() -> Value {
        json!({"name": "doggie", "photoUrls": ["https://example.test/a.png"],
               "tags": [{"id": 701, "name": "friendly"}, {"id": 702, "name": "calm"}]})
    }

    /// The walkthrough's reproduction: the stub demands every element's
    /// members, and the proof now finds them, with two elements and with one.
    #[test]
    fn a_member_of_a_list_of_input_objects_is_proven_at_each_elements_index() {
        let dir = workspace(&[("add_pet", "/pets", ADD_PET, pet_body())]);
        let r = report(dir.path());
        let write = r
            .writes
            .iter()
            .find(|w| w.operation == "post:/pets")
            .unwrap();
        assert!(write.body_proven, "{:#?}", write.gaps);

        let one = r#"pets_addPet(input: { name: "doggie", photoUrls: ["https://example.test/a.png"], tags: [{ id: 701, name: "friendly" }] })"#;
        let dir = workspace(&[(
            "add_pet",
            "/pets",
            one,
            json!({"name": "doggie", "photoUrls": ["https://example.test/a.png"],
                   "tags": [{"id": 701, "name": "friendly"}]}),
        )]);
        let r = report(dir.path());
        let write = r
            .writes
            .iter()
            .find(|w| w.operation == "post:/pets")
            .unwrap();
        assert!(write.body_proven, "{:#?}", write.gaps);
    }

    /// The rule is every element, not at least one: an element whose member
    /// the case passes and the demanded body leaves out is a gap naming that
    /// element's own pointer. An element the case passes without the member
    /// demands nothing.
    #[test]
    fn an_element_missing_the_member_in_the_demanded_body_is_a_gap_at_its_own_index() {
        let mut body = pet_body();
        body["tags"][1].as_object_mut().unwrap().remove("id");
        let dir = workspace(&[("add_pet", "/pets", ADD_PET, body.clone())]);
        let r = report(dir.path());
        assert_eq!(
            unasserted(&r, "post:/pets"),
            vec!["post:/pets: pets_addPet(input.tags.id) has no executed case whose stub demands its value at body /tags/1/id (no case supplies it behind a stub that demands it there)"],
        );

        // A wrong value at one index is the same gap, at that index.
        let mut wrong = pet_body();
        wrong["tags"][0]["name"] = json!("grumpy");
        let dir = workspace(&[("add_pet", "/pets", ADD_PET, wrong)]);
        let r = report(dir.path());
        assert_eq!(
            unasserted(&r, "post:/pets"),
            vec!["post:/pets: pets_addPet(input.tags.name) has no executed case whose stub demands its value at body /tags/0/name (no case supplies it behind a stub that demands it there)"],
        );

        // The case leaves the member out of the second element too: the
        // first element carries it and is demanded, which proves it.
        let call = ADD_PET.replace("{ id: 702, name: \"calm\" }", "{ name: \"calm\" }");
        let dir = workspace(&[("add_pet", "/pets", &call, body)]);
        let r = report(dir.path());
        assert!(unasserted(&r, "post:/pets").is_empty(), "{:#?}", r.writes);
    }

    /// A list inside an object inside a list: each `*` is its own element's
    /// index, `/lines/1/options/0/code`.
    #[test]
    fn a_nested_list_inside_an_object_inside_a_list_is_proven_at_both_indexes() {
        let call = r#"pets_addOrder(input: { lines: [{ sku: "A-100", options: [{ code: "gift" }, { code: "wrap" }] }, { sku: "B-200", options: [{ code: "rush" }] }] })"#;
        let body = json!({"lines": [
            {"sku": "A-100", "options": [{"code": "gift"}, {"code": "wrap"}]},
            {"sku": "B-200", "options": [{"code": "rush"}]}
        ]});
        let dir = workspace(&[("add_order", "/orders", call, body.clone())]);
        let r = report(dir.path());
        let write = r
            .writes
            .iter()
            .find(|w| w.operation == "post:/orders")
            .unwrap();
        assert!(write.body_proven, "{:#?}", write.gaps);

        let mut wrong = body;
        wrong["lines"][1]["options"][0]["code"] = json!("slow");
        let dir = workspace(&[("add_order", "/orders", call, wrong)]);
        let r = report(dir.path());
        assert_eq!(
            unasserted(&r, "post:/orders"),
            vec!["post:/orders: pets_addOrder(input.lines.options.code) has no executed case whose stub demands its value at body /lines/1/options/0/code (no case supplies it behind a stub that demands it there)"],
        );
    }

    /// A top-level list of input objects the connector places member by
    /// member: each member is proven at each element's index, and the list
    /// proof reads the same demands.
    #[test]
    fn a_top_level_list_placed_member_by_member_is_proven_per_element() {
        let call =
            r#"pets_addTagSet(tags: [{ id: 701, name: "friendly" }, { id: 702, name: "calm" }])"#;
        let body = json!({"tags": [{"id": 701, "name": "friendly"}, {"id": 702, "name": "calm"}]});
        let dir = workspace(&[("add_tag_set", "/tagsets", call, body.clone())]);
        let r = report(dir.path());
        let write = r
            .writes
            .iter()
            .find(|w| w.operation == "post:/tagsets")
            .unwrap();
        assert!(write.body_proven, "{:#?}", write.gaps);
        let lists = r
            .obligations
            .iter()
            .find(|o| o.id == "serialization.list-argument-proof")
            .unwrap();
        assert_eq!(lists.status, ObligationStatus::Pass, "{:#?}", lists);

        let mut wrong = body;
        wrong["tags"][1]["id"] = json!(703);
        let dir = workspace(&[("add_tag_set", "/tagsets", call, wrong)]);
        let r = report(dir.path());
        assert_eq!(
            unasserted(&r, "post:/tagsets"),
            vec!["post:/tagsets: pets_addTagSet(tags.id) has no executed case whose stub demands its value at body /tags/1/id (no case supplies it behind a stub that demands it there)"],
        );
    }

    /// A scalar list and a plain member keep their pointers: a wrong value
    /// is reported at `/photoUrls` and `/name`, the whole array compared.
    /// Read on its own members only, this passes before the list-member
    /// change as after it.
    #[test]
    fn a_scalar_list_and_a_plain_member_keep_their_pointers() {
        let mut wrong = pet_body();
        wrong["photoUrls"] = json!(["https://example.test/b.png"]);
        wrong["name"] = json!("kitty");
        let dir = workspace(&[("add_pet", "/pets", ADD_PET, wrong)]);
        let r = report(dir.path());
        let own: Vec<&str> = unasserted(&r, "post:/pets")
            .into_iter()
            .filter(|m| m.contains("(input.name)") || m.contains("(input.photoUrls)"))
            .collect();
        assert_eq!(
            own,
            vec![
                "post:/pets: pets_addPet(input.name) has no executed case whose stub demands its value at body /name (no case supplies it behind a stub that demands it there)",
                "post:/pets: pets_addPet(input.photoUrls) has no executed case whose stub demands its value at body /photoUrls (no case supplies it behind a stub that demands it there)",
            ],
        );
    }
}

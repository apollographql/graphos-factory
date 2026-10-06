//! Declared foreign types (ADR 0132): `decisions add --foreign-type NAME`
//! records the name on a new record's file, never on a numbered one;
//! `decisions::foreign_types` reads it from resolved records only; and
//! `entity::foreign_type_findings` holds a declared type's shape. Which
//! rules honour the set is a target's choice (`target.rs` here, the
//! targets' suites for the products).

use graphos_factory_core::{decisions as log, entity, json};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use tempfile::TempDir;

fn decisions(dir: &Path, args: &[&str]) -> i32 {
    let mut argv: Vec<String> = vec![args[0].to_string(), dir.to_string_lossy().to_string()];
    argv.extend(args[1..].iter().map(|a| a.to_string()));
    graphos_factory_core::cmd::decisions::main(&argv)
}

/// Add a record declaring `names` foreign, resolved by the user's answer
/// or left open.
fn declare(dir: &Path, names: &[&str], resolved: bool) -> i32 {
    let mut args = vec![
        "add",
        "--title",
        "Product is owned by the products subgraph",
        "--question",
        "Which subgraph owns Product?",
        "--choice",
        "the products subgraph",
        "--choice",
        "this one",
        "--date",
        "2026-10-06",
    ];
    for n in names {
        args.extend_from_slice(&["--foreign-type", n]);
    }
    if resolved {
        args.extend_from_slice(&["--resolved", "--chosen", "1", "--by", "user"]);
    }
    decisions(dir, &args)
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn only_record(dir: &Path) -> Value {
    let doc = log::load(dir, None).unwrap();
    let records = json::get_arr(&doc, "decisions").unwrap();
    assert_eq!(records.len(), 1);
    records[0].clone()
}

#[test]
fn the_flag_is_recorded_once_per_name_on_the_records_own_file() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(declare(d, &["Product", "Review", "Product"], true), 0);
    let rec = only_record(d);
    assert_eq!(rec["foreign_types"], json!(["Product", "Review"]));
    assert!(!d.join(".factory/decisions.json").exists());
    let doc = log::load(d, None).unwrap();
    assert_eq!(log::foreign_types(&doc), set(&["Product", "Review"]));
}

#[test]
fn only_a_resolved_record_declares_a_foreign_type() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(declare(d, &["Product"], false), 0);
    let id = json::get_str(&only_record(d), "id").unwrap().to_string();
    assert!(log::foreign_types(&log::load(d, None).unwrap()).is_empty());

    assert_eq!(
        decisions(
            d,
            &["resolve", "--id", &id, "--chosen", "1", "--by", "user"]
        ),
        0
    );
    assert_eq!(
        log::foreign_types(&log::load(d, None).unwrap()),
        set(&["Product"])
    );

    assert_eq!(decisions(d, &["supersede", "--id", &id]), 0);
    assert!(log::foreign_types(&log::load(d, None).unwrap()).is_empty());
}

#[test]
fn a_root_type_or_a_non_name_is_refused_and_nothing_is_written() {
    for name in ["Query", "Mutation", "Subscription", "Not-A-Name", "1st"] {
        let dir = TempDir::new().unwrap();
        assert_eq!(declare(dir.path(), &[name], true), 1, "{}", name);
        assert!(!dir.path().join(".factory").exists(), "{}", name);
    }
}

/// The single file holds the numbered records written before records had
/// their own files, and it never gains a field one of them did not carry.
#[test]
fn a_numbered_record_never_gains_the_field() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join(".factory")).unwrap();
    let old = json!({"contract_version": 1, "decisions": [{
        "id": "D-0001",
        "title": "Old answer",
        "status": "resolved",
        "date": "2026-09-02",
        "question": "Which way?",
        "resolution": {"note": "this way", "by": "agent"}
    }]});
    std::fs::write(d.join(".factory/decisions.json"), json::pretty(&old)).unwrap();
    let mut doc = log::load(d, None).unwrap();
    doc["decisions"][0]["foreign_types"] = json!(["Product"]);
    assert!(log::save(d, &doc, None).is_err());
    let schema = graphos_factory_core::schemas::load("decisions.schema.json", None).unwrap();
    let mut file = old.clone();
    file["decisions"][0]["foreign_types"] = json!(["Product"]);
    assert!(!graphos_factory_core::jsonschema::validate(&file, &schema).is_empty());
}

const SDL: &str = r#"type Product @key(fields: "id") {
  id: ID!
  weight: Int @external
  reviewCount: Int
    @requires(fields: "weight")
    @connect(source: "s", http: { GET: "/reviews?product={$this.id}&w={$this.weight}" }, selection: "$.count")
  label: String
    @requires(fields: "colour")
    @connect(source: "s", http: { GET: "/labels/{$this.id}" }, selection: "$.label")
}

type Keyless {
  id: ID!
}

type Mismatched @key(fields: "sku", resolvable: false) {
  id: ID!
}

type Query {
  s_products: [Product] @connect(source: "s", http: { GET: "/products" }, selection: "id")
}
"#;

#[test]
fn a_declared_types_shape_is_held_and_an_undeclared_one_is_not_read() {
    let found = entity::foreign_type_findings(SDL, &set(&["Product", "Keyless", "Mismatched"]));
    let rules: Vec<(&str, &str, String)> = found
        .iter()
        .map(|(sev, rule, msg, _)| (*sev, *rule, msg.clone()))
        .collect();
    assert_eq!(
        rules,
        vec![
            (
                "error",
                "foreign-type-without-key",
                "Keyless is declared another subgraph's type (a resolved decision's foreign_types) but carries no @key: the supergraph joins it to the owner's entity only by the owner's key; write the owner's @key(fields: …) on it, with resolvable: false for a reference stub".to_string()
            ),
            (
                "error",
                "foreign-type-key-field-missing",
                "Mismatched carries @key(fields: \"sku\") but declares no sku field: declare every field the owner's key names, typed as the owner types it".to_string()
            ),
            (
                "warn",
                "requires-on-foreign-type",
                "Product.label carries @requires(fields: \"colour\"), but Product declares no colour field: declare each required field on Product with @external, typed as the owner types it".to_string()
            ),
        ]
    );
    // Each finding names the line its type is declared on.
    assert_eq!(found[0].3, Some(12));
    // Nothing declared, nothing read; a declared name the schema lacks is
    // not judged.
    assert!(entity::foreign_type_findings(SDL, &BTreeSet::new()).is_empty());
    assert!(entity::foreign_type_findings(SDL, &set(&["Absent"])).is_empty());
}

/// Only an object type can be an entity: a declared name the schema
/// declares as any other kind is `foreign-type-not-object`, naming the kind.
#[test]
fn a_declared_name_that_is_not_an_object_type_is_named_with_its_kind() {
    let sdl = "enum S_Colour { RED }\ninput Filter { q: String }\ninterface Node { id: ID! }\nunion Thing = Keyless\nscalar Stamp\ntype Keyless { id: ID! }\n";
    let found =
        entity::foreign_type_findings(sdl, &set(&["S_Colour", "Filter", "Node", "Thing", "Stamp"]));
    let kinds: Vec<(&str, String, Option<usize>)> = found
        .iter()
        .map(|(sev, rule, msg, line)| {
            assert_eq!(*rule, "foreign-type-not-object");
            (*sev, msg.split(": only").next().unwrap().to_string(), *line)
        })
        .collect();
    let says = |name: &str, kind: &str| {
        format!(
            "{} is declared another subgraph's type (a resolved decision's foreign_types) but this schema declares it as {}",
            name, kind
        )
    };
    assert_eq!(
        kinds,
        vec![
            ("error", says("Filter", "an input"), Some(2)),
            ("error", says("Node", "an interface"), Some(3)),
            ("error", says("S_Colour", "an enum"), Some(1)),
            ("error", says("Stamp", "a scalar"), Some(5)),
            ("error", says("Thing", "a union"), Some(4)),
        ]
    );
}

/// The entity rules read a declared type as the owner's: no lookup for an
/// extension keyed by `$this`, no consumer wanted for a resolvable key, and
/// its key and `@external` fields never unresolved. Undeclared, the same
/// schema is held as today.
#[test]
fn the_entity_rules_read_a_declared_type_as_the_owners() {
    let sdl = r#"type Product @key(fields: "id") {
  id: ID!
  weight: Int @external
  reviewCount: Int
    @requires(fields: "weight")
    @connect(source: "s", http: { GET: "/reviews?product={$this.id}&w={$this.weight}" }, selection: "$.count")
}

type Query {
  s_ping: String @connect(source: "s", http: { GET: "/ping" }, selection: "$")
}
"#;
    let workspace = json!({"type_prefix": "S", "field_prefix": "s"});
    let rules = |foreign: &BTreeSet<String>| -> Vec<&'static str> {
        entity::check_with(sdl, &workspace, None, None, foreign)
            .into_iter()
            .map(|(_, rule, _)| rule)
            .collect()
    };
    assert_eq!(
        rules(&BTreeSet::new()),
        vec![
            "entity-field-unresolved",
            "entity-without-consumer",
            "entity-without-lookup"
        ]
    );
    assert!(rules(&set(&["Product"])).is_empty());
    // `check` is the undeclared reading.
    assert_eq!(
        entity::check(sdl, &workspace, None, None).len(),
        rules(&BTreeSet::new()).len()
    );
}

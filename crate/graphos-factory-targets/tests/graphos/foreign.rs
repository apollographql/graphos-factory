//! Entities another subgraph owns (ADR 0132): a type a resolved decision
//! declares foreign keeps its owner's name and is read as the owner's
//! entity, extended here by `$this`-keyed field connectors or referenced by
//! a `resolvable: false` stub; the target's three rules hold its shape. On
//! copies of this target's pilot, with the snippets in
//! `fixtures/foreign/` (the owner's side, `accounts.graphql`, is composed
//! beside them by hand: the suite runs no rover).

use super::{copy_of, findings, fixture, graphos_pilot, lint_json, run};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// The pilot's schema with `snippet` (a file under `fixtures/foreign/`,
/// or SDL) declared before `type Query`.
fn with_type(ws: &Path, sdl: &str) {
    let schema = ws.join("gitea.graphql");
    let text = std::fs::read_to_string(&schema).unwrap();
    assert_eq!(text.matches("type Query {").count(), 1);
    std::fs::write(
        &schema,
        text.replacen("type Query {", &format!("{}type Query {{", sdl), 1),
    )
    .unwrap();
}

fn snippet(name: &str) -> String {
    std::fs::read_to_string(fixture("foreign").join(name)).unwrap()
}

/// The stub, referenced from `Gitea_Issue.author` and mapped by each of
/// the three issue connectors as `author: user { login }`.
fn with_stub(ws: &Path, stub: &str) {
    with_type(ws, stub);
    let schema = ws.join("gitea.graphql");
    let text = std::fs::read_to_string(&schema).unwrap();
    let field = "  user: Gitea_User\n  originalAuthor";
    assert_eq!(text.matches(field).count(), 1);
    let text = text.replacen(
        field,
        "  \"The account that opened the issue, resolved by the accounts subgraph.\"\n  author: Account\n  user: Gitea_User\n  originalAuthor",
        1,
    );
    assert_eq!(text.matches("\n      user {\n").count(), 3);
    let text = text.replace(
        "\n      user {\n",
        "\n      author: user { login }\n      user {\n",
    );
    std::fs::write(&schema, text).unwrap();
}

/// Record `name` foreign, as the user's answer (`resolved`) or an open
/// question.
fn declare(ws: &Path, name: &str, resolved: bool) -> (Option<i32>, String, String) {
    let dir = ws.to_string_lossy().into_owned();
    let mut args = vec![
        "decisions",
        "add",
        &dir,
        "--title",
        "Account is owned by the accounts subgraph",
        "--question",
        "Which subgraph owns Account?",
        "--choice",
        "the accounts subgraph",
        "--choice",
        "this one",
        "--foreign-type",
        name,
    ];
    if resolved {
        args.extend_from_slice(&["--resolved", "--chosen", "1", "--by", "user"]);
    }
    run(&args)
}

/// Acknowledge the schema edit, so lint reports what the edit is and not
/// that it was made.
fn lock(ws: &Path) {
    let (code, stdout, stderr) = run(&["lock", &ws.to_string_lossy()]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
}

fn rules(report: &Value) -> BTreeSet<String> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap().to_string())
        .collect()
}

/// What the edit adds to the pilot's own findings, by rule.
fn added(report: &Value) -> BTreeSet<String> {
    let pilot = rules(&lint_json(&graphos_pilot()));
    rules(report).difference(&pilot).cloned().collect()
}

const PREFIX: &str = "type Account must be prefixed Gitea_ so it cannot collide in the supergraph; to extend or reference a type another subgraph owns under its owner's name, record it as the user's resolved decision: graphos-factory-core decisions add . --foreign-type Account …";

/// Undeclared, the owner's name is a prefix error that says how to declare
/// it, and the extension's key has no lookup here.
#[test]
fn an_undeclared_foreign_type_is_a_prefix_error_naming_the_decision() {
    let ws = copy_of(&graphos_pilot());
    with_type(ws.path(), &snippet("extension.graphql"));
    lock(ws.path());
    let report = lint_json(ws.path());
    let prefix = findings(&report, "type-prefix");
    assert_eq!(prefix.len(), 1, "{}", report);
    assert_eq!(prefix[0]["severity"], "error");
    assert_eq!(prefix[0]["origin"], "core");
    assert_eq!(prefix[0]["message"], PREFIX);
    assert_eq!(
        findings(&report, "entity-without-lookup").len(),
        1,
        "{}",
        report
    );
    // Only an object type gets the hint: an enum is never another
    // subgraph's entity.
    let ws = copy_of(&graphos_pilot());
    with_type(ws.path(), "enum Colour {\n  red\n}\n\n");
    let report = lint_json(ws.path());
    let prefix = findings(&report, "type-prefix");
    assert_eq!(prefix.len(), 1, "{}", report);
    assert_eq!(
        prefix[0]["message"],
        "enum Colour must be prefixed Gitea_ so it cannot collide in the supergraph"
    );
}

/// An open question lifts nothing: only the user's answer does.
#[test]
fn an_open_decision_lifts_nothing() {
    let ws = copy_of(&graphos_pilot());
    with_type(ws.path(), &snippet("extension.graphql"));
    let (code, stdout, stderr) = declare(ws.path(), "Account", false);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    assert!(
        stderr.contains("--foreign-type entries do not count until `decisions resolve`"),
        "{}",
        stderr
    );
    lock(ws.path());
    let report = lint_json(ws.path());
    assert_eq!(findings(&report, "type-prefix").len(), 1, "{}", report);
    assert_eq!(
        findings(&report, "entity-without-lookup").len(),
        1,
        "{}",
        report
    );
}

/// Declared, the extension (the owner's key, one `$this`-keyed field
/// connector) lints as the pilot does, but for the two warnings every
/// connector field without its tests carries. Composed by hand beside
/// `accounts.graphql`, it joins the owner's `Account`.
#[test]
fn a_declared_extension_lints_as_the_pilot_does() {
    let ws = copy_of(&graphos_pilot());
    with_type(ws.path(), &snippet("extension.graphql"));
    let (code, stdout, stderr) = declare(ws.path(), "Account", true);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    // This target reads the record: no note says otherwise.
    assert!(!stderr.contains("note:"), "{}", stderr);
    lock(ws.path());
    let report = lint_json(ws.path());
    assert_eq!(report["errors"], 0, "{}", report);
    assert_eq!(
        added(&report),
        ["link-live-unaccounted", "link-untested"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "{}",
        report
    );
    // The record carries the name, on its own file.
    let (_, stdout, _) = run(&["decisions", "list", &ws.path().to_string_lossy(), "--json"]);
    assert!(stdout.contains("\"foreign_types\""), "{}", stdout);
}

/// Declared, the stub (`resolvable: false`, embedded by its key) lints
/// exactly as the pilot does.
#[test]
fn a_declared_stub_lints_as_the_pilot_does() {
    let ws = copy_of(&graphos_pilot());
    with_stub(ws.path(), &snippet("stub.graphql"));
    let (code, stdout, stderr) = declare(ws.path(), "Account", true);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    lock(ws.path());
    let report = lint_json(ws.path());
    assert_eq!(report["errors"], 0, "{}", report);
    assert!(added(&report).is_empty(), "{}", report);
    assert_eq!(
        report["warnings"],
        lint_json(&graphos_pilot())["warnings"],
        "{}",
        report
    );
}

/// A stub nothing here references still serves no reference: the warning
/// stays for a declared one.
#[test]
fn an_unreferenced_declared_stub_still_has_no_consumer() {
    let ws = copy_of(&graphos_pilot());
    with_type(ws.path(), &snippet("stub.graphql"));
    declare(ws.path(), "Account", true);
    lock(ws.path());
    let report = lint_json(ws.path());
    assert_eq!(report["errors"], 0, "{}", report);
    assert_eq!(
        added(&report),
        ["entity-without-consumer".to_string()]
            .into_iter()
            .collect(),
        "{}",
        report
    );
}

/// The connector that keeps the lookup rule away must read `$this`: one
/// that reads only its arguments resolves nothing from a representation,
/// so the key needs a lookup again.
#[test]
fn a_field_connector_not_keyed_by_this_keeps_the_lookup_rule() {
    let ws = copy_of(&graphos_pilot());
    let ext = snippet("extension.graphql");
    assert!(ext.contains("/users/{$this.login}"));
    let ext = ext
        .replace(
            "giteaUser: Gitea_User",
            "giteaUser(\"the login to look up\" login: String!): Gitea_User",
        )
        .replace("/users/{$this.login}", "/users/{$args.login}");
    with_type(ws.path(), &ext);
    declare(ws.path(), "Account", true);
    lock(ws.path());
    let report = lint_json(ws.path());
    assert!(findings(&report, "type-prefix").is_empty(), "{}", report);
    assert_eq!(
        findings(&report, "entity-without-lookup").len(),
        1,
        "{}",
        report
    );
}

/// A declared foreign type with no `@key` cannot be joined to its owner.
#[test]
fn a_declared_foreign_type_without_a_key_is_an_error() {
    let ws = copy_of(&graphos_pilot());
    let stub = snippet("stub.graphql");
    let keyless = stub.replace(" @key(fields: \"login\", resolvable: false)", "");
    assert_ne!(keyless, stub);
    with_stub(ws.path(), &keyless);
    declare(ws.path(), "Account", true);
    lock(ws.path());
    let report = lint_json(ws.path());
    let keyless = findings(&report, "foreign-type-without-key");
    assert_eq!(keyless.len(), 1, "{}", report);
    assert_eq!(keyless[0]["severity"], "error");
    assert_eq!(keyless[0]["origin"], "graphos-factory");
    assert_eq!(
        keyless[0]["message"],
        "Account is declared another subgraph's type (a resolved decision's foreign_types) but carries no @key: the supergraph joins it to the owner's entity only by the owner's key; write the owner's @key(fields: …) on it, with resolvable: false for a reference stub"
    );
    assert!(keyless[0]["line"].as_u64().is_some(), "{}", report);
    assert!(findings(&report, "type-prefix").is_empty(), "{}", report);
}

/// Every top-level field a `@key` names is declared on the type.
#[test]
fn a_key_field_the_type_does_not_declare_is_an_error() {
    let ws = copy_of(&graphos_pilot());
    let stub = snippet("stub.graphql").replace(
        "@key(fields: \"login\", resolvable: false)",
        "@key(fields: \"id\", resolvable: false)",
    );
    with_stub(ws.path(), &stub);
    declare(ws.path(), "Account", true);
    lock(ws.path());
    let report = lint_json(ws.path());
    let missing = findings(&report, "foreign-type-key-field-missing");
    assert_eq!(missing.len(), 1, "{}", report);
    assert_eq!(missing[0]["severity"], "error");
    assert_eq!(
        missing[0]["message"],
        "Account carries @key(fields: \"id\") but declares no id field: declare every field the owner's key names, typed as the owner types it"
    );
}

/// `@requires` on a foreign type's field names fields the type declares
/// `@external`; one it does not declare is a warning, and an `@external`
/// field is the owner's to resolve, never unresolved here.
#[test]
fn requires_names_fields_the_foreign_type_declares() {
    let field = "  giteaBio: String\n    @requires(fields: \"displayName\")\n    @connect(\n      source: \"gitea\"\n      http: { GET: \"/users/{$this.login}?name={$this.displayName}\" }\n      selection: \"description\"\n    )\n}\n";
    let ext = snippet("extension.graphql");
    let at = ext.rfind("}\n").unwrap();
    let requiring = format!("{}{}{}", &ext[..at], field, &ext[at + 2..]);

    let ws = copy_of(&graphos_pilot());
    with_type(ws.path(), &requiring);
    declare(ws.path(), "Account", true);
    lock(ws.path());
    let report = lint_json(ws.path());
    let requires = findings(&report, "requires-on-foreign-type");
    assert_eq!(requires.len(), 1, "{}", report);
    assert_eq!(requires[0]["severity"], "warn");
    assert_eq!(
        requires[0]["message"],
        "Account.giteaBio carries @requires(fields: \"displayName\"), but Account declares no displayName field: declare each required field on Account with @external, typed as the owner types it"
    );

    let external = requiring.replacen(
        "  login: String!\n",
        "  login: String!\n  displayName: String @external\n",
        1,
    );
    let ws = copy_of(&graphos_pilot());
    with_type(ws.path(), &external);
    declare(ws.path(), "Account", true);
    lock(ws.path());
    let report = lint_json(ws.path());
    assert!(
        findings(&report, "requires-on-foreign-type").is_empty(),
        "{}",
        report
    );
    assert!(
        findings(&report, "entity-field-unresolved").is_empty(),
        "{}",
        report
    );
}

/// The flag takes a GraphQL type name, never a root operation type.
#[test]
fn the_flag_refuses_a_root_type_and_a_non_name() {
    for (name, needle) in [
        ("Query", "a root operation type"),
        ("Not-A-Name", "expected a GraphQL type name"),
    ] {
        let ws = copy_of(&graphos_pilot());
        let (code, stdout, stderr) = declare(ws.path(), name, true);
        assert_eq!(code, Some(1), "{}: {} {}", name, stdout, stderr);
        assert!(stderr.contains(needle), "{}: {}", name, stderr);
    }
}

/// Only an object type can be another subgraph's entity: a declared enum
/// or input keeps `type-prefix`, and the declaration is an error that names
/// the kind.
#[test]
fn a_declared_enum_or_input_is_not_an_entity_and_keeps_the_prefix() {
    for (sdl, kind) in [
        ("enum Colour {\n  red\n  blue\n}\n\n", "an enum"),
        ("input Colour {\n  name: String\n}\n\n", "an input"),
    ] {
        let ws = copy_of(&graphos_pilot());
        with_type(ws.path(), sdl);
        let (code, stdout, stderr) = declare(ws.path(), "Colour", true);
        assert_eq!(code, Some(0), "{} {}", stdout, stderr);
        lock(ws.path());
        let report = lint_json(ws.path());
        let not_object = findings(&report, "foreign-type-not-object");
        assert_eq!(not_object.len(), 1, "{}: {}", kind, report);
        assert_eq!(not_object[0]["severity"], "error");
        assert_eq!(not_object[0]["origin"], "graphos-factory");
        assert_eq!(
            not_object[0]["message"],
            format!(
                "Colour is declared another subgraph's type (a resolved decision's foreign_types) but this schema declares it as {}: only an object type can be another subgraph's entity, so the declaration lifts no rule on it; give it this subgraph's prefix, or take it out of the decision",
                kind
            )
        );
        assert!(not_object[0]["line"].as_u64().is_some(), "{}", report);
        let prefix = findings(&report, "type-prefix");
        assert_eq!(prefix.len(), 1, "{}: {}", kind, report);
        assert_eq!(prefix[0]["severity"], "error");
        assert_eq!(
            prefix[0]["message"],
            format!(
                "{} Colour must be prefixed Gitea_ so it cannot collide in the supergraph",
                kind.split_whitespace().last().unwrap()
            )
        );
    }
}

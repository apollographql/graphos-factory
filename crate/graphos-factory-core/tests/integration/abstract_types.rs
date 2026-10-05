//! The selection readers understand a `...` spread (ADR 0058): the
//! abstract-type idiom `... disc->match([wire, { __typename: "T", … }], …)`
//! that maps a discriminated `oneOf` as a union or interface at
//! `connect/v0.4`. Every reader is exercised against one spec-backed probe,
//! `tests/fixtures/media-union/`, whose inventory is built from its
//! `openapi.json` at test time; no pilot has a union.

use graphos_factory_core::reconcile::{parse_selection, resolve_paths, Node, Spread};
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/media-union")
}

/// A copy of the fixture, `edit` applied to each file's text by its relative
/// path, with `.factory/inventory.json` built from the (edited) spec.
fn workspace(edit: impl Fn(&str, String) -> String) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for rel in [
        "openapi.json",
        "media.graphql",
        "supergraph.yaml",
        "template.yaml",
        ".factory/workspace.yaml",
        ".factory/selection.yaml",
    ] {
        let text = std::fs::read_to_string(fixture().join(rel)).unwrap();
        let to = dir.path().join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::write(to, edit(rel, text)).unwrap();
    }
    let spec = dir.path().join("openapi.json");
    let out = dir.path().join(".factory/inventory.json");
    let code = graphos_factory_core::cmd::inventory::main(&[
        "build".to_string(),
        spec.to_string_lossy().to_string(),
        "--out".to_string(),
        out.to_string_lossy().to_string(),
    ]);
    assert_eq!(code, 0, "inventory build");
    dir
}

fn unedited() -> tempfile::TempDir {
    workspace(|_, t| t)
}

fn arms(node: &Node) -> &[graphos_factory_core::reconcile::SpreadArm] {
    match &node.spread {
        Some(Spread::Match(arms)) => arms,
        other => panic!("not a ->match spread: {:?}", other),
    }
}

fn names(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .map(|n| {
            n.alias
                .clone()
                .or_else(|| n.key.as_ref().map(|k| k.join(".")))
                .unwrap_or_else(|| "?".into())
        })
        .collect()
}

// ─── The parser ─────────────────────────────────────────────────────────────

#[test]
fn a_match_spread_parses_into_arms_with_their_typename_candidate_and_fields() {
    let nodes = parse_selection(
        r#"
        id
        ... kind->match(
          ["book", { __typename: "Book", pages: pages, author { name } }],
          ['movie', $ { __typename: $('Movie') minutes }],
          [@, null]
        )
        title
        "#,
    );
    // The fields after the spread survive: the old recovery skipped `...` a
    // character at a time and read the rest from wherever it landed.
    assert_eq!(names(&nodes), ["id", "kind", "title"]);
    let spread = &nodes[1];
    // The discriminator stays the `->match` node it is.
    assert_eq!(spread.key.as_deref(), Some(&["kind".to_string()][..]));
    assert_eq!(spread.methods, ["match"]);
    let arms = arms(spread);
    assert_eq!(arms.len(), 2, "`[@, null]` adds nothing and is not an arm");
    assert_eq!(arms[0].candidate.as_deref(), Some("book"));
    assert_eq!(arms[0].typename.as_deref(), Some("Book"));
    assert_eq!(names(&arms[0].children), ["pages", "author"]);
    assert_eq!(
        names(&arms[0].children[1].children.clone().unwrap()),
        ["name"]
    );
    assert_eq!(arms[1].candidate.as_deref(), Some("movie"));
    assert_eq!(arms[1].typename.as_deref(), Some("Movie"));
    assert_eq!(names(&arms[1].children), ["minutes"]);
}

#[test]
fn a_catch_all_arm_and_a_computed_typename_keep_their_fields() {
    let nodes = parse_selection(
        r#"... $(radius ?? "none")->match(["none", { __typename: "Rect", width, height }], [@, { __typename: kind, radius }])"#,
    );
    let arms = arms(&nodes[0]);
    assert_eq!(arms[0].candidate.as_deref(), Some("none"));
    assert_eq!(names(&arms[0].children), ["width", "height"]);
    assert_eq!(arms[1].candidate, None, "`@` is not a literal candidate");
    assert_eq!(arms[1].typename, None, "a computed __typename is none");
    assert_eq!(names(&arms[1].children), ["radius"]);
}

#[test]
fn every_other_spread_is_unparsed_opaque_and_keyless() {
    for (text, why) in [
        ("... $.payment { a b }", "sub-selection"),
        ("... kind->echo({ a: 1 })", "`->echo`"),
        ("... kind->match([\"a\", \"A\"])", "not an object or `null`"),
        ("... kind->match(\"a\")", "not a `[candidate, value]` pair"),
        ("... $({ a: 1 })", "an expression"),
    ] {
        let nodes = parse_selection(&format!("{}\nafter", text));
        assert_eq!(nodes.len(), 2, "{}: {:?}", text, names(&nodes));
        let n = &nodes[0];
        match &n.spread {
            Some(Spread::Unparsed(reason)) => {
                assert!(reason.contains(why), "{}: {}", text, reason)
            }
            other => panic!("{}: {:?}", text, other),
        }
        assert!(
            n.opaque && n.key.is_none() && n.children.is_none(),
            "{}",
            text
        );
        assert_eq!(names(&nodes[1..]), ["after"], "{}", text);
    }
}

// ─── Wire-side readers ──────────────────────────────────────────────────────

fn obligations(dir: &Path, op: &str) -> graphos_factory_core::obligations::Report {
    graphos_factory_core::obligations::build(dir, op).unwrap()
}

fn classes(r: &graphos_factory_core::obligations::Report) -> Vec<(String, String)> {
    r.response
        .iter()
        .map(|row| (row.path.clone(), row.class.label()))
        .collect()
}

#[test]
fn obligations_maps_every_arm_field_and_passes_check() {
    let ws = unedited();
    let list = obligations(ws.path(), "get:/media");
    assert_eq!(
        classes(&list),
        [
            ("[].id", "mapped"),
            ("[].kind", "mapped"),
            ("[].minutes", "mapped"),
            ("[].pages", "mapped"),
            ("[].rating", "mapped"),
            ("[].title", "mapped"),
        ]
        .map(|(p, c)| (p.to_string(), c.to_string()))
    );
    assert_eq!(list.check_failure(), None);
    let one = obligations(ws.path(), "get:/media/{id}");
    assert!(
        classes(&one).iter().all(|(_, c)| c == "mapped"),
        "{:?}",
        classes(&one)
    );
    assert_eq!(one.response.len(), 6);
    assert_eq!(one.check_failure(), None);
}

#[test]
fn obligations_leaves_an_arm_field_the_schema_drops_unaccounted() {
    // Load-bearing: take `rating` out of the movie arm and it is dropped.
    let ws = workspace(|rel, t| {
        if rel == "media.graphql" {
            t.replace(
                "{ __typename: \"Media_Movie\", minutes, rating }",
                "{ __typename: \"Media_Movie\", minutes }",
            )
        } else {
            t
        }
    });
    let list = obligations(ws.path(), "get:/media");
    let rating = classes(&list)
        .into_iter()
        .find(|(p, _)| p == "[].rating")
        .unwrap();
    assert_eq!(rating.1, "unaccounted");
}

#[test]
fn obligations_reports_an_unparsed_spread_unresolved_and_keeps_what_is_mapped() {
    let ws = workspace(|rel, t| {
        if rel == "media.graphql" {
            t.replace(
                r#"      ... kind->match(
        ["book", { __typename: "Media_Book", pages: pages }],
        ["movie", { __typename: "Media_Movie", minutes, rating }],
        [@, null]
      )"#,
                "      ... kind->echo({ pages: pages })",
            )
        } else {
            t
        }
    });
    let list = obligations(ws.path(), "get:/media");
    for (path, class) in classes(&list) {
        if matches!(path.as_str(), "[].id" | "[].title") {
            assert_eq!(class, "mapped", "{}", path);
        } else {
            assert_eq!(
                class,
                "unresolved (selection not parsed: a `...` spread of `->echo`, not of `->match` arms)",
                "{}",
                path
            );
        }
    }
}

#[test]
fn reconcile_reports_an_excluded_field_an_arm_maps() {
    let ws = workspace(|rel, t| {
        if rel == ".factory/selection.yaml" {
            t.replace(
                "    graphql: { root: query, name: media }",
                "    graphql: { root: query, name: media }\n    fields:\n      exclude:\n        - \"[]>pages\"",
            )
            .replace(
                "    graphql: { root: query, name: mediaById }",
                "    graphql: { root: query, name: mediaById }\n    fields:\n      exclude:\n        - \"rating\"",
            )
        } else {
            t
        }
    });
    let report = graphos_factory_core::reconcile::reconcile_workspace(ws.path(), None).unwrap();
    let text = graphos_factory_core::reconcile::render_report(&report, ".", None);
    assert!(
        text.contains("[]>pages is excluded by the selection but the connector maps it"),
        "{}",
        text
    );
    assert!(
        text.contains("rating is excluded by the selection but the connector maps it"),
        "{}",
        text
    );
}

#[test]
fn reconcile_is_clean_on_the_fixture() {
    let ws = unedited();
    let report = graphos_factory_core::reconcile::reconcile_workspace(ws.path(), None).unwrap();
    let text = graphos_factory_core::reconcile::render_report(&report, ".", None);
    assert!(text.contains("change (0)"), "{}", text);
}

#[test]
fn resolve_paths_merges_object_variants_but_not_a_scalar_one() {
    let shapes = serde_json::json!({
        "A": {"type": "object", "properties": {"k": {"type": "string"}, "a": {"type": "string"}}},
        "B": {"type": "object", "properties": {"k": {"type": "string"}, "b": {"type": "string"}}}
    });
    let shapes = shapes.as_object().unwrap();
    // `c` is in neither variant. The merge is for the arms alone: `deref`
    // does not merge, so a selection with no spread reads as it always did.
    let nodes = parse_selection(
        "... k->match([\"a\", { __typename: \"A\", a c }], [\"b\", { __typename: \"B\", b }])",
    );
    let objects = serde_json::json!({"oneOf": [{"$ref": "#/shapes/A"}, {"$ref": "#/shapes/B"}]});
    let r = resolve_paths(&nodes, &objects, shapes);
    assert!(r.has("a") && r.has("b"));
    assert_eq!(r.unknown, ["c"], "the merged variants are a closed object");
    // A variant that is not an object keeps the shape open, as before, so
    // nothing is unknown against it.
    let mixed = serde_json::json!({"oneOf": [{"$ref": "#/shapes/A"}, {"type": "string"}]});
    let r = resolve_paths(&nodes, &mixed, shapes);
    assert!(r.unknown.is_empty(), "{:?}", r.unknown);
}

#[test]
fn sparse_derives_a_fields_default_through_the_arms() {
    let ws = unedited();
    let inventory: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ws.path().join(".factory/inventory.json")).unwrap(),
    )
    .unwrap();
    let op = inventory["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["key"] == "get:/media/{id}")
        .unwrap();
    let shapes = inventory["shapes"].as_object().unwrap();
    let selection = "... kind->match(['book', $ { __typename: $('Media_Book') id title pages }], ['movie', $ { __typename: $('Media_Movie') id minutes }], [@, null])";
    assert_eq!(
        graphos_factory_core::sparse::derive(op, shapes, selection),
        graphos_factory_core::sparse::Derived::Fields("id,kind,minutes,pages,title".to_string())
    );
    assert!(matches!(
        graphos_factory_core::sparse::derive(op, shapes, "id ... kind->echo(1)"),
        graphos_factory_core::sparse::Derived::NotDerivable(why) if why.contains("spread")
    ));
}

// ─── Type-directed readers ──────────────────────────────────────────────────

fn lint_rules(dir: &Path) -> Vec<(String, String)> {
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
fn wire_enum_drift_reads_an_arm_against_its_member_type_and_variant() {
    let clean = unedited();
    assert!(
        !lint_rules(clean.path())
            .iter()
            .any(|(r, _)| r == "wire-enum-drift"),
        "{:?}",
        lint_rules(clean.path())
    );
    let ws = workspace(|rel, t| {
        if rel == "media.graphql" {
            t.replace("  G\n  PG\n  R\n", "  G\n  PG\n")
        } else {
            t
        }
    });
    let hits: Vec<String> = lint_rules(ws.path())
        .into_iter()
        .filter(|(r, _)| r == "wire-enum-drift")
        .map(|(_, m)| m)
        .collect();
    assert!(
        hits.iter()
            .any(|m| m.contains("Media_Rating") && m.contains("`R`")),
        "{:?}",
        hits
    );
}

#[test]
fn int_overflow_reads_an_arm_against_its_member_type_and_variant() {
    let clean = unedited();
    assert!(!lint_rules(clean.path())
        .iter()
        .any(|(r, _)| r == "int-overflow"));
    let ws = workspace(|rel, t| {
        if rel == "openapi.json" {
            t.replace("\"format\": \"int32\"", "\"format\": \"int64\"")
        } else {
            t
        }
    });
    let hits: Vec<String> = lint_rules(ws.path())
        .into_iter()
        .filter(|(r, _)| r == "int-overflow")
        .map(|(_, m)| m)
        .collect();
    assert!(
        hits.iter()
            .any(|m| m.contains("`Media_Movie.minutes` is Int") && m.contains("format: int64")),
        "{:?}",
        hits
    );
}

// ─── scaffold ───────────────────────────────────────────────────────────────

#[test]
fn scaffold_renders_typename_and_a_fragment_per_member() {
    let ws = unedited();
    let code =
        graphos_factory_core::cmd::scaffold::main(&[ws.path().to_string_lossy().to_string()]);
    assert_eq!(code, 0, "scaffold refused the union operations");
    let doc = std::fs::read_to_string(ws.path().join("tests/cases/media.graphql")).unwrap();
    let body: String = doc
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| format!("{}\n", l))
        .collect();
    assert_eq!(
        body,
        "query {\n  media_media {\n    __typename\n    ... on Media_Book {\n      id\n      title\n      pages\n    }\n    ... on Media_Movie {\n      id\n      title\n      minutes\n      rating\n    }\n  }\n}\n"
    );
    let one = std::fs::read_to_string(ws.path().join("tests/cases/media_by_id.graphql")).unwrap();
    assert!(
        one.contains("... on Media_Book {") && one.contains("... on Media_Movie {"),
        "{}",
        one
    );
}

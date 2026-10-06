//! The four local-validation files `init` writes for this target:
//! `template.yaml`, `supergraph.yaml`, `tests/router.yaml` and the schema
//! header `<directory>.graphql`, their values
//! from the description document, a pre-existing one left alone, and a
//! fresh workspace whose schema uses both placeholders linting clean.

use super::{findings, fixture, graphos_pilot, lint_json, run};
use graphos_factory_targets::targets::graphos::init;
use serde_json::Value;
use std::path::Path;

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_else(|e| panic!("{}: {}", rel, e))
}

fn init_ws(ws: &Path, spec: &str, extra: &[&str]) -> (Option<i32>, String, String) {
    let ws = ws.to_string_lossy().into_owned();
    let spec = fixture(spec).to_string_lossy().into_owned();
    let mut args = vec![
        "init",
        ws.as_str(),
        "--name",
        "gadget-co",
        "--spec",
        spec.as_str(),
        "--created-at",
        "2026-10-01T00:00:00Z",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

/// An absolute server with a path and two schemes, the default security
/// naming the second: every value comes from the document.
#[test]
fn init_writes_the_four_files_from_the_document() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("gadget-co");
    let (code, stdout, stderr) = init_ws(&ws, "gadgets.openapi.json", &[]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    for rel in [
        "gadget-co.graphql",
        "template.yaml",
        "supergraph.yaml",
        "tests/router.yaml",
    ] {
        assert!(stdout.contains(&format!("\n  {}\n", rel)), "{}", stdout);
    }
    assert_eq!(
        read(&ws, "template.yaml"),
        "variables:
  - name: BASE_URL
    description: \"Base URL of the Gadgets REST API, including the /v2 path\"
    test_default: \"https://api.gadgets.test/v2\"
  # The document declares 2 security schemes (bearerAuth, apiKeyHeader).
  # This describes apiKeyHeader; describe the one the subgraph sends.
  - name: AUTH_EXPR
    description: \"Complete Connectors authentication expression for the Gadgets API key (scheme apiKeyHeader, sent as `X-API-Key: <key>`)\"
    test_default: \"{$env.GADGET_CO_TOKEN}\"
"
    );
    assert_eq!(
        read(&ws, "supergraph.yaml"),
        "# rover's compose config for this one subgraph, for the local layers: not the
# user's supergraph. federation_version must equal .factory/workspace.yaml's.
federation_version: =2.15.2
subgraphs:
  gadget-co:
    routing_url: http://localhost
    schema:
      file: gadget-co.graphql
"
    );
    // The key is <subgraph>.<@source name>: the directory, then the service.
    assert_eq!(
        read(&ws, "tests/router.yaml"),
        "# The e2e layer's router config. A source's key is <subgraph>.<@source name>;
# render moves override_url onto WireMock's port for each run.
connectors:
  sources:
    gadget-co.gadget_co:
      override_url: \"http://localhost:8080\"
include_subgraph_errors:
  all: true
"
    );
    // The pin is the workspace's.
    assert!(read(&ws, ".factory/workspace.yaml").contains("federation_version: \"2.15.2\"\n"));
    // The schema header: links at the workspace's pins, the one source with
    // the chosen scheme's header, and no root field yet.
    assert_eq!(
        read(&ws, "gadget-co.graphql"),
        "# Gadgets — Apollo Connectors subgraph.
#
# The schema header init wrote: the spec links and the one source. Each
# root field (type Query, type Mutation) and its @connect is added at
# apply; until the first apply adds one, compose fails on this file.
# This file is the artifact: edit it directly. Selection lives in
# .factory/selection.yaml, the reasoning in .factory/decisions.json.
#
# Auth: X-API-Key: <credential>, AUTH_EXPR with no scheme prefix.
# Read template.yaml's comment on AUTH_EXPR before the first apply.

extend schema
  @link(url: \"https://specs.apollo.dev/federation/v2.15\", import: [\"@key\"])
  @link(url: \"https://specs.apollo.dev/connect/v0.4\", import: [\"@source\", \"@connect\"])

@source(
  name: \"gadget_co\"
  http: {
    baseURL: \"{{BASE_URL}}\"
    headers: [
      { name: \"X-API-Key\", value: \"{{AUTH_EXPR}}\" }
      { name: \"Accept\", value: \"application/json\" }
      { name: \"Content-Type\", value: \"application/json\" }
      { name: \"User-Agent\", value: \"graphos-factory\" }
    ]
  }
)
"
    );
}

/// A bearer scheme keeps its prefix outside the placeholder, as
/// `template.yaml` describes it; a query-parameter scheme sends no header.
#[test]
fn the_schema_header_carries_the_schemes_prefix_or_no_auth_header() {
    let api = |auth: Value| serde_json::json!({"title": "T", "base_urls": [], "auth": [auth]});
    let bearer = init::schema(
        "t",
        "2.15",
        "v0.4",
        Some(&api(serde_json::json!({
            "kind": "bearer", "scheme_name": "b", "header": "Authorization", "prefix": "Bearer "
        }))),
    );
    assert!(
        bearer.contains("      { name: \"Authorization\", value: \"Bearer {{AUTH_EXPR}}\" }\n"),
        "{}",
        bearer
    );
    assert!(
        bearer.contains("# Auth: Authorization: Bearer <credential>; the scheme prefix stays outside AUTH_EXPR.\n"),
        "{}",
        bearer
    );
    let query = init::schema(
        "t",
        "2.15",
        "v0.4",
        Some(&api(serde_json::json!({
            "kind": "api_key", "scheme_name": "q", "query_param": "api_key"
        }))),
    );
    assert!(!query.contains("{{AUTH_EXPR}}"), "{}", query);
    assert!(
        query.contains("# Auth: the `api_key` query parameter"),
        "{}",
        query
    );
    // One source, and no comment that a raw `@source(` count would read as
    // a second.
    for text in [&bearer, &query] {
        assert_eq!(text.matches("@source(").count(), 1, "{}", text);
        assert!(!text.lines().any(|l| l.starts_with("type ")), "{}", text);
    }
}

/// Spec text with `{{…}}` in it reaches the schema's comment inert: a fresh
/// init lints with no placeholder finding, and the one `@source` is the
/// only one a raw count sees.
#[test]
fn a_title_with_braces_is_no_placeholder_in_the_schema_header() {
    let dir = tempfile::tempdir().unwrap();
    let spec_text = std::fs::read_to_string(fixture("gadgets.openapi.json"))
        .unwrap()
        .replacen(
            "\"title\": \"Gadgets\"",
            "\"title\": \"Widget {{X}} API @source(name: \\\"x\\\")\"",
            1,
        );
    assert!(spec_text.contains("Widget {{X}} API"));
    let spec = dir.path().join("braces.openapi.json");
    std::fs::write(&spec, spec_text).unwrap();
    let ws = dir.path().join("gadget-co");
    let (code, stdout, stderr) = run(&[
        "init",
        ws.to_str().unwrap(),
        "--name",
        "gadget-co",
        "--spec",
        spec.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    let schema = read(&ws, "gadget-co.graphql");
    assert!(
        schema.starts_with(
            "# Widget { {X} } API @source (name: \"x\") — Apollo Connectors subgraph.\n"
        ),
        "{}",
        schema
    );
    assert_eq!(schema.matches("@source(").count(), 1, "{}", schema);
    let report = lint_json(&ws);
    for rule in [
        "unknown-placeholder",
        "undeclared-placeholder",
        "multiple-sources",
        "commented-source",
    ] {
        assert!(findings(&report, rule).is_empty(), "{}: {}", rule, report);
    }
}

/// With the pilot's values, the skeleton's links and source are the
/// pilot's, byte for byte, up to where the pilot's own `errors:` begins.
#[test]
fn the_schema_header_is_the_pilots_layout() {
    let pilot = graphos_pilot();
    let api = serde_json::json!({
        "title": "Gitea REST API v1",
        "base_urls": [],
        "auth": [{"kind": "api_key", "scheme_name": "AuthorizationHeaderToken",
                  "header": "Authorization", "prefix": "token "}]
    });
    let skeleton = init::schema("gitea", "2.12", "v0.4", Some(&api));
    let theirs = read(&pilot, "gitea.graphql");
    assert_eq!(skeleton.lines().next(), theirs.lines().next());
    let code = |s: &str| -> String {
        s.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .map(|l| format!("{}\n", l))
            .collect()
    };
    let ours = code(&skeleton);
    let ours = ours.strip_suffix(")\n").unwrap();
    assert!(
        code(&theirs).starts_with(ours),
        "ours:\n{}\ntheirs:\n{}",
        ours,
        theirs
    );
}

/// A fresh workspace and nothing else: the commands that read the schema
/// find it, and each stops on the step actually missing, never on a bare
/// "No such file".
#[test]
fn a_fresh_init_names_the_next_step_instead_of_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("gadget-co");
    let (code, stdout, stderr) = init_ws(&ws, "gadgets.openapi.json", &[]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.test")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.test")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?}: {:?}", args, out);
    };
    git(&["init", "-q", "."]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    let w = ws.to_str().unwrap();
    for (args, exit, says) in [
        (
            vec!["lock", w, "--check"],
            3,
            "no .factory/applied.lock.yaml",
        ),
        (
            vec!["reconcile", w, "--baseline", "HEAD"],
            2,
            "no .factory/selection.yaml yet: run `graphos-factory-core selection draft .`",
        ),
        (
            vec!["links", "apply", w, "--dry-run"],
            1,
            "no .factory/selection.yaml yet: run `graphos-factory-core selection draft .`",
        ),
    ] {
        let (code, stdout, stderr) = run(&args);
        let all = format!("{}{}", stdout, stderr);
        assert_eq!(code, Some(exit), "{:?}: {}", args, all);
        assert!(all.contains(says), "{:?}: {}", args, all);
        assert!(!all.contains("No such file"), "{:?}: {}", args, all);
    }
}

/// A relative server keeps its path on the local stand-in, with a comment
/// saying the host is to be filled in.
#[test]
fn a_relative_server_keeps_its_path_on_a_local_stand_in() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let (code, stdout, stderr) = init_ws(&ws, "relative.openapi.json", &[]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    assert_eq!(
        read(&ws, "template.yaml"),
        "variables:
  # The document's server URL is relative (/api/v3): http://127.0.0.1:8080 is a
  # local stand-in. Put the host the local and live layers call before the path.
  - name: BASE_URL
    description: \"Base URL of the Sprockets REST API, including the /api/v3 path\"
    test_default: \"http://127.0.0.1:8080/api/v3\"
  - name: AUTH_EXPR
    description: \"Complete Connectors authentication expression for the Sprockets bearer token (scheme bearer, sent as `Authorization: Bearer <token>`)\"
    test_default: \"{$env.GADGET_CO_TOKEN}\"
"
    );
}

/// No security scheme: the variable is still written, with a comment that
/// an authless API drops the header and the entry.
#[test]
fn no_security_scheme_still_declares_auth_expr_with_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let (code, stdout, stderr) = init_ws(&ws, "widgets.openapi.json", &[]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    assert_eq!(
        read(&ws, "template.yaml"),
        "variables:
  - name: BASE_URL
    description: \"Base URL of the Widgets REST API\"
    test_default: \"https://api.widgets.test\"
  # The document declares no security scheme. If the API takes no credential,
  # remove the Authorization header from @source and delete this entry (lint reports
  # a declared variable the schema does not use).
  - name: AUTH_EXPR
    description: \"Complete Connectors authentication expression for the Widgets credential\"
    test_default: \"{$env.GADGET_CO_TOKEN}\"
"
    );
}

/// A document with no server URL gets the stand-in alone, never the
/// inventory's unreachable placeholder.
#[test]
fn no_server_url_gets_the_stand_in_not_the_placeholder() {
    let api = serde_json::json!({
        "title": "Bare",
        "base_urls": [graphos_factory_core::openapi::PLACEHOLDER_BASE_URL],
        "auth": [],
    });
    let text = init::template("bare", Some(&api));
    assert!(
        text.contains("    test_default: \"http://127.0.0.1:8080\"\n"),
        "{}",
        text
    );
    assert!(
        text.contains("  # The document declares no server URL"),
        "{}",
        text
    );
    assert!(!text.contains("example.invalid"), "{}", text);
}

/// With the pilot's values, the two fixed-shape files are the pilot's,
/// byte for byte, once the skeleton's comments are set aside.
#[test]
fn the_skeletons_are_the_pilots_layout() {
    let uncommented = |s: String| -> String {
        s.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .map(|l| format!("{}\n", l))
            .collect()
    };
    let pilot = graphos_pilot();
    assert_eq!(
        uncommented(init::supergraph("gitea", "2.15.2")),
        read(&pilot, "supergraph.yaml")
    );
    assert_eq!(
        uncommented(init::router("gitea", "gitea")),
        read(&pilot, "tests/router.yaml")
    );
}

/// A file that already exists is the caller's: left alone and reported,
/// and init still writes the rest. A second init on the workspace is
/// refused outright, so a hand-edited skeleton survives it too.
#[test]
fn an_existing_file_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("tests")).unwrap();
    std::fs::write(ws.join("template.yaml"), "variables: [] # mine\n").unwrap();
    std::fs::write(ws.join("tests/router.yaml"), "# mine\n").unwrap();
    std::fs::write(ws.join("gadget-co.graphql"), "# my schema\n").unwrap();
    let (code, stdout, stderr) = init_ws(&ws, "gadgets.openapi.json", &["--json"]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        report["left_alone"],
        serde_json::json!(["gadget-co.graphql", "template.yaml", "tests/router.yaml"])
    );
    assert_eq!(read(&ws, "gadget-co.graphql"), "# my schema\n");
    let written: Vec<&str> = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(written.contains(&"supergraph.yaml"), "{:?}", written);
    assert!(!written.contains(&"template.yaml"), "{:?}", written);
    assert_eq!(read(&ws, "template.yaml"), "variables: [] # mine\n");
    assert_eq!(read(&ws, "tests/router.yaml"), "# mine\n");

    // The text report says so.
    let other = dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("supergraph.yaml"), "# mine\n").unwrap();
    let (code, stdout, _) = init_ws(&other, "gadgets.openapi.json", &[]);
    assert_eq!(code, Some(0));
    assert!(
        stdout.contains("\n  supergraph.yaml  (exists, left alone)\n"),
        "{}",
        stdout
    );

    // A second init on a workspace refuses, and touches nothing.
    std::fs::write(other.join("tests/router.yaml"), "# edited\n").unwrap();
    let (code, stdout, stderr) = init_ws(&other, "gadgets.openapi.json", &[]);
    assert_eq!(code, Some(2), "{} {}", stdout, stderr);
    assert!(stderr.contains("already holds a workspace"), "{}", stderr);
    assert_eq!(read(&other, "tests/router.yaml"), "# edited\n");
    assert_eq!(read(&other, "supergraph.yaml"), "# mine\n");
}

/// A fresh workspace has no schema, so lint stops there; once a schema
/// that uses both placeholders exists, the skeletons raise nothing.
#[test]
fn the_skeletons_lint_clean_under_a_schema_that_uses_them() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("gadget-co");
    let (code, stdout, stderr) = init_ws(&ws, "gadgets.openapi.json", &[]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    std::fs::write(
        ws.join("gadget-co.graphql"),
        r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.15", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.4", import: ["@source", "@connect"])

@source(
  name: "gadget_co"
  http: {
    baseURL: "{{BASE_URL}}"
    headers: [{ name: "X-API-Key", value: "{{AUTH_EXPR}}" }]
  }
)

"A gadget."
type Gadget_Co_Gadget {
  "The gadget's id."
  id: ID!
  "The gadget's name."
  name: String
}

type Query {
  "One gadget by id."
  gadget_co_gadget(
    "The gadget's id."
    id: ID!
  ): Gadget_Co_Gadget
    @connect(source: "gadget_co", http: { GET: "/gadgets/{$args.id}" }, selection: "id name")
}
"#,
    )
    .unwrap();
    let report = lint_json(&ws);
    for rule in [
        "missing-template",
        "auth-test-default",
        "undeclared-placeholder",
        "unused-template-variable",
        "missing-test-default",
        "unreadable-file",
        "source-name",
    ] {
        assert!(findings(&report, rule).is_empty(), "{}: {}", rule, report);
    }
    assert_eq!(report["errors"], 0, "{}", report);
}

/// Spec text with newlines (and the Unicode line separators) in a server URL
/// and in security-scheme names must not leave a comment or a scalar: each
/// of the three files parses and holds only its own top-level keys.
#[test]
fn spec_text_with_line_breaks_adds_no_keys() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let (code, stdout, stderr) = init_ws(&ws, "injection.openapi.json", &[]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);

    let keys = |rel: &str| -> Vec<String> {
        let text = read(&ws, rel);
        let parsed = graphos_factory_core::yaml::parse(&text)
            .unwrap_or_else(|e| panic!("{} does not parse: {}\n{}", rel, e, text));
        let mut keys: Vec<String> = parsed.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(keys("template.yaml"), ["variables"]);
    assert_eq!(keys("supergraph.yaml"), ["federation_version", "subgraphs"]);
    assert_eq!(
        keys("tests/router.yaml"),
        ["connectors", "include_subgraph_errors"]
    );

    let template = graphos_factory_core::yaml::parse(&read(&ws, "template.yaml")).unwrap();
    let variables = template["variables"].as_array().unwrap();
    let names: Vec<&str> = variables
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["BASE_URL", "AUTH_EXPR"]);
    for v in variables {
        let mut own: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        own.sort();
        assert_eq!(own, ["description", "name", "test_default"], "{}", v);
    }
    // The comments still carry the text, on one line each.
    let text = read(&ws, "template.yaml");
    assert!(
        text.contains("  # The document's server URL is relative (/api injected: true #):"),
        "{}",
        text
    );
    assert!(
        !text.contains("\u{2028}") && !text.contains('\u{85}') && !text.contains('\r'),
        "{:?}",
        text
    );
    for line in text.lines() {
        let t = line.trim_start();
        assert!(
            t.starts_with('#')
                || t.starts_with("- name:")
                || t.starts_with("description:")
                || t.starts_with("test_default:")
                || t == "variables:",
            "unexpected line {:?} in\n{}",
            line,
            text
        );
    }
}

/// Values that go through `quoted` stay one scalar whatever characters the
/// document holds, including the ones JSON leaves raw.
#[test]
fn a_title_with_raw_json_characters_stays_one_scalar() {
    let api = serde_json::json!({
        "title": "a\u{2028}b\u{85}c\u{7f}d\"e\\f\ng",
        "base_urls": ["https://api.test"],
        "auth": [],
    });
    let text = init::template("t", Some(&api));
    let parsed =
        graphos_factory_core::yaml::parse(&text).unwrap_or_else(|e| panic!("{}\n{}", e, text));
    let description = parsed["variables"][0]["description"].as_str().unwrap();
    assert_eq!(
        description,
        "Base URL of the a\u{2028}b\u{85}c\u{7f}d\"e\\f\ng REST API"
    );
}

/// A symlinked `tests/` would carry `tests/router.yaml` to wherever it
/// points: the file is left alone, as for one that exists, and nothing is
/// written through the link.
#[cfg(unix)]
#[test]
fn a_symlinked_tests_directory_is_not_written_through() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, ws.join("tests")).unwrap();
    let (code, stdout, stderr) = init_ws(&ws, "gadgets.openapi.json", &["--json"]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        report["left_alone"],
        serde_json::json!(["tests/router.yaml"])
    );
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    assert!(ws.join("template.yaml").is_file());
    assert!(ws.join("supergraph.yaml").is_file());
    assert!(std::fs::symlink_metadata(ws.join("tests"))
        .unwrap()
        .file_type()
        .is_symlink());

    // A symlink in the file's own place (even a dangling one) is the same.
    let ws2 = dir.path().join("ws2");
    std::fs::create_dir_all(ws2.join("tests")).unwrap();
    let target = dir.path().join("router-target.yaml");
    std::os::unix::fs::symlink(&target, ws2.join("tests/router.yaml")).unwrap();
    let (code, stdout, stderr) = init_ws(&ws2, "gadgets.openapi.json", &[]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    assert!(!target.exists());

    // And a `tests` that is a plain file is left alone, not an INCOMPLETE.
    let ws3 = dir.path().join("ws3");
    std::fs::create_dir_all(&ws3).unwrap();
    std::fs::write(ws3.join("tests"), "mine\n").unwrap();
    let (code, stdout, stderr) = init_ws(&ws3, "gadgets.openapi.json", &["--json"]);
    assert_eq!(code, Some(0), "{} {}", stdout, stderr);
    assert_eq!(read(&ws3, "tests"), "mine\n");
}

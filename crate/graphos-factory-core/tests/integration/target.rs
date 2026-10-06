//! The target seam (ADR 0114), against a synthetic target: placeholders,
//! the variables file's requiredness, rule overrides, a target rule and its
//! `origin`, the tag vocabulary, `ComposeConfig`, and embedded schemas. The
//! core suite never registers a target in its own process (modules share
//! one); these tests hand the target to the library explicitly.

use graphos_factory_core::lint::{lint_workspace, Findings, LintOptions, LintResult};
use graphos_factory_core::target::{
    ComposeConfig, EvidenceLayer, InitInput, LayerInput, LintInput, Override, Target, BARE,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SDL: &str = r#"extend schema
  @link(url: "https://specs.apollo.dev/federation/v2.12", import: ["@key"])
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(
  name: "widget_co"
  http: {
    baseURL: "{{BASE_URL}}"
    headers: [{ name: "Authorization", value: "Bearer {{AUTH_EXPR}}" }]
  }
)

type Widget_Co_Widget {
  id: ID
  name: String
}

type Query {
  widget_co_listWidgets(limit: Int): [Widget_Co_Widget]
    @connect(source: "widget_co", http: { GET: "/widgets" }, selection: "id name")
}
"#;

const TEMPLATE: &str = "variables:\n  - name: BASE_URL\n    description: \"Base URL\"\n    test_default: \"https://api.widgets.test\"\n  - name: AUTH_EXPR\n    description: \"Complete Connectors authentication expression\"\n    test_default: \"{$env.WIDGET_CO_TOKEN}\"\n";

const WORKSPACE: &str = "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\ncontext_mode: generic\n";

fn example_lint(input: &LintInput, findings: &mut Findings) {
    if !input.dir.join("example.yaml").exists() {
        findings.error(
            "missing-example",
            format!("{} needs example.yaml", input.target.name),
            Some("example.yaml"),
            None,
        );
    }
}

fn shout(message: &str) -> String {
    format!("EXAMPLE: {}", message)
}

fn vocabulary() -> Vec<String> {
    vec!["internal".to_string(), "beta".to_string()]
}

fn example_layer(input: &LayerInput) -> Value {
    json!({"status": if input.evidence["layers"]["lint"]["status"] == "pass" { "pass" } else { "fail" }})
}

fn example_files(_: &InitInput) -> Vec<(PathBuf, String)> {
    vec![(PathBuf::from("example.yaml"), "example: true\n".to_string())]
}

const EXAMPLE: Target = Target {
    name: "example",
    placeholders: &["BASE_URL", "AUTH_EXPR", "REGION"],
    variables_file_required: false,
    required_files: &["example.yaml"],
    output_files: &["example.yaml"],
    commands: &[],
    lint: example_lint,
    lint_rules: &["missing-example"],
    tag_vocabulary: vocabulary,
    rule_overrides: &[
        ("multiple-sources", Override::Severity("warn")),
        ("commented-source", Override::Off),
        ("unknown-placeholder", Override::Message(shout)),
    ],
    foreign_types: false,
    evidence_layers: &[EvidenceLayer {
        name: "example_gate",
        run: example_layer,
        gating: false,
    }],
    compose: ComposeConfig {
        federation_spec_version: Some("2.15"),
        link_imports: &["@key", "@tag"],
    },
    init_files: example_files,
    export_gate: None,
    embedded_schemas: &[("example.schema.json", "{\"type\": \"object\"}")],
};

fn workspace(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut all: Vec<(&str, &str)> = vec![
        ("widget-co.graphql", SDL),
        ("template.yaml", TEMPLATE),
        (".factory/workspace.yaml", WORKSPACE),
    ];
    for (path, text) in files {
        all.retain(|(p, _)| p != path);
        if !text.is_empty() {
            all.push((path, text));
        }
    }
    for (path, text) in all {
        let p = dir.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

fn lint(dir: &Path, target: &Target) -> LintResult {
    lint_workspace(
        dir,
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target,
        },
    )
}

fn finding<'a>(r: &'a LintResult, rule: &str) -> Option<&'a graphos_factory_core::lint::Finding> {
    r.findings.iter().find(|f| f.rule == rule)
}

#[test]
fn a_placeholder_outside_the_targets_set_is_unknown_and_inside_it_is_not() {
    let sdl = SDL.replace(
        "name: String",
        "name: String\n  \"{{REGION}}\"\n  region: String",
    );
    let dir = workspace(&[("widget-co.graphql", &sdl)]);
    let bare = lint(dir.path(), &BARE);
    let f = finding(&bare, "unknown-placeholder").expect("REGION is not one of BARE's");
    assert_eq!(
        f.message,
        "{{REGION}} is not one of this target's placeholders (BASE_URL, AUTH_EXPR); an unknown placeholder cannot be rendered"
    );
    let example = lint(dir.path(), &EXAMPLE);
    assert!(finding(&example, "unknown-placeholder").is_none());
}

#[test]
fn a_message_override_rewrites_the_core_message_and_keeps_the_rule() {
    let sdl = SDL.replace(
        "name: String",
        "name: String\n  \"{{ZONE}}\"\n  zone: String",
    );
    let dir = workspace(&[("widget-co.graphql", &sdl)]);
    let r = lint(dir.path(), &EXAMPLE);
    let f = finding(&r, "unknown-placeholder").unwrap();
    assert!(
        f.message.starts_with("EXAMPLE: {{ZONE}} is not one of"),
        "{}",
        f.message
    );
    assert_eq!(f.origin, "core");
}

#[test]
fn a_severity_override_and_an_off_override_change_only_their_rules() {
    let sdl = SDL.replace(
        "type Widget_Co_Widget",
        "@source(name: \"widget_co_admin\", http: { baseURL: \"{{BASE_URL}}\" })\n# was: @source(name: \"x\")\ntype Widget_Co_Widget",
    );
    let dir = workspace(&[("widget-co.graphql", &sdl)]);
    let bare = lint(dir.path(), &BARE);
    assert_eq!(
        finding(&bare, "multiple-sources").unwrap().severity,
        "error"
    );
    let f = finding(&bare, "commented-source").unwrap();
    assert!(
        f.message.contains("this target counts the raw text"),
        "{}",
        f.message
    );
    assert_eq!(
        finding(&bare, "multiple-sources").unwrap().message,
        "the schema declares 2 @source directives; this target renders exactly one"
    );
    let example = lint(dir.path(), &EXAMPLE);
    assert_eq!(
        finding(&example, "multiple-sources").unwrap().severity,
        "warn"
    );
    assert!(finding(&example, "commented-source").is_none());
    // The rules no override names are untouched.
    assert_eq!(
        finding(&example, "source-name").map(|f| f.severity.as_str()),
        finding(&bare, "source-name").map(|f| f.severity.as_str())
    );
}

#[test]
fn a_target_rule_runs_after_the_core_and_carries_the_targets_origin() {
    let dir = workspace(&[]);
    let r = lint(dir.path(), &EXAMPLE);
    let last = r.findings.last().expect("the target rule fires");
    assert_eq!(last.rule, "missing-example");
    assert_eq!(last.origin, "example");
    assert_eq!(last.message, "example needs example.yaml");
    assert!(r
        .findings
        .iter()
        .filter(|f| f.rule != "missing-example")
        .all(|f| f.origin == "core"));
    let dir = workspace(&[("example.yaml", "example: true\n")]);
    assert!(finding(&lint(dir.path(), &EXAMPLE), "missing-example").is_none());
}

#[test]
fn the_variables_file_is_required_only_where_the_target_requires_it() {
    let dir = workspace(&[("template.yaml", "")]);
    let bare = lint(dir.path(), &BARE);
    assert_eq!(
        finding(&bare, "missing-template").unwrap().message,
        "template.yaml is missing; this target renders the schema from it"
    );
    assert!(finding(&lint(dir.path(), &EXAMPLE), "missing-template").is_none());
}

#[test]
fn unknown_tag_reads_the_targets_vocabulary_and_bare_restricts_nothing() {
    let sdl = SDL.replace(
        "  name: String\n",
        "  name: String @tag(name: \"internal\")\n  label: String @tag(name: \"secret-ish\")\n",
    );
    let dir = workspace(&[("widget-co.graphql", &sdl)]);
    assert!(finding(&lint(dir.path(), &BARE), "unknown-tag").is_none());
    let r = lint(dir.path(), &EXAMPLE);
    let tags: Vec<&str> = r
        .findings
        .iter()
        .filter(|f| f.rule == "unknown-tag")
        .map(|f| f.message.as_str())
        .collect();
    assert_eq!(
        tags,
        vec!["@tag(name: \"secret-ish\") is not in this target's tag vocabulary (internal, beta); no tag-based policy reads it — use a listed name"]
    );
}

fn drift(r: &LintResult) -> Vec<&str> {
    r.findings
        .iter()
        .filter(|f| f.rule == "federation-drift")
        .map(|f| f.message.as_str())
        .collect()
}

/// `link_imports` is the set a schema may import: the SDL imports `@key`
/// and applies neither, and leaves `@tag` out, which is no drift; only a
/// directive of the set the schema applies unimported is.
#[test]
fn compose_config_holds_the_federation_link_to_the_targets_version_and_imports() {
    let dir = workspace(&[]);
    assert!(finding(&lint(dir.path(), &BARE), "federation-drift").is_none());
    assert_eq!(
        drift(&lint(dir.path(), &EXAMPLE)),
        vec!["the schema links federation/v2.12 but this target composes federation 2.15"]
    );
    // Applied, not imported: drift. A `#` comment naming it is no
    // application, and BARE, with an empty set, checks no import.
    let tagged = SDL.replace(
        "type Query {",
        "# @tag(name: \"commented\") is not an application\ntype Query @tag(name: \"public\") {",
    );
    let dir = workspace(&[("widget-co.graphql", &tagged)]);
    assert_eq!(
        drift(&lint(dir.path(), &EXAMPLE)),
        vec![
            "the schema links federation/v2.12 but this target composes federation 2.15",
            "the schema applies @tag but the federation @link does not import it",
        ]
    );
    assert!(finding(&lint(dir.path(), &BARE), "federation-drift").is_none());
    // Imported and applied: no drift for it.
    let imported = tagged.replace("import: [\"@key\"]", "import: [\"@key\", \"@tag\"]");
    let dir = workspace(&[("widget-co.graphql", &imported)]);
    assert_eq!(
        drift(&lint(dir.path(), &EXAMPLE)),
        vec!["the schema links federation/v2.12 but this target composes federation 2.15"]
    );
}

/// Two overrides on one rule apply in order: a target may set a rule's
/// severity and rewrite its message both, and `lint --json` names the
/// rule once.
#[test]
fn every_override_a_target_lists_for_a_rule_applies_in_order() {
    fn reworded(message: &str) -> String {
        message.replace(
            "this target renders exactly one",
            "a second one is allowed here",
        )
    }
    const BOTH: Target = Target {
        name: "beta",
        rule_overrides: &[
            ("multiple-sources", Override::Severity("warn")),
            ("multiple-sources", Override::Message(reworded)),
        ],
        ..BARE
    };
    assert_eq!(BOTH.overridden_rules(), vec!["multiple-sources"]);
    let two = SDL.replacen(
        "type Widget_Co_Widget",
        "@source(name: \"widget_co\", http: { baseURL: \"{{BASE_URL}}\" })\n\ntype Widget_Co_Widget",
        1,
    );
    let dir = workspace(&[("widget-co.graphql", &two)]);
    let core = lint(dir.path(), &BARE);
    let f = finding(&core, "multiple-sources").expect("multiple-sources");
    assert_eq!(f.severity, "error");
    let r = lint(dir.path(), &BOTH);
    let f = finding(&r, "multiple-sources").expect("multiple-sources");
    assert_eq!(f.severity, "warn");
    assert_eq!(
        f.message,
        "the schema declares 2 @source directives; a second one is allowed here"
    );
}

#[test]
fn a_targets_embedded_schema_loads_by_name_beside_the_cores() {
    assert!(graphos_factory_core::schemas::load_from("example.schema.json", None, &[]).is_none());
    assert_eq!(
        graphos_factory_core::schemas::load_from(
            "example.schema.json",
            None,
            EXAMPLE.embedded_schemas
        ),
        Some(json!({"type": "object"}))
    );
    // A core name never resolves to a target's copy.
    let shadow = [("workspace.schema.json", "{\"type\": \"string\"}")];
    assert_ne!(
        graphos_factory_core::schemas::load_from("workspace.schema.json", None, &shadow),
        Some(json!({"type": "string"}))
    );
}

#[test]
fn bare_adds_nothing_but_the_two_core_placeholders() {
    assert_eq!(BARE.placeholders, &["BASE_URL", "AUTH_EXPR"]);
    assert!(BARE.variables_file_required);
    assert!(BARE.commands.is_empty());
    assert!(BARE.rule_overrides.is_empty());
    assert!(BARE.evidence_layers.is_empty());
    assert!(BARE.output_files.is_empty());
    assert!((BARE.tag_vocabulary)().is_empty());
    assert!(BARE.export_gate.is_none());
    assert!(!BARE.foreign_types);
    let none = json!({});
    assert!((BARE.init_files)(&InitInput {
        workspace: &none,
        inventory: &none
    })
    .is_empty());
    assert_eq!(
        (EXAMPLE.init_files)(&InitInput {
            workspace: &none,
            inventory: &none
        }),
        vec![(PathBuf::from("example.yaml"), "example: true\n".to_string())]
    );
}

/// `Target::foreign_types` (ADR 0132): a target that honours a resolved
/// decision's foreign types lets the declared type keep its owner's name
/// and reads it as the owner's entity, and hands the set to its own rules;
/// one that does not (BARE) holds it as any other type. An open record
/// declares nothing under either.
#[test]
fn a_target_that_honours_foreign_types_exempts_only_a_resolved_declaration() {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    fn record(input: &LintInput, _: &mut Findings) {
        SEEN.lock()
            .unwrap()
            .extend(input.foreign_types.iter().cloned());
    }
    const HONOURS: Target = Target {
        name: "honours",
        foreign_types: true,
        lint: record,
        ..BARE
    };
    let extended = SDL.replacen(
        "type Query {",
        "type Product @key(fields: \"id\") {\n  id: ID!\n  widgetCount: Int\n    @connect(source: \"widget_co\", http: { GET: \"/widgets?product={$this.id}\" }, selection: \"$.count\")\n}\n\ntype Query {",
        1,
    );
    let dir = workspace(&[("widget-co.graphql", &extended)]);
    let declare = |resolved: bool| {
        let mut argv: Vec<String> = [
            "add",
            &dir.path().to_string_lossy(),
            "--title",
            "Product is owned by the products subgraph",
            "--question",
            "Which subgraph owns Product?",
            "--foreign-type",
            "Product",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if resolved {
            argv.extend(["--resolved", "--note", "the user said so"].map(String::from));
        }
        assert_eq!(graphos_factory_core::cmd::decisions::main(&argv), 0);
    };
    let rules = |r: &LintResult| -> Vec<String> {
        r.findings
            .iter()
            .filter(|f| f.rule == "type-prefix" || f.rule.starts_with("entity-"))
            .map(|f| f.rule.clone())
            .collect()
    };
    let held = vec![
        "type-prefix".to_string(),
        "entity-field-unresolved".to_string(),
        "entity-without-consumer".to_string(),
        "entity-without-lookup".to_string(),
    ];

    declare(false);
    assert_eq!(rules(&lint(dir.path(), &HONOURS)), held);
    assert!(SEEN.lock().unwrap().is_empty());

    declare(true);
    assert_eq!(rules(&lint(dir.path(), &BARE)), held);
    assert!(rules(&lint(dir.path(), &HONOURS)).is_empty());
    assert_eq!(*SEEN.lock().unwrap(), vec!["Product".to_string()]);
}

/// The harness binary is the core against BARE: `skill.name` is the
/// target's name, and `--help` lists no target command.
#[test]
fn the_core_binary_runs_against_bare() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("\n  evidence "), "{}", help);
    assert!(help.ends_with("  version\n"), "{}", help);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["lint", "--json", "/nonexistent-workspace"])
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["target"],
        json!({"name": "core", "rules": [], "overrides": []})
    );
    assert!(!help.contains("[target "), "{}", help);
}

// Target selection (ADR 0114, Phase 8c), against two synthetic targets.

const ALPHA: Target = Target {
    name: "alpha",
    ..BARE
};
const BETA: Target = Target {
    name: "beta",
    placeholders: &["BASE_URL", "AUTH_EXPR", "REGION"],
    ..BARE
};
static TWO: &[Target] = &[ALPHA, BETA];
static ONE: &[Target] = &[BETA];

fn workspace_named(name: Option<&str>) -> tempfile::TempDir {
    let text = match name {
        Some(n) => WORKSPACE.replace("name: example", &format!("name: {}", n)),
        None => WORKSPACE.replace("skill:\n  name: example\n  version: 0.1.0\n", ""),
    };
    workspace(&[(".factory/workspace.yaml", &text)])
}

#[test]
fn a_workspace_runs_against_the_target_its_skill_name_records() {
    use graphos_factory_core::target::{for_workspace, recorded_name, Unknown};
    let beta = workspace_named(Some("beta"));
    assert_eq!(recorded_name(beta.path()).as_deref(), Some("beta"));
    assert_eq!(
        for_workspace(TWO, beta.path()).unwrap().map(|t| t.name),
        Some("beta")
    );
    let alpha = workspace_named(Some("alpha"));
    assert_eq!(
        for_workspace(TWO, alpha.path()).unwrap().map(|t| t.name),
        Some("alpha")
    );
    // A name no registered target carries is refused, listing them.
    let gamma = workspace_named(Some("gamma"));
    let err = for_workspace(TWO, gamma.path())
        .map(|t| t.map(|t| t.name))
        .unwrap_err();
    assert_eq!(
        err,
        Unknown {
            recorded: "gamma".to_string(),
            registered: "alpha, beta".to_string()
        }
    );
    assert_eq!(
        err.to_string(),
        "workspace.yaml skill.name is \"gamma\", which names no target this binary serves (registered: alpha, beta)"
    );
    // One target registered: its own workspace, and only its own.
    assert_eq!(
        for_workspace(ONE, beta.path()).unwrap().map(|t| t.name),
        Some("beta")
    );
    assert!(for_workspace(ONE, alpha.path()).is_err());
    // Nothing to choose by: no target registered (the core alone), no
    // workspace, or a workspace that records no name.
    assert_eq!(
        for_workspace(&[], gamma.path()).map(|t| t.is_some()),
        Ok(false)
    );
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        for_workspace(TWO, empty.path()).map(|t| t.is_some()),
        Ok(false)
    );
    let unnamed = workspace_named(None);
    assert_eq!(recorded_name(unnamed.path()), None);
    assert_eq!(
        for_workspace(TWO, unnamed.path()).map(|t| t.is_some()),
        Ok(false)
    );
}

/// The name is read through custody (ADR 0025): a symlinked
/// workspace.yaml records nothing, and the command reports it.
#[cfg(unix)]
#[test]
fn a_symlinked_workspace_file_names_no_target() {
    let real = workspace_named(Some("beta"));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".factory")).unwrap();
    std::os::unix::fs::symlink(
        real.path().join(".factory/workspace.yaml"),
        ws.path().join(".factory/workspace.yaml"),
    )
    .unwrap();
    assert_eq!(graphos_factory_core::target::recorded_name(ws.path()), None);
}

#[test]
fn init_takes_the_target_flag_and_requires_it_only_with_more_than_one() {
    use graphos_factory_core::target::for_init;
    assert_eq!(for_init(TWO, Some("beta")).map(|t| t.name), Ok("beta"));
    assert_eq!(for_init(TWO, Some("alpha")).map(|t| t.name), Ok("alpha"));
    assert_eq!(
        for_init(TWO, None).map(|t| t.name),
        Err((
            "target-required",
            "--target NAME is required: this binary serves more than one target (alpha, beta)"
                .to_string()
        ))
    );
    assert_eq!(
        for_init(TWO, Some("gamma")).map(|t| t.name),
        Err((
            "target-unknown",
            "--target \"gamma\" names no target this binary serves (alpha, beta); a gamma workspace is made by `gamma init`".to_string()
        ))
    );
    // Optional with one; the core alone writes its own.
    assert_eq!(for_init(ONE, None).map(|t| t.name), Ok("beta"));
    assert!(for_init(ONE, Some("alpha")).is_err());
    assert_eq!(for_init(&[], None).map(|t| t.name), Ok(BARE.name));
}

#[test]
fn by_name_compares_a_targets_name_with_the_recorded_string_only() {
    use graphos_factory_core::target::{by_name, names};
    assert_eq!(by_name(TWO, "beta").map(|t| t.placeholders.len()), Some(3));
    assert!(by_name(TWO, "Beta").is_none());
    assert!(by_name(TWO, "").is_none());
    assert_eq!(names(TWO), "alpha, beta");
    assert_eq!(names(&[]), "");
}

/// The core binary registers no target: a workspace naming any target at
/// all runs against BARE rather than being refused, and `init --target`
/// with one it does not have is a usage error.
#[test]
fn the_core_binary_refuses_no_workspace_by_its_name() {
    let ws = workspace_named(Some("gamma"));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["lint", "--json"])
        .arg(ws.path())
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["target"]["name"], "core");
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("init")
        .arg(dir.path().join("w"))
        .args(["--name", "widget-co", "--spec", "/nonexistent.json"])
        .args(["--target", "gamma", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["code"], "target-unknown");
}

use graphos_factory_core::render::{
    env_override_name, env_var_from_auth_expr, render_schema, to_config_expressions,
};
use serde_json::{json, Value};
use std::collections::HashMap;

fn variables() -> Vec<Value> {
    vec![
        json!({"name": "BASE_URL", "test_default": "https://api.widgets.test"}),
        json!({"name": "AUTH_EXPR", "test_default": "{$env.WIDGET_CO_TOKEN}"}),
    ]
}

const SDL: &str = "baseURL: \"{{BASE_URL}}\"\nheaders: [{ name: \"Authorization\", value: \"Bearer {{AUTH_EXPR}}\" }]\n";

#[test]
fn placeholders_are_replaced_by_their_test_defaults() {
    let r = render_schema(SDL, &variables(), "widget_co", &HashMap::new()).unwrap();
    assert!(r.sdl.contains("baseURL: \"https://api.widgets.test\""));
    assert!(r.sdl.contains("value: \"Bearer {$env.WIDGET_CO_TOKEN}\""));
    assert!(!r.sdl.contains("{{"));
}

#[test]
fn an_exported_service_name_variable_overrides_the_test_default() {
    let env: HashMap<String, String> = [(
        "WIDGET_CO_BASE_URL".to_string(),
        "http://localhost:8080".to_string(),
    )]
    .into_iter()
    .collect();
    let r = render_schema(SDL, &variables(), "widget_co", &env).unwrap();
    assert!(r.sdl.contains("baseURL: \"http://localhost:8080\""));
    assert_eq!(
        env_override_name("widget_co", "BASE_URL"),
        "WIDGET_CO_BASE_URL"
    );
}

#[test]
fn a_placeholder_the_template_does_not_declare_is_refused() {
    let err = render_schema(
        &format!("{}extra: \"{{{{REGION}}}}\"", SDL),
        &variables(),
        "widget_co",
        &HashMap::new(),
    )
    .unwrap_err();
    assert!(err.contains("REGION"));
}

#[test]
fn a_variable_with_no_test_default_is_refused() {
    let vars = vec![json!({"name": "BASE_URL"}), variables()[1].clone()];
    let err = render_schema(SDL, &vars, "widget_co", &HashMap::new()).unwrap_err();
    assert!(err.contains("no test_default"));
}

#[test]
fn auth_expr_must_render_a_complete_unquoted_local_expression() {
    let bad = vec![
        variables()[0].clone(),
        json!({"name": "AUTH_EXPR", "test_default": "test-token"}),
    ];
    assert!(render_schema(SDL, &bad, "widget_co", &HashMap::new())
        .unwrap_err()
        .contains("complete unquoted local expression"));
    let quoted = vec![
        variables()[0].clone(),
        json!({"name": "AUTH_EXPR", "test_default": "{$env.'WIDGET_CO_TOKEN'}"}),
    ];
    assert!(render_schema(SDL, &quoted, "widget_co", &HashMap::new())
        .unwrap_err()
        .contains("complete unquoted local expression"));
}

#[test]
fn env_var_from_auth_expr_reads_the_environment_name_or_nothing() {
    assert_eq!(
        env_var_from_auth_expr(Some("{$env.WIDGET_CO_TOKEN}")),
        Some("WIDGET_CO_TOKEN".to_string())
    );
    assert_eq!(env_var_from_auth_expr(Some("Bearer {$env.X}")), None);
    assert_eq!(env_var_from_auth_expr(None), None);
}

#[test]
fn unit_rendering_rewrites_env_to_config_and_names_the_variables() {
    let r = render_schema(SDL, &variables(), "widget_co", &HashMap::new()).unwrap();
    let (sdl, names) = to_config_expressions(&r.sdl);
    assert!(sdl.contains("{$config.WIDGET_CO_TOKEN}"));
    assert!(!sdl.contains("$env."));
    assert_eq!(names, vec!["WIDGET_CO_TOKEN"]);
}

#[test]
fn args_and_this_expressions_are_left_alone_by_the_unit_rewrite() {
    let sdl =
        "selection: \"id name\" http: { GET: \"/widgets/{$args.id}\" } value: \"{$this.token}\"";
    assert_eq!(to_config_expressions(sdl).0, sdl);
}

// ── tests/router.yaml for one run: ports from the suite, not the file ──

const ROUTER_YAML: &str = "connectors:\n  sources:\n    widget-co.widget_co:\n      override_url: \"http://localhost:8080\"\ninclude_subgraph_errors:\n  all: true\n";

#[test]
fn router_config_moves_local_override_urls_to_the_wiremock_port_and_sets_the_listeners() {
    let out = graphos_factory_core::render::router_config(ROUTER_YAML, 8090, 4100, 8188).unwrap();
    let parsed = graphos_factory_core::yaml::parse(&out).unwrap();
    assert_eq!(
        parsed["connectors"]["sources"]["widget-co.widget_co"]["override_url"],
        "http://localhost:8090"
    );
    assert_eq!(parsed["supergraph"]["listen"], "127.0.0.1:4100");
    assert_eq!(parsed["health_check"]["listen"], "127.0.0.1:8188");
    assert_eq!(parsed["include_subgraph_errors"]["all"], true);
}

#[test]
fn router_config_keeps_a_path_and_other_listener_keys() {
    let text = "connectors:\n  sources:\n    a.c:\n      override_url: \"http://127.0.0.1:8080/api/v1\"\nsupergraph:\n  introspection: true\n";
    let out = graphos_factory_core::render::router_config(text, 9000, 4000, 8088).unwrap();
    let parsed = graphos_factory_core::yaml::parse(&out).unwrap();
    assert_eq!(
        parsed["connectors"]["sources"]["a.c"]["override_url"],
        "http://127.0.0.1:9000/api/v1"
    );
    assert_eq!(parsed["supergraph"]["introspection"], true);
    assert_eq!(parsed["supergraph"]["listen"], "127.0.0.1:4000");
}

#[test]
fn router_config_refuses_a_file_that_is_not_a_mapping() {
    assert!(graphos_factory_core::render::router_config("- a\n- b\n", 1, 2, 3).is_err());
}

#[test]
fn router_config_handles_ipv6_userinfo_and_an_existing_listen_key() {
    let text = "connectors:\n  sources:\n    a.b:\n      override_url: \"http://[::1]/api\"\n    a.c:\n      override_url: \"http://u:p@localhost:8080\"\n    a.d:\n      override_url: \"http://[::1]:8080/x\"\nsupergraph:\n  listen: 0.0.0.0:9999\n  introspection: true\nhealth_check:\n  enabled: true\n";
    let out = graphos_factory_core::render::router_config(text, 8090, 4100, 8188).unwrap();
    let parsed = graphos_factory_core::yaml::parse(&out).unwrap();
    let sources = &parsed["connectors"]["sources"];
    assert_eq!(sources["a.b"]["override_url"], "http://[::1]:8090/api");
    assert_eq!(sources["a.c"]["override_url"], "http://u:p@localhost:8090");
    assert_eq!(sources["a.d"]["override_url"], "http://[::1]:8090/x");
    assert_eq!(
        parsed["supergraph"]["listen"], "127.0.0.1:4100",
        "the run's port wins"
    );
    assert_eq!(parsed["supergraph"]["introspection"], true);
    assert_eq!(parsed["health_check"]["listen"], "127.0.0.1:8188");
    assert_eq!(parsed["health_check"]["enabled"], true);
}

#[test]
fn router_config_refuses_an_override_url_it_cannot_move_and_a_config_with_none() {
    let remote = "connectors:\n  sources:\n    a.b:\n      override_url: \"https://sandbox.example.com/v1\"\n";
    let err = graphos_factory_core::render::router_config(remote, 8090, 4100, 8188).unwrap_err();
    assert!(err.contains("sandbox.example.com"), "{}", err);
    let schemeless = "connectors:\n  sources:\n    a.b:\n      override_url: \"localhost:8080\"\n";
    let err =
        graphos_factory_core::render::router_config(schemeless, 8090, 4100, 8188).unwrap_err();
    assert!(err.contains("localhost:8080"), "{}", err);
    let none = "include_subgraph_errors:\n  all: true\n";
    let err = graphos_factory_core::render::router_config(none, 8090, 4100, 8188).unwrap_err();
    assert!(err.contains("no connectors.sources"), "{}", err);
}

/// The command: ports from flags, then the environment, then the defaults;
/// no router config unless a port flag asks; a bad port is an error.
#[test]
fn the_render_command_resolves_ports_and_writes_the_router_config_only_on_request() {
    let pilot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    assert!(pilot.join("tests/router.yaml").exists());
    let out = tempfile::tempdir().unwrap();
    let bin = env!("CARGO_BIN_EXE_graphos-factory-bare");
    let run = |args: &[&str], env: &[(&str, &str)]| -> (i32, String, String) {
        let mut cmd = std::process::Command::new(bin);
        cmd.arg("render")
            .arg(pilot.to_str().unwrap())
            .arg("--out")
            .arg(out.path().to_str().unwrap())
            .args(args)
            .env_remove("WIREMOCK_PORT")
            .env_remove("ROUTER_PORT")
            .env_remove("ROUTER_HEALTH_PORT");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let o = cmd.output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )
    };
    // No port flag: no router config at all, whatever the environment says.
    let (code, stdout, _) = run(&[], &[("WIREMOCK_PORT", "8090")]);
    assert_eq!(code, 0);
    assert!(!stdout.contains("ROUTER_CONFIG="), "{}", stdout);
    assert!(!out.path().join("router.run.yaml").exists());
    // A flag, an environment variable and a default, together.
    let (code, stdout, stderr) = run(&["--wiremock-port", "8090"], &[("ROUTER_PORT", "4321")]);
    assert_eq!(code, 0, "{}", stderr);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("ROUTER_CONFIG="))
        .expect("ROUTER_CONFIG line");
    let cfg = std::fs::read_to_string(unquote(line.trim_start_matches("ROUTER_CONFIG="))).unwrap();
    let parsed = graphos_factory_core::yaml::parse(&cfg).unwrap();
    assert_eq!(
        parsed["connectors"]["sources"]["gitea.gitea"]["override_url"],
        "http://localhost:8090"
    );
    assert_eq!(parsed["supergraph"]["listen"], "127.0.0.1:4321");
    assert_eq!(parsed["health_check"]["listen"], "127.0.0.1:8088");
    // Not a port.
    let (code, _, stderr) = run(&["--router-port", "many"], &[]);
    assert_eq!(code, 1);
    assert!(stderr.contains("is not a port"), "{}", stderr);
}

// ── the KEY=value lines are shell words: `eval` them, do not split them ──

/// The value of one `KEY=value` line as the shell would see it after `eval`.
fn unquote(word: &str) -> String {
    let inner = word
        .strip_prefix('\'')
        .and_then(|w| w.strip_suffix('\''))
        .unwrap_or_else(|| panic!("not a shell-quoted word: {}", word));
    inner.replace(r"'\''", "'")
}

#[test]
fn sh_quote_makes_one_word_of_whitespace_and_survives_a_quote() {
    use graphos_factory_core::render::sh_quote;
    assert_eq!(sh_quote("A B"), "'A B'");
    assert_eq!(sh_quote(""), "''");
    assert_eq!(sh_quote("/tmp/my dir/x.graphql"), "'/tmp/my dir/x.graphql'");
    assert_eq!(sh_quote("it's"), r"'it'\''s'");
    assert_eq!(unquote(&sh_quote("it's a; rm -rf /")), "it's a; rm -rf /");
}

/// A workspace whose schema uses *two* `{$env.NAME}` credentials — one from
/// AUTH_EXPR, one written into the schema directly, as Adobe Analytics
/// (`Authorization: Bearer` plus `x-api-key`) and Google SA360 both need.
/// `render --unit` then reports two names in CONFIG_VARS, and unit.sh does
/// `eval "$(... render ... --unit)"`: unquoted, the second name is run as a
/// command, the layer dies with `NAME: command not found`, and `evidence`
/// records connector_unit as `skipped — tool missing` instead of failing.
#[test]
fn two_credentials_make_config_vars_one_shell_word_that_is_safe_to_eval() {
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    std::fs::create_dir_all(w.join(".factory")).unwrap();
    std::fs::write(
        w.join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: two_creds\ndirectory: two-creds\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\n",
    )
    .unwrap();
    std::fs::write(
        w.join("template.yaml"),
        "variables:\n  - name: BASE_URL\n    test_default: \"https://api.example.test\"\n  - name: AUTH_EXPR\n    test_default: \"{$env.TWO_CREDS_TOKEN}\"\n",
    )
    .unwrap();
    std::fs::write(
        w.join("two-creds.graphql"),
        "baseURL: \"{{BASE_URL}}\"\nheaders: [{ name: \"Authorization\", value: \"Bearer {{AUTH_EXPR}}\" }, { name: \"x-api-key\", value: \"{$env.TWO_CREDS_CLIENT_ID}\" }]\n",
    )
    .unwrap();
    std::fs::write(
        w.join("supergraph.yaml"),
        "federation_version: \"=2.12.0\"\nsubgraphs:\n  two-creds:\n    routing_url: http://localhost:8080\n    schema:\n      file: two-creds.graphql\n",
    )
    .unwrap();

    let out = tempfile::tempdir().unwrap();
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "render",
            w.to_str().unwrap(),
            "--out",
            out.path().to_str().unwrap(),
            "--unit",
        ])
        .env_remove("TWO_CREDS_BASE_URL")
        .output()
        .unwrap();
    let stdout = String::from_utf8(o.stdout).unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    // Still exactly one CONFIG_VARS line.
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with("CONFIG_VARS="))
        .collect();
    assert_eq!(lines.len(), 1, "{}", stdout);

    // What unit.sh does. Before the fix this exits non-zero with
    // "TWO_CREDS_CLIENT_ID: command not found" under `set -eu`.
    let shell = std::process::Command::new("sh")
        .arg("-c")
        .arg("set -eu\neval \"$1\"\nfor n in $CONFIG_VARS; do printf 'VAR:%s\\n' \"$n\"; done\nprintf 'SCHEMA:%s\\n' \"$RENDERED_SCHEMA\"\n")
        .arg("sh")
        .arg(&stdout)
        .output()
        .unwrap();
    let shell_out = String::from_utf8_lossy(&shell.stdout).to_string();
    let shell_err = String::from_utf8_lossy(&shell.stderr).to_string();
    assert_eq!(
        shell.status.code(),
        Some(0),
        "eval of render's output failed: {}{}",
        shell_out,
        shell_err
    );
    assert!(shell_err.is_empty(), "{}", shell_err);
    // Both names round-trip through the eval, as separate words of $CONFIG_VARS.
    assert!(shell_out.contains("VAR:TWO_CREDS_TOKEN\n"), "{}", shell_out);
    assert!(
        shell_out.contains("VAR:TWO_CREDS_CLIENT_ID\n"),
        "{}",
        shell_out
    );
    // …and the rendered schema really carries both as $config expressions.
    let schema = shell_out
        .lines()
        .find_map(|l| l.strip_prefix("SCHEMA:"))
        .expect("SCHEMA line");
    let rendered = std::fs::read_to_string(schema).unwrap();
    assert!(
        rendered.contains("{$config.TWO_CREDS_TOKEN}"),
        "{}",
        rendered
    );
    assert!(
        rendered.contains("{$config.TWO_CREDS_CLIENT_ID}"),
        "{}",
        rendered
    );
    assert!(!rendered.contains("$env."), "{}", rendered);

    assert_eq!(
        unquote(lines[0].trim_start_matches("CONFIG_VARS=")),
        "TWO_CREDS_TOKEN TWO_CREDS_CLIENT_ID"
    );
}

/// A workspace path with a space in it: every path render prints is one word
/// too, so `eval` does not silently truncate `$RENDERED_SCHEMA`.
#[test]
fn a_path_with_a_space_survives_the_eval() {
    let root = tempfile::tempdir().unwrap();
    let out = root.path().join("out dir");
    let pilot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "render",
            pilot.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(o.stdout).unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let shell = std::process::Command::new("sh")
        .arg("-c")
        .arg("set -eu\neval \"$1\"\ntest -f \"$RENDERED_SCHEMA\"\ntest -f \"$COMPOSE_CONFIG\"\nprintf 'OK:%s\\n' \"$RENDERED_SCHEMA\"\n")
        .arg("sh")
        .arg(&stdout)
        .output()
        .unwrap();
    assert_eq!(
        shell.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&shell.stdout),
        String::from_utf8_lossy(&shell.stderr)
    );
    assert!(
        String::from_utf8_lossy(&shell.stdout).contains("out dir/"),
        "{}",
        String::from_utf8_lossy(&shell.stdout)
    );
}

/// A minimal on-disk workspace for exercising `cmd::render`'s composition-
/// pin check in isolation, distinct from any committed pilot.
fn minimal_workspace(dir: &std::path::Path, workspace_pin: &str, supergraph_pin: &str) {
    std::fs::create_dir_all(dir.join(".factory")).unwrap();
    std::fs::write(
        dir.join(".factory/workspace.yaml"),
        format!(
            "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"{workspace_pin}\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("widget-co.graphql"),
        "extend schema\n  @link(url: \"https://specs.apollo.dev/federation/v2.12\", import: [\"@key\"])\n\ntype Query {\n  widget_co_ping: String\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("supergraph.yaml"),
        format!(
            "federation_version: ={supergraph_pin}\nsubgraphs:\n  widget_co:\n    routing_url: http://localhost\n    schema:\n      file: widget-co.graphql\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_composition_pin_that_drifts_from_workspace_yaml_fails() {
    // Genuine composition-pin drift: supergraph.yaml pins a different
    // plugin version than workspace.yaml's own federation_version records.
    // render.rs's own compose-config check must still catch this — it is
    // unrelated to, and unaffected by, the federation-spec-version lint fix.
    let dir = tempfile::tempdir().unwrap();
    minimal_workspace(dir.path(), "2.15.1", "2.14.0");
    let out = tempfile::tempdir().unwrap();
    let code = graphos_factory_core::cmd::render::main(&[
        dir.path().to_str().unwrap().to_string(),
        "--out".to_string(),
        out.path().to_str().unwrap().to_string(),
    ]);
    assert_eq!(code, 1);
}

#[test]
fn a_composition_pin_that_matches_workspace_yaml_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    minimal_workspace(dir.path(), "2.15.1", "2.15.1");
    let out = tempfile::tempdir().unwrap();
    let code = graphos_factory_core::cmd::render::main(&[
        dir.path().to_str().unwrap().to_string(),
        "--out".to_string(),
        out.path().to_str().unwrap().to_string(),
    ]);
    assert_eq!(code, 0);
}

/// Unit never touches the network, so its render keeps the real host
/// (template.yaml's BASE_URL test_default) even when `<SERVICE>_BASE_URL`
/// points e2e and live at a test host; the plain render still honours the
/// override, and so does a unit render with no test_default to fall back to
/// (ADR 0055).
#[test]
fn the_unit_render_ignores_the_base_url_override_when_a_test_default_exists() {
    let pilot = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    let bin = env!("CARGO_BIN_EXE_graphos-factory-bare");
    let rendered = |ws: &std::path::Path, unit: bool| -> String {
        let out = tempfile::tempdir().unwrap();
        let mut cmd = std::process::Command::new(bin);
        cmd.arg("render")
            .arg(ws.to_str().unwrap())
            .arg("--out")
            .arg(out.path().to_str().unwrap())
            .env("GITEA_BASE_URL", "https://test-host.example/api/v1");
        if unit {
            cmd.arg("--unit");
        }
        let o = cmd.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let stdout = String::from_utf8_lossy(&o.stdout).to_string();
        let line = stdout
            .lines()
            .find(|l| l.starts_with("RENDERED_SCHEMA="))
            .expect("RENDERED_SCHEMA line");
        std::fs::read_to_string(unquote(line.trim_start_matches("RENDERED_SCHEMA="))).unwrap()
    };
    // Assert on the rendered baseURL argument: gitea.graphql also names the
    // real host in a header comment, which a render keeps.
    let unit = rendered(&pilot, true);
    assert!(
        unit.contains("baseURL: \"http://127.0.0.1:3000/api/v1\""),
        "unit keeps the real host"
    );
    assert!(
        !unit.contains("test-host.example"),
        "unit ignores the override"
    );
    let plain = rendered(&pilot, false);
    assert!(
        plain.contains("baseURL: \"https://test-host.example/api/v1\""),
        "compose/e2e/live honour it"
    );

    // No test_default: the override is the only host, so unit keeps it.
    let ws = tempfile::tempdir().unwrap();
    for rel in [
        ".factory/workspace.yaml",
        "gitea.graphql",
        "supergraph.yaml",
    ] {
        let to = ws.path().join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(pilot.join(rel), to).unwrap();
    }
    std::fs::write(
        ws.path().join("template.yaml"),
        "variables:\n  - name: BASE_URL\n  - name: AUTH_EXPR\n    test_default: \"{$env.GITEA_TOKEN}\"\n",
    )
    .unwrap();
    assert!(rendered(ws.path(), true).contains("baseURL: \"https://test-host.example/api/v1\""));

    // An empty test_default is no test_default (render_schema and lint agree),
    // so unit keeps the override rather than failing as if it were unset.
    std::fs::write(
        ws.path().join("template.yaml"),
        "variables:\n  - name: BASE_URL\n    test_default: \"\"\n  - name: AUTH_EXPR\n    test_default: \"{$env.GITEA_TOKEN}\"\n",
    )
    .unwrap();
    assert!(rendered(ws.path(), true).contains("baseURL: \"https://test-host.example/api/v1\""));
}

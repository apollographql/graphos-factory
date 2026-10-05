//! render — placeholder-free temporary copies of the schema for rover.
//!
//!   render [workspace-dir] --out DIR [--unit]
//!          [--wiremock-port N] [--router-port N] [--health-port N]
//!
//! Prints `KEY=value` lines for the calling shell to eval, every value one
//! shell-quoted word: SERVICE, DIRECTORY, CONNECT_SPEC, FEDERATION_VERSION,
//! RENDERED_SCHEMA, COMPOSE_CONFIG, ROUTER_CONFIG (only when a port flag is
//! given: a copy of tests/router.yaml with every local override_url on the
//! WireMock port and the router's listen addresses set, written as
//! router.run.yaml; a port not given as a flag comes from $WIREMOCK_PORT /
//! $ROUTER_PORT / $ROUTER_HEALTH_PORT, else 8080 / 4000 / 8088) and, with
//! --unit, CONFIG_VARS. With --unit, `<SERVICE>_BASE_URL` is ignored when
//! template.yaml gives BASE_URL a test_default: unit renders the real host.

use crate::args::{Args, Flags};
use crate::json::{get_arr, get_str};
use crate::render::{render_schema, router_config, sh_quote, to_config_expressions};
use std::collections::HashMap;
use std::path::Path;

/// The usage text: `render --help` prints it on stdout (ADR 0086).
pub const USAGE: &str = "usage: render [workspace-dir] --out DIR [--unit] [--wiremock-port N] [--router-port N] [--health-port N]";

fn run(args: &Args) -> Result<(), String> {
    let dir = Path::new(&args.dir()).to_path_buf();
    let out = Path::new(args.get("out").ok_or(USAGE)?).to_path_buf();
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;

    let workspace = crate::yaml::parse(&crate::factory_io::read_to_string(
        &dir,
        ".factory/workspace.yaml",
    )?)?;
    let directory = get_str(&workspace, "directory")
        .ok_or("workspace.yaml has no directory")?
        .to_string();
    let service = get_str(&workspace, "service")
        .ok_or("workspace.yaml has no service")?
        .to_string();
    let template_file = dir.join("template.yaml");
    let variables: Vec<serde_json::Value> = if template_file.exists() {
        get_arr(&crate::yaml::parse_file(&template_file)?, "variables")
            .cloned()
            .unwrap_or_default()
    } else {
        vec![]
    };
    let mut env: HashMap<String, String> = std::env::vars().collect();
    // Unit never touches the network: its suite asserts requests against the
    // real host, template.yaml's BASE_URL test_default. The <SERVICE>_BASE_URL
    // override points e2e and live at a test host and must not move the unit
    // render with it, or a service whose test host differs from its real
    // host cannot pass unit and live in one run (ADR 0055). Without a
    // test_default the override is the only host there is, so it stays; an
    // empty test_default counts as none, as in render_schema and lint.
    if args.has("unit")
        && variables.iter().any(|v| {
            get_str(v, "name") == Some("BASE_URL")
                && crate::json::get(v, "test_default").is_some_and(|d| d.as_str() != Some(""))
        })
    {
        env.remove(&crate::render::env_override_name(&service, "BASE_URL"));
    }
    // Validates `directory` against workspace.schema.json's pattern before it
    // reaches either a filesystem read (here) or, below, the rendered
    // schema's own output filename (ADR 0075/0078 B1): a workspace-supplied
    // `directory` that is absolute or carries a path separator must never
    // become part of a path this command builds, on either side.
    let (_, sdl) = crate::reconcile::read_schema_file(&dir, &workspace)?;
    let mut rendered = render_schema(&sdl, &variables, &service, &env)?.sdl;

    let mut config_vars: Vec<String> = Vec::new();
    if args.has("unit") {
        let (converted, names) = to_config_expressions(&rendered);
        rendered = converted;
        config_vars = names;
    }

    let rendered_schema = out.join(format!("{}.rendered.graphql", directory));
    std::fs::write(&rendered_schema, &rendered).map_err(|e| e.to_string())?;
    let rendered_abs = std::fs::canonicalize(&rendered_schema).unwrap_or(rendered_schema.clone());

    let supergraph_file = dir.join("supergraph.yaml");
    let compose_config = out.join("supergraph.compose.yaml");
    let supergraph = std::fs::read_to_string(&supergraph_file)
        .map_err(|e| format!("{}: {}", supergraph_file.display(), e))?
        .replace(
            &format!("file: {}.graphql", directory),
            &format!("file: {}", rendered_abs.display()),
        );
    std::fs::write(&compose_config, &supergraph).map_err(|e| e.to_string())?;
    let compose_abs = std::fs::canonicalize(&compose_config).unwrap_or(compose_config);

    let pinned = crate::yaml::parse(&supergraph)?;
    let pin = pinned
        .get("federation_version")
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => crate::json::compact(other),
        })
        .unwrap_or_default();
    let pin = pin.strip_prefix('=').unwrap_or(&pin).to_string();
    let workspace_pin = get_str(&workspace, "federation_version")
        .unwrap_or("")
        .to_string();
    if !pin.is_empty() && pin != workspace_pin {
        return Err(format!(
            "supergraph.yaml pins federation {} but .factory/workspace.yaml pins {}",
            pin, workspace_pin
        ));
    }

    // The router config is the e2e layer's: it is rendered only when a run
    // asks for it with a port flag, so compose, unit and live — which also
    // call render — never read tests/router.yaml at all.
    let router_file = dir.join("tests").join("router.yaml");
    let wants_router = ["wiremock-port", "router-port", "health-port"]
        .iter()
        .any(|f| args.has(f));
    let mut router_out: Option<std::path::PathBuf> = None;
    if wants_router {
        if !router_file.exists() {
            return Err(format!("{} is missing", router_file.display()));
        }
        let port = |flag: &str, var: &str, default: u16| -> Result<u16, String> {
            let raw = match args.get(flag) {
                Some(v) => v.to_string(),
                None => match std::env::var(var) {
                    Ok(v) if !v.trim().is_empty() => v,
                    _ => return Ok(default),
                },
            };
            raw.trim()
                .parse::<u16>()
                .map_err(|_| format!("--{} / ${}: {} is not a port", flag, var, raw))
        };
        let text = std::fs::read_to_string(&router_file)
            .map_err(|e| format!("{}: {}", router_file.display(), e))?;
        let rendered = router_config(
            &text,
            port("wiremock-port", "WIREMOCK_PORT", 8080)?,
            port("router-port", "ROUTER_PORT", 4000)?,
            port("health-port", "ROUTER_HEALTH_PORT", 8088)?,
        )
        .map_err(|e| format!("{}: {}", router_file.display(), e))?;
        let target = out.join("router.run.yaml");
        std::fs::write(&target, rendered).map_err(|e| e.to_string())?;
        router_out = Some(std::fs::canonicalize(&target).unwrap_or(target));
    }

    // Every value is one shell-quoted word: the scripts `eval` these lines,
    // and a value with whitespace in it — CONFIG_VARS with two credentials, a
    // path with a space — otherwise becomes a second word the shell runs as a
    // command.
    println!("SERVICE={}", sh_quote(&service));
    println!("DIRECTORY={}", sh_quote(&directory));
    println!(
        "CONNECT_SPEC={}",
        sh_quote(get_str(&workspace, "connect_spec").unwrap_or(""))
    );
    println!("FEDERATION_VERSION={}", sh_quote(&workspace_pin));
    println!(
        "RENDERED_SCHEMA={}",
        sh_quote(&rendered_abs.display().to_string())
    );
    println!(
        "COMPOSE_CONFIG={}",
        sh_quote(&compose_abs.display().to_string())
    );
    if let Some(r) = &router_out {
        println!("ROUTER_CONFIG={}", sh_quote(&r.display().to_string()));
    }
    if args.has("unit") {
        println!("CONFIG_VARS={}", sh_quote(&config_vars.join(" ")));
    }
    Ok(())
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["unit"],
    valued: &["out", "wiremock-port", "router-port", "health-port"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    match run(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("render: {}", e);
            1
        }
    }
}

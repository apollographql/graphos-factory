//! Small subcommands the shell wrappers use so they never parse YAML.

use crate::json::{get_arr, get_str};
use std::path::Path;

/// auth-env [workspace]: the environment variable named by AUTH_EXPR's
/// test_default (`{$env.NAME}`), or nothing when the template has none.
pub fn auth_env(argv: &[String]) -> i32 {
    let dir = argv
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| ".".to_string());
    let file = Path::new(&dir).join("template.yaml");
    if !file.exists() {
        return 0;
    }
    let template = match crate::yaml::parse_file(&file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("auth-env: {}", e);
            return 1;
        }
    };
    let auth = get_arr(&template, "variables")
        .into_iter()
        .flatten()
        .find(|v| get_str(v, "name") == Some("AUTH_EXPR"));
    if let Some(name) =
        auth.and_then(|v| crate::render::env_var_from_auth_expr(get_str(v, "test_default")))
    {
        print!("{}", name);
    }
    0
}

/// yaml2json <file>: one YAML document as compact JSON.
pub fn yaml2json(argv: &[String]) -> i32 {
    let file = match argv.first() {
        Some(f) => f,
        None => {
            eprintln!("usage: yaml2json <file>");
            return 1;
        }
    };
    match crate::yaml::parse_file(Path::new(file)) {
        Ok(v) => {
            println!("{}", crate::json::compact(&v));
            0
        }
        Err(e) => {
            eprintln!("yaml2json: {}", e);
            1
        }
    }
}

/// error-statuses [workspace]: one line per included operation,
/// `<root field>\t<documented>\t<executed>\t<operation key>`, each status
/// column a comma list or `-`. `documented` is the non-2xx statuses the
/// inventory documents; `executed` is the subset the last `evidence` run saw
/// answer the operation's own request in a passing e2e case
/// (`layers.wiremock_e2e.error_coverage`), `-` when no run is recorded. e2e.sh
/// reads the field, the documented statuses and the key for its
/// upstream-status check (ADR 0077).
pub fn error_statuses(argv: &[String]) -> i32 {
    let dir = argv
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| ".".to_string());
    let dir = Path::new(&dir);
    let read = |rel: &str, yaml: bool| -> Result<serde_json::Value, String> {
        let text = crate::factory_io::read_to_string(dir, rel).map_err(|e| e.to_string())?;
        if yaml {
            crate::yaml::parse(&text)
        } else {
            crate::json::parse(&text)
        }
        .map_err(|e| format!("{}: {}", rel, e))
    };
    let (workspace, selection, inventory) = match (
        read(".factory/workspace.yaml", true),
        read(".factory/selection.yaml", true),
        read(".factory/inventory.json", false),
    ) {
        (Ok(w), Ok(s), Ok(i)) => (w, s, i),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            eprintln!("error-statuses: {}", e);
            return 1;
        }
    };
    let prefix = get_str(&workspace, "field_prefix").unwrap_or("");
    let ops = get_arr(&inventory, "operations")
        .unwrap_or(&Vec::new())
        .clone();
    // The statuses a recorded run executed, per operation; absent file, or a
    // run that never reached the e2e layer, means none recorded.
    let recorded: Option<serde_json::Value> =
        crate::factory_io::read_to_string_optional(dir, ".factory/evidence/latest.json")
            .ok()
            .flatten()
            .and_then(|t| crate::json::parse(&t).ok());
    let list = |v: Vec<String>| {
        if v.is_empty() {
            "-".to_string()
        } else {
            v.join(",")
        }
    };
    for (key, entry) in crate::json::get_obj(&selection, "operations")
        .into_iter()
        .flatten()
    {
        if !crate::json::truthy(crate::json::get(entry, "include")) {
            continue;
        }
        let name = match crate::json::get(entry, "graphql").and_then(|g| get_str(g, "name")) {
            Some(n) => n,
            None => continue,
        };
        let documented: Vec<String> = ops
            .iter()
            .find(|o| get_str(o, "key") == Some(key.as_str()))
            .map(crate::cmd::scaffold::documented_error_statuses)
            .unwrap_or_default()
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        let executed: Vec<String> = recorded
            .as_ref()
            .and_then(|r| crate::json::get(r, "layers"))
            .and_then(|l| crate::json::get(l, "wiremock_e2e"))
            .and_then(|l| crate::json::get(l, "error_coverage"))
            .and_then(|c| crate::json::get(c, key))
            .and_then(|c| crate::json::get_arr(c, "executed"))
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        println!(
            "{}_{}\t{}\t{}\t{}",
            prefix,
            name,
            list(documented),
            list(executed),
            key
        );
    }
    0
}

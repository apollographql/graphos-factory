//! Placeholder rendering: the temporary, placeholder-free copies of the
//! schema that rover needs, without touching the committed schema.
//!
//! Two rewrites happen here and nowhere else: `{{PLACEHOLDER}}` becomes its
//! template.yaml `test_default` (or the `<SERVICE>_<NAME>` environment
//! variable when exported), and with `unit`, static `{$env.NAME}` becomes
//! `{$config.NAME}` because the Connector Testing Framework injects $config
//! but not $env.

use crate::json::get_str;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

fn auth_expr_re() -> Regex {
    Regex::new(r"^\{\s*\$env\.([A-Z][A-Z0-9_]*)\s*\}$").unwrap()
}

pub fn env_override_name(service: &str, variable: &str) -> String {
    format!("{}_{}", service.to_uppercase(), variable)
}

/// The env var a static AUTH_EXPR resolves to, or None when it is not static.
pub fn env_var_from_auth_expr(value: Option<&str>) -> Option<String> {
    auth_expr_re()
        .captures(value.unwrap_or(""))?
        .get(1)
        .map(|m| m.as_str().to_string())
}

#[derive(Debug)]
pub struct Rendered {
    pub sdl: String,
    pub used: Vec<(String, String)>,
}

pub fn render_schema(
    sdl: &str,
    variables: &[Value],
    service: &str,
    env: &HashMap<String, String>,
) -> Result<Rendered, String> {
    let mut out = sdl.to_string();
    let mut used = Vec::new();
    for variable in variables {
        let name = match get_str(variable, "name") {
            Some(n) => n,
            None => continue,
        };
        let override_name = env_override_name(service, name);
        let value: Option<String> = env.get(&override_name).cloned().or_else(|| {
            crate::json::get(variable, "test_default").map(|v| match v {
                Value::String(s) => s.clone(),
                other => crate::json::compact(other),
            })
        });
        let value = match value {
            Some(v) if !v.is_empty() => v,
            _ => {
                return Err(format!(
                    "template.yaml gives {} no test_default and {} is not set",
                    name, override_name
                ))
            }
        };
        if name == "AUTH_EXPR" && env_var_from_auth_expr(Some(&value)).is_none() {
            return Err(format!(
                "AUTH_EXPR.test_default must be a complete unquoted local expression like {{$env.EXAMPLE_TOKEN}}, got {}",
                serde_json::to_string(&value).unwrap_or_default()
            ));
        }
        out = out.replace(&format!("{{{{{}}}}}", name), &value);
        used.push((name.to_string(), value));
    }
    let leftover = Regex::new(r"\{\{\s*([A-Z0-9_]+)\s*\}\}").unwrap();
    if let Some(m) = leftover.captures(&out) {
        return Err(format!(
            "{{{{{}}}}} is used by the schema but not declared in template.yaml",
            &m[1]
        ));
    }
    Ok(Rendered { sdl: out, used })
}

/// Rewrite static {$env.NAME} to {$config.NAME} for `rover connector test`.
pub fn to_config_expressions(sdl: &str) -> (String, Vec<String>) {
    let re = Regex::new(r"\{\s*\$env\.([A-Z][A-Z0-9_]*)\s*\}").unwrap();
    let mut names: Vec<String> = Vec::new();
    let out = re
        .replace_all(sdl, |caps: &regex::Captures| {
            let name = caps[1].to_string();
            if !names.contains(&name) {
                names.push(name.clone());
            }
            format!("{{$config.{}}}", name)
        })
        .to_string();
    (out, names)
}

/// `tests/router.yaml` for one run of the e2e layer: every `override_url`
/// that points at a local WireMock takes `wiremock_port`, and the router is
/// told where to listen (`supergraph.listen`, `health_check.listen`), so two
/// suites can run side by side on different ports without editing the file
/// the workspace commits. An `override_url` that is not local, or a config
/// with none, is an error: the e2e layer exists to point every connector at
/// WireMock, and a miss is better caught here than as an unmatched upstream
/// request thirty seconds later.
pub fn router_config(
    text: &str,
    wiremock_port: u16,
    router_port: u16,
    health_port: u16,
) -> Result<String, String> {
    let mut config = crate::yaml::parse(text)?;
    let root = config
        .as_object_mut()
        .ok_or("tests/router.yaml is not a mapping")?;
    let mut seen: Vec<String> = Vec::new();
    let mut stayed: Vec<String> = Vec::new();
    if let Some(sources) = root
        .get_mut("connectors")
        .and_then(|c| c.get_mut("sources"))
        .and_then(Value::as_object_mut)
    {
        for (name, source) in sources.iter_mut() {
            match source.get_mut("override_url") {
                Some(Value::String(url)) => {
                    seen.push(url.clone());
                    match with_port(url, wiremock_port) {
                        Some(rewritten) => *url = rewritten,
                        None => stayed.push(url.clone()),
                    }
                }
                Some(other) => {
                    return Err(format!(
                        "connectors.sources.{}.override_url is {}, not a URL string",
                        name,
                        crate::json::describe(other)
                    ))
                }
                None => {}
            }
        }
    }
    if seen.is_empty() {
        return Err("tests/router.yaml declares no connectors.sources.*.override_url — the e2e layer would call the real API".to_string());
    }
    if !stayed.is_empty() {
        return Err(format!(
            "override_url {} is not a local URL the e2e layer can move onto WireMock's port (expected http://localhost:<port> or http://127.0.0.1:<port>)",
            stayed
                .iter()
                .map(|u| format!("\"{}\"", u))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let listen = |root: &mut serde_json::Map<String, Value>, section: &str, port: u16| {
        let entry = root
            .entry(section.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(serde_json::Map::new());
        }
        entry.as_object_mut().unwrap().insert(
            "listen".to_string(),
            Value::from(format!("127.0.0.1:{}", port)),
        );
    };
    listen(root, "supergraph", router_port);
    listen(root, "health_check", health_port);
    Ok(crate::yaml::stringify(&config, 0))
}

/// `http://localhost:8080/x` → `http://localhost:<port>/x`, for a URL whose
/// host is the local machine (`localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0`,
/// with or without a port or userinfo); anything else is None.
fn with_port(url: &str, port: u16) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let host = if hostport.starts_with('[') {
        match hostport.find(']') {
            Some(end) => &hostport[..=end],
            None => return None,
        }
    } else {
        hostport
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(hostport)
    };
    if !matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0") {
        return None;
    }
    Some(match userinfo {
        Some(u) => format!("{}://{}@{}:{}{}", scheme, u, host, port, path),
        None => format!("{}://{}:{}{}", scheme, host, port, path),
    })
}

/// One value as a single POSIX shell word, for the `KEY=value` lines the
/// `render` command prints and every layer script `eval`s.
///
/// Whitespace in a value is the whole point: two `{$env.NAME}` credentials
/// make `CONFIG_VARS` two words, and an unquoted second word is run as a
/// command — which `evidence` then records as a missing tool and the layer as
/// `skipped` rather than failing. Paths can carry spaces for the same reason.
/// Single-quoting is total: nothing inside `'…'` is special to the shell, and
/// an embedded quote closes, escapes and reopens (`'\''`).
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

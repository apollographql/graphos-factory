//! The four local-validation files `init` writes for this target, as
//! skeletons for one subgraph: `template.yaml` (the two placeholders and
//! their local test values), `supergraph.yaml` (rover's compose config for
//! this subgraph alone, not the user's supergraph), `tests/router.yaml`
//! (the e2e layer's router config) and `<directory>.graphql` (the schema's
//! header: the two spec links and the one source, no root field yet).
//! Their layout is the pilot's (`pilots/graphos/gitea/`), so a diff against
//! it shows values and the comments a skeleton adds, nothing else. A file that already exists is
//! left alone (the core's `init` reports it); none of them is under
//! `.factory/`, so none goes through custody.

use graphos_factory_core::json::{get, get_arr, get_str};
use graphos_factory_core::openapi::PLACEHOLDER_BASE_URL;
use graphos_factory_core::target::InitInput;
use serde_json::Value;
use std::path::PathBuf;

/// `BASE_URL`'s test value when the document names no absolute server: a
/// local stand-in, with a comment saying what to fill in.
pub const LOCAL_BASE_URL: &str = "http://127.0.0.1:8080";

/// Where `tests/router.yaml` points the source; `render` moves the port onto
/// WireMock's for each e2e run.
pub const OVERRIDE_URL: &str = "http://localhost:8080";

/// The four files, workspace-relative, with their contents.
pub fn files(input: &InitInput) -> Vec<(PathBuf, String)> {
    let workspace = input.workspace;
    let service = get_str(workspace, "service").unwrap_or_default();
    let directory = get_str(workspace, "directory").unwrap_or(service);
    let pin = get_str(workspace, "federation_version").unwrap_or_default();
    // The schema links the spec version the workspace pins: its own
    // `federation_spec_version` when it records one, else the plugin's.
    let spec = get_str(workspace, "federation_spec_version").unwrap_or(pin);
    let connect = get_str(workspace, "connect_spec").unwrap_or_default();
    let api = get(input.inventory, "api");
    vec![
        (
            PathBuf::from(format!("{}.graphql", directory)),
            schema(service, &major_minor(spec), connect, api),
        ),
        (PathBuf::from("template.yaml"), template(service, api)),
        (PathBuf::from("supergraph.yaml"), supergraph(directory, pin)),
        (
            PathBuf::from("tests/router.yaml"),
            router(directory, service),
        ),
    ]
}

/// A YAML double-quoted scalar: JSON's string escapes are a subset of
/// YAML's, so a title with a quote or a colon in it stays one value. JSON
/// leaves DEL, the C1 controls and the Unicode line separators raw, and a
/// YAML reader either refuses them or reads them as a line break, so they
/// are written as `\uXXXX` escapes too.
fn quoted(s: &str) -> String {
    let json = serde_json::to_string(s).unwrap_or_default();
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if breaks_text(c) {
            out.push_str(&format!("\\u{:04X}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// A character that must not reach a YAML file as itself: a control
/// (newline, carriage return, tab, NEL, DEL, ...), the line and paragraph
/// separators, and the byte-order and non-character code points.
fn breaks_text(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{FFFE}' | '\u{FFFF}'
        )
}

/// A spec-derived string made safe for a `#` comment line: every character
/// that could end the line (or that YAML refuses) becomes a space, so the
/// text can never start a key of its own.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if breaks_text(c) { ' ' } else { c })
        .collect()
}

/// Spec text made inert in the schema file: one line, no `{{`/`}}` that
/// lint would read as a placeholder, and no `@source(` that a raw count
/// would take for a second source.
fn inert(s: &str) -> String {
    one_line(s)
        .replace("{{", "{ {")
        .replace("}}", "} }")
        .replace("@source(", "@source (")
}

/// Comment lines at the variable list's indent. Every line is made
/// single-line here, the one place spec text enters a comment.
fn comment(out: &mut String, lines: &[String]) {
    for line in lines {
        out.push_str("  # ");
        out.push_str(&one_line(line));
        out.push('\n');
    }
}

/// `template.yaml`: `BASE_URL` and `AUTH_EXPR`, each with a description and
/// a `test_default` that lint and `render` accept as written.
pub fn template(service: &str, api: Option<&Value>) -> String {
    let title = api
        .and_then(|a| get_str(a, "title"))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(service);
    let mut out = String::from("variables:\n");

    let base = base_url(api);
    comment(&mut out, &base.note);
    out.push_str("  - name: BASE_URL\n");
    let description = match url_path(&base.url) {
        Some(path) => format!(
            "Base URL of the {} REST API, including the {} path",
            title, path
        ),
        None => format!("Base URL of the {} REST API", title),
    };
    out.push_str(&format!("    description: {}\n", quoted(&description)));
    out.push_str(&format!("    test_default: {}\n", quoted(&base.url)));

    let auth = auth(api);
    comment(&mut out, &auth.note);
    out.push_str("  - name: AUTH_EXPR\n");
    out.push_str(&format!(
        "    description: {}\n",
        quoted(&format!(
            "Complete Connectors authentication expression for {}",
            auth.what.replace("{title}", title)
        ))
    ));
    out.push_str(&format!(
        "    test_default: {}\n",
        quoted(&format!("{{$env.{}_TOKEN}}", service.to_uppercase()))
    ));
    out
}

struct Chosen {
    url: String,
    note: Vec<String>,
}

/// The first absolute http(s) server URL the inventory records (not the
/// placeholder it records for a document with none), trailing `/` dropped;
/// else a relative one on the local stand-in; else the stand-in alone.
fn base_url(api: Option<&Value>) -> Chosen {
    let urls: Vec<&str> = api
        .and_then(|a| get_arr(a, "base_urls"))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let absolute = urls.iter().find(|u| {
        **u != PLACEHOLDER_BASE_URL
            && url::Url::parse(u).is_ok_and(|p| {
                matches!(p.scheme(), "http" | "https")
                    && p.host_str().is_some_and(|h| !h.ends_with(".invalid"))
            })
    });
    if let Some(u) = absolute {
        return Chosen {
            url: u.trim_end_matches('/').to_string(),
            note: Vec::new(),
        };
    }
    if let Some(path) = urls.iter().find(|u| u.starts_with('/')) {
        let path = path.trim_end_matches('/');
        return Chosen {
            url: format!("{}{}", LOCAL_BASE_URL, path),
            note: vec![
                format!(
                    "The document's server URL is relative ({}): {} is a",
                    if path.is_empty() { "/" } else { path },
                    LOCAL_BASE_URL
                ),
                "local stand-in. Put the host the local and live layers call before the path."
                    .into(),
            ],
        };
    }
    Chosen {
        url: LOCAL_BASE_URL.to_string(),
        note: vec![
            format!(
                "The document declares no server URL: {} is a local stand-in.",
                LOCAL_BASE_URL
            ),
            "Set the URL (host and path prefix) the local and live layers call.".into(),
        ],
    }
}

/// The URL's path when it has one beyond `/`.
fn url_path(u: &str) -> Option<String> {
    let parsed = url::Url::parse(u).ok()?;
    let path = parsed.path().trim_end_matches('/');
    (!path.is_empty()).then(|| path.to_string())
}

struct Auth {
    /// What the credential is, with `{title}` for the API's title.
    what: String,
    note: Vec<String>,
    /// Where the credential goes.
    place: Place,
}

/// A header and the prefix before the credential, or the query parameter
/// that carries it.
enum Place {
    Header { name: String, prefix: String },
    Query(String),
}

/// The scheme `AUTH_EXPR` describes: the first one the document's default
/// security names, else the first it declares, of the kinds a header or
/// query credential carries.
fn auth(api: Option<&Value>) -> Auth {
    let schemes: Vec<&Value> = api
        .and_then(|a| get_arr(a, "auth"))
        .into_iter()
        .flatten()
        .filter(|s| {
            matches!(
                get_str(s, "kind"),
                Some("bearer" | "basic" | "api_key" | "oauth2")
            )
        })
        .collect();
    let preferred = api
        .and_then(|a| get_arr(a, "security"))
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|alternative| alternative.keys())
        .find_map(|name| {
            schemes
                .iter()
                .find(|s| get_str(s, "scheme_name") == Some(name.as_str()))
        });
    let chosen = match preferred.or(schemes.first()) {
        Some(s) => *s,
        None => {
            return Auth {
                place: Place::Header {
                    name: "Authorization".into(),
                    prefix: String::new(),
                },
                what: "the {title} credential".into(),
                note: vec![
                "The document declares no security scheme. If the API takes no credential,".into(),
                "remove the Authorization header from @source and delete this entry (lint reports"
                    .into(),
                "a declared variable the schema does not use).".into(),
            ],
            }
        }
    };
    let kind = get_str(chosen, "kind").unwrap_or_default();
    let (noun, token) = match kind {
        "bearer" => ("bearer token", "<token>"),
        "basic" => ("Basic credentials", "<credentials>"),
        "oauth2" => ("OAuth 2 access token", "<token>"),
        _ => ("API key", "<key>"),
    };
    let name = get_str(chosen, "scheme_name").unwrap_or_default();
    let mut detail: Vec<String> = Vec::new();
    if !name.is_empty() {
        detail.push(format!("scheme {}", name));
    }
    match (get_str(chosen, "header"), get_str(chosen, "query_param")) {
        (Some(header), _) => detail.push(format!(
            "sent as `{}: {}{}`",
            header,
            get_str(chosen, "prefix").unwrap_or_default(),
            token
        )),
        (None, Some(param)) => detail.push(format!("sent as the `{}` query parameter", param)),
        (None, None) => {}
    }
    let what = if detail.is_empty() {
        format!("the {{title}} {}", noun)
    } else {
        format!("the {{title}} {} ({})", noun, detail.join(", "))
    };
    let note = if schemes.len() > 1 {
        let names: Vec<&str> = schemes
            .iter()
            .filter_map(|s| get_str(s, "scheme_name"))
            .collect();
        vec![
            format!(
                "The document declares {} security schemes ({}).",
                schemes.len(),
                names.join(", ")
            ),
            format!(
                "This describes {}; describe the one the subgraph sends.",
                if name.is_empty() { "the first" } else { name }
            ),
        ]
    } else {
        Vec::new()
    };
    let place = match (get_str(chosen, "header"), get_str(chosen, "query_param")) {
        (None, Some(param)) => Place::Query(param.to_string()),
        (header, _) => Place::Header {
            name: header.unwrap_or("Authorization").to_string(),
            prefix: get_str(chosen, "prefix").unwrap_or_default().to_string(),
        },
    };
    Auth { what, note, place }
}

/// `2.15` from `2.15.2` (and from `2.15`): the version a `federation/v…`
/// link names.
fn major_minor(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// A GraphQL string literal's body: JSON's string escapes are GraphQL's,
/// and a line separator JSON leaves raw is escaped too.
fn graphql_string(s: &str) -> String {
    let quoted = quoted(s);
    quoted[1..quoted.len() - 1].to_string()
}

/// The headers the pilot's source sends besides the credential.
const FIXED_HEADERS: [(&str, &str); 3] = [
    ("Accept", "application/json"),
    ("Content-Type", "application/json"),
    ("User-Agent", "graphos-factory"),
];

/// `<directory>.graphql`: a header comment, the two spec links and the one
/// source with the pilot's headers, the credential's prefix outside
/// `{{AUTH_EXPR}}` as `template.yaml` describes it. No root field: a
/// subgraph with none does not compose, so compose fails until the first
/// apply adds one, and the comment says so. The comment never spells the
/// source directive with its parenthesis, which a reader counting raw
/// `@source(` text would take for a second source.
pub fn schema(service: &str, federation: &str, connect: &str, api: Option<&Value>) -> String {
    let title = api
        .and_then(|a| get_str(a, "title"))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(service);
    let auth = auth(api);
    let mut out = format!(
        "# {} — Apollo Connectors subgraph.\n\
         #\n\
         # The schema header init wrote: the spec links and the one source. Each\n\
         # root field (type Query, type Mutation) and its @connect is added at\n\
         # apply; until the first apply adds one, compose fails on this file.\n\
         # This file is the artifact: edit it directly. Selection lives in\n\
         # .factory/selection.yaml, the reasoning in .factory/decisions.json.\n",
        inert(title)
    );
    let mut headers: Vec<(String, String)> = Vec::new();
    match &auth.place {
        Place::Header { name, prefix } => {
            let (name, prefix) = (inert(name), inert(prefix));
            out.push_str(&format!(
                "#\n# Auth: {}: {}<credential>{}\n",
                name,
                prefix,
                if prefix.is_empty() {
                    ", AUTH_EXPR with no scheme prefix."
                } else {
                    "; the scheme prefix stays outside AUTH_EXPR."
                }
            ));
            headers.push((name.clone(), format!("{}{{{{AUTH_EXPR}}}}", prefix)));
        }
        Place::Query(param) => out.push_str(&format!(
            "#\n# Auth: the `{}` query parameter carries the credential, so no header\n\
             # sends AUTH_EXPR: it goes on each connector's request, and lint's\n\
             # unused-template-variable for it clears at the first apply.\n",
            inert(param)
        )),
    }
    if !auth.note.is_empty() {
        out.push_str("# Read template.yaml's comment on AUTH_EXPR before the first apply.\n");
    }
    headers.extend(
        FIXED_HEADERS
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string())),
    );
    out.push_str(&format!(
        "\nextend schema\n  \
         @link(url: \"https://specs.apollo.dev/federation/v{federation}\", import: [\"@key\"])\n  \
         @link(url: \"https://specs.apollo.dev/connect/{connect}\", import: [\"@source\", \"@connect\"])\n\
         \n\
         @source(\n  \
         name: \"{service}\"\n  \
         http: {{\n    \
         baseURL: \"{{{{BASE_URL}}}}\"\n    \
         headers: [\n"
    ));
    for (name, value) in headers {
        out.push_str(&format!(
            "      {{ name: \"{}\", value: \"{}\" }}\n",
            graphql_string(&name),
            graphql_string(&value)
        ));
    }
    out.push_str("    ]\n  }\n)\n");
    out
}

/// `supergraph.yaml`: the subgraph alone, under its directory name, at the
/// workspace's Federation pin.
pub fn supergraph(directory: &str, pin: &str) -> String {
    format!(
        "# rover's compose config for this one subgraph, for the local layers: not the\n\
         # user's supergraph. federation_version must equal .factory/workspace.yaml's.\n\
         federation_version: ={pin}\n\
         subgraphs:\n  \
         {directory}:\n    \
         routing_url: http://localhost\n    \
         schema:\n      \
         file: {directory}.graphql\n"
    )
}

/// `tests/router.yaml`: the one source, keyed `<subgraph>.<@source name>`
/// (the directory, and `workspace.service`, which `source-name` holds the
/// `@source` name to), pointed at the local WireMock.
pub fn router(directory: &str, service: &str) -> String {
    format!(
        "# The e2e layer's router config. A source's key is <subgraph>.<@source name>;\n\
         # render moves override_url onto WireMock's port for each run.\n\
         connectors:\n  \
         sources:\n    \
         {directory}.{service}:\n      \
         override_url: \"{OVERRIDE_URL}\"\n\
         include_subgraph_errors:\n  \
         all: true\n"
    )
}

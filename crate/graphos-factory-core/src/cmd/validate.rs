//! validate — fixture and unit-suite bodies against the oracle (layer 4).
//!
//!   validate [workspace-dir] [--json]
//!
//! Every WireMock response body (and every `equalToJson` request body) in
//! tests/fixtures/mappings, and every `apiResponseBody` / expected request
//! body in tests/*.connector.yaml, is checked against the shape the
//! inventory documents for its operation and status. The oracle is the spec
//! (inventory.json) or, for a no-spec workspace, the schema inferred from
//! recorded samples.
//!
//! A body the oracle did not compare is never a pass. Its status says why:
//!   pass / fail   the oracle compared the body
//!   unchecked     nothing to compare against: the spec documents no shape
//!                 for that operation and status, or the stub has no JSON
//!                 body (204, redirect)
//!   unmatched     the oracle could not even find the operation or read the
//!                 body: a fixture or unit entry names a method + path no
//!                 inventory operation has, the suite is not readable YAML,
//!                 a body is not JSON — a test asserting a request the spec
//!                 does not describe
//!   waived        an unchecked or unmatched body the engineer has accepted
//!                 in selection.yaml `waivers[]` (crate::waivers)
//!
//! Exit codes: 0 no fail and no unmatched · 1 otherwise (or the workspace
//! cannot be read). `unchecked` alone never fails; lint's evidence rules
//! and the per-operation evidence keep it visible.

use crate::args::{Args, Flags};
use crate::conformance::{conform, find_operation_for_request, response_shape_for};
use crate::json::{get, get_arr, get_str, obj, pretty};
use crate::op_match::OpHints;
use crate::waivers::Waiver;
use serde_json::Value;
use std::path::Path;

/// Name the oracle the inventory's shapes came from: the pinned description
/// document (whatever its format) when the workspace has one, else the
/// inferred schema, else the inventory itself.
fn oracle_label(dir: &Path) -> String {
    let entries = crate::sources::read_sources_lock(dir)
        .ok()
        .flatten()
        .map(|lock| crate::sources::document_entries(&lock))
        .unwrap_or_default();
    let pinned = entries
        .iter()
        .filter(|e| crate::sources::not_followed(e, &entries).is_none())
        .map(|e| e.path.clone())
        .find(|p| dir.join(p).exists());
    if let Some(p) = pinned {
        return p;
    }
    for candidate in [
        "openapi.json",
        "openapi.yaml",
        "swagger.json",
        "swagger.yaml",
    ] {
        if dir.join(candidate).exists() {
            return candidate.to_string();
        }
    }
    if crate::factory_io::is_file(dir, ".factory/inferred-schema.json") {
        return "inferred-schema.json (recorded samples)".to_string();
    }
    "inventory.json".to_string()
}

/// The path prefixes an absolute request URL in a unit suite may carry
/// before the spec's own path: the spec's server URLs, and the workspace's
/// rendered `BASE_URL` (template.yaml's `test_default`), which is the URL
/// `rover connector test` actually builds. The second matters when the spec
/// declares no usable server — a Swagger 2.0 document whose `basePath` is a
/// template, for instance — and the inventory's `base_urls` is empty.
pub fn base_path_prefixes(inventory: &Value, dir: &Path) -> Vec<String> {
    let mut urls: Vec<String> = get(inventory, "api")
        .and_then(|a| get_arr(a, "base_urls"))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if let Ok(template) = crate::yaml::parse_file(&dir.join("template.yaml")) {
        if let Some(v) = get_arr(&template, "variables")
            .into_iter()
            .flatten()
            .find(|v| get_str(v, "name") == Some("BASE_URL"))
            .and_then(|v| get_str(v, "test_default"))
        {
            urls.push(v.to_string());
        }
    }
    let mut prefixes: Vec<String> = urls
        .iter()
        .filter_map(|u| url::Url::parse(u).ok())
        .map(|u| u.path().trim_end_matches('/').to_string())
        .filter(|p| !p.is_empty() && p != "/")
        .collect();
    prefixes.sort_by_key(|p| std::cmp::Reverse(p.len()));
    prefixes.dedup();
    prefixes
}

/// Drop the first matching prefix (`/api/v1`) from an absolute request path.
pub fn strip_prefixes(pathname: &str, prefixes: &[String]) -> String {
    for prefix in prefixes {
        if let Some(rest) = pathname.strip_prefix(prefix.as_str()) {
            if rest.is_empty() {
                return "/".to_string();
            }
            if rest.starts_with('/') {
                return rest.to_string();
            }
        }
    }
    pathname.to_string()
}

/// Drop the spec's base URL path prefix (`/api`) from an absolute request path.
pub fn strip_base_path(pathname: &str, inventory: &Value) -> String {
    let prefixes: Vec<String> = get(inventory, "api")
        .and_then(|a| get_arr(a, "base_urls"))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|u| url::Url::parse(u).ok())
        .map(|u| u.path().trim_end_matches('/').to_string())
        .filter(|p| !p.is_empty() && p != "/")
        .collect();
    strip_prefixes(pathname, &prefixes)
}

struct Result_ {
    where_: String,
    operation: Option<String>,
    kind: Option<&'static str>,
    http_status: Option<Value>,
    status: &'static str,
    reason: Option<String>,
    problems: Vec<String>,
    note: Option<String>,
    /// Index into the workspace's waivers when the result was waived.
    waiver: Option<usize>,
}

fn result_json(r: &Result_, waivers: &[Waiver]) -> Value {
    let mut o = obj();
    o.insert("where".into(), Value::from(r.where_.as_str()));
    o.insert(
        "operation".into(),
        r.operation.clone().map(Value::from).unwrap_or(Value::Null),
    );
    if let Some(k) = r.kind {
        o.insert("kind".into(), Value::from(k));
    }
    if let Some(h) = &r.http_status {
        o.insert("httpStatus".into(), h.clone());
    }
    o.insert("status".into(), Value::from(r.status));
    if let Some(reason) = &r.reason {
        o.insert("reason".into(), Value::from(reason.as_str()));
    }
    if r.status == "pass" || r.status == "fail" {
        o.insert(
            "problems".into(),
            Value::Array(r.problems.iter().map(|p| Value::from(p.as_str())).collect()),
        );
    }
    if let Some(n) = &r.note {
        o.insert("note".into(), Value::from(n.as_str()));
    }
    if let Some(w) = r.waiver.and_then(|i| waivers.get(i)) {
        o.insert("waiver".into(), waiver_json(w));
    }
    Value::Object(o)
}

fn status_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => crate::json::compact(other),
    }
}

fn waiver_json(w: &Waiver) -> Value {
    crate::json::object(vec![
        ("target", Value::from(w.target())),
        ("status", Value::from(w.status.as_str())),
        (
            "reason",
            w.reason.clone().map(Value::from).unwrap_or(Value::Null),
        ),
        (
            "decision",
            w.decision.clone().map(Value::from).unwrap_or(Value::Null),
        ),
        (
            "context",
            w.context.clone().map(Value::from).unwrap_or(Value::Null),
        ),
    ])
}

/// What the layer knows after checking a workspace: the JSON summary
/// `validate --json` prints (`oracle`, `counts`, `operations`, `results`,
/// `waivers`) — also what lint and codify read.
#[derive(Debug)]
pub struct Report {
    pub json: Value,
    /// fail > 0 or unmatched > 0.
    pub failing: bool,
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &[],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let json = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    let report = match validate_workspace(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("validate: {}", e);
            return 1;
        }
    };
    if json {
        print!("{}", pretty(&report.json));
    } else {
        print_text(&report.json);
    }
    if report.failing {
        1
    } else {
        0
    }
}

/// The count of one status in a report.
fn count(json: &Value, status: &str) -> u64 {
    get(json, "counts")
        .and_then(|c| get(c, status))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// `N bodies conform, N do not, N unchecked[, N unmatched][, N waived]` —
/// the two new counts appear only when non-zero, so a workspace without them
/// reads as it always did.
pub fn summary_line(json: &Value) -> String {
    let mut s = format!(
        "{} bodies conform, {} do not, {} unchecked",
        count(json, "pass"),
        count(json, "fail"),
        count(json, "unchecked")
    );
    if count(json, "unmatched") > 0 {
        s.push_str(&format!(", {} unmatched", count(json, "unmatched")));
    }
    if count(json, "waived") > 0 {
        s.push_str(&format!(", {} waived", count(json, "waived")));
    }
    s
}

fn print_text(json: &Value) {
    for r in get_arr(json, "results").into_iter().flatten() {
        let status = get_str(r, "status").unwrap_or("");
        if status == "pass" {
            continue;
        }
        let mut line = format!(
            "{:<9} {}{}{}",
            status,
            get_str(r, "where").unwrap_or(""),
            get_str(r, "kind")
                .map(|k| format!(" [{}]", k))
                .unwrap_or_default(),
            get_str(r, "reason")
                .map(|x| format!(" — {}", x))
                .unwrap_or_default()
        );
        if let Some(w) = get(r, "waiver") {
            line.push_str(&format!(
                "; waived: {}{}",
                get_str(w, "reason").unwrap_or(""),
                get_str(w, "decision")
                    .map(|d| format!(" ({})", d))
                    .unwrap_or_default()
            ));
        }
        println!("{}", line);
        for p in get_arr(r, "problems").into_iter().flatten() {
            println!("          {}", p.as_str().unwrap_or(""));
        }
    }
    for w in get(json, "waivers")
        .and_then(|w| get_arr(w, "unused"))
        .into_iter()
        .flatten()
    {
        println!(
            "waiver    unused: {} ({}) — no body currently has that status; remove it from selection.yaml",
            get_str(w, "target").unwrap_or(""),
            get_str(w, "status").unwrap_or("")
        );
    }
    println!(
        "validate: oracle {}; {}",
        get_str(json, "oracle").unwrap_or(""),
        summary_line(json)
    );
    if count(json, "unmatched") > 0 {
        println!(
            "validate: {} unmatched — a test asserts a request the spec does not describe (or a body is unreadable); fix the matcher, the reader, or document the endpoint through a pinned-source patch; an accepted gap is waived with `graphos-factory-core codify --waive`",
            count(json, "unmatched")
        );
    }
}

/// The operations a stub declares, for a path several operations match
/// equally (ADR 0044) — only through its own file name. The case it is named
/// after declares by its name (`<case>`, `<case>_minimal`) and by the root
/// fields its document `tests/cases/<case>.graphql` selects, which is what
/// settles a variant named for its own case (`campaign_not_found`).
///
/// A stub whose `metadata."x-cases"` does not list its own name serves some
/// other request of those cases — a nested one (`ad_campaign.json`, tagged
/// `ad`, answering the Campaign fetch under `meta_ad`), or none at all (`[]`,
/// an unreachable recording). What the case selects at the root is not the
/// operation such a stub answers, so it declares nothing. `fixtures` names a
/// stub after its first case and lists it first, so its own stubs declare.
fn stub_declares<'h>(dir: &Path, stem: &str, mapping: &Value, hints: &'h OpHints) -> Vec<&'h str> {
    if let Some(list) = get(mapping, "metadata").and_then(|m| get_arr(m, "x-cases")) {
        if !list.iter().any(|c| c.as_str() == Some(stem)) {
            return Vec::new();
        }
    }
    let document = std::fs::read_to_string(
        dir.join("tests")
            .join("cases")
            .join(format!("{}.graphql", stem)),
    )
    .unwrap_or_default();
    let mut out: Vec<&str> = Vec::new();
    for k in hints
        .for_case(stem)
        .into_iter()
        .chain(hints.for_document(&document))
    {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

pub fn validate_workspace(dir: &Path) -> Result<Report, String> {
    let inventory_file = dir.join(".factory").join("inventory.json");
    let inventory =
        match crate::factory_io::read_to_string_optional(dir, ".factory/inventory.json")? {
            Some(text) => crate::json::parse(&text)?,
            None => {
                return Err(format!(
                    "no {}; run inventory build first",
                    inventory_file.display()
                ))
            }
        };
    let oracle = oracle_label(dir);
    let prefixes = base_path_prefixes(&inventory, dir);
    let shapes = get(&inventory, "shapes")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let selection: Option<Value> =
        crate::factory_io::read_to_string_optional(dir, ".factory/selection.yaml")?
            .and_then(|s| crate::yaml::parse(&s).ok());
    let waivers: Vec<Waiver> = selection
        .as_ref()
        .map(crate::waivers::read_waivers)
        .unwrap_or_default();
    // What a stub's cases or a unit entry's `target` declares, for a path
    // several operations match equally (ADR 0044). The schema ties an entity
    // entry to its type; it is an ordinary workspace file, not `.factory`.
    let workspace: Option<Value> =
        crate::factory_io::read_to_string_optional(dir, ".factory/workspace.yaml")?
            .and_then(|s| crate::yaml::parse(&s).ok());
    // Validated and read through custody (ADR 0075/0078 B1): a raw
    // dir.join(directory + ".graphql") read here let a poisoned or
    // out-of-pattern `directory` in workspace.yaml serve an outside file as
    // the schema, with validate (and evidence's conformance layer, which
    // calls this same function) exiting 0 over it.
    let sdl = workspace
        .as_ref()
        .and_then(|w| crate::reconcile::read_schema_file(dir, w).ok())
        .map(|(_, sdl)| sdl);
    let hints = crate::op_match::OpHints::from_selection(
        workspace.as_ref(),
        selection.as_ref(),
        sdl.as_deref(),
    );
    let mut results: Vec<Result_> = Vec::new();

    let mappings_dir = dir.join("tests").join("fixtures").join("mappings");
    let mut mapping_files: Vec<String> = std::fs::read_dir(&mappings_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    mapping_files.sort();
    for file in mapping_files {
        let mapping = match crate::json::parse(
            &std::fs::read_to_string(mappings_dir.join(&file)).unwrap_or_default(),
        ) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let req = get(&mapping, "request")
            .cloned()
            .unwrap_or(Value::Object(obj()));
        let method = get_str(&req, "method").unwrap_or("GET").to_string();
        let url_path = get_str(&req, "urlPath")
            .or_else(|| get_str(&req, "urlPathTemplate"))
            .map(str::to_string)
            .or_else(|| get_str(&req, "url").map(|u| u.split('?').next().unwrap_or("").to_string()))
            .or_else(|| get_str(&req, "urlPattern").map(str::to_string))
            .unwrap_or_default();
        let url_query = stub_query(&req);
        let where_ = format!("tests/fixtures/mappings/{}", file);
        let declared = stub_declares(
            dir,
            file.strip_suffix(".json").unwrap_or(&file),
            &mapping,
            &hints,
        );
        let op = if url_path.is_empty() {
            Ok(None)
        } else {
            find_operation_for_request(&inventory, &method, &url_path, &url_query, &declared)
        };
        let op = match op {
            Ok(Some(o)) => o,
            Err(e) => {
                // A stub that answers no case's root call — a nested request, or
                // an unreachable recording `fixtures` rewrites under its own
                // name — declares nothing and can only be waived.
                let reason = format!(
                    "{}; a stub declares only through its own name: name it after the case whose root call it answers (tests/cases/<case>.graphql selects a root field a selection entry declares); a stub for a nested request or an unreachable recording (its metadata \"x-cases\" does not list its own name) declares nothing, so waive it: graphos-factory-core codify --waive {} --status unmatched --reason R",
                    e, where_
                );
                results.push(Result_ {
                    where_,
                    operation: None,
                    kind: None,
                    http_status: None,
                    status: "unmatched",
                    reason: Some(reason),
                    problems: vec![],
                    note: None,
                    waiver: None,
                });
                continue;
            }
            Ok(None) => {
                results.push(Result_ {
                    where_,
                    operation: None,
                    kind: None,
                    http_status: None,
                    status: "unmatched",
                    reason: Some(format!(
                        "no inventory operation matches {} {}",
                        method, url_path
                    )),
                    problems: vec![],
                    note: None,
                    waiver: None,
                });
                continue;
            }
        };
        let key = get_str(op, "key").unwrap_or("").to_string();
        let response = get(&mapping, "response");
        let status = response
            .and_then(|r| get(r, "status"))
            .cloned()
            .unwrap_or(Value::from(200));
        let mut body = response.and_then(|r| get(r, "jsonBody")).cloned();
        if body.is_none() {
            if let Some(text) = response.and_then(|r| get_str(r, "body")) {
                body = crate::json::parse(text).ok();
            }
        }
        if body.is_none() {
            if let Some(f) = response.and_then(|r| get_str(r, "bodyFileName")) {
                let path = dir.join("tests").join("fixtures").join("__files").join(f);
                if let Ok(text) = std::fs::read_to_string(path) {
                    body = crate::json::parse(&text).ok();
                }
            }
        }
        let target = response_shape_for(op, &status_str(&status));
        match body {
            None => results.push(Result_ {
                where_: where_.clone(),
                operation: Some(key.clone()),
                kind: Some("response"),
                http_status: None,
                status: "unchecked",
                reason: Some(format!(
                    "no JSON body in the stub response (status {})",
                    status_str(&status)
                )),
                problems: vec![],
                note: None,
                waiver: None,
            }),
            Some(body) => match target.as_ref().and_then(|t| t.shape_ref.clone()) {
                None => results.push(Result_ {
                    where_: where_.clone(),
                    operation: Some(key.clone()),
                    kind: Some("response"),
                    http_status: None,
                    status: "unchecked",
                    reason: Some(format!(
                        "the spec documents no body shape for {} → {}",
                        key,
                        status_str(&status)
                    )),
                    problems: vec![],
                    note: None,
                    waiver: None,
                }),
                Some(shape_ref) => {
                    let problems = conform(
                        &body,
                        &crate::json::object(vec![("$ref", Value::from(shape_ref))]),
                        &shapes,
                        "$",
                        "response",
                    );
                    let documented = target.as_ref().map(|t| t.documented).unwrap_or(true);
                    results.push(Result_ {
                        where_: where_.clone(),
                        operation: Some(key.clone()),
                        kind: Some("response"),
                        http_status: Some(status.clone()),
                        status: if problems.is_empty() { "pass" } else { "fail" },
                        reason: None,
                        problems,
                        note: if documented {
                            None
                        } else {
                            Some(format!("status {} is not documented; checked against the documented success shape", status_str(&status)))
                        },
                        waiver: None,
                    });
                }
            },
        }
        for pattern in get_arr(&req, "bodyPatterns").into_iter().flatten() {
            let eq = match crate::json::field(pattern, "equalToJson") {
                Some(e) => e,
                None => continue,
            };
            let req_body = match eq {
                Value::String(s) => match crate::json::parse(s) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
                other => other.clone(),
            };
            match get(op, "request_body").and_then(|r| get_str(r, "shape_ref")) {
                None => results.push(Result_ {
                    where_: where_.clone(),
                    operation: Some(key.clone()),
                    kind: Some("request"),
                    http_status: None,
                    status: "unchecked",
                    reason: Some("the spec documents no request body shape".to_string()),
                    problems: vec![],
                    note: None,
                    waiver: None,
                }),
                Some(shape_ref) => {
                    let problems = conform(
                        &req_body,
                        &crate::json::object(vec![("$ref", Value::from(shape_ref))]),
                        &shapes,
                        "$",
                        "request",
                    );
                    results.push(Result_ {
                        where_: where_.clone(),
                        operation: Some(key.clone()),
                        kind: Some("request"),
                        http_status: None,
                        status: if problems.is_empty() { "pass" } else { "fail" },
                        reason: None,
                        problems,
                        note: None,
                        waiver: None,
                    });
                }
            }
        }
    }

    let tests_dir = dir.join("tests");
    let mut suites: Vec<String> = std::fs::read_dir(&tests_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".connector.yaml"))
                .collect()
        })
        .unwrap_or_default();
    suites.sort();
    for file in suites {
        let suite = match crate::yaml::parse(
            &std::fs::read_to_string(tests_dir.join(&file)).unwrap_or_default(),
        ) {
            Ok(s) => s,
            Err(e) => {
                results.push(Result_ {
                    where_: format!("tests/{}", file),
                    operation: None,
                    kind: None,
                    http_status: None,
                    status: "unmatched",
                    reason: Some(format!("suite is not readable YAML: {}", e)),
                    problems: vec![],
                    note: None,
                    waiver: None,
                });
                continue;
            }
        };
        for entry in get_arr(&suite, "tests").into_iter().flatten() {
            let where_ = format!("tests/{} › {}", file, get_str(entry, "name").unwrap_or(""));
            let cr = get(entry, "expect")
                .and_then(|e| get(e, "connectorRequest"))
                .cloned()
                .unwrap_or(Value::Object(obj()));
            let method = get_str(&cr, "method").unwrap_or("GET").to_string();
            let url = get_str(&cr, "url").and_then(|u| url::Url::parse(u).ok());
            let declared = get_str(entry, "target")
                .map(|t| hints.for_target(t))
                .unwrap_or_default();
            let op = match url.as_ref() {
                Some(u) => find_operation_for_request(
                    &inventory,
                    &method,
                    &strip_prefixes(u.path(), &prefixes),
                    &u.query_pairs()
                        .map(|(k, v)| (k.into_owned(), v.into_owned()))
                        .collect::<Vec<_>>(),
                    &declared,
                ),
                None => Ok(None),
            };
            let op = match op {
                Ok(Some(o)) => o,
                Err(e) => {
                    results.push(Result_ {
                        where_,
                        operation: None,
                        kind: None,
                        http_status: None,
                        status: "unmatched",
                        reason: Some(format!(
                            "{}; the entry's target must be the root field a selection entry declares, or the type an entity entry's root field returns",
                            e
                        )),
                        problems: vec![],
                        note: None,
                        waiver: None,
                    });
                    continue;
                }
                Ok(None) => {
                    results.push(Result_ {
                        where_,
                        operation: None,
                        kind: None,
                        http_status: None,
                        status: "unmatched",
                        reason: Some(format!(
                            "no inventory operation matches {} {}",
                            method,
                            url.as_ref()
                                .map(|u| u.path().to_string())
                                .unwrap_or_else(|| "?".to_string())
                        )),
                        problems: vec![],
                        note: None,
                        waiver: None,
                    });
                    continue;
                }
            };
            let key = get_str(op, "key").unwrap_or("").to_string();
            if let Some(text) = get_str(entry, "apiResponseBody") {
                let body = crate::json::parse(text).ok();
                let default_status = get(op, "response")
                    .and_then(|r| get(r, "status"))
                    .map(status_str)
                    .unwrap_or_else(|| "200".to_string());
                let target = response_shape_for(op, &default_status);
                match body {
                    None => results.push(Result_ {
                        where_: where_.clone(),
                        operation: Some(key.clone()),
                        kind: Some("response"),
                        http_status: None,
                        status: "unmatched",
                        reason: Some("apiResponseBody is not JSON".to_string()),
                        problems: vec![],
                        note: None,
                        waiver: None,
                    }),
                    Some(body) => match target.and_then(|t| t.shape_ref) {
                        None => results.push(Result_ {
                            where_: where_.clone(),
                            operation: Some(key.clone()),
                            kind: Some("response"),
                            http_status: None,
                            status: "unchecked",
                            reason: Some("the spec documents no response body shape".to_string()),
                            problems: vec![],
                            note: None,
                            waiver: None,
                        }),
                        Some(shape_ref) => {
                            let problems = conform(
                                &body,
                                &crate::json::object(vec![("$ref", Value::from(shape_ref))]),
                                &shapes,
                                "$",
                                "response",
                            );
                            results.push(Result_ {
                                where_: where_.clone(),
                                operation: Some(key.clone()),
                                kind: Some("response"),
                                http_status: None,
                                status: if problems.is_empty() { "pass" } else { "fail" },
                                reason: None,
                                problems,
                                note: None,
                                waiver: None,
                            });
                        }
                    },
                }
            }
            if let Some(text) = get_str(&cr, "body") {
                let body = crate::json::parse(text).ok();
                match body {
                    None => results.push(Result_ {
                        where_: where_.clone(),
                        operation: Some(key.clone()),
                        kind: Some("request"),
                        http_status: None,
                        status: "unmatched",
                        reason: Some("expected body is not JSON".to_string()),
                        problems: vec![],
                        note: None,
                        waiver: None,
                    }),
                    Some(body) => match get(op, "request_body")
                        .and_then(|r| get_str(r, "shape_ref"))
                    {
                        None => results.push(Result_ {
                            where_: where_.clone(),
                            operation: Some(key.clone()),
                            kind: Some("request"),
                            http_status: None,
                            status: "unchecked",
                            reason: Some("the spec documents no request body shape".to_string()),
                            problems: vec![],
                            note: None,
                            waiver: None,
                        }),
                        Some(shape_ref) => {
                            let problems = conform(
                                &body,
                                &crate::json::object(vec![("$ref", Value::from(shape_ref))]),
                                &shapes,
                                "$",
                                "request",
                            );
                            results.push(Result_ {
                                where_: where_.clone(),
                                operation: Some(key.clone()),
                                kind: Some("request"),
                                http_status: None,
                                status: if problems.is_empty() { "pass" } else { "fail" },
                                reason: None,
                                problems,
                                note: None,
                                waiver: None,
                            });
                        }
                    },
                }
            }
        }
    }

    // Waivers: an unchecked or unmatched body the engineer has accepted.
    let mut used = vec![false; waivers.len()];
    for r in results.iter_mut() {
        if r.status != "unchecked" && r.status != "unmatched" {
            continue;
        }
        if let Some(i) = waivers
            .iter()
            .position(|w| w.matches(&r.where_, r.operation.as_deref(), r.status))
        {
            r.status = "waived";
            r.waiver = Some(i);
            used[i] = true;
        }
    }

    let mut counts: std::collections::BTreeMap<&str, u64> =
        ["pass", "fail", "unchecked", "unmatched", "waived"]
            .into_iter()
            .map(|k| (k, 0))
            .collect();
    for r in &results {
        *counts.entry(r.status).or_insert(0) += 1;
    }
    // Per operation: the worst thing seen. An unmatched body that still
    // names its operation (unreadable JSON) is that operation's failure.
    let rank = |s: &str| match s {
        "fail" | "unmatched" => 4,
        "pass" => 3,
        "waived" => 2,
        _ => 1,
    };
    let mut by_operation = obj();
    for r in &results {
        let key = match &r.operation {
            Some(k) => k.clone(),
            None => continue,
        };
        let next = if r.status == "unmatched" {
            "fail"
        } else {
            r.status
        };
        let prev = by_operation.get(&key).and_then(Value::as_str);
        if prev.map(|p| rank(p) < rank(next)).unwrap_or(true) {
            by_operation.insert(key, Value::from(next));
        }
    }
    let unused: Vec<Value> = waivers
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|(w, _)| waiver_json(w))
        .collect();
    let applied: Vec<Value> = waivers
        .iter()
        .zip(&used)
        .filter(|(_, u)| **u)
        .map(|(w, _)| waiver_json(w))
        .collect();
    let json = crate::json::object(vec![
        ("oracle", Value::from(oracle)),
        (
            "counts",
            crate::json::object(counts.iter().map(|(k, v)| (*k, Value::from(*v))).collect()),
        ),
        ("operations", Value::Object(by_operation)),
        (
            "results",
            Value::Array(results.iter().map(|r| result_json(r, &waivers)).collect()),
        ),
        (
            "waivers",
            crate::json::object(vec![
                ("applied", Value::Array(applied)),
                ("unused", Value::Array(unused)),
            ]),
        ),
    ]);
    let failing = counts["fail"] > 0 || counts["unmatched"] > 0;
    Ok(Report { json, failing })
}

/// The query a WireMock stub matches, decoded: the inline query of `url`, or
/// the `equalTo` values of `queryParameters` (beside `urlPath`). Other
/// matchers (`matches`, `contains`) state no literal value and are skipped.
fn stub_query(req: &Value) -> Vec<(String, String)> {
    if let Some((_, q)) = get_str(req, "url").and_then(|u| u.split_once('?')) {
        return url::form_urlencoded::parse(q.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
    }
    get(req, "queryParameters")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, m)| get_str(m, "equalTo").map(|v| (k.clone(), v.to_string())))
        .collect()
}

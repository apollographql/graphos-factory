//! fixtures — recorded samples -> case-scoped WireMock mappings.
//!
//!   fixtures [workspace] [--check]
//!
//! .factory/recordings.yaml says which sample answers which test case. Each
//! case becomes tests/fixtures/mappings/<case>.json whose request matcher is
//! derived from the sample's actual request and whose response is the
//! recorded status and body. Unreachable samples become mappings with
//! `"x-cases": []`. --check rewrites nothing and exits 1 on drift.

use crate::args::{Args, Flags};
use crate::json::{get, get_arr, get_obj, get_str, obj, pretty};
use regex::Regex;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct MappingOptions<'a> {
    pub case_names: Option<Vec<String>>,
    pub note: Option<&'a str>,
    pub base_path: &'a str,
}

/// Build a WireMock mapping from a recorded sample. `dir_name`/`n` identify
/// the sample file for the provenance metadata.
pub fn mapping_from_sample(
    sample: &Value,
    dir_name: &str,
    n: &str,
    opts: &MappingOptions,
) -> Result<Value, String> {
    let request = get(sample, "request").ok_or("sample has no request")?;
    let url = url::Url::parse(get_str(request, "url").ok_or("sample request has no url")?)
        .map_err(|e| e.to_string())?;
    let mut url_path = url.path().to_string();
    if !opts.base_path.is_empty() && opts.base_path != "/" && url_path.starts_with(opts.base_path) {
        let rest = url_path[opts.base_path.len()..].to_string();
        url_path = if rest.is_empty() {
            "/".to_string()
        } else {
            rest
        };
    }
    let mut req = obj();
    req.insert(
        "method".into(),
        get(request, "method").cloned().unwrap_or(Value::Null),
    );
    req.insert("urlPath".into(), Value::from(url_path));
    let mut params = obj();
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut keys: Vec<String> = Vec::new();
    for (k, _) in &pairs {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    for key in keys {
        let values: Vec<&String> = pairs
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .collect();
        let matcher = if values.len() == 1 {
            crate::json::object(vec![("equalTo", Value::from(values[0].as_str()))])
        } else {
            crate::json::object(vec![(
                "hasExactly",
                Value::Array(
                    values
                        .iter()
                        .map(|v| crate::json::object(vec![("equalTo", Value::from(v.as_str()))]))
                        .collect(),
                ),
            )])
        };
        params.insert(key, matcher);
    }
    if !params.is_empty() {
        req.insert("queryParameters".into(), Value::Object(params));
    }
    let cred = Regex::new(r"(?i)^authorization$|api[-_]?key").unwrap();
    for (name, value) in get_obj(request, "headers").into_iter().flatten() {
        if cred.is_match(name) {
            let value_s = value.as_str().unwrap_or("");
            let matcher = if value_s.contains("{$env.") {
                crate::json::object(vec![("matches", Value::from(".+"))])
            } else {
                crate::json::object(vec![("equalTo", Value::from(value_s))])
            };
            let headers = req.entry("headers").or_insert_with(|| Value::Object(obj()));
            if let Value::Object(h) = headers {
                h.insert(name.clone(), matcher);
            }
        }
    }
    let response = get(sample, "response").ok_or("sample has no response")?;
    let mut resp = obj();
    resp.insert(
        "status".into(),
        get(response, "status").cloned().unwrap_or(Value::Null),
    );
    let mut headers = obj();
    if let Some(ct) = get_str(response, "content_type") {
        headers.insert("Content-Type".into(), Value::from(ct));
    }
    resp.insert("headers".into(), Value::Object(headers));
    let body = crate::json::field(response, "body");
    let is_json = get(response, "json") != Some(&Value::Bool(false));
    match body {
        Some(b) if is_json && b.is_object() || is_json && b.is_array() => {
            resp.insert("jsonBody".into(), b.clone());
        }
        Some(Value::Null) | None => {}
        Some(b) => {
            let text = match b {
                Value::String(s) => s.clone(),
                other => crate::json::compact(other),
            };
            resp.insert("body".into(), Value::from(text));
        }
    }
    let mut metadata = obj();
    metadata.insert(
        "x-recorded-from".into(),
        Value::from(format!(".factory/samples/{}/{}.json", dir_name, n)),
    );
    metadata.insert(
        "x-recorded-at".into(),
        get(sample, "recorded_at").cloned().unwrap_or(Value::Null),
    );
    if let Some(cases) = &opts.case_names {
        metadata.insert(
            "x-cases".into(),
            Value::Array(cases.iter().map(|c| Value::from(c.as_str())).collect()),
        );
    }
    if let Some(note) = opts.note {
        metadata.insert("x-note".into(), Value::from(note));
    }
    Ok(crate::json::object(vec![
        ("metadata", Value::Object(metadata)),
        ("request", Value::Object(req)),
        ("response", Value::Object(resp)),
    ]))
}

/// A recording that sent none of an endpoint's optional query parameters
/// derives a matcher with no `queryParameters` at all — unconstrained, so it
/// also matches a sibling mapping's request that did send one. Any query
/// parameter key a sibling mapping for the same method+urlPath asserts must
/// be explicitly `absent` here, or the two race once every stub loads
/// together (`e2e.sh` no longer resets stub mappings between cases).
pub fn assert_absent_siblings(mappings: &mut [&mut Value]) {
    let mut keys_by_endpoint: std::collections::HashMap<(String, String), Vec<String>> =
        std::collections::HashMap::new();
    for mapping in mappings.iter() {
        let Some(req) = get(mapping, "request") else {
            continue;
        };
        let endpoint = (
            get_str(req, "method").unwrap_or("").to_string(),
            get_str(req, "urlPath").unwrap_or("").to_string(),
        );
        let entry = keys_by_endpoint.entry(endpoint).or_default();
        for key in get_obj(req, "queryParameters").into_iter().flatten() {
            if !entry.contains(key.0) {
                entry.push(key.0.clone());
            }
        }
    }
    for mapping in mappings.iter_mut() {
        let Some(req) = get(*mapping, "request") else {
            continue;
        };
        let endpoint = (
            get_str(req, "method").unwrap_or("").to_string(),
            get_str(req, "urlPath").unwrap_or("").to_string(),
        );
        let Some(all_keys) = keys_by_endpoint.get(&endpoint) else {
            continue;
        };
        let present: Vec<String> = get_obj(req, "queryParameters")
            .into_iter()
            .flatten()
            .map(|(k, _)| k.clone())
            .collect();
        let missing: Vec<&String> = all_keys.iter().filter(|k| !present.contains(k)).collect();
        if missing.is_empty() {
            continue;
        }
        let req_mut = mapping
            .get_mut("request")
            .and_then(Value::as_object_mut)
            .unwrap();
        let params = req_mut
            .entry("queryParameters")
            .or_insert_with(|| Value::Object(obj()))
            .as_object_mut()
            .unwrap();
        for key in missing {
            params.insert(
                key.clone(),
                crate::json::object(vec![("absent", Value::Bool(true))]),
            );
        }
    }
}

fn load_sample(dir: &Path, reference: &str) -> Result<(Value, String, String), String> {
    let (sub, n) = reference
        .split_once('/')
        .ok_or_else(|| format!("bad sample reference {}", reference))?;
    let rel = format!(".factory/samples/{}/{}.json", sub, n);
    let text = match crate::factory_io::read_to_string_optional(dir, &rel)? {
        Some(text) => text,
        None => {
            return Err(format!(
                "no sample {} ({})",
                reference,
                dir.join(&rel).display()
            ))
        }
    };
    let sample = crate::json::parse(&text)?;
    Ok((sample, sub.to_string(), n.to_string()))
}

fn run(args: &Args) -> Result<i32, String> {
    let check = args.has("check");
    let dir = Path::new(&args.dir()).to_path_buf();
    const MANIFEST: &str = ".factory/recordings.yaml";
    let manifest_file = dir.join(MANIFEST);
    let manifest = match crate::factory_io::read_to_string_optional(&dir, MANIFEST)? {
        Some(text) => crate::yaml::parse(&text).map_err(|e| format!("{}: {}", MANIFEST, e))?,
        None => return Err(format!("no {}", manifest_file.display())),
    };
    let manifest = if manifest.is_null() {
        Value::Object(obj())
    } else {
        manifest
    };
    let out_dir = dir.join("tests").join("fixtures").join("mappings");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let mut base_path = String::new();
    let template_file = dir.join("template.yaml");
    if template_file.exists() {
        let template = crate::yaml::parse_file(&template_file)?;
        if let Some(base) = get_arr(&template, "variables")
            .into_iter()
            .flatten()
            .find(|v| get_str(v, "name") == Some("BASE_URL"))
            .and_then(|v| get_str(v, "test_default"))
        {
            if let Ok(u) = url::Url::parse(base) {
                base_path = u.path().trim_end_matches('/').to_string();
            }
        }
    }

    let mut planned: Vec<(PathBuf, Value)> = Vec::new();
    let cases = get_obj(&manifest, "cases").cloned().unwrap_or_default();
    for (case_name, spec) in &cases {
        let (sample, sub, n) = load_sample(
            &dir,
            get_str(spec, "sample").ok_or_else(|| format!("case {} has no sample", case_name))?,
        )?;
        let extra: Vec<String> = get_arr(spec, "also")
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let case_names = if extra.is_empty() {
            None
        } else {
            let mut all = vec![case_name.clone()];
            all.extend(extra);
            Some(all)
        };
        let mapping = mapping_from_sample(
            &sample,
            &sub,
            &n,
            &MappingOptions {
                case_names,
                note: None,
                base_path: &base_path,
            },
        )?;
        planned.push((out_dir.join(format!("{}.json", case_name)), mapping));
    }
    let unreachable = get_arr(&manifest, "unreachable")
        .cloned()
        .unwrap_or_default();
    for entry in &unreachable {
        let (sample, sub, n) = load_sample(
            &dir,
            get_str(entry, "sample").ok_or("unreachable entry has no sample")?,
        )?;
        let note = get_str(entry, "note").unwrap_or("recorded; no case can issue this request");
        let mapping = mapping_from_sample(
            &sample,
            &sub,
            &n,
            &MappingOptions {
                case_names: Some(vec![]),
                note: Some(note),
                base_path: &base_path,
            },
        )?;
        planned.push((
            out_dir.join(format!("unreachable_{}_{}.json", sub, n)),
            mapping,
        ));
    }

    let mut refs: Vec<&mut Value> = planned.iter_mut().map(|(_, m)| m).collect();
    assert_absent_siblings(&mut refs);

    let mut changed = 0;
    for (file, mapping) in &planned {
        let text = pretty(mapping);
        let current = std::fs::read_to_string(file).ok();
        if current.as_deref() != Some(text.as_str()) {
            changed += 1;
            if check {
                println!("drift: {}", crate::json::relative(&dir, file));
            } else {
                std::fs::write(file, text).map_err(|e| e.to_string())?;
            }
        }
    }
    let mut foreign: Vec<String> = std::fs::read_dir(&out_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().map(|e| e == "json").unwrap_or(false)
                        && !planned.iter().any(|(f, _)| f == p)
                })
                .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    foreign.sort();
    for f in foreign {
        println!(
            "note: tests/fixtures/mappings/{} is not derived from a recording",
            f
        );
    }

    if check {
        if changed > 0 {
            println!(
                "fixtures: {} mapping(s) differ from their recordings",
                changed
            );
            Ok(1)
        } else {
            println!(
                "fixtures: {} mapping(s) match their recordings",
                planned.len()
            );
            Ok(0)
        }
    } else {
        println!(
            "fixtures: wrote {} of {} mapping(s) from {} case(s) and {} unreachable recording(s)",
            changed,
            planned.len(),
            cases.len(),
            unreachable.len()
        );
        Ok(0)
    }
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["check"],
    valued: &[],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fixtures: {}", e);
            1
        }
    }
}

//! probe — one recorded, scrubbed request against the live API.
//!
//!   probe [workspace] --key "get:/deck/{deck_id}/draw/" --url URL
//!         [--method GET] [--header "Name: value"]... [--body JSON]
//!         [--auth-env NAME --auth-header "Authorization: Bearer {value}"]
//!         [--note TEXT] [--allow-write] [--allow-residual] [--dry-run] [--json]
//!
//! GET/HEAD only unless --allow-write; everything is scrubbed before it
//! touches disk; a residual secret-looking value refuses the write (exit 4).
//! The request goes through curl so the container's proxy and CA apply.
//! Exit codes: 1 error · 2 write without --allow-write · 3 credential unset · 4 residual.

use crate::args::{Args, Flags};
use crate::json::{get, get_arr, get_str, obj, pretty};
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// `get:/deck/{deck_id}/draw/` -> `get_deck_deck_id_draw`
pub fn sample_dir(key: &str) -> String {
    let lower = key.to_lowercase();
    let collapsed = Regex::new(r"[^a-z0-9]+")
        .unwrap()
        .replace_all(&lower, "_")
        .to_string();
    collapsed.trim_matches('_').to_string()
}

fn fail(msg: &str, code: i32) -> i32 {
    eprintln!("probe: {}", msg);
    code
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["allow-write", "allow-residual", "dry-run", "json"],
    valued: &[
        "key",
        "url",
        "method",
        "header",
        "body",
        "auth-env",
        "auth-header",
        "note",
    ],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    let (key, url) = match (args.get("key"), args.get("url")) {
        (Some(k), Some(u)) => (k.to_string(), u.to_string()),
        _ => {
            return fail(
                "usage: probe [workspace] --key <method:/path> --url <url> [options]",
                1,
            )
        }
    };
    if !Regex::new(r"^(get|put|post|delete|patch|head):/")
        .unwrap()
        .is_match(&key)
    {
        return fail(
            &format!("--key must be {{method}}:{{path}}, got {}", key),
            1,
        );
    }
    let method = args
        .get("method")
        .map(str::to_string)
        .unwrap_or_else(|| key.split(':').next().unwrap_or("").to_string())
        .to_uppercase();
    if !matches!(method.as_str(), "GET" | "HEAD") && !args.has("allow-write") {
        return fail(&format!("{} is a write; re-run with --allow-write only against a sandbox you are allowed to change", method), 2);
    }

    let mut headers: Vec<String> = args.all("header");
    let mut credential: Option<String> = None;
    let mut credential_value: Option<String> = None;
    if let Some(env_name) = args.get("auth-env") {
        let value = match std::env::var(env_name) {
            Ok(v) if !v.is_empty() => v,
            _ => return fail(&format!("environment variable {} is not set", env_name), 3),
        };
        let template = args
            .get("auth-header")
            .unwrap_or("Authorization: Bearer {value}");
        headers.push(template.replace("{value}", &value));
        credential = Some(env_name.to_string());
        credential_value = Some(value);
    }
    if !headers
        .iter()
        .any(|h| Regex::new(r"(?i)^accept:").unwrap().is_match(h))
    {
        headers.push("Accept: application/json".to_string());
    }

    let out_dir = std::env::temp_dir().join(format!("probe-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&out_dir);
    let body_file = out_dir.join("body");
    let head_file = out_dir.join("headers");
    let mut curl_args: Vec<String> = vec![
        "-sS".into(),
        "--max-time".into(),
        "30".into(),
        "-X".into(),
        method.clone(),
        "-o".into(),
        body_file.to_string_lossy().to_string(),
        "-D".into(),
        head_file.to_string_lossy().to_string(),
        "-w".into(),
        "%{http_code}".into(),
    ];
    for h in &headers {
        curl_args.push("-H".into());
        curl_args.push(h.clone());
    }
    if let Some(b) = args.get("body") {
        curl_args.push("-H".into());
        curl_args.push("Content-Type: application/json".into());
        curl_args.push("--data-binary".into());
        curl_args.push(b.to_string());
    }
    curl_args.push(url.clone());

    if args.has("dry-run") {
        let shown: Vec<String> = headers
            .iter()
            .map(|h| match (&credential, &credential_value) {
                (Some(c), Some(v)) if h.contains(v.as_str()) => {
                    h.replace(v.as_str(), &format!("{{$env.{}}}", c))
                }
                _ => h.clone(),
            })
            .collect();
        println!(
            "probe (dry run): {} {}\n  headers: {}",
            method,
            url,
            shown.join(" | ")
        );
        return 0;
    }

    let started = crate::now_iso();
    let res = match Command::new("curl").args(&curl_args).output() {
        Ok(r) => r,
        Err(e) => return fail(&format!("curl failed: {}", e), 1),
    };
    if !res.status.success() {
        return fail(
            &format!(
                "curl failed: {}",
                String::from_utf8_lossy(&res.stderr).trim()
            ),
            1,
        );
    }
    let status: i64 = String::from_utf8_lossy(&res.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    let raw_headers = std::fs::read_to_string(&head_file).unwrap_or_default();
    let content_type = Regex::new(r"(?im)^content-type:\s*(.+)$")
        .unwrap()
        .captures(&raw_headers)
        .map(|m| m[1].trim().to_string());
    let raw_body = std::fs::read_to_string(&body_file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&out_dir);

    let (body, body_is_json) = if raw_body.trim().is_empty() {
        (Value::Null, false)
    } else {
        match crate::json::parse(&raw_body) {
            Ok(v) => (v, true),
            Err(_) => (Value::from(raw_body.clone()), false),
        }
    };

    let mut request_headers = obj();
    for h in &headers {
        let (name, rest) = h.split_once(':').unwrap_or((h, ""));
        let val = rest.trim().to_string();
        let stored = match (&credential, &credential_value) {
            (Some(c), Some(v)) if val.contains(v.as_str()) => format!("{{$env.{}}}", c),
            _ => val,
        };
        request_headers.insert(name.trim().to_string(), Value::from(stored));
    }
    let to_scrub = crate::json::object(vec![
        ("url", Value::from(url.as_str())),
        ("request_headers", Value::Object(request_headers)),
        ("body", body),
    ]);
    let scrubbed = crate::scrub::scrub(&to_scrub, &[], "$");
    if scrubbed.has_residual() && !args.has("allow-residual") {
        eprintln!(
            "probe: refusing to write — {} value(s) still look like secrets after scrubbing:",
            scrubbed.residual.len()
        );
        for r in &scrubbed.residual {
            eprintln!("  {}: {}", r.path, r.sample);
        }
        eprintln!("probe: inspect them; re-run with --allow-residual if they are not credentials");
        return 4;
    }

    let request_body = match args.get("body") {
        Some(b) => crate::json::parse(b).unwrap_or(Value::Null),
        None => Value::Null,
    };
    let mut scrub_info = crate::json::object(vec![
        ("replacements", Value::Object(scrubbed.replacements.clone())),
        (
            "credential",
            credential.clone().map(Value::from).unwrap_or(Value::Null),
        ),
    ]);
    if !scrubbed.residual.is_empty() {
        crate::json::set(
            &mut scrub_info,
            "residual_accepted",
            Value::Array(
                scrubbed
                    .residual
                    .iter()
                    .map(|r| {
                        crate::json::object(vec![
                            ("path", Value::from(r.path.as_str())),
                            ("sample", Value::from(r.sample.as_str())),
                        ])
                    })
                    .collect(),
            ),
        );
    }
    let sample = crate::json::object(vec![
        ("contract_version", Value::from(1)),
        ("operation", Value::from(key.as_str())),
        ("recorded_at", Value::from(started.as_str())),
        ("provenance", Value::from("probe")),
        (
            "request",
            crate::json::object(vec![
                ("method", Value::from(method.as_str())),
                (
                    "url",
                    get(&scrubbed.value, "url").cloned().unwrap_or(Value::Null),
                ),
                (
                    "headers",
                    get(&scrubbed.value, "request_headers")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                ("body", request_body),
            ]),
        ),
        (
            "response",
            crate::json::object(vec![
                ("status", Value::from(status)),
                (
                    "content_type",
                    content_type.clone().map(Value::from).unwrap_or(Value::Null),
                ),
                (
                    "body",
                    crate::json::field(&scrubbed.value, "body")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
                ("json", Value::Bool(body_is_json)),
            ]),
        ),
        ("scrub", scrub_info),
        (
            "note",
            args.get("note").map(Value::from).unwrap_or(Value::Null),
        ),
    ]);

    let samples_rel = format!(".factory/samples/{}", sample_dir(&key));
    if let Err(e) = crate::factory_io::create_dir_all(&dir, &samples_rel) {
        return fail(&e.to_string(), 1);
    }
    let existing: Vec<u64> = crate::factory_io::read_dir(&dir, &samples_rel)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .strip_suffix(".json")
                        .and_then(|n| n.parse::<u64>().ok())
                })
                .collect()
        })
        .unwrap_or_default();
    let n = existing.iter().max().map(|m| m + 1).unwrap_or(1);
    let sample_rel = format!("{}/{}.json", samples_rel, n);
    let file = dir.join(&sample_rel);
    if let Err(e) = crate::factory_io::write_in_place(&dir, &sample_rel, pretty(&sample).as_bytes())
    {
        return fail(&e.to_string(), 1);
    }

    const SOURCES_LOCK: &str = ".factory/sources.lock.yaml";
    let mut lock = match crate::factory_io::read_to_string_optional(&dir, SOURCES_LOCK) {
        Ok(Some(text)) => crate::yaml::parse(&text).unwrap_or(Value::Object(obj())),
        Ok(None) => Value::Object(obj()),
        Err(e) => return fail(&e.to_string(), 1),
    };
    if lock.is_null() {
        lock = Value::Object(obj());
    }
    let origin = url::Url::parse(&url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| url.clone());
    let mut sources = get_arr(&lock, "sources").cloned().unwrap_or_default();
    let idx = sources.iter().position(|s| {
        get_str(s, "kind") == Some("probe")
            && get_str(s, "base_url") == Some(origin.as_str())
            && get_str(s, "credential") == credential.as_deref()
    });
    let idx = match idx {
        Some(i) => i,
        None => {
            let mut entry = crate::json::object(vec![
                ("kind", Value::from("probe")),
                ("base_url", Value::from(origin.as_str())),
                ("retrieved_at", Value::from(started.as_str())),
                ("operations", Value::Array(vec![])),
            ]);
            if let Some(c) = &credential {
                crate::json::set(&mut entry, "credential", Value::from(c.as_str()));
            }
            sources.push(entry);
            sources.len() - 1
        }
    };
    crate::json::set(
        &mut sources[idx],
        "retrieved_at",
        Value::from(started.as_str()),
    );
    let mut ops = get_arr(&sources[idx], "operations")
        .cloned()
        .unwrap_or_default();
    if !ops.iter().any(|o| o.as_str() == Some(key.as_str())) {
        ops.push(Value::from(key.as_str()));
    }
    crate::json::set(&mut sources[idx], "operations", Value::Array(ops));
    crate::json::set(&mut lock, "sources", Value::Array(sources));
    let _ = crate::factory_io::write_in_place(
        &dir,
        SOURCES_LOCK,
        crate::yaml::stringify(&lock, 0).as_bytes(),
    );

    let summary = format!(
        "{} {} -> {} {}",
        method,
        url,
        status,
        content_type.unwrap_or_default()
    )
    .trim()
    .to_string();
    let reps: Vec<String> = scrubbed
        .replacements
        .iter()
        .map(|(k, v)| format!("{}×{}", k, v))
        .collect();
    println!(
        "probe: {}\n  saved {}{}",
        summary,
        crate::json::relative(&dir, &file),
        if reps.is_empty() {
            String::new()
        } else {
            format!("  (scrubbed: {})", reps.join(", "))
        }
    );
    if args.has("json") {
        println!(
            "{}",
            crate::json::compact(
                get(&sample, "response")
                    .and_then(|r| crate::json::field(r, "body"))
                    .unwrap_or(&Value::Null)
            )
        );
    }
    0
}

//! Read-only, input-bound selection review. This is not permission to write.
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

const INPUTS: &[&str] = &[
    "workspace.yaml",
    "inventory.json",
    "selection.yaml",
    "sources.lock.yaml",
    "applied.lock.yaml",
    "decisions.json",
    "findings.json",
    "memory.md",
];

/// A candidate document the caller named on the command line — not a
/// `.factory` file, so custody does not apply, but the same "regular file
/// only" rule does.
fn read_regular(path: &Path) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => std::fs::read_to_string(path)
            .map(Some)
            .map_err(|e| e.to_string()),
        Ok(_) => Err(format!("{} must be a regular file", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn snapshot(dir: &Path) -> Result<(Value, Value), String> {
    let root = dir.canonicalize().map_err(|e| e.to_string())?;
    // Custody (ADR 0025) refuses a symlinked `.factory`, but the review's
    // own message says which of the two it is, so keep the check explicit.
    match crate::factory_io::symlink_metadata(&root, crate::factory_io::STATE_DIR) {
        Ok(Some(meta)) if meta.is_dir() => {}
        Ok(None) => return Err(".factory is missing; this is not a service workspace".into()),
        Ok(Some(_)) | Err(_) => return Err(".factory must be a directory, not a symlink".into()),
    }
    let mut contents = serde_json::Map::new();
    let mut hashes = serde_json::Map::new();
    for name in INPUTS {
        let content =
            crate::factory_io::read_to_string_optional(&root, &format!(".factory/{name}"))
                .map_err(String::from)?;
        hashes.insert(
            name.to_string(),
            content
                .as_deref()
                .map(crate::spans::sha256_hex)
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        contents.insert(
            name.to_string(),
            content.map(Value::String).unwrap_or(Value::Null),
        );
    }
    for name in ["workspace.yaml", "inventory.json"] {
        if contents[name].is_null() {
            return Err(format!("missing .factory/{name}"));
        }
    }
    let binding = json!({"workspace": root.to_str().ok_or("workspace path must be UTF-8")?, "files": hashes,
        "selection_schema": crate::spans::sha256_hex(crate::schemas::SELECTION),
        "inventory_schema": crate::spans::sha256_hex(crate::schemas::INVENTORY)});
    Ok((binding, Value::Object(contents)))
}

fn validate(value: &Value, schema: &str) -> Result<(), String> {
    let schema = crate::schemas::load(schema, None).ok_or("missing embedded schema")?;
    let errors = crate::jsonschema::validate(value, &schema);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("invalid document: {errors:?}"))
    }
}

/// Snapshot only when candidate is absent. A candidate requires the input token
/// obtained before authoring; checking a reviewed candidate also supplies its token.
pub fn review(
    dir: &Path,
    candidate: Option<&str>,
    expected_input: Option<&str>,
    expected_review: Option<&str>,
) -> Result<Value, String> {
    let (binding, contents) = snapshot(dir)?;
    let input_token = crate::spans::sha256_hex(&binding.to_string());
    if expected_input.is_some_and(|token| token != input_token) {
        return Err("stale selection inputs; take a new snapshot and review again".into());
    }
    let inventory: Value = serde_json::from_str(contents["inventory.json"].as_str().unwrap())
        .map_err(|e| e.to_string())?;
    validate(&inventory, "inventory.schema.json")?;
    let before_text = contents["selection.yaml"].as_str();
    let before = before_text
        .map(crate::yaml::parse)
        .transpose()?
        .unwrap_or(Value::Null);
    if !before.is_null() {
        validate(&before, "selection.schema.json")?;
    }
    let mut report = json!({"contract_version": 1, "kind": "selection-review", "input_token": input_token,
        "binding": binding, "before_yaml": before_text, "before": before});
    if let Some(text) = candidate {
        if expected_input.is_none() {
            return Err("candidate review requires --expect-input from a prior snapshot".into());
        }
        let after = crate::yaml::parse(text)?;
        validate(&after, "selection.schema.json")?;
        let known: BTreeSet<&str> = inventory["operations"]
            .as_array()
            .ok_or("inventory operations missing")?
            .iter()
            .filter_map(|op| op["key"].as_str())
            .collect();
        let operations = after["operations"]
            .as_object()
            .ok_or("selection operations missing")?;
        for key in operations.keys() {
            if !known.contains(key.as_str()) {
                return Err(format!("unknown inventory operation: {key}"));
            }
        }
        let keys: BTreeSet<&String> = before["operations"]
            .as_object()
            .into_iter()
            .flat_map(|ops| ops.keys())
            .chain(operations.keys())
            .collect();
        let changes: Vec<Value> = keys.into_iter().filter(|key| before["operations"][*key] != after["operations"][*key]).map(|key| json!({"key": key, "before": before["operations"][key], "after": after["operations"][key]})).collect();
        let sections: Vec<&str> = ["defaults", "overrides", "waivers", "links"]
            .into_iter()
            .filter(|key| before[*key] != after[*key])
            .collect();
        let unconfirmed: Vec<&String> = operations
            .iter()
            .filter(|(_, op)| op["include"] == true && op["response"]["confirmed"] == false)
            .map(|(key, _)| key)
            .collect();
        // A drafted link is listed by `<shape> > <path>`; a declined one
        // (include: false) is an answer and is not.
        let unconfirmed_links: Vec<String> = after["links"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|link| link["include"] == true && link["confirmed"] == false)
            .filter_map(|link| {
                Some(format!(
                    "{} > {}",
                    link["shape"].as_str()?,
                    link["path"].as_str()?
                ))
            })
            .collect();
        let query_writes: Vec<&str> = inventory["operations"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|op| {
                let key = op["key"].as_str()?;
                let selected = &after["operations"][key];
                (op["semantics"] == "write"
                    && selected["include"] == true
                    && selected["graphql"]["root"] == "query")
                    .then_some(key)
            })
            .collect();
        let review_token = crate::spans::sha256_hex(&json!([input_token, text]).to_string());
        if expected_review.is_some_and(|token| token != review_token) {
            return Err("stale review; candidate bytes changed".into());
        }
        report["candidate_yaml"] = json!(text);
        report["review_token"] = json!(review_token);
        report["operation_changes"] = json!(changes);
        report["changed_sections"] = json!(sections);
        report["unconfirmed_responses"] = json!(unconfirmed);
        report["unconfirmed_links"] = json!(unconfirmed_links);
        report["query_write_decisions"] = json!(query_writes);
        report["candidate"] = after;
    } else if expected_review.is_some() {
        return Err("--expect-review requires a candidate".into());
    }
    // Detect ordinary concurrent edits during the read. This is not a filesystem
    // transaction or protection against an adversarial process racing the reader.
    if snapshot(dir)?.0 != binding {
        return Err("selection inputs changed while reading".into());
    }
    Ok(report)
}

/// The flags `selection review` accepts, for dispatch (ADR 0097); the
/// parser below also refuses a repeat.
pub const FLAGS: crate::args::Flags = crate::args::Flags {
    boolean: &[],
    valued: &["candidate", "expect-input", "expect-review"],
};

pub fn main(argv: &[String]) -> i32 {
    // Reject unknown/duplicate flags so a typo cannot disable a stale-input check.
    let mut dir = None;
    let mut flags = std::collections::BTreeMap::new();
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        if arg.starts_with('-') {
            if !["--candidate", "--expect-input", "--expect-review"].contains(&arg.as_str())
                || flags.contains_key(arg)
            {
                eprintln!("selection review: unknown or repeated flag {arg}");
                return 1;
            }
            let Some(value) = args.next().filter(|value| !value.starts_with("--")) else {
                eprintln!("selection review: {arg} needs a value");
                return 1;
            };
            flags.insert(arg.clone(), value.clone());
        } else if dir.replace(arg.clone()).is_some() {
            eprintln!("selection review: expected one workspace");
            return 1;
        }
    }
    let result = (|| {
        let candidate = flags
            .get("--candidate")
            .map(|path| {
                read_regular(Path::new(path))
                    .and_then(|text| text.ok_or("candidate file missing".into()))
            })
            .transpose()?;
        review(
            Path::new(dir.as_deref().unwrap_or(".")),
            candidate.as_deref(),
            flags.get("--expect-input").map(String::as_str),
            flags.get("--expect-review").map(String::as_str),
        )
    })();
    match result {
        Ok(report) => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            0
        }
        Err(error) => {
            eprintln!("selection review: {error}");
            1
        }
    }
}

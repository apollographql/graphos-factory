//! A small JSON Schema validator, sized to the contract schemas in
//! `schemas/` and nothing more.
//!
//! Supported keywords: $ref (local `#/...` pointers only), type, enum, const,
//! required, properties, patternProperties, additionalProperties,
//! minProperties, maxProperties, propertyNames, items, minItems,
//! uniqueItems, minimum, maximum, pattern, minLength, format (date-time
//! only), oneOf, anyOf, allOf, not, if/then/else, nullable-by-union-type.
//! An unrecognised keyword is ignored. Error messages are the same text the
//! previous implementation produced, because lint quotes them verbatim.

use crate::json::{compact, describe, is_integer};
use regex::Regex;
use serde_json::Value;

fn type_matches(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => is_integer(v),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => false,
    }
}

fn date_time(s: &str) -> bool {
    let re =
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[-+]\d{2}:\d{2})$").unwrap();
    re.is_match(s)
}

fn resolve<'a>(root: &'a Value, reference: &str) -> Result<&'a Value, String> {
    if !reference.starts_with("#/") {
        return Err(format!("only local $ref is supported, got {}", reference));
    }
    let mut node = root;
    for raw in reference[2..].split('/') {
        let part = raw.replace("~1", "/").replace("~0", "~");
        node = match node {
            Value::Object(o) => o
                .get(&part)
                .ok_or_else(|| format!("unresolvable $ref {}", reference))?,
            Value::Array(a) => part
                .parse::<usize>()
                .ok()
                .and_then(|i| a.get(i))
                .ok_or_else(|| format!("unresolvable $ref {}", reference))?,
            _ => return Err(format!("unresolvable $ref {}", reference)),
        };
    }
    Ok(node)
}

/// Validate `value` against `schema`; returns human-readable errors.
pub fn validate(value: &Value, schema: &Value) -> Vec<String> {
    validate_at(value, schema, schema, "")
}

fn validate_at(value: &Value, schema: &Value, root: &Value, path: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let at = if path.is_empty() { "/" } else { path };
    let s = match schema.as_object() {
        Some(s) => s,
        None => return errors,
    };

    if let Some(Value::String(r)) = s.get("$ref") {
        return match resolve(root, r) {
            Ok(target) => validate_at(value, target, root, path),
            Err(e) => vec![e],
        };
    }

    if let Some(t) = s.get("type") {
        let types: Vec<&str> = match t {
            Value::String(x) => vec![x.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if !types.iter().any(|t| type_matches(t, value)) {
            errors.push(format!(
                "{} expected {}, got {}",
                at,
                types.join(" or "),
                describe(value)
            ));
            return errors;
        }
    }

    if let Some(Value::Array(options)) = s.get("enum") {
        if !options.iter().any(|e| e == value) {
            let list: Vec<String> = options.iter().map(compact).collect();
            errors.push(format!("{} must be one of {}", at, list.join(", ")));
        }
    }
    if let Some(c) = s.get("const") {
        if c != value {
            errors.push(format!("{} must be {}", at, compact(c)));
        }
    }

    if let Value::String(text) = value {
        if let Some(Value::String(p)) = s.get("pattern") {
            match Regex::new(p) {
                Ok(re) => {
                    if !re.is_match(text) {
                        errors.push(format!("{} must match {}", at, p));
                    }
                }
                // A pattern the validator cannot compile must not read as a
                // pass: that is how a re-copied schema would silently weaken.
                Err(e) => errors.push(format!(
                    "{}: the schema's pattern {} does not compile ({})",
                    at, p, e
                )),
            }
        }
        if let Some(min) = s.get("minLength").and_then(Value::as_u64) {
            if (text.chars().count() as u64) < min {
                errors.push(format!("{} must be at least {} characters", at, min));
            }
        }
        if s.get("format").and_then(Value::as_str) == Some("date-time") && !date_time(text) {
            errors.push(format!("{} must be an RFC 3339 date-time", at));
        }
    }

    if let Value::Number(n) = value {
        let f = n.as_f64().unwrap_or(0.0);
        if let Some(min) = s.get("minimum").and_then(Value::as_f64) {
            if f < min {
                errors.push(format!("{} must be >= {}", at, s["minimum"]));
            }
        }
        if let Some(max) = s.get("maximum").and_then(Value::as_f64) {
            if f > max {
                errors.push(format!("{} must be <= {}", at, s["maximum"]));
            }
        }
    }

    if let Value::Array(items) = value {
        if let Some(min) = s.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < min {
                errors.push(format!("{} must have at least {} items", at, min));
            }
        }
        if s.get("uniqueItems").and_then(Value::as_bool) == Some(true) {
            let mut seen = std::collections::HashSet::new();
            for item in items {
                let key = compact(item);
                if seen.contains(&key) {
                    errors.push(format!("{} has a duplicate item {}", at, key));
                }
                seen.insert(key);
            }
        }
        if let Some(item_schema) = s.get("items") {
            for (i, item) in items.iter().enumerate() {
                errors.extend(validate_at(
                    item,
                    item_schema,
                    root,
                    &format!("{}/{}", path, i),
                ));
            }
        }
    }

    if let Value::Object(map) = value {
        if let Some(min) = s.get("minProperties").and_then(Value::as_u64) {
            if (map.len() as u64) < min {
                errors.push(format!("{} must have at least {} properties", at, min));
            }
        }
        if let Some(max) = s.get("maxProperties").and_then(Value::as_u64) {
            if (map.len() as u64) > max {
                errors.push(format!("{} must have at most {} properties", at, max));
            }
        }
        if let Some(Value::Array(required)) = s.get("required") {
            for key in required.iter().filter_map(Value::as_str) {
                if !map.contains_key(key) {
                    errors.push(format!("{} is missing required property \"{}\"", at, key));
                }
            }
        }
        if let Some(names_schema) = s.get("propertyNames") {
            for key in map.keys() {
                let why = validate_at(&Value::from(key.as_str()), names_schema, root, path);
                if !why.is_empty() {
                    errors.push(format!(
                        "{}/{} is not an allowed property name ({})",
                        path,
                        key,
                        why.join("; ").replace(&format!("{} ", at), "")
                    ));
                }
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        let patterns: Vec<(Regex, &Value)> = s
            .get("patternProperties")
            .and_then(Value::as_object)
            .map(|p| {
                p.iter()
                    .filter_map(|(k, v)| Regex::new(k).ok().map(|re| (re, v)))
                    .collect()
            })
            .unwrap_or_default();
        for (key, item) in map {
            let child_path = format!("{}/{}", path, key);
            let mut matched = false;
            if let Some(sub) = props.and_then(|p| p.get(key)) {
                matched = true;
                errors.extend(validate_at(item, sub, root, &child_path));
            }
            for (re, sub) in &patterns {
                if re.is_match(key) {
                    matched = true;
                    errors.extend(validate_at(item, sub, root, &child_path));
                }
            }
            if !matched {
                match s.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        errors.push(format!("{} is not a known property", child_path))
                    }
                    Some(sub @ Value::Object(_)) => {
                        errors.extend(validate_at(item, sub, root, &child_path))
                    }
                    _ => {}
                }
            }
        }
    }

    if let Some(Value::Array(all)) = s.get("allOf") {
        for sub in all {
            errors.extend(validate_at(value, sub, root, path));
        }
    }
    if let Some(Value::Array(any)) = s.get("anyOf") {
        if !any
            .iter()
            .any(|sub| validate_at(value, sub, root, path).is_empty())
        {
            errors.push(format!("{} matches none of the allowed shapes", at));
        }
    }
    if let Some(Value::Array(one)) = s.get("oneOf") {
        let matches = one
            .iter()
            .filter(|sub| validate_at(value, sub, root, path).is_empty())
            .count();
        if matches != 1 {
            errors.push(format!(
                "{} must match exactly one allowed shape, matched {}",
                at, matches
            ));
        }
    }
    if let Some(not) = s.get("not") {
        if validate_at(value, not, root, path).is_empty() {
            errors.push(format!("{} must not match the excluded shape", at));
        }
    }
    if let Some(condition) = s.get("if") {
        let holds = validate_at(value, condition, root, path).is_empty();
        let branch = if holds { s.get("then") } else { s.get("else") };
        if let Some(sub) = branch {
            for e in validate_at(value, sub, root, path) {
                errors.push(format!(
                    "{} (a condition on the surrounding shape {}holds)",
                    e,
                    if holds { "" } else { "does not " }
                ));
            }
        }
    }

    errors
}

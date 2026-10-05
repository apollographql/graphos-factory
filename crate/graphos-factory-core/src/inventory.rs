//! The inventory as a document: helpers that work on `inventory.json`
//! itself, whatever it was built from.
//!
//! `openapi.rs` builds an inventory from the OpenAPI 3 model (a Swagger 2.0
//! document reaches it through `swagger.rs`); `infer.rs` builds shapes from
//! recorded samples. Everything here is downstream of both and knows nothing
//! about the description format: `inventory diff` and `inventory describe`,
//! and the naming helpers every producer of a shape name shares.

use crate::json::{compact, field, get, get_arr, get_str, obj, Object};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashSet};

/// The last segment of a `#/shapes/Name` (or `#/components/schemas/Name`) reference.
pub fn shape_name(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

/// `s.replace(/[^A-Za-z0-9]+(.)?/g, (_, c) => c ? c.toUpperCase() : "").replace(/^(.)/, c => c.toUpperCase())`
pub fn cap(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphanumeric() {
            out.push(chars[i]);
            i += 1;
        } else {
            while i < chars.len() && !chars[i].is_ascii_alphanumeric() {
                i += 1;
            }
            if i < chars.len() {
                out.extend(chars[i].to_uppercase());
                i += 1;
            }
        }
    }
    let mut c = out.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => out,
    }
}

/// The API-level facts a diff reports, each as its own `api` entry beside
/// the per-operation lists: a summary can move with no operation changing
/// (ADR 0038's `api.pagination`).
const API_FIELDS: [&str; 1] = ["pagination"];

/// Compare two inventories; used by `inventory diff` after a spec refresh.
pub fn diff_inventories(before: &Value, after: &Value) -> Value {
    let by_key = |inv: &Value| -> Map<String, Value> {
        let mut m = Map::new();
        for o in get_arr(inv, "operations").into_iter().flatten() {
            if let Some(k) = get_str(o, "key") {
                m.insert(k.to_string(), o.clone());
            }
        }
        m
    };
    let a = by_key(before);
    let b = by_key(after);
    let added: Vec<Value> = b
        .keys()
        .filter(|k| !a.contains_key(*k))
        .map(|k| Value::from(k.as_str()))
        .collect();
    let removed: Vec<Value> = a
        .keys()
        .filter(|k| !b.contains_key(*k))
        .map(|k| Value::from(k.as_str()))
        .collect();
    let mut changed = Vec::new();
    for (key, next) in &b {
        let prev = match a.get(key) {
            Some(p) => p,
            None => continue,
        };
        let mut fields: Vec<Value> = Vec::new();
        for f in [
            "method",
            "path",
            "summary",
            "support",
            "support_reason",
            "semantics",
            "deprecated",
            "pagination",
        ] {
            if compact(field(prev, f).unwrap_or(&Value::Null))
                != compact(field(next, f).unwrap_or(&Value::Null))
            {
                fields.push(Value::from(f));
            }
        }
        if compact(field(prev, "parameters").unwrap_or(&Value::Null))
            != compact(field(next, "parameters").unwrap_or(&Value::Null))
        {
            fields.push(Value::from("parameters"));
        }
        if compact(field(prev, "request_body").unwrap_or(&Value::Null))
            != compact(field(next, "request_body").unwrap_or(&Value::Null))
        {
            fields.push(Value::from("request_body"));
        }
        // The response changed when its record (status, content type, the
        // envelope facts) or the shape it resolves to did — reported once.
        if compact(field(prev, "response").unwrap_or(&Value::Null))
            != compact(field(next, "response").unwrap_or(&Value::Null))
            || compact(&shape_of(prev, before)) != compact(&shape_of(next, after))
        {
            fields.push(Value::from("response"));
        }
        if !fields.is_empty() {
            changed.push(crate::json::object(vec![
                ("key", Value::from(key.as_str())),
                ("fields", Value::Array(fields)),
            ]));
        }
    }
    let mut api = Vec::new();
    for f in API_FIELDS {
        let at = |inv: &Value| {
            get(inv, "api")
                .and_then(|a| field(a, f))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let (prev, next) = (at(before), at(after));
        if compact(&prev) == compact(&next) {
            continue;
        }
        let keys = |v: &Value| -> BTreeSet<String> {
            v.as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default()
        };
        let fields: Vec<Value> = keys(&prev)
            .union(&keys(&next))
            .filter(|k| {
                compact(field(&prev, k).unwrap_or(&Value::Null))
                    != compact(field(&next, k).unwrap_or(&Value::Null))
            })
            .map(|k| Value::from(k.as_str()))
            .collect();
        api.push(crate::json::object(vec![
            ("field", Value::from(f)),
            ("fields", Value::Array(fields)),
            ("before", prev),
            ("after", next),
        ]));
    }
    crate::json::object(vec![
        ("added", Value::Array(added)),
        ("removed", Value::Array(removed)),
        ("changed", Value::Array(changed)),
        ("api", Value::Array(api)),
    ])
}

/// One `api` entry of a diff as a line: `api.pagination: cursor -> offset
/// (counts, request, response, style)`.
pub fn api_change_line(entry: &Value) -> String {
    let style = |side: &str| {
        get(entry, side)
            .and_then(|v| get_str(v, "style"))
            .unwrap_or("absent")
            .to_string()
    };
    let fields: Vec<&str> = get_arr(entry, "fields")
        .map(|f| f.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    format!(
        "api.{}: {} -> {} ({})",
        get_str(entry, "field").unwrap_or(""),
        style("before"),
        style("after"),
        fields.join(", ")
    )
}

fn shape_of(op: &Value, inventory: &Value) -> Value {
    let response = match get(op, "response") {
        Some(r) => r,
        None => return Value::Null,
    };
    match get_str(response, "shape_ref") {
        None => response.clone(),
        Some(r) => crate::json::object(vec![(
            "shape",
            get(inventory, "shapes")
                .and_then(|s| get(s, shape_name(r)))
                .cloned()
                .unwrap_or(Value::Null),
        )]),
    }
}

/// Every shape reachable from `start` through `$ref`s, `start` included:
/// the names an operation's request or response actually depends on.
pub fn shape_closure<I: IntoIterator<Item = String>>(start: I, shapes: &Object) -> Vec<String> {
    fn refs_in(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(r) = o.get("$ref").and_then(Value::as_str) {
                    out.push(shape_name(r).to_string());
                }
                for child in o.values() {
                    refs_in(child, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|i| refs_in(i, out)),
            _ => {}
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let mut stack: Vec<String> = start.into_iter().collect();
    while let Some(name) = stack.pop() {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name.clone());
        if let Some(shape) = shapes.get(&name) {
            let mut found = Vec::new();
            refs_in(shape, &mut found);
            stack.extend(found);
        }
    }
    seen
}

/// Inline every #/shapes/... reference in a shape, for `inventory describe`.
pub fn expand_shape(shape: &Value, shapes: &Object, depth: usize, seen: &HashSet<String>) -> Value {
    if depth > 8 {
        return shape.clone();
    }
    match shape {
        Value::Object(o) => {
            if let Some(r) = o.get("$ref").and_then(Value::as_str) {
                let name = shape_name(r).to_string();
                if seen.contains(&name) {
                    return crate::json::object(vec![("$recursive", Value::from(name))]);
                }
                let mut next = seen.clone();
                next.insert(name.clone());
                let expanded = match shapes.get(&name) {
                    Some(s) => expand_shape(s, shapes, depth + 1, &next),
                    None => Value::Null,
                };
                // An expansion boundary (ADR 0047) keeps its annotation beside
                // the inlined target: the relationship, its target and its
                // default projection with the evidence for it.
                return match (o.get("x-expansion"), expanded) {
                    (Some(x), Value::Object(mut t)) => {
                        t.insert("x-expansion".into(), x.clone());
                        Value::Object(t)
                    }
                    (_, expanded) => expanded,
                };
            }
            let mut out = obj();
            for (k, v) in o {
                out.insert(
                    k.clone(),
                    if v.is_object() || v.is_array() {
                        expand_shape(
                            v,
                            shapes,
                            depth + if k == "properties" { 0 } else { 1 },
                            seen,
                        )
                    } else {
                        v.clone()
                    },
                );
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| {
                    if v.is_object() || v.is_array() {
                        expand_shape(v, shapes, depth + 1, seen)
                    } else {
                        v.clone()
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

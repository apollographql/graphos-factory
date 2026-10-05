//! Conformance: does a JSON body agree with an inventory shape?
//!
//! The independent oracle of the validation stack (layer 4). Shapes are the
//! inventory's JSON-Schema-like dialect: `$ref: "#/shapes/Name"`, `type`
//! (string or array), `nullable`, `enum`, `properties`, `required`,
//! `additionalProperties`, `items`, `oneOf`, `anyOf`, `allOf`, `format`
//! (ignored). Schema-object semantics (OpenAPI 3 and Swagger 2.0 alike) apply:
//! additional properties are allowed
//! unless `additionalProperties: false`, and `readOnly`/`writeOnly` are
//! honoured for request bodies.

use crate::json::{compact, describe, get, get_arr, get_obj, get_str, is_integer, Object};
use regex::Regex;
use serde_json::Value;

fn type_check(t: &str, v: &Value) -> bool {
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

fn shape_name(reference: &str) -> &str {
    reference.rsplit('/').next().unwrap_or(reference)
}

enum Resolved<'a> {
    Shape(Option<&'a Value>),
    Error(String),
}

fn resolve<'a>(shape: &'a Value, shapes: &'a Object) -> Resolved<'a> {
    let mut current: Option<&Value> = Some(shape);
    let mut hops = 0;
    while let Some(c) = current {
        match c
            .as_object()
            .and_then(|o| o.get("$ref"))
            .and_then(Value::as_str)
        {
            Some(r) => {
                let name = shape_name(r);
                if !shapes.contains_key(name) {
                    return Resolved::Error(format!("unknown shape {}", r));
                }
                hops += 1;
                if hops > 20 {
                    return Resolved::Error(format!("reference loop at {}", r));
                }
                current = shapes.get(name);
            }
            None => break,
        }
    }
    Resolved::Shape(current)
}

fn types_of(s: &Value) -> Vec<String> {
    match get(s, "type") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(t)) => vec![t.clone()],
        _ => vec![],
    }
}

/// Check `value` against `shape`; returns problems, empty when conformant.
pub fn conform(
    value: &Value,
    shape: &Value,
    shapes: &Object,
    path: &str,
    direction: &str,
) -> Vec<String> {
    if !shape.is_object() {
        return vec![];
    }
    // `nullable: true` beside a `$ref` belongs to the referencing property,
    // not the referenced shape: honour it before dereferencing.
    if value.is_null() && get(shape, "nullable") == Some(&Value::Bool(true)) {
        return vec![];
    }
    let s = match resolve(shape, shapes) {
        Resolved::Error(e) => return vec![format!("{}: {}", path, e)],
        Resolved::Shape(s) => s,
    };
    let s = match s {
        Some(s) if s.as_object().map(|o| !o.is_empty()).unwrap_or(false) => s,
        _ => return vec![],
    };
    if let Some(u) = get_str(s, "x-unresolved") {
        return vec![format!(
            "{}: shape is unreadable in the source document ({})",
            path, u
        )];
    }

    if value.is_null() {
        let types = types_of(s);
        if get(s, "nullable") == Some(&Value::Bool(true)) || types.iter().any(|t| t == "null") {
            return vec![];
        }
        if get(s, "type").is_none()
            && get(s, "properties").is_none()
            && get(s, "items").is_none()
            && get(s, "oneOf").is_none()
            && get(s, "anyOf").is_none()
            && get(s, "enum").is_none()
        {
            return vec![];
        }
        if let Some(variants) = get_arr(s, "oneOf").or_else(|| get_arr(s, "anyOf")) {
            if variants
                .iter()
                .any(|v| conform(&Value::Null, v, shapes, path, direction).is_empty())
            {
                return vec![];
            }
        }
        return vec![format!(
            "{}: null is not allowed (shape is not nullable)",
            path
        )];
    }

    let mut problems = Vec::new();

    let one_of = get_arr(s, "oneOf");
    if let Some(variants) = one_of.or_else(|| get_arr(s, "anyOf")) {
        let results: Vec<Vec<String>> = variants
            .iter()
            .map(|v| conform(value, v, shapes, path, direction))
            .collect();
        if !results.iter().any(|r| r.is_empty()) {
            let summary: Vec<String> = results
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    format!(
                        "variant {}: {}",
                        i,
                        r.first().cloned().unwrap_or_else(|| "?".to_string())
                    )
                })
                .collect();
            problems.push(format!(
                "{}: matches none of {} {} variants ({})",
                path,
                variants.len(),
                if one_of.is_some() { "oneOf" } else { "anyOf" },
                summary.join("; ")
            ));
        }
    }

    let mut types = types_of(s);
    if types.is_empty() && get(s, "properties").is_some() {
        types.push("object".into());
    }
    if types.is_empty() && get(s, "items").is_some() {
        types.push("array".into());
    }
    if !types.is_empty() {
        let ok = types.iter().any(|t| type_check(t, value));
        if !ok {
            problems.push(format!(
                "{}: expected {}, got {}",
                path,
                types.join("|"),
                describe(value)
            ));
            return problems;
        }
    }

    if let Some(options) = get_arr(s, "enum") {
        if !options.iter().any(|e| e == value) {
            let list: Vec<String> = options.iter().map(compact).collect();
            problems.push(format!(
                "{}: {} is not one of {}",
                path,
                compact(value),
                list.join(", ")
            ));
        }
    }

    if let (Value::Array(items), Some(item_shape)) = (value, get(s, "items")) {
        for (i, item) in items.iter().enumerate() {
            problems.extend(conform(
                item,
                item_shape,
                shapes,
                &format!("{}[{}]", path, i),
                direction,
            ));
        }
    }

    if let Value::Object(map) = value {
        let props = get_obj(s, "properties").cloned().unwrap_or_default();
        let prop_shape = |key: &str| -> Value {
            match props.get(key) {
                Some(p) => match resolve(p, shapes) {
                    Resolved::Shape(Some(r)) => r.clone(),
                    _ => Value::Object(Object::new()),
                },
                None => Value::Object(Object::new()),
            }
        };
        for key in get_arr(s, "required")
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let p = prop_shape(key);
            if direction == "request" && get(&p, "readOnly") == Some(&Value::Bool(true)) {
                continue;
            }
            if direction == "response" && get(&p, "writeOnly") == Some(&Value::Bool(true)) {
                continue;
            }
            if !map.contains_key(key) {
                problems.push(format!("{}: missing required property \"{}\"", path, key));
            }
        }
        for (key, item) in map {
            if let Some(prop) = props.get(key) {
                let p = prop_shape(key);
                if direction == "request" && get(&p, "readOnly") == Some(&Value::Bool(true)) {
                    problems.push(format!(
                        "{}.{}: is readOnly and must not be sent",
                        path, key
                    ));
                }
                if direction == "response" && get(&p, "writeOnly") == Some(&Value::Bool(true)) {
                    problems.push(format!(
                        "{}.{}: is writeOnly and is never returned",
                        path, key
                    ));
                }
                problems.extend(conform(
                    item,
                    prop,
                    shapes,
                    &format!("{}.{}", path, key),
                    direction,
                ));
            } else {
                match get(s, "additionalProperties") {
                    Some(Value::Bool(false)) => {
                        problems.push(format!("{}.{}: is not a documented property", path, key))
                    }
                    Some(ap @ Value::Object(_)) => problems.extend(conform(
                        item,
                        ap,
                        shapes,
                        &format!("{}.{}", path, key),
                        direction,
                    )),
                    _ => {}
                }
            }
        }
        for member in get_arr(s, "allOf").into_iter().flatten() {
            problems.extend(conform(value, member, shapes, path, direction));
        }
    }

    problems
}

/// Turn a path template (`/widgets/{id}`) into a matcher for concrete request paths.
pub fn path_matcher(template: &str) -> Regex {
    let mut pattern = String::from("^");
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        pattern.push_str(&regex::escape(&rest[..open]));
        match rest[open..].find('}') {
            Some(close) => {
                pattern.push_str("[^/]+");
                rest = &rest[open + close + 1..];
            }
            None => {
                pattern.push_str(&regex::escape(&rest[open..]));
                rest = "";
            }
        }
    }
    pattern.push_str(&regex::escape(rest));
    pattern.push('$');
    Regex::new(&pattern).unwrap_or_else(|_| Regex::new("^$").unwrap())
}

/// Find the inventory operation a concrete request path targets: of the
/// templates that match it, the most specific (fewest path variables).
/// Equally specific templates are settled by `declared`, the operations the
/// case or stub declares; a tie they do not settle is an error naming the
/// candidates (ADR 0044).
pub fn find_operation<'a>(
    inventory: &'a Value,
    method: &str,
    path: &str,
    declared: &[&str],
) -> Result<Option<&'a Value>, String> {
    let method = method.to_uppercase();
    let vars = |op: &Value| get_str(op, "path").unwrap_or("").matches('{').count();
    let matching: Vec<&Value> = get_arr(inventory, "operations")
        .into_iter()
        .flatten()
        .filter(|op| {
            get_str(op, "method") == Some(method.as_str())
                && path_matcher(get_str(op, "path").unwrap_or("")).is_match(path)
        })
        .collect();
    let fewest = matching.iter().map(|op| vars(op)).min();
    let candidates: Vec<&Value> = matching
        .into_iter()
        .filter(|op| Some(vars(op)) == fewest)
        .collect();
    crate::op_match::disambiguate(candidates, declared)
        .map_err(|e| format!("{} {} {}", method, path, e))
}

/// Find the operation a recorded request targets when the request carries a
/// query: an inventory operation keyed by a literal query string
/// (`/query?q=SELECT+Id+FROM+Account`, ADR 0054) matches when its path
/// template matches and every one of its query pairs appears in the request
/// with the same decoded value. With no such operation, the path alone is
/// matched by `find_operation`, as before.
pub fn find_operation_for_request<'a>(
    inventory: &'a Value,
    method: &str,
    path: &str,
    query: &[(String, String)],
    declared: &[&str],
) -> Result<Option<&'a Value>, String> {
    if !query.is_empty() {
        let method_up = method.to_uppercase();
        let vars = |op: &Value| get_str(op, "path").unwrap_or("").matches('{').count();
        let matching: Vec<&Value> = get_arr(inventory, "operations")
            .into_iter()
            .flatten()
            .filter(|op| get_str(op, "method") == Some(method_up.as_str()))
            .filter(|op| {
                let Some((base, keyed)) = get_str(op, "path").and_then(|p| p.split_once('?'))
                else {
                    return false;
                };
                path_matcher(base).is_match(path)
                    && url::form_urlencoded::parse(keyed.as_bytes())
                        .all(|(k, v)| query.iter().any(|(qk, qv)| *qk == k && *qv == v))
            })
            .collect();
        if !matching.is_empty() {
            let fewest = matching.iter().map(|op| vars(op)).min();
            let candidates: Vec<&Value> = matching
                .into_iter()
                .filter(|op| Some(vars(op)) == fewest)
                .collect();
            return crate::op_match::disambiguate(candidates, declared)
                .map_err(|e| format!("{} {} {}", method_up, path, e));
        }
    }
    find_operation(inventory, method, path, declared)
}

pub struct ShapeTarget {
    pub shape_ref: Option<String>,
    pub documented: bool,
}

/// The status-specific response shape for an operation, or None when the spec has none.
pub fn response_shape_for(op: &Value, status: &str) -> Option<ShapeTarget> {
    if let Some(response) = get(op, "response") {
        let rs = get(response, "status").map(|v| match v {
            Value::String(s) => s.clone(),
            other => compact(other),
        });
        if rs.as_deref() == Some(status)
            || (rs.as_deref() == Some("2XX") && status.starts_with('2'))
        {
            return Some(ShapeTarget {
                shape_ref: get_str(response, "shape_ref").map(str::to_string),
                documented: true,
            });
        }
    }
    let error = get_arr(op, "errors").into_iter().flatten().find(|e| {
        let es = get(e, "status").map(|v| match v {
            Value::String(s) => s.clone(),
            other => compact(other),
        });
        es.as_deref() == Some(status) || es.as_deref() == Some("default")
    });
    if let Some(e) = error {
        return Some(ShapeTarget {
            shape_ref: get_str(e, "shape_ref").map(str::to_string),
            documented: true,
        });
    }
    if status.starts_with('2') {
        if let Some(response) = get(op, "response") {
            return Some(ShapeTarget {
                shape_ref: get_str(response, "shape_ref").map(str::to_string),
                documented: false,
            });
        }
    }
    None
}

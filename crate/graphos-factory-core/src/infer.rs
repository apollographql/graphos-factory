//! Shape inference: recorded samples -> an inventory-dialect shape.
//!
//! The no-spec workspace's conformance oracle. Inference is deliberately
//! conservative: a property is `required` only when every sample carried it
//! and there are at least two samples; a value seen as a type and null is
//! `nullable`; strings are never promoted to enums without a hint; objects
//! are maps only where a hint says so. More samples make the oracle
//! stricter, which is the right direction.

use crate::json::{compact, get, get_arr, get_str, obj, Object};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};

const MAX_OBSERVED: usize = 24;

#[derive(Default, Clone)]
pub struct Hints {
    pub maps: HashSet<String>,
    pub enums: HashSet<String>,
    pub vocabulary: HashMap<String, BTreeSet<String>>,
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() || n.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false) {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Bool(_) => "boolean",
    }
}

fn date_time() -> Regex {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[-+]\d{2}:\d{2})$").unwrap()
}
fn date() -> Regex {
    Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap()
}
fn uri() -> Regex {
    Regex::new(r"^https?://\S+$").unwrap()
}

/// JavaScript default sort: by UTF-16 code units, which for these strings
/// equals byte order.
fn js_sort(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// Infer one shape from observed values at the same position. `None` entries
/// stand for JavaScript `undefined` (the key was absent in that sample).
pub fn infer_shape(values: &[Option<&Value>], hints: &Hints, path: &str) -> Value {
    let present: Vec<&Value> = values.iter().filter_map(|v| *v).collect();
    if present.is_empty() {
        return Value::Object(obj());
    }
    let mut kinds: Vec<&str> = Vec::new();
    for v in &present {
        let k = kind_of(v);
        if !kinds.contains(&k) {
            kinds.push(k);
        }
    }
    let nullable = kinds.contains(&"null");
    kinds.retain(|k| *k != "null");
    let mut shape = obj();

    if kinds.is_empty() {
        shape.insert("nullable".into(), Value::Bool(true));
        shape.insert("x-samples".into(), Value::from(present.len()));
        return Value::Object(shape);
    }
    if kinds.contains(&"integer") && kinds.contains(&"number") {
        kinds.retain(|k| *k != "integer");
    }
    shape.insert(
        "type".into(),
        if kinds.len() == 1 {
            Value::from(kinds[0])
        } else {
            Value::Array(kinds.iter().map(|k| Value::from(*k)).collect())
        },
    );
    if nullable {
        shape.insert("nullable".into(), Value::Bool(true));
    }

    if kinds.contains(&"object") {
        let objects: Vec<&Object> = present.iter().filter_map(|v| v.as_object()).collect();
        if hints.maps.contains(path) {
            let values: Vec<Option<&Value>> =
                objects.iter().flat_map(|o| o.values().map(Some)).collect();
            shape.insert(
                "additionalProperties".into(),
                infer_shape(&values, hints, &format!("{}.*", path)),
            );
            let mut keys: Vec<String> = Vec::new();
            for o in &objects {
                for k in o.keys() {
                    if !keys.contains(k) {
                        keys.push(k.clone());
                    }
                }
            }
            keys.truncate(MAX_OBSERVED);
            shape.insert(
                "x-observed-keys".into(),
                Value::Array(keys.into_iter().map(Value::from).collect()),
            );
        } else {
            let mut keys: Vec<String> = Vec::new();
            for o in &objects {
                for k in o.keys() {
                    if !keys.contains(k) {
                        keys.push(k.clone());
                    }
                }
            }
            let mut props = obj();
            for key in &keys {
                let vals: Vec<Option<&Value>> = objects.iter().map(|o| o.get(key)).collect();
                props.insert(
                    key.clone(),
                    infer_shape(&vals, hints, &format!("{}.{}", path, key)),
                );
            }
            shape.insert("properties".into(), Value::Object(props));
            if objects.len() >= 2 {
                let required: Vec<Value> = keys
                    .iter()
                    .filter(|k| objects.iter().all(|o| o.contains_key(*k)))
                    .map(|k| Value::from(k.as_str()))
                    .collect();
                if !required.is_empty() {
                    shape.insert("required".into(), Value::Array(required));
                }
            }
        }
    }

    if kinds.contains(&"array") {
        let items: Vec<Option<&Value>> = present
            .iter()
            .filter_map(|v| v.as_array())
            .flat_map(|a| a.iter().map(Some))
            .collect();
        shape.insert(
            "items".into(),
            if items.is_empty() {
                Value::Object(obj())
            } else {
                infer_shape(&items, hints, &format!("{}[]", path))
            },
        );
    }

    if kinds.contains(&"string") {
        let strings: Vec<&str> = present.iter().filter_map(|v| v.as_str()).collect();
        let mut distinct: Vec<String> = Vec::new();
        for s in &strings {
            if !distinct.iter().any(|d| d == s) {
                distinct.push(s.to_string());
            }
        }
        let dt = date_time();
        let d = date();
        let u = uri();
        if strings.iter().all(|s| dt.is_match(s)) {
            shape.insert("format".into(), Value::from("date-time"));
        } else if strings.iter().all(|s| d.is_match(s)) {
            shape.insert("format".into(), Value::from("date"));
        } else if strings.iter().all(|s| u.is_match(s)) {
            shape.insert("format".into(), Value::from("uri"));
        }
        if hints.enums.contains(path) {
            let global = hints.vocabulary.get(path);
            let mut all: BTreeSet<String> = global.cloned().unwrap_or_default();
            for s in &distinct {
                all.insert(s.clone());
            }
            shape.insert(
                "enum".into(),
                Value::Array(
                    js_sort(all.into_iter().collect())
                        .into_iter()
                        .map(Value::from)
                        .collect(),
                ),
            );
            if let Some(g) = global {
                if g.len() > distinct.len() {
                    shape.insert(
                        "x-enum-scope".into(),
                        Value::from(format!(
                            "vocabulary from {} distinct values across all samples",
                            g.len()
                        )),
                    );
                }
            }
        } else if !shape.contains_key("format") && distinct.len() <= MAX_OBSERVED {
            shape.insert(
                "x-observed-values".into(),
                Value::Array(js_sort(distinct).into_iter().map(Value::from).collect()),
            );
        }
    }

    shape.insert("x-samples".into(), Value::from(present.len()));
    Value::Object(shape)
}

/// Split a hint path (`$.piles.*.cards[].value`) into segments:
/// `$`, `piles`, `*`, `cards`, `[]`, `value`.
fn segments(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = path.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '.' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else if c == '[' && chars.get(i + 1) == Some(&']') {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            out.push("[]".to_string());
            i += 1;
        } else {
            current.push(c);
        }
        i += 1;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn values_at<'a>(value: Option<&'a Value>, segs: &[String], out: &mut Vec<&'a Value>) {
    let v = match value {
        Some(v) => v,
        None => return,
    };
    if segs.is_empty() {
        out.push(v);
        return;
    }
    let head = segs[0].as_str();
    let rest = &segs[1..];
    match head {
        "$" => values_at(Some(v), rest, out),
        "[]" => {
            if let Value::Array(items) = v {
                for item in items {
                    values_at(Some(item), rest, out);
                }
            }
        }
        "*" => {
            if let Value::Object(o) = v {
                for item in o.values() {
                    values_at(Some(item), rest, out);
                }
            }
        }
        key => {
            if let Value::Object(o) = v {
                values_at(o.get(key), rest, out);
            }
        }
    }
}

/// Every string observed across ALL bodies at the hinted paths. Paths in one
/// group share one vocabulary.
pub fn collect_vocabulary(
    bodies: &[&Value],
    groups: &[Vec<String>],
) -> HashMap<String, BTreeSet<String>> {
    let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
    for group in groups {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for path in group {
            let segs = segments(path);
            for body in bodies {
                let mut found = Vec::new();
                values_at(Some(body), &segs, &mut found);
                for v in found {
                    if let Some(s) = v.as_str() {
                        seen.insert(s.to_string());
                    }
                }
            }
        }
        for path in group {
            out.entry(path.clone())
                .or_default()
                .extend(seen.iter().cloned());
        }
    }
    out
}

fn is_ok(sample: &Value) -> bool {
    let r = match get(sample, "response") {
        Some(r) => r,
        None => return false,
    };
    let status = get(r, "status").and_then(Value::as_i64).unwrap_or(0);
    (200..300).contains(&status) && get(r, "json") != Some(&Value::Bool(false))
}

fn status_string(sample: &Value) -> String {
    get(sample, "response")
        .and_then(|r| get(r, "status"))
        .map(compact)
        .unwrap_or_default()
}

/// Group recorded samples by operation and infer per operation.
pub fn infer_operations(
    samples: &[Value],
    hints_doc: &Value,
    vocabulary: &HashMap<String, BTreeSet<String>>,
) -> Object {
    let mut by_op: Vec<(String, Vec<&Value>)> = Vec::new();
    for s in samples {
        let key = match get_str(s, "operation") {
            Some(k) if get(s, "response").is_some() => k.to_string(),
            _ => continue,
        };
        match by_op.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(s),
            None => by_op.push((key, vec![s])),
        }
    }
    let mut out = obj();
    for (key, list) in by_op {
        let h = hints_for(hints_doc, &key, vocabulary);
        let ok: Vec<&Value> = list.iter().copied().filter(|s| is_ok(s)).collect();
        let errors: Vec<&Value> = list
            .iter()
            .copied()
            .filter(|s| {
                let status = get(s, "response")
                    .and_then(|r| get(r, "status"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                !(200..300).contains(&status)
            })
            .collect();
        let mut entry = obj();
        entry.insert("samples".into(), Value::from(list.len()));
        if !ok.is_empty() {
            let mut statuses: Vec<String> = Vec::new();
            for s in &ok {
                let st = status_string(s);
                if !statuses.contains(&st) {
                    statuses.push(st);
                }
            }
            let statuses = js_sort(statuses);
            let bodies: Vec<Option<&Value>> = ok
                .iter()
                .map(|s| get(s, "response").and_then(|r| crate::json::field(r, "body")))
                .collect();
            entry.insert(
                "response".into(),
                crate::json::object(vec![
                    (
                        "status",
                        if statuses.len() == 1 {
                            Value::from(statuses[0].clone())
                        } else {
                            Value::Array(statuses.iter().map(|s| Value::from(s.as_str())).collect())
                        },
                    ),
                    ("shape", infer_shape(&bodies, &h, "$")),
                ]),
            );
        }
        if !errors.is_empty() {
            let mut statuses: Vec<String> = Vec::new();
            for s in &errors {
                let st = status_string(s);
                if !statuses.contains(&st) {
                    statuses.push(st);
                }
            }
            let mut err_obj = obj();
            for status in js_sort(statuses) {
                let bodies: Vec<Option<&Value>> = errors
                    .iter()
                    .filter(|s| {
                        status_string(s) == status
                            && get(s, "response").and_then(|r| get(r, "json"))
                                != Some(&Value::Bool(false))
                    })
                    .map(|s| get(s, "response").and_then(|r| crate::json::field(r, "body")))
                    .collect();
                err_obj.insert(
                    status,
                    if bodies.is_empty() {
                        crate::json::object(vec![("x-note", Value::from("non-JSON body"))])
                    } else {
                        infer_shape(&bodies, &h, "$")
                    },
                );
            }
            entry.insert("errors".into(), Value::Object(err_obj));
        }
        out.insert(key, Value::Object(entry));
    }
    out
}

/// The hint paths that apply to one operation key (`operation: "*"` or the key).
pub fn hints_for(
    hints_doc: &Value,
    key: &str,
    vocabulary: &HashMap<String, BTreeSet<String>>,
) -> Hints {
    let pick = |list: Option<&Vec<Value>>| -> HashSet<String> {
        let mut out = HashSet::new();
        for entry in list.into_iter().flatten() {
            match entry {
                Value::String(s) => {
                    out.insert(s.clone());
                }
                Value::Object(_) => {
                    let op = get_str(entry, "operation");
                    if op == Some(key) || op == Some("*") {
                        for p in get_arr(entry, "paths")
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                        {
                            out.insert(p.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        out
    };
    Hints {
        maps: pick(get_arr(hints_doc, "maps")),
        enums: pick(get_arr(hints_doc, "enums")),
        vocabulary: vocabulary.clone(),
    }
}

/// Enum hint entries as path groups; each group shares one vocabulary.
pub fn enum_groups(hints_doc: &Value) -> Vec<Vec<String>> {
    get_arr(hints_doc, "enums")
        .into_iter()
        .flatten()
        .map(|e| match e {
            Value::String(s) => vec![s.clone()],
            other => get_arr(other, "paths")
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        })
        .filter(|g: &Vec<String>| !g.is_empty())
        .collect()
}

/// Name for the shape an operation's inventory record should reference.
pub fn shape_name_for(key: &str) -> String {
    let (method, p) = key.split_once(':').unwrap_or((key, ""));
    let parts: Vec<String> = p
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            if seg.starts_with('{') {
                format!(
                    "By{}",
                    crate::inventory::cap(&seg[1..seg.len().saturating_sub(1)])
                )
            } else {
                crate::inventory::cap(seg)
            }
        })
        .collect();
    format!(
        "{}{}Response",
        crate::inventory::cap(method),
        parts.join("")
    )
}

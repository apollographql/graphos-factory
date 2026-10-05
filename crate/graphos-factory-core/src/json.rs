//! Small helpers over `serde_json::Value` that give the port the same
//! semantics JavaScript had: integral floats are integers, `undefined`
//! becomes "absent", objects keep insertion order (serde_json's
//! `preserve_order` feature), and pretty output matches `JSON.stringify(v,
//! null, 2)` byte for byte for the values this tool writes.

use serde_json::{Map, Value};

pub type Object = Map<String, Value>;

/// JavaScript has one number type: `JSON.parse("2.0")` is the integer 2.
/// Apply after every parse so `kindOf`, `Number.isInteger` and output
/// formatting agree with the JavaScript instruments.
pub fn normalize_numbers(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if n.as_i64().is_none()
                    && n.as_u64().is_none()
                    && f.fract() == 0.0
                    && f.abs() < 9.0e15
                {
                    *v = Value::from(f as i64);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_numbers),
        Value::Object(map) => map.values_mut().for_each(normalize_numbers),
        _ => {}
    }
}

pub fn parse(text: &str) -> Result<Value, String> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    normalize_numbers(&mut v);
    Ok(v)
}

/// `JSON.stringify(value, null, 2)` followed by a newline.
pub fn pretty(value: &Value) -> String {
    let mut s = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string());
    s.push('\n');
    s
}

/// `JSON.stringify(value)`.
pub fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

pub fn obj() -> Object {
    Map::new()
}

pub fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object()
        .and_then(|o| o.get(key))
        .filter(|x| !x.is_null())
}

/// Like `get` but keeps an explicit null (JavaScript `key in obj`).
pub fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(key))
}

pub fn get_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    get(v, key).and_then(Value::as_str)
}

pub fn get_bool(v: &Value, key: &str) -> Option<bool> {
    get(v, key).and_then(Value::as_bool)
}

pub fn get_arr<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    get(v, key).and_then(Value::as_array)
}

pub fn get_obj<'a>(v: &'a Value, key: &str) -> Option<&'a Object> {
    get(v, key).and_then(Value::as_object)
}

pub fn is_object(v: &Value) -> bool {
    v.is_object()
}

/// JavaScript `Number.isInteger`.
pub fn is_integer(v: &Value) -> bool {
    match v {
        Value::Number(n) => {
            n.is_i64() || n.is_u64() || n.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false)
        }
        _ => false,
    }
}

/// JavaScript `typeof`/Array.isArray description used in error messages.
pub fn describe(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
    }
}

/// Truthiness as JavaScript sees it, for the few places the port relies on it.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

pub fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Deep structural equality (JavaScript `deepEqual` in lib/validate.mjs).
pub fn deep_equal(a: &Value, b: &Value) -> bool {
    a == b
}

/// Build an object from ordered pairs.
pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

pub fn set(v: &mut Value, key: &str, value: Value) {
    if let Value::Object(m) = v {
        m.insert(key.to_string(), value);
    }
}

pub fn remove(v: &mut Value, key: &str) {
    if let Value::Object(m) = v {
        m.shift_remove(key);
    }
}

/// Canonicalize the deepest ancestor of `path` that exists and re-append the
/// rest. `std::fs::canonicalize` needs the whole path to exist, so a file a
/// dry run has not written yet keeps the symlinked spelling its parent had —
/// on macOS `/var/folders/…` beside a canonicalized `/private/var/folders/…`,
/// which turns every relative path into a chain of `..`.
fn canonicalize_existing_prefix(path: &std::path::Path) -> std::path::PathBuf {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c;
    }
    let mut base = path.to_path_buf();
    while base.pop() {
        if let Ok(root) = std::fs::canonicalize(&base) {
            let mut out = root;
            for part in path.strip_prefix(&base).into_iter().flatten() {
                out.push(part);
            }
            return out;
        }
    }
    path.to_path_buf()
}

/// `path.relative(from, to)` for the simple case used in output lines.
pub fn relative(from: &std::path::Path, to: &std::path::Path) -> String {
    let from = std::fs::canonicalize(from).unwrap_or_else(|_| from.to_path_buf());
    let to_abs = if to.is_absolute() {
        to.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(to))
            .unwrap_or_else(|_| to.to_path_buf())
    };
    let to_abs = canonicalize_existing_prefix(&to_abs);
    match to_abs.strip_prefix(&from) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => {
            // Walk up from `from` until a common prefix.
            let mut ups = Vec::new();
            let mut base = from.clone();
            loop {
                if let Ok(rel) = to_abs.strip_prefix(&base) {
                    let mut parts: Vec<String> = ups.clone();
                    parts.push(rel.to_string_lossy().to_string());
                    return parts.join("/");
                }
                if !base.pop() {
                    return to_abs.to_string_lossy().to_string();
                }
                ups.push("..".to_string());
            }
        }
    }
}

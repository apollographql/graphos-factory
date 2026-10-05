//! JSON Patch (RFC 6902) for pinned sources.
//!
//! A source document (an OpenAPI or Swagger file) is pinned in two copies:
//! the vendor's bytes, never edited, and the working copy the agent and the
//! engineer may change. The difference between the two is the *patch set*
//! recorded in `sources.lock.yaml`, each operation carrying the reason it
//! exists. This module computes that set from two documents (`diff`),
//! replays it (`apply`), and hashes documents by content (`canonical_sha256`)
//! so formatting never counts as an edit.
//!
//! Only `add`, `remove` and `replace` are produced; `apply` also accepts
//! `test` (RFC 6902 §4.6) and ignores the non-RFC keys the lock adds
//! (`reason`, `decision`, `verified`, `was`).

use crate::json::{compact, get_str, obj};
use serde_json::Value;

/// RFC 6901 escaping for one reference token.
pub fn escape_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

pub fn unescape_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

pub(crate) fn tokens(pointer: &str) -> Result<Vec<String>, String> {
    if pointer.is_empty() {
        return Ok(vec![]);
    }
    if !pointer.starts_with('/') {
        return Err(format!("JSON pointer must start with '/': {:?}", pointer));
    }
    Ok(pointer[1..].split('/').map(unescape_token).collect())
}

/// SHA-256 (hex) of the document's compact JSON form: the same content in
/// JSON or YAML, pretty or minified, hashes the same.
pub fn canonical_sha256(doc: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(compact(doc).as_bytes());
    format!("{:x}", h.finalize())
}

/// SHA-256 (hex) of raw bytes — for the upstream copy, which is pinned as
/// the vendor published it.
pub fn bytes_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// One patch operation. `was` is the value a `remove` or `replace` took
/// away — not part of RFC 6902, kept so a refresh can tell "the vendor
/// changed this too" from "the patch still applies".
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub op: &'static str,
    pub path: String,
    pub value: Option<Value>,
    pub was: Option<Value>,
}

impl Op {
    pub fn to_value(&self) -> Value {
        let mut m = obj();
        m.insert("op".into(), Value::from(self.op));
        m.insert("path".into(), Value::from(self.path.as_str()));
        if let Some(v) = &self.value {
            m.insert("value".into(), v.clone());
        }
        if let Some(w) = &self.was {
            m.insert("was".into(), w.clone());
        }
        Value::Object(m)
    }
}

/// The operations that turn `from` into `to`. Objects diff per key, arrays of
/// equal length per index; any other difference is one `replace` of the
/// whole node, so a reordered or resized array is a single operation with
/// the old value kept in `was`.
pub fn diff(from: &Value, to: &Value) -> Vec<Op> {
    let mut out = Vec::new();
    diff_into(from, to, String::new(), &mut out);
    out
}

fn diff_into(from: &Value, to: &Value, path: String, out: &mut Vec<Op>) {
    if from == to {
        return;
    }
    match (from, to) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                match b.get(k) {
                    Some(vb) => diff_into(va, vb, format!("{}/{}", path, escape_token(k)), out),
                    None => out.push(Op {
                        op: "remove",
                        path: format!("{}/{}", path, escape_token(k)),
                        value: None,
                        was: Some(va.clone()),
                    }),
                }
            }
            for (k, vb) in b {
                if !a.contains_key(k) {
                    out.push(Op {
                        op: "add",
                        path: format!("{}/{}", path, escape_token(k)),
                        value: Some(vb.clone()),
                        was: None,
                    });
                }
            }
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
            for (i, (va, vb)) in a.iter().zip(b.iter()).enumerate() {
                diff_into(va, vb, format!("{}/{}", path, i), out);
            }
        }
        _ => out.push(Op {
            op: "replace",
            path,
            value: Some(to.clone()),
            was: Some(from.clone()),
        }),
    }
}

/// Apply a patch set (RFC 6902 objects, extra keys ignored) to a document.
/// Fails naming the first operation that does not apply.
pub fn apply(doc: &Value, patches: &[Value]) -> Result<Value, String> {
    let mut d = doc.clone();
    for (i, p) in patches.iter().enumerate() {
        let op = get_str(p, "op").ok_or_else(|| format!("patch {}: no `op`", i))?;
        let path = get_str(p, "path").ok_or_else(|| format!("patch {}: no `path`", i))?;
        let describe = |what: &str| format!("patch {} ({} {}): {}", i, op, path, what);
        let toks = tokens(path).map_err(|e| describe(&e))?;
        match op {
            "add" => {
                let v = p
                    .as_object()
                    .and_then(|o| o.get("value"))
                    .cloned()
                    .ok_or_else(|| describe("no `value`"))?;
                set_at(&mut d, &toks, v, true).map_err(|e| describe(&e))?;
            }
            "replace" => {
                let v = p
                    .as_object()
                    .and_then(|o| o.get("value"))
                    .cloned()
                    .ok_or_else(|| describe("no `value`"))?;
                if resolve(&d, &toks).is_none() {
                    return Err(describe("the target does not exist"));
                }
                set_at(&mut d, &toks, v, false).map_err(|e| describe(&e))?;
            }
            "remove" => remove_at(&mut d, &toks).map_err(|e| describe(&e))?,
            "test" => {
                let expected = p.as_object().and_then(|o| o.get("value"));
                if resolve(&d, &toks) != expected {
                    return Err(describe("the value is not what `test` expects"));
                }
            }
            other => return Err(describe(&format!("unsupported op {:?}", other))),
        }
    }
    Ok(d)
}

/// The node a pointer names, if any (explicit nulls count as present).
pub fn resolve<'a>(doc: &'a Value, toks: &[String]) -> Option<&'a Value> {
    let mut cur = doc;
    for t in toks {
        cur = match cur {
            Value::Object(o) => o.get(t)?,
            Value::Array(a) => a.get(t.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn set_at(doc: &mut Value, toks: &[String], value: Value, adding: bool) -> Result<(), String> {
    if toks.is_empty() {
        *doc = value;
        return Ok(());
    }
    let (parent_toks, last) = toks.split_at(toks.len() - 1);
    let parent = resolve_mut(doc, parent_toks).ok_or("the parent does not exist")?;
    let key = &last[0];
    match parent {
        Value::Object(o) => {
            o.insert(key.clone(), value);
            Ok(())
        }
        Value::Array(a) => {
            if adding && key == "-" {
                a.push(value);
                return Ok(());
            }
            let i: usize = key
                .parse()
                .map_err(|_| format!("{:?} is not an array index", key))?;
            if adding {
                if i > a.len() {
                    return Err(format!("index {} is past the end of the array", i));
                }
                a.insert(i, value);
            } else {
                *a.get_mut(i).ok_or(format!("index {} is out of range", i))? = value;
            }
            Ok(())
        }
        _ => Err("the parent is a scalar".to_string()),
    }
}

fn remove_at(doc: &mut Value, toks: &[String]) -> Result<(), String> {
    if toks.is_empty() {
        return Err("cannot remove the document root".to_string());
    }
    let (parent_toks, last) = toks.split_at(toks.len() - 1);
    let parent = resolve_mut(doc, parent_toks).ok_or("the parent does not exist")?;
    let key = &last[0];
    match parent {
        Value::Object(o) => o
            .shift_remove(key)
            .map(|_| ())
            .ok_or_else(|| format!("no member {:?}", key)),
        Value::Array(a) => {
            let i: usize = key
                .parse()
                .map_err(|_| format!("{:?} is not an array index", key))?;
            if i >= a.len() {
                return Err(format!("index {} is out of range", i));
            }
            a.remove(i);
            Ok(())
        }
        _ => Err("the parent is a scalar".to_string()),
    }
}

fn resolve_mut<'a>(doc: &'a mut Value, toks: &[String]) -> Option<&'a mut Value> {
    let mut cur = doc;
    for t in toks {
        cur = match cur {
            Value::Object(o) => o.get_mut(t)?,
            Value::Array(a) => a.get_mut(t.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Whether a pointer lies under a prefix (`/paths/~1a` is under `/paths`).
pub fn under(pointer: &str, prefix: &str) -> bool {
    pointer == prefix || pointer.starts_with(&format!("{}/", prefix))
}

/// The value-free description of a patch for reports: `replace /a/b (was 1)`.
pub fn describe(p: &Value) -> String {
    let op = get_str(p, "op").unwrap_or("?");
    let path = get_str(p, "path").unwrap_or("");
    match (op, crate::json::field(p, "was")) {
        ("remove", Some(w)) | ("replace", Some(w)) => {
            format!("{} {} (was {})", op, path, compact(w))
        }
        _ => format!("{} {}", op, path),
    }
}

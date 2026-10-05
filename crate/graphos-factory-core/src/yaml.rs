//! YAML in and out.
//!
//! Reading goes through serde_yaml (anchors, aliases, block scalars, flow
//! collections) and lands in `serde_json::Value` with merge keys applied and
//! integral floats normalized, so every downstream module sees the same
//! values the JavaScript instruments did. A document containing more than
//! one YAML document is refused, never truncated.
//!
//! Writing is a small emitter for the shapes this tool writes
//! (sources.lock.yaml, small manifests): block maps and sequences, quoted
//! only where a plain scalar would reparse as something else. It mirrors the
//! previous emitter so files it rewrites keep their style.

use serde_json::Value;

pub fn parse(text: &str) -> Result<Value, String> {
    let yaml: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("more than one document") {
            "multiple YAML documents are not supported".to_string()
        } else {
            msg
        }
    })?;
    let mut yaml = yaml;
    yaml.apply_merge().map_err(|e| e.to_string())?;
    let mut v = convert(yaml)?;
    crate::json::normalize_numbers(&mut v);
    Ok(v)
}

fn convert(y: serde_yaml::Value) -> Result<Value, String> {
    Ok(match y {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else {
                Value::from(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_yaml::Value::String(s) => Value::String(s),
        serde_yaml::Value::Sequence(items) => {
            Value::Array(items.into_iter().map(convert).collect::<Result<_, _>>()?)
        }
        serde_yaml::Value::Mapping(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    serde_yaml::Value::String(s) => s,
                    serde_yaml::Value::Number(n) => n.to_string(),
                    serde_yaml::Value::Bool(b) => b.to_string(),
                    serde_yaml::Value::Null => "null".to_string(),
                    other => return Err(format!("unsupported mapping key {:?}", other)),
                };
                out.insert(key, convert(v)?);
            }
            Value::Object(out)
        }
        serde_yaml::Value::Tagged(t) => convert(t.value)?,
    })
}

pub fn parse_file(path: &std::path::Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    parse(&text)
}

fn plain_safe(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/'))
}

fn needs_quote(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let lower = text.to_ascii_lowercase();
    if plain_safe(text)
        && !matches!(
            lower.as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
        )
    {
        return false;
    }
    true
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// One string as a YAML scalar: plain when YAML reads it back unchanged,
/// double-quoted otherwise. For writers that emit YAML text line by line.
pub fn scalar(text: &str) -> String {
    if text.contains('\n') || needs_quote(text) {
        quote(text)
    } else {
        text.to_string()
    }
}

fn write_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if s.contains('\n') || needs_quote(s) {
                quote(s)
            } else {
                s.clone()
            }
        }
        other => crate::json::compact(other),
    }
}

fn non_empty(v: &Value) -> bool {
    match v {
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => false,
    }
}

/// Emit block YAML for maps, arrays and scalars; ends with a newline.
pub fn stringify(value: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match value {
        Value::Array(items) => {
            if items.is_empty() {
                return "[]\n".to_string();
            }
            items
                .iter()
                .map(|item| {
                    if item.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                        let body = stringify(item, indent + 2);
                        // "-" replaces the first character of the indented body.
                        format!("{}-{}", pad, &body[indent + 1..])
                    } else {
                        format!("{}- {}", pad, stringify(item, indent + 2).trim_start())
                    }
                })
                .collect()
        }
        Value::Object(map) => {
            if map.is_empty() {
                return "{}\n".to_string();
            }
            map.iter()
                .map(|(key, item)| {
                    let k = if needs_quote(key) {
                        quote(key)
                    } else {
                        key.clone()
                    };
                    if non_empty(item) {
                        format!("{}{}:\n{}", pad, k, stringify(item, indent + 2))
                    } else {
                        format!("{}{}: {}", pad, k, stringify(item, indent + 2).trim_start())
                    }
                })
                .collect()
        }
        other => format!("{}\n", write_scalar(other)),
    }
}

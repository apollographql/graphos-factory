//! The response's factual signals, and the envelope the tool *suggests* from
//! them.
//!
//! An envelope — "the single root property the useful content sits under" —
//! is a **judgement**: it decides whether the schema exposes
//! `Widget_Co_WidgetList` or a bare `[Widget_Co_Widget]`, and no amount of
//! spec reading settles it. Judgements live in `selection.yaml`
//! (`operations.<key>.response.envelope`), never in `inventory.json`
//! (ADR 0018).
//!
//! What the inventory records instead are the *facts* the judgement rests
//! on, each checkable against the description document in seconds:
//! whether the success shape is itself an array, how many root properties it
//! has, which of them are arrays, which are hypermedia links, which names a
//! total, and which could carry a next cursor. `suggest_envelope` reads only
//! those facts, so the tool's proposal is reproducible and the agent can see
//! why it was made.

use crate::json::{get, get_arr, get_obj, get_str, Object};
use serde_json::Value;
use std::collections::HashSet;

/// Root properties that are hypermedia, never payload. A HAL-ish `_links`
/// array is the commonest reason an object looks like it has "one array
/// property" when it has none worth flattening to.
pub const LINK_PROPERTIES: [&str; 2] = ["_links", "links"];

/// Root properties that name how many items the whole collection has. Their
/// presence is what distinguishes a list wrapper from a resource that
/// happens to embed a list.
pub const TOTAL_PROPERTIES: [&str; 6] = [
    "total_items",
    "total_count",
    "total",
    "count",
    "totalCount",
    "total_results",
];

fn is_link(name: &str) -> bool {
    LINK_PROPERTIES.contains(&name)
}

fn deref<'a>(
    shape: Option<&'a Value>,
    shapes: &'a Object,
    seen: &mut HashSet<String>,
) -> Option<Value> {
    let s = shape?;
    match get_str(s, "$ref") {
        Some(r) => {
            let name = r.rsplit('/').next().unwrap_or(r).to_string();
            if !seen.insert(name.clone()) {
                return None;
            }
            deref(shapes.get(&name), shapes, seen)
        }
        None => Some(s.clone()),
    }
}

fn is_array(shape: Option<&Value>) -> bool {
    shape.and_then(|s| get_str(s, "type")) == Some("array")
}

/// Can this property carry a string? The same test the pagination detector
/// uses, so a HAL `next: { href }` link or a page object is not a cursor.
fn stringish(shape: Option<&Value>, shapes: &Object) -> bool {
    let s = match deref(shape, shapes, &mut HashSet::new()) {
        Some(s) => s,
        None => return false,
    };
    match get(&s, "type") {
        Some(Value::String(t)) if t == "string" => return true,
        Some(Value::Array(ts)) if ts.iter().any(|t| t.as_str() == Some("string")) => return true,
        // An untyped property is a string as far as a cursor is concerned;
        // one with `properties` or `items` is an object or a list.
        None if get(&s, "properties").is_none() && get(&s, "items").is_none() => return true,
        _ => {}
    }
    for key in ["anyOf", "oneOf"] {
        for branch in get_arr(&s, key).into_iter().flatten() {
            if stringish(Some(branch), shapes) {
                return true;
            }
        }
    }
    false
}

/// The factual signals of one success response shape, as the keys
/// `response_facts` splices into an inventory operation's `response` record.
/// Every field is omitted when it carries no information (false, zero,
/// empty, absent), so a response with nothing to say adds nothing.
pub fn response_facts(shape: Option<&Value>, shapes: &Object) -> Vec<(&'static str, Value)> {
    let resolved = deref(shape, shapes, &mut HashSet::new());
    let mut out: Vec<(&'static str, Value)> = Vec::new();
    if is_array(resolved.as_ref()) {
        out.push(("root_is_array", Value::Bool(true)));
        return out;
    }
    let props = match resolved.as_ref().and_then(|r| get_obj(r, "properties")) {
        Some(p) => p,
        None => return out,
    };
    let names: Vec<&String> = props.keys().collect();
    let links: Vec<&str> = names
        .iter()
        .filter(|n| is_link(n))
        .map(|n| n.as_str())
        .collect();
    let arrays: Vec<&str> = names
        .iter()
        .filter(|n| is_array(deref(props.get(n.as_str()), shapes, &mut HashSet::new()).as_ref()))
        .map(|n| n.as_str())
        .collect();
    let content: Vec<&str> = names
        .iter()
        .filter(|n| !is_link(n))
        .map(|n| n.as_str())
        .collect();
    let total = names
        .iter()
        .find(|n| TOTAL_PROPERTIES.contains(&n.as_str()))
        .map(|n| n.as_str());
    let cursors: Vec<&str> = names
        .iter()
        .filter(|n| {
            crate::openapi::NEXT_KEYS.contains(&n.as_str())
                && stringish(props.get(n.as_str()), shapes)
        })
        .map(|n| n.as_str())
        .collect();

    out.push(("root_property_count", Value::from(names.len())));
    if !links.is_empty() {
        out.push(("link_root_properties", strs(&links)));
    }
    if !arrays.is_empty() {
        out.push(("array_root_properties", strs(&arrays)));
    }
    if content.len() == 1 {
        out.push(("sole_root_property", Value::from(content[0])));
    }
    if let Some(t) = total {
        out.push(("total_items_property", Value::from(t)));
    }
    if !cursors.is_empty() {
        out.push(("cursor_root_properties", strs(&cursors)));
    }
    out
}

fn strs(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(*s)).collect())
}

/// The envelope the tool proposes for a response, from the facts alone.
///
/// Three ways a root object earns an envelope, and no others:
///
/// 1. it has exactly one property that is not a hypermedia link — the
///    content sits under that key whatever its type (`{ "version": "1.20" }`);
/// 2. exactly one of its non-link properties is an array **and** a sibling
///    names a total or carries a next cursor — the mark of a list wrapper;
/// 3. exactly one of its non-link properties is an array and there is
///    exactly one other non-link property — a list beside a status flag
///    (`{ "ok": true, "data": [...] }`).
///
/// Anything else is a resource that happens to embed a list, and flattening
/// to that list would drop the rest of the payload. This is the rule the
/// pre-ADR-0018 detector got wrong: it took the sole array property however
/// many siblings it had, so a Mailchimp audience with 22 root properties
/// reported `envelope: "modules"`.
///
/// The result is a *suggestion*. `selection draft` writes it into
/// `selection.yaml` with `confirmed: false`; the agent confirms it with the
/// user (SKILL.md, `select`).
pub fn suggest_envelope(response: Option<&Value>) -> Option<String> {
    let r = response?;
    if get(r, "root_is_array") == Some(&Value::Bool(true)) {
        return None;
    }
    let count = get(r, "root_property_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let links = get_arr(r, "link_root_properties")
        .map(|a| a.len())
        .unwrap_or(0);
    let content = count.saturating_sub(links);
    if content == 1 {
        return get_str(r, "sole_root_property").map(str::to_string);
    }
    let arrays: Vec<&str> = get_arr(r, "array_root_properties")
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|n| !is_link(n))
        .collect();
    if arrays.len() != 1 {
        return None;
    }
    let paged = get_str(r, "total_items_property").is_some()
        || get_arr(r, "cursor_root_properties")
            .map(|a| !a.is_empty())
            .unwrap_or(false);
    if paged || content == 2 {
        return Some(arrays[0].to_string());
    }
    None
}

/// Does the payload the selection reads come back as a list? True for a bare
/// array response, and for an envelope that names an array property. Derived
/// from the facts and the chosen envelope rather than recorded, so it can
/// never disagree with either.
pub fn is_list(response: Option<&Value>, envelope: Option<&str>) -> bool {
    let r = match response {
        Some(r) => r,
        None => return false,
    };
    if get(r, "root_is_array") == Some(&Value::Bool(true)) {
        return true;
    }
    match envelope {
        Some(e) => get_arr(r, "array_root_properties")
            .into_iter()
            .flatten()
            .any(|a| a.as_str() == Some(e)),
        None => false,
    }
}

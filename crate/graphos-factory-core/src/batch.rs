//! batch — which keyed types a bulk-by-keys endpoint could resolve.
//!
//! A type-level `$batch` connector resolves many entities in one request,
//! so the router does not make one call per key (the N+1 that
//! connectors-language.md § Entities warns about). It needs an operation
//! that takes a *list of keys* and answers with an array of the type,
//! each item carrying the key so the router can match it back.
//!
//! `find` reads only facts — the inventory's operations, parameters and
//! shapes — plus the selection's judgement of which types are keyed
//! (`graphql.entity: true`, `graphql.key`). It decides nothing: per type it
//! reports every operation that returns an array of the type, whether that
//! operation accepts the type's keys as a list, and how the list is passed.
//!
//! Verdicts:
//! - `batchable` — at least one read returns an array of the type (or of
//!   another shape of the same entity that carries the key) and accepts a
//!   list of the key — an array parameter, a comma-separated one, or an
//!   array body property whose name is the key's (`id`, `ids`,
//!   `<type>Id(s)`) — with no problem.
//! - `partial-shape`, `needs-scope`, `style-conflict`, `paginated` — every
//!   key-list lookup has a problem; the verdict names the first problem of
//!   the candidate with the fewest (see [`Problem`]).
//! - `list-no-key-filter` — operations return an array of the type, but
//!   none takes a list of its key.
//! - `none` — nothing returns an array of the type (or the key is compound).

use crate::json::{get, get_str, Object};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Batchable,
    /// The only key-list lookups answer with a shape that lacks some of the
    /// type's fields: a batch would resolve partial records.
    PartialShape,
    /// The only key-list lookups need another parameter a type-level
    /// connector cannot supply (a path segment, a required filter).
    NeedsScope,
    /// The only key-list lookups declare one serialization and describe
    /// another (an exploded array documented as comma-separated).
    StyleConflict,
    /// The only key-list lookups are paginated: a batch bigger than one
    /// page would silently drop the entities past it.
    Paginated,
    ListNoKeyFilter,
    None,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Batchable => "batchable",
            Verdict::PartialShape => "partial-shape",
            Verdict::NeedsScope => "needs-scope",
            Verdict::StyleConflict => "style-conflict",
            Verdict::Paginated => "paginated",
            Verdict::ListNoKeyFilter => "list-no-key-filter",
            Verdict::None => "none",
        }
    }

    fn of_problem(kind: &str) -> Verdict {
        match kind {
            "partial-shape" => Verdict::PartialShape,
            "needs-scope" => Verdict::NeedsScope,
            "paginated" => Verdict::Paginated,
            _ => Verdict::StyleConflict,
        }
    }
}

/// Why a key-list candidate cannot back a `$batch` connector as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// `partial-shape`, `needs-scope`, `style-conflict` or `paginated`.
    pub kind: String,
    pub detail: String,
}

/// How an operation accepts a list of keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParam {
    /// The parameter's wire name, or the body property's name.
    pub name: String,
    /// `query`, `path`, `header` or `body`.
    pub location: String,
    /// `repeated` (`?id=1&id=2`), `comma-separated` (`?ids=1,2`),
    /// `space-delimited` (`?ids=1%202`), `pipe-delimited` (`?ids=1|2`) or
    /// `array` (a JSON array in the body).
    pub passing: String,
    /// A maximum list size the source documents, when it states one.
    pub max_size: Option<u64>,
    /// Other parameters the operation requires, which a type-level
    /// connector has no value for (`$batch` carries only the keys).
    pub companions: Vec<String>,
    /// The declared serialization and the description disagree.
    pub conflict: Option<String>,
}

/// One operation that returns an array of the type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub operation: String,
    /// The shape of the array's items: the type itself or a shape named
    /// after it that carries the key (`ListingSummary` for `Listing`).
    pub item_shape: String,
    /// `root` when the response is the array, else `envelope:<path>`.
    pub via: String,
    pub key_param: Option<KeyParam>,
    /// Why this key-list candidate cannot back a `$batch` connector; empty
    /// when it can. Always empty without a key list.
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeReport {
    /// The inventory shape name.
    pub type_name: String,
    /// The key field the lookup matches on.
    pub key: String,
    /// The selection operation that keys the type, or None when the type
    /// was found only because of `--all-types`.
    pub keyed_by: Option<String>,
    pub verdict: Verdict,
    pub candidates: Vec<Candidate>,
    /// Whether the schema declares a type-level `@connect` using `$batch`
    /// on this type.
    pub has_batch_connector: bool,
    /// Why no verdict could be reached beyond `none`, when there is a reason.
    pub note: Option<String>,
    /// The selection's `graphql.batch` judgement: true to batch, false a
    /// recorded decline, None not yet decided.
    pub batch: Option<bool>,
    /// A paste-ready type-level `$batch` connector against the first
    /// candidate that takes a key list, when one can be drafted honestly.
    pub draft: Option<String>,
    /// Why no draft was written for a batchable type.
    pub draft_note: Option<String>,
    /// The schema's type for this shape, when the schema defines one.
    pub sdl_type: Option<String>,
}

fn shapes(inventory: &Value) -> Object {
    get(inventory, "shapes")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn ref_name(v: &Value) -> Option<&str> {
    get_str(v, "$ref").map(crate::inventory::shape_name)
}

/// Follow `$ref`s to a shape object, at most a few hops.
fn deref<'a>(mut v: &'a Value, shapes: &'a Object) -> &'a Value {
    for _ in 0..8 {
        match ref_name(v).and_then(|n| shapes.get(n)) {
            Some(next) => v = next,
            None => break,
        }
    }
    v
}

/// The object branches of a shape: itself, or each `oneOf`/`anyOf`/`allOf`
/// member, dereferenced. Each shape is expanded once: an inventory records
/// OpenAPI polymorphism as a cycle (`PetOut: oneOf [CatOut]`, `CatOut: allOf
/// [PetOut, …]`), and a member already being expanded contributes nothing.
fn branches<'a>(v: &'a Value, shapes: &'a Object) -> Vec<&'a Value> {
    let mut seen = BTreeSet::new();
    let mut out = branches_seen(v, shapes, &mut seen);
    if out.is_empty() {
        out.push(deref(v, shapes));
    }
    out
}

fn branches_seen<'a>(
    v: &'a Value,
    shapes: &'a Object,
    seen: &mut BTreeSet<*const Value>,
) -> Vec<&'a Value> {
    let v = deref(v, shapes);
    if !seen.insert(v as *const Value) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for combinator in ["oneOf", "anyOf", "allOf"] {
        if let Some(members) = get(v, combinator).and_then(Value::as_array) {
            for m in members {
                out.extend(branches_seen(m, shapes, seen));
            }
        }
    }
    if out.is_empty() {
        out.push(v);
    }
    out
}

fn is_array(v: &Value) -> bool {
    match get(v, "type") {
        Some(Value::String(t)) => t == "array",
        Some(Value::Array(ts)) => ts.iter().any(|t| t.as_str() == Some("array")),
        _ => false,
    }
}

/// The item shape name of an array whose items are a `$ref`; None for an
/// array of scalars or of arrays.
fn array_item_ref(v: &Value, shapes: &Object) -> Option<String> {
    let v = deref(v, shapes);
    if !is_array(v) {
        return None;
    }
    get(v, "items").and_then(ref_name).map(str::to_string)
}

/// What an array holds: a named shape, or an object declared inline.
#[derive(Debug, Clone)]
enum Item {
    Named(String),
    Inline(Value),
}

impl Item {
    fn label(&self) -> String {
        match self {
            Item::Named(n) => n.clone(),
            Item::Inline(_) => INLINE.to_string(),
        }
    }
}

/// `item_shape` of a candidate whose array items are declared inline.
pub const INLINE: &str = "(inline)";

fn array_item(v: &Value, shapes: &Object) -> Option<Item> {
    if let Some(n) = array_item_ref(v, shapes) {
        return Some(Item::Named(n));
    }
    let v = deref(v, shapes);
    if !is_array(v) {
        return None;
    }
    get(v, "items")
        .filter(|i| get(i, "properties").is_some())
        .map(|i| Item::Inline(i.clone()))
}

fn property<'a>(v: &'a Value, name: &str, shapes: &'a Object) -> Option<&'a Value> {
    branches(v, shapes)
        .into_iter()
        .find_map(|b| get(b, "properties").and_then(|p| get(p, name)))
}

/// `a.b.c` under a shape.
fn at_path<'a>(v: &'a Value, path: &str, shapes: &'a Object) -> Option<&'a Value> {
    let mut cur = v;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = property(cur, seg, shapes)?;
    }
    Some(cur)
}

/// Every `(item shape, via)` an operation's success response is an array of:
/// the root itself, the selection's envelope, or the response facts'
/// `sole_root_property` / single `array_root_properties`.
fn response_items(op: &Value, envelope: Option<&str>, shapes: &Object) -> Vec<(Item, String)> {
    let response = match get(op, "response") {
        Some(r) if !r.is_null() => r,
        _ => return Vec::new(),
    };
    let root = match get_str(response, "shape_ref") {
        Some(r) => serde_json::json!({ "$ref": r }),
        None => return Vec::new(),
    };
    if let Some(item) = array_item(&root, shapes) {
        return vec![(item, "root".to_string())];
    }
    let mut paths: Vec<String> = Vec::new();
    if let Some(e) = envelope {
        paths.push(e.to_string());
    } else if let Some(p) = get_str(response, "sole_root_property") {
        paths.push(p.to_string());
    } else if let Some(arr) = get(response, "array_root_properties").and_then(Value::as_array) {
        if arr.len() == 1 {
            paths.extend(arr.iter().filter_map(Value::as_str).map(str::to_string));
        }
    }
    if paths.is_empty() {
        if let Some((prop, item)) = wrapper_array(&root, shapes) {
            return vec![(item, format!("inferred:{}", prop))];
        }
    }
    paths
        .into_iter()
        .filter_map(|p| {
            let v = at_path(&root, &p, shapes)?;
            array_item(v, shapes).map(|item| (item, format!("envelope:{}", p)))
        })
        .collect()
}

/// The properties of the non-error branches of a response that is a
/// `oneOf`/`anyOf` of success and error bodies (an envelope API's
/// `{success, results}` beside `ErrorResponse`); empty for any other response.
fn wrapper_properties<'a>(root: &'a Value, shapes: &'a Object) -> Vec<(&'a String, &'a Value)> {
    let top = deref(root, shapes);
    let mut out = Vec::new();
    for member in ["oneOf", "anyOf"]
        .iter()
        .filter_map(|c| get(top, c).and_then(Value::as_array))
        .flatten()
    {
        if ref_name(member).is_some_and(|n| n.contains("Error")) {
            continue;
        }
        for b in branches(member, shapes) {
            if let Some(props) = get(b, "properties").and_then(Value::as_object) {
                out.extend(props.iter());
            }
        }
    }
    out
}

/// Such a wrapper with no recorded envelope: its one property holding an
/// array, and what the array holds, when there is exactly one.
fn wrapper_array(root: &Value, shapes: &Object) -> Option<(String, Item)> {
    let found: Vec<(String, Item)> = wrapper_properties(root, shapes)
        .into_iter()
        .filter_map(|(name, prop)| array_item(prop, shapes).map(|i| (name.clone(), i)))
        .collect();
    if found.len() == 1 {
        found.into_iter().next()
    } else {
        None
    }
}

/// Such a wrapper: its one property holding a `$ref`'d object shape (the
/// entity a by-key lookup returns), when there is exactly one.
fn wrapper_object(root: &Value, shapes: &Object) -> Option<String> {
    let mut found: Vec<String> = wrapper_properties(root, shapes)
        .into_iter()
        .filter_map(|(_, prop)| {
            ref_name(prop)
                .filter(|n| shapes.contains_key(*n) && !is_array(deref(prop, shapes)))
                .map(str::to_string)
        })
        .collect();
    found.dedup();
    if found.len() == 1 {
        found.pop()
    } else {
        None
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Wire names that name a list of `key` for `type_name`: `id`, `ids`,
/// `amenityId`, `amenity_ids`, `ids[]`.
fn names_the_key(param: &str, key: &str, type_name: &str) -> bool {
    let p = norm(param);
    let k = norm(key);
    let t = norm(type_name);
    let mut accepted = vec![k.clone(), format!("{}s", k)];
    if !k.starts_with(&t) {
        accepted.push(format!("{}{}", t, k));
        accepted.push(format!("{}{}s", t, k));
    }
    accepted.contains(&p)
}

fn says_comma_separated(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    [
        "comma-separated",
        "comma separated",
        "comma-delimited",
        "comma delimited",
        "separated by commas",
        "separated by a comma",
    ]
    .iter()
    .any(|p| t.contains(p))
}

/// A maximum list size stated in the key parameter's own description: "max
/// 50", "up to 1,000 ids", "at most 25", "maximum of 200", "no more than
/// 10", "limit of 50". A number followed within the sentence by "page" is a
/// page size ("up to 100 results per page"), not a list size, and is
/// skipped. The operation's description is never read: its maxima are
/// about the response.
fn documented_max(description: &str) -> Option<u64> {
    let re = regex::Regex::new(
        r"(?i)\b(?:max(?:imum)?(?:\s+of)?|up\s+to|at\s+most|no\s+more\s+than|limit(?:ed)?\s+(?:of|to))\s+(\d{1,3}(?:,\d{3})+|\d{1,7})\b",
    )
    .expect("static regex");
    let found = re.captures_iter(description).find_map(|c| {
        let number = c.get(1)?;
        let tail = &description[number.end()..];
        let sentence = tail.split(['.', ';', '\n']).next().unwrap_or("");
        if sentence.to_ascii_lowercase().contains("page") {
            return None;
        }
        number.as_str().replace(',', "").parse().ok()
    });
    found
}

fn says_required(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    !t.contains("not required")
        && [
            "must specify",
            "must be specified",
            "must be provided",
            "is required",
            "are required",
        ]
        .iter()
        .any(|p| t.contains(p))
}

/// Every parameter of `op` other than the key list (`key_param`) that the
/// operation requires, by its `required` flag or its description ("Must
/// specify either advertiserId or floodlightConfigurationId"), plus
/// `extra` (required body properties). A type-level connector has values
/// for none of them.
fn companions(op: &Value, key_param: Option<&str>, extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in get(op, "parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = match get_str(p, "name") {
            Some(n) if Some(n) != key_param => n,
            _ => continue,
        };
        let location = get_str(p, "in").unwrap_or("query");
        if get(p, "required").and_then(Value::as_bool) == Some(true) {
            out.push(format!("{} ({})", name, location));
        } else if says_required(get_str(p, "description").unwrap_or("")) {
            out.push(format!("{} ({}, documented as required)", name, location));
        }
    }
    out.extend(extra.iter().cloned());
    out
}

/// The property names a shape declares, across its branches.
fn property_names(shape: &Value, shapes: &Object) -> BTreeSet<String> {
    branches(shape, shapes)
        .into_iter()
        .filter_map(|b| get(b, "properties").and_then(Value::as_object))
        .flat_map(|p| p.keys().cloned())
        .collect()
}

/// Whether and how `op` accepts a list of `key` values for `type_name`.
fn key_list_param(op: &Value, key: &str, type_name: &str, shapes: &Object) -> Option<KeyParam> {
    let params = get(op, "parameters").and_then(Value::as_array);
    for p in params.into_iter().flatten() {
        let name = match get_str(p, "name") {
            Some(n) => n,
            None => continue,
        };
        if !names_the_key(name, key, type_name) {
            continue;
        }
        let location = get_str(p, "in").unwrap_or("query").to_string();
        let description = get_str(p, "description").unwrap_or("");
        let mut conflict = None;
        let passing = if is_array(p) {
            let style = get_str(p, "style");
            let explode = get(p, "explode").and_then(Value::as_bool);
            let declared = match location.as_str() {
                "query" | "cookie" => {
                    // OpenAPI's default query style is form with explode true.
                    match style.unwrap_or("form") {
                        "form" if explode == Some(false) => "comma-separated",
                        "form" => "repeated",
                        "spaceDelimited" => "space-delimited",
                        "pipeDelimited" => "pipe-delimited",
                        // deepObject serialises an object, not a list of keys.
                        _ => continue,
                    }
                }
                // path and header default to `simple`: comma-separated.
                _ => "comma-separated",
            };
            // The prose is read even when the type is an array: a spec that
            // leaves explode at its default and describes a comma list
            // (Confluence `/pages?id=`) cannot be trusted either way.
            if declared == "repeated" && says_comma_separated(description) {
                conflict = Some(format!(
                    "`{}` is an array with {} (repeated, `?{}=1&{}=2`), but its description says comma-separated",
                    name,
                    if explode.is_some() || style.is_some() { "form style" } else { "the default form style" },
                    name,
                    name
                ));
            }
            declared
        } else if says_comma_separated(description) {
            "comma-separated"
        } else {
            continue;
        };
        return Some(KeyParam {
            name: name.to_string(),
            location,
            passing: passing.to_string(),
            // A parameter's `maxItems` is not an inventory fact (ADR 0068),
            // so only its own prose can state a maximum.
            max_size: documented_max(description),
            companions: companions(op, Some(name), &[]),
            conflict,
        });
    }
    let body = get(op, "request_body").filter(|b| !b.is_null())?;
    let body_shape = serde_json::json!({ "$ref": get_str(body, "shape_ref")? });
    for b in branches(&body_shape, shapes) {
        let props = match get(b, "properties").and_then(Value::as_object) {
            Some(p) => p,
            None => continue,
        };
        for (name, prop) in props {
            if !names_the_key(name, key, type_name) {
                continue;
            }
            let prop = deref(prop, shapes);
            if !is_array(prop) {
                continue;
            }
            let description = get_str(prop, "description").unwrap_or("");
            let max = get(prop, "maxItems")
                .and_then(Value::as_u64)
                .or_else(|| documented_max(description));
            let required: Vec<String> = get(b, "required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|r| *r != name.as_str())
                .map(|r| format!("{} (body)", r))
                .collect();
            return Some(KeyParam {
                name: name.clone(),
                location: "body".to_string(),
                passing: "array".to_string(),
                max_size: max,
                companions: companions(op, None, &required),
                conflict: None,
            });
        }
    }
    None
}

/// A shape name without the representation suffixes vendors put on one
/// entity's several shapes: `TaskResponse`, `TaskCompact` -> `Task`;
/// `ListingSummary` -> `Listing`; `CandidateInfoResponse` -> `Candidate`.
/// Every suffix is a representation word, never a noun for another entity:
/// `Item` is not one (`OrderItem` is a line item of an `Order`), which the
/// first cut had and review removed.
pub fn stem(name: &str) -> &str {
    const SUFFIXES: [&str; 24] = [
        "Response",
        "Result",
        "Results",
        "Success",
        "Compact",
        "Summary",
        "Bare",
        "Full",
        "List",
        "Details",
        "Detail",
        "Info",
        "Base",
        "Record",
        "Object",
        "Data",
        "Dto",
        "Model",
        "Resource",
        "Representation",
        "Minimal",
        "Short",
        "Brief",
        "Expanded",
    ];
    let mut s = name;
    loop {
        match SUFFIXES
            .iter()
            .find(|suffix| s.len() > suffix.len() && s.ends_with(*suffix))
        {
            Some(suffix) => s = &s[..s.len() - suffix.len()],
            None => return s,
        }
    }
}

/// Whether `shape` carries a reference to `type_name` other than `key`: a
/// property `<type>Id`, `<type>_id`, `<type>Uuid` or `<type>Key` (an
/// `OrderDetail` with `orderId` is a child of `Order`, not `Order`).
fn references_type(shape: &Value, type_name: &str, key: &str, shapes: &Object) -> bool {
    let t = norm(stem(type_name));
    let k = norm(key);
    property_names(shape, shapes).iter().any(|p| {
        let p = norm(p);
        p != k
            && ["id", "ids", "uuid", "key"]
                .iter()
                .any(|s| p == format!("{}{}", t, s))
    })
}

/// Whether `item` is `type_name`, or another shape of the same entity (the
/// same `stem`) that carries `key` and no reference to the type, so each
/// item can be matched back to a requested key. An inline item object has
/// no name: it counts when it carries `key`, does not reference the type,
/// and the operation's path names the entity (`/interviewerPool.list` for
/// `InterviewerPool`).
fn same_entity(item: &Item, op_path: &str, type_name: &str, key: &str, shapes: &Object) -> bool {
    match item {
        Item::Named(name) => {
            name == type_name
                || (stem(name) == stem(type_name)
                    && shapes.get(name).is_some_and(|s| {
                        property(s, key, shapes).is_some()
                            && !references_type(s, type_name, key, shapes)
                    }))
        }
        Item::Inline(v) => {
            property(v, key, shapes).is_some()
                && !references_type(v, type_name, key, shapes)
                && norm(op_path).contains(&norm(stem(type_name)))
        }
    }
}

/// Whether an operation only reads: a GET or HEAD, one the inventory
/// classifies as a read, or one the selection confirmed as a read by giving
/// it `graphql.root: query` (the `[read?]` POST the user confirmed). A POST
/// that the source classifies otherwise, a lookup that deletes included, is
/// never a batch candidate.
fn is_read(op: &Value, selection: Option<&Value>) -> bool {
    let method = get_str(op, "method").unwrap_or("").to_ascii_uppercase();
    if matches!(method.as_str(), "GET" | "HEAD") {
        return true;
    }
    get_str(op, "semantics") == Some("read")
        || selection
            .and_then(|s| get(s, "graphql"))
            .and_then(|g| get_str(g, "root"))
            == Some("query")
}

/// The type's fields `item` does not declare, sorted, or None when it
/// declares them all (the type itself always does). A batch lookup must
/// answer with the full record, or the router fills the entity partly.
fn missing_fields(item: &Item, type_name: &str, shapes: &Object) -> Option<Vec<String>> {
    let theirs = match item {
        Item::Named(n) if n == type_name => return None,
        Item::Named(n) => shapes.get(n).map(|s| property_names(s, shapes))?,
        Item::Inline(v) => property_names(v, shapes),
    };
    let ours = property_names(shapes.get(type_name)?, shapes);
    let missing: Vec<String> = ours.difference(&theirs).cloned().collect();
    if missing.is_empty() {
        None
    } else {
        Some(missing)
    }
}

/// The operation's own `pagination` fact, as `style page, size param
/// limit`, or None when the inventory records none (or style `none`).
/// `unknown` — a size parameter and no paging style — still caps the
/// answer, so it counts. The API-level `api.pagination` is a summary of the
/// operations' and is not read.
fn paginated(op: &Value) -> Option<String> {
    let p =
        get(op, "pagination").filter(|p| p.is_object() && get_str(p, "style") != Some("none"))?;
    let mut parts = vec![format!(
        "style {}",
        get_str(p, "style").unwrap_or("unknown")
    )];
    if let Some(size) = get_str(p, "size_param") {
        parts.push(format!("size param {}", size));
    }
    Some(parts.join(", "))
}

/// The selection's per-operation judgement, or Null.
fn selection_op<'a>(selection: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    selection
        .and_then(|s| get(s, "operations"))
        .and_then(|o| get(o, key))
}

fn envelope_of<'a>(selection: Option<&'a Value>, op_key: &str) -> Option<&'a str> {
    selection_op(selection, op_key)
        .and_then(|o| get(o, "response"))
        .and_then(|r| get_str(r, "envelope"))
}

/// The shape an operation's response resolves to once the selection's
/// envelope is unwrapped: the keyed type of an entity operation.
fn result_shape(op: &Value, envelope: Option<&str>, shapes: &Object) -> Option<String> {
    let reference = get(op, "response").and_then(|r| get_str(r, "shape_ref"))?;
    let root = serde_json::json!({ "$ref": reference });
    match envelope {
        None => Some(
            wrapper_object(&root, shapes)
                .unwrap_or_else(|| crate::inventory::shape_name(reference).to_string()),
        ),
        Some(path) => at_path(&root, path, shapes)
            .and_then(ref_name)
            .map(str::to_string),
    }
}

/// Whether a directive argument's value mentions `$batch` in any string it
/// holds (`http: { GET: "/w", queryParams: "ids: $batch.id" }`).
fn mentions_batch(value: &apollo_compiler::ast::Value) -> bool {
    use apollo_compiler::ast::Value as V;
    match value {
        V::String(s) => s.contains("$batch"),
        V::List(items) => items.iter().any(|v| mentions_batch(v)),
        V::Object(fields) => fields.iter().any(|(_, v)| mentions_batch(v)),
        _ => false,
    }
}

/// Type names in `sdl` that carry a type-level `@connect` (or a
/// `@link`-namespaced `@<ns>__connect`) whose arguments use `$batch`, on
/// the type's definition or on an `extend type`. The SDL is parsed, so a
/// `$batch` in a description, a comment or a root field's connector does
/// not count. An empty SDL has none; one that does not parse is an error,
/// never "no connector".
pub fn batch_connector_types(sdl: &str) -> Result<BTreeSet<String>, String> {
    use apollo_compiler::ast::{Definition, Document};
    let mut out = BTreeSet::new();
    if sdl.trim().is_empty() {
        return Ok(out);
    }
    let doc = Document::parse(sdl, "schema.graphql").map_err(|e| e.errors.to_string())?;
    for def in &doc.definitions {
        let (name, directives) = match def {
            Definition::ObjectTypeDefinition(t) => (&t.name, &t.directives),
            Definition::ObjectTypeExtension(t) => (&t.name, &t.directives),
            _ => continue,
        };
        let batched = directives.iter().any(|d| {
            (d.name == "connect" || d.name.ends_with("__connect"))
                && d.arguments.iter().any(|a| mentions_batch(&a.value))
        });
        if batched {
            out.insert(name.to_string());
        }
    }
    Ok(out)
}

fn has_batch(type_name: &str, type_override: Option<&str>, batch_types: &BTreeSet<String>) -> bool {
    batch_types.iter().any(|t| {
        t == type_name
            || t.ends_with(&format!("_{}", type_name))
            || type_override.is_some_and(|o| t == o || t.ends_with(&format!("_{}", o)))
    })
}

fn id_like_key(type_name: &str, shape: &Value, shapes: &Object) -> Option<String> {
    let t = norm(type_name);
    let props: Vec<String> = branches(shape, shapes)
        .into_iter()
        .filter_map(|b| get(b, "properties").and_then(Value::as_object))
        .flat_map(|p| p.keys().cloned())
        .collect();
    for wanted in ["id", "uuid"] {
        if props.iter().any(|p| p == wanted) {
            return Some(wanted.to_string());
        }
    }
    props
        .into_iter()
        .find(|p| norm(p) == format!("{}id", t) || norm(p) == format!("{}uuid", t))
}

/// The report for every keyed type (and, with `all_types`, every response
/// object type with an id-like field), sorted keyed first, then by name.
/// Errs only when `sdl` is not empty and does not parse: an unreadable
/// schema cannot say whether a `$batch` connector exists.
pub fn find(
    inventory: &Value,
    selection: Option<&Value>,
    sdl: &str,
    all_types: bool,
) -> Result<Vec<TypeReport>, String> {
    let shapes = shapes(inventory);
    let ops: Vec<&Value> = get(inventory, "operations")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let batch_types = batch_connector_types(sdl)?;

    // (type, key, keyed_by, type_name override, batch judgement)
    type Target = (String, String, Option<String>, Option<String>, Option<bool>);
    let mut targets: Vec<Target> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for op in &ops {
        let key = match get_str(op, "key") {
            Some(k) => k,
            None => continue,
        };
        let sel = match selection_op(selection, key) {
            Some(s) => s,
            None => continue,
        };
        if get(sel, "include").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let graphql = match get(sel, "graphql") {
            Some(g) => g,
            None => continue,
        };
        if get(graphql, "entity").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let shape = match result_shape(op, envelope_of(selection, key), &shapes) {
            // An entity operation that answers with an array (a listings
            // API's `GET /amenities?ids=`) keys the array's item type.
            Some(s) => shapes
                .get(&s)
                .and_then(|v| array_item_ref(v, &shapes))
                .unwrap_or(s),
            None => continue,
        };
        if !seen.insert(shape.clone()) {
            continue;
        }
        let entity_key = get_str(graphql, "key").unwrap_or("id").to_string();
        let type_override = get_str(graphql, "type_name").map(str::to_string);
        let batch = get(graphql, "batch").and_then(Value::as_bool);
        targets.push((
            shape,
            entity_key,
            Some(key.to_string()),
            type_override,
            batch,
        ));
    }
    if all_types {
        let mut response_shapes: BTreeSet<String> = BTreeSet::new();
        for op in &ops {
            let key = get_str(op, "key").unwrap_or("");
            let envelope = envelope_of(selection, key);
            if let Some(s) = result_shape(op, envelope, &shapes) {
                response_shapes.insert(s);
            }
            for (item, _) in response_items(op, envelope, &shapes) {
                if let Item::Named(n) = item {
                    response_shapes.insert(n);
                }
            }
        }
        let mut stems: BTreeSet<String> = seen.iter().map(|n| stem(n).to_string()).collect();
        // The name that is its own stem first, so `Listing` represents
        // `ListingSummary` rather than the other way round.
        let mut ordered: Vec<String> = response_shapes.into_iter().collect();
        ordered.sort_by_key(|n| (stem(n) != n.as_str(), n.len(), n.clone()));
        for name in ordered {
            if seen.contains(&name) || stems.contains(stem(&name)) {
                continue;
            }
            let shape = match shapes.get(&name) {
                Some(s) => s,
                None => continue,
            };
            if is_array(deref(shape, &shapes)) {
                continue;
            }
            if let Some(k) = id_like_key(stem(&name), shape, &shapes) {
                seen.insert(name.clone());
                stems.insert(stem(&name).to_string());
                targets.push((name, k, None, None, None));
            }
        }
    }

    let mut out = Vec::new();
    let source = source_name(sdl);
    // Parsed once, when the first draft needs it.
    let mut parsed: Option<Option<apollo_compiler::Schema>> = None;
    for (type_name, key, keyed_by, type_override, batch) in targets {
        let mut candidates = Vec::new();
        let mut note = None;
        if key.split_whitespace().count() != 1 || key.contains('{') {
            note = Some(format!(
                "compound key {:?}: a list parameter cannot carry it",
                key
            ));
        } else {
            for op in &ops {
                let op_key = match get_str(op, "key") {
                    Some(k) => k,
                    None => continue,
                };
                if !is_read(op, selection_op(selection, op_key)) {
                    continue;
                }
                let envelope = envelope_of(selection, op_key);
                let op_path = get_str(op, "path").unwrap_or("");
                for (item, via) in response_items(op, envelope, &shapes) {
                    if !same_entity(&item, op_path, &type_name, &key, &shapes) {
                        continue;
                    }
                    let key_param = key_list_param(op, &key, &type_name, &shapes);
                    let mut problems = Vec::new();
                    if let Some(k) = &key_param {
                        if let Some(missing) = missing_fields(&item, &type_name, &shapes) {
                            problems.push(Problem {
                                kind: "partial-shape".into(),
                                detail: format!(
                                    "{} lacks {} of {}",
                                    item.label(),
                                    missing.join(", "),
                                    type_name
                                ),
                            });
                        }
                        if !k.companions.is_empty() {
                            problems.push(Problem {
                                kind: "needs-scope".into(),
                                detail: format!(
                                    "{} also needs {}, which a type-level connector cannot supply",
                                    op_key,
                                    k.companions.join(", ")
                                ),
                            });
                        }
                        if let Some(c) = &k.conflict {
                            problems.push(Problem {
                                kind: "style-conflict".into(),
                                detail: c.clone(),
                            });
                        }
                        if let Some(p) = paginated(op) {
                            problems.push(Problem {
                                kind: "paginated".into(),
                                detail: format!(
                                    "{} is paginated ({}): a batch larger than one page silently drops the entities past it",
                                    op_key, p
                                ),
                            });
                        }
                    }
                    candidates.push(Candidate {
                        operation: op_key.to_string(),
                        item_shape: item.label(),
                        via,
                        key_param,
                        problems,
                    });
                }
            }
        }
        // Clean key-list candidates first, then those with the fewest
        // problems, then the lists with no key filter.
        candidates.sort_by_key(|c| (c.key_param.is_none(), c.problems.len()));
        let verdict = if candidates
            .iter()
            .any(|c| c.key_param.is_some() && c.problems.is_empty())
        {
            Verdict::Batchable
        } else if let Some(c) = candidates.iter().find(|c| c.key_param.is_some()) {
            Verdict::of_problem(&c.problems[0].kind)
        } else if !candidates.is_empty() {
            Verdict::ListNoKeyFilter
        } else {
            Verdict::None
        };
        let has_batch_connector = has_batch(&type_name, type_override.as_deref(), &batch_types);
        let sdl_type = sdl_type_name(sdl, &type_name, type_override.as_deref());
        // A draft only for a `batchable` type, against a key-list candidate
        // with no problem; any other verdict gets the reason instead.
        let (draft, draft_note) = match candidates
            .iter()
            .find(|c| c.key_param.is_some() && c.problems.is_empty())
        {
            Some(c) => {
                let op = ops
                    .iter()
                    .find(|o| get_str(o, "key") == Some(c.operation.as_str()))
                    .expect("candidate operation");
                let item = response_items(op, envelope_of(selection, &c.operation), &shapes)
                    .into_iter()
                    .find(|(_, via)| *via == c.via)
                    .and_then(|(item, _)| match item {
                        Item::Named(n) => shapes.get(&n).cloned(),
                        Item::Inline(v) => Some(v),
                    });
                if parsed.is_none() {
                    // A syntax error leaves no draft ("the schema does not
                    // parse") rather than one read from a partial schema.
                    parsed = Some(apollo_compiler::Schema::parse(sdl, "schema.graphql").ok());
                }
                let inputs = DraftInputs {
                    schema: parsed.as_ref().and_then(Option::as_ref),
                    sdl_type: sdl_type.as_deref(),
                    source: source.as_deref(),
                    item: item.as_ref(),
                    shapes: &shapes,
                };
                match render_connector(op, c, &key, &inputs) {
                    Ok(d) => (Some(d), None),
                    Err(why) => (None, Some(why)),
                }
            }
            None => (None, Some(refusal(verdict, &candidates, &key, &type_name))),
        };
        out.push(TypeReport {
            type_name,
            key,
            keyed_by,
            verdict,
            candidates,
            has_batch_connector,
            note,
            batch,
            draft,
            draft_note,
            sdl_type,
        });
    }
    out.sort_by(|a, b| {
        a.keyed_by
            .is_none()
            .cmp(&b.keyed_by.is_none())
            .then_with(|| a.type_name.cmp(&b.type_name))
    });
    Ok(out)
}

/// Keyed types that are batchable but have no `$batch` connector and no
/// recorded decline (`graphql.batch: false`): what `batch find --check` and
/// lint's `batchable-entity-unbatched` fail on.
pub fn missing_batch(reports: &[TypeReport]) -> Vec<&TypeReport> {
    reports
        .iter()
        .filter(|r| {
            r.keyed_by.is_some()
                && r.verdict == Verdict::Batchable
                && !r.has_batch_connector
                && r.batch != Some(false)
        })
        .collect()
}

/// The shape an operation resolves to once the selection's envelope is
/// unwrapped (ADR 0068): what the entity rules (ADR 0076) compare a keyed
/// type with.
pub fn operation_result_shape(
    op: &Value,
    selection: Option<&Value>,
    shapes: &Object,
) -> Option<String> {
    let key = get_str(op, "key").unwrap_or("");
    result_shape(op, envelope_of(selection, key), shapes)
}

/// Whether a parameter's wire name names `key` for `type_name` (`id`,
/// `ids`, `<type>Id`, `<type>_ids`): the one rule batch find and the entity
/// rules share.
pub fn param_names_key(param: &str, key: &str, type_name: &str) -> bool {
    names_the_key(param, key, type_name)
}

/// Why a type that is not `batchable` gets no draft: the verdict, and what
/// the best key-list candidate lacks (the missing fields, the unbound
/// parameters, both serializations).
fn refusal(verdict: Verdict, candidates: &[Candidate], key: &str, type_name: &str) -> String {
    let problems = candidates.iter().find(|c| c.key_param.is_some()).map(|c| {
        c.problems
            .iter()
            .map(|p| p.detail.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    });
    match (verdict, problems) {
        (Verdict::ListNoKeyFilter, _) => format!(
            "{}: no operation that lists {} takes a list of `{}`",
            verdict.label(),
            type_name,
            key
        ),
        (Verdict::None, _) => format!(
            "{}: nothing returns an array of {}",
            verdict.label(),
            type_name
        ),
        (v, Some(p)) if !p.is_empty() => format!("{}: {}", v.label(), p),
        (v, _) => v.label().to_string(),
    }
}

/// The workspace's `@source` name (exactly one, the core's model).
fn source_name(sdl: &str) -> Option<String> {
    let re = regex::Regex::new(r#"@source\(\s*name:\s*"([^"]+)""#).expect("static regex");
    re.captures(sdl).map(|c| c[1].to_string())
}

/// Every object type the schema defines or extends, parsed (so a type in
/// a description or comment is not one), in document order.
fn sdl_types(sdl: &str) -> Vec<String> {
    use apollo_compiler::ast::{Definition, Document};
    let doc = match Document::parse(sdl, "schema.graphql") {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for def in &doc.definitions {
        let name = match def {
            Definition::ObjectTypeDefinition(t) => t.name.to_string(),
            Definition::ObjectTypeExtension(t) => t.name.to_string(),
            _ => continue,
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The schema's type for an inventory shape: the shape name itself,
/// `<prefix>_<shape>`, or the selection's `graphql.type_name`. An exact
/// name wins; among `_<shape>` suffix matches the shortest does, so shape
/// `Item` is `P_Item`, not `P_Order_Item`.
fn sdl_type_name(sdl: &str, shape: &str, type_override: Option<&str>) -> Option<String> {
    let wanted: Vec<&str> = std::iter::once(shape).chain(type_override).collect();
    let types = sdl_types(sdl);
    if let Some(t) = types.iter().find(|n| wanted.contains(&n.as_str())) {
        return Some(t.clone());
    }
    types
        .into_iter()
        .filter(|n| wanted.iter().any(|w| n.ends_with(&format!("_{}", w))))
        .min_by_key(|n| n.len())
}

/// A schema type's fields as a draft selection sees them.
#[derive(Debug, Default)]
struct DraftFields {
    /// Scalar and enum fields with no arguments and no connector of their
    /// own: the ones the lookup's response fills.
    leaves: Vec<String>,
    /// Fields whose type is an object, interface or union: left out, to map
    /// by hand.
    objects: Vec<String>,
    /// Fields with arguments or their own `@connect`: resolved elsewhere, so
    /// not in the type-level selection.
    resolved_elsewhere: Vec<String>,
}

/// The fields of `type_name` in the parsed schema (its extensions
/// included), split the way a draft selection needs them.
fn draft_fields(schema: &apollo_compiler::Schema, type_name: &str) -> Option<DraftFields> {
    use apollo_compiler::schema::ExtendedType;
    let object = match schema.types.get(type_name)? {
        ExtendedType::Object(o) => o,
        _ => return None,
    };
    let mut out = DraftFields::default();
    for (name, field) in &object.fields {
        let name = name.to_string();
        if !field.arguments.is_empty() || field.directives.get("connect").is_some() {
            out.resolved_elsewhere.push(name);
            continue;
        }
        let named = field.ty.inner_named_type();
        match schema.types.get(named) {
            Some(ExtendedType::Object(_))
            | Some(ExtendedType::Interface(_))
            | Some(ExtendedType::Union(_)) => out.objects.push(name),
            _ => out.leaves.push(name),
        }
    }
    Some(out)
}

/// The selection entry for GraphQL field `gql` against the lookup's item
/// wire shape: `gql` when a property has that name, `gql: wire` when exactly
/// one property is the one a generator names `gql` (`amenity_category` for
/// `amenityCategory`), Err when no property maps with confidence. The
/// selection reads the wire, never the schema: a GraphQL name the response
/// lacks resolves to null, and `e2e --generate` would bake the null in.
fn wire_entry(gql: &str, props: &BTreeSet<String>) -> Result<String, Vec<String>> {
    if props.contains(gql) {
        return Ok(gql.to_string());
    }
    let cased: Vec<&String> = props
        .iter()
        .filter(|p| crate::cmd::scaffold::camel_of(p) == gql)
        .collect();
    match cased.as_slice() {
        [one] => Ok(format!("{}: {}", gql, one)),
        _ => Err(cased.into_iter().cloned().collect()),
    }
}

/// The type-level `$batch` connector for `candidate`, in the form the
/// e2e layer proved on a listings API (ADR 0068 step 2): repeated query values are
/// `name: $batch.key`, comma-separated ones `->joinNotNull(',')`, a body
/// array is `name: $batch.key` in `body`. Err names why no honest draft
/// exists.
/// What `render_connector` reads besides the candidate: the parsed schema
/// and the wire shape of the lookup's array items.
struct DraftInputs<'a> {
    schema: Option<&'a apollo_compiler::Schema>,
    sdl_type: Option<&'a str>,
    source: Option<&'a str>,
    /// The lookup's item shape (dereferenced by `property_names`).
    item: Option<&'a Value>,
    shapes: &'a Object,
}

fn render_connector(
    op: &Value,
    candidate: &Candidate,
    key: &str,
    d: &DraftInputs,
) -> Result<String, String> {
    let k = candidate.key_param.as_ref().expect("a key-list candidate");
    let sdl_type = d
        .sdl_type
        .ok_or("the schema has no type for this shape yet")?;
    let source = d.source.ok_or("the schema has no @source")?;
    let method = get_str(op, "method").unwrap_or("GET").to_ascii_uppercase();
    let path = get_str(op, "path").unwrap_or("/");
    // A type-level connector only has $batch (and $config): any other path
    // parameter has nothing to fill it.
    let others: Vec<&str> = get(op, "parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| get_str(p, "in") == Some("path") && get_str(p, "name") != Some(k.name.as_str()))
        .filter_map(|p| get_str(p, "name"))
        .collect();
    if !others.is_empty() {
        return Err(format!(
            "{} needs path parameter(s) {} that a type-level connector cannot fill ($batch carries only the keys)",
            candidate.operation,
            others.join(", ")
        ));
    }
    let required: Vec<&str> = get(op, "parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| {
            get(p, "required").and_then(Value::as_bool) == Some(true)
                && get_str(p, "in") != Some("path")
                && get_str(p, "name") != Some(k.name.as_str())
        })
        .filter_map(|p| get_str(p, "name"))
        .collect();
    if !required.is_empty() {
        return Err(format!(
            "{} requires {} besides the keys; supply it in the draft by hand",
            candidate.operation,
            required.join(", ")
        ));
    }
    let value = match k.passing.as_str() {
        "repeated" | "array" => format!("$batch.{}", key),
        "comma-separated" => format!("$batch.{}->joinNotNull(',')", key),
        other => return Err(format!("no proven rendering for a {} key list", other)),
    };
    let http = match k.location.as_str() {
        "query" => format!(
            "{{ {}: \"{}\", queryParams: \"{}: {}\" }}",
            method, path, k.name, value
        ),
        "body" => format!(
            "{{ {}: \"{}\", body: \"{}: {}\" }}",
            method, path, k.name, value
        ),
        other => {
            return Err(format!(
                "no proven rendering for a key list in the {}",
                other
            ))
        }
    };
    let schema = d.schema.ok_or("the schema does not parse")?;
    let fields = draft_fields(schema, sdl_type)
        .ok_or_else(|| format!("{} is not an object type in the schema", sdl_type))?;
    // `@key` and `$batch` name the key by its GraphQL field; the selection
    // below maps that field to its wire property like any other.
    if !fields.leaves.iter().any(|f| f == key) {
        return Err(format!(
            "the key `{}` is not a scalar field of {}; write the draft by hand",
            key, sdl_type
        ));
    }
    let item = d.item.ok_or_else(|| {
        format!(
            "{}'s item shape is not in the inventory",
            candidate.operation
        )
    })?;
    let props = property_names(item, d.shapes);
    let mut entries = Vec::new();
    let mut unmapped = Vec::new();
    for f in &fields.leaves {
        match wire_entry(f, &props) {
            Ok(e) => entries.push(e),
            Err(tied) if tied.is_empty() => unmapped.push(format!("`{}`", f)),
            Err(tied) => unmapped.push(format!("`{}` (both {})", f, tied.join(" and "))),
        }
    }
    if !unmapped.is_empty() {
        return Err(format!(
            "no wire property of {} maps with confidence to {} of {} (properties: {}); write the selection by hand, reading how the type's other connectors map it",
            candidate.item_shape,
            unmapped.join(", "),
            sdl_type,
            props.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    let objects = fields.objects;
    let fields = entries.join(" ");
    let selection = match candidate.via.split_once(':') {
        Some((_, prop)) => format!("$.{} {{ {} }}", prop, fields),
        None => fields,
    };
    let mut out = format!(
        "type {}\n  @key(fields: \"{}\")\n  @connect(\n    source: \"{}\"\n    http: {}\n    selection: \"{}\"\n",
        sdl_type, key, source, http, selection
    );
    if let Some(max) = k.max_size {
        out.push_str(&format!("    batch: {{ maxSize: {} }}\n", max));
    }
    out.push_str("  )");
    if !objects.is_empty() {
        out.push_str(&format!(
            "\n# left out of the selection (object fields; map them by hand): {}",
            objects.join(", ")
        ));
    }
    Ok(out)
}

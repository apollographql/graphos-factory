//! OpenAPI 3.x -> inventory.json.
//!
//! This reader understands one dialect. A Swagger 2.0 document is rewritten
//! into this model by `swagger.rs` first; `spec::read` is the front door that
//! loads a file, names its format and routes it here.
//!
//! A *reader*, not a generator: it decides nothing about GraphQL. It answers
//! "what does this API offer, and what can a connector express?" and records
//! every operation as supported, unsupported (with a reason), or unresolved.
//! The one judgement it encodes is the support classification, because that
//! is a property of Apollo Connectors, not of the API.
//!
//! Key order and edge behaviour deliberately match the previous JavaScript
//! reader so the committed pilot inventories reproduce byte for byte.

use crate::inventory::{cap, shape_name};
use crate::json::{compact, field, get, get_arr, get_obj, get_str, obj, truthy, Object};
pub use crate::spec::value_to_string;
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Path-item visiting order. Since ADR 0069 this order no longer chooses a
/// `candidate_entity_link` target: only a canonical GET-by-id operation is one.
const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "patch", "head", "options", "trace",
];

fn json_content() -> Regex {
    Regex::new(r"(?i)^application/([a-z0-9!#$&^_.+-]*\+)?json\b").unwrap()
}
fn form_content() -> Regex {
    Regex::new(r"(?i)^application/x-www-form-urlencoded\b").unwrap()
}
/// `application/json` and its `+json` vendor forms (shared with the Swagger reader).
pub fn is_json_content_type(t: &str) -> bool {
    json_content().is_match(t)
}
pub fn is_form_content_type(t: &str) -> bool {
    form_content().is_match(t)
}

struct Refs<'a> {
    spec: &'a Value,
}

impl<'a> Refs<'a> {
    fn resolve(&self, node: Option<&'a Value>) -> Result<Option<&'a Value>, String> {
        let mut current = node;
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(c) = current {
            match c
                .as_object()
                .and_then(|o| o.get("$ref"))
                .and_then(Value::as_str)
            {
                Some(r) => {
                    if seen.contains(r) {
                        return Err(format!("circular $ref chain at {}", r));
                    }
                    seen.insert(r.to_string());
                    current = Some(self.lookup(r)?);
                }
                None => break,
            }
        }
        Ok(current)
    }

    fn lookup(&self, reference: &str) -> Result<&'a Value, String> {
        if !reference.starts_with("#/") {
            return Err(format!("external $ref is not supported: {}", reference));
        }
        let mut node = self.spec;
        for raw in reference[2..].split('/') {
            let part = percent_decode(raw).replace("~1", "/").replace("~0", "~");
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
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 0 && i + 2 <= bytes.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn is_ref(v: &Value) -> Option<&str> {
    v.as_object()
        .and_then(|o| o.get("$ref"))
        .and_then(Value::as_str)
}

/// Merge an allOf chain into one object schema, resolving refs as it goes.
fn flatten_all_of(schema: &Value, refs: &Refs, seen: &HashSet<String>) -> Result<Value, String> {
    let s = match schema.as_object() {
        Some(s) => s,
        None => return Ok(schema.clone()),
    };
    if is_ref(schema).is_some() {
        return Ok(schema.clone());
    }
    let all = match s.get("allOf").and_then(Value::as_array) {
        Some(a) => a,
        None => return Ok(schema.clone()),
    };

    // `type: object` is added at the end, and only when the merge actually
    // produced object structure (non-empty `properties`) or a member
    // explicitly declared its own `type`. Seeding it here unconditionally
    // was the bug: an allOf combining a $ref'd `oneOf` (no `type` of its
    // own) with an annotation-only fragment (e.g. `{"example": 10}`) has
    // no object-shaped member at all, so the "merge" was fabricating
    // `type: object` beside a `oneOf` of scalars — a schema no instance
    // can ever satisfy. See `flatten_all_of`'s tests for the reproduced
    // case (a vendor's `OverlayAssessmentDataValue.value`).
    let mut merged = obj();
    merged.insert("properties".into(), Value::Object(obj()));
    merged.insert("required".into(), Value::Array(vec![]));
    let mut sources: Vec<String> = Vec::new();
    let mut rest = s.clone();
    rest.shift_remove("allOf");

    fn absorb(
        member: &Value,
        depth: usize,
        visited: &HashSet<String>,
        refs: &Refs,
        merged: &mut Object,
        sources: &mut Vec<String>,
    ) -> Result<(), String> {
        if !member.is_object() {
            return Ok(());
        }
        if let Some(r) = is_ref(member) {
            if visited.contains(r) || depth > 12 {
                // A cycle through allOf cannot be merged into a finite object.
                let list = merged
                    .entry("allOf")
                    .or_insert_with(|| Value::Array(vec![]));
                if let Value::Array(a) = list {
                    a.push(crate::json::object(vec![("$ref", Value::from(r))]));
                }
                return Ok(());
            }
            sources.push(r.to_string());
            let mut next = visited.clone();
            next.insert(r.to_string());
            let target = refs.lookup(r)?;
            let flat = flatten_all_of(target, refs, &next)?;
            return absorb(&flat, depth + 1, &next, refs, merged, sources);
        }
        let flat = flatten_all_of(member, refs, visited)?;
        if let Value::Object(fo) = flat {
            for (k, v) in fo {
                match k.as_str() {
                    "properties" => {
                        if let (Some(Value::Object(dst)), Value::Object(src)) =
                            (merged.get_mut("properties"), v)
                        {
                            for (pk, pv) in src {
                                dst.insert(pk, pv);
                            }
                        }
                    }
                    "required" => {
                        if let (Some(Value::Array(dst)), Value::Array(src)) =
                            (merged.get_mut("required"), v)
                        {
                            dst.extend(src);
                        }
                    }
                    "allOf" => {
                        let list = merged
                            .entry("allOf")
                            .or_insert_with(|| Value::Array(vec![]));
                        if let (Value::Array(dst), Value::Array(src)) = (list, v) {
                            dst.extend(src);
                        }
                    }
                    "x-allof-sources" => {
                        if let Value::Array(src) = v {
                            sources
                                .extend(src.iter().filter_map(Value::as_str).map(str::to_string));
                        }
                    }
                    _ => {
                        if !merged.contains_key(&k) || k == "type" {
                            merged.insert(k, v);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    for member in all {
        absorb(member, 0, seen, refs, &mut merged, &mut sources)?;
    }
    // Object.assign(merged, rest): the schema's own keys override, wholesale.
    for (k, v) in rest {
        merged.insert(k, v);
    }
    let props_empty = merged
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.is_empty())
        .unwrap_or(true);
    if props_empty && !merged.contains_key("allOf") {
        merged.shift_remove("properties");
    }
    // Only default to `type: object` once the merge actually produced
    // object structure. A member's own explicit `type` (any value) was
    // already absorbed above and is left untouched either way.
    if !props_empty && !merged.contains_key("type") {
        merged.insert("type".into(), Value::from("object"));
    }
    let required: Vec<Value> = {
        let mut seen_req = HashSet::new();
        merged
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|v| seen_req.insert(compact(v)))
            .collect()
    };
    if required.is_empty() {
        merged.shift_remove("required");
    } else {
        merged.insert("required".into(), Value::Array(required));
    }
    if !sources.is_empty() {
        let mut seen_src = HashSet::new();
        let uniq: Vec<Value> = sources
            .into_iter()
            .filter(|s| seen_src.insert(s.clone()))
            .map(Value::from)
            .collect();
        merged.insert("x-allof-sources".into(), Value::Array(uniq));
    }
    Ok(Value::Object(merged))
}

/// Convert spec schemas into inventory `shapes`, keeping named schemas by reference.
struct ShapeSet<'a> {
    refs: &'a Refs<'a>,
    shapes: Object,
    names: HashMap<String, String>,
    order: Vec<String>, // refs in nameFor order, for `taken`
    pending: std::collections::VecDeque<(String, String)>,
    broken: HashMap<String, String>,
}

/// Copied verbatim. `exclusiveMinimum`/`exclusiveMaximum` are a number in
/// OpenAPI 3.1 and a boolean qualifying `minimum`/`maximum` in 3.0 and
/// Swagger 2.0; the inventory keeps whichever the source wrote (ADR 0065).
const COPY_KEYS: [&str; 20] = [
    "title",
    "description",
    "format",
    "enum",
    "const",
    "default",
    "example",
    "pattern",
    "minimum",
    "maximum",
    // An array's bounds (a body's `custodians`, 1..100): facts the argument's
    // doc comment must state (schema-authoring.md § Argument constraints).
    "minItems",
    "maxItems",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "deprecated",
    "readOnly",
    "writeOnly",
    "nullable",
    "x-allof-sources",
    "x-expansion",
];

impl<'a> ShapeSet<'a> {
    fn new(refs: &'a Refs<'a>) -> Self {
        ShapeSet {
            refs,
            shapes: obj(),
            names: HashMap::new(),
            order: Vec::new(),
            pending: Default::default(),
            broken: HashMap::new(),
        }
    }

    fn name_for(&mut self, reference: &str) -> String {
        if let Some(n) = self.names.get(reference) {
            return n.clone();
        }
        let last = reference.rsplit('/').next().unwrap_or(reference);
        let base: String = last
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let taken: HashSet<&String> = self.names.values().collect();
        let mut name = base.clone();
        let mut n = 2;
        while taken.contains(&name) {
            name = format!("{}{}", base, n);
            n += 1;
        }
        self.names.insert(reference.to_string(), name.clone());
        self.order.push(reference.to_string());
        self.pending
            .push_back((reference.to_string(), name.clone()));
        name
    }

    fn convert(&mut self, schema: Option<&Value>, depth: usize) -> Result<Value, String> {
        let schema = match schema {
            None | Some(Value::Null) => return Ok(Value::Null),
            Some(s) => s,
        };
        if let Value::Bool(b) = schema {
            return Ok(if *b {
                Value::Object(obj())
            } else {
                crate::json::object(vec![("not", Value::Object(obj()))])
            });
        }
        if let Some(r) = is_ref(schema) {
            let name = self.name_for(r);
            let mut out = vec![("$ref", Value::from(format!("#/shapes/{}", name)))];
            // OpenAPI 3.0 and Swagger 2.0 ignore siblings of `$ref`, but
            // `nullable: true` beside one (go-swagger's `x-nullable`, every
            // Gitea `Issue.milestone`) is how a spec says "this reference may
            // be null" without wrapping it in allOf. Keep exactly that one so
            // the conformance oracle accepts the null the API sends.
            if get(schema, "nullable") == Some(&Value::Bool(true)) {
                out.push(("nullable", Value::Bool(true)));
            }
            // An expansion boundary (ADR 0047) annotates the reference
            // itself: the relationship to another node and its default
            // projection. The reference and its target shape are kept.
            if let Some(x) = get(schema, "x-expansion") {
                out.push(("x-expansion", x.clone()));
            }
            return Ok(crate::json::object(out));
        }
        if depth > 25 {
            return Ok(crate::json::object(vec![(
                "description",
                Value::from("depth limit reached during inventory conversion"),
            )]));
        }
        let flat = flatten_all_of(schema, self.refs, &HashSet::new())?;
        let f = match flat.as_object() {
            Some(f) => f.clone(),
            None => return Ok(Value::Object(obj())),
        };
        let mut out = obj();
        for key in COPY_KEYS {
            if let Some(v) = f.get(key) {
                out.insert(key.into(), v.clone());
            }
        }
        if let Some(t) = f.get("type") {
            out.insert("type".into(), t.clone());
        } else if f.get("properties").is_some() && !f["properties"].is_null() {
            out.insert("type".into(), Value::from("object"));
        } else if f.get("items").is_some() && !f["items"].is_null() {
            out.insert("type".into(), Value::from("array"));
        }
        if let Some(items) = f.get("items").filter(|v| truthy(Some(v))) {
            let v = self.convert(Some(items), depth + 1)?;
            out.insert("items".into(), v);
        }
        if let Some(Value::Object(props)) = f.get("properties") {
            let mut converted = obj();
            for (k, v) in props {
                converted.insert(k.clone(), self.convert(Some(v), depth + 1)?);
            }
            out.insert("properties".into(), Value::Object(converted));
        }
        if let Some(req) = f.get("required").filter(|v| truthy(Some(v))) {
            out.insert("required".into(), req.clone());
        }
        match f.get("additionalProperties") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => {}
            Some(Value::Bool(true)) => {
                out.insert("additionalProperties".into(), Value::Bool(true));
            }
            Some(other) => {
                let v = self.convert(Some(other), depth + 1)?;
                out.insert("additionalProperties".into(), v);
            }
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            if let Some(Value::Array(list)) = f.get(key) {
                let mut converted = Vec::new();
                for s in list {
                    converted.push(self.convert(Some(s), depth + 1)?);
                }
                out.insert(key.into(), Value::Array(converted));
            }
        }
        if let Some(d) = f
            .get("discriminator")
            .and_then(|d| get_str(d, "propertyName"))
        {
            out.insert("discriminator".into(), Value::from(d));
        }
        if out.is_empty() {
            out.insert(
                "description".into(),
                Value::from("Schema is empty in the OpenAPI document."),
            );
        }
        Ok(Value::Object(out))
    }

    fn drain(&mut self) -> Result<(), String> {
        while let Some((reference, name)) = self.pending.pop_front() {
            if self.shapes.get(&name).map(truthy_obj).unwrap_or(false) {
                continue;
            }
            self.shapes.insert(name.clone(), Value::Object(obj()));
            let converted = match self.refs.lookup(&reference) {
                Ok(target) => self.convert(Some(target), 0),
                Err(e) => Err(e),
            };
            match converted {
                Ok(v) => {
                    self.shapes.insert(name, v);
                }
                Err(e) => {
                    self.broken
                        .insert(name.clone(), format!("{}: {}", reference, e));
                    self.shapes.insert(
                        name,
                        crate::json::object(vec![
                            (
                                "description",
                                Value::from(format!("Unreadable in the source document ({}).", e)),
                            ),
                            ("x-unresolved", Value::from(reference)),
                        ]),
                    );
                }
            }
        }
        Ok(())
    }
}

/// A JavaScript object is truthy even when empty; `if (this.shapes[name])`.
fn truthy_obj(v: &Value) -> bool {
    !v.is_null()
}

#[derive(Clone)]
struct ContentEntry {
    content_type: String,
    media: Value,
    kind: &'static str,
}

fn content_entry(content: Option<&Value>) -> Option<ContentEntry> {
    let c = content?.as_object()?;
    let types: Vec<&String> = c.keys().collect();
    if let Some(t) = types.iter().find(|t| json_content().is_match(t)) {
        return Some(ContentEntry {
            content_type: (*t).clone(),
            media: c[*t].clone(),
            kind: "json",
        });
    }
    if let Some(t) = types.iter().find(|t| form_content().is_match(t)) {
        return Some(ContentEntry {
            content_type: (*t).clone(),
            media: c[*t].clone(),
            kind: "form",
        });
    }
    let first = types.first()?;
    Some(ContentEntry {
        content_type: (*first).clone(),
        media: c[*first].clone(),
        kind: "other",
    })
}

fn success_response<'a>(
    responses: Option<&'a Value>,
    refs: &Refs<'a>,
) -> Result<Option<(String, &'a Value)>, String> {
    let r = match responses.and_then(Value::as_object) {
        Some(r) => r,
        None => return Ok(None),
    };
    let codes: Vec<&String> = r.keys().collect();
    let two = Regex::new(r"^2\d\d$").unwrap();
    let mut ordered: Vec<&String> = codes.iter().copied().filter(|c| two.is_match(c)).collect();
    ordered.sort();
    ordered.extend(codes.iter().copied().filter(|c| c.as_str() == "2XX"));
    ordered.extend(codes.iter().copied().filter(|c| c.as_str() == "default"));
    for code in ordered {
        if let Some(response) = refs.resolve(r.get(code))? {
            return Ok(Some((code.clone(), response)));
        }
    }
    Ok(None)
}

const CURSOR_PARAMS: [&str; 7] = [
    "cursor",
    "after",
    "starting_after",
    "page_token",
    "pageToken",
    "next",
    "continuation_token",
];
const PAGE_PARAMS: [&str; 4] = ["page", "page_number", "pageNumber", "page_index"];
const OFFSET_PARAMS: [&str; 3] = ["offset", "start", "skip"];
const SIZE_PARAMS: [&str; 8] = [
    "limit",
    "per_page",
    "page_size",
    "pageSize",
    "count",
    "max_results",
    "size",
    "page_limit",
];
/// Response keys that carry the next cursor, most specific first: a
/// `next_page_token` (FullStory) or `nextPageToken` (Google Discovery) is
/// preferred to a bare `next`, which is as often a URL.
pub const NEXT_KEYS: [&str; 9] = [
    "next_cursor",
    "nextCursor",
    "next_page_token",
    "nextPageToken",
    "next",
    "next_page",
    "after",
    "cursor",
    "end_cursor",
];

/// Response keys that carry a link to the next page when the request takes
/// no paging parameter at all: the client follows the link instead of sending
/// a cursor back. Salesforce's SOQL `nextRecordsUrl` (a relative path such as
/// `/services/data/v67.0/query/01g…-2000`) and OData's `@odata.nextLink`.
/// Read only when the parameters imply no style (ADR 0038's cursor search is
/// unchanged).
pub const NEXT_LINK_KEYS: [&str; 2] = ["nextRecordsUrl", "@odata.nextLink"];

/// Read verbs a POST's operationId or last path segment can carry: a POST
/// that searches, renders or previews may be a read. They only ever produce
/// a `read_hint`; the recorded `semantics` stays `write` (naming.md). Verbs
/// that name an action as often as a reading (`diff`, `export`, `check`,
/// `test`, `verify`) are deliberately absent: `repoApplyDiffPatch` writes.
const POST_READ_WORDS: [&str; 14] = [
    "search",
    "query",
    "filter",
    "lookup",
    "find",
    "list",
    "preview",
    "render",
    "validate",
    "count",
    "calculate",
    "estimate",
    "compare",
    "get",
];

/// Verbs that make an operationId an action whatever else it says:
/// `createSavedSearch` creates, `addToList` adds. An operationId carrying one
/// contributes no read hint; the path may still (`createSchedulePreview` on
/// `/schedules/preview`).
const POST_ACTION_WORDS: [&str; 46] = [
    "create",
    "add",
    "update",
    "edit",
    "delete",
    "remove",
    "upload",
    "import",
    "set",
    "send",
    "start",
    "stop",
    "cancel",
    "trigger",
    "run",
    "apply",
    "merge",
    "sync",
    "reset",
    "revoke",
    "rename",
    "transfer",
    "accept",
    "reject",
    "pin",
    "unpin",
    "dismiss",
    "submit",
    "generate",
    "migrate",
    "link",
    "unlink",
    "adopt",
    "publish",
    "archive",
    "restore",
    "move",
    "copy",
    "assign",
    "invite",
    "register",
    "subscribe",
    "enable",
    "disable",
    "approve",
    "deploy",
];

/// Split an identifier into lower-case words on camelCase, `_`, `-`, `.`
/// and `:` (Google-style custom methods, `/things:search`).
fn words(ident: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = ident.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if matches!(c, '_' | '-' | '.' | ':' | ' ') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        let boundary = c.is_ascii_uppercase()
            && i > 0
            && (chars[i - 1].is_ascii_lowercase()
                || chars[i - 1].is_ascii_digit()
                || (i + 1 < chars.len() && chars[i + 1].is_ascii_lowercase()));
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Why a POST might be a read, when its operationId or the path's last
/// literal segment carries a read verb (`renderMarkdown`, `/repos/search`,
/// `repoGetFileContentsPost`) and the operationId carries no action verb
/// (`createSavedSearch` does not read). The inventory still records the
/// operation as `write`; this is what the agent surfaces for the user to
/// confirm before choosing `root: query` (ADR 0015).
pub fn read_hint(operation_id: &str, path: &str) -> Option<String> {
    let id_words = words(operation_id);
    let vetoed = id_words
        .iter()
        .any(|w| POST_ACTION_WORDS.contains(&w.as_str()));
    if !vetoed {
        if let Some(w) = id_words
            .iter()
            .find(|w| POST_READ_WORDS.contains(&w.as_str()))
        {
            return Some(format!(
                "operationId {} carries `{}`: possibly a read — confirm before choosing root: query",
                operation_id, w
            ));
        }
    }
    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|seg| !seg.is_empty() && !seg.starts_with('{'))?;
    let w = words(last)
        .into_iter()
        .find(|w| POST_READ_WORDS.contains(&w.as_str()))?;
    Some(format!(
        "path segment `{}` carries `{}`: possibly a read — confirm before choosing root: query",
        last, w
    ))
}

/// The pagination a set of query-parameter names and response shapes imply:
/// style from the parameter vocabulary, the next-cursor key from the first
/// response shape that carries one.
fn pagination_for(names: &HashSet<String>, responses: &[Option<&Value>], shapes: &Object) -> Value {
    let has = |list: &[&str]| {
        list.iter()
            .find(|n| names.contains(**n))
            .map(|s| s.to_string())
    };
    let cursor = has(&CURSOR_PARAMS);
    let page = has(&PAGE_PARAMS);
    let offset = has(&OFFSET_PARAMS);
    let size = has(&SIZE_PARAMS);

    let (style, request) = if let Some(c) = cursor {
        ("cursor", Some(c))
    } else if let Some(p) = page {
        ("page", Some(p))
    } else if let Some(o) = offset {
        ("offset", Some(o))
    } else if size.is_some() {
        ("unknown", None)
    } else {
        ("none", None)
    };

    let mut response: Option<String> = None;
    let mut next_url: Option<String> = None;
    if style == "cursor" {
        for shape in responses.iter().flatten() {
            if let Some((found, url)) = find_next_key(Some(shape), shapes, 0, "") {
                response = Some(found);
                next_url = url;
                break;
            }
        }
    }
    let mut fields = vec![
        ("style", Value::from(style)),
        ("request", request.map(Value::from).unwrap_or(Value::Null)),
        ("size_param", size.map(Value::from).unwrap_or(Value::Null)),
        ("response", response.map(Value::from).unwrap_or(Value::Null)),
    ];
    // No paging parameter, but the response links to the next page: the
    // pages are followed, not requested (`next_link`). The link's path is the
    // fact; `size_param` is kept when there is one.
    if style == "none" || style == "unknown" {
        if let Some(link) = responses
            .iter()
            .flatten()
            .find_map(|shape| find_next_link(Some(shape), shapes, 0, ""))
        {
            fields[0] = ("style", Value::from("next_link"));
            next_url = Some(link);
        }
    }
    if let Some(url) = next_url {
        fields.push(("next_url", Value::from(url)));
    }
    crate::json::object(fields)
}

/// The API-wide summary, derived from the per-operation facts and never from
/// a pooled set of parameter names: pooled, one `cursor` parameter outweighed
/// Mailchimp's 58 offset-paged operations. `style` is the style most
/// operations carry among cursor, page, offset and next_link (ADR 0052), a
/// tie going to the earlier; `unknown` (a size parameter and no style) is
/// the summary only when no operation carries one of the four, since it is
/// the absence of a style and must not outvote one. `counts` records every
/// style's operation count, `unknown` included. `request`,
/// `size_param` and `response` are the values most common among the
/// operations of that style, a tie going to the first in document order;
/// `next_url` is voted among those whose `response` is the winner.
/// An API with no paginated operation is `none`, with no `counts`.
fn detect_pagination(operations: &[Value]) -> Value {
    const PRECEDENCE: [&str; 4] = ["cursor", "page", "offset", "next_link"];
    let blocks: Vec<&Value> = operations
        .iter()
        .filter_map(|op| get(op, "pagination"))
        .collect();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for b in &blocks {
        if let Some(style) = get_str(b, "style") {
            *counts.entry(style.to_string()).or_default() += 1;
        }
    }
    let Some(style) = counts
        .iter()
        .filter_map(|(style, n)| {
            let rank = PRECEDENCE.iter().position(|p| p == style)?;
            Some((style, (*n, std::cmp::Reverse(rank))))
        })
        .max_by_key(|(_, key)| *key)
        .map(|(style, _)| style)
        .or_else(|| counts.get_key_value("unknown").map(|(style, _)| style))
        .cloned()
    else {
        return pagination_for(&HashSet::new(), &[], &Object::new());
    };
    let of_style: Vec<&Value> = blocks
        .into_iter()
        .filter(|b| get_str(b, "style") == Some(style.as_str()))
        .collect();
    let most_common = |blocks: &[&Value], field: &str| -> Value {
        let mut seen: Vec<(&str, usize)> = Vec::new();
        for value in blocks.iter().filter_map(|b| get_str(b, field)) {
            match seen.iter_mut().find(|(v, _)| *v == value) {
                Some((_, n)) => *n += 1,
                None => seen.push((value, 1)),
            }
        }
        // `max_by_key` keeps the last maximum; reverse so a tie goes to
        // the first value in document order.
        seen.iter()
            .rev()
            .max_by_key(|(_, n)| *n)
            .map(|(v, _)| Value::from(*v))
            .unwrap_or(Value::Null)
    };
    let response = most_common(&of_style, "response");
    // `next_url` sits beside the cursor it came with: vote only among the
    // operations whose `response` is the winner, so a Graph-API list's
    // `paging.next` never pairs with a flat list's `next_cursor`.
    let with_response: Vec<&Value> = of_style
        .iter()
        .copied()
        .filter(|b| get_str(b, "response") == response.as_str())
        .collect();
    let next_url = most_common(&with_response, "next_url");
    let mut fields = vec![
        ("style", Value::from(style.as_str())),
        ("request", most_common(&of_style, "request")),
        ("size_param", most_common(&of_style, "size_param")),
        ("response", response),
    ];
    if !next_url.is_null() {
        fields.push(("next_url", next_url));
    }
    fields.push((
        "counts",
        Value::Object(
            counts
                .into_iter()
                .map(|(k, v)| (k, Value::from(v)))
                .collect(),
        ),
    ));
    crate::json::object(fields)
}

/// A cursor or a next-page link is a string: `type: string`, a nullable
/// union, an anyOf/oneOf with a string branch (how PagerDuty writes nullable
/// references, and how OpenAPI 3.1 writes a nullable link), a `$ref` to one,
/// or a property the spec leaves untyped — the name is then the only signal,
/// as it was before the type rule. An object or array is a link object or a
/// page, never a cursor: a HAL-style `next: { href }` is not a value the
/// connector can hand back as `nextCursor`.
fn is_stringish(v: &Value, shapes: &Object, depth: usize) -> bool {
    if depth > 4 {
        return false;
    }
    let resolved = match is_ref(v) {
        Some(r) => match shapes.get(shape_name(r)) {
            Some(s) => s,
            None => return false,
        },
        None => v,
    };
    if let Some(branches) = get_arr(resolved, "anyOf").or_else(|| get_arr(resolved, "oneOf")) {
        return branches.iter().any(|b| is_stringish(b, shapes, depth + 1));
    }
    match get(resolved, "type") {
        Some(Value::String(t)) => t == "string",
        Some(Value::Array(ts)) => ts.iter().any(|t| t.as_str() == Some("string")),
        _ => {
            get(resolved, "properties").is_none()
                && get(resolved, "items").is_none()
                && get(resolved, "enum").is_none()
        }
    }
}

fn find_next_key(
    shape: Option<&Value>,
    shapes: &Object,
    depth: usize,
    prefix: &str,
) -> Option<(String, Option<String>)> {
    let shape = shape?;
    if depth > 3 {
        return None;
    }
    let resolved = match is_ref(shape) {
        Some(r) => shapes.get(shape_name(r))?,
        None => shape,
    };
    let props = get_obj(resolved, "properties")?;
    let path = |key: &str| {
        if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}.{}", prefix, key)
        }
    };
    for key in NEXT_KEYS {
        if props
            .get(key)
            .map(|v| is_stringish(v, shapes, 0))
            .unwrap_or(false)
        {
            // Graph-API paging (Meta): `{ cursors: { before, after }, next }`,
            // where `next` is a full URL and the cursor to hand back is
            // `cursors.after`. Only for `next` / `next_page` with a sibling
            // `cursors` object carrying a string `after` or `end_cursor`
            // (`after` preferred); the URL is kept as `next_url`.
            if key == "next" || key == "next_page" {
                let cursors = props.get("cursors").and_then(|c| match is_ref(c) {
                    Some(r) => shapes.get(shape_name(r)),
                    None => Some(c),
                });
                let inner = cursors.and_then(|c| get_obj(c, "properties"));
                if let Some(inner) = inner {
                    if let Some(cursor) = ["after", "end_cursor"].into_iter().find(|k| {
                        inner
                            .get(*k)
                            .map(|v| is_stringish(v, shapes, 0))
                            .unwrap_or(false)
                    }) {
                        return Some((path(&format!("cursors.{}", cursor)), Some(path(key))));
                    }
                }
            }
            return Some((path(key), None));
        }
    }
    let meta = Regex::new(r"(?i)meta|pagination|paging|page_info|pageInfo").unwrap();
    for (key, value) in props {
        if !meta.is_match(key) {
            continue;
        }
        let next_prefix = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", prefix, key)
        };
        if let Some(nested) = find_next_key(Some(value), shapes, depth + 1, &next_prefix) {
            return Some(nested);
        }
    }
    None
}

/// The dotted path to a next-page link (NEXT_LINK_KEYS) in a response shape:
/// a string property (`is_stringish`, the cursor's own test) at the top
/// level or inside a meta/pagination object, the same places
/// `find_next_key` looks for a cursor.
fn find_next_link(
    shape: Option<&Value>,
    shapes: &Object,
    depth: usize,
    prefix: &str,
) -> Option<String> {
    let shape = shape?;
    if depth > 3 {
        return None;
    }
    let resolved = match is_ref(shape) {
        Some(r) => shapes.get(shape_name(r))?,
        None => shape,
    };
    let props = get_obj(resolved, "properties")?;
    let path = |key: &str| {
        if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}.{}", prefix, key)
        }
    };
    for key in NEXT_LINK_KEYS {
        if props.get(key).is_some_and(|v| is_stringish(v, shapes, 0)) {
            return Some(path(key));
        }
    }
    let meta = Regex::new(r"(?i)meta|pagination|paging|page_info|pageInfo").unwrap();
    props
        .iter()
        .filter(|(key, _)| meta.is_match(key))
        .find_map(|(key, value)| find_next_link(Some(value), shapes, depth + 1, &path(key)))
}

/// The last word a follow-up's path parameter may carry: `{queryLocator}`,
/// `{cursor}`, `{nextToken}`, `{pageToken}`, `{page}`. The last word, not a
/// substring, so a key such as Microsoft Graph's `{sitePage-id}` or a
/// `{tokenId}` is not read as a locator.
const NEXT_LINK_PARAM_WORDS: [&str; 5] = ["locator", "cursor", "token", "page", "next"];

/// One operation as the `next_operation` post-pass sees it: method, path,
/// key, and its own `next_link` link path, if it has one.
type Route = (String, String, String, Option<String>);

/// The operation that fetches the page a `next_link` points at: a GET whose
/// path is the list's own path (without its query string) plus one trailing
/// parameter whose last word is a locator, cursor, token, page or next, and
/// whose own response carries the same link, since the page it returns links
/// on to the next one. Salesforce: `…/query?q=…` pages through
/// `…/query/{queryLocator}`. Nothing is recorded when no such operation
/// exists or more than one does.
fn next_link_operation(path: &str, link: &str, routes: &[Route]) -> Option<String> {
    let base = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    let mut found = routes.iter().filter(|(method, candidate, _, own_link)| {
        if !method.eq_ignore_ascii_case("GET") || own_link.as_deref() != Some(link) {
            return false;
        }
        let Some(rest) = candidate.strip_prefix(base) else {
            return false;
        };
        let Some(param) = rest.strip_prefix("/{").and_then(|r| r.strip_suffix('}')) else {
            return false;
        };
        !param.contains(['/', '{', '}'])
            && words(param)
                .last()
                .is_some_and(|w| NEXT_LINK_PARAM_WORDS.contains(&w.as_str()))
    });
    let first = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some(first.2.clone())
}

fn detect_auth(spec: &Value, refs: &Refs) -> Result<Vec<Value>, String> {
    let schemes = get(spec, "components").and_then(|c| get_obj(c, "securitySchemes"));
    let mut out = Vec::new();
    if let Some(schemes) = schemes {
        for (scheme_name, raw) in schemes {
            let s = refs
                .resolve(Some(raw))?
                .cloned()
                .unwrap_or(Value::Object(obj()));
            let kind = get_str(&s, "type").unwrap_or("").to_lowercase();
            let name = Value::from(scheme_name.as_str());
            let entry = match kind.as_str() {
                "http" => {
                    let scheme = get_str(&s, "scheme").unwrap_or("").to_lowercase();
                    let kind = if scheme == "basic" { "basic" } else { "bearer" };
                    crate::json::object(vec![
                        ("kind", Value::from(kind)),
                        ("header", Value::from("Authorization")),
                        (
                            "prefix",
                            Value::from(if kind == "basic" { "Basic " } else { "Bearer " }),
                        ),
                        ("scheme_name", name),
                        ("source", Value::from("spec")),
                    ])
                }
                "apikey" => {
                    let mut e = crate::json::object(vec![
                        ("kind", Value::from("api_key")),
                        ("scheme_name", name),
                        ("source", Value::from("spec")),
                    ]);
                    if get_str(&s, "in") == Some("query") {
                        crate::json::set(
                            &mut e,
                            "query_param",
                            get(&s, "name").cloned().unwrap_or(Value::Null),
                        );
                    } else {
                        crate::json::set(
                            &mut e,
                            "header",
                            Value::from(get_str(&s, "name").unwrap_or("Authorization")),
                        );
                    }
                    e
                }
                "oauth2" | "openidconnect" => {
                    let mut e = crate::json::object(vec![
                        ("kind", Value::from("oauth2")),
                        ("header", Value::from("Authorization")),
                        ("prefix", Value::from("Bearer ")),
                        ("scheme_name", name),
                        ("source", Value::from("spec")),
                    ]);
                    if let Some(flows) = oauth2_flows(&s) {
                        crate::json::set(&mut e, "oauth2", flows);
                    }
                    e
                }
                _ => crate::json::object(vec![
                    ("kind", Value::from("unknown")),
                    ("scheme_name", name),
                    ("source", Value::from("spec")),
                ]),
            };
            out.push(entry);
        }
    }
    if out.is_empty() {
        out.push(crate::json::object(vec![
            ("kind", Value::from("unknown")),
            ("source", Value::from("spec")),
        ]));
    }
    Ok(out)
}

/// The flows an `oauth2` scheme declares, and the authorization-code flow in
/// full: its endpoints and every scope with its description, verbatim; the
/// other flows are named so a reader can see what the document offered. `None` when the scheme
/// declares no flow at all (an `openIdConnect` scheme, or a bare
/// `type: oauth2`).
fn oauth2_flows(scheme: &Value) -> Option<Value> {
    let flows = get_obj(scheme, "flows")?;
    let mut names: Vec<Value> = Vec::new();
    let mut auth_code: Option<Value> = None;
    for (raw, flow) in flows {
        let name = match raw.as_str() {
            "authorizationCode" => "authorization_code",
            "clientCredentials" => "client_credentials",
            "implicit" => "implicit",
            "password" => "password",
            "deviceCode" | "urn:ietf:params:oauth:grant-type:device_code" => "device_code",
            _ => continue,
        };
        names.push(Value::from(name));
        if name != "authorization_code" {
            continue;
        }
        let (authorization_url, token_url) = match (
            get(flow, "authorizationUrl").map(value_to_string),
            get(flow, "tokenUrl").map(value_to_string),
        ) {
            (Some(a), Some(t)) if !a.is_empty() && !t.is_empty() => (a, t),
            // Without both endpoints there is no flow to run; the name
            // alone records that the document claimed one.
            _ => continue,
        };
        let mut block = crate::json::object(vec![
            ("authorization_url", Value::from(authorization_url)),
            ("token_url", Value::from(token_url)),
        ]);
        if let Some(r) = get(flow, "refreshUrl")
            .map(value_to_string)
            .filter(|r| !r.is_empty())
        {
            crate::json::set(&mut block, "refresh_url", Value::from(r));
        }
        let scopes: Vec<Value> = get_obj(flow, "scopes")
            .into_iter()
            .flatten()
            .map(|(scope, description)| {
                crate::json::object(vec![
                    ("name", Value::from(scope.as_str())),
                    (
                        "description",
                        match description {
                            Value::String(d) if !d.is_empty() => Value::from(d.as_str()),
                            _ => Value::Null,
                        },
                    ),
                ])
            })
            .collect();
        crate::json::set(&mut block, "scopes", Value::Array(scopes));
        auth_code = Some(block);
    }
    if names.is_empty() {
        return None;
    }
    let mut out = crate::json::object(vec![("flows", Value::Array(names))]);
    if let Some(block) = auth_code {
        crate::json::set(&mut out, "authorization_code", block);
    }
    Some(out)
}

/// A security requirement (`security:` on the document or an operation),
/// verbatim, when it is well formed: a list of alternatives, each a map
/// from scheme name to a list of scopes. Anything else is not a requirement
/// the reader can vouch for and is left out.
fn security_requirement(v: Option<&Value>) -> Option<Value> {
    let alternatives = v?.as_array()?;
    let mut out = Vec::new();
    for alt in alternatives {
        let map = alt.as_object()?;
        let mut entry = obj();
        for (scheme, scopes) in map {
            let scopes: Vec<Value> = scopes
                .as_array()?
                .iter()
                .map(|s| Value::from(value_to_string(s)))
                .collect();
            entry.insert(scheme.clone(), Value::Array(scopes));
        }
        out.push(Value::Object(entry));
    }
    Some(Value::Array(out))
}

fn base_urls(spec: &Value) -> Vec<String> {
    match get_arr(spec, "servers").filter(|s| !s.is_empty()) {
        Some(servers) => servers.iter().filter_map(expand_server).collect(),
        None => vec![],
    }
}

fn expand_server(server: &Value) -> Option<String> {
    let mut url = get(server, "url").map(value_to_string).unwrap_or_default();
    if url.is_empty() {
        return None;
    }
    if let Some(vars) = get_obj(server, "variables") {
        for (name, variable) in vars {
            if let Some(d) = field(variable, "default") {
                url = url.replace(&format!("{{{}}}", name), &value_to_string(d));
            }
        }
    }
    Some(url)
}

fn synthesize_id(method: &str, path: &str) -> String {
    let parts: Vec<String> = path
        .split('/')
        .filter(|p| !p.is_empty())
        .map(|p| {
            if p.starts_with('{') {
                format!("By{}", cap(&p[1..p.len().saturating_sub(1)]))
            } else {
                cap(p)
            }
        })
        .collect();
    format!("{}{}", method.to_lowercase(), parts.join(""))
}

fn classify(
    parameters: &[Value],
    response: Option<&ContentEntryLike>,
    request_body: Option<&ContentEntryLike>,
) -> (String, Option<String>) {
    let response = match response {
        None => {
            return (
                "unsupported".into(),
                Some("no success response is documented".into()),
            )
        }
        Some(r) => r,
    };
    if response.kind == "other" {
        return (
            "unsupported".into(),
            Some(format!(
                "success response is {}; Apollo Connectors map JSON responses",
                response.content_type.clone().unwrap_or_default()
            )),
        );
    }
    if let Some(rb) = request_body {
        if rb.kind == "other" {
            return (
                "unsupported".into(),
                Some(format!(
                    "request body is {}; Apollo Connectors send JSON or form bodies",
                    rb.content_type.clone().unwrap_or_default()
                )),
            );
        }
    }
    if let Some(deep) = parameters.iter().find(|p| {
        get_str(p, "style") == Some("deepObject")
            || (get_str(p, "type") == Some("object") && get_str(p, "in") == Some("query"))
    }) {
        return (
            "needs_review".into(),
            Some(format!(
                "query parameter \"{}\" is an object; connectors cannot expand deepObject/exploded object params",
                get_str(deep, "name").unwrap_or("")
            )),
        );
    }
    if let Some(arr) = parameters.iter().find(|p| {
        get_str(p, "type") == Some("array")
            && get_str(p, "in") == Some("query")
            && get(p, "explode") == Some(&Value::Bool(true))
    }) {
        return (
            "needs_review".into(),
            Some(format!(
                "query parameter \"{}\" repeats (explode: true); check the connector's URI template renders it",
                get_str(arr, "name").unwrap_or("")
            )),
        );
    }
    ("supported".into(), None)
}

struct ContentEntryLike {
    kind: String,
    content_type: Option<String>,
}

fn convert_parameter(
    raw: &Value,
    refs: &Refs,
    shapes: &mut ShapeSet,
) -> Result<Option<Value>, String> {
    let p = match refs.resolve(Some(raw))? {
        Some(p) => p,
        None => return Ok(None),
    };
    let name = match get_str(p, "name") {
        Some(n) => n.to_string(),
        None => return Ok(None),
    };
    let location = match get_str(p, "in") {
        Some(i) => i.to_string(),
        None => return Ok(None),
    };
    let schema_node = get(p, "schema");
    let schema: Option<&Value> = match schema_node {
        Some(s) => Some(refs.resolve(Some(s))?.unwrap_or(s)),
        None => None,
    };
    let shape = match schema_node {
        Some(s) => Some(shapes.convert(Some(s), 0)?),
        None => None,
    };
    let mut out = obj();
    out.insert("name".into(), Value::from(name));
    out.insert("in".into(), Value::from(location.clone()));
    out.insert(
        "required".into(),
        Value::Bool(truthy(get(p, "required")) || location == "path"),
    );
    out.insert(
        "type".into(),
        schema
            .and_then(|s| get(s, "type"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert(
        "format".into(),
        schema
            .and_then(|s| get(s, "format"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    out.insert(
        "description".into(),
        get(p, "description").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "explode".into(),
        get(p, "explode").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "style".into(),
        get(p, "style").cloned().unwrap_or(Value::Null),
    );
    out.insert(
        "shape_ref".into(),
        shape
            .as_ref()
            .and_then(|s| get(s, "$ref"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    if let Some(e) = schema.and_then(|s| get(s, "enum")).filter(|e| e.is_array()) {
        out.insert("enum".into(), e.clone());
    }
    // Capture default/minimum/maximum from the resolved schema.
    // - default: clone verbatim (any JSON type)
    // - minimum/maximum: clone only if numeric
    if let Some(d) = schema.and_then(|s| get(s, "default")) {
        out.insert("default".into(), d.clone());
    }
    if let Some(m) = schema
        .and_then(|s| get(s, "minimum"))
        .filter(|v| v.is_number())
    {
        out.insert("minimum".into(), m.clone());
    }
    if let Some(m) = schema
        .and_then(|s| get(s, "maximum"))
        .filter(|v| v.is_number())
    {
        out.insert("maximum".into(), m.clone());
    }
    // exclusiveMinimum/exclusiveMaximum: a number (OpenAPI 3.1) or a boolean
    // qualifying minimum/maximum (3.0, Swagger 2.0), kept as written (ADR 0065).
    for key in ["exclusiveMinimum", "exclusiveMaximum"] {
        if let Some(v) = schema
            .and_then(|s| get(s, key))
            .filter(|v| v.is_number() || v.is_boolean())
        {
            out.insert(key.into(), v.clone());
        }
    }
    Ok(Some(Value::Object(out)))
}

/// The documented non-2xx responses, and for each the converted body schema
/// when it was written inline rather than as a `$ref`. The inline shape gets
/// a synthesized name once every shape is resolved, the same way an inline
/// success or request body does: dropping it would lose the fact that the
/// body has, say, a `message` field (ADR 0043).
fn error_responses(
    responses: Option<&Value>,
    refs: &Refs,
    shapes: &mut ShapeSet,
) -> Result<(Vec<Value>, Vec<Option<Value>>), String> {
    let mut out = Vec::new();
    let mut inline = Vec::new();
    let r = match responses.and_then(Value::as_object) {
        Some(r) => r,
        None => return Ok((out, inline)),
    };
    for (status, raw) in r {
        if status.starts_with('2') || status == "2XX" {
            continue;
        }
        let response = match refs.resolve(Some(raw)) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let entry = content_entry(response.and_then(|r| get(r, "content")));
        let (shape_ref, inline_shape) = match entry.as_ref().and_then(|e| get(&e.media, "schema")) {
            Some(schema) => {
                let converted = shapes.convert(Some(schema), 0)?;
                match get(&converted, "$ref").cloned() {
                    Some(r) => (r, None),
                    None => (Value::Null, Some(converted)),
                }
            }
            None => (Value::Null, None),
        };
        out.push(crate::json::object(vec![
            ("status", Value::from(status.as_str())),
            (
                "description",
                response
                    .and_then(|r| get(r, "description"))
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            ("shape_ref", shape_ref),
        ]));
        inline.push(inline_shape);
    }
    Ok((out, inline))
}

fn unique_shape_name(shapes: &Object, base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while shapes.contains_key(&name) {
        name = format!("{}{}", base, n);
        n += 1;
    }
    name
}

/// Case-normalized name for the entity-link cross-reference (ADR 0033):
/// letters and digits only, lowercased, so `petId` and `pet_id` compare
/// equal — the snake_case/camelCase fold PR #9's TypeScript generator later
/// got bitten by.
pub(crate) fn normalize_link_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A parameter record's or shape property's type family for entity-link
/// matching: a family match (string/ID-shaped vs integer-shaped), never
/// exact GraphQL scalar equality. Reads `type` — a string, or OpenAPI 3.1's
/// `["integer", "null"]` — and, when there is none, folds an `anyOf`/`oneOf`
/// with exactly one branch that is not `{"type": "null"}` to that branch's
/// family: the FastAPI nullable form (`anyOf: [{integer}, {null}]`), which
/// is how AppWorld Spotify writes 12 of the 36 foreign keys its benchmark
/// harness wires (5 at a shape's top level, 7 on list items). A real union
/// (string | integer) has no family. `None` for anything else (boolean,
/// array, object, or no declared type), which never candidates.
pub(crate) fn link_type_family(node: &Value) -> Option<&'static str> {
    if let Some(t) = get(node, "type") {
        return family_of_type(t);
    }
    let branches = get_arr(node, "anyOf").or_else(|| get_arr(node, "oneOf"))?;
    let mut typed = branches
        .iter()
        .filter(|b| get_str(b, "type") != Some("null"));
    let only = typed.next()?;
    if typed.next().is_some() {
        return None;
    }
    link_type_family(only)
}

/// The family of a bare `type` value: `"string"`, `"integer"`/`"number"`,
/// or the first non-`"null"` entry of a 3.1 type array.
fn family_of_type(t: &Value) -> Option<&'static str> {
    let first = match t {
        Value::String(s) => Some(s.as_str()),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .find(|s| *s != "null"),
        _ => None,
    }?;
    match first {
        "string" => Some("string"),
        "integer" | "number" => Some("integer"),
        _ => None,
    }
}

/// The by-id operation a relationship field can resolve through (ADR 0069):
/// `GET`, exactly one *distinct* `in: path` parameter name, that parameter
/// as the path's last segment, a response shape, and no other required
/// parameter outside the headers (a required header is the credential, not
/// an argument). Counting distinct names, not entries, matters: OpenAPI lets
/// an operation re-declare a path-item-level parameter to override it, and
/// the parser keeps both copies (`shared.iter().chain(own.iter())`, below),
/// so a genuine GET-by-id can carry the same `in: path` name twice.
/// Returns the parameter's wire name. `/orders/{orderId}/receipt`,
/// `/orgs/{org}/repos/{repo}` and every DELETE are not targets.
pub(crate) fn is_canonical_get_by_id(record: &Value) -> Option<String> {
    if get_str(record, "method") != Some("GET") {
        return None;
    }
    get(record, "response").and_then(|r| get_str(r, "shape_ref"))?;
    let path = get_str(record, "path")?.trim_end_matches('/');
    let mut path_params: HashSet<&str> = HashSet::new();
    for p in get_arr(record, "parameters")? {
        let location = get_str(p, "in").unwrap_or("");
        if location == "path" {
            if let Some(name) = get_str(p, "name") {
                path_params.insert(name);
            }
        } else if location != "header" && truthy(get(p, "required")) {
            return None;
        }
    }
    if path_params.len() != 1 {
        return None;
    }
    let name = path_params.into_iter().next()?;
    if !path.ends_with(&format!("{{{}}}", name)) {
        return None;
    }
    Some(name.to_string())
}

/// Why a canonical GET-by-id operation is still no link target (ADR 0085):
/// the one record its success response describes must carry the key its
/// trailing path parameter names, or the operation reads something *about*
/// the key rather than the record the key identifies. `None` when it does.
///
/// The record is the response shape's top level, or — when that is an
/// envelope of exactly one property holding an object (inline, or a `$ref`
/// to a shape with `properties`) — that object: `{"pet": {...}}`. It
/// carries the key when a property's case-normalized name is the
/// parameter's or ends with it (`song_review_id` for `{review_id}`: the
/// record qualifies its own key), or when the parameter's normalized name
/// ends in `id` and the record has an `id` (`/pets/{petId}` returning
/// `Pet {id}`); the property must be in the parameter's type family. A
/// root-array response has no one record and is refused.
///
/// Measured (inventory build, before -> after): AppWorld splitwise 56 -> 21,
/// every one of the 35 refused facts an `email` property pointed at
/// `get:/splitwise/balance/person/{email}`, whose response is the balance
/// with that person (`direction`, `total`, `breakdown`) and which answers
/// 422 for the caller's own email; gitea 8 -> 0 (3 `user_id ->
/// get:/activitypub/user-id/{user-id}`, whose `ActivityPub` carries only
/// `@context`; 4 `username -> get:/users/{username}`, whose `User` spells
/// the key `login`; 1 `owner -> get:/packages/{owner}`, a list). Spotify
/// 36, Amazon 35 and the other six AppWorld apps are unchanged, and every
/// harness relationship stays a fact.
pub(crate) fn link_target_refusal(
    record: &Value,
    parameter: &str,
    shapes: &Object,
) -> Option<String> {
    let response = get(record, "response");
    let shape = response
        .and_then(|r| get_str(r, "shape_ref"))
        .and_then(|r| shapes.get(shape_name(r)));
    let shape = match shape {
        Some(s) => s,
        None => return Some("its response has no shape".to_string()),
    };
    if response.and_then(|r| get(r, "root_is_array")) == Some(&Value::Bool(true))
        || get(shape, "items").is_some()
    {
        return Some("its response is a list, not one record".to_string());
    }
    let family = get_arr(record, "parameters")
        .into_iter()
        .flatten()
        .find(|p| get_str(p, "in") == Some("path") && get_str(p, "name") == Some(parameter))
        .and_then(link_type_family);
    let key = normalize_link_name(parameter);
    let carries = |props: &Object| {
        props.iter().any(|(name, prop)| {
            let n = normalize_link_name(name);
            (n.ends_with(&key) || (n == "id" && key.len() > 2 && key.ends_with("id")))
                && link_type_family(prop).is_some()
                && link_type_family(prop) == family
        })
    };
    let top = get_obj(shape, "properties");
    if top.is_some_and(|p| carries(p)) {
        return None;
    }
    // A one-property envelope: `{"pet": {...}}` or `{"pet": {"$ref": ...}}`.
    if let Some(props) = top.filter(|p| p.len() == 1) {
        let inner = props.values().next().expect("one property");
        let inner = match get_str(inner, "$ref") {
            Some(r) => shapes.get(shape_name(r)),
            None => Some(inner),
        };
        if inner
            .and_then(|i| get_obj(i, "properties"))
            .is_some_and(|p| carries(p))
        {
            return None;
        }
    }
    let id_too = if key.len() > 2 && key.ends_with("id") {
        " nor an `id`"
    } else {
        ""
    };
    Some(format!(
        "its response carries no `{}`{} of the parameter's type: it describes something other than the record the key identifies",
        parameter, id_too
    ))
}

/// A path parameter whose name says nothing about what it identifies. On
/// gitea, 26 `id` and 18 `name` hits out of the original 66 were collisions
/// against `get:/admin/hooks/{id}` and `get:/gitignore/templates/{name}`;
/// after the canonical-target rule and the per-shape self-link guard, with
/// pass 2 walking every shape a response reaches through `$ref`, this rule
/// takes the remaining 62 facts to 8 (the 54 it drops: 30 `name ->
/// get:/gitignore/templates/{name}`, 20 `id -> get:/admin/hooks/{id}`, 4
/// `id -> get:/notifications/threads/{id}`). On pagerduty the walk would
/// otherwise add 53 bare-`id` hits, every one `-> get:/escalation_policies/{id}`.
/// This is the named limitation of ADR 0069: a bare-`{id}` API gets few or
/// no hints.
pub(crate) fn is_generic_link_name(normalized: &str) -> bool {
    matches!(normalized, "id" | "name")
}

/// The parameters that identify a shape: the case-normalized trailing `{p}`
/// of every operation whose response is that shape. A property named after
/// one of them is the record's own id, never a foreign key — and it must be
/// judged per shape, because a component returned by a POST and by its
/// GET-by-id (gitea `Hook`) is visited from both. Measured on gitea with the
/// generic-name guard switched off, this removes 16 of the 78 facts the
/// other rules leave (78 -> 62): 14 `id` and 2 `name`, each a shape's own
/// trailing parameter that the weaker "target does not return the host
/// shape" check refused at the shape's own GET-by-id and that then matched
/// the next canonical operation with that name (13 of them
/// `get:/admin/hooks/{id}`). With the generic guard on it changes no count on
/// the pilots or AppWorld; what it alone catches is a non-generic own id
/// whose identifying operation is not itself a target (`Commit.sha` returned
/// by `get:/repos/{repo}/commits/{sha}` against a canonical `get:/blobs/{sha}`).
/// A shape only a `$ref` from a response reaches has no operation returning
/// it, so its set is empty.
fn shapes_own_params(operations: &[Pending], target: &str) -> HashSet<String> {
    let mut own = HashSet::new();
    for pending in operations {
        let returns_target = get(&pending.record, "response")
            .and_then(|r| get_str(r, "shape_ref"))
            .map(shape_name)
            == Some(target);
        if !returns_target {
            continue;
        }
        let path = match get_str(&pending.record, "path") {
            Some(p) => p.trim_end_matches('/'),
            None => continue,
        };
        for param in get_arr(&pending.record, "parameters").into_iter().flatten() {
            if get_str(param, "in") != Some("path") {
                continue;
            }
            if let Some(name) = get_str(param, "name") {
                if path.ends_with(&format!("{{{}}}", name)) {
                    own.insert(normalize_link_name(name));
                }
            }
        }
    }
    own
}

/// How deep the relationship walk descends through inline `properties` and
/// `items` below a host shape (ADR 0069). Twelve is the ceiling
/// `defaults.max_depth` lets a schema reach, so nothing deeper could become
/// a field; a `$ref` is never followed, so the only thing this bounds is a
/// pathologically nested inline document. `pub(crate)` so `inventory links`
/// reads with the same walk instead of a second copy of it.
pub(crate) const LINK_WALK_DEPTH: usize = 12;

/// Visit every property node reachable from one host shape without
/// crossing a `$ref` (ADR 0069): `properties.<p>` (path segment `<p>`),
/// `items` (segment `[]` at the root, or `[]` appended to the enclosing
/// property's segment), and the inline `properties`/`items` of each. `f`
/// sees the property node and its path from the shape's root in the fields
/// grammar — `album_id`, `[]>album_id` under a root-array shape,
/// `songs[]>album_id` under an inline list, `owner>account_id` under an
/// inline object. A `$ref` is not descended: the named shape it points at
/// is walked in its own right when a response reaches it, because pass 2
/// walks the `$ref`-closure of every response shape
/// (`response_shape_closure`), and following it here is what would loop on
/// the `Widget <-> User` cycle every fixture carries. With `$ref`s never
/// followed, an inline tree is finite; `seen` (one fresh set per shape,
/// keyed by the node's path) makes every path visit once regardless, and
/// the depth cap bounds a pathological inline document. The one walk the
/// builder and `inventory links` share.
pub(crate) fn walk_link_hosts(
    node: &mut Value,
    path: &mut Vec<String>,
    seen: &mut HashSet<String>,
    depth: usize,
    f: &mut impl FnMut(&mut Value, &[String]),
) {
    if depth > LINK_WALK_DEPTH {
        return;
    }
    if node.get("$ref").is_some() {
        return;
    }
    // The cycle guard: a node path is entered once. `""` is the shape root.
    if !seen.insert(path.join(">")) {
        return;
    }
    if let Some(props) = node.get_mut("properties").and_then(Value::as_object_mut) {
        for (name, prop) in props.iter_mut() {
            path.push(name.clone());
            f(prop, &path[..]);
            walk_link_hosts(prop, path, seen, depth + 1, f);
            path.pop();
        }
    }
    if let Some(items) = node.get_mut("items") {
        // `songs` -> `songs[]`; a root-array shape has no enclosing
        // property, so the segment is a bare `[]`.
        let pushed = match path.last_mut() {
            Some(last) => {
                last.push_str("[]");
                false
            }
            None => {
                path.push("[]".to_string());
                true
            }
        };
        walk_link_hosts(items, path, seen, depth + 1, f);
        if pushed {
            path.pop();
        } else if let Some(last) = path.last_mut() {
            let n = last.len() - 2;
            last.truncate(n);
        }
    }
}

/// The shapes pass 2 walks (ADR 0069, R30): every operation's response
/// shape, then every named shape reachable from one through `$ref` at any
/// depth of its tree — `properties.<p>.$ref`, `items.$ref`, a nested
/// inline node's `$ref` — and on through each shape so reached, each name
/// once. A components-based spec's list endpoint is `{type: array, items:
/// {$ref: Item}}`, so without this an item component no operation returns
/// directly would never be walked (gitea: 57 such shapes, 5 facts).
/// Request and error bodies are not seeds: a shape reached only from them
/// describes nothing a response carries, so it is no host. This is what
/// makes `walk_link_hosts` never following a `$ref` sound. The collection
/// is `inventory::shape_closure`, the one `$ref`-closure `inventory
/// describe` already uses (a cycle ends at the first repeat); its order is
/// deterministic and changes no fact, since a shape's facts depend on that
/// shape alone.
fn response_shape_closure(operations: &[Pending], resolved_shapes: &Object) -> Vec<String> {
    let roots = operations.iter().filter_map(|pending| {
        get(&pending.record, "response")
            .and_then(|r| get_str(r, "shape_ref"))
            .map(|r| shape_name(r).to_string())
    });
    crate::inventory::shape_closure(roots, resolved_shapes)
}

/// The relationship fact (ADR 0033, tightened by ADR 0069 and ADR 0085).
/// Pass 1 collects the trailing path parameter of every canonical GET-by-id
/// operation (method GET, one `in: path` parameter that ends the path, a
/// response shape, no other required non-header parameter) whose response
/// is one record carrying that key (`link_target_refusal`; the refused ones
/// are what `inventory links` lists under "refused as link targets"). Pass 2 walks every
/// response shape and every named shape a response reaches through `$ref`
/// (`response_shape_closure`), each once — its top-level properties, a
/// root array's items, and every inline list or object below them, the
/// walk itself never crossing a `$ref` — and records a
/// `candidate_entity_link` fact on each property whose case-normalized
/// name and type family match one of those parameters, unless the name is
/// a bare `id`/`name`, the parameter belongs to an operation returning
/// this very shape, or the property is one of this shape's own path
/// parameters (a shape only a `$ref` reaches has none). The fact says
/// which operation/parameter matched and whether that operation returns a
/// list (`list_context`: false on every fact since ADR 0085 refuses a
/// target whose response is a list; kept because the contract carries it).
/// Guidance only: nothing here is enforced or linted; the judgement lives
/// in selection.yaml `links:` (Phase 7as (b)). Comparisons use raw wire
/// names (pre-rename), so a later GraphQL rename never affects them.
///
/// At the operation counts this repo builds, a plain scan over every
/// parameter and every shape property is fine — nothing cleverer belongs
/// here.
fn attach_candidate_entity_links(operations: &[Pending], resolved_shapes: &mut Object) {
    struct PathParam {
        operation: String,
        parameter: String,
        family: &'static str,
        list_context: bool,
        response_shape: Option<String>,
    }

    let mut path_params: Vec<PathParam> = Vec::new();
    for pending in operations {
        let op_key = match get_str(&pending.record, "key") {
            Some(k) => k.to_string(),
            None => continue,
        };
        let parameter = match is_canonical_get_by_id(&pending.record) {
            Some(p) => p,
            None => continue,
        };
        if is_generic_link_name(&normalize_link_name(&parameter)) {
            continue;
        }
        if link_target_refusal(&pending.record, &parameter, resolved_shapes).is_some() {
            continue;
        }
        let param = get_arr(&pending.record, "parameters")
            .into_iter()
            .flatten()
            .find(|p| get_str(p, "name") == Some(parameter.as_str()));
        let family = match param.and_then(link_type_family) {
            Some(f) => f,
            None => continue,
        };
        let list_context = get(&pending.record, "response").and_then(|r| get(r, "root_is_array"))
            == Some(&Value::Bool(true));
        let response_shape = get(&pending.record, "response")
            .and_then(|r| get_str(r, "shape_ref"))
            .map(|r| shape_name(r).to_string());
        path_params.push(PathParam {
            operation: op_key,
            parameter,
            family,
            list_context,
            response_shape,
        });
    }

    // Pass 2: every distinct response shape and every named shape a
    // response reaches through `$ref`, once each. The fact is per shape,
    // not per visiting operation (Task 1's own-parameter check is per
    // shape for the same reason), and the walk covers the shape's whole
    // inline tree — root-array items, nested lists, nested objects — so a
    // foreign key on a list item is found where the benchmark found it.
    for target in response_shape_closure(operations, resolved_shapes) {
        let own = shapes_own_params(operations, &target);
        let shape = match resolved_shapes.get_mut(&target) {
            Some(s) => s,
            None => continue,
        };
        let mut path: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        walk_link_hosts(
            shape,
            &mut path,
            &mut seen,
            0,
            &mut |prop: &mut Value, path: &[String]| {
                let prop_family = match link_type_family(prop) {
                    Some(f) => f,
                    None => return,
                };
                // Defensive only: pass 2 visits each shape once (the closure
                // names each once), so this guards a shape that already
                // carried a fact before the walk reached it.
                if prop.get("candidate_entity_link").is_some() {
                    return;
                }
                let prop_name = match path.last() {
                    Some(p) => p,
                    None => return,
                };
                let normalized = normalize_link_name(prop_name);
                if is_generic_link_name(&normalized) || own.contains(&normalized) {
                    return;
                }
                let hit = path_params.iter().find(|pp| {
                    pp.response_shape.as_deref() != Some(target.as_str())
                        && pp.family == prop_family
                        && normalize_link_name(&pp.parameter) == normalized
                });
                let pp = match hit {
                    Some(pp) => pp,
                    None => return,
                };
                crate::json::set(
                    prop,
                    "candidate_entity_link",
                    crate::json::object(vec![
                        ("operation", Value::from(pp.operation.clone())),
                        ("parameter", Value::from(pp.parameter.clone())),
                        ("list_context", Value::Bool(pp.list_context)),
                    ]),
                );
            },
        );
    }
}

pub struct Built {
    pub inventory: Value,
    pub warnings: Vec<String>,
    /// Component reference (`#/components/schemas/Widget List`) -> the
    /// inventory shape name it became (`Widget_List`), for anything that
    /// has to map a spec pointer back onto a shape.
    pub shape_names: HashMap<String, String>,
}

struct Pending {
    record: Value,
    response_shape: Option<Value>,
    request_shape: Option<Value>,
    /// One per `errors` entry, in order: the inline body shape still to be
    /// named, or `None` for a `$ref` or an undocumented body.
    error_shapes: Vec<Option<Value>>,
}

/// Build an inventory from an OpenAPI document.
pub fn build_inventory(spec: &Value) -> Result<Built, String> {
    let refs = Refs { spec };
    let mut shapes = ShapeSet::new(&refs);
    let mut warnings = Vec::new();
    let mut operations: Vec<Pending> = Vec::new();
    let mut unresolved: Vec<Value> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();

    let unresolved_entry = |hint: &str, reason: &str| {
        crate::json::object(vec![
            ("hint", Value::from(hint)),
            ("source", Value::from("spec")),
            ("reason", Value::from(reason)),
        ])
    };

    if let Some(paths) = get_obj(spec, "paths") {
        for (path, raw_item) in paths {
            let item = match refs.resolve(Some(raw_item)) {
                Ok(i) => i,
                Err(e) => {
                    unresolved.push(unresolved_entry(
                        path,
                        &format!("path item did not resolve: {}", e),
                    ));
                    continue;
                }
            };
            let shared: Vec<Value> = item
                .and_then(|i| get_arr(i, "parameters"))
                .cloned()
                .unwrap_or_default();
            for method in METHODS {
                let raw = match item.and_then(|i| get(i, method)) {
                    Some(r) => r,
                    None => continue,
                };
                let op = match refs.resolve(Some(raw)) {
                    Ok(Some(o)) => o,
                    Ok(None) => continue,
                    Err(e) => {
                        unresolved.push(unresolved_entry(&format!("{} {}", method, path), &e));
                        continue;
                    }
                };
                let key = format!("{}:{}", method, path);
                if seen_keys.contains(&key) {
                    warnings.push(format!(
                        "duplicate operation key {}; the later definition was ignored",
                        key
                    ));
                    continue;
                }
                seen_keys.insert(key.clone());

                // Everything that can fail on a broken $ref is inside this closure.
                let own: Vec<Value> = get_arr(op, "parameters").cloned().unwrap_or_default();
                let attempt = (|| -> Result<(Vec<Value>, Option<Value>, Option<Value>, Option<(String, Option<ContentEntry>)>, Vec<String>), String> {
                    let mut parameters: Vec<Value> = Vec::new();
                    for p in shared.iter().chain(own.iter()) {
                        if let Some(c) = convert_parameter(p, &refs, &mut shapes)? {
                            parameters.push(c);
                        }
                    }
                    // Inputs the model cannot carry are named, never dropped
                    // silently: parts the Swagger reader could not fold into the
                    // body, and parameters at a location OpenAPI 3 does not
                    // define (`body`, `formData` in an OpenAPI 3 document).
                    let mut unmodelled: Vec<String> = crate::json::strings(get(op, "x-factory-unmodelled-parameters"));
                    parameters.retain(|p| {
                        let loc = get_str(p, "in").unwrap_or("");
                        if matches!(loc, "path" | "query" | "header" | "cookie") {
                            true
                        } else {
                            unmodelled.push(format!("{} ({}: not an OpenAPI 3 location)", get_str(p, "name").unwrap_or("?"), loc));
                            false
                        }
                    });

                    let mut request_body: Option<Value> = None;
                    let mut request_shape: Option<Value> = None;
                    let rb = match get(op, "requestBody") {
                        Some(r) => refs.resolve(Some(r))?,
                        None => None,
                    };
                    let rb_entry = rb.and_then(|r| content_entry(get(r, "content")));
                    if let Some(entry) = rb_entry {
                        request_shape = match get(&entry.media, "schema") {
                            Some(s) => Some(shapes.convert(Some(s), 0)?),
                            None => None,
                        };
                        request_body = Some(crate::json::object(vec![
                            ("content_type", Value::from(entry.content_type.clone())),
                            ("required", Value::Bool(truthy(rb.and_then(|r| get(r, "required"))))),
                            ("shape_ref", request_shape.as_ref().and_then(|s| get(s, "$ref")).cloned().unwrap_or(Value::Null)),
                            ("kind", Value::from(entry.kind)),
                        ]));
                    }

                    let mut response_entry: Option<(String, Option<ContentEntry>)> = None;
                    if let Some((status, response)) = success_response(get(op, "responses"), &refs)? {
                        let entry = content_entry(get(response, "content"));
                        response_entry = Some((status, entry));
                    }
                    Ok((parameters, request_body, request_shape, response_entry, unmodelled))
                })();

                let (parameters, request_body, request_shape, response_entry, unmodelled) =
                    match attempt {
                        Ok(x) => x,
                        Err(e) => {
                            unresolved.push(unresolved_entry(&key, &e));
                            continue;
                        }
                    };

                let response_shape = match response_entry
                    .as_ref()
                    .and_then(|(_, e)| e.as_ref())
                    .and_then(|e| get(&e.media, "schema"))
                {
                    Some(schema) => Some(shapes.convert(Some(schema), 0)?),
                    None => None,
                };

                let (errors, error_shapes) =
                    error_responses(get(op, "responses"), &refs, &mut shapes)?;

                let mut record = obj();
                record.insert("key".into(), Value::from(key.clone()));
                record.insert(
                    "operation_id".into(),
                    get(op, "operationId")
                        .cloned()
                        .unwrap_or_else(|| Value::from(synthesize_id(method, path))),
                );
                record.insert("method".into(), Value::from(method.to_uppercase()));
                record.insert("path".into(), Value::from(path.as_str()));
                record.insert(
                    "summary".into(),
                    get(op, "summary").cloned().unwrap_or(Value::Null),
                );
                record.insert(
                    "description".into(),
                    get(op, "description").cloned().unwrap_or(Value::Null),
                );
                record.insert(
                    "deprecated".into(),
                    Value::Bool(truthy(get(op, "deprecated"))),
                );
                record.insert(
                    "tags".into(),
                    get(op, "tags")
                        .filter(|t| t.is_array())
                        .cloned()
                        .unwrap_or(Value::Array(vec![])),
                );
                // A POST is a write until the engineer decides otherwise: a
                // write mistaken for a read escapes every control that gates
                // writes, while a read mistaken for a write only changes the
                // schema's root (ADR 0015). What the name suggests is recorded
                // beside it as `read_hint`, for the agent to surface and the
                // user to confirm. Only the spec's own operationId feeds the
                // hint: a synthesized one (`postAccountsByListIdMembers`)
                // would carry the path's parameter names.
                record.insert(
                    "semantics".into(),
                    Value::from(match method {
                        "get" | "head" => "read",
                        _ => "write",
                    }),
                );
                if method == "post" {
                    if let Some(hint) = read_hint(get_str(op, "operationId").unwrap_or(""), path) {
                        record.insert("read_hint".into(), Value::from(hint));
                    }
                }
                record.insert("provenance".into(), Value::from("spec"));
                record.insert("confidence".into(), Value::from(1));
                record.insert("parameters".into(), Value::Array(parameters.clone()));
                let rb_kind = request_body
                    .as_ref()
                    .and_then(|r| get_str(r, "kind"))
                    .map(str::to_string);
                let rb_content_type = request_body
                    .as_ref()
                    .and_then(|r| get_str(r, "content_type"))
                    .map(str::to_string);
                record.insert(
                    "request_body".into(),
                    match &request_body {
                        Some(rb) => {
                            let mut c = rb.clone();
                            crate::json::remove(&mut c, "kind");
                            c
                        }
                        None => Value::Null,
                    },
                );
                record.insert("response".into(), Value::Null);
                // The operation's own requirement only: an absent key means
                // api.security applies, `[]` means the document says the
                // operation takes no credential.
                if let Some(security) = security_requirement(get(op, "security")) {
                    record.insert("security".into(), security);
                }
                record.insert("errors".into(), Value::Array(errors));
                record.insert("support".into(), Value::from("supported"));
                record.insert("support_reason".into(), Value::Null);

                if let Some((status, entry)) = &response_entry {
                    record.insert(
                        "response".into(),
                        crate::json::object(vec![
                            ("status", Value::from(status.as_str())),
                            (
                                "content_type",
                                entry
                                    .as_ref()
                                    .map(|e| Value::from(e.content_type.clone()))
                                    .unwrap_or(Value::Null),
                            ),
                            (
                                "shape_ref",
                                response_shape
                                    .as_ref()
                                    .and_then(|s| get(s, "$ref"))
                                    .cloned()
                                    .unwrap_or(Value::Null),
                            ),
                        ]),
                    );
                }

                let response_like = response_entry.as_ref().map(|(_, e)| ContentEntryLike {
                    kind: e
                        .as_ref()
                        .map(|e| e.kind.to_string())
                        .unwrap_or_else(|| "empty".to_string()),
                    content_type: e.as_ref().map(|e| e.content_type.clone()),
                });
                let request_like = request_body.as_ref().map(|_| ContentEntryLike {
                    kind: rb_kind.unwrap_or_else(|| "json".into()),
                    content_type: rb_content_type,
                });
                let (mut support, mut reason) =
                    classify(&parameters, response_like.as_ref(), request_like.as_ref());
                if !unmodelled.is_empty() {
                    let note = format!(
                        "parameter(s) not modelled: {} — check what the API actually accepts",
                        unmodelled.join(", ")
                    );
                    warnings.push(format!("{}: {}", key, note));
                    if support == "supported" {
                        support = "needs_review".to_string();
                        reason = Some(note);
                    }
                }
                record.insert("support".into(), Value::from(support));
                record.insert(
                    "support_reason".into(),
                    reason.map(Value::from).unwrap_or(Value::Null),
                );

                operations.push(Pending {
                    record: Value::Object(record),
                    response_shape,
                    request_shape,
                    error_shapes,
                });
            }
        }
    }

    shapes.drain()?;
    let mut resolved_shapes = std::mem::take(&mut shapes.shapes);

    for pending in operations.iter_mut() {
        let record = &mut pending.record;
        let operation_id = get_str(record, "operation_id").unwrap_or("").to_string();
        // This operation's own pagination: the query parameters it declares
        // and the next-cursor key its response carries. `api.pagination`
        // below summarises the whole API; an operation can differ from it.
        let own_params: HashSet<String> = get_arr(record, "parameters")
            .into_iter()
            .flatten()
            .filter(|p| get_str(p, "in") == Some("query"))
            .filter_map(|p| get_str(p, "name").map(str::to_string))
            .collect();
        let own_pagination = pagination_for(
            &own_params,
            &[pending.response_shape.as_ref()],
            &resolved_shapes,
        );
        // No block for an operation that declares no paging parameter: a
        // missing `pagination` reads as "not paginated", and 366 of Gitea's
        // 467 operations would otherwise carry an empty one. `unknown` (a
        // size parameter with no page or cursor) is kept — worth a look.
        if get_str(&own_pagination, "style") != Some("none") {
            crate::json::set(record, "pagination", own_pagination);
        }
        if let Some(response_shape) = &pending.response_shape {
            if get(record, "response").is_some() {
                // This operation's own documented non-2xx shapes, resolved
                // (inline ones from `pending.error_shapes`, named ones by
                // their already-set `shape_ref`) -- status-code evidence
                // for `success_shape::resolve` below (ADR 0080).
                let op_error_shapes: Vec<(String, Value)> = get_arr(record, "errors")
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .filter_map(|(i, e)| {
                        let status = get_str(e, "status")?.to_string();
                        let shape = pending
                            .error_shapes
                            .get(i)
                            .and_then(|s| s.clone())
                            .or_else(|| {
                                get_str(e, "shape_ref")
                                    .map(|r| crate::json::object(vec![("$ref", Value::from(r))]))
                            })?;
                        Some((status, shape))
                    })
                    .collect();
                // Property-name matching alone is never sufficient evidence
                // (item 6): only an unambiguous status-code correlation or a
                // shared boolean discriminator promotes a branch. Ambiguous
                // unions are left exactly as found -- no fact, no guess.
                let resolution = crate::success_shape::resolve(
                    response_shape,
                    &resolved_shapes,
                    &op_error_shapes,
                );
                let effective_shape = resolution
                    .as_ref()
                    .map(|r| &r.branch)
                    .unwrap_or(response_shape);
                // The facts the envelope judgement rests on, never the
                // judgement itself: that lives in selection.yaml (ADR 0018).
                let facts =
                    crate::envelope::response_facts(Some(effective_shape), &resolved_shapes);
                let response = record.get_mut("response").unwrap();
                if get(response, "shape_ref").is_none() {
                    let inline = unique_shape_name(
                        &resolved_shapes,
                        &format!("{}Response", cap(&operation_id)),
                    );
                    resolved_shapes.insert(inline.clone(), response_shape.clone());
                    crate::json::set(
                        response,
                        "shape_ref",
                        Value::from(format!("#/shapes/{}", inline)),
                    );
                }
                for (key, value) in facts {
                    crate::json::set(response, key, value);
                }
                if let Some(res) = resolution {
                    let ref_str = match get_str(&res.branch, "$ref") {
                        Some(r) => r.to_string(),
                        None => {
                            let inline = unique_shape_name(
                                &resolved_shapes,
                                &format!("{}SuccessResponse", cap(&operation_id)),
                            );
                            resolved_shapes.insert(inline.clone(), res.branch.clone());
                            format!("#/shapes/{}", inline)
                        }
                    };
                    crate::json::set(response, "referenced_shape", Value::from(ref_str));
                }
            }
        }
        if get(record, "request_body").is_some()
            && get(record, "request_body")
                .and_then(|r| get(r, "shape_ref"))
                .is_none()
        {
            let rb = record.get_mut("request_body").unwrap();
            match &pending.request_shape {
                Some(shape) => {
                    let inline = unique_shape_name(
                        &resolved_shapes,
                        &format!("{}Request", cap(&operation_id)),
                    );
                    resolved_shapes.insert(inline.clone(), shape.clone());
                    crate::json::set(rb, "shape_ref", Value::from(format!("#/shapes/{}", inline)));
                }
                None => crate::json::remove(rb, "shape_ref"),
            }
        }
        if let Some(Value::Array(errors)) = record.get_mut("errors") {
            for (error, shape) in errors.iter_mut().zip(&pending.error_shapes) {
                let shape = match shape {
                    Some(s) if get(error, "shape_ref").map_or(true, Value::is_null) => s,
                    _ => continue,
                };
                let status = get_str(error, "status").unwrap_or("").to_string();
                let inline = unique_shape_name(
                    &resolved_shapes,
                    &format!("{}Error{}", cap(&operation_id), cap(&status)),
                );
                resolved_shapes.insert(inline.clone(), shape.clone());
                crate::json::set(
                    error,
                    "shape_ref",
                    Value::from(format!("#/shapes/{}", inline)),
                );
            }
        }

        let checks: Vec<(&str, Option<String>)> = vec![
            (
                "response",
                get(record, "response")
                    .and_then(|r| get_str(r, "shape_ref"))
                    .map(str::to_string),
            ),
            (
                "request body",
                get(record, "request_body")
                    .and_then(|r| get_str(r, "shape_ref"))
                    .map(str::to_string),
            ),
        ];
        for (where_, reference) in checks {
            let name = match reference.as_deref().map(shape_name) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if let Some(why) = shapes.broken.get(&name) {
                let reason = format!("{} schema does not resolve ({})", where_, why);
                crate::json::set(record, "support", Value::from("unsupported"));
                crate::json::set(record, "support_reason", Value::from(reason.clone()));
                let key = get_str(record, "key").unwrap_or("").to_string();
                unresolved.push(unresolved_entry(&key, &reason));
            }
        }
    }

    attach_candidate_entity_links(&operations, &mut resolved_shapes);

    let mut records: Vec<Value> = operations.into_iter().map(|p| p.record).collect();
    // A `next_link` page is fetched by another operation: record which one,
    // when the document declares exactly one (ADR 0052).
    let next_link = |r: &Value| {
        get(r, "pagination")
            .filter(|p| get_str(p, "style") == Some("next_link"))
            .and_then(|p| get_str(p, "next_url"))
            .map(str::to_string)
    };
    let routes: Vec<Route> = records
        .iter()
        .filter_map(|r| {
            Some((
                get_str(r, "method")?.to_string(),
                get_str(r, "path")?.to_string(),
                get_str(r, "key")?.to_string(),
                next_link(r),
            ))
        })
        .collect();
    for record in records.iter_mut() {
        let follow = next_link(record)
            .and_then(|link| next_link_operation(get_str(record, "path")?, &link, &routes));
        if let Some(key) = follow {
            if let Some(Value::Object(p)) = record.get_mut("pagination") {
                p.insert("next_operation".into(), Value::from(key));
            }
        }
    }
    let info = get(spec, "info");
    let mut api = obj();
    api.insert(
        "title".into(),
        info.and_then(|i| get(i, "title"))
            .cloned()
            .unwrap_or_else(|| Value::from("Untitled API")),
    );
    if let Some(v) = info
        .and_then(|i| get(i, "version"))
        .filter(|v| truthy(Some(v)))
    {
        api.insert("version".into(), Value::from(value_to_string(v)));
    }
    api.insert(
        "description".into(),
        info.and_then(|i| get(i, "description"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    let mut urls = base_urls(spec);
    if urls.is_empty() {
        urls.push("https://example.invalid".to_string());
        warnings.push("the spec declares no server URL; base_urls is a placeholder and must be set before apply".to_string());
    }
    api.insert(
        "base_urls".into(),
        Value::Array(urls.into_iter().map(Value::from).collect()),
    );
    api.insert("auth".into(), Value::Array(detect_auth(spec, &refs)?));
    if let Some(security) = security_requirement(get(spec, "security")) {
        api.insert("security".into(), security);
    }
    api.insert("pagination".into(), detect_pagination(&records));

    let inventory = crate::json::object(vec![
        ("contract_version", Value::from(1)),
        ("api", Value::Object(api)),
        ("operations", Value::Array(records)),
        ("shapes", Value::Object(resolved_shapes)),
        ("unresolved", Value::Array(unresolved)),
    ]);
    Ok(Built {
        inventory,
        warnings,
        shape_names: shapes.names.clone(),
    })
}

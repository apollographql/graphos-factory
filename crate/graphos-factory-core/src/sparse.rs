//! Sparse fieldsets: an API whose GETs take a string query parameter naming
//! the fields to return (Graph's `fields=id,name,business{name}`). A root
//! field on such an operation declares that parameter as a `String` argument
//! whose default is the list its own connector maps, and forwards it; lint
//! checks the default and the forwarding, scaffold derives the tests from the
//! declared default (ADR 0045).
//!
//! The default is **derived**, never chosen: the sorted, comma-joined wire
//! names the connector's `selection:` reads from the entity the operation
//! returns — the response object for a node GET, the `data[]` item for a
//! paged edge (pagination fact present, `data` the only array root), with
//! the page wrapper left out. A child selected under an expansion boundary
//! (`x-expansion` on the property) beyond its verified default projection
//! renders as a group, `business{name}`, recursively. Anything else the
//! derivation cannot express is `NotDerivable`, and the author records the
//! literal in a decision instead of guessing.

use crate::json::{get, get_arr, get_obj, get_str, Object};
use crate::reconcile::{
    deref, has_match_spread, is_array, merge_variants, parse_selection, Node, Spread,
};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// The query parameter a workspace names in `sparse_fieldsets.param`, or
/// `fields`.
pub fn param_name(workspace: &Value) -> String {
    get(workspace, "sparse_fieldsets")
        .and_then(|s| get_str(s, "param"))
        .unwrap_or("fields")
        .to_string()
}

/// Whether the workspace has the sparse-fieldsets rule on:
/// `sparse_fieldsets.enabled`, on when absent. A source whose `fields`
/// parameter means something else (the fields a search looks in, Google's
/// `items(id)` partial-response syntax) turns it off.
pub fn enabled(workspace: &Value) -> bool {
    get(workspace, "sparse_fieldsets")
        .and_then(|s| get(s, "enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// The operation's **string** sparse-fieldsets query parameter, when the
/// workspace has the rule on and the operation is a GET taking one.
pub fn string_param<'a>(workspace: &Value, op: &'a Value) -> Option<&'a Value> {
    if !enabled(workspace) {
        return None;
    }
    query_param(op, &param_name(workspace)).filter(|p| param_type(p) == "string")
}

/// The operation's query parameter named `param`, when the operation is a GET.
pub fn query_param<'a>(op: &'a Value, param: &str) -> Option<&'a Value> {
    if !get_str(op, "method").is_some_and(|m| m.eq_ignore_ascii_case("GET")) {
        return None;
    }
    get_arr(op, "parameters")
        .into_iter()
        .flatten()
        .find(|p| get_str(p, "in") == Some("query") && get_str(p, "name") == Some(param))
}

/// The parameter's declared type (`string`, `array`, …), or `unknown`.
pub fn param_type(p: &Value) -> String {
    get_str(p, "type")
        .or_else(|| get(p, "schema").and_then(|s| get_str(s, "type")))
        .unwrap_or("unknown")
        .to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Derived {
    Fields(String),
    NotDerivable(String),
}

/// The `fields` default an operation's connector selection implies.
pub fn derive(op: &Value, shapes: &Object, selection_text: &str) -> Derived {
    match derive_inner(op, shapes, selection_text) {
        Ok(parts) if parts.is_empty() => {
            Derived::NotDerivable("the connector selects no field".to_string())
        }
        Ok(parts) => Derived::Fields(parts.join(",")),
        Err(e) => Derived::NotDerivable(e),
    }
}

/// The array root a list response's items sit under, or `None` for a node:
/// `data` for a paged edge (a pagination fact, `data` the single array
/// root), otherwise the envelope the inventory's response facts suggest
/// when it names an array root (`crate::envelope::suggest_envelope`:
/// `{data: [...]}`, `{results: [...], count}`, `{data: [...], meta: {...}}`).
/// A fields list names the items' fields there, and an expansion group is
/// relative to an item. The one rule every sparse reader shares.
pub fn list_root(op: &Value) -> Option<String> {
    let response = get(op, "response");
    let roots: Vec<&str> = response
        .and_then(|r| get_arr(r, "array_root_properties"))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if get(op, "pagination").is_some() && roots == ["data"] {
        return Some("data".to_string());
    }
    crate::envelope::suggest_envelope(response).filter(|e| roots.contains(&e.as_str()))
}

fn shape_ref(r: &str) -> Value {
    crate::json::object(vec![("$ref", Value::from(r))])
}

fn derive_inner(op: &Value, shapes: &Object, selection_text: &str) -> Result<Vec<String>, String> {
    let response = get(op, "response");
    let root_ref = response
        .and_then(|r| get_str(r, "shape_ref"))
        .ok_or("the operation documents no response shape")?;
    let nodes = parse_selection(selection_text);
    if let Some(why) = nodes.iter().find_map(|n| match &n.spread {
        Some(Spread::Unparsed(why)) => Some(why),
        _ => None,
    }) {
        return Err(format!(
            "the connector reads {}, which a fields list cannot name",
            why
        ));
    }
    // A union or interface root is read through its merged variants, and
    // only when the selection spreads over them (ADR 0058).
    let root = if has_match_spread(&nodes) {
        merge_variants(Some(&shape_ref(root_ref)), shapes)
    } else {
        deref(Some(&shape_ref(root_ref)), shapes, &mut HashSet::new())
    }
    .ok_or("the response shape does not resolve")?;
    let array_roots: Vec<&str> = response
        .and_then(|r| get_arr(r, "array_root_properties"))
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if let Some(pagination) = get(op, "pagination") {
        if array_roots != ["data"] {
            return Err(format!(
                "the response is paged but its items are not the single array root `data` (array roots: {})",
                if array_roots.is_empty() { "none".to_string() } else { array_roots.join(", ") }
            ));
        }
        // The page wrapper: the array root and every key the pagination
        // fact reads its cursor or next link from.
        let mut wrapper: Vec<String> = vec!["data".to_string()];
        for k in ["response", "next_url"] {
            if let Some(path) = get_str(pagination, k) {
                wrapper.push(path.split('.').next().unwrap_or(path).to_string());
            }
        }
        let mut items: Option<&Vec<Node>> = None;
        for n in &nodes {
            let top = single_key(n)?;
            if top == "data" {
                items = n.children.as_ref();
            } else if !wrapper.iter().any(|w| w == &top) {
                return Err(format!(
                    "the connector selects `{}` beside the page items, which a fields list cannot request",
                    top
                ));
            }
        }
        let items = items.ok_or("the connector selects no `data` items")?;
        let data = get_obj(&root, "properties")
            .and_then(|p| p.get("data"))
            .ok_or("the response shape has no `data` property")?;
        let data = deref(Some(data), shapes, &mut HashSet::new());
        let item = data
            .as_ref()
            .filter(|d| is_array(Some(d)))
            .and_then(|d| get(d, "items"))
            .and_then(|i| deref(Some(i), shapes, &mut HashSet::new()))
            .ok_or("`data` is not an array of objects")?;
        return names(items, &item, shapes);
    }
    // A list wrapper with no pagination fact (`{data: [...]}`,
    // `{results: [...], count}`) is not a node: its fields list names the
    // items' fields, and reading it as a node would give `data`.
    if let Some(root) = list_root(op) {
        return Err(format!(
            "the response is a list under `{}` with no pagination fact, so it is neither a node nor a paged edge",
            root
        ));
    }
    if is_array(Some(&root)) || get(&root, "properties").is_none() {
        return Err(
            "the response is neither an entity object nor a single-array-root page".to_string(),
        );
    }
    names(&nodes, &root, shapes)
}

/// The one wire name a selection node reads, or why it has none.
fn single_key(n: &Node) -> Result<String, String> {
    match &n.key {
        Some(segs) if segs.len() == 1 && segs[0] != "*" => Ok(segs[0].clone()),
        Some(segs) => Err(format!(
            "the connector reads the path `{}`, which a fields list cannot name",
            segs.join(".")
        )),
        None => {
            Err("the connector selects a computed or literal value with no wire name".to_string())
        }
    }
}

/// The verified default projection of an expansion boundary: `Some(leaves)`
/// only for a non-empty list of names backed by `evidence: {kind: probe|doc,
/// ref}` (the contract's own terms), `None` otherwise. Anything else —
/// `unverified`, no evidence, `[]`, a non-string leaf — fails closed as
/// unverified: an empty or malformed list would make the relationship offer
/// nothing and vanish from `source-coverage`.
pub fn verified_default(expansion: &Value) -> Option<Vec<String>> {
    let evidence = get(expansion, "evidence")?;
    if !matches!(get_str(evidence, "kind"), Some("probe" | "doc"))
        || get_str(evidence, "ref").is_none_or(|r| r.trim().is_empty())
    {
        return None;
    }
    let leaves = get_arr(expansion, "default")?;
    let names: Vec<String> = leaves
        .iter()
        .map(|l| {
            l.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .collect::<Option<_>>()?;
    (!names.is_empty()).then_some(names)
}

/// The fields expression a root field sends upstream: the declared default
/// when `queryParams` forwards `$args.<param>` (what the router sends when
/// a caller omits the argument), or the literal when it sends `$("…")`.
/// `None` when the field sends no such parameter — then the source's
/// default projection applies to every boundary.
pub fn wire_expression(field_text: &str, param: &str) -> Option<String> {
    let sent = crate::lint::slot_expression(field_text, "queryParams", param)?;
    let sent = sent.trim();
    if sent == format!("$args.{}", param) {
        return declared_arg(field_text, param).and_then(|d| d.default);
    }
    let inner = sent.strip_prefix("$(")?.strip_suffix(')')?.trim();
    ["\"", "'"].iter().find_map(|q| {
        inner
            .strip_prefix(q)
            .and_then(|i| i.strip_suffix(q))
            .map(str::to_string)
    })
}

/// The `x-expansion` on a property or on its array items — including the
/// items of a named list shape the property references (`{$ref: LabelList}`
/// with `LabelList.items` annotated), as `source-coverage` sees it.
pub fn expansion<'a>(prop: &'a Value, shapes: &'a Object) -> Option<&'a Value> {
    let mut current = prop;
    for _ in 0..8 {
        if let Some(x) = get(current, "x-expansion")
            .or_else(|| get(current, "items").and_then(|i| get(i, "x-expansion")))
        {
            return Some(x);
        }
        current = get_str(current, "$ref")
            .and_then(|r| r.strip_prefix("#/shapes/"))
            .and_then(|name| shapes.get(name))?;
    }
    None
}

fn names(nodes: &[Node], shape: &Value, shapes: &Object) -> Result<Vec<String>, String> {
    Ok(names_by_wire(nodes, shape, shapes)?.into_values().collect())
}

/// Each wire key the nodes read, with its rendering in the fields list.
fn names_by_wire(
    nodes: &[Node],
    shape: &Value,
    shapes: &Object,
) -> Result<BTreeMap<String, String>, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let merged = has_match_spread(nodes)
        .then(|| merge_variants(Some(shape), shapes))
        .flatten();
    let shape = merged.as_ref().unwrap_or(shape);
    let props = get_obj(shape, "properties");
    // A `??`/`?!` operand that reads a path (`a: x ?? y`) must be requested
    // too, or the fallback never arrives; a literal one reads nothing.
    let reads = nodes
        .iter()
        .flat_map(|n| std::iter::once(n).chain(n.fallbacks.iter().filter(|f| f.key.is_some())));
    for n in reads {
        // A spread's arms read from this same object (ADR 0058); any other
        // spread's keys are unknowable, so no fields list can name them.
        match &n.spread {
            Some(Spread::Match(arms)) => {
                for arm in arms {
                    for (wire, rendered) in names_by_wire(&arm.children, shape, shapes)? {
                        insert_once(&mut out, wire, rendered)?;
                    }
                }
                if n.key.is_none() {
                    continue;
                }
            }
            Some(Spread::Unparsed(why)) => {
                return Err(format!(
                    "the connector reads {}, which a fields list cannot name",
                    why
                ));
            }
            None => {}
        }
        let wire = single_key(n)?;
        let prop = props
            .and_then(|p| p.get(&wire))
            .ok_or_else(|| format!("`{}` is not a property of the response shape", wire))?;
        let children = n.children.as_ref().filter(|c| !c.is_empty() && !n.opaque);
        let rendered = match (children, expansion(prop, shapes)) {
            (Some(children), Some(x)) => {
                let leaves_only = children.iter().all(|c| c.children.is_none());
                let within_default = verified_default(x).is_some_and(|d| {
                    children
                        .iter()
                        .all(|c| single_key(c).is_ok_and(|k| d.contains(&k)))
                });
                if leaves_only && within_default {
                    wire.clone()
                } else {
                    let resolved = deref(Some(prop), shapes, &mut HashSet::new());
                    let target = match resolved {
                        Some(r) if is_array(Some(&r)) => get(&r, "items")
                            .and_then(|i| deref(Some(i), shapes, &mut HashSet::new())),
                        other => other,
                    }
                    .ok_or_else(|| format!("`{}`'s target shape does not resolve", wire))?;
                    format!(
                        "{}{{{}}}",
                        wire,
                        names(children, &target, shapes)?.join(",")
                    )
                }
            }
            // An embedded value (no boundary) comes back whole.
            _ => wire.clone(),
        };
        insert_once(&mut out, wire, rendered)?;
    }
    Ok(out)
}

fn insert_once(
    out: &mut BTreeMap<String, String>,
    wire: String,
    rendered: String,
) -> Result<(), String> {
    if let Some(prev) = out.get(&wire) {
        if prev != &rendered {
            return Err(format!(
                "the connector reads `{}` twice with different children",
                wire
            ));
        }
    }
    out.insert(wire, rendered);
    Ok(())
}

/// A root field's declaration of one argument: its type and, when it has
/// one, its string default (`fields: String = "id,name"`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredArg {
    pub type_: String,
    pub default: Option<String>,
}

pub fn declared_arg(field_text: &str, name: &str) -> Option<DeclaredArg> {
    let code = crate::graphql::blank(field_text);
    let open = code.find('(')?;
    let close = open + matching_paren(&code[open..])?;
    let re = Regex::new(&format!(
        r"(?:^|[\s(,]){}\s*:\s*((?:\[\s*)*[A-Za-z_][A-Za-z0-9_]*\s*!?(?:\s*\]\s*!?)*)",
        regex::escape(name)
    ))
    .ok()?;
    let m = re.captures(&code[open..close])?;
    let type_: String = m[1].chars().filter(|c| !c.is_whitespace()).collect();
    let after = open + m.get(0)?.end();
    // `blank` keeps offsets, so the literal is read from the original text.
    let rest = &field_text[after..close];
    let default = Regex::new(r#"^\s*=\s*("(?:[^"\\]|\\.)*")"#)
        .ok()?
        .captures(rest)
        .and_then(|d| serde_json::from_str::<String>(&d[1]).ok());
    Some(DeclaredArg { type_, default })
}

fn matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.bytes().enumerate() {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The top-level names of a fields expression: `id,business{name}` →
/// `["id", "business"]`.
pub fn top_level_names(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut cur) = (0i32, String::new());
    for c in expr.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        if depth == 0 && c != '}' {
            cur.push(c);
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// One requested field of a fields expression: its name with any
/// `.modifier(…)` chain stripped (`campaigns.limit(5)` → `campaigns`), and
/// its group's children when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldNode {
    pub name: String,
    pub children: Option<Vec<FieldNode>>,
}

/// A fields expression as a tree. Whitespace and order are not significant;
/// a comma inside `(…)` or `{…}` does not split.
pub fn parse_fields(expr: &str) -> Vec<FieldNode> {
    let (mut braces, mut parens, mut start) = (0i32, 0i32, 0usize);
    let mut parts: Vec<&str> = Vec::new();
    for (i, c) in expr.char_indices() {
        match c {
            '{' => braces += 1,
            '}' => braces -= 1,
            '(' => parens += 1,
            ')' => parens -= 1,
            ',' if braces == 0 && parens == 0 => {
                parts.push(&expr[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&expr[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|part| {
            let (head, inner) = match (part.find('{'), part.ends_with('}')) {
                (Some(open), true) => (&part[..open], Some(&part[open + 1..part.len() - 1])),
                _ => (part, None),
            };
            FieldNode {
                name: head
                    .split(['.', '('])
                    .next()
                    .unwrap_or(head)
                    .trim()
                    .to_string(),
                children: inner.map(parse_fields),
            }
        })
        .collect()
}

/// Every expansion group in a fields expression, by its dotted path, with
/// the names it requests: `business{name,primary_page{id}},id` →
/// `{business: [name, primary_page], business.primary_page: [id]}`. A
/// modifier is not part of the path: `labels.limit(5){id}` → `{labels: [id]}`.
pub fn expansion_groups(expr: &str) -> BTreeMap<String, Vec<String>> {
    fn walk(nodes: &[FieldNode], prefix: &str, out: &mut BTreeMap<String, Vec<String>>) {
        for n in nodes {
            if let Some(children) = &n.children {
                let path = if prefix.is_empty() {
                    n.name.clone()
                } else {
                    format!("{}.{}", prefix, n.name)
                };
                out.insert(
                    path.clone(),
                    children.iter().map(|c| c.name.clone()).collect(),
                );
                walk(children, &path, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(&parse_fields(expr), "", &mut out);
    out
}

/// The resolved decision that names `op_key` and carries `literal` verbatim
/// as the fields expression for it — the recorded answer for a default the
/// derivation cannot express (an upstream alias, a paging modifier).
///
/// Both halves match exactly, because lint is the only gate on the default
/// (scaffold builds its stubs from whatever is declared): the operation is an
/// `affects` entry or a whole key in the text (`get:/act_{id}/campaigns` does
/// not name `get:/act_{id}`), and the literal is a whole backtick span in the
/// resolution ("provide" does not carry `id`). An empty literal is never
/// carried.
pub fn decision_for<'a>(decisions: &'a Value, op_key: &str, literal: &str) -> Option<&'a str> {
    if literal.is_empty() {
        return None;
    }
    get_arr(decisions, "decisions")
        .into_iter()
        .flatten()
        .filter(|d| get_str(d, "status") == Some("resolved"))
        .find(|d| {
            let resolution = get(d, "resolution");
            let text = [
                get_str(d, "title"),
                get_str(d, "context"),
                resolution.and_then(|r| get_str(r, "decision")),
                resolution.and_then(|r| get_str(r, "note")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
            let names_op = names_key(&text, op_key)
                || get_arr(d, "affects")
                    .into_iter()
                    .flatten()
                    .any(|a| a.as_str() == Some(op_key));
            let carries = [
                resolution.and_then(|r| get_str(r, "decision")),
                resolution.and_then(|r| get_str(r, "note")),
            ]
            .into_iter()
            .flatten()
            .any(|t| backtick_spans(t).any(|span| span == literal));
            names_op && carries
        })
        .and_then(|d| get_str(d, "id"))
}

/// A character that can continue an operation key's path.
fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '/' | '{' | '}' | '_' | '-')
}

/// Whether `text` names `key` as a whole token: nothing that could extend the
/// key on either side. A `.` ends it only when no path character follows, so
/// a sentence-final period counts and `get:/x.json` does not name `get:/x`.
fn names_key(text: &str, key: &str) -> bool {
    !key.is_empty()
        && text.match_indices(key).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let mut after = text[at + key.len()..].chars();
            let ends = match after.next() {
                None => true,
                Some('.') => !after.next().is_some_and(is_path_char),
                Some(c) => !is_path_char(c),
            };
            ends && !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
        })
}

/// The contents of every `` `…` `` span in `text`, and of every fenced
/// block (its info string dropped, the rest trimmed).
fn backtick_spans(text: &str) -> impl Iterator<Item = &str> {
    text.split("```").enumerate().flat_map(|(i, part)| {
        let spans: Vec<&str> = if i % 2 == 1 {
            vec![match part.split_once('\n') {
                Some((_, body)) => body.trim(),
                None => part,
            }]
        } else {
            part.split('`').skip(1).step_by(2).collect()
        };
        spans
    })
}

/// The fields of the derived default, by dotted path, that a recorded
/// literal does not request. A decision may add to the derived list (a
/// paging modifier, a wider group, an edge the connector does not map),
/// never take from it: a field it drops is one the connector maps and the
/// source would no longer send. Compared as trees, so order, whitespace and
/// modifiers do not matter (`campaigns.limit(5){name}` keeps
/// `campaigns{name}`); each derived group's children must be requested
/// under the same name, recursively, and a derived bare name is kept by a
/// group of that name.
pub fn dropped_parts(literal: &str, derived: &str) -> Vec<String> {
    fn walk(have: &[FieldNode], want: &[FieldNode], prefix: &str, out: &mut Vec<String>) {
        for w in want {
            let path = if prefix.is_empty() {
                w.name.clone()
            } else {
                format!("{}.{}", prefix, w.name)
            };
            match (have.iter().find(|h| h.name == w.name), &w.children) {
                (None, _) => out.push(path),
                (Some(_), None) => {}
                (Some(h), Some(children)) => match &h.children {
                    Some(got) => walk(got, children, &path, out),
                    // A bare name gets the default projection, not the
                    // children the connector maps.
                    None => out.extend(children.iter().map(|c| format!("{}.{}", path, c.name))),
                },
            }
        }
    }
    let mut out = Vec::new();
    walk(&parse_fields(literal), &parse_fields(derived), "", &mut out);
    out
}

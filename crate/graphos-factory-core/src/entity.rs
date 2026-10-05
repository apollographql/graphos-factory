//! Entity rules (ADR 0076): what a `@key` type needs to be worth its key.
//!
//! A type with `@key` promises other subgraphs, and the router, that it can
//! be found by that key. Every `@key` on the type is read, in sorted order
//! so the findings do not depend on directive order. Four rules check the
//! promise against the schema, the selection and the inventory:
//!
//! - `entity-without-lookup` (error), once per resolvable key: no selected
//!   operation returns the type by that key — a path or query parameter
//!   named for it, a by-id path parameter ending with it, or a body
//!   property on a read — and batch find (ADR 0068) rates no bulk lookup on
//!   that key `batchable`. Nothing can resolve a reference by it. A
//!   `resolvable: false` key is a stub for another subgraph's entity and
//!   needs no lookup here.
//! - `entity-key-not-embedded` (error): a connector's selection embeds the
//!   type in another type's field, and what it builds carries no key in
//!   full. The embedding is read through the selection that produces it
//!   (`pet { id: pet_id }`, `owner: user { … }`, `pet: { id: petId }`), at
//!   the wire path that selection reaches it by. A selection this cannot
//!   follow (a spread, a method chain, `$this`) is not judged.
//! - `entity-without-consumer` (warning): no field of another type and no
//!   root field returns the type, so the key serves nobody in this
//!   subgraph. A stub is held to this too.
//! - `entity-field-unresolved` (warning): a field of the type is mapped by
//!   no connector that reaches the type (a root, field-level or type-level
//!   `@connect` selection) and has no connector of its own: a reference
//!   resolved to that type comes back without it. Not for a stub, whose
//!   fields the owning subgraph resolves.
//!
//! Every walk over shapes visits each named shape once: `inventory build`
//! emits OpenAPI polymorphism (`PetInput: oneOf [CatInput]`, `CatInput:
//! allOf [PetInput, …]`) as a `$ref` cycle.
//!
//! Reads facts (the inventory), judgements (the selection) and the schema;
//! changes nothing.

use crate::json::{get, get_obj, get_str, Object};
use apollo_compiler::schema::ExtendedType;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// One finding: `(severity, rule, message)`.
pub type EntityFinding = (&'static str, &'static str, String);

fn key_fields(fields: &str) -> Vec<String> {
    // Top-level names only: `id`, `a b`; a nested `org { id }` keeps `org`.
    let mut out = Vec::new();
    let mut depth = 0;
    for tok in fields
        .replace('{', " { ")
        .replace('}', " } ")
        .split_whitespace()
    {
        match tok {
            "{" => depth += 1,
            "}" => depth -= 1,
            t if depth == 0 => out.push(t.to_string()),
            _ => {}
        }
    }
    out
}

fn directive_str<'a>(d: &'a apollo_compiler::ast::Directive, arg: &str) -> Option<&'a str> {
    d.specified_argument_by_name(arg).and_then(|v| v.as_str())
}

/// A connector's `(method, path)`, lower-cased method and every `{…}`
/// placeholder reduced to `{}`, query string and trailing `/` dropped: what
/// pairs it with the inventory operation it calls. None when the `http`
/// argument names no method this reads.
fn connector_http(d: &apollo_compiler::ast::Directive) -> Option<(String, String)> {
    let http = d.specified_argument_by_name("http")?.as_object()?;
    http.iter().find_map(|(k, v)| {
        let m = k.as_str();
        matches!(m, "GET" | "POST" | "PUT" | "PATCH" | "DELETE")
            .then(|| v.as_str().map(|p| (m.to_ascii_lowercase(), path_shape(p))))
            .flatten()
    })
}

/// `/pets/{$args.petId}/` and `/pets/{petId}` both read `/pets/{}`.
fn path_shape(path: &str) -> String {
    let path = path.split('?').next().unwrap_or("");
    let mut out = String::new();
    let mut depth = 0;
    for c in path.chars() {
        match c {
            '{' => {
                if depth == 0 {
                    out.push_str("{}");
                }
                depth += 1;
            }
            '}' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim_end_matches('/').to_string()
}

/// Whether a connector's `(method, path)` calls `op`. The operation's path
/// may carry a base-URL prefix the connector's does not, so the connector's
/// path is a suffix.
fn calls(http: &Option<(String, String)>, op: &Value) -> bool {
    match http {
        Some((m, p)) => {
            get_str(op, "method").is_some_and(|om| om.eq_ignore_ascii_case(m))
                && path_shape(get_str(op, "path").unwrap_or("")).ends_with(p.as_str())
        }
        None => false,
    }
}

/// One connector: the type its selection's top level maps (a root field's
/// return type, a field-level connector's field type, a type-level
/// connector's own type), the selection, and its `(method, path)`.
type Connector = (String, String, Option<(String, String)>);

fn connectors(schema: &apollo_compiler::Schema) -> Vec<Connector> {
    let mut out = Vec::new();
    for (name, ty) in &schema.types {
        let obj = match ty {
            ExtendedType::Object(o) => o,
            _ => continue,
        };
        for d in obj.directives.get_all("connect") {
            if let Some(sel) = directive_str(d, "selection") {
                out.push((name.to_string(), sel.to_string(), connector_http(d)));
            }
        }
        for field in obj.fields.values() {
            for d in field.directives.get_all("connect") {
                if let Some(sel) = directive_str(d, "selection") {
                    out.push((
                        field.ty.inner_named_type().to_string(),
                        sel.to_string(),
                        connector_http(d),
                    ));
                }
            }
        }
    }
    out
}

/// A node that maps a field, with the wire path from the connector's
/// response root to the object the node reads from.
type Mapping = (crate::reconcile::Node, Vec<String>);

/// Every `(type, field)` a selection maps, with the node that maps it and
/// where on the wire it sits, following sub-selections into the field
/// types. A rooted node with no alias (`$.values { … }`) is an envelope: its
/// children map the same type one property down. A literal object
/// (`pet: { id: petId }`) reads the object it sits in.
fn walk(
    schema: &apollo_compiler::Schema,
    nodes: &[crate::reconcile::Node],
    ty: &str,
    wire: &[String],
    out: &mut BTreeMap<(String, String), Vec<Mapping>>,
    depth: usize,
) {
    if depth > 12 {
        return;
    }
    let obj = match schema.types.get(ty) {
        Some(ExtendedType::Object(o)) => o,
        _ => return,
    };
    let below = |node: &crate::reconcile::Node| {
        let mut w = wire.to_vec();
        w.extend(node.key.iter().flatten().cloned());
        w
    };
    for node in nodes {
        if node.rooted && node.alias.is_none() {
            if let Some(children) = &node.children {
                walk(schema, children, ty, &below(node), out, depth + 1);
            }
            continue;
        }
        let name = match output_name(node) {
            Some(n) => n,
            None => continue,
        };
        if let Some(field) = obj.fields.get(name.as_str()) {
            out.entry((ty.to_string(), name.clone()))
                .or_default()
                .push((node.clone(), wire.to_vec()));
            if let Some(children) = &node.children {
                walk(
                    schema,
                    children,
                    field.ty.inner_named_type().as_str(),
                    &below(node),
                    out,
                    depth + 1,
                );
            }
        }
    }
}

/// The GraphQL field a selection node outputs: its alias, else the last
/// segment of the path it reads.
fn output_name(node: &crate::reconcile::Node) -> Option<String> {
    node.alias
        .clone()
        .or_else(|| node.key.as_ref().and_then(|k| k.last().cloned()))
}

/// Follow `$ref`s and array items to the object they stand for, recording
/// the last shape name passed. Bounded: a `$ref` chain can loop.
fn settle<'a>(mut v: &'a Value, shapes: &'a Object, name: &mut Option<String>) -> &'a Value {
    for _ in 0..16 {
        if let Some(r) = get_str(v, "$ref") {
            match shapes.get_key_value(crate::inventory::shape_name(r)) {
                Some((n, s)) => {
                    *name = Some(n.clone());
                    v = s;
                    continue;
                }
                None => break,
            }
        }
        match get(v, "items") {
            Some(i) => v = i,
            None => break,
        }
    }
    v
}

/// The objects `v` stands for: itself settled, and every `oneOf`, `anyOf`
/// and `allOf` member, each named shape once (a polymorphic shape is a
/// cycle).
fn branches<'a>(v: &'a Value, shapes: &'a Object) -> Vec<&'a Value> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        let mut name = None;
        let v = settle(v, shapes, &mut name);
        if let Some(n) = name {
            if !seen.insert(n) {
                continue;
            }
        }
        out.push(v);
        for c in ["oneOf", "anyOf", "allOf"] {
            if let Some(members) = get(v, c).and_then(Value::as_array) {
                stack.extend(members.iter());
            }
        }
    }
    out
}

/// Every property name the objects `v` stands for declare.
fn property_names(v: &Value, shapes: &Object) -> BTreeSet<String> {
    branches(v, shapes)
        .into_iter()
        .filter_map(|b| get_obj(b, "properties"))
        .flat_map(|p| p.keys().cloned())
        .collect()
}

/// The object at `path` under `root`, and the name of the last shape the
/// walk passed: the host of an embedding, reached the way the connector's
/// selection reaches it. None when a segment is absent from the wire.
fn at_wire_path<'a>(
    root: &'a Value,
    path: &[String],
    shapes: &'a Object,
) -> Option<(&'a Value, Option<String>)> {
    let mut name = None;
    let mut cur = settle(root, shapes, &mut name);
    for seg in path {
        let next = branches(cur, shapes)
            .into_iter()
            .find_map(|b| get(b, "properties").and_then(|p| get(p, seg.as_str())))?;
        cur = settle(next, shapes, &mut name);
    }
    Some((cur, name))
}

/// The property names of the wire object the children of `node` read,
/// given the host object the node sits in: the host itself for a literal
/// object (`pet: { id: petId }`), else the one property the node reads
/// (`pet { … }`, `owner: user { … }`). None when the node is not a plain
/// sub-selection this can follow (a spread, an opaque value, a
/// multi-segment path, a method chain), the property is absent from the
/// wire, or its shape declares no properties.
fn embedding_source(
    host: &Value,
    node: &crate::reconcile::Node,
    shapes: &Object,
) -> Option<BTreeSet<String>> {
    if node.spread.is_some() || node.opaque || !node.methods.is_empty() {
        return None;
    }
    node.children.as_ref()?;
    let props = match &node.key {
        None => property_names(host, shapes),
        Some(k) if k.len() == 1 => {
            let embedded = branches(host, shapes)
                .into_iter()
                .find_map(|b| get(b, "properties").and_then(|p| get(p, k[0].as_str())))?;
            property_names(embedded, shapes)
        }
        Some(_) => return None,
    };
    (!props.is_empty()).then_some(props)
}

/// The fields of `key` the children of `node` do not produce from `props`:
/// no child outputs the field, or the child reads a single wire property
/// `props` lacks. A child the reader cannot judge (a literal, `$this`, a
/// multi-segment path, a method chain) produces its field. None when a
/// child is a spread: what it adds is unknown.
fn missing_key_fields(
    node: &crate::reconcile::Node,
    key: &[String],
    props: &BTreeSet<String>,
) -> Option<Vec<String>> {
    let children = node.children.as_ref()?;
    if children.iter().any(|c| c.spread.is_some()) {
        return None;
    }
    Some(
        key.iter()
            .filter(|f| {
                match children
                    .iter()
                    .find(|c| output_name(c).as_deref() == Some(f.as_str()))
                {
                    None => true,
                    Some(c) => match &c.key {
                        Some(k) if k.len() == 1 && !c.opaque && c.methods.is_empty() => {
                            !props.contains(&k[0])
                        }
                        _ => false,
                    },
                }
            })
            .cloned()
            .collect(),
    )
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `op` only reads: a GET or HEAD, one the inventory classifies as
/// a read, or one its selection entry gives `graphql.root: query`. Batch
/// find's rule (its private `is_read`), restated because a body key counts
/// only on a read: `POST /pets` with `{id}` in its body creates a pet.
fn reads(op: &Value, entry: Option<&Value>) -> bool {
    let method = get_str(op, "method").unwrap_or("").to_ascii_uppercase();
    matches!(method.as_str(), "GET" | "HEAD")
        || get_str(op, "semantics") == Some("read")
        || entry
            .and_then(|e| get(e, "graphql"))
            .and_then(|g| get_str(g, "root"))
            == Some("query")
}

/// Whether `{name}` is the last segment of `path`, a trailing `/` aside:
/// the by-id shape `/status_updates/{status_gid}`, not the scope in
/// `/orgs/{org_id}/pets`.
fn last_segment_is(path: &str, name: &str) -> bool {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .is_some_and(|seg| seg == format!("{{{}}}", name))
}

/// Whether `op` takes the key `k` of `shape`: a path or query parameter
/// named for it (batch find's rule); a path parameter whose name ends with
/// it and is the path's last segment (`/status_updates/{status_gid}` for
/// `gid`); or, on a read, a request-body property named for it or ending
/// with it (an RPC-style `POST /candidate.info` with `{id}`). Each body shape is
/// visited once: a polymorphic body (`PetInput: oneOf [CatInput]`,
/// `CatInput: allOf [PetInput, …]`) is a `$ref` cycle.
fn carries_key(op: &Value, entry: Option<&Value>, k: &str, shape: &str, shapes: &Object) -> bool {
    let ends = |name: &str| norm(name).ends_with(&norm(k));
    let path = get_str(op, "path").unwrap_or("");
    let params = get(op, "parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    for p in params {
        let name = get_str(p, "name").unwrap_or("");
        match get_str(p, "in") {
            Some("path")
                if crate::batch::param_names_key(name, k, shape)
                    || (ends(name) && last_segment_is(path, name)) =>
            {
                return true
            }
            Some("query") if crate::batch::param_names_key(name, k, shape) => return true,
            _ => {}
        }
    }
    if !reads(op, entry) {
        return false;
    }
    let body = get(op, "request_body").and_then(|b| get(b, "shape_ref"));
    let body_ref = body.map(|r| serde_json::json!({ "$ref": r }));
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut props: Vec<&String> = Vec::new();
    let mut stack: Vec<&Value> = body_ref.iter().collect();
    while let Some(v) = stack.pop() {
        let v = match get_str(v, "$ref") {
            Some(r) => {
                let name = crate::inventory::shape_name(r);
                match shapes.get_key_value(name) {
                    Some((n, s)) if seen.insert(n.as_str()) => s,
                    _ => continue,
                }
            }
            None => v,
        };
        if let Some(p) = get_obj(v, "properties") {
            props.extend(p.keys());
        }
        for c in ["oneOf", "anyOf", "allOf"] {
            if let Some(members) = get(v, c).and_then(Value::as_array) {
                stack.extend(members.iter());
            }
        }
    }
    props
        .iter()
        .any(|p| crate::batch::param_names_key(p, k, shape) || ends(p))
}

fn is_root(name: &str) -> bool {
    matches!(name, "Query" | "Mutation" | "Subscription")
}

/// The entity rules' findings for one workspace.
pub fn check(
    sdl: &str,
    workspace: &Value,
    selection: Option<&Value>,
    inventory: Option<&Value>,
) -> Vec<EntityFinding> {
    let schema = match apollo_compiler::Schema::parse(sdl, "schema.graphql") {
        Ok(s) => s,
        Err(e) => e.partial,
    };
    let empty = Object::new();
    let shapes = inventory
        .and_then(|i| get(i, "shapes"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let ops: Vec<&Value> = inventory
        .and_then(|i| get(i, "operations"))
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let prefix = get_str(workspace, "type_prefix").unwrap_or("");
    let included = |key: &str| {
        selection
            .and_then(|s| get_obj(s, "operations"))
            .and_then(|o| o.get(key))
            .is_some_and(|e| get(e, "include").and_then(Value::as_bool) != Some(false))
    };
    // The inventory shape an SDL type stands for: the selection's
    // `graphql.type_name` override's result shape, else the name without the
    // workspace prefix.
    let mut overrides: BTreeMap<String, String> = BTreeMap::new();
    if let Some(sel_ops) = selection.and_then(|s| get_obj(s, "operations")) {
        for (key, entry) in sel_ops {
            if let Some(tn) = get(entry, "graphql").and_then(|g| get_str(g, "type_name")) {
                if let Some(op) = ops.iter().find(|o| get_str(o, "key") == Some(key.as_str())) {
                    if let Some(s) = crate::batch::operation_result_shape(op, selection, shapes) {
                        let sdl_name = if prefix.is_empty() {
                            tn.to_string()
                        } else {
                            format!("{}_{}", prefix, tn)
                        };
                        overrides.insert(sdl_name, s);
                    }
                }
            }
        }
    }
    // And the shapes the operations behind its root fields return: a type
    // named for the domain (`Deck`) often stands for a shape named for the
    // response (`DeckState`).
    let field_prefix = get_str(workspace, "field_prefix").unwrap_or("");
    let mut returned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if let Some(sel_ops) = selection.and_then(|s| get_obj(s, "operations")) {
        for (key, entry) in sel_ops {
            if get(entry, "include").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let g = match get(entry, "graphql") {
                Some(g) => g,
                None => continue,
            };
            let (root, name) = match (get_str(g, "root").unwrap_or("query"), get_str(g, "name")) {
                (r, Some(n)) => (if r == "mutation" { "Mutation" } else { "Query" }, n),
                _ => continue,
            };
            let field = if field_prefix.is_empty() {
                name.to_string()
            } else {
                format!("{}_{}", field_prefix, name)
            };
            let ret = match schema.types.get(root) {
                Some(ExtendedType::Object(o)) => o
                    .fields
                    .get(field.as_str())
                    .map(|f| f.ty.inner_named_type().to_string()),
                _ => None,
            };
            let op = ops.iter().find(|o| get_str(o, "key") == Some(key.as_str()));
            if let (Some(ret), Some(op)) = (ret, op) {
                if let Some(s) = crate::batch::operation_result_shape(op, selection, shapes) {
                    let item = shapes
                        .get(&s)
                        .and_then(|v| get(v, "items"))
                        .and_then(|i| get_str(i, "$ref"))
                        .map(|r| crate::inventory::shape_name(r).to_string());
                    returned.entry(ret).or_default().insert(item.unwrap_or(s));
                }
            }
        }
    }
    let shape_of = |sdl_type: &str| -> String {
        overrides.get(sdl_type).cloned().unwrap_or_else(|| {
            sdl_type
                .strip_prefix(&format!("{}_", prefix))
                .unwrap_or(sdl_type)
                .to_string()
        })
    };
    let shapes_of = |sdl_type: &str| -> BTreeSet<String> {
        let mut all = returned.get(sdl_type).cloned().unwrap_or_default();
        all.insert(shape_of(sdl_type));
        all
    };
    let family = |a: &str, b: &str| a == b || crate::batch::stem(a) == crate::batch::stem(b);

    // Each connector's mappings, kept per connector so an embedding is read
    // through the selection of the connector that calls the operation.
    let mut resolved: BTreeSet<(String, String)> = BTreeSet::new();
    type Mappings = BTreeMap<(String, String), Vec<Mapping>>;
    let mut mapped: Vec<(Option<(String, String)>, Mappings)> = Vec::new();
    for (ty, sel, http) in connectors(&schema) {
        let mut m = Mappings::new();
        walk(
            &schema,
            &crate::reconcile::parse_selection(&sel),
            &ty,
            &[],
            &mut m,
            0,
        );
        resolved.extend(m.keys().cloned());
        mapped.push((http, m));
    }
    let batch = inventory
        // `find` fails only on a schema that does not parse; this check
        // then runs on the partial parse with no bulk lookup counted, and
        // compose reports the parse error.
        .and_then(|inv| crate::batch::find(inv, selection, sdl, true).ok())
        .unwrap_or_default();
    let entry = |key: &str| {
        selection
            .and_then(|s| get_obj(s, "operations"))
            .and_then(|o| o.get(key))
    };

    let mut out: Vec<EntityFinding> = Vec::new();
    for (name, ty) in &schema.types {
        let obj = match ty {
            ExtendedType::Object(o) => o,
            _ => continue,
        };
        // Every @key, as (fields, top-level names, resolvable), sorted so
        // the findings do not depend on directive order.
        let mut keysets: Vec<(String, Vec<String>, bool)> = obj
            .directives
            .get_all("key")
            .filter_map(|d| {
                let fields = directive_str(d, "fields")?;
                let resolvable = d
                    .specified_argument_by_name("resolvable")
                    .and_then(|v| v.to_bool())
                    != Some(false);
                Some((fields.to_string(), key_fields(fields), resolvable))
            })
            .collect();
        if keysets.is_empty() {
            continue;
        }
        keysets.sort();
        keysets.dedup();
        // Every key `resolvable: false`: a stub for another subgraph's
        // entity, referenced here and resolved there.
        let stub = keysets.iter().all(|(_, _, r)| !r);
        let t = name.as_str();
        let shape = shape_of(t);
        let own = shapes_of(t);
        let is_own = |s: &str| own.iter().any(|o| family(s, o));

        // entity-without-lookup: each resolvable key is a way the router may
        // enter this subgraph, so each needs a lookup of its own.
        for (text, keys, resolvable) in &keysets {
            if !resolvable {
                continue;
            }
            let by_key = ops.iter().any(|op| {
                let op_key = get_str(op, "key").unwrap_or("");
                included(op_key)
                    && crate::batch::operation_result_shape(op, selection, shapes)
                        .is_some_and(|s| is_own(&s))
                    && keys
                        .iter()
                        .all(|k| carries_key(op, entry(op_key), k, &shape, shapes))
            });
            let batchable = batch.iter().any(|r| {
                is_own(&r.type_name)
                    && r.verdict == crate::batch::Verdict::Batchable
                    && keys.as_slice() == [r.key.clone()]
            });
            if !by_key && !batchable {
                out.push((
                    "error",
                    "entity-without-lookup",
                    format!(
                        "{} carries @key(fields: \"{}\") but no selected operation returns {} by that key (a path or query parameter named for it), and batch find rates no bulk-by-keys operation for it batchable: nothing can resolve a reference",
                        t, text, shape
                    ),
                ));
            }
        }

        // entity-key-not-embedded, and the references entity-without-consumer counts
        let mut referenced = false;
        for (other, oty) in &schema.types {
            let oobj = match oty {
                ExtendedType::Object(o) if other.as_str() != t => o,
                _ => continue,
            };
            for (fname, field) in &oobj.fields {
                if field.ty.inner_named_type().as_str() != t {
                    continue;
                }
                referenced = true;
                if is_root(other.as_str()) {
                    continue;
                }
                let at = (other.to_string(), fname.to_string());
                for op in &ops {
                    let op_key = get_str(op, "key").unwrap_or("");
                    if !included(op_key) {
                        continue;
                    }
                    // The selections that produce `other.fname` in the
                    // connectors that call this operation, each read at the
                    // wire path its selection reaches it by. None: nothing
                    // here embeds the entity, so there is nothing to judge.
                    let root = match get(op, "response").and_then(|r| get(r, "shape_ref")) {
                        Some(r) => serde_json::json!({ "$ref": r }),
                        None => continue,
                    };
                    for (http, m) in &mapped {
                        if !calls(http, op) {
                            continue;
                        }
                        for (node, wire) in m.get(&at).into_iter().flatten() {
                            let (host, host_name) = match at_wire_path(&root, wire, shapes) {
                                Some(h) => h,
                                None => continue,
                            };
                            let props = match embedding_source(host, node, shapes) {
                                Some(p) => p,
                                None => continue,
                            };
                            // The embedding is a reference if it carries
                            // every field of at least one key.
                            let mut lacking: Vec<(String, Vec<String>)> = Vec::new();
                            let mut unknown = false;
                            for (text, keys, _) in &keysets {
                                match missing_key_fields(node, keys, &props) {
                                    None => unknown = true,
                                    Some(m) if !m.is_empty() => lacking.push((text.clone(), m)),
                                    Some(_) => {}
                                }
                            }
                            if unknown || lacking.len() < keysets.len() {
                                continue;
                            }
                            let lacks = if lacking.len() == 1 {
                                format!("the key {}", lacking[0].1.join(", "))
                            } else {
                                format!(
                                    "a field of every key: {}",
                                    lacking
                                        .iter()
                                        .map(|(k, m)| format!("\"{}\" ({})", k, m.join(", ")))
                                        .collect::<Vec<_>>()
                                        .join("; ")
                                )
                            };
                            out.push((
                                "error",
                                "entity-key-not-embedded",
                                format!(
                                    "{} embeds {} at {}.{} ({}), but the embedded shape lacks {}: the router cannot turn that embedding into a reference",
                                    op_key,
                                    t,
                                    host_name.as_deref().unwrap_or("(inline)"),
                                    fname,
                                    other,
                                    lacks
                                ),
                            ));
                        }
                    }
                }
            }
        }
        if !referenced {
            out.push((
                "warn",
                "entity-without-consumer",
                format!(
                    "{} carries @key but nothing in this schema returns it — no root field and no field of another type — so the key serves no reference here",
                    t
                ),
            ));
        }

        // entity-field-unresolved: a stub's fields are resolved by the
        // subgraph that owns it; the embedding supplies its key.
        if stub {
            continue;
        }
        let unresolved: Vec<&str> = obj
            .fields
            .iter()
            .filter(|(f, field)| {
                !resolved.contains(&(t.to_string(), f.to_string()))
                    && field.directives.get("connect").is_none()
            })
            .map(|(f, _)| f.as_str())
            .collect();
        if !unresolved.is_empty() {
            out.push((
                "warn",
                "entity-field-unresolved",
                format!(
                    "{}: {} {} mapped by no connector that reaches {} and {} no connector of {} own; a reference resolved to {} comes back without {}",
                    t,
                    unresolved.join(", "),
                    if unresolved.len() == 1 { "is" } else { "are" },
                    t,
                    if unresolved.len() == 1 { "has" } else { "have" },
                    if unresolved.len() == 1 { "its" } else { "their" },
                    t,
                    if unresolved.len() == 1 { "it" } else { "them" }
                ),
            ));
        }
    }
    out.sort_by(|a, b| (a.1, &a.2).cmp(&(b.1, &b.2)));
    out.dedup();
    out
}

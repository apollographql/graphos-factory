//! `spans json-accounting` — every response field the rendered SDL still
//! leaves as the workspace's own JSON scalar (`{type_prefix}_JSON`), matched
//! against a recorded reason on a **resolved** `.factory/decisions.json`
//! record (a `json_reasons` entry, ADR 0073), from the closed vocabulary in
//! [`REASONS`]. The reason lives in decisions, never in `selection.yaml` or
//! a schema doc comment: the decisions-only rule, the same one `null_handling` (ADR
//! 0070) already follows — a reason interleaved with the schema the agent
//! reads to author was mistaken for an instruction or a fact.
//!
//! This reads the CURRENT SDL directly — every object type's fields,
//! including fields nested inside other object types and fields wrapped in
//! a list at any depth (`[JSON]`, `[[JSON!]]!`, …; list/non-null wrappers
//! are stripped before comparing against the scalar name, so nesting depth
//! never hides a field) — rather than walking `inventory.json` paths.
//!
//! `type_recovery::accounting` (the specialist-pipeline copy this ports
//! from, see docs/decisions/0073-json-blob-accounting.md) read only the
//! inventory and the selection and treated every expected property as
//! accounted for whenever `defaults.fields` was `all`, so a field that was
//! still typed JSON in the generated SDL could pass with no field-level
//! reason ever recorded for it. This instrument never reads `defaults`, so
//! that hole cannot reopen here: `defaults.fields: all` is a field-selection
//! policy, never itself a reason.
//!
//! Each of the four reasons is a **checkable claim**, not a free pass once
//! recorded: on every run its predicate is re-evaluated against the
//! CURRENT `.factory/inventory.json` shape at the field's own path: the
//! type's shape (located by name — the GraphQL type name with the
//! workspace's `type_prefix_` stripped — or through its root field's
//! operation, or through a parent's property; see `locate_types`) and the
//! property matching the field name, or its snake_case wire form. A reason
//! on a type located none of those ways is `unresolved`, never read against
//! "no shape". A predicate that no longer holds reports
//! `stale`, never a silent pass; independently, a field whose inventory
//! shape has since become fully typed reports `recoverable` regardless of
//! what reason is on file, because the source has moved past all four
//! problem categories and the field should simply be re-typed. No history,
//! no timestamps: the verdict is a pure function of the current inventory,
//! selection and SDL, read through `factory_io` (ADR 0025).

use crate::json::{get_arr, get_obj, get_str, Object};
use apollo_compiler::ast::{self, Definition, Type};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// The closed vocabulary a `json_reasons` entry's `reason` must be one of.
/// Any other value — including an absent entry — does not count as
/// accounted for. Proposed for the pilots plus the exported workspaces
/// measured in ADR 0073; a service that needs a reason outside this list
/// adds it there; add it here too.
pub const REASONS: [&str; 5] = [
    "free-form-object",
    "recursive",
    "vendor-undocumented",
    "polymorphic-without-discriminator",
    // The workspace cut nesting at its stated depth (`defaults.max_depth`)
    // or cut a recursion, and the inventory has the shape beyond the cut.
    "depth-cap",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A recorded reason whose predicate currently holds, and the
    /// inventory shape is not fully typed.
    Accounted,
    /// No reason recorded at all.
    Unaccounted,
    /// A reason is recorded, but its predicate does not hold against the
    /// current inventory shape — the field moved, the recorded claim did
    /// not.
    Stale,
    /// The inventory now holds a fully typed shape at this field's path,
    /// regardless of any recorded reason: the field should be re-typed,
    /// not re-justified. Takes priority over Accounted/Stale/Unaccounted.
    Recoverable,
    /// A reason is recorded, but no inventory shape could be located for
    /// the field's type — not by name, not through a root field's
    /// operation, not through a parent type's property. The claim cannot
    /// be checked, so it is neither accounted nor stale: an open finding.
    Unresolved,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Accounted => "accounted",
            Status::Unaccounted => "unaccounted",
            Status::Stale => "stale",
            Status::Recoverable => "recoverable",
            Status::Unresolved => "unresolved",
        }
    }

    pub fn is_open_finding(&self) -> bool {
        !matches!(self, Status::Accounted)
    }
}

/// Which side of the wire a row's JSON position is on (Phase 9d): a field
/// the response fills, or a value the caller sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// A field of an object type, root fields included: read against the
    /// response shape.
    Output,
    /// A root field's argument, keyed `Root.field(arg:)` (a schema
    /// coordinate): read against the request body or the parameter it fills.
    Argument,
    /// A field of an input object type: read against the request shape the
    /// type is sent as.
    Input,
}

impl Position {
    pub fn label(&self) -> &'static str {
        match self {
            Position::Output => "output",
            Position::Argument => "argument",
            Position::Input => "input",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FieldRow {
    pub type_name: String,
    /// The field name, or for an argument `field(arg:)`.
    pub field_name: String,
    pub position: Position,
    /// `Some` only when a resolved decision's `json_reasons` names this
    /// field with a reason in [`REASONS`]; an unrecognized reason is the
    /// same as no entry.
    pub reason: Option<String>,
    pub decision: Option<String>,
    pub status: Status,
}

impl FieldRow {
    pub fn key(&self) -> String {
        format!("{}.{}", self.type_name, self.field_name)
    }

    pub fn accounted(&self) -> bool {
        matches!(self.status, Status::Accounted)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TypeCounts {
    pub accounted: usize,
    pub unaccounted: usize,
    pub stale: usize,
    pub recoverable: usize,
    pub unresolved: usize,
}

#[derive(Debug)]
pub struct Report {
    pub json_scalar: String,
    /// Every field on every object type in the SDL (Query and Mutation
    /// included), every root field's argument and every input object
    /// field: the denominator for the typed-to-JSON ratio.
    pub total_fields: usize,
    /// One row per field whose bare type is `json_scalar`.
    pub rows: Vec<FieldRow>,
}

impl Report {
    fn count(&self, status: Status) -> usize {
        self.rows.iter().filter(|r| r.status == status).count()
    }

    pub fn accounted_count(&self) -> usize {
        self.count(Status::Accounted)
    }

    pub fn unaccounted_count(&self) -> usize {
        self.count(Status::Unaccounted)
    }

    pub fn stale_count(&self) -> usize {
        self.count(Status::Stale)
    }

    pub fn recoverable_count(&self) -> usize {
        self.count(Status::Recoverable)
    }

    pub fn unresolved_count(&self) -> usize {
        self.count(Status::Unresolved)
    }

    pub fn unaccounted(&self) -> Vec<&FieldRow> {
        self.rows.iter().filter(|r| !r.accounted()).collect()
    }

    pub fn open_findings(&self) -> Vec<&FieldRow> {
        self.rows
            .iter()
            .filter(|r| r.status.is_open_finding())
            .collect()
    }

    /// Accounted/unaccounted/stale/recoverable/unresolved counts per type, in
    /// type-name order.
    pub fn by_type(&self) -> BTreeMap<String, TypeCounts> {
        let mut out: BTreeMap<String, TypeCounts> = BTreeMap::new();
        for r in &self.rows {
            let entry = out.entry(r.type_name.clone()).or_default();
            match r.status {
                Status::Accounted => entry.accounted += 1,
                Status::Unaccounted => entry.unaccounted += 1,
                Status::Stale => entry.stale += 1,
                Status::Recoverable => entry.recoverable += 1,
                Status::Unresolved => entry.unresolved += 1,
            }
        }
        out
    }

    /// `(typed fields, JSON fields)`. `None` for an empty schema (no object
    /// type has any field at all) — never reported as a ratio of zero.
    pub fn typed_to_json_ratio(&self) -> Option<(usize, usize)> {
        if self.total_fields == 0 {
            return None;
        }
        Some((self.total_fields - self.rows.len(), self.rows.len()))
    }

    pub fn check_failure(&self) -> Option<String> {
        let bad = self.open_findings();
        if bad.is_empty() {
            return None;
        }
        Some(format!(
            "{} of {} JSON field(s) are not accounted for: {}",
            bad.len(),
            self.rows.len(),
            bad.iter()
                .map(|r| format!("{} ({})", r.key(), r.status.label()))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// The bare named type, with every `List`/`NonNull` wrapper stripped —
/// matches `closure::bare_type_name`'s behavior (docs/decisions/0037), kept
/// local rather than shared because this walk needs only object types and
/// that one also needs input types for a different caller.
fn bare_type_name(ty: &Type) -> &str {
    match ty {
        Type::Named(n) | Type::NonNullNamed(n) => n.as_str(),
        Type::List(inner) | Type::NonNullList(inner) => bare_type_name(inner),
    }
}

/// A GraphQL type name with the workspace's `type_prefix_` stripped — the
/// inventory's own shape names are the bare vendor names.
fn bare_shape_name<'a>(type_name: &'a str, type_prefix: &str) -> &'a str {
    type_name
        .strip_prefix(type_prefix)
        .and_then(|s| s.strip_prefix('_'))
        .unwrap_or(type_name)
}

/// `conferenceBridge` -> `conference_bridge`; a no-op on an already-snake
/// name (`body`).
fn snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Where an object type's inventory shape was found.
#[derive(Clone)]
enum Located {
    /// The shape the type's fields are read from.
    Shape(Value),
    /// Reached through an operation, or a parent property, that documents
    /// no shape at all: every field under it is vendor-undocumented.
    Undocumented,
}

/// A property of `shape` matching the field name as written or its
/// snake_case wire form. `None` is itself a fact — "no property recorded
/// there" — not a lookup failure.
fn property<'a>(shape: &'a Value, field_name: &str) -> Option<&'a Value> {
    let props = get_obj(shape, "properties")?;
    if let Some(p) = props.get(field_name) {
        return Some(p);
    }
    let snake = snake_case(field_name);
    if snake != field_name {
        return props.get(&snake);
    }
    None
}

/// `deref`, then through every array's `items`: the object a list field's
/// elements have.
fn element_shape(shape: &Value, shapes: &Object) -> Value {
    let mut v = deref(shape, shapes, &mut HashSet::new());
    let mut hops = 0;
    while get_str(&v, "type") == Some("array") && hops < 8 {
        match v.get("items") {
            Some(items) => v = deref(items, shapes, &mut HashSet::new()),
            None => break,
        }
        hops += 1;
    }
    v
}

/// How many of `fields` a shape's properties name (as written or snake_case).
fn overlap(shape: &Value, fields: &[String]) -> usize {
    fields
        .iter()
        .filter(|f| property(shape, f).is_some())
        .count()
}

/// The object types some path from a root field reaches at `max_depth` or
/// deeper: a root field's return type is level 1, a field of a level-`k`
/// type is level `k + 1` (the count `selection.yaml`'s `defaults.max_depth`
/// caps, schema-authoring.md § Cycles and depth). Types are shared, so a
/// type counts once any path puts it at the cut, however short another path
/// to it is: the set of level `max_depth`, and everything below it.
fn deep_types(doc: &ast::Document, max_depth: usize) -> HashSet<String> {
    let mut fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for def in &doc.definitions {
        if let Definition::ObjectTypeDefinition(o) = def {
            fields.insert(
                o.name.to_string(),
                o.fields
                    .iter()
                    .map(|f| bare_type_name(&f.ty).to_string())
                    .collect(),
            );
        }
    }
    let children = |t: &str| -> Vec<String> {
        fields
            .get(t)
            .into_iter()
            .flatten()
            .filter(|c| fields.contains_key(*c) && *c != "Query" && *c != "Mutation")
            .cloned()
            .collect()
    };
    let mut level: HashSet<String> = ["Query", "Mutation"]
        .iter()
        .flat_map(|r| children(r))
        .collect();
    for _ in 1..max_depth.max(1) {
        level = level.iter().flat_map(|t| children(t)).collect();
    }
    let mut deep = level.clone();
    let mut frontier: Vec<String> = level.into_iter().collect();
    while let Some(t) = frontier.pop() {
        for c in children(&t) {
            if deep.insert(c.clone()) {
                frontier.push(c);
            }
        }
    }
    deep
}

/// `depth-cap`'s claim: the inventory has structure at the field's path (a
/// non-empty object, through `$ref`s and array items) that the workspace cut
/// — its type sits at or below `max_depth` on some path, or the shape is a
/// recursion.
fn depth_cap_holds(found: Option<&Value>, type_bare: &str, deep: bool, shapes: &Object) -> bool {
    let Some(raw) = found else {
        return false;
    };
    if is_recursive(raw, type_bare, shapes) {
        return true;
    }
    let element = element_shape(raw, shapes);
    deep && get_obj(&element, "properties").is_some_and(|p| !p.is_empty())
}

/// Every object type's inventory shape, located three ways, first found
/// wins (Phase 7aw, the Granola dry run's D-0007):
/// 1. by name: the GraphQL type name with `type_prefix_` stripped, a
///    top-level `inventory.json` shape;
/// 2. through a root field: the selected operation it serves, that
///    operation's response shape, or the shape under its envelope, whichever
///    names more of the type's fields;
/// 3. through a parent: a located type's property for the field, through
///    `$ref`s and array `items` — the inline item shape a vendor never
///    promoted to its own component (Granola's audit events).
///
/// A type reached from nothing is absent from the map: its recorded
/// reasons are `unresolved`, never read against "no shape".
fn locate_types(
    doc: &ast::Document,
    shapes: &Object,
    inventory: Option<&Value>,
    selection: Option<&Value>,
    type_prefix: &str,
    field_prefix: &str,
) -> std::collections::HashMap<String, Located> {
    use std::collections::HashMap;
    // Object type -> (field name, bare field type).
    let mut types: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for def in &doc.definitions {
        if let Definition::ObjectTypeDefinition(o) = def {
            types.insert(
                o.name.to_string(),
                o.fields
                    .iter()
                    .map(|f| (f.name.to_string(), bare_type_name(&f.ty).to_string()))
                    .collect(),
            );
        }
    }
    let field_names = |t: &str| -> Vec<String> {
        types
            .get(t)
            .map(|fs| fs.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    };
    let mut located: HashMap<String, Located> = HashMap::new();

    for name in types.keys() {
        if let Some(shape) = shapes.get(bare_shape_name(name, type_prefix)) {
            located.insert(
                name.clone(),
                Located::Shape(deref(shape, shapes, &mut HashSet::new())),
            );
        }
    }

    let roots = root_operations(inventory, selection, field_prefix);
    for root in ["Query", "Mutation"] {
        for (field, ty) in types.get(root).cloned().unwrap_or_default() {
            if located.contains_key(&ty) || !types.contains_key(&ty) {
                continue;
            }
            let Some((op, envelope)) = roots.get(&field) else {
                continue;
            };
            let shape_ref = crate::json::get(op, "response").and_then(|r| get_str(r, "shape_ref"));
            let Some(shape_ref) = shape_ref else {
                located.insert(ty.clone(), Located::Undocumented);
                continue;
            };
            let whole = element_shape(&serde_json::json!({ "$ref": shape_ref }), shapes);
            let mut candidates = vec![whole.clone()];
            if let Some(p) = envelope.as_deref().and_then(|e| property(&whole, e)) {
                candidates.push(element_shape(p, shapes));
            }
            let fields = field_names(&ty);
            if let Some(best) = candidates
                .into_iter()
                .map(|c| (overlap(&c, &fields), c))
                .filter(|(n, _)| *n > 0)
                .max_by_key(|(n, _)| *n)
            {
                located.insert(ty.clone(), Located::Shape(best.1));
            }
        }
    }

    loop {
        let mut found: Vec<(String, Located)> = Vec::new();
        for (name, fields) in &types {
            let Some(parent) = located.get(name) else {
                continue;
            };
            for (field, ty) in fields {
                if !types.contains_key(ty)
                    || located.contains_key(ty)
                    || found.iter().any(|(t, _)| t == ty)
                {
                    continue;
                }
                let child = match parent {
                    Located::Undocumented => Located::Undocumented,
                    Located::Shape(s) => match property(s, field) {
                        Some(p) => Located::Shape(element_shape(p, shapes)),
                        None => Located::Undocumented,
                    },
                };
                found.push((ty.clone(), child));
            }
        }
        if found.is_empty() {
            break;
        }
        located.extend(found);
    }
    located
}

/// Root field -> (operation, envelope), from the selection's included
/// operations.
fn root_operations<'a>(
    inventory: Option<&'a Value>,
    selection: Option<&Value>,
    field_prefix: &str,
) -> std::collections::HashMap<String, (&'a Value, Option<String>)> {
    let mut roots = std::collections::HashMap::new();
    let ops = inventory.and_then(|i| get_arr(i, "operations"));
    for (key, entry) in selection
        .and_then(|s| get_obj(s, "operations"))
        .into_iter()
        .flatten()
    {
        if !crate::json::truthy(crate::json::get(entry, "include")) {
            continue;
        }
        let Some(name) = crate::json::get(entry, "graphql").and_then(|g| get_str(g, "name")) else {
            continue;
        };
        let Some(op) = ops
            .into_iter()
            .flatten()
            .find(|o| get_str(o, "key") == Some(key.as_str()))
        else {
            continue;
        };
        let envelope = crate::json::get(entry, "response")
            .and_then(|r| get_str(r, "envelope"))
            .or_else(|| crate::json::get(op, "response").and_then(|r| get_str(r, "envelope")))
            .map(str::to_string);
        let field = if field_prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}_{}", field_prefix, name)
        };
        roots.insert(field, (op, envelope));
    }
    roots
}

/// A root field's own position is the operation's response: the shape under
/// its envelope when the selection names one the shape has, else the whole
/// response. Kept raw (not stepped into array items), so a list response is
/// judged as the list it is.
fn response_position(op: &Value, envelope: Option<&str>, shapes: &Object) -> Located {
    let Some(shape_ref) = crate::json::get(op, "response").and_then(|r| get_str(r, "shape_ref"))
    else {
        return Located::Undocumented;
    };
    let whole = serde_json::json!({ "$ref": shape_ref });
    if let Some(p) = envelope.and_then(|e| property(&element_shape(&whole, shapes), e).cloned()) {
        return Located::Shape(p);
    }
    Located::Shape(whole)
}

/// Lowercase letters and digits only: `user-id`, `user_id` and `userId` are
/// one name.
fn loose_name(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A parameter as a shape: its `shape_ref` when it has one, else its
/// scalar `type` (and `enum`).
fn parameter_shape(p: &Value) -> Value {
    if let Some(r) = get_str(p, "shape_ref") {
        return serde_json::json!({ "$ref": r });
    }
    let mut m = serde_json::Map::new();
    for key in ["type", "enum", "items"] {
        if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
            m.insert(key.to_string(), v.clone());
        }
    }
    Value::Object(m)
}

/// Whether a body expression forwards the argument `arg`: `$args.arg`, or
/// `$args.arg { … }`, its sub-selection.
fn forwards_argument(expr: &str, arg: &str) -> bool {
    let e = crate::reconcile::strip_comments(expr);
    let Some(rest) = e.trim().strip_prefix("$args.") else {
        return false;
    };
    let Some(after) = rest.strip_prefix(arg) else {
        return false;
    };
    let after = after.trim_start();
    after.is_empty() || (after.starts_with('{') && after.ends_with('}'))
}

/// The `@connect` `http.body` expression of one root field, when it has one.
/// `body` is the root type's body text and `code` is `graphql::blank(body)`.
fn connector_body(body: &str, code: &str, field: &str) -> Option<String> {
    let text = crate::lint::body_field_text(body, code, field)?;
    let args = crate::graphql::directives(&text, "connect")
        .into_iter()
        .next()?
        .args;
    let http = crate::obligations::find_value(&args, "http")?;
    crate::obligations::find_value(&http, "body")
}

/// Where a root field's argument lands in its operation's request, first
/// found wins:
/// 1. the connector's body forwards it whole (`body: "$args.input"`): the
///    request body;
/// 2. a body entry forwards it (`fields: $args.fields`): that property of
///    the request body;
/// 3. a parameter of the same name, compared without case or separators;
/// 4. a request-body property of the same name.
///
/// An input-typed argument (`input_fields` is its type's field names) also
/// takes the request body when the body names any of those fields, the
/// whole-input forward a connector may write as a sub-selection. An
/// operation that documents none of these leaves the argument
/// `Undocumented`.
fn argument_position(
    body_expr: Option<&str>,
    arg: &str,
    op: &Value,
    shapes: &Object,
    input_fields: Option<&[String]>,
) -> Located {
    let body_root = crate::json::get(op, "request_body")
        .and_then(|b| get_str(b, "shape_ref"))
        .map(|r| serde_json::json!({ "$ref": r }));
    let body_element = body_root.as_ref().map(|b| element_shape(b, shapes));
    if let Some(expr) = body_expr {
        if forwards_argument(expr, arg) {
            return match &body_root {
                Some(b) => Located::Shape(b.clone()),
                None => Located::Undocumented,
            };
        }
        for (key, value) in crate::obligations::parse_body_pairs(expr) {
            if forwards_argument(&value, arg) {
                return match body_element.as_ref().and_then(|b| property(b, &key)) {
                    Some(p) => Located::Shape(p.clone()),
                    None => Located::Undocumented,
                };
            }
        }
    }
    let wanted = loose_name(arg);
    if let Some(p) = get_arr(op, "parameters")
        .into_iter()
        .flatten()
        .find(|p| get_str(p, "name").map(loose_name).as_deref() == Some(wanted.as_str()))
    {
        return Located::Shape(parameter_shape(p));
    }
    if let Some(p) = body_element.as_ref().and_then(|b| property(b, arg)) {
        return Located::Shape(p.clone());
    }
    if let (Some(fields), Some(b), Some(element)) = (input_fields, &body_root, &body_element) {
        if overlap(element, fields) > 0 {
            return Located::Shape(b.clone());
        }
    }
    Located::Undocumented
}

/// The request side's positions: every root field argument (keyed
/// `(root, field, arg)`) and every input object type, located the way
/// [`locate_types`] locates object types:
/// 1. an input type by name, `type_prefix_` stripped;
/// 2. an argument through its operation's request ([`argument_position`]),
///    and an input type through the first argument of that type;
/// 3. an input type through a located parent input type's property.
///
/// A root field the selection ties to no operation leaves its arguments,
/// and any input type reached only through them, absent: `unresolved`.
#[allow(clippy::type_complexity)]
fn locate_inputs(
    sdl: &str,
    doc: &ast::Document,
    shapes: &Object,
    roots: &std::collections::HashMap<String, (&Value, Option<String>)>,
    type_prefix: &str,
) -> (
    std::collections::HashMap<(String, String, String), Located>,
    std::collections::HashMap<String, Located>,
) {
    use std::collections::HashMap;
    // Input type -> (field name, bare field type).
    let mut inputs: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for def in &doc.definitions {
        if let Definition::InputObjectTypeDefinition(i) = def {
            inputs.insert(
                i.name.to_string(),
                i.fields
                    .iter()
                    .map(|f| (f.name.to_string(), bare_type_name(&f.ty).to_string()))
                    .collect(),
            );
        }
    }
    let mut types: HashMap<String, Located> = HashMap::new();
    for name in inputs.keys() {
        if let Some(shape) = shapes.get(bare_shape_name(name, type_prefix)) {
            types.insert(
                name.clone(),
                Located::Shape(deref(shape, shapes, &mut HashSet::new())),
            );
        }
    }

    let mut args: HashMap<(String, String, String), Located> = HashMap::new();
    for def in &doc.definitions {
        let Definition::ObjectTypeDefinition(o) = def else {
            continue;
        };
        let root = o.name.as_str();
        if root != "Query" && root != "Mutation" {
            continue;
        }
        let Some(root_body) = crate::graphql::type_body(sdl, root) else {
            continue;
        };
        let root_code = crate::graphql::blank(&root_body.body);
        for f in &o.fields {
            let Some((op, _)) = roots.get(f.name.as_str()) else {
                continue;
            };
            if f.arguments.is_empty() {
                continue;
            }
            let body_expr = connector_body(&root_body.body, &root_code, f.name.as_str());
            for a in &f.arguments {
                let ty = bare_type_name(&a.ty);
                let fields: Option<Vec<String>> = inputs
                    .get(ty)
                    .map(|fs| fs.iter().map(|(n, _)| n.clone()).collect());
                let at = argument_position(
                    body_expr.as_deref(),
                    a.name.as_str(),
                    op,
                    shapes,
                    fields.as_deref(),
                );
                if fields.is_some() && !types.contains_key(ty) {
                    let child = match &at {
                        Located::Shape(s) => Located::Shape(element_shape(s, shapes)),
                        Located::Undocumented => Located::Undocumented,
                    };
                    types.insert(ty.to_string(), child);
                }
                args.insert(
                    (root.to_string(), f.name.to_string(), a.name.to_string()),
                    at,
                );
            }
        }
    }

    loop {
        let mut found: Vec<(String, Located)> = Vec::new();
        for (name, fields) in &inputs {
            let Some(parent) = types.get(name) else {
                continue;
            };
            for (field, ty) in fields {
                if !inputs.contains_key(ty)
                    || types.contains_key(ty)
                    || found.iter().any(|(t, _)| t == ty)
                {
                    continue;
                }
                let child = match parent {
                    Located::Undocumented => Located::Undocumented,
                    Located::Shape(s) => match property(s, field) {
                        Some(p) => Located::Shape(element_shape(p, shapes)),
                        None => Located::Undocumented,
                    },
                };
                found.push((ty.clone(), child));
            }
        }
        if found.is_empty() {
            break;
        }
        types.extend(found);
    }
    (args, types)
}

/// Follow a `{"$ref": "#/shapes/Name"}` chain to its concrete shape,
/// cycle-safe. A shape with no `$ref` is returned as-is.
fn deref(shape: &Value, shapes: &Object, seen: &mut HashSet<String>) -> Value {
    match get_str(shape, "$ref") {
        Some(r) => {
            let name = r.rsplit('/').next().unwrap_or(r).to_string();
            if !seen.insert(name.clone()) {
                return shape.clone();
            }
            match shapes.get(&name) {
                Some(next) => deref(next, shapes, seen),
                None => shape.clone(),
            }
        }
        None => shape.clone(),
    }
}

/// Whether a shape is JSON Schema's `null` type on its own (`{"type":
/// "null"}`), the second branch of a nullable `anyOf`.
fn is_null_type(shape: &Value) -> bool {
    get_str(shape, "type") == Some("null")
}

/// A nullable wrapper is the shape it wraps: `anyOf`/`oneOf` with exactly
/// one non-null branch (the FastAPI spelling, `anyOf: [{X}, {type: null}]`)
/// folds to that branch, and an OpenAPI 3.1 type array (`["integer",
/// "null"]`) to its one non-null type. Nullability is not polymorphism, so
/// neither is a reason to keep the field JSON. Anything else is returned
/// as-is.
fn fold_nullable(shape: &Value) -> Value {
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = shape.get(key) {
            let mut real = branches.iter().filter(|b| !is_null_type(b));
            if let (Some(only), None) = (real.next(), real.next()) {
                if branches.len() > 1 && shape.get("discriminator").is_none() {
                    return fold_nullable(only);
                }
            }
        }
    }
    if let Some(Value::Array(types)) = shape.get("type") {
        let mut real = types.iter().filter(|t| t.as_str() != Some("null"));
        if let (Some(only @ Value::String(_)), None) = (real.next(), real.next()) {
            let mut folded = shape.clone();
            if let Value::Object(m) = &mut folded {
                m.insert("type".to_string(), only.clone());
            }
            return folded;
        }
    }
    shape.clone()
}

/// `deref` with a nullable wrapper folded on either side of the `$ref`.
fn resolve(shape: &Value, shapes: &Object) -> Value {
    let folded = fold_nullable(&deref(shape, shapes, &mut HashSet::new()));
    fold_nullable(&deref(&folded, shapes, &mut HashSet::new()))
}

fn is_free_form_object(resolved: &Value) -> bool {
    get_str(resolved, "type") == Some("object")
        && get_obj(resolved, "properties")
            .map(|p| p.is_empty())
            .unwrap_or(true)
}

fn is_polymorphic_without_discriminator(resolved: &Value) -> bool {
    (resolved.get("oneOf").is_some() || resolved.get("anyOf").is_some())
        && resolved.get("discriminator").is_none()
}

/// Whether resolving `shape`'s properties/items/oneOf/anyOf/allOf ever
/// reaches a `$ref` naming `target`, bounded to `depth` hops.
fn refers_back_to(
    shape: &Value,
    target: &str,
    shapes: &Object,
    depth: usize,
    seen: &mut HashSet<String>,
) -> bool {
    if depth == 0 {
        return false;
    }
    if let Some(r) = get_str(shape, "$ref") {
        let name = r.rsplit('/').next().unwrap_or(r);
        if name == target {
            return true;
        }
        if !seen.insert(name.to_string()) {
            return false;
        }
        return shapes
            .get(name)
            .is_some_and(|next| refers_back_to(next, target, shapes, depth - 1, seen));
    }
    if let Some(props) = get_obj(shape, "properties") {
        for v in props.values() {
            if refers_back_to(v, target, shapes, depth - 1, seen) {
                return true;
            }
        }
    }
    for key in ["items", "oneOf", "anyOf", "allOf"] {
        match shape.get(key) {
            Some(Value::Array(arr)) => {
                for v in arr {
                    if refers_back_to(v, target, shapes, depth - 1, seen) {
                        return true;
                    }
                }
            }
            Some(v) => {
                if refers_back_to(v, target, shapes, depth - 1, seen) {
                    return true;
                }
            }
            None => {}
        }
    }
    false
}

/// "The shape refers back to itself": starting from `raw`'s own `$ref`
/// target, does resolving refs ever loop back to that same name. When `raw`
/// carries no `$ref` it is recursive only if it is an inline composite (an
/// object with properties, or a union) that reaches back to the enclosing
/// type: a primitive or an enum never is, whatever its siblings do.
fn is_recursive(raw: &Value, type_bare: &str, shapes: &Object) -> bool {
    // A list field's `$ref` sits on its `items` (`parts: [MessagePart]`),
    // and a nullable one's on its one non-null branch.
    let mut element = fold_nullable(raw);
    while get_str(&element, "$ref").is_none() {
        match element.get("items") {
            Some(items) => element = fold_nullable(items),
            None => break,
        }
    }
    match get_str(&element, "$ref") {
        Some(r) => {
            let target = r.rsplit('/').next().unwrap_or(r).to_string();
            let start = match shapes.get(&target) {
                Some(s) => s,
                None => return false,
            };
            let mut seen = HashSet::new();
            seen.insert(target.clone());
            refers_back_to(start, &target, shapes, 8, &mut seen)
        }
        None => {
            let composite = get_obj(&element, "properties").is_some_and(|p| !p.is_empty())
                || ["oneOf", "anyOf", "allOf"]
                    .iter()
                    .any(|k| element.get(*k).is_some());
            if !composite {
                return false;
            }
            let mut seen = HashSet::new();
            seen.insert(type_bare.to_string());
            refers_back_to(&element, type_bare, shapes, 8, &mut seen)
        }
    }
}

fn predicate_holds(reason: &str, found: Option<&Value>, type_bare: &str, shapes: &Object) -> bool {
    match reason {
        "vendor-undocumented" => found.is_none(),
        "free-form-object" => found
            .map(|raw| is_free_form_object(&resolve(raw, shapes)))
            .unwrap_or(false),
        "polymorphic-without-discriminator" => found
            .map(|raw| is_polymorphic_without_discriminator(&resolve(raw, shapes)))
            .unwrap_or(false),
        "recursive" => found
            .map(|raw| is_recursive(raw, type_bare, shapes))
            .unwrap_or(false),
        // Evaluated in `build`, which knows the type's depth; reaching here
        // means it did not hold.
        "depth-cap" => false,
        _ => false,
    }
}

/// Independent of any recorded reason: has the inventory moved past every
/// one of the four problem categories at this field's path — a real
/// primitive, a non-empty object, an array, or an enum, and neither
/// recursive nor an undiscriminated union.
fn is_recoverable(found: Option<&Value>, type_bare: &str, shapes: &Object) -> bool {
    let raw = match found {
        Some(r) => r,
        None => return false,
    };
    let resolved = resolve(raw, shapes);
    if is_free_form_object(&resolved) {
        return false;
    }
    if is_polymorphic_without_discriminator(&resolved) {
        return false;
    }
    if is_recursive(raw, type_bare, shapes) {
        return false;
    }
    match get_str(&resolved, "type") {
        Some("object") => get_obj(&resolved, "properties")
            .map(|p| !p.is_empty())
            .unwrap_or(false),
        // An array is typed only when its items are: `[{}]` or an array of
        // free-form maps is still JSON's job.
        Some("array") => resolved
            .get("items")
            .is_some_and(|items| is_recoverable(Some(items), type_bare, shapes)),
        Some("string") | Some("integer") | Some("number") | Some("boolean") => true,
        _ => resolved.get("enum").is_some(),
    }
}

/// The schema is an ordinary workspace file the user edits by hand, so it is
/// read as a named path (ADR 0025): through custody when the path points
/// inside `.factory/`, plainly otherwise. A later switch to
/// `reconcile::read_schema_file` (ADR 0075, once it is on main) replaces this.
fn read_schema(dir: &Path, schema_file: &str) -> Result<String, String> {
    let bytes = crate::factory_io::read_named_path(&dir.join(schema_file))?;
    String::from_utf8(bytes).map_err(|_| format!("{}: not valid UTF-8", schema_file))
}

/// One `json_reasons` entry, plus the id of the resolved decision
/// that carries it, so a report can point at the record the same way
/// `json_fields` used to carry an inline `decision:` pointer.
struct JsonReasonEntry {
    type_name: String,
    field: String,
    reason: String,
    decision: String,
}

/// Every `json_reasons` entry on a **resolved** decision in
/// `.factory/decisions.json` (ADR 0073). An open or superseded record's
/// entries do not count; the schema/selection are never read for this —
/// per-field reasons live only here (the decisions-only rule).
fn json_reason_decisions(doc: &Value) -> Vec<JsonReasonEntry> {
    get_arr(doc, "decisions")
        .into_iter()
        .flatten()
        .filter(|d| get_str(d, "status") == Some("resolved"))
        .flat_map(|d| {
            let id = get_str(d, "id").unwrap_or_default().to_string();
            get_arr(d, "json_reasons")
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |entry| (id.clone(), entry))
        })
        .filter_map(|(id, entry)| {
            Some(JsonReasonEntry {
                type_name: get_str(&entry, "type")?.to_string(),
                field: get_str(&entry, "field")?.to_string(),
                reason: get_str(&entry, "reason")?.to_string(),
                decision: id,
            })
        })
        .collect()
}

pub fn build(dir: &Path) -> Result<Report, String> {
    let workspace_text = crate::factory_io::read_to_string(dir, ".factory/workspace.yaml")
        .map_err(|e| e.to_string())?;
    let workspace = crate::yaml::parse(&workspace_text)?;
    let type_prefix = get_str(&workspace, "type_prefix")
        .ok_or_else(|| ".factory/workspace.yaml has no type_prefix".to_string())?;
    let json_scalar = format!("{}_JSON", type_prefix);
    let field_prefix = get_str(&workspace, "field_prefix").unwrap_or("");

    let schema_file = crate::reconcile::schema_file_of(&workspace)?;
    if !dir.join(&schema_file).exists() {
        return Err(format!(
            "not_run: {} does not exist yet (nothing to check)",
            schema_file
        ));
    }
    let sdl = read_schema(dir, &schema_file)?;

    // Per-field reasons live only in decisions.json, never selection.yaml or
    // a schema doc comment (ADR 0073, the decisions-only rule). A missing log is normal
    // (no reason has ever been recorded), same as a missing selection used
    // to be for this instrument.
    let json_reasons = match crate::decisions::load_present(dir, None)
        .map_err(|e| format!("the decisions log: {}", e))?
    {
        Some(doc) => json_reason_decisions(&doc),
        None => Vec::new(),
    };

    // A missing inventory is normal (a no-spec or freshly initialized
    // workspace); every predicate then reads as "no shape recorded there".
    let inventory: Option<Value> =
        match crate::factory_io::read_to_string_optional(dir, ".factory/inventory.json")
            .map_err(|e| e.to_string())?
        {
            Some(text) => Some(
                crate::json::parse(&text).map_err(|e| format!(".factory/inventory.json: {}", e))?,
            ),
            None => None,
        };
    let shapes: Object = inventory
        .as_ref()
        .and_then(|i| get_obj(i, "shapes").cloned())
        .unwrap_or_default();
    // The selection ties root fields to operations, the second way a type's
    // shape is located; a missing one leaves only the other two.
    let selection: Option<Value> =
        match crate::factory_io::read_to_string_optional(dir, ".factory/selection.yaml")
            .map_err(|e| e.to_string())?
        {
            Some(text) => Some(crate::yaml::parse(&text)?),
            None => None,
        };

    let doc = ast::Document::parse(&sdl, schema_file.as_str()).map_err(|e| e.to_string())?;

    let located = locate_types(
        &doc,
        &shapes,
        inventory.as_ref(),
        selection.as_ref(),
        type_prefix,
        field_prefix,
    );

    // The workspace's stated cut (6, the skill's default, when unset).
    let max_depth = selection
        .as_ref()
        .and_then(|s| crate::json::get(s, "defaults"))
        .and_then(|d| crate::json::get(d, "max_depth"))
        .and_then(Value::as_u64)
        .unwrap_or(6) as usize;
    let deep = deep_types(&doc, max_depth);
    let roots = root_operations(inventory.as_ref(), selection.as_ref(), field_prefix);
    let (arg_positions, input_types) = locate_inputs(&sdl, &doc, &shapes, &roots, type_prefix);

    // One JSON position: its key, where it was found (`None`: nowhere to
    // look, so `unresolved`), and whether its type sits at the depth cut.
    let row = |type_name: &str,
               field_name: String,
               position: Position,
               at: Option<Option<&Value>>,
               deep_type: bool|
     -> FieldRow {
        let type_bare = bare_shape_name(type_name, type_prefix).to_string();
        // The latest resolved record that names the field wins: a later
        // decision is the newer judgement, as a resolve --force would be.
        let entry = json_reasons
            .iter()
            .rev()
            .find(|j| j.type_name == type_name && j.field == field_name);
        let reason = entry
            .map(|j| j.reason.as_str())
            .filter(|r| REASONS.contains(r))
            .map(str::to_string);
        let decision = entry.map(|j| j.decision.clone());
        let unresolved = at.is_none();
        let found = at.flatten();
        // A holding depth-cap outranks recoverable: re-typing the field
        // would undo the workspace's stated cut, which is the reason.
        let depth_cap = reason.as_deref() == Some("depth-cap")
            && !unresolved
            && depth_cap_holds(found, &type_bare, deep_type, &shapes);
        let status = if depth_cap {
            Status::Accounted
        } else if is_recoverable(found, &type_bare, &shapes) {
            Status::Recoverable
        } else {
            match &reason {
                None => Status::Unaccounted,
                Some(_) if unresolved => Status::Unresolved,
                Some(r) => {
                    if predicate_holds(r, found, &type_bare, &shapes) {
                        Status::Accounted
                    } else {
                        Status::Stale
                    }
                }
            }
        };
        FieldRow {
            type_name: type_name.to_string(),
            field_name,
            position,
            reason,
            decision,
            status,
        }
    };
    let shape_at = |located: Option<&Located>, field: &str| -> Option<Option<Value>> {
        match located {
            Some(Located::Shape(s)) => Some(property(s, field).cloned()),
            Some(Located::Undocumented) => Some(None),
            None => None,
        }
    };

    let mut total_fields = 0usize;
    let mut rows = Vec::new();
    for def in &doc.definitions {
        match def {
            Definition::ObjectTypeDefinition(o) => {
                let type_name = o.name.to_string();
                let is_root = type_name == "Query" || type_name == "Mutation";
                for f in &o.fields {
                    total_fields += 1;
                    if is_root {
                        for a in &f.arguments {
                            total_fields += 1;
                            if bare_type_name(&a.ty) != json_scalar {
                                continue;
                            }
                            let key = (type_name.clone(), f.name.to_string(), a.name.to_string());
                            let at = arg_positions.get(&key).map(|l| match l {
                                Located::Shape(s) => Some(s.clone()),
                                Located::Undocumented => None,
                            });
                            rows.push(row(
                                &type_name,
                                format!("{}({}:)", f.name, a.name),
                                Position::Argument,
                                at.as_ref().map(Option::as_ref),
                                false,
                            ));
                        }
                    }
                    if bare_type_name(&f.ty) != json_scalar {
                        continue;
                    }
                    // A root field's own position is its operation's
                    // response; any other field's is its type's property.
                    let at = if is_root {
                        roots.get(f.name.as_str()).map(|(op, envelope)| {
                            match response_position(op, envelope.as_deref(), &shapes) {
                                Located::Shape(s) => Some(s),
                                Located::Undocumented => None,
                            }
                        })
                    } else {
                        shape_at(located.get(&type_name), f.name.as_str())
                    };
                    rows.push(row(
                        &type_name,
                        f.name.to_string(),
                        Position::Output,
                        at.as_ref().map(Option::as_ref),
                        deep.contains(&type_name),
                    ));
                }
            }
            Definition::InputObjectTypeDefinition(i) => {
                let type_name = i.name.to_string();
                for f in &i.fields {
                    total_fields += 1;
                    if bare_type_name(&f.ty) != json_scalar {
                        continue;
                    }
                    let at = shape_at(input_types.get(&type_name), f.name.as_str());
                    rows.push(row(
                        &type_name,
                        f.name.to_string(),
                        Position::Input,
                        at.as_ref().map(Option::as_ref),
                        false,
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(Report {
        json_scalar,
        total_fields,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn workspace(dir: &Path) {
        write(
            dir,
            ".factory/workspace.yaml",
            "directory: widgets\ntype_prefix: Widgets\nfield_prefix: widgets\n",
        );
    }

    /// A `.factory/decisions.json` with one record carrying the given
    /// `json_reasons` entries (`(type, field, reason)`), `resolved` unless
    /// `open` is true (an open record's entries do not count, ADR 0073).
    fn decisions_with_json_reasons(
        dir: &Path,
        id: &str,
        open: bool,
        entries: &[(&str, &str, &str)],
    ) {
        let reasons: Vec<String> = entries
            .iter()
            .map(|(t, f, r)| format!(r#"{{"type":"{}","field":"{}","reason":"{}"}}"#, t, f, r))
            .collect();
        let resolution = if open {
            String::new()
        } else {
            r#","resolution":{"decision":"test fixture"}"#.to_string()
        };
        write(
            dir,
            ".factory/decisions.json",
            &format!(
                r#"{{"contract_version":1,"decisions":[{{"id":"{}","title":"t","status":"{}","date":"2026-01-01","json_reasons":[{}]{}}}]}}"#,
                id,
                if open { "open" } else { "resolved" },
                reasons.join(","),
                resolution
            ),
        );
    }

    fn inventory(dir: &Path, shapes_json: &str) {
        write(
            dir,
            ".factory/inventory.json",
            &format!("{{\"shapes\":{}}}", shapes_json),
        );
    }

    const SDL_ONE_JSON_FIELD: &str = r#"
scalar Widgets_JSON

type Widgets_Widget {
  id: ID!
  name: String
  extra: Widgets_JSON
}

type Query {
  widget: Widgets_Widget
}
"#;

    fn row<'a>(report: &'a Report, key: &str) -> &'a FieldRow {
        report.rows.iter().find(|r| r.key() == key).unwrap()
    }

    #[test]
    fn an_unrecorded_json_field_is_unaccounted() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        let report = build(tmp.path()).unwrap();
        assert_eq!(report.rows.len(), 1);
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Unaccounted
        );
    }

    /// The defect this instrument replaces (ADR 0073): a `defaults.fields:
    /// all` selection, with the JSON field actually included and no
    /// field-level reason recorded, must still be flagged red — and once a
    /// reason whose predicate genuinely holds is recorded (in decisions.json,
    /// never the selection that carries the `defaults.fields: all` policy),
    /// it goes green.
    #[test]
    fn defaults_fields_all_is_never_itself_a_reason() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        write(
            tmp.path(),
            ".factory/selection.yaml",
            "contract_version: 1\ndefaults:\n  fields: all\noperations:\n  \"get:/widget\":\n    include: true\n    fields:\n      include: [id, name, extra]\n",
        );
        let red = build(tmp.path()).unwrap();
        assert!(red.check_failure().is_some());
        assert_eq!(
            row(&red, "Widgets_Widget.extra").status,
            Status::Unaccounted
        );

        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "free-form-object")],
        );
        let green = build(tmp.path()).unwrap();
        assert!(green.check_failure().is_none());
        assert_eq!(
            row(&green, "Widgets_Widget.extra").status,
            Status::Accounted
        );
    }

    #[test]
    fn a_recorded_reason_from_the_closed_list_accounts_for_the_field() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "free-form-object")],
        );
        let report = build(tmp.path()).unwrap();
        let r = row(&report, "Widgets_Widget.extra");
        assert_eq!(r.status, Status::Accounted);
        assert_eq!(r.reason.as_deref(), Some("free-form-object"));
        assert_eq!(r.decision.as_deref(), Some("D-0001"));
        assert!(report.check_failure().is_none());
    }

    /// The CLI refuses `--json-reason` with an unknown value before it is
    /// ever written (`tests/integration/decisions.rs`); this proves the second,
    /// independent guard — `decisions.schema.json`'s own enum — refuses to
    /// even load a hand-edited or legacy record outside the closed list,
    /// rather than silently reading it as no reason at all.
    #[test]
    fn a_reason_outside_the_closed_list_fails_to_load_not_silently_unaccounted() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "because-i-said-so")],
        );
        let err = build(tmp.path()).unwrap_err();
        assert!(err.contains("decisions.json"), "{}", err);
    }

    /// ADR 0073, the decisions-only rule: only a **resolved** decision's `json_reasons`
    /// count. An open record's entry is exactly as if nothing were recorded.
    #[test]
    fn only_a_resolved_decisions_json_reasons_count() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            true,
            &[("Widgets_Widget", "extra", "free-form-object")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Unaccounted
        );
    }

    #[test]
    fn a_json_field_nested_two_levels_deep_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(
            tmp.path(),
            "widgets.graphql",
            r#"
scalar Widgets_JSON

type Widgets_Detail {
  blob: Widgets_JSON
}

type Widgets_Widget {
  id: ID!
  details: [Widgets_Detail]
}

type Query {
  widget: Widgets_Widget
}
"#,
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].key(), "Widgets_Detail.blob");
    }

    #[test]
    fn a_list_and_non_null_wrapped_json_field_is_found() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(
            tmp.path(),
            "widgets.graphql",
            r#"
scalar Widgets_JSON

type Widgets_Widget {
  id: ID!
  tags: [Widgets_JSON!]!
}

type Query {
  widget: Widgets_Widget
}
"#,
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].key(), "Widgets_Widget.tags");
    }

    #[test]
    fn no_json_scalar_declared_reports_zero_fields() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(
            tmp.path(),
            "widgets.graphql",
            "type Widgets_Widget {\n  id: ID!\n  name: String\n}\n\ntype Query {\n  widget: Widgets_Widget\n}\n",
        );
        let report = build(tmp.path()).unwrap();
        assert!(report.rows.is_empty());
        // Widgets_Widget (id, name) + Query (widget) = 3 fields, 0 JSON.
        assert_eq!(report.typed_to_json_ratio(), Some((3, 0)));
    }

    #[test]
    fn a_missing_decisions_json_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        // No .factory/decisions.json at all — no reason has ever been
        // recorded, same lenience the old selection.yaml-backed read had.
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Unaccounted
        );
    }

    #[test]
    fn typed_to_json_ratio_counts_every_object_type_field() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        let report = build(tmp.path()).unwrap();
        // Widgets_Widget (id, name, extra) + Query (widget) = 4 fields, 1 JSON.
        assert_eq!(report.typed_to_json_ratio(), Some((3, 1)));
    }

    #[test]
    fn no_type_prefix_is_an_error_not_a_panic() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".factory/workspace.yaml",
            "directory: widgets\n",
        );
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        assert!(build(tmp.path()).is_err());
    }

    #[test]
    fn a_missing_schema_file_reports_not_run_not_a_generic_error() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        let err = build(tmp.path()).unwrap_err();
        assert!(err.starts_with("not_run:"), "{}", err);
    }

    // --- one test per reason, true then false --------------------------

    #[test]
    fn free_form_object_true_accounts_for_the_field() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "free-form-object")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Accounted
        );
    }

    #[test]
    fn free_form_object_false_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        // Really a polymorphic union without a discriminator now, not a
        // free-form object: the recorded reason no longer fits, and the
        // real shape is still a different one of the four problems, so
        // this is stale rather than recoverable.
        inventory(
            tmp.path(),
            r##"{"Widget":{"properties":{"extra":{"oneOf":[{"$ref":"#/shapes/A"},{"$ref":"#/shapes/B"}]}}},"A":{"type":"object"},"B":{"type":"object"}}"##,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "free-form-object")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(row(&report, "Widgets_Widget.extra").status, Status::Stale);
    }

    #[test]
    fn recursive_true_accounts_for_the_field() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r##"{"Widget":{"properties":{"extra":{"$ref":"#/shapes/Node"}}},"Node":{"type":"object","properties":{"child":{"$ref":"#/shapes/Node"}}}}"##,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "recursive")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Accounted
        );
    }

    #[test]
    fn recursive_false_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        // A genuine free-form object, not recursive: the recorded reason
        // is wrong, and it is still a (different) open problem.
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "recursive")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(row(&report, "Widgets_Widget.extra").status, Status::Stale);
    }

    #[test]
    fn vendor_undocumented_true_accounts_for_the_field() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        // The Widget shape is recorded, without the property.
        inventory(
            tmp.path(),
            r#"{"Widget":{"type":"object","properties":{"id":{"type":"string"}}}}"#,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "vendor-undocumented")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Accounted
        );
    }

    /// The operation serving the root field documents no response shape:
    /// the type is located as undocumented, so vendor-undocumented holds.
    #[test]
    fn vendor_undocumented_holds_under_an_operation_with_no_response_shape() {
        let tmp = tempfile::tempdir().unwrap();
        inline_items_workspace(tmp.path(), "null");
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_AuditEvent", "data", "vendor-undocumented")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_AuditEvent.data").status,
            Status::Accounted
        );
    }

    /// No shape reached by name, by a root field's operation, or by a
    /// parent: the recorded claim cannot be checked, for any reason
    /// (vendor-undocumented included), and is an open finding.
    #[test]
    fn a_type_with_no_located_shape_is_unresolved_not_stale_or_accounted() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(tmp.path(), r#"{"Other":{"type":"object"}}"#);
        for reason in REASONS {
            decisions_with_json_reasons(
                tmp.path(),
                "D-0001",
                false,
                &[("Widgets_Widget", "extra", reason)],
            );
            let report = build(tmp.path()).unwrap();
            let r = row(&report, "Widgets_Widget.extra");
            assert_eq!(r.status, Status::Unresolved, "{}", reason);
            assert!(r.status.is_open_finding());
            assert!(report.check_failure().unwrap().contains("(unresolved)"));
        }
    }

    #[test]
    fn vendor_undocumented_false_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        // The vendor now documents it (as a free-form object, so it does
        // not also read as recoverable).
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "vendor-undocumented")],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(row(&report, "Widgets_Widget.extra").status, Status::Stale);
    }

    #[test]
    fn polymorphic_without_discriminator_true_accounts_for_the_field() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r##"{"Widget":{"properties":{"extra":{"oneOf":[{"$ref":"#/shapes/A"},{"$ref":"#/shapes/B"}]}}},"A":{"type":"object"},"B":{"type":"object"}}"##,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[(
                "Widgets_Widget",
                "extra",
                "polymorphic-without-discriminator",
            )],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Accounted
        );
    }

    #[test]
    fn polymorphic_without_discriminator_false_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        // A discriminator is now documented: the union is no longer
        // undiscriminated, but resolving it into a concrete type is a
        // separate judgement (ADR item 6) this instrument does not make,
        // so it is stale, not recoverable.
        inventory(
            tmp.path(),
            r##"{"Widget":{"properties":{"extra":{"oneOf":[{"$ref":"#/shapes/A"},{"$ref":"#/shapes/B"}],"discriminator":{"propertyName":"kind"}}}},"A":{"type":"object"},"B":{"type":"object"}}"##,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[(
                "Widgets_Widget",
                "extra",
                "polymorphic-without-discriminator",
            )],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(row(&report, "Widgets_Widget.extra").status, Status::Stale);
    }

    #[test]
    fn recoverable_overrides_a_recorded_reason() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        // Recorded when the vendor documented nothing; the spec has since
        // caught up with a plain, fully typed string.
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"string"}}}}"#,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Widgets_Widget", "extra", "vendor-undocumented")],
        );
        let report = build(tmp.path()).unwrap();
        let r = row(&report, "Widgets_Widget.extra");
        assert_eq!(r.status, Status::Recoverable);
        assert!(r.status.is_open_finding());
        assert!(report.check_failure().is_some());
    }

    /// Granola's audit events (the 2026-09-29 dry run, D-0007): the item
    /// shape is inline under `GetV1AuditResponse.properties.events.items`,
    /// never promoted to its own `shapes` entry, so the type's shape is only
    /// reachable through the root field's operation.
    const SDL_INLINE_ITEMS: &str = r#"
scalar Widgets_JSON

type Widgets_AuditEvent {
  id: ID!
  actor: Widgets_JSON
  data: Widgets_JSON
}

type Widgets_AuditEventList {
  events: [Widgets_AuditEvent]
  cursor: String
}

type Query {
  widgets_listAuditEvents: Widgets_AuditEventList
}
"#;

    fn inline_items_workspace(dir: &Path, shape_ref: &str) {
        workspace(dir);
        write(dir, "widgets.graphql", SDL_INLINE_ITEMS);
        write(
            dir,
            ".factory/selection.yaml",
            "contract_version: 1\noperations:\n  \"get:/v1/audit\":\n    include: true\n    graphql: { root: query, name: listAuditEvents }\n",
        );
        write(
            dir,
            ".factory/inventory.json",
            &format!(
                r#"{{"operations":[{{"key":"get:/v1/audit","response":{{"status":"200","shape_ref":{}}}}}],
"shapes":{{"GetV1AuditResponse":{{"type":"object","properties":{{
  "events":{{"type":"array","items":{{"type":"object","properties":{{
    "id":{{"type":"string"}},
    "actor":{{"oneOf":[{{"type":"object","properties":{{"object":{{"enum":["user"]}}}}}},{{"type":"object","properties":{{"object":{{"enum":["api_key"]}}}}}}]}},
    "data":{{"type":"object","additionalProperties":true}}}}}}}},
  "cursor":{{"type":"string"}}}}}}}}}}"#,
                shape_ref
            ),
        );
    }

    #[test]
    fn an_inline_item_shape_is_found_through_the_root_fields_operation() {
        let tmp = tempfile::tempdir().unwrap();
        inline_items_workspace(tmp.path(), r##""#/shapes/GetV1AuditResponse""##);
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[
                ("Widgets_AuditEvent", "data", "free-form-object"),
                (
                    "Widgets_AuditEvent",
                    "actor",
                    "polymorphic-without-discriminator",
                ),
            ],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_AuditEvent.data").status,
            Status::Accounted
        );
        assert_eq!(
            row(&report, "Widgets_AuditEvent.actor").status,
            Status::Accounted
        );
    }

    #[test]
    fn an_inline_item_shape_still_reads_stale_when_its_claim_is_false() {
        let tmp = tempfile::tempdir().unwrap();
        inline_items_workspace(tmp.path(), r##""#/shapes/GetV1AuditResponse""##);
        // `data` is a free-form object, not a union: the claim is false.
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[(
                "Widgets_AuditEvent",
                "data",
                "polymorphic-without-discriminator",
            )],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_AuditEvent.data").status,
            Status::Stale
        );
    }

    /// A chain of types three levels deep under the root field, each shape
    /// named so the inventory has it; `Widgets_Leaf.config` is JSON where
    /// the inventory has a structured object. `max_depth` is the selection's.
    fn depth_workspace(dir: &Path, max_depth: Option<u64>, config_shape: &str) {
        workspace(dir);
        write(
            dir,
            "widgets.graphql",
            r#"
scalar Widgets_JSON

type Widgets_Root {
  mid: Widgets_Mid
}

type Widgets_Mid {
  leaf: Widgets_Leaf
}

type Widgets_Leaf {
  config: Widgets_JSON
}

type Query {
  widgets_root: Widgets_Root
}
"#,
        );
        let defaults = max_depth
            .map(|d| format!("defaults:\n  max_depth: {}\n", d))
            .unwrap_or_default();
        write(
            dir,
            ".factory/selection.yaml",
            &format!("contract_version: 1\n{}operations: {{}}\n", defaults),
        );
        inventory(
            dir,
            &format!(
                r##"{{"Root":{{"type":"object","properties":{{"mid":{{"$ref":"#/shapes/Mid"}}}}}},
"Mid":{{"type":"object","properties":{{"leaf":{{"$ref":"#/shapes/Leaf"}}}}}},
"Leaf":{{"type":"object","properties":{{"config":{}}}}}}}"##,
                config_shape
            ),
        );
        decisions_with_json_reasons(
            dir,
            "D-0001",
            false,
            &[("Widgets_Leaf", "config", "depth-cap")],
        );
    }

    const STRUCTURED: &str = r#"{"type":"object","properties":{"target":{"type":"string"}}}"#;

    /// `Widgets_Leaf` is level 3; with `max_depth: 3` the field sits at the
    /// cut and the inventory has a typed object beyond it. The recorded
    /// depth-cap accounts for it, although the shape alone reads recoverable.
    #[test]
    fn depth_cap_holds_at_the_stated_depth_and_outranks_recoverable() {
        let tmp = tempfile::tempdir().unwrap();
        depth_workspace(tmp.path(), Some(3), STRUCTURED);
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Leaf.config").status,
            Status::Accounted
        );
    }

    /// The same field above the cut (`max_depth: 6`, the default): the
    /// claim is false, and the typed shape reads recoverable.
    #[test]
    fn depth_cap_does_not_hold_above_the_stated_depth() {
        for max_depth in [Some(6), None] {
            let tmp = tempfile::tempdir().unwrap();
            depth_workspace(tmp.path(), max_depth, STRUCTURED);
            let report = build(tmp.path()).unwrap();
            assert_eq!(
                row(&report, "Widgets_Leaf.config").status,
                Status::Recoverable,
                "{:?}",
                max_depth
            );
        }
    }

    /// At the cut but with nothing beyond it (a free-form object): the claim
    /// that the inventory has the shape past the cut is false.
    #[test]
    fn depth_cap_needs_a_shape_beyond_the_cut() {
        let tmp = tempfile::tempdir().unwrap();
        depth_workspace(tmp.path(), Some(3), r#"{"type":"object"}"#);
        let report = build(tmp.path()).unwrap();
        assert_eq!(row(&report, "Widgets_Leaf.config").status, Status::Stale);
    }

    /// A recursion cut holds at any depth: the gmail/slides `Level3` form.
    #[test]
    fn depth_cap_holds_on_a_recursion_cut() {
        let tmp = tempfile::tempdir().unwrap();
        depth_workspace(
            tmp.path(),
            None,
            r##"{"type":"array","items":{"$ref":"#/shapes/Leaf"}}"##,
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Leaf.config").status,
            Status::Accounted
        );
    }

    /// An array is typed only when its items are: Databricks' `details`
    /// (`items` with no schema) and rows of free-form maps are not
    /// recoverable.
    #[test]
    fn an_array_of_untyped_items_is_not_recoverable() {
        for items in [
            r#"{"description":"Schema is empty in the OpenAPI document."}"#,
            r#"{"type":"object","additionalProperties":{}}"#,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            workspace(tmp.path());
            write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
            inventory(
                tmp.path(),
                &format!(
                    r#"{{"Widget":{{"type":"object","properties":{{"extra":{{"type":"array","items":{}}}}}}}}}"#,
                    items
                ),
            );
            let report = build(tmp.path()).unwrap();
            assert_eq!(
                row(&report, "Widgets_Widget.extra").status,
                Status::Unaccounted,
                "{}",
                items
            );
        }
        // An array of strings still is.
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r#"{"Widget":{"type":"object","properties":{"extra":{"type":"array","items":{"type":"string"}}}}}"#,
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Recoverable
        );
    }

    /// Review of #156: a primitive on a type that recurses through *another*
    /// property is not itself recursive. It is recoverable, and neither
    /// `recursive` nor `depth-cap` accounts for it.
    #[test]
    fn a_primitive_beside_a_self_reference_is_recoverable_not_recursive() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(
            tmp.path(),
            "widgets.graphql",
            "scalar Widgets_JSON\n\ntype Widgets_Widget {\n  id: ID!\n  nickname: Widgets_JSON\n  note: Widgets_JSON\n  manager: Widgets_Widget\n}\n\ntype Query {\n  widget: Widgets_Widget\n}\n",
        );
        inventory(
            tmp.path(),
            r##"{"Widget":{"type":"object","properties":{"nickname":{"type":"string"},"note":{"type":"string"},"manager":{"$ref":"#/shapes/Widget"}}}}"##,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[
                ("Widgets_Widget", "nickname", "recursive"),
                ("Widgets_Widget", "note", "depth-cap"),
            ],
        );
        let report = build(tmp.path()).unwrap();
        for key in ["Widgets_Widget.nickname", "Widgets_Widget.note"] {
            assert_eq!(row(&report, key).status, Status::Recoverable, "{}", key);
        }
        assert!(report.check_failure().is_some());
    }

    /// Review of #156: `anyOf: [{$ref: X}, {type: null}]` is a nullable X,
    /// not an undiscriminated union, and a 3.1 `["integer", "null"]` is an
    /// integer. Both are recoverable.
    #[test]
    fn a_nullable_wrapper_is_the_shape_it_wraps() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(
            tmp.path(),
            "widgets.graphql",
            "scalar Widgets_JSON\n\ntype Widgets_Widget {\n  id: ID!\n  address: Widgets_JSON\n  count: Widgets_JSON\n  either: Widgets_JSON\n}\n\ntype Query {\n  widget: Widgets_Widget\n}\n",
        );
        inventory(
            tmp.path(),
            r##"{"Widget":{"type":"object","properties":{
                "address":{"anyOf":[{"$ref":"#/shapes/Address"},{"type":"null"}]},
                "count":{"type":["integer","null"]},
                "either":{"anyOf":[{"$ref":"#/shapes/Address"},{"type":"string"}]}}},
              "Address":{"type":"object","properties":{"city":{"type":"string"}}}}"##,
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[
                (
                    "Widgets_Widget",
                    "address",
                    "polymorphic-without-discriminator",
                ),
                (
                    "Widgets_Widget",
                    "either",
                    "polymorphic-without-discriminator",
                ),
            ],
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.address").status,
            Status::Recoverable
        );
        assert_eq!(
            row(&report, "Widgets_Widget.count").status,
            Status::Recoverable
        );
        // A real two-branch union is still what the reason says it is.
        assert_eq!(
            row(&report, "Widgets_Widget.either").status,
            Status::Accounted
        );
    }

    /// A later resolved record naming the same field is the newer judgement.
    #[test]
    fn the_latest_resolved_record_for_a_field_wins() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        write(
            tmp.path(),
            ".factory/decisions.json",
            r#"{"contract_version":1,"decisions":[
              {"id":"D-0001","title":"t","status":"resolved","date":"2026-01-01","resolution":{"decision":"d"},
               "json_reasons":[{"type":"Widgets_Widget","field":"extra","reason":"recursive"}]},
              {"id":"D-0002","title":"t","status":"resolved","date":"2026-01-02","resolution":{"decision":"d"},
               "json_reasons":[{"type":"Widgets_Widget","field":"extra","reason":"free-form-object"}]}]}"#,
        );
        let report = build(tmp.path()).unwrap();
        let r = row(&report, "Widgets_Widget.extra");
        assert_eq!(r.status, Status::Accounted);
        assert_eq!(r.decision.as_deref(), Some("D-0002"));
    }

    /// ADR 0103: a superseded record stops counting, like an open one.
    #[test]
    fn a_superseded_decisions_json_reasons_do_not_count() {
        let tmp = tempfile::tempdir().unwrap();
        workspace(tmp.path());
        write(tmp.path(), "widgets.graphql", SDL_ONE_JSON_FIELD);
        inventory(
            tmp.path(),
            r#"{"Widget":{"properties":{"extra":{"type":"object"}}}}"#,
        );
        write(
            tmp.path(),
            ".factory/decisions.json",
            r#"{"contract_version":1,"decisions":[
              {"id":"D-0001","title":"t","status":"superseded","date":"2026-01-01","resolution":{"decision":"d"},
               "json_reasons":[{"type":"Widgets_Widget","field":"extra","reason":"free-form-object"}]}]}"#,
        );
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Widgets_Widget.extra").status,
            Status::Unaccounted
        );
    }

    // ── Phase 9d: the request side and root fields ──────────────────────

    const REQUEST_SDL: &str = r#"
scalar Widgets_JSON

type Widgets_Widget {
  id: ID!
}

input Widgets_WidgetInput {
  name: String
  meta: Widgets_JSON
}

type Query {
  widgets_list(
    pageSize: Widgets_JSON
  ): Widgets_JSON
    @connect(source: "w", http: { GET: "/widgets", queryParams: "page-size: $args.pageSize" }, selection: "$")
}

type Mutation {
  widgets_create(labels: Widgets_JSON, extra: Widgets_JSON): Widgets_Widget
    @connect(source: "w", http: { POST: "/widgets", body: """
      tags: $args.labels
      extra: $args.extra
      """ }, selection: "id")
  widgets_replace(input: Widgets_JSON!): Widgets_Widget
    @connect(source: "w", http: { PUT: "/widgets", body: "$args.input" }, selection: "id")
  widgets_update(input: Widgets_WidgetInput): Widgets_Widget
    @connect(source: "w", http: { PATCH: "/widgets", body: "$args.input" }, selection: "id")
  widgets_orphan(blob: Widgets_JSON): Widgets_Widget
}
"#;

    /// The inventory and selection behind [`REQUEST_SDL`]: `tags` is a typed
    /// list, `extra` a free-form object, the PUT body and `meta` typed
    /// objects, `page-size` an integer query parameter, and the list
    /// response a typed array.
    fn request_workspace(dir: &Path) {
        workspace(dir);
        write(dir, "widgets.graphql", REQUEST_SDL);
        write(
            dir,
            ".factory/inventory.json",
            r##"{
  "operations": [
    {"key": "get:/widgets", "parameters": [{"name": "page-size", "in": "query", "type": "integer", "shape_ref": null}],
     "request_body": null, "response": {"shape_ref": "#/shapes/WidgetList"}},
    {"key": "post:/widgets", "parameters": [],
     "request_body": {"shape_ref": "#/shapes/CreateWidget"}, "response": {"shape_ref": "#/shapes/Widget"}},
    {"key": "put:/widgets", "parameters": [],
     "request_body": {"shape_ref": "#/shapes/ReplaceWidget"}, "response": {"shape_ref": "#/shapes/Widget"}},
    {"key": "patch:/widgets", "parameters": [],
     "request_body": {"shape_ref": "#/shapes/UpdateWidget"}, "response": {"shape_ref": "#/shapes/Widget"}}
  ],
  "shapes": {
    "Widget": {"type": "object", "properties": {"id": {"type": "string"}}},
    "WidgetList": {"type": "array", "items": {"$ref": "#/shapes/Widget"}},
    "CreateWidget": {"type": "object", "properties": {
      "tags": {"type": "array", "items": {"type": "string"}},
      "extra": {"type": "object"}}},
    "ReplaceWidget": {"type": "object", "properties": {"name": {"type": "string"}}},
    "UpdateWidget": {"type": "object", "properties": {
      "name": {"type": "string"},
      "meta": {"type": "object", "properties": {"color": {"type": "string"}}}}}
  }
}"##,
        );
        write(
            dir,
            ".factory/selection.yaml",
            "contract_version: 1\noperations:\n  \"get:/widgets\":\n    include: true\n    graphql: { root: query, name: list }\n  \"post:/widgets\":\n    include: true\n    graphql: { root: mutation, name: create }\n  \"put:/widgets\":\n    include: true\n    graphql: { root: mutation, name: replace }\n  \"patch:/widgets\":\n    include: true\n    graphql: { root: mutation, name: update }\n",
        );
    }

    /// A body entry forwards a JSON argument into a
    /// request property the inventory types, so the argument is recoverable.
    /// Before Phase 9d no argument was a row at all.
    #[test]
    fn a_json_argument_a_body_entry_sends_into_a_typed_property_is_recoverable() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let report = build(tmp.path()).unwrap();
        let r = row(&report, "Mutation.widgets_create(labels:)");
        assert_eq!(r.position, Position::Argument);
        assert_eq!(r.status, Status::Recoverable);
    }

    /// The body entry's wire key, not the argument's name, finds the
    /// property: `extra` is free-form, so it needs a reason, and the
    /// free-form-object reason holds there.
    #[test]
    fn a_json_argument_into_a_free_form_property_is_accounted_by_its_reason() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let key = "Mutation.widgets_create(extra:)";
        assert_eq!(
            row(&build(tmp.path()).unwrap(), key).status,
            Status::Unaccounted
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Mutation", "widgets_create(extra:)", "free-form-object")],
        );
        assert_eq!(
            row(&build(tmp.path()).unwrap(), key).status,
            Status::Accounted
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Mutation", "widgets_create(extra:)", "recursive")],
        );
        assert_eq!(row(&build(tmp.path()).unwrap(), key).status, Status::Stale);
    }

    #[test]
    fn a_json_argument_forwarded_as_the_whole_body_is_read_against_the_body() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Mutation.widgets_replace(input:)").status,
            Status::Recoverable
        );
    }

    /// A query parameter matches without case or separators: `page-size` is
    /// the argument `pageSize`.
    #[test]
    fn a_json_argument_matching_a_typed_parameter_is_recoverable() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let report = build(tmp.path()).unwrap();
        assert_eq!(
            row(&report, "Query.widgets_list(pageSize:)").status,
            Status::Recoverable
        );
    }

    /// An input type is reached through the argument that sends it, and its
    /// JSON field is read against the request body's property.
    #[test]
    fn an_input_field_is_read_against_the_request_shape_its_type_is_sent_as() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let report = build(tmp.path()).unwrap();
        let r = row(&report, "Widgets_WidgetInput.meta");
        assert_eq!(r.position, Position::Input);
        assert_eq!(r.status, Status::Recoverable);
    }

    /// A root field's own JSON return is read against its operation's
    /// response. Before Phase 9d a root field's type was never located, so
    /// it could only ever be unaccounted or unresolved.
    #[test]
    fn a_root_field_returning_json_is_read_against_its_response() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let report = build(tmp.path()).unwrap();
        let r = row(&report, "Query.widgets_list");
        assert_eq!(r.position, Position::Output);
        assert_eq!(r.status, Status::Recoverable);
    }

    /// A root field the selection ties to no operation has nowhere to look:
    /// a recorded reason is unresolved, never read against "no shape".
    #[test]
    fn an_argument_of_an_unselected_root_field_is_unresolved() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let key = "Mutation.widgets_orphan(blob:)";
        assert_eq!(
            row(&build(tmp.path()).unwrap(), key).status,
            Status::Unaccounted
        );
        decisions_with_json_reasons(
            tmp.path(),
            "D-0001",
            false,
            &[("Mutation", "widgets_orphan(blob:)", "vendor-undocumented")],
        );
        assert_eq!(
            row(&build(tmp.path()).unwrap(), key).status,
            Status::Unresolved
        );
    }

    /// Arguments and input fields join the denominator, so the ratio still
    /// adds up: 6 object fields, 6 arguments and 2 input fields, 7 of them
    /// JSON.
    #[test]
    fn the_ratio_counts_arguments_and_input_fields() {
        let tmp = tempfile::tempdir().unwrap();
        request_workspace(tmp.path());
        let report = build(tmp.path()).unwrap();
        assert_eq!(report.total_fields, 14);
        assert_eq!(report.rows.len(), 7);
    }
}

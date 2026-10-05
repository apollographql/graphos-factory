//! scaffold — one case, one stub and one unit entry per selected operation,
//! built from the schema and the inventory shapes and marked for audit.
//!
//!   scaffold [workspace] [--op KEY]… [--force] [--dry-run] [--json]
//!
//! The skill's `gen-tests`: for every selected operation that has no
//! `tests/cases/<case>.graphql` yet (or the operations named with `--op`),
//! it writes
//!   tests/cases/<case>.graphql            the root field called with every
//!                                          argument (lists with two elements),
//!                                          selecting everything the connector maps
//!   tests/fixtures/mappings/<case>.json   the stub: method, path, credential
//!                                          header, an exact matcher per argument
//!                                          the connector puts in the query
//!                                          (hasExactly for lists), an exact body
//!                                          for a write whose body mapping is
//!                                          flat, and a response body built from
//!                                          the inventory shape
//!   tests/<dir>.connector.yaml            one entry: scalar $args, the fixture
//!                                          body as apiResponseBody, the request
//!                                          shape, and the mapped response when the
//!                                          selection is plain (no `->` methods)
//!
//! For a keyed type whose schema has a type-level `$batch` connector
//! (`--op batch:<Shape>`, ADR 0071) it writes `<shape>_batch`: a case
//! through a Query field that returns the entity's references, that field's
//! stub (references carrying the key alone, the connector's query keys
//! `absent`), and `<shape>_batch_lookup.json`, owned by the case through
//! `metadata."x-cases"`, demanding exactly the deduplicated key list and
//! marked `metadata."x-required"`, so e2e fails the case unless the router
//! calls it. The root field is one whose own connector selects the key
//! alone under the reference; with none, the case is skipped with the
//! reason. No unit entry: only the router shows that the keys are collected
//! and sent once.
//!
//! Everything is a starting point, not evidence: values are placeholders
//! chosen to be valid (`<name>-1`, 1, true, an enum value, a date), and each
//! file says so — `# scaffold:` in the document, `metadata.x-scaffold` on the
//! stub, a comment on the entry. The author audits, then runs
//! `e2e.sh --generate` for the snapshots.
//!
//! What the scaffold cannot build honestly it leaves out and says so in a
//! note, rather than asserting a guess: a unit body for a mapping that is
//! not flat, a `connectorResponse` the selection reaches but the shape does
//! not, a unit entry for an operation with no documented response body.
//!
//! Existing cases and stubs are never touched without `--op KEY --force`,
//! which also removes the stale scaffold entry from the unit suite. A suite
//! the scaffold did not write is parsed before it is appended to, and the
//! result is parsed again before it is written.
//!
//! Exit codes: 0 · 1 usage, unreadable workspace or an `--op` that matches
//! nothing · 2 nothing to do · 3 at least one selected operation rendered a
//! document that does not validate against the target schema (named, never
//! written — a generator defect, not a normal skip), or an error case whose
//! stub would match exactly what an existing stub matches.
//!
//! `--op KEY --status CODE|all-missing` writes error cases instead (ADR
//! 0077): per status a case whose first line is `# expect-upstream-status:
//! CODE`, and a stub answering CODE with the documented error body, if any.
//! No unit entry: rover cannot set a response status.

use crate::args::{Args, Flags};
use crate::json::{get, get_arr, get_obj, get_str, obj, truthy, Object};
use crate::lint::{
    body_mapping, connector_block, credential_header_values, root_field_args, root_field_text,
    wiring, Arg, BodyMapping,
};
use crate::reconcile::{parse_selection, Node, Spread};
use crate::sdl_index::{base_type, SdlIndex};
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::OnceLock;

/// The usage text: `scaffold --help` prints it on stdout (ADR 0086).
pub const USAGE: &str = "usage: scaffold [workspace] [--op KEY]… [--force] [--dry-run] [--json] [--status CODE|all-missing]";

fn usage(msg: &str) -> i32 {
    eprintln!("scaffold: {}", msg);
    eprintln!("{}", USAGE);
    1
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}
static HTTP_PATH_RE: OnceLock<Regex> = OnceLock::new();
static SELECTION_BLOCK_RE: OnceLock<Regex> = OnceLock::new();
static SELECTION_LINE_RE: OnceLock<Regex> = OnceLock::new();
static RETURN_TYPE_RE: OnceLock<Regex> = OnceLock::new();
static QUERY_BLOCK_RE: OnceLock<Regex> = OnceLock::new();
static QUERY_LINE_RE: OnceLock<Regex> = OnceLock::new();
static AUTH_EXPR_RE: OnceLock<Regex> = OnceLock::new();
static PATH_ARG_RE: OnceLock<Regex> = OnceLock::new();

/// The comment the scaffold puts above every unit entry it writes; `--force`
/// finds and replaces the entry by it.
const ENTRY_MARKER: &str = "# scaffold: generated by graphos-factory-core scaffold";

/// Whether `line` (trimmed) opens a scaffold-written entry: the marker under
/// any binary name, so an entry written before the binary was renamed is
/// still the scaffold's to replace.
fn is_entry_marker(line: &str) -> bool {
    line.strip_prefix("# scaffold: generated by ")
        .and_then(|rest| rest.split_once(' '))
        .is_some_and(|(name, rest)| !name.is_empty() && rest.starts_with("scaffold"))
}

/// A placeholder argument value that renders three ways: as a GraphQL
/// literal, as the string the router puts on the wire, and as JSON.
#[derive(Clone, Debug)]
enum Sample {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Enum(String),
    List(Vec<Sample>),
    /// An input object: its fields, by GraphQL name, in declaration order.
    Object(Vec<(String, Sample)>),
}

impl Sample {
    fn graphql(&self) -> String {
        match self {
            Sample::Str(s) => serde_json::to_string(s).unwrap_or_default(),
            Sample::Int(i) => i.to_string(),
            Sample::Float(f) => format!("{}", f),
            Sample::Bool(b) => b.to_string(),
            Sample::Enum(e) => e.clone(),
            Sample::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Sample::graphql)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Sample::Object(fields) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v.graphql()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    fn wire(&self) -> String {
        match self {
            Sample::Str(s) | Sample::Enum(s) => s.clone(),
            Sample::Int(i) => i.to_string(),
            Sample::Float(f) => format!("{}", f),
            Sample::Bool(b) => b.to_string(),
            Sample::List(items) => items.iter().map(Sample::wire).collect::<Vec<_>>().join(","),
            Sample::Object(_) => crate::json::compact(&self.json()),
        }
    }
    fn json(&self) -> Value {
        match self {
            Sample::Str(s) | Sample::Enum(s) => Value::from(s.as_str()),
            Sample::Int(i) => Value::from(*i),
            Sample::Float(f) => Value::from(*f),
            Sample::Bool(b) => Value::from(*b),
            Sample::List(items) => Value::Array(items.iter().map(Sample::json).collect()),
            Sample::Object(fields) => {
                let mut o = obj();
                for (k, v) in fields {
                    o.insert(k.clone(), v.json());
                }
                Value::Object(o)
            }
        }
    }
    /// An input object, or a list of them: what `rover connector test`
    /// cannot pass as `$args`.
    fn has_object(&self) -> bool {
        match self {
            Sample::Object(_) => true,
            Sample::List(items) => items.iter().any(Sample::has_object),
            _ => false,
        }
    }
    fn is_list(&self) -> bool {
        matches!(self, Sample::List(_))
    }
    fn is_stringy(&self) -> bool {
        matches!(self, Sample::Str(_) | Sample::Enum(_))
    }
}

/// What the spec says about the wire slot an argument feeds: its `type`
/// and `format`, so an `ID` that lands in an integer field is sampled as a
/// number and a `String` with format date-time as a date. A path slot gets
/// no format: its sample must be punctuation-free (ADR 0014).
#[derive(Clone, Debug, Default)]
struct Hint {
    type_: Option<String>,
    format: Option<String>,
    /// The wire slot's first `enum` value, when it declares a closed set a
    /// GraphQL enum could not carry (SCIM URNs, `amazon-bedrock`): a String
    /// placeholder takes it, so the request the case sends is one the spec
    /// accepts.
    enum_first: Option<String>,
}

fn string_sample(name: &str, i: usize, hint: &Hint) -> String {
    match hint.format.as_deref() {
        Some("date-time") => format!("2026-01-0{}T00:00:00Z", (i % 9).max(1)),
        Some("date") => format!("2026-01-0{}", (i % 9).max(1)),
        Some("email") => format!("{}-{}@example.com", name, i),
        Some("uri") | Some("url") => format!("https://example.com/{}/{}", name, i),
        Some("uuid") => format!("00000000-0000-4000-8000-{:012}", i),
        _ => format!("{}-{}", name, i),
    }
}

/// What an input-object argument and a custom scalar are sampled from: the
/// parsed schema (field order, which fields are required) and the inventory
/// shapes (the wire type a custom scalar or a nested field feeds).
struct Inputs<'a> {
    schema: &'a apollo_compiler::Schema,
    shapes: &'a Object,
}

/// `start_time` / `start-time` → `startTime`: how a generator names the
/// GraphQL field for a wire key, so a nested field finds its wire property.
pub(crate) fn camel_of(wire: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for (i, c) in wire.chars().enumerate() {
        if c == '_' || c == '-' || c == '.' || c == ' ' {
            upper = i > 0;
            continue;
        }
        if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else if out.is_empty() {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The wire property a nested GraphQL field `gql` feeds: the property of that
/// name, or the one a generator would have named `gql`.
fn prop_for<'a>(shape: Option<&'a Value>, shapes: &'a Object, gql: &str) -> Option<&'a Value> {
    let s = deref(shape?, shapes)?;
    let s = if get_str(s, "type") == Some("array") {
        deref(get(s, "items")?, shapes)?
    } else {
        s
    };
    let props = get_obj(s, "properties")?;
    props.get(gql).or_else(|| {
        props
            .iter()
            .find(|(k, _)| camel_of(k) == gql)
            .map(|(_, v)| v)
    })
}

/// The type and format of a wire slot's leaf (an array's items).
fn hint_of(shape: Option<&Value>, shapes: &Object) -> Hint {
    let s = match shape.and_then(|s| deref(s, shapes)) {
        Some(s) => s,
        None => return Hint::default(),
    };
    let leaf = if get_str(s, "type") == Some("array") {
        get(s, "items").and_then(|i| deref(i, shapes)).unwrap_or(s)
    } else {
        s
    };
    Hint {
        type_: get_str(leaf, "type").map(str::to_string),
        format: get_str(leaf, "format").map(str::to_string),
        enum_first: enum_first(leaf),
    }
}

fn enum_first(leaf: &Value) -> Option<String> {
    get_arr(leaf, "enum")
        .and_then(|v| v.first())
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A placeholder for one declared argument, valid for its GraphQL type and,
/// where the spec says more, for the wire slot it feeds. `index` is the
/// argument's 0-based position; samples read `<name>-1`, `<name>-2`, ….
/// An input object gets every required field and, when `with_optional`, the
/// first optional field it can sample; a custom scalar gets a value of the
/// JSON type its wire slot declares. None only for a type that cannot be
/// sampled at all (an unknown name, a required field on a cycle).
#[allow(clippy::too_many_arguments)]
fn sample_for(
    arg: &Arg,
    schema: &SdlIndex,
    index: usize,
    hint: &Hint,
    inputs: &Inputs,
    shape: Option<&Value>,
    with_optional: bool,
) -> Option<Sample> {
    let mut stack = Vec::new();
    sample_typed(
        &arg.type_,
        &arg.name,
        schema,
        index,
        hint,
        inputs,
        shape,
        with_optional,
        &mut stack,
    )
}

#[allow(clippy::too_many_arguments)]
fn sample_typed(
    ty: &str,
    name: &str,
    schema: &SdlIndex,
    index: usize,
    hint: &Hint,
    inputs: &Inputs,
    shape: Option<&Value>,
    with_optional: bool,
    stack: &mut Vec<String>,
) -> Option<Sample> {
    if ty.trim_end_matches('!').starts_with('[') {
        let a = sample_one(
            ty,
            name,
            schema,
            index,
            hint,
            inputs,
            shape,
            with_optional,
            stack,
        )?;
        let b = sample_one(
            ty,
            name,
            schema,
            index + 1,
            hint,
            inputs,
            shape,
            with_optional,
            stack,
        )?;
        Some(Sample::List(vec![a, b]))
    } else {
        sample_one(
            ty,
            name,
            schema,
            index,
            hint,
            inputs,
            shape,
            with_optional,
            stack,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn sample_one(
    ty: &str,
    name: &str,
    schema: &SdlIndex,
    i: usize,
    hint: &Hint,
    inputs: &Inputs,
    shape: Option<&Value>,
    with_optional: bool,
    stack: &mut Vec<String>,
) -> Option<Sample> {
    use apollo_compiler::schema::ExtendedType;
    let base = base_type(ty);
    let n = i + 1;
    Some(match base.as_str() {
        "Int" => Sample::Int(n as i64),
        "Float" => Sample::Float(n as f64 + 0.5),
        "Boolean" => Sample::Bool(true),
        // An `ID` accepts an integer literal, so on an integer slot it is
        // one. A `String` does not (GraphQL rejects `1` for it, and the whole
        // document with it): on a numeric or boolean slot, the non-identifying
        // int64 of lessons.md, it is that value as a string.
        "ID" if hint.type_.as_deref() == Some("integer") => Sample::Int(n as i64),
        "ID" | "String" => match hint.type_.as_deref() {
            Some("integer") => Sample::Str(n.to_string()),
            Some("number") => Sample::Str(format!("{}.5", n)),
            Some("boolean") => Sample::Str("true".to_string()),
            _ => match &hint.enum_first {
                Some(v) => Sample::Str(v.clone()),
                None => Sample::Str(string_sample(name, n, hint)),
            },
        },
        other => {
            if let Some(values) = schema.enum_values(other) {
                return Some(Sample::Enum(values[i % values.len()].clone()));
            }
            match inputs.schema.types.get(other) {
                Some(ExtendedType::InputObject(def)) => {
                    // A cycle, or deeper than any selection: stop here.
                    if stack.iter().any(|s| s == other) || stack.len() >= 6 {
                        return None;
                    }
                    stack.push(other.to_string());
                    let mut fields = Vec::new();
                    let mut optional_taken = false;
                    for (j, (fname, fdef)) in def.fields.iter().enumerate() {
                        let required = fdef.ty.is_non_null() && fdef.default_value.is_none();
                        if !required && (!with_optional || optional_taken) {
                            continue;
                        }
                        let fshape = prop_for(shape, inputs.shapes, fname.as_str());
                        let fhint = hint_of(fshape, inputs.shapes);
                        match sample_typed(
                            &fdef.ty.to_string(),
                            fname.as_str(),
                            schema,
                            j,
                            &fhint,
                            inputs,
                            fshape,
                            with_optional,
                            stack,
                        ) {
                            Some(s) => {
                                if !required {
                                    optional_taken = true;
                                }
                                fields.push((fname.to_string(), s));
                            }
                            None if required => {
                                stack.pop();
                                return None;
                            }
                            None => {}
                        }
                    }
                    stack.pop();
                    Sample::Object(fields)
                }
                // A custom scalar is sent as the JSON value it wraps: the
                // wire slot's declared type says which.
                Some(ExtendedType::Scalar(_)) => match hint.type_.as_deref() {
                    Some("integer") => Sample::Int(n as i64),
                    Some("number") => Sample::Float(n as f64 + 0.5),
                    Some("boolean") => Sample::Bool(true),
                    Some("string") => Sample::Str(string_sample(name, n, hint)),
                    _ => {
                        let map = shape
                            .and_then(|s| deref(s, inputs.shapes))
                            .map(|s| get(s, "additionalProperties").is_some())
                            .unwrap_or(false);
                        if map {
                            Sample::Object(vec![(
                                "key".to_string(),
                                Sample::Str(format!("{}-{}", name, n)),
                            )])
                        } else {
                            Sample::Object(Vec::new())
                        }
                    }
                },
                _ => return None,
            }
        }
    })
}

/// Follow `$ref` chains. None when a reference names a shape the inventory
/// does not have.
fn deref<'a>(shape: &'a Value, shapes: &'a Object) -> Option<&'a Value> {
    let mut cur = shape;
    let mut hops = 0;
    while let Some(r) = get_str(cur, "$ref") {
        cur = shapes.get(r.trim_start_matches("#/shapes/"))?;
        hops += 1;
        if hops > 32 {
            return None;
        }
    }
    Some(cur)
}

/// The example builder's state: the shapes, how deep the selection reaches
/// (a self-referential shape is expanded that far and no further), and the
/// references it could not resolve.
struct Examples<'a> {
    shapes: &'a Object,
    depth_limit: usize,
    unresolved: Vec<String>,
    /// Property names from the body's root to the value being built.
    path: Vec<String>,
    /// Where the entity sits in the body: the list root (`data` for a paged
    /// edge, `crate::sparse::list_root`), or the root for a node.
    entity_prefix: Option<String>,
    /// The fields expression's expansion groups, by entity-relative path.
    expanded: std::collections::BTreeMap<String, Vec<String>>,
    notes: Vec<String>,
}

impl<'a> Examples<'a> {
    fn new(shapes: &'a Object, selection_depth: usize) -> Examples<'a> {
        Examples {
            shapes,
            // Deep enough for the selection, with one level of slack for an
            // envelope; never shallower than a plain shape needs.
            depth_limit: selection_depth.max(3) + 2,
            unresolved: Vec::new(),
            path: Vec::new(),
            entity_prefix: None,
            expanded: Default::default(),
            notes: Vec::new(),
        }
    }

    /// Build expansion boundaries (ADR 0045) as the source answers them: an
    /// expanded one with the children the fields expression requests, any
    /// other with its verified default projection.
    fn projected(mut self, entity_prefix: Option<&str>, fields: Option<&str>) -> Self {
        self.entity_prefix = entity_prefix.map(str::to_string);
        self.expanded = fields
            .map(crate::sparse::expansion_groups)
            .unwrap_or_default();
        self
    }

    /// The entity-relative dotted path of property `k` under the current path.
    fn entity_path(&self, k: &str) -> Option<String> {
        let mut segs: Vec<&str> = self.path.iter().map(String::as_str).collect();
        segs.push(k);
        if let Some(p) = &self.entity_prefix {
            if segs.first() != Some(&p.as_str()) {
                return None;
            }
            segs.remove(0);
        }
        Some(segs.join("."))
    }

    /// A boundary property's value, or `None` when its default projection is
    /// unverified and the stub cannot honestly carry it.
    fn boundary(
        &mut self,
        k: &str,
        v: &Value,
        x: &Value,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Option<Value> {
        let rel = self.entity_path(k)?;
        let keep: Vec<String> = match self.expanded.get(&rel) {
            Some(children) => children.clone(),
            None => match crate::sparse::verified_default(x) {
                Some(leaves) => leaves,
                None => {
                    self.notes.push(format!(
                        "`{}` is an expansion boundary whose default projection is unverified; the stub leaves it out — verify the default, then scaffold again",
                        rel
                    ));
                    return None;
                }
            },
        };
        self.path.push(k.to_string());
        let built = self.example_in(v, k, depth + 1, stack);
        self.path.pop();
        let only = |v: &Value| match v {
            Value::Object(o) => Value::Object(
                o.iter()
                    .filter(|(name, _)| keep.contains(name))
                    .map(|(name, v)| (name.clone(), v.clone()))
                    .collect(),
            ),
            other => other.clone(),
        };
        Some(match built {
            Value::Array(items) => Value::Array(items.iter().map(only).collect()),
            other => only(&other),
        })
    }

    /// An example body for an inventory shape: every property present,
    /// values valid for their type and format, arrays with one element.
    fn example(&mut self, shape: &Value, name: &str) -> Value {
        self.example_in(shape, name, 0, &mut Vec::new())
    }

    fn example_in(
        &mut self,
        shape: &Value,
        name: &str,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Value {
        let ref_name =
            get_str(shape, "$ref").map(|r| r.trim_start_matches("#/shapes/").to_string());
        let s = match deref(shape, self.shapes) {
            Some(s) => s,
            None => {
                if let Some(r) = &ref_name {
                    if !self.unresolved.contains(r) {
                        self.unresolved.push(r.clone());
                    }
                }
                return Value::Object(obj());
            }
        };
        // A shape already on the stack is a cycle: expand it while the
        // selection may still reach into it, then stop with its leaf.
        let repeated = ref_name
            .as_ref()
            .map(|r| stack.contains(r))
            .unwrap_or(false);
        if depth >= self.depth_limit || (repeated && depth >= self.depth_limit.saturating_sub(2)) {
            return self.leaf_keeping_required(s, name, &mut stack.clone());
        }
        if let Some(r) = &ref_name {
            stack.push(r.clone());
        }
        let out = self.example_body(s, name, depth, stack);
        if ref_name.is_some() {
            stack.pop();
        }
        out
    }

    fn example_body(
        &mut self,
        s: &Value,
        name: &str,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Value {
        if let Some(variants) = get_arr(s, "oneOf").or_else(|| get_arr(s, "anyOf")) {
            if let Some(first) = variants.first() {
                return self.example_in(first, name, depth + 1, stack);
            }
        }
        if let Some(all) = get_arr(s, "allOf") {
            let mut merged = obj();
            for part in all {
                if let Value::Object(o) = self.example_in(part, name, depth + 1, stack) {
                    merged.extend(o);
                }
            }
            return Value::Object(merged);
        }
        if let Some(values) = get_arr(s, "enum") {
            if let Some(first) = values.first() {
                return first.clone();
            }
        }
        let ty = match get(s, "type") {
            Some(Value::String(t)) => t.clone(),
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .find(|t| *t != "null")
                .unwrap_or("string")
                .to_string(),
            _ => {
                if get(s, "properties").is_some() {
                    "object".to_string()
                } else if get(s, "items").is_some() {
                    "array".to_string()
                } else {
                    "string".to_string()
                }
            }
        };
        match ty.as_str() {
            "object" => {
                let mut out = obj();
                for (k, v) in get_obj(s, "properties").into_iter().flatten() {
                    if let Some(x) = crate::sparse::expansion(v, self.shapes) {
                        if self.entity_path(k).is_some() {
                            if let Some(value) = self.boundary(k, v, x, depth, stack) {
                                out.insert(k.clone(), value);
                            }
                            continue;
                        }
                    }
                    self.path.push(k.clone());
                    let value = self.example_in(v, k, depth + 1, stack);
                    self.path.pop();
                    out.insert(k.clone(), value);
                }
                Value::Object(out)
            }
            "array" => match get(s, "items") {
                Some(items) => Value::Array(vec![self.example_in(items, name, depth + 1, stack)]),
                None => Value::Array(vec![]),
            },
            "integer" => Value::from(1),
            "number" => Value::from(1.5),
            "boolean" => Value::from(true),
            _ => Value::from(match get_str(s, "format") {
                Some("date-time") => "2026-01-01T00:00:00Z".to_string(),
                Some("date") => "2026-01-01".to_string(),
                Some("email") => "user@example.com".to_string(),
                Some("uri") | Some("url") => format!("https://example.com/{}", name),
                Some("uuid") => "00000000-0000-4000-8000-000000000001".to_string(),
                _ => format!("{}-1", name),
            }),
        }
    }
}

impl<'a> Examples<'a> {
    /// The smallest value of a cut shape that still conforms: an object keeps
    /// every property its shape requires, each as its own smallest value, so
    /// a branch the depth cap or a cycle cut short is not a body the spec
    /// rejects (`missing required property`). Only required properties are
    /// followed, so a shape on a cycle (Databricks `Task.for_each_task.task`)
    /// is revisited safely; past 16 levels (a cycle every step of which is
    /// required) the bare leaf ends it.
    fn leaf_keeping_required(&mut self, s: &Value, name: &str, stack: &mut Vec<String>) -> Value {
        let mut out = leaf(s, name);
        let required: Vec<String> = get_arr(s, "required")
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        if let (Value::Object(o), Some(props)) = (&mut out, get_obj(s, "properties")) {
            for r in &required {
                let prop = match props.get(r) {
                    Some(p) => p,
                    None => continue,
                };
                let ref_name =
                    get_str(prop, "$ref").map(|x| x.trim_start_matches("#/shapes/").to_string());
                let target = match deref(prop, self.shapes) {
                    Some(t) => t,
                    None => continue,
                };
                let value = match &ref_name {
                    Some(_) if stack.len() > 16 => leaf(target, r),
                    Some(rn) => {
                        stack.push(rn.clone());
                        let v = self.leaf_keeping_required(target, r, stack);
                        stack.pop();
                        v
                    }
                    None => self.leaf_keeping_required(target, r, stack),
                };
                o.insert(r.clone(), value);
            }
        }
        out
    }
}

/// The smallest value of a shape's type: what a self-referential shape
/// (`Repository.parent`) or the depth cap gets instead of recursing.
fn leaf(s: &Value, name: &str) -> Value {
    if let Some(first) = get_arr(s, "enum").and_then(|v| v.first()) {
        return first.clone();
    }
    match get(s, "type").and_then(Value::as_str) {
        Some("object") => Value::Object(obj()),
        Some("array") => Value::Array(vec![]),
        Some("integer") => Value::from(1),
        Some("number") => Value::from(1.5),
        Some("boolean") => Value::from(true),
        Some("string") => Value::from(format!("{}-1", name)),
        _ => {
            if get(s, "properties").is_some() {
                Value::Object(obj())
            } else if get(s, "items").is_some() {
                Value::Array(vec![])
            } else {
                Value::from(format!("{}-1", name))
            }
        }
    }
}

fn exposed_name(node: &Node) -> Option<String> {
    node.alias
        .clone()
        .or_else(|| node.key.as_ref().and_then(|k| k.last().cloned()))
}

/// How many object levels the selection descends (rooted nodes hoist).
fn selection_depth(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .map(|n| {
            let below = n.children.as_deref().map(selection_depth).unwrap_or(0);
            if n.rooted && n.alias.is_none() {
                below + n.key.as_ref().map(|k| k.len()).unwrap_or(0)
            } else {
                below + n.key.as_ref().map(|k| k.len()).unwrap_or(1)
            }
        })
        .max()
        .unwrap_or(0)
}

/// The GraphQL selection set a document should ask for: everything the
/// connector maps, by its exposed name. Rooted nodes (`$.incident { … }`)
/// hoist their children.
fn render_selection(nodes: &[Node], indent: usize) -> String {
    let pad = " ".repeat(indent);
    // A `...` spread's arms are the members of a union or interface (ADR
    // 0058): the document asks for `__typename` and one fragment per member.
    // The fields beside the spread go into every fragment, because on a
    // union they are not fields of the abstract type itself.
    let mut members: Vec<(String, Vec<Node>)> = Vec::new();
    for n in nodes {
        if let Some(Spread::Match(arms)) = &n.spread {
            for arm in arms {
                let Some(t) = &arm.typename else { continue };
                match members.iter_mut().find(|(m, _)| m == t) {
                    Some((_, children)) => children.extend(arm.children.iter().cloned()),
                    None => members.push((t.clone(), arm.children.clone())),
                }
            }
        }
    }
    let plain: Vec<Node> = nodes
        .iter()
        .filter(|n| n.spread.is_none())
        .cloned()
        .collect();
    if !members.is_empty() {
        let mut out = format!("{}__typename\n", pad);
        for (member, children) in members {
            let mut inside = plain.clone();
            inside.extend(children);
            out.push_str(&format!(
                "{}... on {} {{\n{}{}}}\n",
                pad,
                member,
                render_selection(&inside, indent + 2),
                pad
            ));
        }
        return out;
    }
    let mut out = String::new();
    for n in &plain {
        if n.rooted && n.alias.is_none() {
            // `$.version` alone: the field is a scalar, there is nothing to select.
            if let Some(children) = &n.children {
                out.push_str(&render_selection(children, indent));
            }
            continue;
        }
        let name = match exposed_name(n) {
            Some(x) => x,
            None => continue,
        };
        match &n.children {
            // An `alias: { … }` group (the D-0031/R3 entity-reference-stub
            // idiom) keeps its compact minimal selection from its own keys.
            Some(_) if !n.literal_keys.is_empty() => {
                out.push_str(&format!(
                    "{}{} {{ {} }}\n",
                    pad,
                    name,
                    n.literal_keys.join(" ")
                ));
            }
            Some(children) => {
                let inner = render_selection(children, indent + 2);
                if inner.trim().is_empty() {
                    out.push_str(&format!("{}{}\n", pad, name));
                } else {
                    out.push_str(&format!("{}{} {{\n{}{}}}\n", pad, name, inner, pad));
                }
            }
            // A literal-object stub (the D-0031/R3 entity-reference-stub
            // idiom, `alias: { id: x }`) has no real sub-selection to
            // recurse into, but its own captured top-level keys (almost
            // always just `id`) are themselves valid field names on
            // whatever entity it references -- render them as a minimal
            // selection instead of a bare field name, which is invalid
            // GraphQL whenever this field's own type is an object or
            // interface.
            None if !n.literal_keys.is_empty() => {
                out.push_str(&format!(
                    "{}{} {{ {} }}\n",
                    pad,
                    name,
                    n.literal_keys.join(" ")
                ));
            }
            None => out.push_str(&format!("{}{}\n", pad, name)),
        }
    }
    out
}

/// The shape one wire key leads to (through an array's items, and `$ref`s).
fn prop_shape(shape: &Value, seg: &str, shapes: &Object) -> Option<Value> {
    let s = deref(shape, shapes)?;
    let s = if get_str(s, "type") == Some("array") {
        deref(get(s, "items")?, shapes)?
    } else {
        s
    };
    get_obj(s, "properties")?.get(seg).cloned()
}

/// An array shape's item shape; any other shape itself.
fn items_of(shape: &Value, shapes: &Object) -> Value {
    match deref(shape, shapes) {
        Some(s) if get_str(s, "type") == Some("array") => get(s, "items")
            .and_then(|i| deref(i, shapes))
            .cloned()
            .unwrap_or(Value::Null),
        Some(s) => s.clone(),
        None => Value::Null,
    }
}

fn is_empty_container(v: &Value) -> bool {
    match v {
        Value::Object(o) => o.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Put every path the selection names into the example body. The body is
/// built to a depth cap that counts array hops and the selection's depth
/// does not, so a selected branch that crosses several arrays can be cut to
/// `{}` before its leaf; each such branch is rebuilt from the shape along
/// that path. A body that already carries the selection is left untouched.
fn deepen(nodes: &[Node], body: &mut Value, shape: &Value, shapes: &Object) {
    if let Value::Array(items) = body {
        let item_shape = items_of(shape, shapes);
        for item in items {
            deepen(nodes, item, &item_shape, shapes);
        }
        return;
    }
    for n in nodes {
        let key = match &n.key {
            Some(k) if !n.opaque && n.methods.is_empty() && !k.is_empty() => k,
            _ => continue,
        };
        // The shapes along the key path, or nothing to do.
        let mut along: Vec<Value> = Vec::new();
        let mut sh = shape.clone();
        for seg in key {
            match prop_shape(&sh, seg, shapes) {
                Some(next) => {
                    along.push(next.clone());
                    sh = next;
                }
                None => break,
            }
        }
        if along.len() != key.len() {
            continue;
        }
        // Insert whatever the path lacks, one level at a time.
        let mut reached = true;
        for (i, seg) in key.iter().enumerate() {
            let parent = match walk_mut(body, &key[..i]).and_then(Value::as_object_mut) {
                Some(o) => o,
                None => {
                    reached = false;
                    break;
                }
            };
            if !parent.contains_key(seg) {
                let depth = n.children.as_deref().map(selection_depth).unwrap_or(0);
                let fresh = Examples::new(shapes, depth).example(&along[i], seg);
                parent.insert(seg.clone(), fresh);
            }
        }
        if !reached {
            continue;
        }
        let cur_shape = along.last().cloned().unwrap_or(Value::Null);
        let cur = match walk_mut(body, key) {
            Some(c) => c,
            None => continue,
        };
        if let Some(children) = &n.children {
            if is_empty_container(cur) {
                let depth = selection_depth(children);
                let name = key.last().map(String::as_str).unwrap_or("value");
                let fresh = Examples::new(shapes, depth).example(&cur_shape, name);
                if !is_empty_container(&fresh) {
                    *cur = fresh;
                }
            }
            deepen(children, cur, &cur_shape, shapes);
        }
    }
}

fn walk_mut<'a>(v: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut cur = v;
    for seg in path {
        cur = cur.as_object_mut()?.get_mut(seg)?;
    }
    Some(cur)
}

/// Walk the example body with the selection and the schema types together:
/// a string leaf whose GraphQL field is an enum takes the enum's first value,
/// so the router does not reject the placeholder.
fn align_enums(nodes: &[Node], body: &mut Value, schema: &mut SdlIndex, type_name: Option<&str>) {
    for n in nodes {
        if n.rooted && n.alias.is_none() {
            if let (Some(children), Some(key)) = (&n.children, &n.key) {
                if let Some(inner) = dig_mut(body, key) {
                    align_enums(children, inner, schema, type_name);
                }
            }
            continue;
        }
        let (name, key) = match (exposed_name(n), &n.key) {
            (Some(a), Some(k)) if !n.opaque => (a, k),
            _ => continue,
        };
        let ftype = type_name.and_then(|t| schema.field_type(t, &name));
        let base = ftype.as_deref().map(base_type);
        let first_enum = base
            .as_deref()
            .and_then(|b| schema.enum_values(b))
            .map(|v| Value::from(v[0].as_str()));
        let target = match dig_mut(body, key) {
            Some(t) => t,
            None => continue,
        };
        match (&n.children, base.as_deref()) {
            (Some(children), Some(b)) => match target {
                Value::Array(items) => {
                    for item in items {
                        align_enums(children, item, schema, Some(b));
                    }
                }
                other => align_enums(children, other, schema, Some(b)),
            },
            (None, Some(_)) => {
                if let Some(first) = first_enum {
                    match target {
                        Value::Array(items) => {
                            for item in items {
                                *item = first.clone();
                            }
                        }
                        other => *other = first,
                    }
                }
            }
            _ => {}
        }
    }
}

fn dig_mut<'a>(body: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    let mut cur = body;
    for seg in path {
        cur = match cur {
            Value::Object(o) => o.get_mut(seg)?,
            Value::Array(a) => a.first_mut()?.as_object_mut()?.get_mut(seg)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn dig<'a>(body: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut cur = body;
    for seg in path {
        cur = match cur {
            Value::Object(o) => o.get(seg)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Why `project` gave up: the selection needs the mapping language, or it
/// names a key the payload does not carry.
#[derive(Debug, PartialEq)]
enum Unprojectable {
    MappingLanguage,
    MissingKey(String),
}

/// The connector's response for `body`: the selection applied node by node.
/// Err as soon as a node needs the mapping language (`->` methods, `$(…)`),
/// because the scaffold does not evaluate it, or names a key the payload
/// lacks — asserting the partial object would claim the field is absent.
fn project(nodes: &[Node], body: &Value) -> Result<Value, Unprojectable> {
    // A list root: the selection applies to every element.
    if let Value::Array(items) = body {
        return Ok(Value::Array(
            items
                .iter()
                .map(|i| project(nodes, i))
                .collect::<Result<Vec<_>, _>>()?,
        ));
    }
    let key_of = |n: &Node| -> Result<Vec<String>, Unprojectable> {
        if n.opaque || !n.methods.is_empty() {
            return Err(Unprojectable::MappingLanguage);
        }
        n.key.clone().ok_or(Unprojectable::MappingLanguage)
    };
    // `$.version` alone: the connector returns that value itself.
    if let [n] = nodes {
        if n.rooted && n.alias.is_none() && n.children.is_none() {
            let key = key_of(n)?;
            return dig(body, &key)
                .cloned()
                .ok_or_else(|| Unprojectable::MissingKey(key.join(".")));
        }
    }
    let mut out = obj();
    for n in nodes {
        let key = key_of(n)?;
        if n.rooted && n.alias.is_none() {
            let inner = dig(body, &key).ok_or_else(|| Unprojectable::MissingKey(key.join(".")))?;
            let children = n
                .children
                .as_deref()
                .ok_or(Unprojectable::MappingLanguage)?;
            match project(children, inner)? {
                Value::Object(o) => out.extend(o),
                // `$.items { … }` alone: the connector returns the list itself.
                list @ Value::Array(_) if nodes.len() == 1 => return Ok(list),
                _ => return Err(Unprojectable::MappingLanguage),
            }
            continue;
        }
        let name = exposed_name(n).ok_or(Unprojectable::MappingLanguage)?;
        let value = dig(body, &key).ok_or_else(|| Unprojectable::MissingKey(key.join(".")))?;
        let mapped = match &n.children {
            Some(children) => match value {
                Value::Array(items) => Value::Array(
                    items
                        .iter()
                        .map(|i| project(children, i))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                Value::Object(_) => project(children, value)?,
                Value::Null => Value::Null,
                _ => return Err(Unprojectable::MappingLanguage),
            },
            None => value.clone(),
        };
        out.insert(name, mapped);
    }
    Ok(Value::Object(out))
}

/// The selection with only `wire` kept — at the top for a node, among the
/// items under `list` for a list (the wrapper stays whole).
fn narrow_nodes(nodes: &[Node], wire: &str, list: Option<&str>) -> Vec<Node> {
    nodes
        .iter()
        .filter_map(|n| {
            let key = n
                .key
                .as_ref()
                .filter(|k| k.len() == 1)
                .map(|k| k[0].as_str());
            match list {
                Some(l) => {
                    let mut kept = n.clone();
                    if key == Some(l) {
                        kept.children = n.children.as_ref().map(|c| narrow_nodes(c, wire, None));
                    }
                    Some(kept)
                }
                None => (key == Some(wire)).then(|| n.clone()),
            }
        })
        .collect()
}

/// The stub body with only the `wire` keys kept, matching `narrow_nodes` —
/// at the top for a node, in each item under `list` for a list, whose
/// wrapper stays whole.
fn narrow_body(body: &Value, wire: &[String], list: Option<&str>) -> Value {
    let keep = |v: &Value| match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .filter(|(k, _)| wire.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => other.clone(),
    };
    let Some(l) = list else {
        return keep(body);
    };
    let mut out = body.clone();
    if let Some(items) = out.get_mut(l).and_then(Value::as_array_mut) {
        for item in items.iter_mut() {
            *item = keep(item);
        }
    }
    out
}

/// The response key a top-level part of a fields expression comes back
/// under: `campaigns.limit(10)` → `campaigns`, `name.as(label)` → `label`.
fn returned_key(part: &str) -> String {
    match part.find(".as(") {
        Some(i) => part[i + 4..].trim_end_matches(')').trim().to_string(),
        None => part
            .split(['.', '('])
            .next()
            .unwrap_or(part)
            .trim()
            .to_string(),
    }
}

pub(crate) fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The credential header the schema sends: (lower-cased name, value with
/// `{{AUTH_EXPR}}` rendered as `test-token`, what e2e.sh exports). The same
/// scan lint's `unit-no-credential` uses.
fn credential(sdl: &str) -> Option<(String, String)> {
    credential_header_values(sdl)
        .into_iter()
        .next()
        .map(|(name, value)| {
            (
                name,
                re(&AUTH_EXPR_RE, r"\{\{\s*AUTH_EXPR\s*\}\}")
                    .replace_all(&value, "test-token")
                    .to_string(),
            )
        })
}

/// The `queryParams` block, when it uses the mapping language on some line.
fn query_params_use_mapping_language(field_text: &str) -> bool {
    let block = re(&QUERY_BLOCK_RE, r#"(?s)\bqueryParams\s*:\s*"""(.*?)""""#)
        .captures(field_text)
        .map(|m| m[1].to_string())
        .or_else(|| {
            re(&QUERY_LINE_RE, r#"\bqueryParams\s*:\s*"([^"]*)""#)
                .captures(field_text)
                .map(|m| m[1].to_string())
        });
    block
        .map(|q| q.contains("->") || q.contains("$("))
        .unwrap_or(false)
}

/// The property `key` of a wire shape (of its items, for an array).
fn wire_prop(shape: &Value, key: &str, shapes: &Object) -> Option<Value> {
    let s = deref(shape, shapes)?;
    let s = if get_str(s, "type") == Some("array") {
        deref(get(s, "items")?, shapes)?
    } else {
        s
    };
    get_obj(s, "properties")?.get(key).cloned()
}

/// The GraphQL side of a body mapping, as a shape: every entry walked from
/// the wire shape along its key, and put at the path it reads. At the top
/// level the paths start with the argument, so the result's property `x` is
/// what argument `x` feeds: the whole property for `key: $args.x`, an object
/// built field by field for `a: $args.x.a  b: $args.x.b { … }`. A
/// sub-selection is walked the same way, so a renamed field finds its wire
/// property; a list stays a list of what its elements feed.
fn mapped_shape(entries: &[BodyEntry], wire: &Value, shapes: &Object) -> Value {
    let list = deref(wire, shapes)
        .map(|s| get_str(s, "type") == Some("array"))
        .unwrap_or(false);
    let mut root = crate::json::object(vec![
        ("type", Value::from("object")),
        ("properties", Value::Object(obj())),
    ]);
    for e in entries {
        let prop = match wire_prop(wire, &e.key, shapes) {
            Some(p) => p,
            None => continue,
        };
        let shape = match &e.children {
            Some(c) => mapped_shape(c, &prop, shapes),
            None => prop,
        };
        // Down the path, through objects this walk built; the first entry to
        // claim a path keeps it.
        let mut cur = &mut root;
        for (i, seg) in e.path.iter().enumerate() {
            let props = match cur.get_mut("properties").and_then(Value::as_object_mut) {
                Some(p) => p,
                None => break,
            };
            if i + 1 == e.path.len() {
                props.entry(seg.clone()).or_insert(shape);
                break;
            }
            cur = props.entry(seg.clone()).or_insert_with(|| {
                crate::json::object(vec![
                    ("type", Value::from("object")),
                    ("properties", Value::Object(obj())),
                ])
            });
        }
    }
    if list {
        crate::json::object(vec![("type", Value::from("array")), ("items", root)])
    } else {
        root
    }
}

/// The sample at `path` inside an argument's sample (`filterBy.policyId`).
fn sample_at<'a>(sample: &'a Sample, path: &[String]) -> Option<&'a Sample> {
    let mut cur = sample;
    for seg in path {
        cur = match cur {
            Sample::Object(fields) => fields.iter().find(|(k, _)| k == seg).map(|(_, v)| v)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// One entry of a nested `body` mapping: the wire key, the path it reads
/// (from `$args` at the top level, from the current value inside a
/// sub-selection) and its own sub-selection, if any.
#[derive(Debug, Clone)]
struct BodyEntry {
    key: String,
    path: Vec<String>,
    children: Option<Vec<BodyEntry>>,
}

/// A `body` mapping in the grammar a generator writes for input objects:
/// top-level `key: $args.a.b`, optionally followed by a sub-selection
/// `{ wire: gql  same  other: x.y { … } }` that maps each field back to its
/// wire name (and each element, for a list). None for anything else — a
/// method, `$(…)`, a literal, `$this` — which stays "not flat". The block is
/// read as lint reads it: triple-quoted, or the one-line `"…"` form (ADR
/// 0042).
fn nested_body(field_text: &str) -> Option<Vec<BodyEntry>> {
    let block = connector_block(field_text, "body")?;
    let src: Vec<char> = block.chars().collect();
    let mut i = 0;
    let entries = parse_body_entries(&src, &mut i, true)?;
    skip_body_ws(&src, &mut i);
    if i < src.len() || entries.is_empty() {
        return None;
    }
    Some(entries)
}

fn skip_body_ws(src: &[char], i: &mut usize) {
    while *i < src.len() {
        if src[*i].is_whitespace() || src[*i] == ',' {
            *i += 1;
        } else if src[*i] == '#' {
            while *i < src.len() && src[*i] != '\n' {
                *i += 1;
            }
        } else {
            break;
        }
    }
}

fn body_ident(src: &[char], i: &mut usize) -> Option<String> {
    let start = *i;
    while *i < src.len() && (src[*i].is_ascii_alphanumeric() || src[*i] == '_') {
        *i += 1;
    }
    if *i == start || src[start].is_ascii_digit() {
        *i = start;
        return None;
    }
    Some(src[start..*i].iter().collect())
}

fn body_path(src: &[char], i: &mut usize) -> Option<Vec<String>> {
    let mut segs = vec![body_ident(src, i)?];
    while *i < src.len() && src[*i] == '.' {
        *i += 1;
        segs.push(body_ident(src, i)?);
    }
    Some(segs)
}

fn parse_body_entries(src: &[char], i: &mut usize, top: bool) -> Option<Vec<BodyEntry>> {
    let mut out = Vec::new();
    loop {
        skip_body_ws(src, i);
        if *i >= src.len() || src[*i] == '}' {
            return Some(out);
        }
        // `$args.x` alone at the top: the whole body is that argument.
        if top && out.is_empty() && src[*i] == '$' {
            let prefix: String = src[*i..(*i + 6).min(src.len())].iter().collect();
            if prefix != "$args." {
                return None;
            }
            *i += 6;
            let path = body_path(src, i)?;
            skip_body_ws(src, i);
            if *i < src.len() {
                return None; // anything after the whole-body argument is not this grammar
            }
            out.push(BodyEntry {
                key: String::new(),
                path,
                children: None,
            });
            return Some(out);
        }
        let key = if src[*i] == '"' {
            *i += 1;
            let start = *i;
            while *i < src.len() && src[*i] != '"' {
                *i += 1;
            }
            let k: String = src[start..(*i).min(src.len())].iter().collect();
            *i += 1;
            k
        } else {
            body_ident(src, i)?
        };
        skip_body_ws(src, i);
        let path = if *i < src.len() && src[*i] == ':' {
            *i += 1;
            skip_body_ws(src, i);
            if top {
                // `$args.` then a path: the only value a top-level key takes.
                let prefix: String = src[*i..(*i + 6).min(src.len())].iter().collect();
                if prefix != "$args." {
                    return None;
                }
                *i += 6;
            }
            body_path(src, i)?
        } else if top {
            return None; // a bare key at the top level reads nothing
        } else {
            vec![key.clone()]
        };
        skip_body_ws(src, i);
        let children = if *i < src.len() && src[*i] == '{' {
            *i += 1;
            let c = parse_body_entries(src, i, false)?;
            if *i >= src.len() || src[*i] != '}' {
                return None;
            }
            *i += 1;
            Some(c)
        } else {
            None
        };
        // Anything else right here (`->`, `(`, a literal) is not this grammar.
        skip_body_ws(src, i);
        if *i < src.len() && matches!(src[*i], '-' | '(' | '$' | '@' | '[') {
            return None;
        }
        out.push(BodyEntry {
            key,
            path,
            children,
        });
    }
}

/// What the router sends for a nested `body` mapping, given the arguments'
/// JSON values: an absent argument or field drops its key, a sub-selection
/// maps each field to its wire name, and each element of a list. A
/// whole-body `$args.x` is that argument's value itself.
fn eval_body(entries: &[BodyEntry], args: &Object) -> Value {
    if let [e] = entries {
        if e.key.is_empty() {
            return args
                .get(&e.path[0])
                .and_then(|v| walk_path(v, &e.path[1..]))
                .unwrap_or(Value::Null);
        }
    }
    let mut out = obj();
    for e in entries {
        let cur = match args
            .get(&e.path[0])
            .and_then(|v| walk_path(v, &e.path[1..]))
        {
            Some(v) if !v.is_null() => v,
            _ => continue,
        };
        let v = match &e.children {
            Some(c) => apply_body_selection(c, &cur),
            None => cur,
        };
        out.insert(e.key.clone(), v);
    }
    Value::Object(out)
}

fn apply_body_selection(entries: &[BodyEntry], v: &Value) -> Value {
    match v {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|i| apply_body_selection(entries, i))
                .collect(),
        ),
        Value::Object(o) => {
            let mut out = obj();
            for e in entries {
                let cur = match o.get(&e.path[0]).and_then(|x| walk_path(x, &e.path[1..])) {
                    Some(x) if !x.is_null() => x,
                    _ => continue,
                };
                let mapped = match &e.children {
                    Some(c) => apply_body_selection(c, &cur),
                    None => cur,
                };
                out.insert(e.key.clone(), mapped);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The value at `path` inside `v`, read as the mapping language reads a
/// path: a list is mapped over, each element walked the rest of the way,
/// with `null` for an element that lacks the field (Router 2.17 sends
/// `$args.order.lines.quantity` as `[1, null]` when the second line has
/// none). Outside a list, a missing field is none.
fn walk_path(v: &Value, path: &[String]) -> Option<Value> {
    let (seg, rest) = match path.split_first() {
        Some(p) => p,
        None => return Some(v.clone()),
    };
    match v {
        Value::Array(items) => Some(Value::Array(
            items
                .iter()
                .map(|i| walk_path(i, path).unwrap_or(Value::Null))
                .collect(),
        )),
        Value::Object(o) => walk_path(o.get(seg)?, rest),
        _ => None,
    }
}

/// Percent-encode one path segment or query value (RFC 3986 unreserved set).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn yaml_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => {
            if s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                && !s.is_empty()
                && !s
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
                && !matches!(
                    s.to_ascii_lowercase().as_str(),
                    "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n" | "~"
                )
            {
                s.clone()
            } else {
                serde_json::to_string(s).unwrap_or_default()
            }
        }
        other => crate::json::compact(other),
    }
}

/// Re-indent a unit entry written at two spaces to the suite's item indent.
fn reindent(entry: &str, item_indent: usize) -> String {
    let mut out = String::new();
    for line in entry.lines() {
        if line.trim().is_empty() {
            out.push('\n');
            continue;
        }
        if item_indent >= 2 {
            out.push_str(&" ".repeat(item_indent - 2));
            out.push_str(line);
        } else {
            let strip = (2 - item_indent).min(line.len() - line.trim_start().len());
            out.push_str(&line[strip..]);
        }
        out.push('\n');
    }
    out
}

/// The unit suite as text, opened for appending: parsed first, `tests:`
/// present, and the indentation of its items known.
struct Suite {
    text: String,
    item_indent: usize,
}

fn open_suite(
    path: &Path,
    directory: &str,
    auth_var: &Option<String>,
    today: &str,
) -> Result<Suite, String> {
    if !path.exists() {
        return Ok(Suite {
            text: format!(
                "# rover connector test suite — layer 2 of the validation stack.\n# Started by graphos-factory-core scaffold on {}; every entry it wrote says so.\n# The framework injects $config but not $env; scripts/unit.sh rewrites the\n# schema's {{$env.…}} to {{$config.…}} in a temporary copy, backed by the value below.\nconfig:\n  schema: {}.graphql\n{}\ntests:\n",
                today,
                directory,
                match auth_var {
                    Some(v) => format!("  common:\n    variables:\n      $config:\n        {}: test-token", v),
                    None => String::new(),
                }
            ),
            item_indent: 2,
        });
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let parsed = crate::yaml::parse(&text).map_err(|e| {
        format!(
            "{}: not valid YAML ({}) — fix it before scaffolding into it",
            path.display(),
            e
        )
    })?;
    let mut text = text;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let tests = get(&parsed, "tests");
    match tests {
        None => {
            text.push_str("\ntests:\n");
            return Ok(Suite {
                text,
                item_indent: 2,
            });
        }
        Some(Value::Array(a)) if a.is_empty() => {
            // `tests: []` — open the sequence.
            let mut replaced = false;
            let mut lines: Vec<String> = Vec::new();
            for line in text.lines() {
                if !replaced && line.trim_start() == "tests: []" && !line.starts_with(' ') {
                    lines.push("tests:".to_string());
                    replaced = true;
                } else {
                    lines.push(line.to_string());
                }
            }
            if !replaced {
                return Err(format!(
                    "{}: `tests` is an empty list written in a form the scaffold does not rewrite — replace it with `tests:` and retry",
                    path.display()
                ));
            }
            let mut text = lines.join("\n");
            text.push('\n');
            return Ok(Suite {
                text,
                item_indent: 2,
            });
        }
        Some(Value::Array(_)) => {}
        Some(Value::Null) => {
            return Ok(Suite {
                text,
                item_indent: 2,
            })
        }
        Some(_) => {
            return Err(format!(
                "{}: `tests` is not a list — the scaffold cannot append to it",
                path.display()
            ))
        }
    }
    // The indent of the first item under the top-level `tests:` — which may
    // be column 0, the idiomatic YAML form.
    let mut in_tests = false;
    let mut seen_tests = false;
    let mut item_indent = 2;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if in_tests && (trimmed.starts_with("- ") || trimmed.trim_end() == "-") {
            if item_indent == 2 && !seen_tests {
                item_indent = indent;
            }
            seen_tests = true;
            continue;
        }
        if indent == 0 {
            if seen_tests && trimmed.trim_end() != "tests:" {
                // Entries are appended at the end of the file; a key after
                // `tests:` would swallow them.
                return Err(format!(
                    "{}: `{}` comes after `tests:` — the scaffold appends entries at the end of the file; move `tests:` to the end, or add the entries by hand",
                    path.display(),
                    trimmed.trim_end()
                ));
            }
            in_tests = trimmed.trim_end() == "tests:";
        }
    }
    Ok(Suite { text, item_indent })
}

/// Remove the scaffold-written entry for `target` from the suite text, if
/// there is one; report whether a hand-written entry for it remains.
fn remove_scaffold_entry(text: &str, target: &str) -> (String, bool, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let is_target_line = |l: &str| -> bool {
        l.trim()
            .strip_prefix("target:")
            .map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'') == target)
            .unwrap_or(false)
    };
    let mut out: Vec<&str> = Vec::new();
    let mut removed = false;
    let mut hand_written = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if is_entry_marker(trimmed) {
            let indent = line.len() - trimmed.len();
            // The block: this comment and everything more indented than it,
            // through the item that follows, until the next line at the
            // item's indent or shallower.
            let mut j = i + 1;
            let mut seen_item = false;
            while j < lines.len() {
                let l = lines[j];
                let t = l.trim_start();
                if t.is_empty() {
                    j += 1;
                    continue;
                }
                let ind = l.len() - t.len();
                if ind < indent {
                    break;
                }
                if ind == indent {
                    if t.starts_with("- ") && !seen_item {
                        seen_item = true;
                        j += 1;
                        continue;
                    }
                    if t.starts_with('#') && !seen_item {
                        j += 1;
                        continue;
                    }
                    break;
                }
                j += 1;
            }
            let block = &lines[i..j];
            if block.iter().any(|l| is_target_line(l)) {
                removed = true;
                i = j;
                continue;
            }
            out.extend_from_slice(block);
            i = j;
            continue;
        }
        if is_target_line(line) {
            hand_written = true;
        }
        out.push(line);
        i += 1;
    }
    let mut text = out.join("\n");
    text.push('\n');
    // Collapse runs of blank lines the removal left behind.
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    (text, removed, hand_written)
}

struct Plan {
    key: String,
    case: String,
    document: String,
    mapping: Value,
    /// The unit entry (two-space item indent), or None when the scaffold
    /// cannot write an honest one.
    unit_entry: Option<String>,
    /// `Root.field`, for `--force` to replace the stale entry; empty for a
    /// plan that never has a unit entry (a `$batch` case).
    target: String,
    notes: Vec<String>,
    /// Further stubs the case owns, `(file stem, mapping)`, each tied to the
    /// case by `metadata."x-cases"`: a `$batch` case's key-list lookup.
    extra: Vec<(String, Value)>,
}

fn stub_response(status: &str, body: Option<&Value>, notes: &mut Vec<String>) -> Object {
    let mut resp = obj();
    let code = match status.parse::<u64>() {
        Ok(c) => c,
        Err(_) => {
            notes.push(format!(
                "the spec's response status is `{}`, not a number; the stub answers 200 — set the real status by hand",
                status
            ));
            200
        }
    };
    resp.insert("status".into(), Value::from(code));
    if let Some(b) = body {
        let mut rh = obj();
        rh.insert("Content-Type".into(), Value::from("application/json"));
        resp.insert("headers".into(), Value::Object(rh));
        resp.insert("jsonBody".into(), b.clone());
    }
    resp
}

fn header_matcher(cred: &Option<(String, String)>) -> Option<Value> {
    cred.as_ref().map(|(h, v)| {
        let mut headers = obj();
        let mut m = obj();
        m.insert("equalTo".into(), Value::from(v.as_str()));
        headers.insert(h.clone(), Value::Object(m));
        Value::Object(headers)
    })
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["force", "dry-run", "json"],
    valued: &["op", "status"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    let only: Vec<String> = args.all("op");
    let force = args.has("force");
    let dry_run = args.has("dry-run");
    let json_out = args.has("json");
    // `--status CODE|all-missing` (ADR 0077): error cases for one operation.
    let status_arg: Option<String> = args.get("status").map(str::to_string);
    if args.has("status") && status_arg.is_none() {
        return usage("--status needs a status code (404) or all-missing");
    }
    if status_arg.is_some() && only.len() != 1 {
        return usage(
            "--status writes error cases for one operation: name it with exactly one --op KEY",
        );
    }
    let error_mode = status_arg.is_some();
    if force && only.is_empty() {
        return usage("--force overwrites a case, its stub and its unit entry: name the operation(s) with --op KEY");
    }

    let loaded = match crate::cmd::lock::load(&dir) {
        Ok(l) => l,
        Err(e) => return usage(&e),
    };
    let crate::cmd::lock::Loaded { sdl, inventory, .. } = loaded;
    let inventory = match inventory {
        Some(i) => i,
        None => return usage("no .factory/inventory.json; run inventory build first"),
    };
    let factory_yaml = |rel: &str| -> Result<Value, String> {
        crate::yaml::parse(&crate::factory_io::read_to_string(&dir, rel)?)
            .map_err(|e| format!("{}: {}", rel, e))
    };
    let workspace = match factory_yaml(".factory/workspace.yaml") {
        Ok(w) => w,
        Err(e) => return usage(&e),
    };
    let selection = match factory_yaml(".factory/selection.yaml") {
        Ok(s) => s,
        Err(e) => return usage(&e),
    };
    let prefix = get_str(&workspace, "field_prefix")
        .unwrap_or("")
        .to_string();
    let directory = get_str(&workspace, "directory")
        .unwrap_or("schema")
        .to_string();
    let shapes = get(&inventory, "shapes")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let template = crate::yaml::parse_file(&dir.join("template.yaml")).ok();
    let template_var = |name: &str| -> Option<String> {
        template.as_ref().and_then(|t| {
            get_arr(t, "variables")
                .into_iter()
                .flatten()
                .find(|v| get_str(v, "name") == Some(name))
                .and_then(|v| get_str(v, "test_default").map(str::to_string))
        })
    };
    let base_url =
        template_var("BASE_URL").unwrap_or_else(|| "https://api.example.com".to_string());
    let auth_var = crate::render::env_var_from_auth_expr(template_var("AUTH_EXPR").as_deref());
    let cred = credential(&sdl);
    let mut schema = SdlIndex::new(&sdl);
    let query_args = root_field_args(&sdl, "Query");
    let mutation_args = root_field_args(&sdl, "Mutation");
    let today = crate::today();

    let case_dir = dir.join("tests").join("cases");
    let mapping_dir = dir.join("tests").join("fixtures").join("mappings");
    let suite_path = dir
        .join("tests")
        .join(format!("{}.connector.yaml", directory));

    // Scaffold renders `render_selection`'s tree back into GraphQL text; an
    // opaque, childless node (the D-0031/R3 entity-reference-stub idiom,
    // `alias: { id: x }`) is not a valid *selection* on a field whose own
    // type is an object or interface — GraphQL requires a sub-selection
    // there. Rather than special-case every renderer for every node shape
    // that might resolve to a composite type, validate the fully-rendered
    // document against the real target schema and refuse the one operation
    // it's wrong for: parse and refuse before trusting generated text.
    // `Schema::parse` (not `parse_and_validate`): this codebase's schemas
    // declare @connect/@source/@key/@tag etc. only via `@link`'s own
    // import mechanism (Federation's convention, not core GraphQL), which
    // apollo_compiler's schema validator does not resolve on its own --
    // validating the *schema* itself would refuse every real connector
    // schema here as "directive not defined". `assume_valid` skips that
    // (this repo's own schemas are the trusted input; nothing here
    // fabricates one), and is sound for this call's actual purpose: type
    // lookups to validate the *generated operation document*'s shape,
    // which uses no directives of its own at all.
    let target_schema = match apollo_compiler::Schema::parse(&sdl, "schema.graphql") {
        Ok(s) => apollo_compiler::validation::Valid::assume_valid(s),
        Err(e) => {
            eprintln!("scaffold: the target schema does not parse: {}", e);
            return 1;
        }
    };
    let mut plans: Vec<Plan> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut invalid: Vec<(String, String)> = Vec::new();
    let mut matched_only: Vec<String> = Vec::new();
    // The statuses an error-case run writes, one pass each so every error
    // case gets its own argument values (ADR 0077); one pass otherwise.
    let passes: Vec<Option<String>> = match &status_arg {
        None => vec![None],
        Some(st) => {
            let key = &only[0];
            let op = get_arr(&inventory, "operations")
                .into_iter()
                .flatten()
                .find(|o| get_str(o, "key") == Some(key.as_str()));
            let name = get_obj(&selection, "operations")
                .and_then(|o| o.get(key.as_str()))
                .and_then(|e| get(e, "graphql"))
                .and_then(|g| get_str(g, "name"));
            match (op, name) {
                (Some(op), Some(name)) if st == "all-missing" => {
                    let field = format!("{}_{}", prefix, name);
                    let answered = answered_statuses(&case_dir, &mapping_dir, &field, key);
                    let documented = documented_error_statuses(op);
                    let missing: Vec<Option<String>> = documented
                        .iter()
                        .cloned()
                        .filter(|(d, _)| !answered.iter().any(|a| status_covers(d, a)))
                        .map(|(d, _)| Some(d))
                        .collect();
                    if missing.is_empty() {
                        matched_only.push(key.clone());
                        skipped.push((
                            key.clone(),
                            if documented.is_empty() {
                                "the inventory documents no non-2xx status for it".to_string()
                            } else {
                                "every documented non-2xx status already has an e2e case"
                                    .to_string()
                            },
                        ));
                    }
                    missing
                }
                // Not selected: the loop matches nothing and says so.
                _ if st == "all-missing" => vec![],
                _ => vec![Some(st.clone())],
            }
        }
    };
    let operations: Vec<(&String, &Value)> = get_obj(&selection, "operations")
        .into_iter()
        .flatten()
        .collect();
    for (pass, (key, entry)) in passes
        .iter()
        .flat_map(|p| operations.iter().map(move |o| (p, *o)))
    {
        // Distinct values per status: `name-404` for the 404 case.
        let sample_offset = pass
            .as_deref()
            .and_then(representative_status)
            .map(|c| c as usize - 1)
            .unwrap_or(0);
        if !truthy(get(entry, "include")) {
            continue;
        }
        if !only.is_empty() {
            if !only.iter().any(|o| o == key) {
                continue;
            }
            matched_only.push(key.clone());
        }
        let name = match get(entry, "graphql").and_then(|g| get_str(g, "name")) {
            Some(n) => n,
            None => continue,
        };
        let root = get(entry, "graphql")
            .and_then(|g| get_str(g, "root"))
            .unwrap_or("query");
        let root_type = if root == "query" { "Query" } else { "Mutation" };
        let field = format!("{}_{}", prefix, name);
        let target = format!("{}.{}", root_type, field);
        let case = snake(name);
        let case_exists = case_dir.join(format!("{}.graphql", case)).exists();
        let mapping_exists = mapping_dir.join(format!("{}.json", case)).exists();
        if !error_mode && !force && (case_exists || mapping_exists) {
            let why = if case_exists {
                format!("tests/cases/{}.graphql exists", case)
            } else {
                format!("tests/fixtures/mappings/{}.json exists", case)
            };
            skipped.push((key.clone(), why));
            continue;
        }
        let op = match get_arr(&inventory, "operations")
            .into_iter()
            .flatten()
            .find(|o| get_str(o, "key") == Some(key))
        {
            Some(o) => o,
            None => {
                skipped.push((key.clone(), "not in inventory.json".to_string()));
                continue;
            }
        };
        let declared = if root == "query" {
            &query_args
        } else {
            &mutation_args
        };
        let args_decl: Vec<Arg> = declared
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, a)| a.clone())
            .unwrap_or_default();
        let field_text = match root_field_text(&sdl, root_type, &field) {
            Some(t) => t,
            None => {
                skipped.push((
                    key.clone(),
                    format!("{}.{} is not in the schema", root_type, field),
                ));
                continue;
            }
        };
        let wire = wiring(&field_text);
        let http_path = re(
            &HTTP_PATH_RE,
            r#"\b(?:GET|POST|PUT|PATCH|DELETE|HEAD)\s*:\s*"([^"]+)""#,
        )
        .captures(&field_text)
        .map(|m| m[1].to_string())
        .unwrap_or_else(|| get_str(op, "path").unwrap_or("/").to_string());
        let method = wire
            .verb
            .clone()
            .or_else(|| get_str(op, "method").map(str::to_uppercase))
            .unwrap_or_else(|| "GET".to_string());
        let selection_text = re(&SELECTION_BLOCK_RE, r#"(?s)\bselection\s*:\s*"""(.*?)""""#)
            .captures(&field_text)
            .map(|m| m[1].to_string())
            .or_else(|| {
                re(&SELECTION_LINE_RE, r#"\bselection\s*:\s*"([^"]*)""#)
                    .captures(&field_text)
                    .map(|m| m[1].to_string())
            })
            .unwrap_or_default();
        let nodes = parse_selection(&selection_text);
        let blanked_field = crate::graphql::blank(&field_text);
        let return_type = re(&RETURN_TYPE_RE, r"\)\s*:\s*([\[\]!A-Za-z0-9_]+)")
            .captures(&blanked_field)
            .map(|m| base_type(&m[1]))
            .or_else(|| {
                Regex::new(&format!(
                    r"\b{}\s*:\s*([\[\]!A-Za-z0-9_]+)",
                    regex::escape(&field)
                ))
                .unwrap()
                .captures(&blanked_field)
                .map(|m| base_type(&m[1]))
            });

        let mut notes: Vec<String> = Vec::new();
        let is_write = matches!(method.as_str(), "POST" | "PUT" | "PATCH");
        // The path's own query string (`GET: "/x?fixed=1"`): sent on every
        // request, so asserted on every request.
        let (path_template, literal_query) = match http_path.split_once('?') {
            Some((p, q)) => (
                p.to_string(),
                q.split('&')
                    .filter(|kv| !kv.is_empty())
                    .map(|kv| match kv.split_once('=') {
                        Some((k, v)) => (k.to_string(), urldecode(v)),
                        None => (kv.to_string(), String::new()),
                    })
                    .collect::<Vec<(String, String)>>(),
            ),
            None => (http_path.clone(), Vec::new()),
        };
        let path_args: Vec<String> = re(&PATH_ARG_RE, r"\{\$args\.([A-Za-z_][A-Za-z0-9_]*)\}")
            .captures_iter(&path_template)
            .map(|m| m[1].to_string())
            .collect();
        let is_path_arg = |name: &str| path_args.iter().any(|p| p == name);
        let is_query_key = |name: &str| wire.query_keys.iter().any(|(a, _)| a == name);

        // The body mapping and the request shape.
        let request_content_type = get(op, "request_body")
            .and_then(|r| get_str(r, "content_type"))
            .map(str::to_string);
        let json_request = request_content_type
            .as_deref()
            .map(|c| c.contains("json"))
            .unwrap_or(true);
        let mut nested: Option<Vec<BodyEntry>> = None;
        let body_map: Vec<(String, String)> = match body_mapping(&field_text) {
            BodyMapping::Flat(m) if json_request => m,
            BodyMapping::Flat(_) => {
                notes.push(format!(
                    "the request body is {}, not JSON; the stub asserts no body (the write_body_proof layer reports it as unproven) and the scaffold writes no unit entry (lint missing-unit) — write both by hand",
                    request_content_type.as_deref().unwrap_or("of an unknown content type")
                ));
                Vec::new()
            }
            BodyMapping::NotFlat => {
                // A nested mapping in the generators' grammar is evaluated
                // against the case's own values; anything else is left to hand.
                nested = nested_body(&field_text).filter(|_| json_request);
                if nested.is_none() {
                    notes.push("the body mapping is not a flat `key: $args.x` list; the stub asserts no body (the write_body_proof layer reports it as unproven) and the scaffold writes no unit entry (lint missing-unit) — write both by hand".to_string());
                }
                Vec::new()
            }
            BodyMapping::None => Vec::new(),
        };
        let flat_body = !body_map.is_empty();
        let asserted_body = flat_body || nested.is_some();
        let is_body_key = |name: &str| {
            body_map.iter().any(|(a, _)| a == name)
                || nested
                    .as_ref()
                    .map(|t| t.iter().any(|e| e.path[0] == name))
                    .unwrap_or(false)
        };
        let request_shape = get(op, "request_body")
            .and_then(|r| get_str(r, "shape_ref"))
            .and_then(|sr| {
                deref(
                    &crate::json::object(vec![("$ref", Value::from(sr))]),
                    &shapes,
                )
                .cloned()
            });
        // What the spec says about each argument's wire slot.
        let hint_for = |a: &Arg| -> Hint {
            let param_hint =
                wire.query_keys
                    .iter()
                    .find(|(arg, _)| arg == &a.name)
                    .and_then(|(_, k)| {
                        let base = k.trim_end_matches("[]");
                        get_arr(op, "parameters").into_iter().flatten().find(|p| {
                            get_str(p, "name") == Some(base) || get_str(p, "name") == Some(k)
                        })
                    })
                    .or_else(|| {
                        get_arr(op, "parameters").into_iter().flatten().find(|p| {
                            get_str(p, "in") == Some("path") && get_str(p, "name") == Some(&a.name)
                        })
                    })
                    .map(|p| {
                        let schema = get(p, "schema").unwrap_or(p);
                        let (ty, fmt) = if get_str(schema, "type") == Some("array") {
                            let items = get(schema, "items").unwrap_or(schema);
                            (get_str(items, "type"), get_str(items, "format"))
                        } else {
                            (get_str(schema, "type"), get_str(schema, "format"))
                        };
                        let leaf = if get_str(schema, "type") == Some("array") {
                            get(schema, "items").unwrap_or(schema)
                        } else {
                            schema
                        };
                        Hint {
                            type_: ty.map(str::to_string),
                            format: fmt.map(str::to_string),
                            enum_first: enum_first(leaf),
                        }
                    });
            let mut hint = param_hint
                .or_else(|| {
                    body_map
                        .iter()
                        .find(|(arg, _)| arg == &a.name)
                        .and_then(|(_, k)| {
                            request_shape
                                .as_ref()
                                .and_then(|s| get_obj(s, "properties"))
                                .and_then(|props| props.get(k))
                        })
                        .and_then(|prop| deref(prop, &shapes))
                        .map(|prop| {
                            let leaf = if get_str(prop, "type") == Some("array") {
                                get(prop, "items")
                                    .and_then(|i| deref(i, &shapes))
                                    .unwrap_or(prop)
                            } else {
                                prop
                            };
                            Hint {
                                type_: get_str(leaf, "type").map(str::to_string),
                                format: get_str(leaf, "format").map(str::to_string),
                                enum_first: enum_first(leaf),
                            }
                        })
                })
                .unwrap_or_default();
            // A path segment must stay punctuation-free: no `:` (rover's
            // encoding cannot be asserted) and no `/` (a `uri` sample).
            if is_path_arg(&a.name) {
                hint.format = None;
                hint.enum_first = None;
            }
            hint
        };
        // The wire slot an argument feeds, as a shape: its query parameter's
        // schema, or what the body mapping walks it to in the request shape
        // (every entry that reads it, a nested mapping included), so an
        // input object's fields and a custom scalar are sampled for what the
        // API expects.
        let body_shape: Option<Value> = request_shape.as_ref().map(|rs| {
            let entries: Vec<BodyEntry> = match &nested {
                Some(tree) => tree.clone(),
                None => body_map
                    .iter()
                    .map(|(arg, k)| BodyEntry {
                        key: k.clone(),
                        path: vec![arg.clone()],
                        children: None,
                    })
                    .collect(),
            };
            mapped_shape(&entries, rs, &shapes)
        });
        // A whole-body `$args.x`: its wire shape is the request body's own.
        let whole_arg: Option<String> = match nested.as_deref() {
            Some([e]) if e.key.is_empty() => Some(e.path[0].clone()),
            _ => None,
        };
        // The `queryParams` entries that read an input object's leaf
        // (`"filter_by.x": $args.filterBy.x`, the dotted keys it is sent as):
        // (argument path, wire key).
        let dotted: Vec<(Vec<String>, String)> = wire
            .plain_query_keys
            .iter()
            .filter(|(path, _)| path.len() > 1)
            .cloned()
            .collect();
        // A list on a key whose value is exactly `$args.<path>`, no method,
        // goes on the wire as the key repeated once per element (observed on
        // connect v0.3 with Apollo Router 2.17, and what the Databricks SDK
        // itself sends), whether or not the key ends in `[]`. Read by
        // `wiring()`, so a one-line block counts too (ADR 0042).
        let repeated =
            |k: &str| k.ends_with("[]") || wire.plain_query_keys.iter().any(|(_, p)| p == k);
        let arg_shape = |a: &Arg| -> Option<Value> {
            let query_key = wire
                .query_keys
                .iter()
                .find(|(arg, _)| arg == &a.name)
                .map(|(_, k)| k.clone())
                .or_else(|| {
                    // `"filter_by.x": $args.filterBy.x`: the parameter is the
                    // key's first dotted segment.
                    dotted
                        .iter()
                        .find(|(path, _)| path[0] == a.name)
                        .map(|(_, k)| k.split('.').next().unwrap_or(k).to_string())
                });
            if let Some(k) = query_key {
                let base = k.trim_end_matches("[]");
                // A dotted key (`filter_by.min_weight`) belongs to the
                // parameter named by its first segment.
                let first = base.split('.').next().unwrap_or(base);
                let p = get_arr(op, "parameters")
                    .into_iter()
                    .flatten()
                    .find(|p| get_str(p, "name") == Some(base))
                    .or_else(|| {
                        get_arr(op, "parameters")
                            .into_iter()
                            .flatten()
                            .find(|p| get_str(p, "name") == Some(first))
                    })?;
                return get_str(p, "shape_ref")
                    .map(|r| crate::json::object(vec![("$ref", Value::from(r))]))
                    .or_else(|| get(p, "schema").cloned());
            }
            if whole_arg.as_deref() == Some(a.name.as_str()) {
                return request_shape.clone();
            }
            body_shape
                .as_ref()
                .and_then(|s| get_obj(s, "properties"))
                .and_then(|props| props.get(&a.name))
                .cloned()
        };
        let inputs = Inputs {
            schema: &target_schema,
            shapes: &shapes,
        };
        let sample_args =
            |with_optional: bool, required_only: bool| -> Vec<(Arg, Option<Sample>)> {
                args_decl
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| !required_only || a.is_required())
                    .map(|(i, a)| {
                        let shape = arg_shape(a);
                        let mut hint = hint_for(a);
                        if hint.type_.is_none() {
                            hint = hint_of(shape.as_ref(), &shapes);
                            if is_path_arg(&a.name) {
                                hint.format = None;
                                hint.enum_first = None;
                            }
                        }
                        (
                            a.clone(),
                            sample_for(
                                a,
                                &schema,
                                i + sample_offset,
                                &hint,
                                &inputs,
                                shape.as_ref(),
                                with_optional,
                            ),
                        )
                    })
                    .collect()
            };
        // Required arguments only, and inside an input object its required
        // fields only: the minimal case's values.
        let minimal_samples: Vec<(Arg, Sample)> = sample_args(false, true)
            .into_iter()
            .filter_map(|(a, s)| s.map(|s| (a, s)))
            .collect();
        // Arguments and their samples.
        let mut samples: Vec<(Arg, Sample)> = Vec::new();
        for (a, s) in sample_args(true, false) {
            match s {
                Some(s) => samples.push((a, s)),
                None => notes.push(format!(
                    "argument {}: {} cannot be sampled (a type the schema does not define, or a required field on a cycle); no placeholder — pass it by hand",
                    a.name, a.type_
                )),
            }
        }
        // Sparse fieldsets (ADR 0045): the declared default is what the
        // router sends when a caller omits the argument, so the main case
        // omits it and its stub demands the default; a second case narrows
        // it to one field.
        let sparse_param = crate::sparse::param_name(&workspace);
        let sparse_default: Option<String> = crate::sparse::string_param(&workspace, op)
            .filter(|_| {
                wire.query_keys
                    .iter()
                    .any(|(a, k)| *a == sparse_param && *k == sparse_param)
            })
            .and_then(|_| crate::sparse::declared_arg(&field_text, &sparse_param))
            .and_then(|d| d.default);
        let sparse_arg: Option<Arg> = sparse_default
            .as_ref()
            .and_then(|_| args_decl.iter().find(|a| a.name == sparse_param).cloned());
        if sparse_arg.is_some() {
            samples.retain(|(a, _)| a.name != sparse_param);
        }
        let sample_of = |chosen: &[(Arg, Sample)], name: &str| -> Option<Sample> {
            chosen
                .iter()
                .find(|(a, _)| a.name == name)
                .map(|(_, s)| s.clone())
        };
        // Path: substitute `{$args.x}`, each value percent-encoded.
        let mut path_only = path_template.clone();
        for (a, s) in &samples {
            path_only = path_only.replace(&format!("{{$args.{}}}", a.name), &urlencode(&s.wire()));
        }
        if path_only.contains("{$") {
            notes.push(format!(
                "path {} still carries a placeholder the scaffold could not fill",
                path_only
            ));
        }
        if query_params_use_mapping_language(&field_text) {
            notes.push("a queryParams line uses the mapping language (`->` or `$(…)`); the stub and the unit URL assert the raw argument value for it — check the first e2e run".to_string());
        }
        for (path, k) in &dotted {
            let is_list = sample_of(&samples, &path[0])
                .and_then(|s| sample_at(&s, &path[1..]).map(Sample::is_list))
                .unwrap_or(false);
            if is_list && !repeated(k) {
                notes.push(format!(
                    "{} is a list on query key `{}` through the mapping language: the stub asserts a comma-joined value — check the first e2e run",
                    path.join("."),
                    k
                ));
            }
        }
        for (a, s) in &samples {
            if let Some((_, k)) = wire.query_keys.iter().find(|(arg, _)| arg == &a.name) {
                if s.is_list() && !repeated(k) {
                    notes.push(format!(
                        "{} is a list on query key `{}` through the mapping language: the stub asserts a comma-joined value — check the first e2e run",
                        a.name, k
                    ));
                }
            }
        }

        // Response body from the shape, with enum leaves aligned to the schema.
        let response = get(op, "response");
        let status = response
            .and_then(|r| get(r, "status"))
            .map(|s| match s {
                Value::String(x) => x.clone(),
                other => crate::json::compact(other),
            })
            .unwrap_or_else(|| "200".to_string());
        // Where a list response's items sit: a fields list and its
        // expansion groups are relative to an item (ADR 0045).
        let list_root = crate::sparse::list_root(op);
        // The level a sparse default's names live at: a list's items,
        // unless the default names the list root itself — then the
        // response is a node the envelope rule misread (`{customer,
        // line_items: [...]}` with `customer,line_items`), and its top level
        // is what the source trims (ADR 0045).
        let sparse_list: Option<String> = list_root.clone().filter(|l| {
            !sparse_default.as_deref().is_some_and(|d| {
                crate::sparse::top_level_names(d)
                    .iter()
                    .any(|p| returned_key(p) == *l)
            })
        });
        let mut examples = Examples::new(&shapes, selection_depth(&nodes)).projected(
            list_root.as_deref(),
            sparse_arg.as_ref().and(sparse_default.as_deref()),
        );
        let response_content_type = response
            .and_then(|r| get_str(r, "content_type"))
            .map(str::to_string);
        let json_response = response_content_type
            .as_deref()
            .map(|c| c.contains("json"))
            .unwrap_or(true);
        // ADR 0080: a two-branch success/error oneOf/anyOf response types
        // as the resolved success branch when either the source made it
        // unambiguous (inventory's own `response.referenced_shape` fact)
        // or a reviewed judgement picked one (the same key in this
        // operation's own selection `response` entry, never written back
        // to inventory.json). Absent both, an otherwise-ambiguous union is
        // never guessed at via the general oneOf fallback (variants.first,
        // used everywhere else in this file for ordinary polymorphism) --
        // it types as opaque JSON and the finding stays open.
        let referenced_shape = response
            .and_then(|r| get_str(r, "referenced_shape"))
            .or_else(|| get(entry, "response").and_then(|r| get_str(r, "referenced_shape")));
        let raw_shape_ref = response.and_then(|r| get_str(r, "shape_ref"));
        let ambiguous_union = referenced_shape.is_none()
            && raw_shape_ref
                .and_then(|sr| shapes.get(sr.trim_start_matches("#/shapes/")))
                .map(|s| crate::success_shape::is_candidate(s, &shapes))
                .unwrap_or(false);
        if ambiguous_union {
            notes.push(format!(
                "{}'s response is a two-branch success/error union with no unambiguous source evidence (a status-code correlation or a shared discriminator) and no response.referenced_shape judgement recorded in selection.yaml; the stub's body is typed as opaque JSON ({{}}) until one is recorded",
                name
            ));
        }
        // The shape the body is built from, and deepen walks below: the
        // resolved success branch, never the union wrapper, which has no
        // properties for `prop_shape` to find a selected key in.
        let body_shape = referenced_shape.or(raw_shape_ref);
        let mut body: Option<Value> = body_shape.filter(|_| json_response).map(|sr| {
            if ambiguous_union {
                Value::Object(obj())
            } else {
                examples.example(&crate::json::object(vec![("$ref", Value::from(sr))]), name)
            }
        });
        if !json_response {
            notes.push(format!(
                "the response is {}, not JSON; the stub answers {} with no body and the scaffold writes no unit entry (lint missing-unit) — write the body by hand",
                response_content_type.as_deref().unwrap_or("of an unknown content type"),
                status
            ));
        }
        notes.append(&mut examples.notes);
        for r in &examples.unresolved {
            notes.push(format!(
                "the inventory has no shape `{}` that the response references; the stub carries `{{}}` there",
                r
            ));
        }
        if let (Some(b), Some(sr)) = (body.as_mut(), body_shape) {
            deepen(
                &nodes,
                b,
                &crate::json::object(vec![("$ref", Value::from(sr))]),
                &shapes,
            );
        }
        // The source answers a sparse GET with the fields the declared
        // default requests and nothing else, so the stub does too: a
        // mapped key the default leaves out resolves null in e2e, as it
        // would against the source (ADR 0045).
        if let (Some(d), Some(_), Some(b)) = (&sparse_default, &sparse_arg, body.as_mut()) {
            let keep: Vec<String> = crate::sparse::top_level_names(d)
                .iter()
                .map(|p| returned_key(p))
                .collect();
            *b = narrow_body(b, &keep, sparse_list.as_deref());
            let entity: Vec<&Node> = match &sparse_list {
                Some(l) => nodes
                    .iter()
                    .filter(|n| n.key.as_deref() == Some(std::slice::from_ref(l)))
                    .flat_map(|n| n.children.iter().flatten())
                    .collect(),
                None => nodes.iter().collect(),
            };
            let unrequested: Vec<String> = entity
                .iter()
                .filter_map(|n| {
                    n.key
                        .as_ref()
                        .filter(|k| k.len() == 1)
                        .map(|k| k[0].clone())
                })
                .filter(|k| !keep.contains(k))
                .collect();
            if !unrequested.is_empty() {
                notes.push(format!(
                    "the `{}` default `{}` does not request `{}`, which the connector maps; the stub leaves it out as the source would, so it resolves null — fix the default (lint sparse-fieldsets)",
                    sparse_param,
                    d,
                    unrequested.join("`, `")
                ));
            }
        }
        match &mut body {
            Some(b) => align_enums(&nodes, b, &mut schema, return_type.as_deref()),
            None if !json_response => {}
            None => notes.push(format!(
                "the spec documents no response body for {}; the stub answers {} with no body, and the scaffold writes no unit entry (lint missing-unit: apiResponseBody would have to carry every selected key) — the e2e case proves it",
                status, status
            )),
        }

        // The stub's request matcher.
        let mut request = obj();
        request.insert("method".into(), Value::from(method.as_str()));
        request.insert("urlPath".into(), Value::from(path_only.as_str()));
        if let Some(h) = header_matcher(&cred) {
            request.insert("headers".into(), h);
        }
        let query_matchers = |chosen: &[(Arg, Sample)]| -> Object {
            let mut qp = obj();
            for (k, v) in &literal_query {
                qp.insert(
                    k.clone(),
                    crate::json::object(vec![("equalTo", Value::from(v.as_str()))]),
                );
            }
            for (arg, wire_key) in &wire.query_keys {
                // A dotted key reads a leaf, not the whole argument: below.
                if dotted.iter().any(|(_, k)| k == wire_key) {
                    continue;
                }
                let s = match sample_of(chosen, arg) {
                    Some(s) => s,
                    None => continue,
                };
                let matcher = match &s {
                    Sample::List(items) if repeated(wire_key) => crate::json::object(vec![(
                        "hasExactly",
                        Value::Array(
                            items
                                .iter()
                                .map(|i| {
                                    crate::json::object(vec![("equalTo", Value::from(i.wire()))])
                                })
                                .collect(),
                        ),
                    )]),
                    other => crate::json::object(vec![("equalTo", Value::from(other.wire()))]),
                };
                qp.insert(wire_key.clone(), matcher);
            }
            // An input object's leaves on dotted keys: each asserted as its
            // leaf's value; a list leaf as the key repeated per element.
            for (path, wire_key) in &dotted {
                let leaf = match sample_of(chosen, &path[0])
                    .and_then(|s| sample_at(&s, &path[1..]).cloned())
                {
                    Some(l) => l,
                    None => continue,
                };
                if matches!(leaf, Sample::Object(_)) {
                    continue;
                }
                let matcher = match &leaf {
                    Sample::List(items) if repeated(wire_key) => crate::json::object(vec![(
                        "hasExactly",
                        Value::Array(
                            items
                                .iter()
                                .map(|i| {
                                    crate::json::object(vec![("equalTo", Value::from(i.wire()))])
                                })
                                .collect(),
                        ),
                    )]),
                    other => crate::json::object(vec![("equalTo", Value::from(other.wire()))]),
                };
                qp.insert(wire_key.clone(), matcher);
            }
            if let (Some(d), Some(_)) = (&sparse_default, &sparse_arg) {
                qp.entry(sparse_param.clone()).or_insert_with(|| {
                    crate::json::object(vec![("equalTo", Value::from(d.as_str()))])
                });
            }
            qp
        };
        let qp = query_matchers(&samples);
        if !qp.is_empty() {
            request.insert("queryParameters".into(), Value::Object(qp));
        }
        // The write body. The stub asserts what the router sends (typed
        // values, the lists included).
        let stub_body = |chosen: &[(Arg, Sample)]| -> Option<Value> {
            if !(is_write && asserted_body) {
                return None;
            }
            if let Some(tree) = &nested {
                let mut args = obj();
                for (a, s) in chosen {
                    args.insert(a.name.clone(), s.json());
                }
                // A whole-body argument the case does not pass: what the
                // router sends then is not known, so nothing is asserted.
                return match eval_body(tree, &args) {
                    Value::Null => None,
                    v => Some(v),
                };
            }
            let mut b = obj();
            for (arg, k) in &body_map {
                if let Some(s) = sample_of(chosen, arg) {
                    b.insert(k.clone(), s.json());
                }
            }
            Some(Value::Object(b))
        };
        let no_body_mapping = is_write && !wire.sends_body;
        let loose_body_reason = if no_body_mapping {
            Some("the connector declares no body mapping: there is no body to assert")
        } else {
            None
        };
        if let Some(b) = stub_body(&samples) {
            request.insert(
                "bodyPatterns".into(),
                Value::Array(vec![crate::json::object(vec![("equalToJson", b)])]),
            );
        }
        let resp = stub_response(&status, body.as_ref(), &mut notes);
        let mut metadata = obj();
        metadata.insert(
            "x-scaffold".into(),
            Value::from(format!(
                "{}: generated by graphos-factory-core scaffold from {} and the inventory shape; placeholder values — audit before committing",
                today, key
            )),
        );
        if let Some(r) = loose_body_reason {
            metadata.insert("x-loose-body".into(), Value::from(r));
        }
        let mapping = crate::json::object(vec![
            ("request", Value::Object(request)),
            ("response", Value::Object(resp)),
            ("metadata", Value::Object(metadata)),
        ]);

        // The document.
        let sel = render_selection(&nodes, 4);
        let selection_set = if sel.trim().is_empty() {
            String::new()
        } else {
            format!(" {{\n{}  }}", sel)
        };
        let document_with = |chosen: &[(Arg, Sample)], what: &str, selection_set: &str| -> String {
            let arg_text = if chosen.is_empty() {
                String::new()
            } else {
                format!(
                    "(\n{}  )",
                    chosen
                        .iter()
                        .map(|(a, s)| format!("    {}: {}\n", a.name, s.graphql()))
                        .collect::<String>()
                )
            };
            format!(
                "# scaffold: generated by graphos-factory-core scaffold on {} from {}.\n# {} Audit, then\n# `e2e.sh --generate` for the snapshot; remove this header when you have.\n{} {{\n  {}{}{}\n}}\n",
                today, key, what, root, field, arg_text, selection_set
            )
        };
        let document_for =
            |chosen: &[(Arg, Sample)], what: &str| document_with(chosen, what, &selection_set);
        let document = document_for(
            &samples,
            if sparse_arg.is_some() {
                "Placeholder values chosen to be valid, every argument passed (lists with two\n# elements) but the fields list, whose declared default the router sends,\n# everything the connector maps selected."
            } else {
                "Placeholder values chosen to be valid, every argument passed (lists with two\n# elements), everything the connector maps selected."
            },
        );
        // A required argument scaffold left out on purpose (no placeholder for
        // an input object or custom scalar; the notes say "pass it by hand")
        // is the one expected validation error: the draft is still written.
        // Anything else means the rendered document is wrong, and the
        // operation is refused.
        let omitted_required: Vec<&str> = args_decl
            .iter()
            .filter(|a| a.is_required() && !samples.iter().any(|(s, _)| s.name == a.name))
            .map(|a| a.name.as_str())
            .collect();
        if let Err(e) = apollo_compiler::ExecutableDocument::parse_and_validate(
            &target_schema,
            &document,
            "generated.graphql",
        ) {
            // Matched on the root field's own coordinate, so a nested field's
            // required argument that happens to share the name still refuses.
            let unexpected = e.errors.iter().any(|d| {
                let m = d.error.to_string();
                !(m.contains("is not provided")
                    && omitted_required
                        .iter()
                        .any(|n| m.contains(&format!("`{}({}:)`", target, n))))
            });
            if unexpected {
                invalid.push((
                    key.clone(),
                    format!(
                        "the rendered document is not valid GraphQL against the target schema: {}",
                        e
                    ),
                ));
                continue;
            }
        }

        // A write with optional arguments also gets its minimal case:
        // required arguments only, the body (when flat) asserted exactly, so
        // the dropped-null behaviour is proven (the write_body_proof layer's serialization.mutation-cases).
        let required: Vec<(Arg, Sample)> = minimal_samples;
        let mut minimal: Option<(String, String, Value)> = None;
        if is_write && !args_decl.is_empty() && required.len() < samples.len() {
            let mut req = obj();
            req.insert("method".into(), Value::from(method.as_str()));
            req.insert("urlPath".into(), Value::from(path_only.as_str()));
            if let Some(h) = header_matcher(&cred) {
                req.insert("headers".into(), h);
            }
            // Every query key the full case asserts and this one drops is
            // `absent`: without it this stub also matches the full case's
            // request, and once every stub loads together WireMock's
            // tie-break, not the running case, decides which answers.
            let mut qp = query_matchers(&required);
            for (k, _) in query_matchers(&samples) {
                qp.entry(k)
                    .or_insert_with(|| serde_json::json!({ "absent": true }));
            }
            if !qp.is_empty() {
                req.insert("queryParameters".into(), Value::Object(qp));
            }
            match stub_body(&required) {
                Some(b) => {
                    req.insert(
                        "bodyPatterns".into(),
                        Value::Array(vec![crate::json::object(vec![("equalToJson", b)])]),
                    );
                }
                None if wire.sends_body => notes.push(
                    "the minimal case's stub asserts no body either — write it by hand".to_string(),
                ),
                None => {}
            }
            let mresp = stub_response(&status, body.as_ref(), &mut Vec::new());
            let mut mmeta = obj();
            mmeta.insert(
                "x-scaffold".into(),
                Value::from(format!(
                    "{}: generated by graphos-factory-core scaffold from {} (required arguments only); placeholder values — audit before committing",
                    today, key
                )),
            );
            if let Some(r) = loose_body_reason {
                mmeta.insert("x-loose-body".into(), Value::from(r));
            }
            minimal = Some((
                format!("{}_minimal", case),
                document_for(
                    &required,
                    "Only the required arguments: the body must be exactly these keys — every\n# optional argument dropped, not sent as null.",
                ),
                crate::json::object(vec![
                    ("request", Value::Object(req)),
                    ("response", Value::Object(mresp)),
                    ("metadata", Value::Object(mmeta)),
                ]),
            ));
        }

        // The unit entry — only when the scaffold can write an honest one:
        // a response body to supply, and a body assertion when the
        // connector declares a body (rover refuses a write entry without
        // one; an asserted `{}` would be a falsehood).
        // A fields default with a comma cannot be asserted by rover: it
        // percent-encodes the comma and the URL compare fails on every
        // request form, so the two e2e cases are the proof (ADR 0045).
        let comma_default =
            sparse_arg.is_some() && sparse_default.as_deref().is_some_and(|d| d.contains(','));
        if comma_default {
            notes.push(format!(
                "no unit entry: the `{}` default `{}` puts a comma in the asserted query string, which rover percent-encodes so the URL assertion cannot match; the e2e cases prove the default and a narrowed value (lint missing-unit accepts this). When no other operation has a unit entry there is no suite and unit.sh fails: hand-write one that passes a single field (`{}: \"id\"`), or an empty suite whose header cites the decision (`not_run`, never a pass; testing.md § Scaffolding the tests)",
                sparse_param,
                sparse_default.as_deref().unwrap_or(""),
                sparse_param
            ));
        }
        let unit_samples: Vec<(Arg, Sample)> = match (&sparse_arg, &sparse_default) {
            (Some(a), Some(d)) => {
                let mut v = samples.clone();
                v.push((a.clone(), Sample::Str(d.clone())));
                v
            }
            _ => samples.clone(),
        };
        // rover connector test rejects an object-valued $args ("invalid type:
        // map, expected a string"), and one passed as a JSON string is never
        // built into an input object: a required input object means no unit
        // entry at all; an optional one is left out of it, like a list.
        // A whole-body `$args.x` whose value is an object is the body itself:
        // leaving it out would assert `{}`, so it counts as required here.
        let whole_body_arg = whole_arg.clone();
        let required_input_object = samples
            .iter()
            .any(|(a, s)| s.has_object() && a.is_required());
        let whole_body_object = samples
            .iter()
            .any(|(a, s)| s.has_object() && whole_body_arg.as_deref() == Some(a.name.as_str()));
        let required_object = required_input_object || whole_body_object;
        if required_input_object && body.is_some() {
            notes.push("a required input-object argument cannot be passed to rover connector test (it rejects object-valued $args); the scaffold writes no unit entry (lint missing-unit) and the e2e case proves the request".to_string());
        } else if whole_body_object && body.is_some() {
            notes.push("the whole-body argument is an object, which rover connector test cannot pass (it rejects object-valued $args), and leaving it out would assert an empty body; the scaffold writes no unit entry (lint missing-unit) and the e2e case proves the request".to_string());
        }
        // A unit entry whose request body `validate` would fail is not
        // written either: rover drops a list $args and stringifies a scalar,
        // so a required body list is missing and a required integer, number
        // or boolean is a string. A waiver cannot cover a body that fails,
        // only one the oracle could not judge; the e2e case proves the request.
        let body_cannot_conform = is_write
            && asserted_body
            && !required_object
            && samples.iter().any(|(a, s)| {
                a.is_required()
                    && is_body_key(&a.name)
                    && !s.has_object()
                    && (s.is_list() || !s.is_stringy())
            });
        if body_cannot_conform && body.is_some() {
            notes.push("a required list or integer/number/boolean body argument cannot be sent by rover connector test as the spec wants it (it drops list $args and stringifies scalars), so that unit request body would fail `graphos-factory-core validate`; the scaffold writes no unit entry (lint missing-unit) and the e2e case proves the request".to_string());
        }
        let unit_entry = match &body {
            Some(body_value)
                if (!wire.sends_body || asserted_body)
                    && !comma_default
                    && !required_object
                    && !body_cannot_conform =>
            {
                // What the unit entry can carry. rover connector test rejects
                // list-valued $args, percent-encodes `:` and `,` in query
                // values in a way the URL assertion cannot match, and
                // stringifies every scalar it puts in a body — an integer or
                // boolean body field asserted as a string fails the request
                // conformance check. The e2e case proves what is left out; a
                // required body field stays, whatever its type.
                let mut unit_args: Vec<(Arg, Sample)> = Vec::new();
                let (mut dropped_lists, mut dropped_punct, mut dropped_typed, mut kept_typed) =
                    (false, false, false, false);
                let mut dropped_objects = false;
                for (a, s) in &unit_samples {
                    if s.has_object() {
                        dropped_objects = true;
                        continue;
                    }
                    if s.is_list() {
                        dropped_lists = true;
                        continue;
                    }
                    let w = s.wire();
                    if is_query_key(&a.name) && (w.contains(':') || w.contains(',')) {
                        dropped_punct = true;
                        continue;
                    }
                    if is_body_key(&a.name) && !s.is_stringy() {
                        if a.is_required() {
                            kept_typed = true;
                        } else {
                            dropped_typed = true;
                            continue;
                        }
                    }
                    unit_args.push((a.clone(), s.clone()));
                }
                if dropped_objects {
                    notes.push("optional input-object arguments are left out of the unit entry: rover connector test rejects object-valued $args (the e2e case proves them)".to_string());
                }
                if dropped_lists {
                    notes.push("list arguments are left out of the unit entry: rover connector test rejects list-valued $args (the e2e case proves them)".to_string());
                }
                if dropped_punct {
                    notes.push("query arguments whose sample carries `:` or `,` are left out of the unit entry: rover percent-encodes them and the URL assertion cannot match (the e2e case proves them)".to_string());
                }
                if dropped_typed {
                    notes.push("optional integer/number/boolean body arguments are left out of the unit entry: rover stringifies every scalar $args, and the asserted body must conform to the request shape (the e2e case proves them)".to_string());
                }
                if kept_typed {
                    // Only where body_cannot_conform above does not apply: a
                    // method it does not count as a write (a DELETE that
                    // sends a body).
                    notes.push("a required integer/number/boolean body argument stays in the unit entry as the string rover sends; `graphos-factory-core validate` will fail that request body, and a waiver cannot cover a body that fails — drop the entry by hand and let the e2e case prove the request".to_string());
                }
                let mut query_string: Vec<String> = literal_query
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, urlencode(v)))
                    .collect();
                for (arg, k) in &wire.query_keys {
                    if let Some(s) = sample_of(&unit_args, arg) {
                        query_string.push(format!("{}={}", k, urlencode(&s.wire())));
                    }
                }
                let unit_url = format!(
                    "{}{}{}",
                    base_url.trim_end_matches('/'),
                    path_only,
                    if query_string.is_empty() {
                        String::new()
                    } else {
                        format!("?{}", query_string.join("&"))
                    }
                );
                let mapped = match project(&nodes, body_value) {
                    Ok(m) => Some(m),
                    Err(Unprojectable::MappingLanguage) => {
                        notes.push("the selection uses the mapping language (`->` or `$(…)`); the unit entry asserts no connectorResponse (lint unit-no-response) — write it by hand".to_string());
                        None
                    }
                    Err(Unprojectable::MissingKey(k)) => {
                        notes.push(format!(
                        "the selection names `{}`, which the shape-built body does not carry (a self-referential shape cut short, or a field the spec does not document); the unit entry asserts no connectorResponse (lint unit-no-response) — fill the body and write it by hand",
                        k
                    ));
                        None
                    }
                };
                let indent = |v: &Value, n: usize| {
                    crate::json::pretty(v)
                        .trim_end()
                        .replace('\n', &format!("\n{}", " ".repeat(n)))
                };
                let mut unit = format!(
                "  {} on {} from {}; audit the\n  # values, then remove this comment.\n  - name: \"{} request shape{}\"\n    target: \"{}\"\n",
                ENTRY_MARKER,
                today,
                key,
                field,
                if mapped.is_some() { " and mapping" } else { "" },
                target
            );
                if !unit_args.is_empty() {
                    unit.push_str("    variables:\n      $args:\n");
                    for (a, s) in &unit_args {
                        unit.push_str(&format!("        {}: {}\n", a.name, yaml_scalar(&s.json())));
                    }
                }
                unit.push_str(&format!(
                    "    apiResponseBody: |\n      {}\n",
                    indent(body_value, 6)
                ));
                unit.push_str(&format!(
                    "    expect:\n      connectorRequest:\n        method: {}\n        url: {}\n",
                    method, unit_url
                ));
                if let Some((h, v)) = &cred {
                    unit.push_str(&format!("        headers:\n          {}: {}\n", h, v));
                }
                if asserted_body && is_write {
                    // What rover builds from scalar $args: every value a string.
                    let mut ub = obj();
                    if let Some(tree) = &nested {
                        let mut args = obj();
                        for (a, s) in &unit_args {
                            args.insert(a.name.clone(), Value::from(s.wire()));
                        }
                        if let Value::Object(o) = eval_body(tree, &args) {
                            ub = o;
                        }
                    }
                    for (arg, k) in &body_map {
                        if let Some(s) = sample_of(&unit_args, arg) {
                            ub.insert(k.clone(), Value::from(s.wire()));
                        }
                    }
                    unit.push_str(&format!(
                        "        body: |\n          {}\n",
                        crate::json::compact(&Value::Object(ub))
                    ));
                }
                if let Some(m) = &mapped {
                    unit.push_str(&format!(
                        "      connectorResponse: |\n        {}\n",
                        indent(m, 8)
                    ));
                }
                Some(unit)
            }
            _ => None,
        };
        // The narrowed case: one field requested, the stub demanding exactly
        // it and answering with it alone, the document selecting it alone.
        let narrowed: Option<Plan> = match (&sparse_arg, &sparse_default) {
            (Some(arg), Some(d)) => {
                let names = crate::sparse::top_level_names(d);
                let one = if names.iter().any(|n| n == "id") {
                    Some("id".to_string())
                } else {
                    names.first().cloned()
                };
                one.map(|one| {
                    let mut chosen = samples.clone();
                    chosen.push((arg.clone(), Sample::Str(one.clone())));
                    let sel = render_selection(&narrow_nodes(&nodes, &one, sparse_list.as_deref()), 4);
                    let set = if sel.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" {{\n{}  }}", sel)
                    };
                    let doc = document_with(
                        &chosen,
                        &format!(
                            "The `{}` argument narrowed to one field, `{}`: the stub demands exactly\n# that value and answers with that field alone.",
                            sparse_param, one
                        ),
                        &set,
                    );
                    let mut m = mapping.clone();
                    m["request"]["queryParameters"] = Value::Object(query_matchers(&chosen));
                    let nbody = body
                        .as_ref()
                        .map(|b| narrow_body(b, std::slice::from_ref(&one), sparse_list.as_deref()));
                    m["response"] =
                        Value::Object(stub_response(&status, nbody.as_ref(), &mut Vec::new()));
                    m["metadata"]["x-scaffold"] = Value::from(format!(
                        "{}: generated by graphos-factory-core scaffold from {} with `{}` narrowed to `{}`; placeholder values — audit before committing",
                        today, key, sparse_param, one
                    ));
                    Plan {
                        key: key.clone(),
                        case: format!("{}_{}_narrowed", case, sparse_param),
                        document: doc,
                        mapping: m,
                        unit_entry: None,
                        target: target.clone(),
                        notes: vec![],
                        extra: Vec::new(),
                    }
                })
            }
            _ => None,
        };
        if let Some(status) = pass {
            let _ = (&unit_entry, &narrowed, &minimal);
            let documented = documented_error_statuses(op);
            {
                let status = status.clone();
                let code = match representative_status(&status) {
                    Some(c) => c,
                    None => {
                        skipped.push((
                            key.clone(),
                            format!(
                                "--status {}: not a non-2xx status code, a range (4XX) or default",
                                status
                            ),
                        ));
                        continue;
                    }
                };
                let ecase = format!("{}_{}", case, status.to_ascii_lowercase());
                if !force
                    && (case_dir.join(format!("{}.graphql", ecase)).exists()
                        || mapping_dir.join(format!("{}.json", ecase)).exists())
                {
                    skipped.push((key.clone(), format!("tests/cases/{}.graphql exists", ecase)));
                    continue;
                }
                let body_shape = documented
                    .iter()
                    .find(|(st, _)| *st == status)
                    .and_then(|(_, sr)| sr.clone());
                let mut enotes = Vec::new();
                let mut m = mapping.clone();
                let body = body_shape.as_ref().map(|sr| {
                    Examples::new(&shapes, 3).example(
                        &crate::json::object(vec![("$ref", Value::from(sr.as_str()))]),
                        &ecase,
                    )
                });
                m["response"] =
                    Value::Object(stub_response(&code.to_string(), body.as_ref(), &mut enotes));
                if body.is_none() {
                    enotes.push(format!(
                        "the inventory documents no body for {} on {}; the stub answers with none",
                        status, key
                    ));
                }
                m["metadata"] = serde_json::json!({ "x-scaffold": format!(
                    "{}: generated by graphos-factory-core scaffold: the {} error case of {}; placeholder values — audit before committing",
                    today, status, key
                ) });
                let collision =
                    existing_request_collision(&mapping_dir, &m["request"]).or_else(|| {
                        // Two cases planned in this run are not on disk yet:
                        // compare the plans with each other too.
                        plans
                            .iter()
                            .find(|p| p.mapping["request"] == m["request"])
                            .map(|p| format!("the {} case planned in this run", p.case))
                    });
                if let Some(other) = collision {
                    invalid.push((
                        key.clone(),
                        format!(
                            "the {} error case's stub would match exactly the requests {} matches, so WireMock could answer either; give the two stubs distinct matchers by hand",
                            status, other
                        ),
                    ));
                    continue;
                }
                let doc = format!(
                    "# expect-upstream-status: {code}\n# scaffold: the {status} error case of {key} — the stub answers {code} {body}; e2e.sh fails the case when WireMock served another status, before the snapshot diff. Audit, then e2e.sh --generate.\n{document}",
                    code = code,
                    status = status,
                    key = key,
                    body = if body_shape.is_some() {
                        "with the documented error body"
                    } else {
                        "with no body (none documented)"
                    },
                    document = document,
                );
                plans.push(Plan {
                    key: key.clone(),
                    case: ecase,
                    document: doc,
                    mapping: m,
                    unit_entry: None,
                    target: String::new(),
                    notes: enotes,
                    extra: Vec::new(),
                });
            }
            continue;
        }
        plans.push(Plan {
            key: key.clone(),
            case,
            document,
            mapping,
            unit_entry,
            target: target.clone(),
            notes,
            extra: Vec::new(),
        });
        plans.extend(narrowed);
        if let Some((mcase, mdoc, mmapping)) = minimal {
            plans.push(Plan {
                key: key.clone(),
                case: mcase,
                document: mdoc,
                mapping: mmapping,
                unit_entry: None,
                target: target.clone(),
                notes: vec![],
                extra: Vec::new(),
            });
        }
    }

    // `batch:<Type>` names a $batch case, matched below against the keyed
    // types that have a $batch connector.
    let unmatched: Vec<&String> = only
        .iter()
        .filter(|o| !o.starts_with("batch:") && !matched_only.contains(o))
        .collect();
    if !unmatched.is_empty() {
        return usage(&format!(
            "--op {} matched no included operation in .factory/selection.yaml",
            unmatched
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    plans.extend(batch_plans(&mut BatchInputs {
        sdl: &sdl,
        inventory: &inventory,
        selection: &selection,
        shapes: &shapes,
        prefix: &prefix,
        schema: &target_schema,
        cred: &cred,
        today: &today,
        case_dir: &case_dir,
        only: &only,
        force,
        skipped: &mut skipped,
        invalid: &mut invalid,
    }));
    let unmatched_batch: Vec<&String> = only
        .iter()
        .filter(|o| o.starts_with("batch:"))
        .filter(|o| {
            !plans.iter().any(|p| &p.key == *o)
                && !skipped.iter().any(|(k, _)| k == *o)
                && !invalid.iter().any(|(k, _)| k == *o)
        })
        .collect();
    if !unmatched_batch.is_empty() {
        return usage(&format!(
            "--op {} matched no keyed type with a type-level $batch connector (batch:<inventory shape>, as `graphos-factory-core batch find` names it)",
            unmatched_batch
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let suite_rel = crate::json::relative(&dir, &suite_path);
    if plans.is_empty() {
        if json_out {
            print!(
                "{}",
                crate::json::pretty(&crate::json::object(vec![
                    ("dry_run", Value::Bool(dry_run)),
                    ("suite", Value::from(suite_rel.as_str())),
                    ("written", Value::Array(vec![])),
                    ("skipped", skipped_json(&skipped)),
                    ("invalid", skipped_json(&invalid)),
                ]))
            );
        } else if invalid.is_empty() && error_mode {
            println!("scaffold: nothing to do (--force regenerates an existing error case):");
            for (k, why) in &skipped {
                println!("  {} — {}", k, why);
            }
        } else if error_mode {
            println!("scaffold: refused — no error case could be written:");
            for (k, why) in skipped.iter().chain(&invalid) {
                println!("  {} — {}", k, why);
            }
        } else if invalid.is_empty() {
            println!("scaffold: nothing to do — every selected operation already has a case (use --op KEY --force to regenerate one)");
            for (k, why) in &skipped {
                println!("  {} — {}", k, why);
            }
        } else {
            println!("scaffold: refused — every selected operation either already has a case or rendered invalid GraphQL:");
            for (k, why) in &skipped {
                println!("  {} — {}", k, why);
            }
            for (k, why) in &invalid {
                println!("  {} — {}", k, why);
            }
        }
        return if invalid.is_empty() { 2 } else { 3 };
    }

    // The suite: parsed before anything is appended, and only opened when
    // there is an entry to append or, under `--force`, when an existing suite
    // may hold the scaffold's stale entry for a target that now gets none
    // (ADR 0051 lever 8). A missing suite is never created for nothing (an
    // empty suite makes lint's missing-unit fire for every operation), and
    // the file is written only when an entry went in or came out.
    let mut suite_changed = false;
    let mut suite: Option<Suite> =
        if plans.iter().any(|p| p.unit_entry.is_some()) || (force && suite_path.exists()) {
            match open_suite(&suite_path, &directory, &auth_var, &today) {
                Ok(s) => Some(s),
                Err(e) => return usage(&e),
            }
        } else {
            None
        };
    // The targets some plan in this run writes an entry for: a `_minimal`
    // case shares its full case's target and never has an entry of its own.
    let entry_targets: std::collections::HashSet<String> = plans
        .iter()
        .filter(|p| p.unit_entry.is_some())
        .map(|p| p.target.clone())
        .collect();
    let mut written: Vec<Value> = Vec::new();
    for p in &mut plans {
        let case_file = case_dir.join(format!("{}.graphql", p.case));
        let mapping_file = mapping_dir.join(format!("{}.json", p.case));
        if let (None, Some(suite), true, false, false) = (
            &p.unit_entry,
            suite.as_mut(),
            force,
            entry_targets.contains(&p.target),
            p.target.is_empty(),
        ) {
            // No plan writes an entry for this target now: the scaffold's
            // previous one is stale, so it goes.
            let (text, removed, _) = remove_scaffold_entry(&suite.text, &p.target);
            if removed {
                suite.text = text;
                suite_changed = true;
                p.notes.push(format!(
                    "the previous scaffold entry for {} was removed from {} (no unit entry is written now)",
                    p.target, suite_rel
                ));
            }
        }
        if let (Some(entry), Some(suite)) = (&p.unit_entry, suite.as_mut()) {
            if force {
                let (text, removed, hand_written) = remove_scaffold_entry(&suite.text, &p.target);
                suite.text = text;
                if removed {
                    p.notes.push(format!(
                        "the previous scaffold entry for {} was replaced in {}",
                        p.target, suite_rel
                    ));
                }
                if hand_written {
                    p.notes.push(format!(
                        "{} also has a hand-written entry for {}; the new scaffold entry is appended beside it — keep one",
                        suite_rel, p.target
                    ));
                }
            }
            if !suite.text.ends_with('\n') {
                suite.text.push('\n');
            }
            suite.text.push('\n');
            suite.text.push_str(&reindent(entry, suite.item_indent));
            suite_changed = true;
        }
        written.push(crate::json::object(vec![
            ("operation", Value::from(p.key.as_str())),
            ("case", Value::from(p.case.as_str())),
            (
                "document",
                Value::from(crate::json::relative(&dir, &case_file)),
            ),
            (
                "mapping",
                Value::from(crate::json::relative(&dir, &mapping_file)),
            ),
            ("unit_entry", Value::Bool(p.unit_entry.is_some())),
            (
                "extra_mappings",
                Value::Array(
                    p.extra
                        .iter()
                        .map(|(stem, _)| {
                            Value::from(crate::json::relative(
                                &dir,
                                &mapping_dir.join(format!("{}.json", stem)),
                            ))
                        })
                        .collect(),
                ),
            ),
            (
                "notes",
                Value::Array(p.notes.iter().map(|n| Value::from(n.as_str())).collect()),
            ),
        ]));
    }
    // The result must still be a suite.
    if let Some(suite) = &suite {
        if let Err(e) = crate::yaml::parse(&suite.text) {
            return usage(&format!(
                "the suite with the new entries appended is not valid YAML ({}); nothing was written — report this",
                e
            ));
        }
    }

    if !dry_run {
        for p in &plans {
            let case_file = case_dir.join(format!("{}.graphql", p.case));
            let mapping_file = mapping_dir.join(format!("{}.json", p.case));
            if let Err(e) = std::fs::create_dir_all(&case_dir)
                .and_then(|_| std::fs::create_dir_all(&mapping_dir))
            {
                eprintln!("scaffold: tests/: {}", e);
                return 1;
            }
            if let Err(e) = std::fs::write(&case_file, &p.document) {
                eprintln!("scaffold: {}: {}", case_file.display(), e);
                return 1;
            }
            if let Err(e) = std::fs::write(&mapping_file, crate::json::pretty(&p.mapping)) {
                eprintln!("scaffold: {}: {}", mapping_file.display(), e);
                return 1;
            }
            for (stem, mapping) in &p.extra {
                let extra_file = mapping_dir.join(format!("{}.json", stem));
                if let Err(e) = std::fs::write(&extra_file, crate::json::pretty(mapping)) {
                    eprintln!("scaffold: {}: {}", extra_file.display(), e);
                    return 1;
                }
            }
        }
        if let (Some(suite), true) = (&suite, suite_changed) {
            if let Err(e) = std::fs::write(&suite_path, &suite.text) {
                eprintln!("scaffold: {}: {}", suite_path.display(), e);
                return 1;
            }
        }
    }
    if json_out {
        print!(
            "{}",
            crate::json::pretty(&crate::json::object(vec![
                ("dry_run", Value::Bool(dry_run)),
                ("suite", Value::from(suite_rel.as_str())),
                ("written", Value::Array(written)),
                ("skipped", skipped_json(&skipped)),
                ("invalid", skipped_json(&invalid)),
            ]))
        );
    } else {
        println!(
            "scaffold{}: {} operation{} → tests/cases, tests/fixtures/mappings and {}",
            if dry_run { " (dry run)" } else { "" },
            plans.len(),
            if plans.len() == 1 { "" } else { "s" },
            suite_rel
        );
        for p in &plans {
            println!("  {} → {}", p.key, p.case);
            for n in &p.notes {
                println!("      note: {}", n);
            }
        }
        for (k, why) in &skipped {
            println!("  skipped {} — {}", k, why);
        }
        for (k, why) in &invalid {
            println!("  refused {} — {}", k, why);
        }
        let d = dir.display();
        println!(
            "  next: audit every value, then `bash $S/e2e.sh {} --generate` for the snapshots, `bash $S/unit.sh {}`, `graphos-factory-core validate {}`, `graphos-factory-core lint {}` ($S: the skill's scripts directory)",
            d, d, d, d
        );
    }
    if invalid.is_empty() {
        0
    } else {
        3
    }
}

fn skipped_json(skipped: &[(String, String)]) -> Value {
    Value::Array(
        skipped
            .iter()
            .map(|(k, why)| {
                crate::json::object(vec![
                    ("operation", Value::from(k.as_str())),
                    ("reason", Value::from(why.as_str())),
                ])
            })
            .collect(),
    )
}

/// The non-2xx statuses the inventory documents for an operation, once
/// each, with the error body's shape when one is recorded (ADR 0077).
pub(crate) fn documented_error_statuses(op: &Value) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for e in get_arr(op, "errors").into_iter().flatten() {
        let status = match get_str(e, "status") {
            Some(s) if !s.starts_with('2') => s.to_string(),
            _ => continue,
        };
        if !out.iter().any(|(s, _)| *s == status) {
            out.push((status, get_str(e, "shape_ref").map(str::to_string)));
        }
    }
    out
}

/// The code an error case's stub answers for a documented status: the
/// code itself, the lowest of a range (`4XX` -> 400), 500 for `default`.
fn representative_status(status: &str) -> Option<u16> {
    if let Ok(n) = status.parse::<u16>() {
        return (!(200..300).contains(&n) && (100..600).contains(&n)).then_some(n);
    }
    let s = status.to_ascii_uppercase();
    if s == "DEFAULT" {
        return Some(500);
    }
    if s.len() == 3 && s.ends_with("XX") {
        return s[..1]
            .parse::<u16>()
            .ok()
            .filter(|d| *d != 2)
            .map(|d| d * 100);
    }
    None
}

/// Whether a stub answering `answered` exercises the documented `status`
/// (the rule `failure-case-missing` applies, ADR 0072).
pub(crate) fn status_covers(status: &str, answered: &str) -> bool {
    if answered.starts_with('2') {
        return false;
    }
    if status.eq_ignore_ascii_case("default") {
        return true;
    }
    let s = status.to_ascii_uppercase();
    if s.len() == 3 && s.ends_with("XX") {
        return answered.starts_with(&s[..1]);
    }
    status == answered
}

/// Whether `mapping` answers `case`, by e2e.sh's classification: its
/// `metadata."x-cases"`, `x-shared`, or a basename matching the case.
fn mapping_serves(mapping_name: &str, mapping: &Value, case: &str) -> bool {
    let case = case.replace('-', "_");
    let metadata = get(mapping, "metadata");
    if let Some(Value::Array(list)) = metadata.and_then(|m| crate::json::field(m, "x-cases")) {
        return list
            .iter()
            .filter_map(Value::as_str)
            .any(|c| c.replace('-', "_") == case);
    }
    if metadata.and_then(|m| crate::json::field(m, "x-shared")) == Some(&Value::Bool(true)) {
        return true;
    }
    mapping_name.trim_end_matches(".json").replace('-', "_") == case
}

fn mapping_files(mapping_dir: &Path) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(mapping_dir) {
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        for f in files {
            if let Some(m) = std::fs::read_to_string(&f)
                .ok()
                .and_then(|t| crate::json::parse(&t).ok())
            {
                out.push((f.file_name().unwrap().to_string_lossy().to_string(), m));
            }
        }
    }
    out
}

/// Whether `path` (a request path, query already dropped) ends with the
/// operation's path `template`, segment by segment: a `{parameter}` stands
/// for exactly one non-empty segment, every other segment must be equal, and
/// a base path may stand in front (ADR 0077). e2e.sh applies the same rule to
/// the router's request journal.
pub(crate) fn path_ends_with_template(path: &str, template: &str) -> bool {
    let p: Vec<&str> = path.split('/').collect();
    let q: Vec<&str> = template.trim_start_matches('/').split('/').collect();
    if p.len() < q.len() {
        return false;
    }
    p[p.len() - q.len()..].iter().zip(&q).all(|(ps, qs)| {
        if qs.starts_with('{') && qs.ends_with('}') {
            !ps.is_empty()
        } else {
            ps == qs
        }
    })
}

/// Whether a stub's `request` matcher is for the operation `method_path`
/// (`get:/repos/{o}/{r}`): the method equal ignoring case (an absent method or
/// `ANY` matches any), and a path form this reads that ends with the
/// template: `urlPath`, `url` (query dropped), `urlPathTemplate` (parameter
/// names ignored) or `urlPathPattern` (its regex tested against the template
/// with each `{parameter}` replaced by a sample segment). Any other form
/// matches nothing, so it earns no credit.
pub(crate) fn stub_request_is_for(request: &Value, method_path: &str) -> bool {
    let (method, template) = match method_path.split_once(':') {
        Some(x) => x,
        None => return false,
    };
    match get_str(request, "method") {
        Some(m) if !m.eq_ignore_ascii_case("ANY") && !m.eq_ignore_ascii_case(method) => {
            return false
        }
        _ => {}
    }
    if let Some(p) = get_str(request, "urlPath") {
        return path_ends_with_template(p, template);
    }
    if let Some(u) = get_str(request, "url") {
        return path_ends_with_template(u.split('?').next().unwrap_or(u), template);
    }
    if let Some(p) = get_str(request, "urlPathTemplate") {
        let param = Regex::new(r"\{[^}/]*\}").unwrap();
        let (a, b) = (
            param.replace_all(p.trim_start_matches('/'), "{}"),
            param.replace_all(template.trim_start_matches('/'), "{}"),
        );
        let (a, b): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
        return a.len() >= b.len() && a[a.len() - b.len()..] == b[..];
    }
    if let Some(p) = get_str(request, "urlPathPattern") {
        let sample = Regex::new(r"\{[^}/]*\}")
            .unwrap()
            .replace_all(template, "sample");
        return Regex::new(&format!("^(?:{})$", p))
            .map(|re| re.is_match(&sample))
            .unwrap_or(false);
    }
    false
}

/// The statuses the own stubs of the e2e cases calling `field` answer to the
/// operation's OWN request (`method_path`, `get:/repos/{o}/{r}`): a status
/// served to any other request in the case, a nested lookup say, is not the
/// operation's (ADR 0077). A status is read as a string or an unsigned
/// integer; any other JSON value is not one.
fn answered_statuses(
    case_dir: &Path,
    mapping_dir: &Path,
    field: &str,
    method_path: &str,
) -> Vec<String> {
    let mappings = mapping_files(mapping_dir);
    let mut out = Vec::new();
    for entry in std::fs::read_dir(case_dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.extension().is_none_or(|e| e != "graphql") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        if crate::lint::passed_args(&text, field).is_none() {
            continue;
        }
        let case = p.file_stem().unwrap().to_string_lossy().to_string();
        for (name, m) in &mappings {
            if !mapping_serves(name, m, &case) {
                continue;
            }
            if !get(m, "request").is_some_and(|r| stub_request_is_for(r, method_path)) {
                continue;
            }
            match get(m, "response").and_then(|r| get(r, "status")) {
                Some(Value::String(s)) => out.push(s.clone()),
                Some(v) => {
                    if let Some(n) = v.as_u64() {
                        out.push(n.to_string());
                    }
                }
                None => {}
            }
        }
    }
    out
}

/// The existing stub whose request matcher is identical to `request`.
fn existing_request_collision(mapping_dir: &Path, request: &Value) -> Option<String> {
    mapping_files(mapping_dir)
        .into_iter()
        .find(|(_, m)| get(m, "request") == Some(request))
        .map(|(n, _)| n)
}

struct BatchInputs<'a> {
    sdl: &'a str,
    inventory: &'a Value,
    selection: &'a Value,
    shapes: &'a Object,
    prefix: &'a str,
    schema: &'a apollo_compiler::validation::Valid<apollo_compiler::Schema>,
    cred: &'a Option<(String, String)>,
    today: &'a str,
    case_dir: &'a Path,
    only: &'a [String],
    force: bool,
    skipped: &'a mut Vec<(String, String)>,
    invalid: &'a mut Vec<(String, String)>,
}

/// The object fields of `type_name` in the schema: `(name, named type,
/// required arguments)`.
fn schema_fields(
    schema: &apollo_compiler::Schema,
    type_name: &str,
) -> Vec<(String, String, usize)> {
    match schema.types.get(type_name) {
        Some(apollo_compiler::schema::ExtendedType::Object(o)) => o
            .fields
            .iter()
            .map(|(n, f)| {
                let required = f
                    .arguments
                    .iter()
                    .filter(|a| a.ty.is_non_null() && a.default_value.is_none())
                    .count();
                (n.to_string(), f.ty.inner_named_type().to_string(), required)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn is_object(schema: &apollo_compiler::Schema, name: &str) -> bool {
    matches!(
        schema.types.get(name),
        Some(apollo_compiler::schema::ExtendedType::Object(_))
    )
}

/// The array a response body holds its items in: the root, or the
/// property path `via` names (`envelope:values`, `inferred:results`).
fn items_mut<'v>(body: &'v mut Value, via: Option<&str>) -> Option<&'v mut Vec<Value>> {
    match via {
        None => body.as_array_mut(),
        Some(path) => {
            let segs: Vec<String> = path.split('.').map(str::to_string).collect();
            dig_mut(body, &segs).and_then(Value::as_array_mut)
        }
    }
}

/// `root`, `envelope:p` or `inferred:p` → the path under the body, if any.
fn via_path(via: &str) -> Option<&str> {
    via.split_once(':').map(|(_, p)| p)
}

/// What a root connector's own `selection` maps under `reference`: the
/// exposed names of that node's children (a rooted, unaliased envelope
/// node such as `$.results { … }` hoists its children), or a literal
/// object's keys. None when the selection does not name `reference`, or
/// names it with nothing readable under it.
fn selected_under(field_text: &str, reference: &str) -> Option<Vec<String>> {
    fn find<'n>(nodes: &'n [Node], reference: &str) -> Option<&'n Node> {
        for n in nodes {
            if exposed_name(n).as_deref() == Some(reference) && !(n.rooted && n.alias.is_none()) {
                return Some(n);
            }
        }
        nodes
            .iter()
            .filter(|n| n.rooted && n.alias.is_none())
            .find_map(|n| find(n.children.as_deref().unwrap_or(&[]), reference))
    }
    let text = connector_block(field_text, "selection")?;
    let nodes = parse_selection(&text);
    let node = find(&nodes, reference)?;
    match &node.children {
        Some(children) => Some(children.iter().filter_map(exposed_name).collect()),
        None if !node.literal_keys.is_empty() => Some(node.literal_keys.clone()),
        None => None,
    }
}

/// One case per keyed type whose schema has a type-level `$batch`
/// connector (ADR 0068 step 2): a root field that returns the entity's
/// references carrying only the key, a stub for that root operation, and a
/// second stub — the case's own, by `metadata."x-cases"` — for the bulk
/// lookup, demanding exactly the deduplicated key list in the way `batch
/// find` detected (`equalTo "k1,k2,k3"` comma-separated, `hasExactly`
/// repeated, `equalToJson` for a body array), marked `metadata."x-required"`
/// so e2e fails the case when the router never calls it. The root field
/// must be one whose own connector selects the key alone under the
/// reference: one that maps the entity's other fields lets the planner
/// resolve them locally, the lookup is never called, and the case would
/// prove nothing.
fn batch_plans(i: &mut BatchInputs) -> Vec<Plan> {
    let mut plans = Vec::new();
    let reports = match crate::batch::find(i.inventory, Some(i.selection), i.sdl, false) {
        Ok(r) => r,
        Err(e) => {
            i.invalid.push((
                "batch:*".to_string(),
                format!("the schema does not parse: {}", e),
            ));
            return plans;
        }
    };
    let ops: Vec<&Value> = get_arr(i.inventory, "operations")
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    for r in &reports {
        if r.keyed_by.is_none() || !r.has_batch_connector {
            continue;
        }
        let key = format!("batch:{}", r.type_name);
        if !i.only.is_empty() && !i.only.contains(&key) {
            continue;
        }
        let candidate = match r
            .candidates
            .iter()
            .find(|c| c.key_param.is_some() && c.problems.is_empty())
        {
            Some(c) => c,
            None => {
                i.skipped.push((key.clone(), format!("{} has a $batch connector but batch find reports it {}, not batchable; nothing to prove against", r.type_name, r.verdict.label())));
                continue;
            }
        };
        let kp = candidate.key_param.as_ref().expect("key param");
        let entity = match &r.sdl_type {
            Some(t) => t.clone(),
            None => continue,
        };
        let case = format!("{}_batch", snake(&r.type_name));
        if i.case_dir.join(format!("{}.graphql", case)).exists()
            && !(i.force && i.only.contains(&key))
        {
            i.skipped
                .push((key.clone(), format!("tests/cases/{}.graphql exists", case)));
            continue;
        }
        // The referencing root field: a Query field with no required
        // argument whose type has a field of the entity's type, backed by a
        // selected GET with no path parameter, whose own connector selects
        // the key alone under that field. Only then must the router come
        // back to the $batch connector for the rest.
        let mut chosen: Option<(String, String, String, &Value, Option<String>)> = None;
        // Roots passed over because their selection maps more than the key.
        let mut resolves_locally: Vec<String> = Vec::new();
        for (field, ty, required) in schema_fields(i.schema, "Query") {
            if required > 0 || !is_object(i.schema, &ty) {
                continue;
            }
            let reference = match schema_fields(i.schema, &ty)
                .into_iter()
                .find(|(_, t, _)| *t == entity)
            {
                Some((f, _, _)) => f,
                None => continue,
            };
            let name = if i.prefix.is_empty() {
                field.clone()
            } else {
                field
                    .strip_prefix(&format!("{}_", i.prefix))
                    .unwrap_or(&field)
                    .to_string()
            };
            let sel_op = get_obj(i.selection, "operations").and_then(|o| {
                o.iter().find(|(_, v)| {
                    get(v, "graphql").and_then(|g| get_str(g, "name")) == Some(name.as_str())
                        && get(v, "include").and_then(Value::as_bool) != Some(false)
                })
            });
            let (op_key, sel) = match sel_op {
                Some((k, v)) => (k.clone(), v),
                None => continue,
            };
            let op = match ops
                .iter()
                .find(|o| get_str(o, "key") == Some(op_key.as_str()))
            {
                Some(o) => *o,
                None => continue,
            };
            if get_str(op, "method")
                .map(str::to_ascii_uppercase)
                .as_deref()
                != Some("GET")
                || get_str(op, "path").unwrap_or("").contains('{')
            {
                continue;
            }
            let envelope = get(sel, "response")
                .and_then(|r| get_str(r, "envelope"))
                .map(str::to_string);
            // What the root connector itself maps under the reference: the
            // key alone, or the planner has no reason to call the lookup.
            let field_text = root_field_text(i.sdl, "Query", &field).unwrap_or_default();
            match selected_under(&field_text, &reference) {
                Some(names) if names.len() == 1 && names[0] == r.key => {}
                Some(names) => {
                    resolves_locally.push(format!(
                        "Query.{} selects {} {{ {} }}",
                        field,
                        reference,
                        names.join(" ")
                    ));
                    continue;
                }
                None => {
                    resolves_locally.push(format!(
                        "Query.{}'s selection does not map {} readably",
                        field, reference
                    ));
                    continue;
                }
            }
            if chosen.is_none() {
                chosen = Some((field, ty, reference, op, envelope));
            }
        }
        let (field, root_type, reference, op, envelope) = match chosen {
            Some(c) => c,
            None if !resolves_locally.is_empty() => {
                i.skipped.push((key.clone(), format!(
                    "no root field selects {}'s references as the key `{}` alone, so the router would resolve them without the $batch lookup and a case would pass without calling it ({}); select only `{}` under the reference in one root connector, or write the {} case by hand",
                    entity, r.key, resolves_locally.join("; "), r.key, case
                )));
                continue;
            }
            None => {
                i.skipped.push((key.clone(), format!(
                    "no selected Query field without required arguments (a GET with no path parameter) returns a type with a {} field; write the {} case by hand",
                    entity, case
                )));
                continue;
            }
        };
        let mut notes = vec![format!(
            "the $batch connector on {} is proven by {} answering: its stub demands the {} key list {} {} exactly",
            entity, candidate.operation, kp.passing, kp.location, kp.name
        )];
        let keys: Vec<String> = (1..=3).map(|n| format!("{}-{}", r.key, n)).collect();
        let ref_of = |k: &str| {
            let mut o = obj();
            o.insert(r.key.clone(), Value::from(k));
            Value::Object(o)
        };

        // The root stub: the operation's sampled body, two items whose
        // references share one key, so the lookup must deduplicate.
        let root_shape = crate::json::object(vec![(
            "$ref",
            Value::from(
                get(op, "response")
                    .and_then(|r| get_str(r, "shape_ref"))
                    .unwrap_or(""),
            ),
        )]);
        let mut examples = Examples::new(i.shapes, 3);
        let mut root_body = examples.example(&root_shape, &field);
        let items = match items_mut(&mut root_body, envelope.as_deref()) {
            Some(items) => items,
            None => {
                i.skipped.push((
                    key.clone(),
                    format!(
                        "the {} stub body has no item list to put {} references in",
                        field, entity
                    ),
                ));
                continue;
            }
        };
        let template = items
            .first()
            .cloned()
            .unwrap_or_else(|| Value::Object(obj()));
        let mut two = Vec::new();
        for (n, pair) in [[&keys[0], &keys[1]], [&keys[1], &keys[2]]]
            .iter()
            .enumerate()
        {
            let mut item = template.clone();
            if let Some(o) = item.as_object_mut() {
                if let Some(Value::String(_)) = o.get("id") {
                    let short = root_type.rsplit('_').next().unwrap_or(&root_type);
                    o.insert(
                        "id".into(),
                        Value::from(format!("{}-{}", snake(short), n + 1)),
                    );
                }
                o.insert(
                    reference.clone(),
                    Value::Array(pair.iter().map(|k| ref_of(k)).collect()),
                );
            }
            two.push(item);
        }
        *items = two;
        let mut request = obj();
        request.insert("method".into(), Value::from("GET"));
        request.insert(
            "urlPath".into(),
            Value::from(get_str(op, "path").unwrap_or("/")),
        );
        // Every query key the root connector can send is absent, so this
        // stub cannot answer another case's request for the same path.
        let field_text = root_field_text(i.sdl, "Query", &field).unwrap_or_default();
        let absent: Object = wiring(&field_text)
            .query_keys
            .iter()
            .map(|(_, k)| (k.clone(), serde_json::json!({ "absent": true })))
            .collect();
        if !absent.is_empty() {
            request.insert("queryParameters".into(), Value::Object(absent));
        }
        if let Some(h) = header_matcher(i.cred) {
            request.insert("headers".into(), h);
        }
        let mut root_notes = Vec::new();
        let mut mapping = obj();
        mapping.insert("request".into(), Value::Object(request.clone()));
        mapping.insert(
            "response".into(),
            Value::Object(stub_response("200", Some(&root_body), &mut root_notes)),
        );
        mapping.insert(
            "metadata".into(),
            serde_json::json!({ "x-scaffold": format!(
                "{}: generated by graphos-factory-core scaffold for the $batch connector on {}, from {}; placeholder values — audit before committing",
                i.today, entity, get_str(op, "key").unwrap_or("")
            ) }),
        );
        notes.append(&mut root_notes);

        // The lookup stub.
        let lookup_op = ops
            .iter()
            .find(|o| get_str(o, "key") == Some(candidate.operation.as_str()))
            .expect("candidate operation");
        let mut lookup_request = obj();
        let method = get_str(lookup_op, "method")
            .unwrap_or("GET")
            .to_ascii_uppercase();
        lookup_request.insert("method".into(), Value::from(method));
        lookup_request.insert(
            "urlPath".into(),
            Value::from(get_str(lookup_op, "path").unwrap_or("/")),
        );
        match (kp.location.as_str(), kp.passing.as_str()) {
            ("query", "comma-separated") => {
                lookup_request.insert(
                    "queryParameters".into(),
                    serde_json::json!({ kp.name.clone(): { "equalTo": keys.join(",") } }),
                );
            }
            ("query", "repeated") => {
                lookup_request.insert(
                    "queryParameters".into(),
                    serde_json::json!({ kp.name.clone(): { "hasExactly": keys.iter().map(|k| serde_json::json!({ "equalTo": k })).collect::<Vec<_>>() } }),
                );
            }
            ("body", "array") => {
                lookup_request.insert(
                    "bodyPatterns".into(),
                    serde_json::json!([{ "equalToJson": { kp.name.clone(): keys } }]),
                );
            }
            (loc, passing) => {
                i.skipped.push((
                    key.clone(),
                    format!("no stub matcher for a {} key list in the {}", passing, loc),
                ));
                continue;
            }
        }
        if let Some(h) = header_matcher(i.cred) {
            lookup_request.insert("headers".into(), h);
        }
        let lookup_shape = crate::json::object(vec![(
            "$ref",
            Value::from(
                get(lookup_op, "response")
                    .and_then(|r| get_str(r, "shape_ref"))
                    .unwrap_or(""),
            ),
        )]);
        let mut lookup_body = Examples::new(i.shapes, 3).example(&lookup_shape, &r.type_name);
        match items_mut(&mut lookup_body, via_path(&candidate.via)) {
            Some(items) => {
                let template = items
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Value::Object(obj()));
                *items = keys
                    .iter()
                    .map(|k| {
                        let mut item = template.clone();
                        if let Some(o) = item.as_object_mut() {
                            o.insert(r.key.clone(), Value::from(k.as_str()));
                        }
                        item
                    })
                    .collect();
            }
            None => {
                i.skipped.push((
                    key.clone(),
                    format!(
                        "the {} stub body has no item list for {}",
                        candidate.operation, entity
                    ),
                ));
                continue;
            }
        }
        let mut lookup = obj();
        lookup.insert("request".into(), Value::Object(lookup_request));
        let mut lookup_notes = Vec::new();
        lookup.insert(
            "response".into(),
            Value::Object(stub_response("200", Some(&lookup_body), &mut lookup_notes)),
        );
        lookup.insert(
            "metadata".into(),
            serde_json::json!({
                "x-cases": [case.clone()],
                "x-required": true,
                "x-scaffold": format!(
                    "{}: generated by graphos-factory-core scaffold: the {} $batch lookup, demanding keys {}; placeholder values — audit before committing",
                    i.today, entity, keys.join(", ")
                ),
            }),
        );
        notes.append(&mut lookup_notes);

        // The case: the root field, its type's key when it has one, and the
        // entity's scalar fields through the reference.
        let leaf = |t: &str| -> Vec<String> {
            schema_fields(i.schema, t)
                .into_iter()
                .filter(|(_, ty, req)| *req == 0 && !is_object(i.schema, ty))
                .map(|(n, _, _)| n)
                .collect()
        };
        let root_leaves = leaf(&root_type);
        let mut outer: Vec<String> = root_leaves
            .iter()
            .filter(|n| n.as_str() == "id")
            .cloned()
            .collect();
        if outer.is_empty() {
            outer.extend(root_leaves.first().cloned());
        }
        let inner = leaf(&entity);
        let document = format!(
            "# scaffold: generated by graphos-factory-core scaffold — proves the $batch connector on {entity}: {field} returns {reference} as keys only, and one {lookup} request must answer all three ({keys}). Audit, then e2e.sh --generate.\nquery {{\n  {field} {{\n{outer}    {reference} {{\n{inner}    }}\n  }}\n}}\n",
            entity = entity,
            field = field,
            reference = reference,
            lookup = candidate.operation,
            keys = keys.join(", "),
            outer = outer.iter().map(|n| format!("    {}\n", n)).collect::<String>(),
            inner = inner.iter().map(|n| format!("      {}\n", n)).collect::<String>(),
        );
        if let Err(e) = apollo_compiler::ExecutableDocument::parse_and_validate(
            i.schema,
            &document,
            "case.graphql",
        ) {
            i.invalid.push((
                key.clone(),
                format!("the rendered case does not validate: {}", e.errors),
            ));
            continue;
        }
        plans.push(Plan {
            key,
            case: case.clone(),
            document,
            mapping: Value::Object(mapping),
            unit_entry: None,
            target: String::new(),
            notes,
            extra: vec![(format!("{}_lookup", case), Value::Object(lookup))],
        });
    }
    plans
}

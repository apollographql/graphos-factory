//! inventory links — every candidate_entity_link fact in an inventory, flat
//! (ADR 0069). The fact sits wherever its property sits in its host shape —
//! a response shape, or a named shape a response reaches through `$ref` —
//! (`shapes.<S>.properties.<p>`, `shapes.<S>.items.properties.<p>`,
//! `shapes.<S>.properties.<a>.items.properties.<p>`, …), which
//! `inventory describe --no-shape` hides and an expanded `describe` buries;
//! this prints one row per fact with its path in the fields grammar, the
//! operations returning the host shape (none, for a host only a `$ref`
//! reaches), and — when the workspace has a
//! selection — whether the by-id operation the fact points at is included.
//! Nothing is stored: the rows are recomputed from the inventory each run.
//!
//!   inventory links [workspace] [--inventory FILE] [--json]

use crate::args::{Args, Flags};
use crate::inventory::shape_name;
use crate::json::{get, get_arr, get_obj, get_str, obj, pretty, truthy};
use crate::openapi::walk_link_hosts;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// One `candidate_entity_link` fact, located.
pub struct CandidateLink {
    /// The inventory shape (`#/shapes/<Name>`) whose property carries the fact.
    pub shape: String,
    /// The property's path from the shape's root in the fields grammar:
    /// `album_id`, `[]>album_id`, `songs[]>album_id`, `owner>account_id`.
    pub path: String,
    /// The matched by-id operation, `{method}:{path}`.
    pub operation: String,
    /// That operation's trailing path parameter, raw wire name.
    pub parameter: String,
    /// Whether the matched operation returns a list (the fact's own flag).
    pub list_context: bool,
    /// Whether the host property sits inside a list (`[]` in the path).
    pub host_is_list_item: bool,
    /// Every operation whose response shape is `shape`, in inventory order;
    /// empty for a shape a response reaches only through `$ref`.
    pub returned_by: Vec<String>,
    /// `Some(include)` once a selection has been read (an operation the
    /// selection does not list is `Some(false)`); `None` when none was.
    pub target_selected: Option<bool>,
}

impl CandidateLink {
    pub fn to_json(&self) -> Value {
        crate::json::object(vec![
            ("shape", Value::from(self.shape.as_str())),
            ("path", Value::from(self.path.as_str())),
            ("operation", Value::from(self.operation.as_str())),
            ("parameter", Value::from(self.parameter.as_str())),
            ("list_context", Value::Bool(self.list_context)),
            ("host_is_list_item", Value::Bool(self.host_is_list_item)),
            (
                "returned_by",
                Value::Array(
                    self.returned_by
                        .iter()
                        .map(|k| Value::from(k.as_str()))
                        .collect(),
                ),
            ),
            (
                "target_selected",
                match self.target_selected {
                    Some(b) => Value::Bool(b),
                    None => Value::Null,
                },
            ),
        ])
    }

    /// The fact as one annotated clause — `<path> -> <operation>
    /// (<parameter>)`, then ` (list item)` when the host sits inside a list
    /// and ` (target is not selected)` when a selection was read and the
    /// by-id operation is not included. `inventory links` prints it after
    /// `<shape> > `; `inventory list` prints it after `links: `.
    pub fn note(&self) -> String {
        let mut s = format!("{} -> {} ({})", self.path, self.operation, self.parameter);
        if self.host_is_list_item {
            s.push_str(" (list item)");
        }
        if self.target_selected == Some(false) {
            s.push_str(" (target is not selected)");
        }
        s
    }
}

/// Every fact carried by one shape, as `(path in the fields grammar, fact)`,
/// in walk order. The same `openapi::walk_link_hosts` the builder uses
/// places the facts, so the reader cannot drift from the placement grammar;
/// the walk takes `&mut`, so it runs on a clone of the node and the
/// inventory is never touched.
fn facts_of(shape: &Value) -> Vec<(String, Value)> {
    let mut scratch = shape.clone();
    let mut path: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut found: Vec<(String, Value)> = Vec::new();
    walk_link_hosts(
        &mut scratch,
        &mut path,
        &mut seen,
        0,
        &mut |prop: &mut Value, path: &[String]| {
            if let Some(fact) = get(prop, "candidate_entity_link") {
                found.push((path.join(">"), fact.clone()));
            }
        },
    );
    found
}

/// Every fact in the inventory, in shape order then walk order, with
/// `target_selected` unset.
pub fn candidate_links(inventory: &Value) -> Vec<CandidateLink> {
    let operations = get_arr(inventory, "operations")
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for (shape, node) in get_obj(inventory, "shapes").into_iter().flatten() {
        let found = facts_of(node);
        if found.is_empty() {
            continue;
        }
        let returned_by: Vec<String> = operations
            .iter()
            .filter(|op| {
                get(op, "response")
                    .and_then(|r| get_str(r, "shape_ref"))
                    .map(shape_name)
                    == Some(shape.as_str())
            })
            .filter_map(|op| get_str(op, "key").map(str::to_string))
            .collect();
        for (path, fact) in found {
            out.push(CandidateLink {
                shape: shape.clone(),
                host_is_list_item: path.contains("[]"),
                path,
                operation: get_str(&fact, "operation").unwrap_or("").to_string(),
                parameter: get_str(&fact, "parameter").unwrap_or("").to_string(),
                list_context: get(&fact, "list_context") == Some(&Value::Bool(true)),
                returned_by: returned_by.clone(),
                target_selected: None,
            });
        }
    }
    out
}

/// Fill `target_selected` from a parsed selection.yaml: the by-id
/// operation's `include`, false when the selection does not list it.
pub fn mark_selected(links: &mut [CandidateLink], selection: &Value) {
    let ops = get_obj(selection, "operations");
    for link in links.iter_mut() {
        link.target_selected = Some(
            ops.and_then(|o| o.get(&link.operation))
                .map(|entry| truthy(get(entry, "include")))
                .unwrap_or(false),
        );
    }
}

/// The selection beside an inventory, when the inventory lives in a
/// workspace's `.factory/` and the workspace has one. Read under custody.
/// `split_workspace_path` is the one `.factory` splitter (ADR 0025): it
/// yields `.` for a bare `.factory/inventory.json` and `None` for a path
/// with no `.factory` component at all (nothing beside it to read — `Ok(None)`)
/// or one carrying `..` (unsound: the root it would derive is not the
/// workspace the `.factory` component actually sits in). The two `None`
/// causes are not the same fact, so a `..` is checked first and refused
/// outright rather than folded into "no selection" — the caller asked
/// about a real file; the fact that the path spells it unsoundly should not
/// look identical to there being no selection at all.
pub fn selection_beside(inventory_file: &Path) -> Result<Option<Value>, String> {
    if inventory_file
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!(
            "selection not read: {} contains `..`",
            inventory_file.display()
        ));
    }
    let (workspace, _) = match crate::factory_io::split_workspace_path(inventory_file) {
        Some(split) => split,
        None => return Ok(None),
    };
    match crate::factory_io::read_to_string_optional(&workspace, ".factory/selection.yaml") {
        Ok(Some(text)) => crate::yaml::parse(&text)
            .map(Some)
            .map_err(|e| format!(".factory/selection.yaml: {}", e)),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The GraphQL field name a link proposes, and the one place it is derived
/// (ADR 0069): the singular of the by-id operation's last static path
/// segment, camelCased with its first letter lowercased — `/albums/{id}` →
/// `album`, `/payment_cards/{id}` → `paymentCard`, `/categories/{id}` →
/// `category`, `/users/{username}/orgs` → `org`, `/Users/{id}` → `user`;
/// `item` when the path has no static segment at all (`/{id}`), and `item`
/// again whenever the derivation is empty or is not itself a legal GraphQL
/// name (`/s/{id}`, `/2fa/{id}` — a leading digit is never a legal first
/// character). `selection draft` writes it into `links[].field`, and
/// reconcile, lint, `links apply` and scaffold fall back to it when an
/// entry has no `field`. A judgement the user confirms, never a fact the
/// inventory stores.
pub fn link_field_name(op_path: &str) -> String {
    let last = op_path
        .split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{'))
        .last()
        .unwrap_or("item");
    let name = lower_first(&camel(&singular(last)));
    if is_graphql_name(&name) {
        name
    } else {
        "item".to_string()
    }
}

/// The GraphQL field-naming convention lowercases a name's first letter;
/// `camel` never touches the first character (nothing precedes it), so this
/// is the one place that happens.
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `[_A-Za-z][_0-9A-Za-z]*` — a legal GraphQL name. `link_field_name` falls
/// back to `item` when the derivation produces anything else: an empty
/// segment (`/s/{id}` singularises to `""`), or one that starts with a
/// digit (`/2fa/{id}`).
fn is_graphql_name(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `ies` → `y`; the plural endings `sses`, `uses`, `xes`, `zes`, `ches`,
/// `shes` → drop `es` (`statuses` → `status`, `branches` → `branch`,
/// `boxes` → `box`) — a bare `ses` is not enough: `releases`, `responses`,
/// `databases` and `licenses` end in `es` too but are not one of those six
/// endings, so only their trailing `s` comes off (`releases` → `release`,
/// not `releas`); `ss` on its own is already singular (`glass` stays
/// `glass`); any other trailing `s` is dropped; anything else is already
/// singular.
fn singular(segment: &str) -> String {
    if let Some(stem) = segment.strip_suffix("ies") {
        return format!("{}y", stem);
    }
    let drops_es = ["sses", "uses", "xes", "zes", "ches", "shes"]
        .iter()
        .any(|suffix| segment.ends_with(suffix));
    if drops_es {
        return segment[..segment.len() - 2].to_string();
    }
    if segment.ends_with("ss") {
        return segment.to_string();
    }
    if let Some(stem) = segment.strip_suffix('s') {
        return stem.to_string();
    }
    segment.to_string()
}

/// `payment_card` / `payment-card` → `paymentCard`; a name without a
/// separator is unchanged.
fn camel(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut upper_next = false;
    for c in text.chars() {
        if c == '_' || c == '-' {
            upper_next = true;
            continue;
        }
        if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A canonical GET-by-id operation that is still no link target, and why
/// (ADR 0085): its response does not describe the record its key names.
/// The facts are what survived; this is where the refusals are recorded, so
/// an agent can tell "no hint" from "the target was looked at and refused".
/// Operations the generic-name rule (`id`, `name`) or a parameter with no
/// type family drops are not listed: those are ADR 0069's named limitation,
/// not a judgement about the target.
pub struct RefusedTarget {
    pub operation: String,
    pub parameter: String,
    pub reason: String,
}

impl RefusedTarget {
    pub fn to_json(&self) -> Value {
        crate::json::object(vec![
            ("operation", Value::from(self.operation.as_str())),
            ("parameter", Value::from(self.parameter.as_str())),
            ("reason", Value::from(self.reason.as_str())),
        ])
    }
}

/// Every refused target in `inventory`, in operation order — the same
/// rules `inventory build` applies, read back from the built inventory.
pub fn refused_targets(inventory: &Value) -> Vec<RefusedTarget> {
    let empty = serde_json::Map::new();
    let shapes = get_obj(inventory, "shapes").unwrap_or(&empty);
    let mut out = Vec::new();
    for op in get_arr(inventory, "operations").into_iter().flatten() {
        let Some(key) = get_str(op, "key") else {
            continue;
        };
        let Some(parameter) = crate::openapi::is_canonical_get_by_id(op) else {
            continue;
        };
        if crate::openapi::is_generic_link_name(&crate::openapi::normalize_link_name(&parameter)) {
            continue;
        }
        let has_family = get_arr(op, "parameters")
            .into_iter()
            .flatten()
            .find(|p| get_str(p, "name") == Some(parameter.as_str()))
            .and_then(crate::openapi::link_type_family)
            .is_some();
        if !has_family {
            continue;
        }
        if let Some(reason) = crate::openapi::link_target_refusal(op, &parameter, shapes) {
            out.push(RefusedTarget {
                operation: key.to_string(),
                parameter,
                reason,
            });
        }
    }
    out
}

fn fail(message: &str) -> i32 {
    eprintln!("inventory links: {}", message);
    1
}

pub const USAGE: &str = "usage: inventory links [workspace] [--inventory FILE] [--json]";

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["inventory"],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    // Like `inventory --help`: the usage on stdout, exit 0, nothing read.
    if args.has("help") || args.positional.iter().any(|p| p == "-h") {
        println!("{}", USAGE);
        return 0;
    }
    if args.positional.len() > 1 {
        return fail(USAGE);
    }
    let dir = args.dir();
    let workspace = Path::new(&dir);
    // The inventory: the file named by `--inventory`, else the workspace's
    // `.factory/inventory.json`. The named route locates the selection
    // beside whatever file it was given, through `selection_beside`; the
    // workspace route already holds a trusted custody root (the positional
    // argument), so it reads the selection directly from it instead of
    // re-deriving a root from `workspace.join(rel)` — a path that can carry
    // a `..` the caller's own argument introduced even though `workspace`
    // itself names the same directory soundly.
    let (_file, text, selection) = match args.get("inventory") {
        Some(named) => {
            let file = Path::new(named).to_path_buf();
            let bytes = match crate::factory_io::read_named_path(&file) {
                Ok(b) => b,
                Err(e) => return fail(&e),
            };
            let text = match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(_) => return fail(&format!("{}: not UTF-8", file.display())),
            };
            let selection = match selection_beside(&file) {
                Ok(s) => s,
                Err(e) => return fail(&e),
            };
            (file, text, selection)
        }
        None => {
            let rel = super::inventory::DEFAULT_INVENTORY_REL;
            let text = match crate::factory_io::read_to_string_optional(workspace, rel) {
                Ok(Some(t)) => t,
                Ok(None) => {
                    return fail(&format!(
                        "no inventory at {} — run `inventory build <document>` first",
                        workspace.join(rel).display()
                    ))
                }
                Err(e) => return fail(&e.to_string()),
            };
            let selection = match crate::factory_io::read_to_string_optional(
                workspace,
                ".factory/selection.yaml",
            ) {
                Ok(Some(t)) => match crate::yaml::parse(&t) {
                    Ok(v) => Some(v),
                    Err(e) => return fail(&format!(".factory/selection.yaml: {}", e)),
                },
                Ok(None) => None,
                Err(e) => return fail(&e.to_string()),
            };
            (workspace.join(rel), text, selection)
        }
    };
    let inventory = match crate::json::parse(&text) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
    let mut links = candidate_links(&inventory);
    if let Some(sel) = &selection {
        mark_selected(&mut links, sel);
    }
    let refused = refused_targets(&inventory);
    if args.has("json") {
        let mut report = obj();
        report.insert(
            "candidate_links".to_string(),
            Value::Array(links.iter().map(CandidateLink::to_json).collect()),
        );
        report.insert(
            "refused_targets".to_string(),
            Value::Array(refused.iter().map(RefusedTarget::to_json).collect()),
        );
        print!("{}", pretty(&Value::Object(report)));
        return 0;
    }
    if links.is_empty() {
        println!("0 candidate links (no response property matches the path parameter of a canonical GET-by-id operation whose response carries it)");
        print_refused(&refused);
        return 0;
    }
    let mut shapes: Vec<&str> = Vec::new();
    for link in &links {
        if shapes.last() != Some(&link.shape.as_str()) {
            shapes.push(&link.shape);
            println!();
        }
        println!("{} > {}", link.shape, link.note());
        if link.returned_by.is_empty() {
            // A host only a `$ref` from a response reaches (a list's
            // `items.$ref`, say): no operation returns it directly.
            println!("    returned by: no operation directly (reached through a `$ref`)");
        } else {
            println!("    returned by: {}", link.returned_by.join(", "));
        }
    }
    println!(
        "\n{} candidate links across {} shapes",
        links.len(),
        shapes.len()
    );
    if selection.is_none() {
        println!("(no .factory/selection.yaml beside the inventory: target selection unknown)");
    }
    print_refused(&refused);
    0
}

/// The refused targets, after the facts: one line each, nothing when none.
fn print_refused(refused: &[RefusedTarget]) {
    if refused.is_empty() {
        return;
    }
    println!(
        "\n{} GET-by-id operation(s) refused as link targets:",
        refused.len()
    );
    for r in refused {
        println!("  {} ({}): {}", r.operation, r.parameter, r.reason);
    }
}

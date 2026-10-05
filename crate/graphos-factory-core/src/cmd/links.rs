//! links — the relationship fields confirmed `links:` entries ask for.
//!
//!   links apply [workspace] --dry-run [--link "<shape> > <path>"] [--json]
//!
//! A confirmed, included `links:` entry in `selection.yaml` (ADR 0069) is a
//! field on its host type, resolved by the by-id operation's GET through a
//! field-level `@connect` keyed by `$this`. This prints that field's text for
//! the agent to paste into the host type. It never writes the schema: a
//! schema is edited span by span, never regenerated, and the refusals only
//! the authored schema can answer — a circular selection, a field-name
//! collision, a host without the foreign key — are reported here rather
//! than discovered at compose.
//!
//! The printed field copies the by-id root field's selection verbatim,
//! rewrites its `{$args.<p>}` path segment to `{$this.<fk>}` with the fk as
//! the host declares it, reuses the one `@source`'s name, carries no `@key`,
//! and mirrors the credential of the by-id root connector (R1) exactly as
//! lint's `link-credential` reads it (`lint::by_id_root_fields`,
//! `lint::mirrored_credential`): no argument and no header when that
//! connector authenticates through the one `@source`; the same argument
//! declaration and the same header entry or query parameter when it carries
//! a per-call credential. The root connector's static settings come along
//! (R51): a static query pair on its URI, every header entry that reads no
//! `$args` except `Authorization` (the credential's slot, which the mirror
//! owns), and every `queryParams` entry that reads no `$args`. An argument
//! that is not the credential does not.
//!
//! A foreign key the host declares nullable gets the null guard (ADR 0084):
//! the router has no way to skip a field-level connector, so for a parent
//! whose fk is null it sends the GET with an empty final segment
//! (`/albums/`), and the answer — a 307, a 404, a list — fails the field or,
//! worse, maps into a record nobody referenced. The printed connector
//! carries `isSuccess` that accepts any answer when `$this.<fk>` is null
//! (and otherwise the one `@source`'s `isSuccess`, or 2xx), and a selection
//! that maps to null when `$this.<fk>` is null, whatever the body holds.
//!
//! A confirmed entry no current fact backs is refused `target-refused`
//! (ADR 0100): the same `reconcile::LinkStaleness` reason lint's
//! `link-target-refused` and reconcile's stale drift give, and the same
//! exemption — an entry whose `decision:` names a resolved decision in
//! `.factory/decisions.json` prints as any other (gitea's D-0018).
//!
//! Exit codes: 0 printed · 1 usage, an unreadable workspace, or a refusal
//! (its kind is printed) · 2 nothing to do (no confirmed, included entry
//! left to apply).

use crate::args::{Args, Flags};
use crate::json::{get_arr, get_str};
use crate::lint::{
    by_id_root_fields, header_entry_texts, interpolated_arg_names, mirrored_credential,
    static_query_params, CallCredential, CredentialSlot,
};
use crate::op_match::OpHints;
use crate::reconcile::{
    field_spans, link_connectors, link_decision, link_hosts, link_reference_problems,
    parse_selection, read_links, stale_link_remedy, Link, LinkDecision, LinkProblem, LinkStaleness,
    Node, Spread,
};
use crate::sdl_index::{base_type, SdlIndex};
use regex::{NoExpand, Regex};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::OnceLock;

pub const USAGE: &str = "usage: graphos-factory-core links apply [workspace] --dry-run [--link \"<shape> > <path>\"] [--json]
  prints the field-level @connect for every confirmed, included links: entry; never writes the schema";

/// Why a confirmed link cannot be rendered. Every variant is something the
/// inventory cannot know and only the selection and the authored schema can
/// tell.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkRefusal {
    /// The by-id selection, copied onto the host, selects back into a type
    /// already on its path — in practice the host — and composition fails
    /// with CIRCULAR_REFERENCE (R52). `via` is the type path the copied
    /// selection walks, target first, the re-entered type last.
    Circular { via: Vec<String> },
    /// The by-id operation returns the host type itself. Since ADR 0085 a
    /// target's response carries the key its parameter names, so the same
    /// GraphQL type on both ends means the host's foreign key is the
    /// target's own key field: the field would re-read its own record. A
    /// summary type linking to a distinct detail type by the same id
    /// (`Spotify_Album.album: Spotify_AlbumDetails`) is not this: it is the
    /// summary -> detail expansion the benchmark harness wires, and it
    /// renders.
    SelfLink,
    /// The host type already declares a field of that name.
    FieldExists(String),
    /// An earlier confirmed entry in this run already prints a field of
    /// that name on the same host type for another relationship — through
    /// another operation, or keyed by another foreign key. Kind
    /// `field-exists` too: the fix is the same, a distinct `field:`.
    FieldClaimed {
        field: String,
        host: String,
        /// The entry that printed it, with its operation and fk field.
        by: String,
        by_operation: String,
        by_fk: String,
        /// This entry's operation and fk field.
        operation: String,
        fk: String,
    },
    /// The host type declares no field for the link's foreign key, so the
    /// printed `{$this.<fk>}` would read a field that is not there and
    /// composition fails (R50). `link` is the entry's `<shape> > <path>`.
    NoFkField { host: String, link: String },
    /// The link's operation is not an included GET in the inventory, no
    /// Query field carries a connector to it (or several do and the
    /// selection does not say which), or the one that does is not a by-id
    /// read the field can copy.
    NoRootField(String),
    /// No host type can be derived for the link's shape and path: the shape
    /// or path does not resolve in the inventory, or no included operation's
    /// root field returns a type for the shape (R42).
    NoHost(String),
    /// The host declares the foreign key nullable and the null guard
    /// (ADR 0084) cannot be written for this schema: connect/v0.2 and
    /// earlier have no `?!`, and under connect/v0.3 a by-id selection that
    /// holds a value reading no response body would survive a null parent.
    /// `reason` says which.
    NullableFk {
        host: String,
        fk: String,
        empty_get: String,
        reason: String,
    },
    /// The entry is confirmed but no current fact backs it (ADR 0098): its
    /// by-id operation is a target ADR 0085 refuses, or the inventory
    /// carries no `candidate_entity_link` fact for it. `reason` is
    /// `reconcile::LinkStaleness::reason`, the text lint's
    /// `link-target-refused` and reconcile's stale drift give; `pasted`
    /// names each `Type.field (line N)` already in the schema; `remedy`
    /// is `reconcile::stale_link_remedy`, by what the entry's `decision:`
    /// says (ADR 0113 §4). An entry whose `decision:` names a resolved
    /// `keep` decision is kept and never refused this way
    /// (`reconcile::link_kept_by_decision`).
    TargetRefused {
        reason: String,
        pasted: Vec<String>,
        remedy: String,
    },
}

impl LinkRefusal {
    pub fn kind(&self) -> &'static str {
        match self {
            LinkRefusal::Circular { .. } => "circular",
            LinkRefusal::SelfLink => "self",
            LinkRefusal::FieldExists(_) | LinkRefusal::FieldClaimed { .. } => "field-exists",
            LinkRefusal::NoRootField(_) => "no-root-field",
            LinkRefusal::NoHost(_) => "no-host",
            LinkRefusal::NoFkField { .. } => "no-fk-field",
            LinkRefusal::NullableFk { .. } => "nullable-fk",
            LinkRefusal::TargetRefused { .. } => "target-refused",
        }
    }
}

impl std::fmt::Display for LinkRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkRefusal::Circular { via } => write!(
                f,
                "circular — the by-id selection, copied onto the host, selects back into {} ({}); composition would fail with CIRCULAR_REFERENCE. Exclude the field that selects back from the by-id operation (`fields.exclude`, then apply it) or decline the link (`include: false` with a `reason`)",
                via.last().map(String::as_str).unwrap_or("?"),
                via.join(" -> ")
            ),
            LinkRefusal::SelfLink => write!(
                f,
                "self — the by-id operation returns the host type itself, so the field would re-read the record it sits on: decline it (`include: false`), or give the summary its own type when the detail carries fields the summary lacks"
            ),
            LinkRefusal::FieldExists(name) => write!(
                f,
                "field-exists — the host type already declares `{}`; set field: on the links entry to another camelCase name",
                name
            ),
            LinkRefusal::FieldClaimed {
                field,
                host,
                by,
                by_operation,
                by_fk,
                operation,
                fk,
            } => write!(
                f,
                "field-exists — links entry \"{}\" already prints `{}` on {} through {} keyed by `{}`, and this entry would print it through {} keyed by `{}`; set a distinct field: on one of the two entries (another camelCase name)",
                by, field, host, by_operation, by_fk, operation, fk
            ),
            LinkRefusal::NoRootField(detail) => write!(f, "no-root-field — {}", detail),
            LinkRefusal::NoHost(detail) => write!(f, "no-host — {}", detail),
            LinkRefusal::NoFkField { host, link } => write!(
                f,
                "no-fk-field — {} declares no field for {}; include the property in the selection and apply it before the link",
                host, link
            ),
            LinkRefusal::NullableFk {
                host,
                fk,
                empty_get,
                reason,
            } => write!(
                f,
                "nullable-fk — {host}.{fk} is nullable, so for a parent whose {fk} is null the router sends GET {empty_get} and the field fails (CONNECTOR_FETCH) or maps that answer; the null guard cannot be printed here: {reason}"
            ),
            LinkRefusal::TargetRefused {
                reason,
                pasted,
                remedy,
            } if pasted.is_empty() => write!(f, "target-refused — {reason}; {remedy}"),
            LinkRefusal::TargetRefused {
                reason,
                pasted,
                remedy,
            } => write!(
                f,
                "target-refused — {reason}; it is pasted as {}: {remedy}",
                pasted.join(", ")
            ),
        }
    }
}

/// The one `@source`'s `name`, which the relationship field reuses: the
/// directive's own `name:` argument, never a `name:` nested in its `http`
/// block (`headers: [{ name: "Authorization", … }]`), whichever comes first.
/// A target may count raw `@source(` text, so nothing here ever emits one.
pub fn source_name(sdl: &str) -> Option<String> {
    static NAME_RE: OnceLock<Regex> = OnceLock::new();
    let d = crate::graphql::directives(sdl, "source")
        .into_iter()
        .next()?;
    // Strings blanked, so a bracket or `name:` inside a value counts for
    // nothing; the directive's own arguments sit at depth zero.
    let code = crate::graphql::blank(&d.args);
    let depth_at = |at: usize| {
        code.as_bytes()[..at].iter().fold(0i32, |depth, c| match c {
            b'(' | b'{' | b'[' => depth + 1,
            b')' | b'}' | b']' => depth - 1,
            _ => depth,
        })
    };
    let at = NAME_RE
        .get_or_init(|| Regex::new(r"\bname\s*:").unwrap())
        .find_iter(&code)
        .map(|m| m.start())
        .find(|at| depth_at(*at) == 0)?;
    crate::reconcile::string_arg(&d.args[at..], "name")
}

/// The status test a connector applies when it declares no `isSuccess`: the
/// router's own, a 2xx answer.
pub const DEFAULT_IS_SUCCESS: &str = "$status->gte(200)->and($status->lt(300))";

/// The one `@source`'s `isSuccess` expression, when it declares one: what a
/// field-level connector inherits, and what the null guard keeps for a
/// parent whose fk is set. A block string (`"""…"""`) is folded onto one
/// line, its quotes escaped, because the guard prints a plain string; read
/// as a plain string it would capture `""` and print `[@, ])`.
fn source_is_success(sdl: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let d = crate::graphql::directives(sdl, "source")
        .into_iter()
        .next()?;
    let m = RE
        .get_or_init(|| {
            Regex::new(r#"\bisSuccess\s*:\s*(?:"""((?s:.*?))"""|"((?:[^"\\]|\\.)*)")"#).unwrap()
        })
        .captures(&d.args)?;
    match (m.get(1), m.get(2)) {
        (Some(block), _) => Some(
            block
                .as_str()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace('\\', "\\\\")
                .replace('"', "\\\""),
        ),
        (None, Some(plain)) => Some(plain.as_str().to_string()),
        _ => None,
    }
}

/// `$this.<fk>` as the null guard reads it (ADR 0084): `?!` supplies `0`
/// only when the key is absent, never when it is null. The router always
/// carries a null fk as null; `rover connector test` gives the response side
/// no `$this` at all, and without the fallback every unit entry for the
/// field would take the null branch.
fn guard_subject(fk: &str) -> String {
    format!("$($this.{} ?! 0)", fk)
}

/// The null guard's `isSuccess` (ADR 0084): any answer is a success when
/// `$this.<fk>` is null — the selection then maps to null — and otherwise
/// `base` decides, as it would without the guard.
pub fn null_guard_is_success(fk: &str, base: &str) -> String {
    format!("{}{}])", null_guard_is_success_head(fk), base)
}

/// What every guarded `isSuccess` starts with, whatever its `base`: the text
/// lint's `link-null-guard` looks for.
pub fn null_guard_is_success_head(fk: &str) -> String {
    format!("{}->match([null, true], [@, ", guard_subject(fk))
}

/// The head of a guarded selection (ADR 0084): `$this.<fk>` null maps the
/// field to null; any other value selects from the response body `$`.
pub fn null_guard_head(fk: &str) -> String {
    format!("{}->match([null, null], [@, $", guard_subject(fk))
}

/// A value in a selection that reads no response body: under the
/// subselection form of the guard it would survive a null parent and make
/// a record of nothing. The first one found, as written.
fn body_free_value(selection: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\$\(|\$(?:args|this|config|context|env|status|request|response)\b").unwrap()
    })
    .find(&crate::graphql::blank(selection))
    .map(|m| selection[m.start()..m.end()].to_string())
}

/// The minor version of the connect spec the schema links, when it links one.
fn connect_minor(sdl: &str) -> Option<u32> {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"specs\.apollo\.dev/connect/v0\.(\d+)").unwrap())
        .captures(sdl)
        .and_then(|m| m[1].parse().ok())
}

/// `selection` re-indented under the guard (ADR 0084). Under connect/v0.4
/// the arm form — `$($this.<fk> ?! 0)->match([null, null], [@, $ { … }])` —
/// maps a null parent to null exactly; composition rejects the subselection
/// form's nested objects there (`cannot find field`). Under connect/v0.3
/// only the subselection form — `…->match([null, null], [@, $]) { … }` —
/// composes, and a null parent maps to null only when every value reads the
/// body, which `render` checks first.
fn guarded_selection(fk: &str, selection: &str, arm: bool) -> String {
    let lines: Vec<&str> = selection.lines().collect();
    let first = lines.iter().position(|l| !l.trim().is_empty());
    let last = lines.iter().rposition(|l| !l.trim().is_empty());
    let body: Vec<&str> = match (first, last) {
        (Some(a), Some(b)) => lines[a..=b].to_vec(),
        _ => Vec::new(),
    };
    let indent = body
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let inner: String = body
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!(
                    "        {}",
                    &l[indent.min(l.len() - l.trim_start().len())..]
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if arm {
        format!(
            "\n      {} {{\n{}\n      }}])\n      ",
            null_guard_head(fk),
            inner
        )
    } else {
        format!(
            "\n      {}]) {{\n{}\n      }}\n      ",
            null_guard_head(fk),
            inner
        )
    }
}

/// The root connector's GET URI exactly as written, query string included:
/// `Connect.path` drops a templated query string, and a static pair in it
/// (`?format=full`) is part of the read the field copies.
fn raw_get_uri(decl: &str) -> Option<String> {
    static URI_RE: OnceLock<Regex> = OnceLock::new();
    let d = crate::graphql::directives(decl, "connect")
        .into_iter()
        .next()?;
    URI_RE
        .get_or_init(|| Regex::new(r#"\bGET\s*:\s*"([^"]*)""#).unwrap())
        .captures(&d.args)
        .map(|m| m[1].to_string())
}

/// The Query field the link copies: the one by-id root field reaching
/// `GET <op_path>` (`lint::by_id_root_fields`, the same reading
/// `link-credential` makes). Several — a Graph-style `/{id}` API — are
/// settled by the root field the selection declares for the operation
/// (`hints.for_field`, ADR 0044); anything else is refused, never the
/// declaration order.
fn by_id_root(
    sdl: &str,
    op_key: &str,
    op_path: &str,
    hints: &OpHints,
) -> Result<String, LinkRefusal> {
    let roots = by_id_root_fields(sdl, Some("GET"), Some(op_path));
    match roots.as_slice() {
        [] => Err(LinkRefusal::NoRootField(format!(
            "no Query field carries @connect(GET \"{}\") for {}; include and apply the by-id operation first",
            op_path, op_key
        ))),
        [one] => Ok(one.clone()),
        several => {
            let declared: Vec<&String> = several
                .iter()
                .filter(|r| hints.for_field("Query", r).contains(&op_key))
                .collect();
            match declared.as_slice() {
                [one] => Ok((*one).clone()),
                _ => Err(LinkRefusal::NoRootField(format!(
                    "{} Query fields reach GET {} ({}) and the selection declares none of them alone for {}; give the operation its graphql name",
                    several.len(),
                    op_path,
                    several.join(", "),
                    op_key
                ))),
            }
        }
    }
}

/// The cycle the copied selection would make, as composition reads one
/// (R52): `CIRCULAR_REFERENCE` is a connector's selection entering a type
/// already on its own path. The printed field's path starts at the host and
/// enters the target, then follows the by-id selection — the fields it
/// selects, as it selects them, and nothing else the types declare. A field
/// the selection does not select is never walked, so another field-level
/// `@connect` (a sub-resource read, the opposite link) is out of reach: two
/// links in opposite directions are not circular, and whether one is does
/// not depend on whether the other is pasted yet. A field the selection
/// does select is walked even when it carries its own `@connect` — rover
/// reports that re-entry too (`Widget.owner.widgets`, measured).
/// `Some(via)` is the type path, target first, the re-entered type last.
fn selection_cycle(
    index: &mut SdlIndex,
    host: &str,
    target: &str,
    selection: &str,
) -> Option<Vec<String>> {
    let mut path = vec![host.to_string(), target.to_string()];
    walk_selection(index, &parse_selection(selection), &mut path).map(|cycle| cycle[1..].to_vec())
}

/// One step of `selection_cycle` into type `next` under `children`: the
/// cycle when `next` is already on the path, else whatever the children
/// re-enter.
fn step_into(
    index: &mut SdlIndex,
    next: String,
    children: &[Node],
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    if path.contains(&next) {
        let mut cycle = path.clone();
        cycle.push(next);
        return Some(cycle);
    }
    path.push(next);
    let hit = walk_selection(index, children, path);
    path.pop();
    hit
}

/// One level of `selection_cycle`: `nodes` read on the type last on `path`.
fn walk_selection(
    index: &mut SdlIndex,
    nodes: &[Node],
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    let here = path.last().cloned()?;
    for n in nodes {
        // A spread's arms land in the member type each one names (ADR 0058).
        if let Some(Spread::Match(arms)) = &n.spread {
            for arm in arms {
                if let Some(member) = &arm.typename {
                    let hit = step_into(index, member.clone(), &arm.children, path);
                    if hit.is_some() {
                        return hit;
                    }
                }
            }
        }
        let (Some(key), Some(children)) = (&n.key, &n.children) else {
            continue;
        };
        if n.opaque {
            continue;
        }
        // `$.data { … }`: the children land in the same type.
        if n.rooted && n.alias.is_none() {
            if let Some(hit) = walk_selection(index, children, path) {
                return Some(hit);
            }
            continue;
        }
        let Some(field) = n.alias.clone().or_else(|| key.last().cloned()) else {
            continue;
        };
        let Some(next) = index.field_type(&here, &field).map(|t| base_type(&t)) else {
            continue;
        };
        let hit = step_into(index, next, children, path);
        if hit.is_some() {
            return hit;
        }
    }
    None
}

/// The field text for one confirmed link on one host type (ADR 0069): the
/// by-id root field's selection verbatim, its path with `{$args.p}`
/// rewritten to `{$this.<fk_field>}`, the one `@source`'s name, no `@key`,
/// and the by-id root connector's credential mirrored (R1) — no argument and
/// no header when it authenticates through the one `@source`; the same
/// argument declaration and the same header entry or query parameter when
/// it carries a per-call credential — with its static headers and
/// `queryParams` entries copied (R51). Refuses `self`, `no-fk-field` (the
/// host declares no `fk_field`, R50), `field-exists`, `circular` and
/// `no-root-field` (the last also for a target operation that is not a GET,
/// a tie between by-id root fields, and a root field that sends a
/// credential argument it does not declare). Without the selection's hints
/// a tie is always refused; `links apply` passes them.
pub fn render_link_field(
    link: &Link,
    host: &str,
    fk_field: &str,
    op: &Value,
    sdl: &str,
) -> Result<String, LinkRefusal> {
    render(link, host, fk_field, op, sdl, &OpHints::default())
}

fn render(
    link: &Link,
    host: &str,
    fk_field: &str,
    op: &Value,
    sdl: &str,
    hints: &OpHints,
) -> Result<String, LinkRefusal> {
    let op_key = get_str(op, "key").unwrap_or(link.operation.as_str());
    let op_path = get_str(op, "path").unwrap_or("?");
    if !get_str(op, "method").is_some_and(|m| m.eq_ignore_ascii_case("GET")) {
        return Err(LinkRefusal::NoRootField(format!(
            "{} is not a GET; a relationship field resolves through a GET-by-id operation only",
            op_key
        )));
    }
    let root = by_id_root(sdl, op_key, op_path, hints)?;
    let span = field_spans(sdl, "Query")
        .into_iter()
        .find(|s| s.name == root)
        .expect("by_id_root_fields names a Query field it read");
    let connect = span
        .connect
        .clone()
        .expect("by_id_root_fields matched on the connector");
    let mut index = SdlIndex::new(sdl);
    let target = index
        .field_type("Query", &root)
        .map(|t| base_type(&t))
        .ok_or_else(|| {
            LinkRefusal::NoRootField(format!("Query.{} declares no return type", root))
        })?;
    if target == host {
        return Err(LinkRefusal::SelfLink);
    }
    // `{$this.<fk>}` reads a field of the host: `link_hosts` names the fk by
    // its wire name when no root field's walk finds it declared (R50).
    if index.field_type(host, fk_field).is_none() {
        return Err(LinkRefusal::NoFkField {
            host: host.to_string(),
            link: link.key(),
        });
    }
    // The one accessor (R17): `field:` when set, else `inventory_links::link_field_name`.
    let field = link.field_name();
    if index.field_type(host, &field).is_some() {
        return Err(LinkRefusal::FieldExists(field));
    }
    let selection = connect.selection.clone().unwrap_or_default();
    if let Some(via) = selection_cycle(&mut index, host, &target, &selection) {
        return Err(LinkRefusal::Circular { via });
    }
    let source = source_name(sdl).ok_or_else(|| {
        LinkRefusal::NoRootField("the schema declares no @source to reuse".to_string())
    })?;
    let uri = raw_get_uri(&span.decl).unwrap_or_else(|| connect.path.clone().unwrap_or_default());
    let (path_part, query) = match uri.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (uri.clone(), String::new()),
    };
    static ARG_RE: OnceLock<Regex> = OnceLock::new();
    let arg_re =
        ARG_RE.get_or_init(|| Regex::new(r"\{\$args\.([A-Za-z_][A-Za-z0-9_]*)\}").unwrap());
    let count = arg_re.find_iter(&path_part).count();
    if count != 1 {
        return Err(LinkRefusal::NoRootField(format!(
            "Query.{} interpolates {} $args in its path; a by-id field interpolates exactly one",
            root, count
        )));
    }
    // `NoExpand`: `$this` in a plain replacement would read as a capture name.
    let this = format!("{{$this.{}}}", fk_field);
    let path = arg_re.replace(&path_part, NoExpand(&this)).to_string();
    // A nullable fk gets the null guard (ADR 0084): the router cannot skip
    // the request, so it must not fail on, or map, the empty-segment GET.
    let nullable = index
        .field_type(host, fk_field)
        .is_some_and(|t| !t.trim_end().ends_with('!'));
    let (is_success, selection) = if nullable {
        // connect/v0.4 composes only the arm form (the subselection form's
        // nested objects fail satisfiability, measured on rover 0.41.0);
        // connect/v0.3 composes only the subselection form; connect/v0.2
        // and earlier parse no `?!`.
        let minor = connect_minor(sdl);
        let arm = minor.is_none_or(|m| m >= 4);
        let refuse = |reason: String| LinkRefusal::NullableFk {
            host: host.to_string(),
            fk: fk_field.to_string(),
            empty_get: arg_re.replace(&path_part, NoExpand("")).to_string(),
            reason,
        };
        if let Some(m) = minor.filter(|m| *m < 3) {
            return Err(refuse(format!(
                "the schema links connect/v0.{}, which has no `?!` for the guard to read the fk with — move it to connect/v0.4 and print again",
                m
            )));
        }
        if let Some(token) = body_free_value(&selection).filter(|_| !arm) {
            return Err(refuse(format!(
                "under connect/v0.3 the guard wraps the selection in a subselection, and the by-id selection holds `{}`, a value that reads no response and would survive a null parent as a record of nothing — move the schema to connect/v0.4, or drop that value from the by-id selection, and print again",
                token
            )));
        }
        let base = source_is_success(sdl).unwrap_or_else(|| DEFAULT_IS_SUCCESS.to_string());
        (
            format!(
                "\n      isSuccess: \"{}\"",
                null_guard_is_success(fk_field, &base)
            ),
            guarded_selection(fk_field, &selection, arm),
        )
    } else {
        (String::new(), selection)
    };

    // The credential, as `link-credential` reads it (R1): what the first
    // by-id root field carrying one sends per call, or nothing when the one
    // @source authenticates every request. Derived from the SDL, never stored.
    let (credential_root, mirrored): (String, Vec<CallCredential>) =
        mirrored_credential(sdl, Some("GET"), Some(op_path)).unwrap_or_default();
    if let Some(k) = mirrored.iter().find(|k| k.arg_type.is_empty()) {
        return Err(LinkRefusal::NoRootField(format!(
            "Query.{} sends {{$args.{}}} but declares no argument {}; fix the root field first",
            credential_root, k.arg, k.arg
        )));
    }
    let mut arg_decls: Vec<String> = Vec::new();
    // The root connector's static headers first, verbatim (R51): an entry
    // that reads `$args` is a per-call credential, which the mirror below
    // carries, and `Authorization` is the credential's slot — the one
    // @source's or the mirrored argument's, never copied as a constant.
    let root_args = crate::graphql::directives(&span.decl, "connect")
        .into_iter()
        .next()
        .map(|d| d.args)
        .unwrap_or_default();
    let mut header_entries: Vec<String> = header_entry_texts(&root_args)
        .into_iter()
        .filter(|entry| {
            !entry.contains("$args")
                && !crate::reconcile::string_arg(entry, "name")
                    .is_some_and(|n| n.eq_ignore_ascii_case("authorization"))
        })
        .map(|entry| format!("{{ {} }}", entry.trim()))
        .collect();
    let mut query_credentials: Vec<(&str, &str)> = Vec::new();
    for k in &mirrored {
        let decl = format!("{}: {}", k.arg, k.arg_type);
        if !arg_decls.contains(&decl) {
            arg_decls.push(decl);
        }
        match &k.slot {
            CredentialSlot::Header { name, value } => {
                let entry = format!("{{ name: \"{}\", value: \"{}\" }}", name, value);
                if !header_entries.contains(&entry) {
                    header_entries.push(entry);
                }
            }
            CredentialSlot::Query { param } => {
                query_credentials.push((param.as_str(), k.arg.as_str()))
            }
        }
    }
    // The root URI's query string: a static pair stays, a credential pair
    // stays as written, any other `$args` pair is the root field's own
    // argument and not the link's. A credential the root sends through
    // `queryParams` joins the URI's query string — the same slot on the wire.
    let mut pairs: Vec<String> = Vec::new();
    let key_of = |pair: &str| pair.split_once('=').map_or(pair, |(k, _)| k).to_string();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let reads = interpolated_arg_names(pair);
        let credential = query_credentials
            .iter()
            .any(|(param, arg)| key_of(pair) == *param && reads.iter().any(|r| r == arg));
        if credential {
            // The pair is copied as written, so every argument it reads must
            // be one the printed field declares.
            if let Some(other) = reads
                .iter()
                .find(|r| !mirrored.iter().any(|k| &k.arg == *r))
            {
                return Err(LinkRefusal::NoRootField(format!(
                    "Query.{} sends its credential in query parameter {} together with {{$args.{}}}, which is no credential; write the field by hand",
                    credential_root,
                    key_of(pair),
                    other
                )));
            }
        }
        if reads.is_empty() || credential {
            pairs.push(pair.to_string());
        }
    }
    // A credential the root sends through `queryParams` joins the URI as
    // `param={$args.<a>}`, which is the same request only when the entry is
    // exactly `$args.<a>`. An expression or a deeper path would be dropped.
    let plain = crate::lint::wiring(&span.decl).plain_query_keys;
    for (param, arg) in &query_credentials {
        if !pairs.iter().any(|p| key_of(p) == *param) {
            if !plain
                .iter()
                .any(|(path, key)| key == param && path.len() == 1 && path[0] == *arg)
            {
                return Err(LinkRefusal::NoRootField(format!(
                    "Query.{} sends its credential {} through a queryParams expression, not exactly $args.{}; write the field by hand, sending the same expression",
                    credential_root, param, arg
                )));
            }
            pairs.push(format!("{}={{$args.{}}}", param, arg));
        }
    }
    let path = if pairs.is_empty() {
        path
    } else {
        format!("{}?{}", path, pairs.join("&"))
    };
    let args = if arg_decls.is_empty() {
        String::new()
    } else {
        format!("({})", arg_decls.join(", "))
    };
    let headers = if header_entries.is_empty() {
        String::new()
    } else {
        format!(", headers: [{}]", header_entries.join(", "))
    };
    // The root connector's static `queryParams` entries (R51); one that reads
    // `$args` is the credential's (joined to the URI above) or the root
    // field's own argument, and not the link's.
    let static_params = static_query_params(&span.decl);
    let query_params = if static_params.is_empty() {
        String::new()
    } else {
        format!(", queryParams: \"\"\"{}\"\"\"", static_params.join(" "))
    };
    Ok(format!(
        "  \"\"\"\n  The {target} referenced by `{fk}`, fetched by the router through {op}, one request per {host} that selects it; select this instead of calling Query.{root} per item.\n  \"\"\"\n  {field}{args}: {target}\n    @connect(\n      source: \"{source}\"\n      http: {{ GET: \"{path}\"{headers}{query_params} }}{is_success}\n      selection: \"\"\"{selection}\"\"\"\n    )\n",
        target = target,
        fk = fk_field,
        op = op_key,
        host = host,
        root = root,
        field = field,
        args = args,
        source = source,
        path = path,
        headers = headers,
        query_params = query_params,
        is_success = is_success,
        selection = selection,
    ))
}

fn parse_link_key(key: &str) -> Option<(String, String)> {
    let (shape, path) = key.split_once(" > ")?;
    let (shape, path) = (shape.trim(), path.trim());
    if shape.is_empty() || path.is_empty() {
        return None;
    }
    Some((shape.to_string(), path.to_string()))
}

fn wants_help(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--help" || a == "-h")
}

pub fn main(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("apply") => apply(&argv[1..]),
        Some("--help") | Some("-h") | Some("help") => {
            println!("{}", USAGE);
            0
        }
        _ => {
            eprintln!("links: expected a subcommand (apply)");
            eprintln!("{}", USAGE);
            1
        }
    }
}

/// A reference problem of a confirmed link, as the refusal it amounts to:
/// no host when the shape or path does not resolve, no root field when the
/// by-id operation is unknown or excluded. The checks are reconcile's
/// (`link_reference_problems`), the one set behind lint's `link-*` rules too.
fn reference_refusal(link: &Link, problem: LinkProblem) -> LinkRefusal {
    match problem {
        LinkProblem::UnknownShape => LinkRefusal::NoHost(format!(
            "shape {} is not in inventory.json; fix the entry's `shape`",
            link.shape
        )),
        LinkProblem::UnknownPath => LinkRefusal::NoHost(format!(
            "path {} does not resolve in shape {}; fix the entry's `path`",
            link.path, link.shape
        )),
        LinkProblem::UnknownOperation => {
            LinkRefusal::NoRootField(format!("{} is not in inventory.json", link.operation))
        }
        LinkProblem::OperationExcluded => LinkRefusal::NoRootField(format!(
            "{} is not included by the selection — the by-id operation is the field's provenance; include and apply it first",
            link.operation
        )),
    }
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const APPLY_FLAGS: Flags = Flags {
    boolean: &["dry-run", "json"],
    valued: &["link"],
};

fn apply(argv: &[String]) -> i32 {
    if wants_help(argv) {
        println!("{}", USAGE);
        return 0;
    }
    let args = Args::parse(argv, &APPLY_FLAGS);
    if !args.has("dry-run") {
        eprintln!(
            "links apply: this command prints the field text and never writes the schema — pass --dry-run"
        );
        eprintln!("{}", USAGE);
        return 1;
    }
    let json_out = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    // `.factory/*` goes through custody; the schema is an ordinary workspace
    // file the user edits by hand (ADR 0025).
    let factory = |rel: &str| crate::factory_io::read_to_string(&dir, rel).map_err(String::from);
    let loaded = (|| -> Result<(Value, Value, Value, String, String), String> {
        let workspace = crate::yaml::parse(&factory(".factory/workspace.yaml")?)?;
        let selection = crate::yaml::parse(&factory(".factory/selection.yaml")?)?;
        let inventory = crate::json::parse(&factory(".factory/inventory.json")?)?;
        let (schema_file, sdl) = crate::reconcile::read_schema_file(&dir, &workspace)?;
        Ok((workspace, selection, inventory, sdl, schema_file))
    })();
    let (workspace, selection, inventory, sdl, schema_file) = match loaded {
        Ok(v) => v,
        Err(e) => {
            eprintln!("links apply: {}", e);
            return 1;
        }
    };
    // The selection's declared root fields attribute each root field to its
    // operation when `link_hosts` derives the host, and settle a tie between
    // by-id root fields (ADR 0044, R11): the same hints reconcile builds.
    let hints = OpHints::from_selection(Some(&workspace), Some(&selection), Some(sdl.as_str()));
    let wanted = match args.get("link") {
        Some(key) => match parse_link_key(key) {
            Some(w) => Some(w),
            None => {
                eprintln!(
                    "links apply: --link takes \"<shape> > <path>\", got {:?}",
                    key
                );
                return 1;
            }
        },
        None => None,
    };
    let links = read_links(&selection);
    if let Some((s, p)) = &wanted {
        if !links.iter().any(|l| &l.shape == s && &l.path == p) {
            eprintln!(
                "links apply: no links entry \"{} > {}\" in .factory/selection.yaml",
                s, p
            );
            return 1;
        }
    }
    let ops: Vec<Value> = get_arr(&inventory, "operations")
        .cloned()
        .unwrap_or_default();
    // What the schema already carries: a link whose field is there is
    // applied, and reconcile — not this — reports any drift in it.
    let pasted_links = link_connectors(&sdl);
    let applied: Vec<(String, String)> = pasted_links
        .iter()
        .map(|lc| (lc.type_name.clone(), lc.field.clone()))
        .collect();
    // Which confirmed entries no current fact backs (ADR 0098): the one
    // definition lint and reconcile read, and the one exemption — a
    // `decision:` naming a resolved `keep` decision (ADR 0113 §4). An
    // unreadable log keeps no stale link, as in reconcile.
    let staleness = LinkStaleness::new(&inventory);
    let decisions = crate::decisions::load_present(&dir, None).ok().flatten();
    let mut index = SdlIndex::new(&sdl);
    // Several shapes the schema gives one GraphQL type (a list item and a
    // detail read of the same record) resolve to one `(host, field)`: the
    // field is printed once, for the first entry, and serves the others —
    // but only when it is the same relationship, one operation keyed by one
    // fk. `(host, field, operation, fk, by)`.
    let mut claimed: Vec<(String, String, String, String, String)> = Vec::new();
    let mut printed: Vec<Value> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let mut refused: Vec<(String, LinkRefusal)> = Vec::new();
    for link in &links {
        if let Some((s, p)) = &wanted {
            if &link.shape != s || &link.path != p {
                continue;
            }
        }
        let key = link.key();
        if !link.include {
            skipped.push((key, "declined (include: false)".to_string()));
            continue;
        }
        if !link.confirmed {
            skipped.push((
                key,
                "still the tool's draft (confirmed: false); agree it with the user, then set confirmed: true".to_string(),
            ));
            continue;
        }
        if let Some(problem) = link_reference_problems(link, &inventory, &selection)
            .into_iter()
            .next()
        {
            refused.push((key, reference_refusal(link, problem)));
            continue;
        }
        let op = ops
            .iter()
            .find(|o| get_str(o, "key") == Some(link.operation.as_str()))
            .expect("link_reference_problems found the operation");
        let hosts = link_hosts(link, &inventory, &sdl, &mut index, &hints);
        let field = link.field_name();
        // A stale entry is refused before any host question, as lint's
        // `link-target-refused` replaces `link-no-host`: the remedy is to
        // decline the link, not to give it a host or print it.
        let verdict = link_decision(link, decisions.as_ref());
        if !matches!(verdict, LinkDecision::Keep(_)) {
            if let Some(reason) = staleness.reason(link) {
                let pasted: Vec<String> = pasted_links
                    .iter()
                    .filter(|lc| lc.field == field && hosts.iter().any(|(h, _)| h == &lc.type_name))
                    .map(|lc| format!("{}.{} (line {})", lc.type_name, lc.field, lc.span.line))
                    .collect();
                let host_names: Vec<String> = hosts.iter().map(|(h, _)| h.clone()).collect();
                let remedy =
                    stale_link_remedy(link, &verdict, &reason, &host_names, !pasted.is_empty());
                refused.push((
                    key,
                    LinkRefusal::TargetRefused {
                        reason,
                        pasted,
                        remedy,
                    },
                ));
                continue;
            }
        }
        if hosts.is_empty() {
            refused.push((
                key,
                LinkRefusal::NoHost(format!(
                    "no included operation's root field returns a type for shape {}; include and apply an operation that returns it first",
                    link.shape
                )),
            ));
            continue;
        }
        for (host, fk_field) in hosts {
            if applied.contains(&(host.clone(), field.clone())) {
                skipped.push((
                    key.clone(),
                    format!(
                        "already applied: {}.{} carries its field-level @connect (reconcile reports any drift under links)",
                        host, field
                    ),
                ));
                continue;
            }
            if let Some((_, _, operation, fk, by)) = claimed
                .iter()
                .find(|(h, f, _, _, _)| h == &host && f == &field)
            {
                if operation == &link.operation && fk == &fk_field {
                    skipped.push((
                        key.clone(),
                        format!(
                            "printed once: {}.{} is the field links entry \"{}\" prints, and it serves this entry too",
                            host, field, by
                        ),
                    ));
                } else {
                    refused.push((
                        key.clone(),
                        LinkRefusal::FieldClaimed {
                            field: field.clone(),
                            host: host.clone(),
                            by: by.clone(),
                            by_operation: operation.clone(),
                            by_fk: fk.clone(),
                            operation: link.operation.clone(),
                            fk: fk_field.clone(),
                        },
                    ));
                }
                continue;
            }
            match render(link, &host, &fk_field, op, &sdl, &hints) {
                Ok(text) => {
                    claimed.push((
                        host.clone(),
                        field.clone(),
                        link.operation.clone(),
                        fk_field.clone(),
                        key.clone(),
                    ));
                    printed.push(json!({
                    "link": key,
                    "shape": link.shape,
                    "path": link.path,
                    "host": host,
                    "fk_field": fk_field,
                    "field": field,
                    "operation": link.operation,
                    "text": text,
                    }));
                }
                Err(r) => refused.push((key.clone(), r)),
            }
        }
    }
    let code = if !refused.is_empty() {
        1
    } else if printed.is_empty() {
        2
    } else {
        0
    };
    if json_out {
        let report = json!({
            "file": schema_file,
            "dry_run": true,
            "links": printed,
            "skipped": skipped.iter().map(|(l, r)| json!({"link": l, "reason": r})).collect::<Vec<_>>(),
            "refused": refused.iter().map(|(l, r)| json!({"link": l, "kind": r.kind(), "detail": r.to_string()})).collect::<Vec<_>>(),
        });
        println!("{}", crate::json::pretty(&report));
        return code;
    }
    for p in &printed {
        println!(
            "# {}.{} — paste into `type {} {{ … }}` in {}; links entry \"{}\" via {}",
            p["host"].as_str().unwrap_or("?"),
            p["field"].as_str().unwrap_or("?"),
            p["host"].as_str().unwrap_or("?"),
            schema_file,
            p["link"].as_str().unwrap_or("?"),
            p["operation"].as_str().unwrap_or("?")
        );
        println!("{}", p["text"].as_str().unwrap_or(""));
    }
    for (l, r) in &skipped {
        eprintln!("links apply: skipped {}: {}", l, r);
    }
    for (l, r) in &refused {
        eprintln!("links apply: refused {}: {}", l, r);
    }
    if printed.is_empty() && refused.is_empty() {
        eprintln!("links apply: nothing to do — no confirmed, included links entry left to apply");
    }
    code
}

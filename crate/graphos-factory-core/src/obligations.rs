//! `source-coverage` — for one operation, every request-body and response
//! path the source offers (path, query and header parameters are not
//! classified, ADR 0037), classified against what the schema actually does with it
//! (docs/plan.md, "spans obligations"): it answers "what did the source
//! offer that the schema never mentions." `closure.rs` supplies the
//! schema's field and argument types it reads.
//!
//! Two independent sections, **response** and **request**, each with its
//! own leaf rows. A leaf's `Class` is exactly one of:
//! - `Mapped` (json: bool) — reachable through the connector's own
//!   `selection:` (response) or `http.body`/argument forwarding (request);
//!   `json: true` when the connector exposes the whole subtree as one JSON
//!   scalar rather than a per-field selection.
//! - `OmittedDecided` — a decision's structured `omits` array
//!   (`decisions.json`, ADR 0036) names this operation, this direction, and
//!   this path or a parent of it. Carries the entry's own `reason`
//!   (`editorial` or `consumed`).
//! - `Consumed` (response only) — no decision names this path, but the
//!   operation's effective `isSuccess`/`errors` configuration reads it. The
//!   envelope pattern is a property of the connector mechanism, not of one
//!   service, so it is accounted for by itself: it needs no `omits` entry.
//!   An existing `reason: consumed` entry still wins and reports as
//!   `OmittedDecided { reason: Consumed }` (reviewed) instead — the two
//!   labels differ only in whether a decision has actually looked at this
//!   operation, not in whether the field is fine to leave unmapped.
//! - `Unaccounted` — offered, and none of the above.
//! - `Unresolved(detail)` — the walk could not tell: a depth-limit marker
//!   baked into `inventory.json`, a source construct `openapi.rs` silently
//!   drops (`prefixItems`/`const`/`discriminator.mapping`), or an
//!   expression (selection method, consumed-config method, request body
//!   expression) outside the small grammar this tool actually parses.
//!   Never counted as `unaccounted` (an unresolved field might really be
//!   covered) and never counted as `mapped` (it might really not be) —
//!   `--check` ignores it entirely, per CLAUDE.md's "skipped is not passed."
//!
//! `structurally_consumed` (response only) is the informational fact behind
//! `Class::Consumed`/`OmittedDecided { reason: Consumed }`: this exact path
//! is read by the operation's effective `isSuccess`/`errors` configuration.
//! It is reported on every row regardless of `Class`, including `Mapped`
//! (a field can be both selected and read by the envelope check).

use crate::closure::{self, Schema};
use crate::json::{get, get_arr, get_obj, get_str, Object};
use crate::op_match::OpHints;
use crate::reconcile::{self, FieldSpan, Node};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Request,
    Response,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Direction::Request => "request",
            Direction::Response => "response",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Editorial,
    Consumed,
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Reason::Editorial => "editorial",
            Reason::Consumed => "consumed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    Mapped {
        json: bool,
    },
    Consumed,
    OmittedDecided {
        reason: Reason,
    },
    Unaccounted,
    Unresolved(String),
    /// An expansion boundary whose default projection is not verified
    /// (ADR 0047): one row for the relationship, its children not
    /// enumerated. A fact about the source, so neither a mapping nor an
    /// omission clears it; only a verified default does.
    UnverifiedDefault {
        target: String,
    },
    /// The schema maps a child of an expansion boundary that the outgoing
    /// request does not ask for — beyond the verified default of an
    /// unexpanded boundary, or outside an expansion's group (ADR 0047): the
    /// source never sends it, so the field would resolve null. Not an
    /// offered path; a finding about the request.
    TransportExpansionMissing {
        boundary: String,
        sent: Vec<String>,
    },
}

impl Class {
    pub fn label(&self) -> String {
        match self {
            Class::Mapped { json: false } => "mapped".to_string(),
            Class::Mapped { json: true } => "mapped (json)".to_string(),
            Class::Consumed => "consumed".to_string(),
            Class::OmittedDecided { reason } => format!("omitted-decided ({})", reason.label()),
            Class::Unaccounted => "unaccounted".to_string(),
            Class::Unresolved(detail) => format!("unresolved ({})", detail),
            Class::UnverifiedDefault { target } => format!(
                "unverified-default ({}: default projection not verified)",
                target
            ),
            Class::TransportExpansionMissing { boundary, sent } => format!(
                "transport-expansion-missing (the request does not ask for this child of `{}`; the source sends {})",
                boundary,
                sent.join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub path: String,
    pub direction: Direction,
    pub class: Class,
    /// Response only: this exact path is read by the operation's effective
    /// `isSuccess`/`errors` configuration, independent of whether a
    /// decision has reviewed this operation for it (see module doc).
    pub structurally_consumed: bool,
    /// Why an `unaccounted` row is not omitted although an `omits` entry
    /// covers it: the entry's decision is open or superseded, so it does not
    /// count (only a resolved decision's omits do).
    pub note: Option<String>,
}

#[derive(Debug, Default)]
pub struct Counts {
    pub offered: usize,
    pub mapped: usize,
    pub consumed: usize,
    pub omitted: usize,
    pub unaccounted: usize,
    pub unresolved: usize,
    pub unverified_default: usize,
    pub transport_expansion_missing: usize,
    /// Resolved `omits` entries on this operation and direction that no
    /// offered row needs any more (ADR 0103): not rows, so not in `offered`.
    pub stale_omit: usize,
}

fn tally(rows: &[Row]) -> Counts {
    let mut c = Counts::default();
    for r in rows {
        if let Class::TransportExpansionMissing { .. } = r.class {
            c.transport_expansion_missing += 1;
            continue;
        }
        c.offered += 1;
        if r.structurally_consumed {
            c.consumed += 1;
        }
        match &r.class {
            Class::Mapped { .. } => c.mapped += 1,
            Class::Consumed => {}
            Class::OmittedDecided { .. } => c.omitted += 1,
            Class::Unaccounted => c.unaccounted += 1,
            Class::Unresolved(_) => c.unresolved += 1,
            Class::UnverifiedDefault { .. } => c.unverified_default += 1,
            Class::TransportExpansionMissing { .. } => {}
        }
    }
    c
}

pub enum DroppedStatus {
    Ran,
    Skipped(&'static str),
}

/// How the schema keeps one behaviour fact: a sentence the source states
/// about what omitting an optional argument does (ADR 0066, 0095).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BehaviourClass {
    /// The argument's doc comment carries the sentence.
    Documented,
    /// A resolved decision records why the doc comment does not carry it.
    Waived { reason: String, decision: String },
    /// The doc comment leaves it out and nothing records why.
    Unaccounted,
}

impl BehaviourClass {
    pub fn label(&self) -> String {
        match self {
            BehaviourClass::Documented => "documented".to_string(),
            BehaviourClass::Waived { reason, decision } => {
                format!("waived ({}, {})", reason, decision)
            }
            BehaviourClass::Unaccounted => "unaccounted".to_string(),
        }
    }
}

/// One behaviour fact of an operation: an optional argument whose source
/// says what omitting it does.
#[derive(Debug, Clone)]
pub struct BehaviourRow {
    /// The source the argument reaches: `query:x`, `body:a.b`, ... — what a
    /// decision waives it by.
    pub path: String,
    /// The root field and the argument it is carried on: `Mutation.pay(paymentCardId)`.
    pub carrier: String,
    /// The source's sentence.
    pub sentence: String,
    pub class: BehaviourClass,
    /// Why an `unaccounted` row is not waived although a waiver covers it.
    pub note: Option<String>,
}

#[derive(Debug, Default)]
pub struct BehaviourCounts {
    pub offered: usize,
    pub documented: usize,
    pub waived: usize,
    pub unaccounted: usize,
}

pub struct Report {
    pub op_key: String,
    pub response: Vec<Row>,
    pub request: Vec<Row>,
    /// Facts about what omitting an argument does (ADR 0095). Empty for an
    /// operation whose source states none, so the report of such an
    /// operation is unchanged.
    pub behaviour: Vec<BehaviourRow>,
    pub dropped_status: DroppedStatus,
    /// Resolved `omits` entries on this operation that no row needs (ADR
    /// 0103), both directions, in decisions.json order.
    pub stale: Vec<StaleOmit>,
}

impl Report {
    pub fn response_counts(&self) -> Counts {
        let mut c = tally(&self.response);
        c.stale_omit = self.stale_in(Direction::Response).count();
        c
    }
    pub fn request_counts(&self) -> Counts {
        let mut c = tally(&self.request);
        c.stale_omit = self.stale_in(Direction::Request).count();
        c
    }
    pub fn stale_in(&self, direction: Direction) -> impl Iterator<Item = &StaleOmit> {
        self.stale.iter().filter(move |s| s.direction == direction)
    }
    pub fn behaviour_counts(&self) -> BehaviourCounts {
        let mut c = BehaviourCounts::default();
        for r in &self.behaviour {
            c.offered += 1;
            match r.class {
                BehaviourClass::Documented => c.documented += 1,
                BehaviourClass::Waived { .. } => c.waived += 1,
                BehaviourClass::Unaccounted => c.unaccounted += 1,
            }
        }
        c
    }
    pub fn unaccounted_total(&self) -> usize {
        self.response_counts().unaccounted + self.request_counts().unaccounted
    }
    /// Why `source-coverage --check` fails, or `None` when it passes. The
    /// bar is zero `unaccounted`, zero `unresolved`, zero
    /// `unverified-default` **and** zero `transport-expansion-missing` in
    /// both directions: a path the classifier could not resolve is not known
    /// to be handled, a boundary whose default projection is unverified
    /// leaves its offered paths unknown, and a mapped boundary child the
    /// request never asks for resolves null, so all three fail closed rather
    /// than passing silently.
    pub fn check_failure(&self) -> Option<String> {
        let mut parts = Vec::new();
        for (direction, c) in [
            ("response", self.response_counts()),
            ("request", self.request_counts()),
        ] {
            if c.unaccounted > 0 {
                parts.push(format!("{} unaccounted {}", direction, c.unaccounted));
            }
            if c.unresolved > 0 {
                parts.push(format!("{} unresolved {}", direction, c.unresolved));
            }
            if c.unverified_default > 0 {
                parts.push(format!(
                    "{} unverified-default {}",
                    direction, c.unverified_default
                ));
            }
            if c.transport_expansion_missing > 0 {
                parts.push(format!(
                    "{} transport-expansion-missing {}",
                    direction, c.transport_expansion_missing
                ));
            }
        }
        let behaviour = self.behaviour_counts();
        if behaviour.unaccounted > 0 {
            parts.push(format!("behaviour unaccounted {}", behaviour.unaccounted));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

// ─── Decisions: structured `Omits:` blocks ───────────────────────────────────

#[derive(Debug, Clone)]
pub struct OmitEntry {
    /// The decision record this entry sits on, and that record's status.
    /// Only a `resolved` record's entries classify a path as omitted.
    pub decision: String,
    pub status: String,
    pub operation: String,
    pub direction: Direction,
    pub path: String,
    pub reason: Reason,
}

/// Every structured `omits` entry across every decision record in a loaded
/// `decisions.json` document (`crate::decisions::load`, ADR 0036) — reads
/// the schema-governed field only, no `decisions.md` fallback, matching
/// origin's own migration stance for every other field. A record missing
/// `omits`, or with an empty array, contributes nothing; an entry the
/// schema itself would have already refused on write never appears here in
/// the first place — this is a read of an already-validated document, not a
/// second validation pass.
pub fn omits_from_doc(doc: &Value) -> Vec<OmitEntry> {
    let mut out = Vec::new();
    let Some(decisions) = get_arr(doc, "decisions") else {
        return out;
    };
    for rec in decisions {
        let Some(entries) = get_arr(rec, "omits") else {
            continue;
        };
        for e in entries {
            let (Some(operation), Some(path)) = (get_str(e, "operation"), get_str(e, "path"))
            else {
                continue;
            };
            let direction = match get_str(e, "direction") {
                Some("request") => Direction::Request,
                // A behaviour waiver is not a wire omission (ADR 0095): it
                // is read by `behaviour_waivers` and never covers a path.
                Some("behaviour") => continue,
                _ => Direction::Response,
            };
            let reason = match get_str(e, "reason") {
                Some("consumed") => Reason::Consumed,
                _ => Reason::Editorial,
            };
            out.push(OmitEntry {
                decision: get_str(rec, "id").unwrap_or("").to_string(),
                status: get_str(rec, "status").unwrap_or("").to_string(),
                operation: operation.to_string(),
                direction,
                path: path.to_string(),
                reason,
            });
        }
    }
    out
}

/// One `omits` entry with `direction: behaviour` (ADR 0095): the recorded
/// reason an omission sentence the source states is not carried by the
/// argument's doc comment. `path` is the argument's source, as
/// `query:x`, `path:x`, `header:X-Card` or `body:a.b`.
#[derive(Debug, Clone)]
pub struct BehaviourWaiver {
    pub decision: String,
    pub status: String,
    pub operation: String,
    pub path: String,
    pub reason: String,
}

/// Every behaviour waiver across every decision record in a loaded
/// `decisions.json`. Only a resolved decision's count, as for a wire omit.
pub fn behaviour_waivers(doc: &Value) -> Vec<BehaviourWaiver> {
    let mut out = Vec::new();
    for rec in get_arr(doc, "decisions").into_iter().flatten() {
        for e in get_arr(rec, "omits").into_iter().flatten() {
            if get_str(e, "direction") != Some("behaviour") {
                continue;
            }
            let (Some(operation), Some(path)) = (get_str(e, "operation"), get_str(e, "path"))
            else {
                continue;
            };
            out.push(BehaviourWaiver {
                decision: get_str(rec, "id").unwrap_or("").to_string(),
                status: get_str(rec, "status").unwrap_or("").to_string(),
                operation: operation.to_string(),
                path: path.to_string(),
                reason: get_str(e, "reason").unwrap_or("").to_string(),
            });
        }
    }
    out
}

/// Does an `Omits:` entry cover `path` for this operation and direction? A
/// parent path covers its own children (`results.openings[]` covers
/// `results.openings[].id`) — never by type name, never by prose, only this
/// structured, exact match.
fn omit_covers(entry: &OmitEntry, op_key: &str, direction: Direction, path: &str) -> bool {
    entry.operation == op_key
        && entry.direction == direction
        // `.` is the root path: the whole direction, including the empty
        // root row an EmptyResponse 204 offers (`""`, which the decisions
        // schema cannot hold since a path must match `\S`).
        && (entry.path == ROOT_OMIT
            || path == entry.path
            || path.starts_with(&format!("{}.", entry.path))
            || path.starts_with(&format!("{}[", entry.path)))
}

/// The `omits` spelling of the root path (ADR 0036, amended in review).
pub const ROOT_OMIT: &str = ".";

pub(crate) fn find_omit<'a>(
    omits: &'a [OmitEntry],
    op_key: &str,
    direction: Direction,
    path: &str,
) -> Option<&'a OmitEntry> {
    omits
        .iter()
        .find(|e| e.status == "resolved" && omit_covers(e, op_key, direction, path))
}

/// An `omits` entry that covers `path` but sits on an open or superseded
/// decision: it does not count, and the row says why.
fn uncounted_omit_note(
    omits: &[OmitEntry],
    op_key: &str,
    direction: Direction,
    path: &str,
) -> Option<String> {
    omits
        .iter()
        .find(|e| e.status != "resolved" && omit_covers(e, op_key, direction, path))
        .map(|e| {
            format!(
                "omitted by {}, which is {}: only a resolved decision's or a current finding's omits count",
                e.decision,
                if e.status.is_empty() {
                    "unset"
                } else {
                    &e.status
                }
            )
        })
}

// ─── Stale omits (ADR 0103) ──────────────────────────────────────────────────

/// Why a resolved `omits` entry no longer describes the service. A stale
/// entry leaves no offered path unaccounted, so it is not part of the
/// `--check` bar; lint warns on it (`stale-omit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staleness {
    /// Every row the entry covers is mapped by the schema.
    Mapped,
    /// Every row it covers is mapped or read by the operation's effective
    /// `isSuccess`/`errors` configuration, at least one of them read, and
    /// the entry's reason is `editorial`: those rows are accounted for
    /// without an omit (a `reason: consumed` entry is the reviewed form of
    /// the same thing, so it is never stale on this count).
    Consumed,
    /// The operation is in the inventory but offers no row at the path.
    NotOffered,
    /// The inventory has no such operation.
    NoOperation,
}

impl Staleness {
    pub fn label(self) -> &'static str {
        match self {
            Staleness::Mapped => "mapped",
            Staleness::Consumed => "consumed",
            Staleness::NotOffered => "not-offered",
            Staleness::NoOperation => "no-operation",
        }
    }

    fn explain(self) -> &'static str {
        match self {
            Staleness::Mapped => "the schema now maps every path it covers",
            Staleness::Consumed => "every path it covers is now mapped or read by the operation's isSuccess/errors configuration, which accounts for it without an omit",
            Staleness::NotOffered => "the source no longer offers that path for this operation",
            Staleness::NoOperation => "inventory.json has no such operation",
        }
    }
}

/// One resolved `omits` entry that no offered row needs.
#[derive(Debug, Clone)]
pub struct StaleOmit {
    pub decision: String,
    pub operation: String,
    pub direction: Direction,
    pub path: String,
    pub why: Staleness,
}

impl StaleOmit {
    /// What is stale and why, without the fix.
    pub fn describe(&self) -> String {
        format!(
            "{} omits `{}` ({}) on {}, but {}",
            self.decision,
            self.path,
            self.direction.label(),
            self.operation,
            self.why.explain()
        )
    }

    /// How to clear it, through the only writer of the file it sits in.
    pub fn fix(&self) -> String {
        if crate::findings::is_finding_id(&self.decision) {
            format!(
                "supersede it (`graphos-factory-core findings supersede . --id {}`) and record any of its omits that still apply on a new finding (`findings add . --title … --body … --omit …`)",
                self.decision
            )
        } else {
            format!(
                "supersede it (`graphos-factory-core decisions supersede . --id {}`) and record any of its omits that still apply: an editorial one on a new decision (`decisions add . --question … --choice … --resolved … --omit …`), a consumed or not-applicable one on a finding (`findings add . … --omit …`)",
                self.decision
            )
        }
    }

    /// The file the entry sits in.
    pub fn file(&self) -> &'static str {
        if crate::findings::is_finding_id(&self.decision) {
            crate::findings::file_of(&self.decision)
        } else {
            crate::decisions::file_of(&self.decision)
        }
    }
}

/// Does a row need this entry? A mapped row never does; a row the envelope
/// reads needs an `editorial` entry no more; a row the classifier could not
/// settle (unresolved, unverified default) is counted as needing it, since
/// staleness is only reported when it is known.
fn row_needs(row: &Row, entry: &OmitEntry) -> bool {
    match row.class {
        Class::Mapped { .. } => false,
        Class::OmittedDecided { .. } | Class::Consumed => {
            !(row.structurally_consumed && entry.reason == Reason::Editorial)
        }
        _ => true,
    }
}

/// The resolved entries on `op_key` and `direction` that no row needs.
fn stale_for(
    omits: &[OmitEntry],
    op_key: &str,
    direction: Direction,
    rows: &[Row],
) -> Vec<StaleOmit> {
    let mut out = Vec::new();
    for e in omits
        .iter()
        .filter(|e| e.status == "resolved" && e.operation == op_key && e.direction == direction)
    {
        let covered: Vec<&Row> = rows
            .iter()
            .filter(|r| !matches!(r.class, Class::TransportExpansionMissing { .. }))
            .filter(|r| omit_covers(e, op_key, direction, &r.path))
            .collect();
        let why = if covered.is_empty() {
            Staleness::NotOffered
        } else if covered.iter().any(|r| row_needs(r, e)) {
            continue;
        } else if covered
            .iter()
            .all(|r| matches!(r.class, Class::Mapped { .. }))
        {
            Staleness::Mapped
        } else {
            Staleness::Consumed
        };
        out.push(StaleOmit {
            decision: e.decision.clone(),
            operation: e.operation.clone(),
            direction,
            path: e.path.clone(),
            why,
        });
    }
    out
}

/// Every stale resolved `omits` entry in the workspace, whichever operation
/// it names — selected or not, in the inventory or not — in decisions.json
/// order. An operation whose report cannot be built contributes nothing
/// (its entries are not known to be stale), and neither does a
/// decisions.json that does not load.
pub fn stale_omits(prepared: &Prepared) -> Vec<StaleOmit> {
    let Ok((_, omits)) = &prepared.decisions else {
        return Vec::new();
    };
    let in_inventory: BTreeSet<&str> = prepared
        .loaded
        .inventory
        .as_ref()
        .and_then(|inv| get_arr(inv, "operations"))
        .into_iter()
        .flatten()
        .filter_map(|o| get_str(o, "key"))
        .collect();
    let mut reports: BTreeMap<String, Vec<StaleOmit>> = BTreeMap::new();
    let mut out = Vec::new();
    for e in omits.iter().filter(|e| e.status == "resolved") {
        if !in_inventory.contains(e.operation.as_str()) {
            out.push(StaleOmit {
                decision: e.decision.clone(),
                operation: e.operation.clone(),
                direction: e.direction,
                path: e.path.clone(),
                why: Staleness::NoOperation,
            });
            continue;
        }
        let stale = reports.entry(e.operation.clone()).or_insert_with(|| {
            build_prepared(prepared, &e.operation)
                .map(|r| r.stale)
                .unwrap_or_default()
        });
        if let Some(s) = stale
            .iter()
            .find(|s| s.decision == e.decision && s.direction == e.direction && s.path == e.path)
        {
            out.push(s.clone());
        }
    }
    out
}

// ─── Shape classification (shared by the offered walk and both mapped walks) ─

/// What one wire shape node resolves to, for the purposes of this walker.
/// Never more than one variant per node — see `classify`'s own priority
/// rules for a mixed `oneOf`/`anyOf`.
enum Kind<'a> {
    /// A construct `openapi.rs` silently drops was found on the named shape
    /// this position resolves to (the keyword named).
    Dropped(String),
    /// `inventory.json`'s own depth-limit marker.
    DepthLimit,
    /// A named shape already open earlier on this same walk path — stop
    /// rather than recurse forever on a self-referential type.
    Recursive,
    /// Every non-null variant's own object `properties`, unioned by name.
    /// `also_leaf` is true when at least one non-null variant is *not*
    /// object-shaped (a mixed union, e.g. `boolean | {..} | null`) — the
    /// position is then reported both as its own leaf and via its children.
    Object {
        props: BTreeMap<String, &'a Value>,
        also_leaf: bool,
    },
    /// An array with a known item shape.
    Array(&'a Value),
    /// An array whose converted shape carries no `items` at all (an
    /// unresolved item shape, not itself a dropped-construct case).
    BareArray,
    /// A dictionary (`additionalProperties`, no `properties`) — reported as
    /// one leaf, its value type not decomposed (`obligations`' own rule:
    /// "dictionaries ... one path each").
    Dict(&'a Value),
    /// A plain scalar, enum, or a union of only scalar/null variants.
    Scalar,
}

/// `v` is, on its own (not decomposed through a `oneOf`/`anyOf`), an object
/// type with nothing of its own to offer: `properties` absent (a vendor's
/// `AssessmentListRequest`) or present but empty (its
/// `ApiKeyInfoRequest`, `{"properties": {}}`) — both spellings of "no
/// properties" — and not a dictionary (`is_dict` already requires
/// `additionalProperties`).
///
/// Only meaningful at the offered-root (`offered_rows` uses it to match the
/// existing null-root short-circuit: an empty request/response body sends
/// nothing, the same as no body at all). Nested — reached through a
/// property — this same shape is a value the wire actually sends, to be
/// mapped or omitted with a reason like any other leaf (Asana's
/// `BatchResponse.body`/`.headers`, an intentional "arbitrary JSON"
/// passthrough idiom, selected and mapped); `classify`/`classify_resolved`
/// deliberately does not special-case it, so it still classifies as
/// `Kind::Scalar` and contributes its one opaque leaf, exactly as before
/// this shape was ever distinguished.
fn is_bare_empty_object(v: &Value) -> bool {
    let is_object_type = get_str(v, "type") == Some("object")
        || get_arr(v, "type")
            .map(|t| t.iter().any(|x| x.as_str() == Some("object")))
            .unwrap_or(false);
    let no_properties = get_obj(v, "properties")
        .map(|p| p.is_empty())
        .unwrap_or(true);
    is_object_type && no_properties && !is_dict(v) && !has_array_type(v)
}

const DEPTH_LIMIT_TEXT: &str = "depth limit reached during inventory conversion";

fn is_depth_limit_marker(v: &Value) -> bool {
    v.as_object()
        .map(|o| {
            o.len() == 1 && o.get("description").and_then(Value::as_str) == Some(DEPTH_LIMIT_TEXT)
        })
        .unwrap_or(false)
}

fn immediate_ref_name(v: &Value) -> Option<&str> {
    v.as_object()
        .and_then(|o| o.get("$ref"))
        .and_then(Value::as_str)
        .map(crate::inventory::shape_name)
}

/// Every shape name a cycle guard keys on for `v`: its own `$ref`
/// (`immediate_ref_name`), or, for a `oneOf`/`anyOf`/`allOf` of several
/// references (HubSpot's list filter: an array whose items are a `oneOf`
/// of seven branch shapes, each of which holds that array again), the name
/// of each reference in the list, nested lists included. `collect_variants`
/// folds all the variants into one object, so the walk below it meets a
/// different branch on each lap; a guard on a single name never sees the
/// cycle, and the walk recurses until the stack overflows (ADR 0102).
fn guard_ref_names(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_guard_names(v, &mut out, 0);
    out
}

fn collect_guard_names(v: &Value, out: &mut Vec<String>, depth: usize) {
    if depth > 8 {
        return;
    }
    if let Some(name) = immediate_ref_name(v) {
        out.push(name.to_string());
        return;
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(variants) = get_arr(v, key) {
            for variant in variants {
                if get_str(variant, "type") != Some("null") {
                    collect_guard_names(variant, out, depth + 1);
                }
            }
        }
    }
}

/// Follow a `$ref` chain to its terminal, non-alias value — an
/// inventory.json shape can itself be nothing but a further `$ref` (Asana's
/// `JobResponse -> JobBase -> JobCompact`), not only real content, so one
/// hop is not always enough. Returns the *first* name in the chain
/// (matching `immediate_ref_name` on the caller's own untouched value, the
/// same identity `walk_offered`'s recursion guard keys on) alongside the
/// final resolved value. `None` means the chain itself cycles back to an
/// earlier name before terminating (a pure alias cycle, with no real
/// content anywhere in it) — the caller reports `Kind::Recursive` rather
/// than resolve forever.
fn deref_one<'a>(v: &'a Value, shapes: &'a Object) -> Option<(Option<&'a str>, &'a Value)> {
    let first_name = immediate_ref_name(v);
    let mut current = v;
    let mut chain: Vec<&str> = Vec::new();
    while let Some(name) = immediate_ref_name(current) {
        if chain.contains(&name) {
            return None;
        }
        chain.push(name);
        match shapes.get(name) {
            Some(next) => current = next,
            None => break,
        }
    }
    Some((first_name, current))
}

fn has_array_type(v: &Value) -> bool {
    get_str(v, "type") == Some("array")
        || get_arr(v, "type")
            .map(|t| t.iter().any(|x| x.as_str() == Some("array")))
            .unwrap_or(false)
        || get(v, "items").is_some()
}

fn is_dict(v: &Value) -> bool {
    get_obj(v, "properties").is_none()
        && match get(v, "additionalProperties") {
            Some(Value::Bool(true)) => true,
            Some(Value::Object(_)) => true,
            _ => false,
        }
}

/// Classify `shape` (a raw, possibly-`$ref` property value) against the
/// dropped-construct check (`dropped`, if the workspace has a usable source
/// document) and the shape it resolves to. `seen` guards a self-referential
/// named shape on this same walk branch; it is a `Vec`, not a `HashSet`,
/// because a node is pushed/popped around its own recursive calls only —
/// order never matters, membership does.
fn classify<'a>(
    shape: &'a Value,
    shapes: &'a Object,
    dropped: &DroppedConstructs,
    seen: &mut Vec<String>,
) -> Kind<'a> {
    if let Some(name) = immediate_ref_name(shape) {
        if let Some(keyword) = dropped.check(name) {
            return Kind::Dropped(keyword);
        }
    }
    let Some((name, resolved)) = deref_one(shape, shapes) else {
        return Kind::Recursive;
    };
    if is_depth_limit_marker(resolved) {
        return Kind::DepthLimit;
    }
    match name {
        Some(n) => {
            if seen.contains(&n.to_string()) {
                return Kind::Recursive;
            }
        }
        None => {
            // Recursive once every reference the position can be is already
            // open on this branch. A `oneOf` of several branch shapes
            // qualifies only when all of them are open; a lone reference
            // (or nullable wrapper) is the single-name case as before.
            let names = guard_ref_names(shape);
            if !names.is_empty() && names.iter().all(|n| seen.contains(n)) {
                return Kind::Recursive;
            }
        }
    }
    classify_resolved(resolved, shapes, dropped, seen, name)
}

fn classify_resolved<'a>(
    resolved: &'a Value,
    shapes: &'a Object,
    dropped: &DroppedConstructs,
    seen: &mut Vec<String>,
    self_name: Option<&str>,
) -> Kind<'a> {
    if let Some(n) = self_name {
        seen.push(n.to_string());
    }
    let mut props: BTreeMap<String, &Value> = BTreeMap::new();
    let mut array_items: Option<&Value> = None;
    let mut bare_array = false;
    let mut dict_value: Option<&Value> = None;
    let mut has_other = false;
    collect_variants(
        resolved,
        shapes,
        dropped,
        seen,
        &mut props,
        &mut array_items,
        &mut bare_array,
        &mut dict_value,
        &mut has_other,
    );
    if let Some(n) = self_name {
        seen.pop();
        let _ = n;
    }
    if !props.is_empty() {
        let also_leaf = has_other || array_items.is_some() || bare_array || dict_value.is_some();
        return Kind::Object { props, also_leaf };
    }
    if let Some(items) = array_items {
        return Kind::Array(items);
    }
    if bare_array {
        return Kind::BareArray;
    }
    if let Some(dv) = dict_value {
        return Kind::Dict(dv);
    }
    Kind::Scalar
}

/// One non-null variant's own contribution, folded into the caller's
/// accumulators. Recurses through `oneOf`/`anyOf` (a variant's own `$ref`
/// chain is resolved by the same `deref_one` as everywhere else, but with
/// no dropped-construct/depth re-check and no *outer* cycle check against
/// `seen` — matching the plan's own stated partial coverage: "only applies
/// ... at that path", not to every nested variant reference used purely for
/// shape classification. A chain-internal cycle within one variant's own
/// `$ref` is still caught, by `deref_one` itself, and treated as an inert
/// "other" contribution rather than resolved forever.
#[allow(clippy::too_many_arguments)]
fn collect_variants<'a>(
    v: &'a Value,
    shapes: &'a Object,
    dropped: &DroppedConstructs,
    seen: &mut Vec<String>,
    props: &mut BTreeMap<String, &'a Value>,
    array_items: &mut Option<&'a Value>,
    bare_array: &mut bool,
    dict_value: &mut Option<&'a Value>,
    has_other: &mut bool,
) {
    let Some((_, v)) = deref_one(v, shapes) else {
        *has_other = true;
        return;
    };
    if get_str(v, "type") == Some("null") {
        return;
    }
    if let Some(p) = get_obj(v, "properties") {
        for (k, val) in p {
            props.entry(k.clone()).or_insert(val);
        }
        return;
    }
    if has_array_type(v) {
        match get(v, "items") {
            Some(items) => {
                if array_items.is_none() {
                    *array_items = Some(items);
                }
            }
            None => *bare_array = true,
        }
        return;
    }
    if is_dict(v) {
        if dict_value.is_none() {
            *dict_value = get(v, "additionalProperties");
        }
        return;
    }
    if let Some(variants) = get_arr(v, "oneOf").or_else(|| get_arr(v, "anyOf")) {
        for variant in variants {
            collect_variants(
                variant,
                shapes,
                dropped,
                seen,
                props,
                array_items,
                bare_array,
                dict_value,
                has_other,
            );
        }
        return;
    }
    *has_other = true;
}

// ─── Dropped-construct detection (step 2, "Dropped constructs") ─────────────

pub struct DroppedConstructs {
    /// Inventory shape name -> the source document's own schema at the
    /// exact normalised reference `Built.shape_names` recorded for it. Only
    /// populated when a pinned, present-on-disk openapi/swagger document
    /// exists; `check` always answers "no" when this is empty, and the
    /// workspace-level status line says why.
    by_name: BTreeMap<String, Value>,
    pub status: &'static str,
}

impl DroppedConstructs {
    /// The keyword found dropped for this inventory shape name, if any:
    /// `prefixItems` with no sibling `items`, `const`, or
    /// `discriminator.mapping` on the source schema, with no trace of it in
    /// the already-converted inventory shape.
    fn check(&self, shape_name: &str) -> Option<String> {
        let source = self.by_name.get(shape_name)?;
        if get(source, "prefixItems").is_some() && get(source, "items").is_none() {
            return Some("prefixItems".to_string());
        }
        if get(source, "const").is_some() {
            return Some("const".to_string());
        }
        if get(source, "discriminator")
            .and_then(|d| get(d, "mapping"))
            .is_some()
        {
            return Some("discriminator.mapping".to_string());
        }
        None
    }
}

/// A minimal JSON-pointer walk (`#/a/b/0/c`) against an already-loaded
/// document — the source document, resolved by exact reference, never a
/// same-name guess (`ShapeSet::name_for`'s own collision suffixing means a
/// shape name and a component name are not always the same string).
fn pointer_lookup<'a>(doc: &'a Value, reference: &str) -> Option<&'a Value> {
    let rel = reference.strip_prefix("#/")?;
    let mut node = doc;
    for raw in rel.split('/') {
        let part = raw.replace("~1", "/").replace("~0", "~");
        node = match node {
            Value::Object(o) => o.get(&part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(node)
}

/// Build the dropped-construct checker for `dir`: find the workspace's
/// pinned, present-on-disk openapi/swagger document (the same selection
/// `validate.rs`'s `oracle_label` already applies), read it the *correct*
/// way (`spec::read`, which runs Swagger normalisation — never
/// `sources::load_document`, which does not), invert `Built.shape_names`
/// (reference -> inventory name becomes name -> reference), and resolve
/// each reference against the *normalised* document `shape_names` was
/// itself built from. No source document (or none readable) is not an
/// error: the sub-check is skipped for the whole workspace, reported as a
/// workspace-level fact, never folded into a pass.
fn build_dropped_constructs(dir: &Path) -> DroppedConstructs {
    let lock = match crate::sources::read_sources_lock(dir) {
        Ok(Some(l)) => l,
        Ok(None) => {
            return DroppedConstructs {
                by_name: BTreeMap::new(),
                status: "skipped (no sources.lock.yaml)",
            }
        }
        // Present but refused (a symlink) or unparsable is not the same fact
        // as absent, and says so.
        Err(_) => {
            return DroppedConstructs {
                by_name: BTreeMap::new(),
                status: "skipped (sources.lock.yaml unreadable)",
            }
        }
    };
    let entries = crate::sources::document_entries(&lock);
    let pinned = entries
        .iter()
        .filter(|e| crate::sources::not_followed(e, &entries).is_none())
        .find(|e| workspace_file_exists(dir, &e.path));
    let Some(entry) = pinned else {
        return DroppedConstructs {
            by_name: BTreeMap::new(),
            status: "skipped (no openapi/swagger source document)",
        };
    };
    let text = match read_workspace_text(dir, &entry.path) {
        Ok(t) => t,
        Err(_) => {
            return DroppedConstructs {
                by_name: BTreeMap::new(),
                status: "skipped (source document unreadable)",
            }
        }
    };
    let loaded = match crate::spec::read(&text, &entry.path) {
        Ok(l) => l,
        Err(_) => {
            return DroppedConstructs {
                by_name: BTreeMap::new(),
                status: "skipped (source document did not parse)",
            }
        }
    };
    let built = match crate::openapi::build_inventory(&loaded.document) {
        Ok(b) => b,
        Err(_) => {
            return DroppedConstructs {
                by_name: BTreeMap::new(),
                status: "skipped (source document did not convert)",
            }
        }
    };
    let mut by_name = BTreeMap::new();
    for (reference, name) in &built.shape_names {
        if let Some(schema) = pointer_lookup(&loaded.document, reference) {
            by_name.insert(name.clone(), schema.clone());
        }
    }
    DroppedConstructs {
        by_name,
        status: "ran",
    }
}

// ─── Offered (shared walk rules, both directions) ────────────────────────────

fn push_child_path(prefix: &str, child: &str) -> String {
    if prefix.is_empty() {
        child.to_string()
    } else {
        format!("{}.{}", prefix, child)
    }
}

/// Expansion boundaries met by a response walk (ADR 0047). `None` on the
/// request side, which ignores `x-expansion`: a request body is sent, not
/// projected.
#[derive(Default)]
struct Boundaries {
    /// Path of each boundary whose default is unverified -> its target.
    unverified: BTreeMap<String, String>,
    /// The outgoing fields expression's groups, by entity-relative path
    /// (`business`, `business.primary_page`): what the wire asks for at
    /// each expanded boundary.
    expanded: BTreeMap<String, Vec<String>>,
    /// Where the entity sits in the response (`data` on a paged edge).
    entity_prefix: Option<String>,
    /// What the source sends at each boundary whose answer is known: the
    /// requested names where the wire expands it, the verified default
    /// leaves where it does not. Path -> names.
    sends: BTreeMap<String, Vec<String>>,
}

impl Boundaries {
    fn for_wire(expression: Option<&str>, entity_prefix: Option<&str>) -> Boundaries {
        Boundaries {
            unverified: BTreeMap::new(),
            sends: BTreeMap::new(),
            expanded: expression
                .map(crate::sparse::expansion_groups)
                .unwrap_or_default(),
            entity_prefix: entity_prefix.map(str::to_string),
        }
    }

    /// The fields expression's group for the boundary at walk path `path`
    /// (`data[].labels[]` on a paged edge is the entity's `labels`).
    fn group(&self, path: &str) -> Option<&Vec<String>> {
        let plain = path.replace("[]", "");
        let rel = match &self.entity_prefix {
            Some(p) => plain.strip_prefix(&format!("{}.", p))?,
            None => plain.as_str(),
        };
        self.expanded.get(rel)
    }
}

/// A response walk that meets `x-expansion` stops there, unless the
/// outgoing fields expression expands it. Unexpanded, a verified default
/// offers exactly its leaves in the source's shape (`owner.id`); an
/// unverified one offers one row for the relationship. Expanded
/// (`business{id,name}`), it offers what the expression requests, each
/// requested child walked by the ordinary rules — so an embedded value
/// comes back whole and a nested boundary stops at its own default. That
/// path is finite because the expression is, so the target itself is not
/// checked against the on-branch cycle guard. An annotation on an array
/// property applies to its items (`labels[]`). Returns false when `shape`
/// is not a boundary.
#[allow(clippy::too_many_arguments)]
fn walk_boundary(
    path: &str,
    shape: &Value,
    shapes: &Object,
    dropped: &DroppedConstructs,
    seen: &mut Vec<String>,
    out: &mut Vec<(String, Option<String>)>,
    subtrees: &mut BTreeSet<String>,
    bounds: &mut Boundaries,
) -> bool {
    let Some(x) = get(shape, "x-expansion") else {
        return false;
    };
    // The annotation may sit beside a `$ref` to a named list shape
    // (`LabelList`), so the reference is resolved before the array check,
    // as `walk_selection` resolves it when it writes `labels[].id`.
    let resolved = deref_one(shape, shapes).map_or(shape, |(_, r)| r);
    let (path, reference) = match [shape, resolved].into_iter().find(|s| has_array_type(s)) {
        Some(list) => (format!("{}[]", path), get(list, "items").unwrap_or(list)),
        None => (path.to_string(), shape),
    };
    let target = get_str(x, "target").unwrap_or("?").to_string();
    if let Some(requested) = bounds.group(&path).cloned() {
        bounds.sends.insert(path.clone(), requested.clone());
        let props =
            deref_one(reference, shapes).and_then(|(_, resolved)| get_obj(resolved, "properties"));
        for name in requested {
            let child_path = push_child_path(&path, &name);
            match props.and_then(|p| p.get(&name)) {
                Some(child) => walk_offered(
                    &child_path,
                    child,
                    shapes,
                    dropped,
                    seen,
                    out,
                    subtrees,
                    Some(bounds),
                ),
                None => out.push((
                    child_path,
                    Some(format!(
                        "the fields expression requests `{}`, which {} does not document",
                        name, target
                    )),
                )),
            }
        }
        return true;
    }
    match crate::sparse::verified_default(x) {
        Some(leaves) => {
            for leaf in &leaves {
                out.push((push_child_path(&path, leaf), None));
            }
            bounds.sends.insert(path, leaves);
        }
        None => {
            out.push((path.clone(), None));
            bounds.unverified.insert(path, target);
        }
    }
    true
}

/// A mapped path beneath a boundary, naming a child the source does not send
/// there — past the verified default of an unexpanded boundary
/// (`business.name` where the source sends only `business.id` unless asked),
/// or outside an expansion's group — is a child the request never asks for
/// (ADR 0047). One row per such path,
/// on every method. An operation that `can_ask` (it takes the sparse
/// parameter and the field sends it) fixes the row by putting the group in
/// its fields expression, so no decision clears it. One with no way to ask
/// (any POST, a GET that does not send the parameter) keeps the row unless a
/// resolved decision naming the operation records the path, as a backtick
/// span, as what the source is verified to send (`crate::sparse::decision_for`).
fn transport_expansion_missing<'a>(
    bounds: &Boundaries,
    mapped: impl Iterator<Item = &'a String>,
    decisions: &Value,
    op_key: &str,
    can_ask: bool,
) -> Vec<Row> {
    let mut rows = Vec::new();
    for path in mapped {
        // The deepest boundary above the path decides.
        let Some((boundary, sent)) = bounds
            .sends
            .iter()
            .filter(|(b, _)| path.starts_with(&format!("{}.", b)))
            .max_by_key(|(b, _)| b.len())
        else {
            continue;
        };
        let child = path[boundary.len() + 1..]
            .split(['.', '['])
            .next()
            .unwrap_or("");
        if sent.iter().any(|d| d == child) {
            continue;
        }
        if !can_ask && crate::sparse::decision_for(decisions, op_key, path).is_some() {
            continue;
        }
        rows.push(Row {
            path: path.clone(),
            direction: Direction::Response,
            class: Class::TransportExpansionMissing {
                boundary: boundary.clone(),
                sent: sent.clone(),
            },
            structurally_consumed: false,
            note: None,
        });
    }
    rows
}

/// The wire paths `shape` offers, walked recursively. `seen` starts empty
/// per top-level call (one call per operation per direction).
#[allow(clippy::too_many_arguments)]
fn walk_offered(
    path: &str,
    shape: &Value,
    shapes: &Object,
    dropped: &DroppedConstructs,
    seen: &mut Vec<String>,
    out: &mut Vec<(String, Option<String>)>,
    subtrees: &mut BTreeSet<String>,
    mut bounds: Option<&mut Boundaries>,
) {
    if let Some(b) = bounds.as_deref_mut() {
        if !path.is_empty() && walk_boundary(path, shape, shapes, dropped, seen, out, subtrees, b) {
            return;
        }
    }
    // A named shape must stay on `seen` for as long as its *children* are
    // being walked, not only while `classify` itself decides its kind — the
    // cycle it guards against (`Node.child: Node`) only ever shows up one
    // level down, in the recursive calls below, after `classify` has
    // already returned. `seen` is a per-branch stack: push before
    // descending into this shape's own children, pop once they're done, so
    // a shared, non-cyclical reuse of the same type in a sibling branch
    // (two different fields both typed `User`) is never wrongly suppressed.
    let guard = guard_ref_names(shape);
    let kind = classify(shape, shapes, dropped, seen);
    seen.extend(guard.iter().cloned());
    match kind {
        Kind::Dropped(keyword) => {
            out.push((path.to_string(), Some(format!("dropped: {}", keyword))))
        }
        Kind::DepthLimit => out.push((
            path.to_string(),
            Some("depth limit at inventory build".to_string()),
        )),
        // A self-referential field (`parent: Node` inside `Node`) is on the
        // wire like any other. The walk does not expand it again; it offers
        // one row that stands for the subtree, so a schema that drops it is
        // `unaccounted` rather than invisible.
        // A pure alias cycle at the root (`A -> B -> A`, no object anywhere)
        // has no content on the wire at all and still offers nothing.
        Kind::Recursive if path.is_empty() => {}
        Kind::Recursive => {
            out.push((path.to_string(), None));
            subtrees.insert(path.to_string());
        }
        Kind::Object { props, also_leaf } => {
            if also_leaf {
                out.push((path.to_string(), None));
            }
            for (name, child) in props {
                walk_offered(
                    &push_child_path(path, &name),
                    child,
                    shapes,
                    dropped,
                    seen,
                    out,
                    subtrees,
                    bounds.as_deref_mut(),
                );
            }
        }
        Kind::Array(items) => walk_offered(
            &format!("{}[]", path),
            items,
            shapes,
            dropped,
            seen,
            out,
            subtrees,
            bounds,
        ),
        Kind::BareArray => out.push((format!("{}[]", path), None)),
        Kind::Dict(value) => {
            // The dict's own value type is a resolution point the plan asks
            // the dropped-construct check to cover too (the real
            // `changedFields` case: a map whose value is a dropped tuple).
            let dict_dropped = immediate_ref_name(value).and_then(|name| dropped.check(name));
            match dict_dropped {
                Some(keyword) => {
                    out.push((path.to_string(), Some(format!("dropped: {}", keyword))))
                }
                None => out.push((path.to_string(), None)),
            }
        }
        Kind::Scalar => out.push((path.to_string(), None)),
    }
    for _ in &guard {
        seen.pop();
    }
}

// The request walk reads `offered_rows_full`; the unit tests read rows alone.
#[cfg(test)]
fn offered_rows(
    root_ref: &Value,
    shapes: &Object,
    dropped: &DroppedConstructs,
) -> Vec<(String, Option<String>)> {
    // No `request_body` (a GET) or a response with no declared shape at all
    // offers nothing — never one phantom leaf at the empty root path.
    if root_ref.is_null() {
        return Vec::new();
    }
    // A declared-but-empty body (`{"type":"object"}`, a vendor's
    // `ApiKeyInfoRequest`) bottoms out the same way: an empty root means
    // nothing to send, exactly like no body at all. Root only — the same
    // shape reached through a property is a real value the wire passes
    // through (Asana's `BatchResponse.body`, an "arbitrary JSON" idiom) and
    // must still report its one opaque leaf, so this check never runs
    // inside `walk_offered`'s own recursion.
    if let Some((_, resolved)) = deref_one(root_ref, shapes) {
        if is_bare_empty_object(resolved) {
            return Vec::new();
        }
    }
    offered_rows_full(root_ref, shapes, dropped, None).0
}

/// `offered_rows`, plus the paths whose one row stands for a subtree the
/// walk does not expand again (a self-referential field): such a row is
/// mapped when the selection reaches it or anything beneath it.
fn offered_rows_full(
    root_ref: &Value,
    shapes: &Object,
    dropped: &DroppedConstructs,
    bounds: Option<&mut Boundaries>,
) -> (Vec<(String, Option<String>)>, BTreeSet<String>) {
    let mut out = Vec::new();
    let mut subtrees = BTreeSet::new();
    if root_ref.is_null() {
        return (out, subtrees);
    }
    if let Some((_, resolved)) = deref_one(root_ref, shapes) {
        if is_bare_empty_object(resolved) {
            return (out, subtrees);
        }
    }
    let mut seen = Vec::new();
    walk_offered(
        "",
        root_ref,
        shapes,
        dropped,
        &mut seen,
        &mut out,
        &mut subtrees,
        bounds,
    );
    (out, subtrees)
}

/// Is `path` strictly beneath the response path `prefix`? The empty path is
/// the whole response (a bare `$`, ADR 0105): every row is beneath it.
fn beneath_prefix(path: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return !path.is_empty();
    }
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

/// Does any mapped path sit beneath `path`?
fn reaches_beneath<'a>(mut mapped: impl Iterator<Item = &'a String>, path: &str) -> bool {
    let dot = format!("{}.", path);
    let bracket = format!("{}[", path);
    mapped.any(|k| k.starts_with(&dot) || k.starts_with(&bracket))
}

// ─── Response — Mapped (selection walk) ──────────────────────────────────────

/// Everything the operation's `selection:` covers: exact paths it reaches
/// (leaf, no further children in the selection — `json` true when the
/// offered shape there is still object/array/dict-shaped, meaning the
/// selection grabbed the whole subtree as one opaque value) and path
/// prefixes a `->method` the walk does not know stopped extraction under
/// (`unresolved`, reported at that prefix, per the plan's own "a method
/// transform the walk cannot follow marks the subtree unresolved").
struct SelectionCoverage {
    exact: BTreeMap<String, bool>,
    /// Path -> why the walk stopped there, naming the construct.
    unresolved_prefixes: Vec<(String, String)>,
    /// Path of an object with a `...` spread the walk does not parse, and
    /// why (ADR 0058). Unlike a prefix above it leaves the paths the
    /// selection maps mapped: the spread may add any other key.
    open_spreads: Vec<(String, String)>,
}

fn walk_selection(
    path: &str,
    nodes: &[Node],
    shape: &Value,
    shapes: &Object,
    dropped: &DroppedConstructs,
    seen: &mut Vec<String>,
    out: &mut SelectionCoverage,
) {
    for node in nodes {
        // `a: x ?? y` reads `y` as well as `x`, in the same context.
        walk_selection(path, &node.fallbacks, shape, shapes, dropped, seen, out);
        // A `...` spread (ADR 0058): each `->match` arm's fields read from
        // the object the spread sits in, so they walk in this same context;
        // the discriminator is classified below as the `->match` node it is.
        // Any other spread could add any key here, so everything under this
        // path the selection does not map is unresolved.
        match &node.spread {
            Some(reconcile::Spread::Match(arms)) => {
                for arm in arms {
                    walk_selection(path, &arm.children, shape, shapes, dropped, seen, out);
                }
            }
            Some(reconcile::Spread::Unparsed(why)) => {
                out.open_spreads
                    .push((path.to_string(), format!("selection not parsed: {}", why)));
                continue;
            }
            None => {}
        }
        // A rooted node with no path step is a bare `$` (ADR 0105): it
        // reads the value in context itself — at the top, the whole
        // response, so `[String]` over a root array of strings maps `[]`.
        // It takes the walk below with no steps, so a method on it or a
        // sub-selection under it is read exactly as on a named field.
        let segs: &[String] = match node.key.as_deref() {
            Some(s) if !s.is_empty() => s,
            Some(_) if node.rooted => &[],
            _ => continue,
        };
        let mut cur: &Value = shape;
        let mut cur_path = path.to_string();
        let mut lost = false;
        for seg in segs {
            if seg == "*" {
                lost = true;
                break;
            }
            match classify(cur, shapes, dropped, seen) {
                Kind::Object { props, .. } => match props.get(seg.as_str()) {
                    Some(&child) => {
                        cur_path = push_child_path(&cur_path, seg);
                        cur = child;
                    }
                    None => {
                        lost = true;
                        break;
                    }
                },
                Kind::Array(items) => {
                    // Selections address array fields transparently: one
                    // more `properties` step, no explicit `[]` written, but
                    // the wire path still carries it (matching every other
                    // path this tool emits).
                    match classify(items, shapes, dropped, seen) {
                        Kind::Object { props, .. } => match props.get(seg.as_str()) {
                            Some(&child) => {
                                cur_path = format!("{}[].{}", cur_path, seg);
                                cur = child;
                            }
                            None => {
                                lost = true;
                                break;
                            }
                        },
                        _ => {
                            lost = true;
                            break;
                        }
                    }
                }
                _ => {
                    lost = true;
                    break;
                }
            }
        }
        if lost {
            continue;
        }
        // `map->entries` turns a dictionary into `{ key, value }` pairs: it
        // reads the whole map, which offers exactly one row (its keys are
        // data), so the selection covers that row whether or not it goes on
        // to pick `key`/`value` children. Only on a dictionary: `->entries`
        // on an object with fixed properties stays unresolved.
        if node.methods == ["entries"]
            && matches!(classify(cur, shapes, dropped, seen), Kind::Dict(_))
        {
            out.exact.insert(cur_path, false);
            continue;
        }
        // `field->match([from, to], …)` translates the value the field holds
        // (null-preserving stringification, enum renames): the connector
        // still reads exactly `field`, so coverage is the field's own, as
        // if the method were absent. Only on a scalar, or with a
        // sub-selection that is walked: an arm over an object or array
        // (`owner->match([null, null], [@, @.name])`) may send any part of
        // it, so it stays unresolved. A chain (`->match->first`) is not.
        // `field->map(@->match(…))` is the same translation applied to each
        // element of an array of scalars (Granola's enum-array `events`).
        let element_translates = node.methods == ["map"]
            && node.element_match
            && node.children.is_none()
            && match classify(cur, shapes, dropped, seen) {
                Kind::Array(item) => matches!(classify(item, shapes, dropped, seen), Kind::Scalar),
                _ => false,
            };
        let translates = (node.methods == ["match"]
            && (node.children.is_some()
                || matches!(classify(cur, shapes, dropped, seen), Kind::Scalar)))
            || element_translates;
        if node.methods == ["match"] && !translates {
            out.unresolved_prefixes.push((
                cur_path.clone(),
                "selection method not parsed: ->match on a non-scalar value".to_string(),
            ));
            continue;
        }
        if !node.methods.is_empty() && !translates {
            out.unresolved_prefixes
                .push((cur_path.clone(), selection_reason(&node.methods)));
            continue;
        }
        // The parser marks every node with a method opaque; a translated
        // node's sub-selection is still a real one.
        let opaque = node.opaque && !translates;
        match &node.children {
            // A selection that reaches into a dictionary (`labels { a }`)
            // covers its one offered row: the map's keys are data, never
            // decomposed into rows of their own.
            Some(_) if !opaque && matches!(classify(cur, shapes, dropped, seen), Kind::Dict(_)) => {
                out.exact.insert(cur_path, false);
            }
            Some(children) if !opaque => {
                walk_selection(&cur_path, children, cur, shapes, dropped, seen, out);
            }
            _ => {
                let is_json = matches!(
                    classify(cur, shapes, dropped, seen),
                    Kind::Object { .. } | Kind::Array(_) | Kind::BareArray | Kind::Dict(_)
                );
                out.exact.insert(cur_path, is_json);
            }
        }
    }
}

fn response_mapped(
    op: &Value,
    shapes: &Object,
    dropped: &DroppedConstructs,
    selection_text: Option<&str>,
) -> SelectionCoverage {
    let mut out = SelectionCoverage {
        exact: BTreeMap::new(),
        unresolved_prefixes: Vec::new(),
        open_spreads: Vec::new(),
    };
    let Some(text) = selection_text else {
        return out;
    };
    let root_ref = root_shape_ref(op, "response");
    let nodes = reconcile::parse_selection(text);
    let mut seen = Vec::new();
    walk_selection("", &nodes, &root_ref, shapes, dropped, &mut seen, &mut out);
    out
}

/// `Value::Null` when the operation has no `request_body`/`response` shape
/// at all (a GET with no body, on the request side) — never a `{"$ref":
/// null}` object, which `classify` would otherwise treat as a real, if
/// unresolvable, shape and emit one phantom leaf at the empty root path for.
fn root_shape_ref(op: &Value, side: &str) -> Value {
    match get(op, side).and_then(|r| get(r, "shape_ref")).cloned() {
        Some(r) => crate::json::object(vec![("$ref", r)]),
        None => Value::Null,
    }
}

// ─── Response — Consumed (isSuccess/errors, a third, small grammar) ─────────

/// One `key: expr` (or bare top-level string) directive-argument value,
/// found at depth 0 in `text` — the same small scanner shape
/// `reconcile::string_arg` uses, written locally rather than exposing that
/// private helper, since the value here can also be a `{...}` object
/// literal (`errors:` itself), not only a string.
fn find_value(text: &str, key: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < text.len() {
        // Every token this scans for is ASCII: step over the continuation
        // bytes of a multi-byte character rather than slice inside it.
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let c = bytes[i];
        if c == b'"' {
            if text[i..].starts_with("\"\"\"") {
                let rel = text[i + 3..].find("\"\"\"")?;
                i = i + 3 + rel + 3;
            } else {
                i += 1;
                while i < text.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            continue;
        }
        if matches!(c, b'{' | b'(' | b'[') {
            depth += 1;
            i += 1;
            continue;
        }
        if matches!(c, b'}' | b')' | b']') {
            depth -= 1;
            i += 1;
            continue;
        }
        if depth == 0 && text[i..].starts_with(key) {
            let before_ok = i == 0 || {
                let p = bytes[i - 1];
                !(p.is_ascii_alphanumeric() || p == b'_')
            };
            let mut j = i + key.len();
            while j < text.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if before_ok && bytes.get(j) == Some(&b':') {
                let mut k = j + 1;
                while k < text.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                return read_value(&text[k..]);
            }
        }
        i += 1;
    }
    None
}

fn read_value(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix("\"\"\"") {
        let end = rest.find("\"\"\"").unwrap_or(rest.len());
        return Some(rest[..end].to_string());
    }
    if s.starts_with('"') {
        let rest = &s[1..];
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
                continue;
            }
            if c == '"' {
                return Some(out);
            }
            out.push(c);
        }
        return Some(out);
    }
    if let Some(rest) = s.strip_prefix('{') {
        let mut depth = 1i32;
        for (idx, c) in rest.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(rest[..idx].to_string());
                    }
                }
                _ => {}
            }
        }
        return Some(rest.to_string());
    }
    None
}

/// The operation a root field's connector serves, or `None` when it serves
/// none or its path is a tie the selection does not settle (reconcile
/// reports that tie; ADR 0044).
fn served_by<'a>(
    inventory: &'a Value,
    hints: &OpHints,
    root: &str,
    fs: &FieldSpan,
) -> Option<&'a Value> {
    let c = fs.connect.as_ref()?;
    reconcile::match_operation(
        inventory,
        c.method.as_deref(),
        c.path.as_deref(),
        &hints.for_field(root, &fs.name),
    )
    .ok()
    .flatten()
}

/// This operation's own `@connect(...)` field span, if its `method`+`path`
/// resolve to `op_key`.
fn field_span_for(
    sdl: &str,
    inventory: &Value,
    hints: &OpHints,
    op_key: &str,
) -> Option<FieldSpan> {
    for root in ["Query", "Mutation"] {
        for fs in reconcile::field_spans(sdl, root) {
            let Some(op) = served_by(inventory, hints, root, &fs) else {
                continue;
            };
            if get_str(op, "key") == Some(op_key) {
                return Some(fs);
            }
        }
    }
    None
}

/// Every root field whose connector serves `op_key`, each with its
/// root-qualified name (`Mutation.x`, the key `closure::Schema` uses) and its
/// own span. Two root fields may share one operation key.
fn field_spans_for(
    sdl: &str,
    inventory: &Value,
    hints: &OpHints,
    op_key: &str,
) -> Vec<(String, FieldSpan)> {
    let mut out = Vec::new();
    for root in ["Query", "Mutation"] {
        for fs in reconcile::field_spans(sdl, root) {
            let Some(op) = served_by(inventory, hints, root, &fs) else {
                continue;
            };
            if get_str(op, "key") == Some(op_key) {
                out.push((format!("{}.{}", root, fs.name), fs));
            }
        }
    }
    out
}

/// The connect directive's own raw argument text for one field span.
fn connect_args(fs: &FieldSpan) -> Option<String> {
    crate::graphql::directives(&fs.decl, "connect")
        .into_iter()
        .next()
        .map(|d| d.args)
}

fn global_source_args(sdl: &str) -> Option<String> {
    crate::graphql::directives(sdl, "source")
        .into_iter()
        .next()
        .map(|d| d.args)
}

struct ErrorConfig {
    is_success: Option<String>,
    message: Option<String>,
    extensions: Option<String>,
}

fn read_error_config(args: &str) -> ErrorConfig {
    let errors_block = find_value(args, "errors");
    let (message, extensions) = match &errors_block {
        Some(block) => (
            find_value(block, "message"),
            find_value(block, "extensions"),
        ),
        None => (None, None),
    };
    ErrorConfig {
        is_success: find_value(args, "isSuccess"),
        message,
        extensions,
    }
}

fn effective_error_config(
    sdl: &str,
    op_key: &str,
    inventory: &Value,
    hints: &OpHints,
) -> ErrorConfig {
    let local = field_span_for(sdl, inventory, hints, op_key)
        .and_then(|fs| connect_args(&fs))
        .map(|args| read_error_config(&args));
    let global = global_source_args(sdl).map(|args| read_error_config(&args));
    let pick = |l: Option<String>, g: Option<String>| l.or(g);
    match (local, global) {
        (Some(l), Some(g)) => ErrorConfig {
            is_success: pick(l.is_success, g.is_success),
            message: pick(l.message, g.message),
            extensions: pick(l.extensions, g.extensions),
        },
        (Some(l), None) => l,
        (None, Some(g)) => g,
        (None, None) => ErrorConfig {
            is_success: None,
            message: None,
            extensions: None,
        },
    }
}

struct ConsumedRef {
    path: String,
    unresolved: Option<String>,
}

/// Split `s` on a top-level doubled `ch` (depth 0, outside any quoted
/// string) — the `??`-fallback chain in an `isSuccess`/`errors` expression.
fn split_top_level_doubled(s: &str, ch: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut start = 0usize;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => in_str = Some(c),
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            _ if depth == 0 && c == ch && chars.get(i + 1) == Some(&ch) => {
                parts.push(chars[start..i].iter().collect::<String>());
                i += 2;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(chars[start..].iter().collect());
    parts
}

fn parse_consumed_path(rooted_stripped: &str) -> ConsumedRef {
    let chars: Vec<char> = rooted_stripped.chars().collect();
    let mut i = 0usize;
    let ident = |chars: &[char], i: &mut usize| -> Option<String> {
        let start = *i;
        while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
            *i += 1;
        }
        if *i > start {
            Some(chars[start..*i].iter().collect())
        } else {
            None
        }
    };
    let Some(first) = ident(&chars, &mut i) else {
        return ConsumedRef {
            path: String::new(),
            unresolved: None,
        };
    };
    let mut segs = vec![first];
    loop {
        if i < chars.len() && chars[i] == '?' && chars.get(i + 1) != Some(&'?') {
            i += 1;
            continue;
        }
        if i + 1 < chars.len() && chars[i] == '-' && chars[i + 1] == '>' {
            i += 2;
            let method = ident(&chars, &mut i).unwrap_or_default();
            if method == "first" {
                if let Some(last) = segs.last_mut() {
                    last.push_str("[]");
                }
                continue;
            }
            return ConsumedRef {
                path: segs.join("."),
                unresolved: Some(format!("consumed expression not parsed: ->{}", method)),
            };
        }
        if i < chars.len() && chars[i] == '.' {
            i += 1;
            match ident(&chars, &mut i) {
                Some(w) => segs.push(w),
                None => break,
            }
            continue;
        }
        break;
    }
    ConsumedRef {
        path: segs.join("."),
        unresolved: None,
    }
}

fn parse_consumed_operand(raw: &str) -> Option<ConsumedRef> {
    let s = raw.trim();
    if s.is_empty() || s == "$status" || s == "true" || s == "false" || s == "null" {
        return None;
    }
    if s.starts_with('\'') || s.starts_with('"') {
        return None;
    }
    if s.chars()
        .next()
        .map(|c| c.is_ascii_digit() || c == '-')
        .unwrap_or(false)
    {
        return None;
    }
    if let Some(rest) = s.strip_prefix("$.") {
        return Some(parse_consumed_path(rest));
    }
    if s.starts_with('$') {
        return Some(ConsumedRef {
            path: String::new(),
            unresolved: Some(format!("consumed expression not parsed: {}", s)),
        });
    }
    if s.chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
    {
        return Some(parse_consumed_path(s));
    }
    None
}

/// One `isSuccess`/`errors.message`/`errors.extensions`-value expression's
/// wire-path references (step 2, "Response — Consumed"). An optional
/// surrounding `$(...)` is stripped first; the remainder is a `??`-chain of
/// operands, each parsed independently — a path contributes its own
/// reference *in addition to* an earlier operand's, not instead of it.
fn parse_consumed_expr(expr: &str) -> Vec<ConsumedRef> {
    let trimmed = expr.trim();
    let inner = match trimmed.strip_prefix("$(").and_then(|r| r.strip_suffix(')')) {
        Some(r) => r,
        None => trimmed,
    };
    split_top_level_doubled(inner, '?')
        .into_iter()
        .filter_map(|op| parse_consumed_operand(&op))
        .collect()
}

/// The `errors.extensions` block's own `key: expr` lines (a bare
/// `alias: expression` idiom, no `{ }` wrapper) — each value parsed by the
/// same rule as `isSuccess`/`errors.message`, independently.
fn parse_extensions_block(block: &str) -> Vec<ConsumedRef> {
    let mut out = Vec::new();
    for line in block.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((_, expr)) = line.split_once(':') {
            out.extend(parse_consumed_expr(expr));
        }
    }
    out
}

fn response_consumed(config: &ErrorConfig) -> Vec<ConsumedRef> {
    let mut out = Vec::new();
    if let Some(expr) = &config.is_success {
        out.extend(parse_consumed_expr(expr));
    }
    if let Some(expr) = &config.message {
        out.extend(parse_consumed_expr(expr));
    }
    if let Some(block) = &config.extensions {
        out.extend(parse_extensions_block(block));
    }
    out
}

// ─── Request — Mapped (http.body + recursive whole-argument forwarding) ────

/// The `key: expr` pairs of a request body or of an object literal inside
/// one, the same reader `source-coverage` walks the request with. Shared with
/// lint's argument tracing (ADR 0083), so the two do not read a body
/// differently.
pub fn parse_body_pairs(body_expr: &str) -> Vec<(String, String)> {
    // A `#` comment runs to the end of its line; left in, an apostrophe in
    // one opens a string that swallows every pair after it.
    let stripped = reconcile::strip_comments(body_expr);
    let trimmed = stripped.trim();
    let inner = strip_dollar_group(trimmed).unwrap_or(trimmed).trim();
    // Entries are separated by commas, by whitespace alone, or by a mix: a
    // bare body (Gitea's create_issue: one `key: expr` per line), a v0.3
    // nested `{ a: $args.a  b: $args.b }` (a sub-selection, which takes no
    // commas), and a v0.4 braced body written with or without them. A
    // pair's own nested `{ }`/`(...)` stays together.
    let inner = inner
        .strip_prefix('{')
        .and_then(|r| r.strip_suffix('}'))
        .unwrap_or(inner);
    split_selection_entries(inner)
        .into_iter()
        .filter_map(|pair| {
            // A quoted key (`"due-date": …`) is read without its quotes and
            // may itself hold a `:`.
            let (k, v) = match pair.chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let close = pair[1..].find(q)? + 1;
                    let v = pair[close + 1..].trim_start().strip_prefix(':')?;
                    (&pair[1..close], v)
                }
                _ => pair.split_once(':')?,
            };
            let k = k.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// The inside of `$( … )` when that one group is the whole expression, not
/// merely its first operand (`$("a") ?? $("b")` is not one group).
pub(crate) fn strip_dollar_group(e: &str) -> Option<&str> {
    let rest = e.strip_prefix("$(")?;
    let mut depth = 1i32;
    let mut in_str: Option<char> = None;
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        if let Some(q) = in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => in_str = Some(c),
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return (i + 1 == rest.len()).then(|| rest[..i].trim());
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a selection list into its `key: expr` entries: a new entry starts
/// at a top-level identifier or quoted key followed by `:`, where
/// whitespace (or a separating comma) precedes it. Nested `{ }`/`(…)`/`[…]`
/// and quoted strings never split.
fn split_selection_entries(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let is_start = |c: char| c.is_ascii_alphabetic() || c == '_';
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut starts = Vec::new();
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        let at_boundary = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == ',';
        match c {
            '\'' | '"' => {
                // A quoted key: the string closes, and a `:` follows it.
                if depth == 0 && at_boundary {
                    let mut j = i + 1;
                    while j < chars.len() && chars[j] != c {
                        j += if chars[j] == '\\' { 2 } else { 1 };
                    }
                    let close = j;
                    j += 1;
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if close < chars.len() && chars.get(j) == Some(&':') {
                        starts.push(i);
                        i = close + 1;
                        continue;
                    }
                }
                in_str = Some(c);
            }
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            _ if depth == 0 && is_start(c) && at_boundary => {
                let mut j = i;
                while j < chars.len() && is_ident(chars[j]) {
                    j += 1;
                }
                let key_end = j;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if chars.get(j) == Some(&':') && chars.get(j + 1) != Some(&':') {
                    starts.push(i);
                }
                i = key_end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let mut parts = Vec::new();
    for (n, &at) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(chars.len());
        let part: String = chars[at..end].iter().collect();
        parts.push(part.trim().trim_end_matches(',').to_string());
    }
    parts
}

/// `$args.a.b.c` or `$this.a.b` — a plain path expression, the request
/// side's simple-match grammar (arithmetic, `??`, and object-literal
/// construction are the other allowed forms per the plan; only the plain
/// path form is resolved into a forward/argument match here, since it's the
/// only one this codebase's real operations use for anything other than a
/// literal).
fn parse_arg_path(expr: &str) -> Option<Vec<String>> {
    let rest = expr
        .trim()
        .strip_prefix("$args.")
        .or_else(|| expr.trim().strip_prefix("$this."))?;
    if rest.is_empty() {
        return None;
    }
    let segs: Vec<&str> = rest.split('.').collect();
    let is_ident = |s: &str| {
        !s.is_empty()
            && s.chars()
                .next()
                .map(|c| c.is_ascii_alphabetic() || c == '_')
                .unwrap_or(false)
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    // Every segment must be a plain identifier — a trailing operator,
    // another `$args`/`$this` reference, or any other character outside
    // this small path grammar means the whole expression is outside what
    // this walk resolves, not a longer path to follow (`$args.a + $args.b`
    // must not be misread as a two-segment path `["a + $args", "b"]`).
    if segs.iter().all(|s| is_ident(s)) {
        Some(segs.into_iter().map(str::to_string).collect())
    } else {
        None
    }
}

/// Why the selection walk stopped at a node: the methods it does not follow,
/// named, so each remaining gap is countable by construct.
fn selection_reason(methods: &[String]) -> String {
    format!(
        "selection method not parsed: {}",
        methods
            .iter()
            .map(|m| format!("->{}", m))
            .collect::<String>()
    )
}

/// Why a request-body expression is outside the grammar, naming the
/// construct: the methods it chains, or its outermost form.
fn request_reason(expr: &str) -> String {
    let e = expr.trim();
    static METHOD_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let methods: Vec<String> = METHOD_RE
        .get_or_init(|| regex::Regex::new(r"->\s*([A-Za-z_][A-Za-z0-9_]*)").unwrap())
        .captures_iter(e)
        .map(|c| format!("->{}", &c[1]))
        .collect();
    let construct = if e.starts_with('{') {
        "object literal".to_string()
    } else if e.starts_with("$args.") && !e.contains("->") && !e.contains("??") && e.contains('{') {
        "argument sub-selection".to_string()
    } else if !methods.is_empty() {
        methods.concat()
    } else if e.contains("??") {
        "`??` fallback".to_string()
    } else if e.contains(['+', '*', '/']) || e.contains(" - ") {
        "arithmetic".to_string()
    } else {
        "expression".to_string()
    };
    format!("request expression not parsed: {}", construct)
}

pub(crate) fn is_literal(expr: &str) -> bool {
    let s = expr.trim();
    s.starts_with('\'')
        || s.starts_with('"')
        || s == "true"
        || s == "false"
        || s == "null"
        || s.chars()
            .next()
            .map(|c| c.is_ascii_digit() || c == '-')
            .unwrap_or(false)
}

/// The request wire fields a whole-argument forward's declared GraphQL
/// input type actually covers, recursing into further input types the same
/// way the response side unions/recurses through `$ref`s. `gql_type` is
/// always `Some` here — `mark_forward` is only ever entered once a caller
/// has already matched a field name against a real GraphQL type.
fn mark_forward<'a>(
    path: String,
    wire_shape: &'a Value,
    shapes: &'a Object,
    dropped: &DroppedConstructs,
    gql_type: &str,
    schema: &Schema,
    json_scalar: &str,
    seen: &mut Vec<String>,
    out: &mut BTreeMap<String, bool>,
) {
    // The recursion bottoms out as JSON exactly here: the GraphQL type
    // actually reached at this position is the raw JSON scalar, not merely
    // "this was forwarded" — the whole wire subtree from here down is
    // opaque, so every offered leaf under it (not only this position
    // itself) is `mapped (json)`, and none of them are matched against a
    // further GraphQL field (there is no further GraphQL structure to
    // match against once the type is a bare JSON scalar).
    if !json_scalar.is_empty() && gql_type == json_scalar {
        let mut leaves = Vec::new();
        walk_offered(
            &path,
            wire_shape,
            shapes,
            dropped,
            seen,
            &mut leaves,
            &mut BTreeSet::new(),
            None,
        );
        for (leaf_path, _unresolved) in leaves {
            out.insert(leaf_path, true);
        }
        return;
    }
    // Same per-branch cycle guard as `walk_offered`, and for the same
    // reason: a named shape's children are recursed into by this function
    // itself, after `classify` has already returned, so the guard must
    // stay pushed across that recursion rather than around `classify`'s own
    // call.
    let guard = guard_ref_names(wire_shape);
    let kind = classify(wire_shape, shapes, dropped, seen);
    seen.extend(guard.iter().cloned());
    match kind {
        Kind::Object { props, also_leaf } => {
            if also_leaf {
                out.insert(path.clone(), false);
            }
            for (name, child) in props {
                if let Some(field_ty) = schema.field_type(gql_type, &name) {
                    mark_forward(
                        push_child_path(&path, &name),
                        child,
                        shapes,
                        dropped,
                        field_ty,
                        schema,
                        json_scalar,
                        seen,
                        out,
                    );
                }
            }
        }
        Kind::Array(items) => {
            mark_forward(
                format!("{}[]", path),
                items,
                shapes,
                dropped,
                gql_type,
                schema,
                json_scalar,
                seen,
                out,
            );
        }
        Kind::BareArray => {
            out.insert(format!("{}[]", path), false);
        }
        Kind::Dict(_) | Kind::Scalar | Kind::Recursive | Kind::DepthLimit | Kind::Dropped(_) => {
            out.insert(path, false);
        }
    }
    for _ in &guard {
        seen.pop();
    }
}

/// `.factory/workspace.yaml` as JSON, or `null` when it is absent or unreadable.
fn workspace_doc(dir: &Path) -> Value {
    crate::factory_io::read_to_string(dir, ".factory/workspace.yaml")
        .ok()
        .and_then(|t| crate::yaml::parse(&t).ok())
        .unwrap_or(Value::Null)
}

fn type_prefix(dir: &Path) -> Option<String> {
    let text = crate::factory_io::read_to_string(dir, ".factory/workspace.yaml").ok()?;
    let ws = crate::yaml::parse(&text).ok()?;
    get_str(&ws, "type_prefix").map(str::to_string)
}

/// A workspace-relative file: `.factory/*` goes through custody (ADR 0025),
/// so a symlinked path is refused rather than followed; anything else is an
/// ordinary file the user authored.
fn read_workspace_text(dir: &Path, rel: &str) -> Result<String, String> {
    let rel = without_cur_dir(rel);
    if Path::new(&rel).starts_with(crate::factory_io::STATE_DIR) {
        crate::factory_io::read_to_string(dir, &rel).map_err(|e| e.to_string())
    } else {
        std::fs::read_to_string(dir.join(&rel)).map_err(|e| format!("{}: {}", rel, e))
    }
}

fn workspace_file_exists(dir: &Path, rel: &str) -> bool {
    let rel = without_cur_dir(rel);
    if Path::new(&rel).starts_with(crate::factory_io::STATE_DIR) {
        crate::factory_io::is_file(dir, &rel)
    } else {
        dir.join(&rel).exists()
    }
}

/// `rel` without `.` components, so `./.factory/x` is recognised as a
/// `.factory` path and goes through custody like `.factory/x`.
fn without_cur_dir(rel: &str) -> String {
    Path::new(rel)
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect::<std::path::PathBuf>()
        .to_string_lossy()
        .into_owned()
}

struct RequestMapping {
    /// path -> is_json
    mapped: BTreeMap<String, bool>,
    unresolved_prefixes: Vec<(String, String)>,
}

// ─── The command ──────────────────────────────────────────────────────────

/// The behaviour facts of one operation (ADR 0095): for every root field
/// serving it, each optional argument whose source states what omitting it
/// does, classified as documented, waived or unaccounted. Read with the
/// argument tracing lint warns from ([`crate::lint::argument_facts`]), so a
/// warning and a row are one reading. An argument the trace does not follow
/// gives no row.
fn behaviour_rows(
    sdl: &str,
    inventory: &Value,
    hints: &OpHints,
    op: &Value,
    op_key: &str,
    waivers: &[BehaviourWaiver],
) -> Vec<BehaviourRow> {
    let shapes = get_obj(inventory, "shapes");
    let mut rows = Vec::new();
    for (qualified, fs) in field_spans_for(sdl, inventory, hints, op_key) {
        let root = qualified.split('.').next().unwrap_or("Query");
        let Some(field_text) = crate::lint::root_field_text(sdl, root, &fs.name) else {
            continue;
        };
        let args = crate::lint::root_field_args(sdl, root)
            .into_iter()
            .find(|(f, _)| *f == fs.name)
            .map(|(_, a)| a)
            .unwrap_or_default();
        for fact in crate::lint::argument_facts(&field_text, &args, op, shapes) {
            let Some(sentence) = fact.sentence.clone() else {
                continue;
            };
            let covers = |w: &&BehaviourWaiver| w.operation == op_key && w.path == fact.key;
            let (class, note) = if fact.carried {
                (BehaviourClass::Documented, None)
            } else if let Some(w) = waivers
                .iter()
                .filter(covers)
                .find(|w| w.status == "resolved")
            {
                (
                    BehaviourClass::Waived {
                        reason: w.reason.clone(),
                        decision: w.decision.clone(),
                    },
                    None,
                )
            } else {
                let note = waivers.iter().find(covers).map(|w| {
                    format!(
                        "waived by {}, which is {}: only a resolved decision's waivers count",
                        w.decision,
                        if w.status.is_empty() {
                            "unset"
                        } else {
                            &w.status
                        }
                    )
                });
                (BehaviourClass::Unaccounted, note)
            };
            rows.push(BehaviourRow {
                path: fact.key.clone(),
                carrier: format!("{}({})", qualified, fact.arg),
                sentence,
                class,
                note,
            });
        }
    }
    rows
}

/// What every operation's report reads from the workspace, read once:
/// `source-coverage` with no OP-KEY classifies every selected operation
/// against one of these (ADR 0101) instead of reloading the workspace, the
/// source document and decisions.json per operation.
pub struct Prepared {
    dir: std::path::PathBuf,
    pub loaded: crate::cmd::lock::Loaded,
    dropped: DroppedConstructs,
    /// Kept as a result so a single-operation run reports an unknown
    /// operation before a decisions.json that does not load, as it did
    /// before the split.
    decisions: Result<(Value, Vec<OmitEntry>), String>,
    ws_doc: Value,
}

/// Loads what [`build_prepared`] reads. Fails when the workspace does not
/// load or has no inventory.json: no operation can be classified then.
pub fn prepare(dir: &Path) -> Result<Prepared, String> {
    let loaded = crate::cmd::lock::load(dir)?;
    if loaded.inventory.is_none() {
        return Err("no inventory.json in this workspace".to_string());
    }
    let dropped = build_dropped_constructs(dir);
    // Resolved decisions and current findings alike (ADR 0113 §2).
    let decisions = crate::findings::load_union(dir, None).map(|doc| {
        let omits = omits_from_doc(&doc);
        (doc, omits)
    });
    Ok(Prepared {
        dir: dir.to_path_buf(),
        loaded,
        dropped,
        decisions,
        ws_doc: workspace_doc(dir),
    })
}

/// One operation's report: [`prepare`] then [`build_prepared`].
pub fn build(dir: &Path, op_key: &str) -> Result<Report, String> {
    build_prepared(&prepare(dir)?, op_key)
}

pub fn build_prepared(prepared: &Prepared, op_key: &str) -> Result<Report, String> {
    let dir = prepared.dir.as_path();
    let loaded = &prepared.loaded;
    let inventory = loaded
        .inventory
        .as_ref()
        .ok_or_else(|| "no inventory.json in this workspace".to_string())?;
    let op = get_arr(inventory, "operations")
        .into_iter()
        .flatten()
        .find(|o| get_str(o, "key") == Some(op_key))
        .ok_or_else(|| format!("no operation {:?} in inventory.json", op_key))?
        .clone();
    let shapes = get_obj(inventory, "shapes").cloned().unwrap_or_default();
    let dropped = &prepared.dropped;

    let (decisions_doc, omits) = match &prepared.decisions {
        Ok((doc, omits)) => (doc, omits),
        Err(e) => return Err(e.clone()),
    };

    // Response
    let response_root = root_shape_ref(&op, "response");
    // Only the first root field on a shared operation key is read here, so a
    // path only a later field selects reports unaccounted (a false alarm,
    // never a false mapped; ADR 0037). The request side unions every field.
    let field_span = field_span_for(&loaded.sdl, inventory, &loaded.hints, op_key);
    // What the operation asks the source for decides what an expansion
    // boundary offers (ADR 0047): the outgoing fields expression, read the
    // way sparse-fieldsets reads it — independent of what the mapping keeps.
    // A workspace that turned sparse-fieldsets off has no fields expression.
    let ws_doc = &prepared.ws_doc;
    let wire = field_span
        .as_ref()
        .filter(|_| crate::sparse::enabled(&ws_doc))
        .and_then(|fs| {
            crate::sparse::wire_expression(&fs.decl, &crate::sparse::param_name(&ws_doc))
        });
    let can_ask = wire.is_some() && crate::sparse::string_param(&ws_doc, &op).is_some();
    // Groups are relative to a list's item: the one list predicate
    // sparse-fieldsets and scaffold use.
    let list_root = crate::sparse::list_root(&op);
    let mut bounds = Boundaries::for_wire(wire.as_deref(), list_root.as_deref());
    let (response_offered, response_subtrees) =
        offered_rows_full(&response_root, &shapes, &dropped, Some(&mut bounds));
    let selection_text = field_span
        .as_ref()
        .and_then(|fs| fs.connect.as_ref())
        .and_then(|c| c.selection.clone());
    let mapped = response_mapped(&op, &shapes, &dropped, selection_text.as_deref());
    let error_config = effective_error_config(&loaded.sdl, op_key, inventory, &loaded.hints);
    let consumed = response_consumed(&error_config);
    let consumed_paths: std::collections::BTreeSet<String> = consumed
        .iter()
        .filter(|c| c.unresolved.is_none())
        .map(|c| c.path.clone())
        .collect();
    let consumed_unresolved: BTreeMap<String, String> = consumed
        .into_iter()
        .filter_map(|c| c.unresolved.map(|u| (c.path, u)))
        .collect();

    let mut response_rows = Vec::new();
    for (path, unresolved) in response_offered {
        let structurally_consumed = consumed_paths.contains(&path);
        let class = if let Some(target) = bounds.unverified.get(&path) {
            Class::UnverifiedDefault {
                target: target.clone(),
            }
        } else if let Some(detail) = unresolved {
            Class::Unresolved(detail)
        } else if let Some(detail) = consumed_unresolved.get(&path) {
            Class::Unresolved(detail.clone())
        } else if let Some(detail) = mapped
            .unresolved_prefixes
            .iter()
            .find_map(|(p, d)| (&path == p || beneath_prefix(&path, p)).then(|| d.clone()))
        {
            Class::Unresolved(detail)
        } else if let Some(&is_json) = mapped.exact.get(&path) {
            Class::Mapped { json: is_json }
        } else if mapped
            .exact
            .iter()
            .any(|(k, &j)| j && beneath_prefix(&path, k))
        {
            Class::Mapped { json: true }
        } else if response_subtrees.contains(&path) && reaches_beneath(mapped.exact.keys(), &path) {
            Class::Mapped { json: false }
        } else if let Some(detail) = mapped.open_spreads.iter().find_map(|(p, d)| {
            (p.is_empty()
                || path.starts_with(&format!("{}.", p))
                || path.starts_with(&format!("{}[", p)))
            .then(|| d.clone())
        }) {
            Class::Unresolved(detail)
        } else if let Some(entry) = find_omit(&omits, op_key, Direction::Response, &path) {
            Class::OmittedDecided {
                reason: entry.reason,
            }
        } else if structurally_consumed {
            Class::Consumed
        } else {
            Class::Unaccounted
        };
        let note = matches!(class, Class::Unaccounted)
            .then(|| uncounted_omit_note(&omits, op_key, Direction::Response, &path))
            .flatten();
        response_rows.push(Row {
            path,
            direction: Direction::Response,
            class,
            structurally_consumed,
            note,
        });
    }

    response_rows.extend(transport_expansion_missing(
        &bounds,
        mapped.exact.keys(),
        &decisions_doc,
        op_key,
        can_ask,
    ));

    // Request
    let request_root = root_shape_ref(&op, "request_body");
    let (request_offered, request_subtrees) =
        offered_rows_full(&request_root, &shapes, &dropped, None);
    let request_map = request_mapped_full(
        dir,
        &loaded.sdl,
        &op,
        op_key,
        inventory,
        &loaded.hints,
        &shapes,
        &dropped,
    );

    let mut request_rows = Vec::new();
    for (path, unresolved) in request_offered {
        let class = if let Some(detail) = unresolved {
            Class::Unresolved(detail)
        } else if let Some(detail) = request_map.unresolved_prefixes.iter().find_map(|(p, d)| {
            (path == *p
                || path.starts_with(&format!("{}.", p))
                || path.starts_with(&format!("{}[", p)))
            .then(|| d.clone())
        }) {
            Class::Unresolved(detail)
        } else if let Some(&is_json) = request_map.mapped.get(&path) {
            Class::Mapped { json: is_json }
        } else if request_subtrees.contains(&path)
            && reaches_beneath(request_map.mapped.keys(), &path)
        {
            Class::Mapped { json: false }
        } else if let Some(entry) = find_omit(&omits, op_key, Direction::Request, &path) {
            Class::OmittedDecided {
                reason: entry.reason,
            }
        } else {
            Class::Unaccounted
        };
        let note = matches!(class, Class::Unaccounted)
            .then(|| uncounted_omit_note(&omits, op_key, Direction::Request, &path))
            .flatten();
        request_rows.push(Row {
            path,
            direction: Direction::Request,
            class,
            structurally_consumed: false,
            note,
        });
    }

    let behaviour = behaviour_rows(
        &loaded.sdl,
        inventory,
        &loaded.hints,
        &op,
        op_key,
        &behaviour_waivers(&decisions_doc),
    );

    let mut stale = stale_for(&omits, op_key, Direction::Response, &response_rows);
    stale.extend(stale_for(&omits, op_key, Direction::Request, &request_rows));
    Ok(Report {
        op_key: op_key.to_string(),
        response: response_rows,
        request: request_rows,
        behaviour,
        stale,
        dropped_status: match dropped.status {
            "ran" => DroppedStatus::Ran,
            other => DroppedStatus::Skipped(other),
        },
    })
}

/// The request side's own mapped computation: top-level `http.body` pairs,
/// each classified as a direct argument path, a literal, a whole-argument
/// forward (recursed against the argument's declared GraphQL input type),
/// or an unresolved expression.
fn request_mapped_full(
    dir: &Path,
    sdl: &str,
    op: &Value,
    op_key: &str,
    inventory: &Value,
    hints: &OpHints,
    shapes: &Object,
    dropped: &DroppedConstructs,
) -> RequestMapping {
    let mut out = RequestMapping {
        mapped: BTreeMap::new(),
        unresolved_prefixes: Vec::new(),
    };
    let request_root = root_shape_ref(op, "request_body");
    let Ok(schema) = closure::build_schema(sdl, Some(inventory)) else {
        return out;
    };
    let top_level_props: std::collections::BTreeSet<String> = {
        let mut seen = Vec::new();
        match classify(&request_root, shapes, dropped, &mut seen) {
            Kind::Object { props, .. } => props.keys().cloned().collect(),
            _ => Default::default(),
        }
    };
    // No `type_prefix` on record means nothing can ever equal the JSON
    // scalar name, so every forward simply bottoms out as an ordinary
    // (non-json) match — never a crash either way.
    let json_scalar = type_prefix(dir)
        .map(|p| format!("{}_JSON", p))
        .unwrap_or_default();
    // Two root fields can serve one operation. Each field's body is read
    // with that field's own argument types, never one field's types paired
    // with another's body; a path is mapped when any field's own body sends
    // it.
    for (field_name, fs) in field_spans_for(sdl, inventory, hints, op_key) {
        let Some(args) = connect_args(&fs) else {
            continue;
        };
        let Some(http_block) = find_value(&args, "http") else {
            continue;
        };
        let Some(body_expr) = find_value(&http_block, "body") else {
            continue;
        };
        let mut one = RequestMapping {
            mapped: BTreeMap::new(),
            unresolved_prefixes: Vec::new(),
        };
        request_mapped_with_prefix(
            &top_level_props,
            &body_expr,
            &request_root,
            shapes,
            dropped,
            &schema,
            &field_name,
            &json_scalar,
            &mut one,
        );
        for (path, json) in one.mapped {
            let entry = out.mapped.entry(path).or_insert(json);
            *entry = *entry && json;
        }
        for prefix in one.unresolved_prefixes {
            if !out.unresolved_prefixes.contains(&prefix) {
                out.unresolved_prefixes.push(prefix);
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn request_mapped_with_prefix(
    top_level_props: &std::collections::BTreeSet<String>,
    body_expr: &str,
    request_root: &Value,
    shapes: &Object,
    dropped: &DroppedConstructs,
    schema: &Schema,
    field_name: &str,
    json_scalar: &str,
    out: &mut RequestMapping,
) {
    let ctx = RequestCtx {
        shapes,
        dropped,
        schema,
        field_name,
        json_scalar,
    };
    // A body that is one argument expression rather than a list of `key:
    // expr` pairs: `$args.input` (the whole argument forwarded) or
    // `$args.input { Wire: field … }` (an argument sub-selection). Its value
    // is read at the request root; parse_body_pairs finds no pairs in it.
    let whole = reconcile::strip_comments(body_expr);
    let whole = whole.trim();
    if whole.starts_with("$args.") {
        map_request_value(&ctx, String::new(), whole, Some(request_root), out);
        return;
    }
    for (key, expr) in parse_body_pairs(body_expr) {
        if !top_level_props.contains(&key) {
            continue;
        }
        let wire = top_level_shape(request_root, shapes, dropped, &key);
        map_request_value(&ctx, key, &expr, wire, out);
    }
}

/// What a request-body walk needs besides the position it is at.
struct RequestCtx<'a> {
    shapes: &'a Object,
    dropped: &'a DroppedConstructs,
    schema: &'a Schema,
    field_name: &'a str,
    json_scalar: &'a str,
}

/// The property `name` of the object `wire` resolves to, if it has one.
fn child_wire<'a>(ctx: &RequestCtx<'a>, wire: &'a Value, name: &str) -> Option<&'a Value> {
    let mut seen = Vec::new();
    match classify(wire, ctx.shapes, ctx.dropped, &mut seen) {
        Kind::Object { props, .. } => props.get(name).copied(),
        _ => None,
    }
}

/// Classify the request paths one body value sends, at wire path `path`
/// (ADR 0050). The grammar, the forms connectors-language.md recommends:
///
/// - a literal: sent, mapped;
/// - an object literal, `{ k: v }` or `$({ k: v })`: each key read against
///   the matching property of the request shape, recursively (a key the
///   shape does not document is not a coverage question);
/// - `$args.x`: the whole argument forwarded, against its input type;
/// - `$args.x { wire: field … }`: an argument sub-selection, each wire key
///   read from the argument's field, recursively;
/// - `$args.a.b`: a deeper argument path, forwarding the input field it names.
///
/// Anything else is unresolved at `path`, naming the construct.
/// The input type an argument path (`$args.a.b…`) names, for both the
/// plain forward and the sub-selection (`$args.a.b { … }`), so the two
/// cannot drift. The walk follows the argument's input type down the path.
/// `None` means the walk stopped and has already recorded the row at
/// `path`: `mapped` as an opaque value once it reaches the JSON scalar,
/// which declares no structure below it, or `unresolved` when the first
/// segment is not an argument of the field or a later one is not a field of
/// the type it reaches.
fn arg_path_type<'a>(
    ctx: &RequestCtx<'a>,
    path: &str,
    segs: &[String],
    out: &mut RequestMapping,
) -> Option<&'a str> {
    let Some(mut arg_ty) = ctx.schema.root_arg_type(ctx.field_name, &segs[0]) else {
        out.unresolved_prefixes.push((
            path.to_string(),
            format!(
                "request expression not parsed: `{}` is not an argument of the field",
                segs[0]
            ),
        ));
        return None;
    };
    for seg in &segs[1..] {
        if !ctx.json_scalar.is_empty() && arg_ty == ctx.json_scalar {
            out.mapped.insert(path.to_string(), true);
            return None;
        }
        match ctx.schema.field_type(arg_ty, seg) {
            Some(t) => arg_ty = t,
            None => {
                out.unresolved_prefixes.push((
                    path.to_string(),
                    format!(
                        "request expression not parsed: `{}` is not a field of {}",
                        seg, arg_ty
                    ),
                ));
                return None;
            }
        }
    }
    Some(arg_ty)
}

fn map_request_value(
    ctx: &RequestCtx,
    path: String,
    expr: &str,
    wire: Option<&Value>,
    out: &mut RequestMapping,
) {
    let e = expr.trim();
    if is_literal(e) {
        out.mapped.insert(path, false);
        return;
    }
    // `$( … )` wraps a literal the same way at every connect version.
    let group = strip_dollar_group(e);
    if group.is_some_and(is_literal) {
        out.mapped.insert(path, false);
        return;
    }
    let e = group.filter(|r| r.starts_with('{')).unwrap_or(e);
    if e.starts_with('{') && e.ends_with('}') {
        let Some(wire) = wire else {
            return;
        };
        // A position the request shape offers as one row (a reference it
        // does not decompose, a dictionary, a JSON value): the literal
        // sends that row.
        let mut seen = Vec::new();
        if !matches!(
            classify(wire, ctx.shapes, ctx.dropped, &mut seen),
            Kind::Object { .. }
        ) {
            out.mapped.insert(path, false);
            return;
        }
        for (key, value) in parse_body_pairs(e) {
            let child = child_wire(ctx, wire, &key);
            if child.is_none() {
                continue;
            }
            map_request_value(ctx, push_child_path(&path, &key), &value, child, out);
        }
        return;
    }
    // `$args.x { … }`: the sub-selection is everything from the first `{`.
    if let Some(open) = e
        .find('{')
        .filter(|_| e.starts_with("$args.") && e.ends_with('}'))
    {
        let head = e[..open].trim();
        if let Some(segs) = parse_arg_path(head) {
            // A deeper head (`$args.input.targeting { … }`) selects from the
            // input field it names.
            let Some(arg_ty) = arg_path_type(ctx, &path, &segs, out) else {
                return;
            };
            if let Some(wire) = wire {
                let nodes = crate::reconcile::parse_selection(&e[open + 1..e.len() - 1]);
                map_sub_selection(ctx, path, &nodes, wire, arg_ty, &mut Vec::new(), out);
            }
            return;
        }
    }
    let Some(segs) = parse_arg_path(e) else {
        out.unresolved_prefixes.push((path, request_reason(e)));
        return;
    };
    // A deeper `$args.a.b` forwards the input field it names.
    let Some(arg_ty) = arg_path_type(ctx, &path, &segs, out) else {
        return;
    };
    if let Some(wire) = wire {
        let mut seen = Vec::new();
        mark_forward(
            path,
            wire,
            ctx.shapes,
            ctx.dropped,
            arg_ty,
            ctx.schema,
            ctx.json_scalar,
            &mut seen,
            &mut out.mapped,
        );
    }
}

/// An argument sub-selection (`$args.x { wire: field nested { … } }`)
/// against the wire shape at `path` and the argument's input type. A list
/// argument steps into the array's items (`requests[]`). Each node sends
/// its wire key (the alias, or the field name) from the input field it
/// names; a nested sub-selection recurses, a bare field forwards the whole
/// field.
fn map_sub_selection(
    ctx: &RequestCtx,
    path: String,
    nodes: &[Node],
    wire: &Value,
    gql_type: &str,
    seen: &mut Vec<String>,
    out: &mut RequestMapping,
) {
    // Same per-branch cycle guard as `mark_forward`. The array arm consumes
    // no selection node, so on a recursive array shape (`Tree = [Tree]`) this
    // guard is the only thing that stops the walk: the items meet the shape
    // they are inside, classify as `Recursive`, and send the one row the
    // offered walk stops at.
    let guard = guard_ref_names(wire);
    let kind = classify(wire, ctx.shapes, ctx.dropped, seen);
    seen.extend(guard.iter().cloned());
    match kind {
        Kind::Array(items) => map_sub_selection(
            ctx,
            format!("{}[]", path),
            nodes,
            items,
            gql_type,
            seen,
            out,
        ),
        Kind::Object { .. } => {
            map_sub_selection_fields(ctx, &path, nodes, wire, gql_type, seen, out)
        }
        // A position the request shape offers as one row: the sub-selection
        // sends that row.
        _ => {
            out.mapped.insert(path, false);
        }
    }
    for _ in &guard {
        seen.pop();
    }
}

/// The nodes of a sub-selection over an object position at `path`.
fn map_sub_selection_fields(
    ctx: &RequestCtx,
    path: &str,
    nodes: &[Node],
    wire: &Value,
    gql_type: &str,
    seen: &mut Vec<String>,
    out: &mut RequestMapping,
) {
    for node in nodes {
        let Some(field) = node
            .key
            .as_ref()
            .filter(|k| k.len() == 1 && node.methods.is_empty() && !node.rooted)
            .map(|k| k[0].clone())
        else {
            // The wire key is the alias, or the entry's own name when it
            // has none (`title->trim` still sends `title`).
            let name = node.alias.clone().or_else(|| {
                node.key
                    .as_ref()
                    .filter(|k| k.len() == 1)
                    .map(|k| k[0].clone())
            });
            if let Some(name) = name {
                let construct = if node.methods.is_empty() {
                    "argument sub-selection entry".to_string()
                } else {
                    node.methods.iter().map(|m| format!("->{}", m)).collect()
                };
                out.unresolved_prefixes.push((
                    push_child_path(path, &name),
                    format!("request expression not parsed: {}", construct),
                ));
            }
            continue;
        };
        let wire_key = node.alias.clone().unwrap_or_else(|| field.clone());
        let child_path = push_child_path(path, &wire_key);
        let Some(child) = child_wire(ctx, wire, &wire_key) else {
            continue;
        };
        let Some(field_ty) = ctx.schema.field_type(gql_type, &field) else {
            out.unresolved_prefixes.push((
                child_path,
                format!(
                    "request expression not parsed: `{}` is not a field of {}",
                    field, gql_type
                ),
            ));
            continue;
        };
        match &node.children {
            Some(children) => {
                map_sub_selection(ctx, child_path, children, child, field_ty, seen, out)
            }
            None => {
                mark_forward(
                    child_path,
                    child,
                    ctx.shapes,
                    ctx.dropped,
                    field_ty,
                    ctx.schema,
                    ctx.json_scalar,
                    seen,
                    &mut out.mapped,
                );
            }
        }
    }
}

fn top_level_shape<'a>(
    request_root: &'a Value,
    shapes: &'a Object,
    dropped: &DroppedConstructs,
    key: &str,
) -> Option<&'a Value> {
    let mut seen = Vec::new();
    match classify(request_root, shapes, dropped, &mut seen) {
        Kind::Object { props, .. } => props.get(key).copied(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn pilot(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../pilots/graphos")
            .join(name)
    }

    fn no_dropped() -> DroppedConstructs {
        DroppedConstructs {
            by_name: BTreeMap::new(),
            status: "skipped (test)",
        }
    }

    // ── Consumed extractor: all seven real config strings, verbatim ──────────

    #[test]
    fn consumed_global_is_success() {
        let refs = parse_consumed_expr("$.success");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, "success");
        assert!(refs[0].unresolved.is_none());
    }

    #[test]
    fn consumed_global_errors_message() {
        let refs = parse_consumed_expr("$($.errors?->first?.message ?? 'Vendor request failed')");
        assert_eq!(
            refs.len(),
            1,
            "the string literal fallback contributes nothing: {:?}",
            refs.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
        assert_eq!(refs[0].path, "errors[].message");
    }

    #[test]
    fn consumed_global_errors_extensions() {
        let refs =
            parse_extensions_block("httpStatus: $status\nparameter: $.errors?->first?.parameter");
        let paths: Vec<&str> = refs.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["errors[].parameter"],
            "$status contributes zero paths"
        );
    }

    #[test]
    fn consumed_d0023_is_success() {
        let refs = parse_consumed_expr("$.success ?? true");
        assert_eq!(refs.len(), 1, "the bare `true` literal contributes nothing");
        assert_eq!(refs[0].path, "success");
    }

    #[test]
    fn consumed_d0023_errors_message_bare_path() {
        let refs = parse_consumed_expr("$(errorInfo.message ?? 'Vendor request failed')");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].path, "errorInfo.message",
            "a bare, un-prefixed relative path is not opaque for lacking $."
        );
    }

    #[test]
    fn consumed_d0023_errors_extensions_multi_key() {
        let refs = parse_extensions_block("code: errorInfo.code\nrequestId: errorInfo.requestId");
        let paths: Vec<&str> = refs.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["errorInfo.code", "errorInfo.requestId"]);
    }

    #[test]
    fn consumed_assessment_start_chained_fallback() {
        let refs = parse_consumed_expr(
            "$($.errors?->first?.message ?? $.message ?? 'Vendor request failed')",
        );
        let paths: Vec<&str> = refs.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["errors[].message", "message"],
            "a second, path-typed ?? operand is extracted in addition to the first"
        );
    }

    #[test]
    fn consumed_status_is_never_a_body_path() {
        let refs = parse_consumed_expr("$status");
        assert!(refs.is_empty());
    }

    #[test]
    fn consumed_unknown_method_marks_unresolved_at_the_partial_path() {
        let refs = parse_consumed_expr("$.errors?->weird?.message");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, "errors");
        assert_eq!(
            refs[0].unresolved.as_deref(),
            Some("consumed expression not parsed: ->weird")
        );
    }

    // ── decisions.json omits reading ──────────────────────────────────────────

    #[test]
    fn omits_from_doc_reads_every_record_and_ignores_one_with_no_omits() {
        let doc = serde_json::json!({
            "contract_version": 1,
            "decisions": [
                {
                    "id": "D-0001", "title": "some decision", "status": "resolved",
                    "date": "2026-09-22",
                    "resolution": {"decision": "omit them"},
                    "omits": [
                        {
                            "operation": "post:/application.info",
                            "direction": "response",
                            "path": "results.openings[]",
                            "reason": "editorial"
                        },
                        {
                            "operation": "post:/application.info",
                            "direction": "response",
                            "path": "success",
                            "reason": "consumed"
                        }
                    ]
                },
                {
                    "id": "D-0002", "title": "another decision, no omits here",
                    "status": "resolved", "date": "2026-09-22",
                    "resolution": {"decision": "n/a"}
                }
            ]
        });
        let entries = omits_from_doc(&doc);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].operation, "post:/application.info");
        assert_eq!(entries[0].direction, Direction::Response);
        assert_eq!(entries[0].path, "results.openings[]");
        assert_eq!(entries[0].reason, Reason::Editorial);
        assert_eq!(entries[1].reason, Reason::Consumed);
    }

    #[test]
    fn omit_covers_a_parent_path_covers_its_children_never_by_type_name() {
        let doc = serde_json::json!({
            "contract_version": 1,
            "decisions": [{
                "id": "D-0001", "title": "x", "status": "resolved", "date": "2026-09-22",
                "resolution": {"decision": "n/a"},
                "omits": [{
                    "operation": "post:/x",
                    "direction": "response",
                    "path": "results.openings[]",
                    "reason": "editorial"
                }]
            }]
        });
        let entries = omits_from_doc(&doc);
        assert!(find_omit(
            &entries,
            "post:/x",
            Direction::Response,
            "results.openings[]"
        )
        .is_some());
        assert!(find_omit(
            &entries,
            "post:/x",
            Direction::Response,
            "results.openings[].id"
        )
        .is_some());
        assert!(find_omit(
            &entries,
            "post:/x",
            Direction::Response,
            "results.openingsOther"
        )
        .is_none());
        // Never inherited to a different operation on the same shared type.
        assert!(find_omit(
            &entries,
            "post:/y",
            Direction::Response,
            "results.openings[]"
        )
        .is_none());
    }

    // ── Request body pairs ────────────────────────────────────────────────────

    #[test]
    fn parse_body_pairs_handles_the_real_multiline_triple_quoted_form() {
        let body = "\n        $({\n          entityType: $args.entityType,\n          entityId: $args.entityId,\n          approvalStepDefinitions: $args.approvalStepDefinitions,\n          submitApprovalRequest: $args.submitApprovalRequest\n        })\n        ";
        let pairs = parse_body_pairs(body);
        assert_eq!(
            pairs,
            vec![
                ("entityType".to_string(), "$args.entityType".to_string()),
                ("entityId".to_string(), "$args.entityId".to_string()),
                (
                    "approvalStepDefinitions".to_string(),
                    "$args.approvalStepDefinitions".to_string()
                ),
                (
                    "submitApprovalRequest".to_string(),
                    "$args.submitApprovalRequest".to_string()
                ),
            ]
        );
    }

    #[test]
    fn parse_body_pairs_handles_the_single_line_form() {
        let pairs = parse_body_pairs("$({applicationId: $args.id})");
        assert_eq!(
            pairs,
            vec![("applicationId".to_string(), "$args.id".to_string())]
        );
    }

    #[test]
    fn parse_body_pairs_empty_object() {
        assert!(parse_body_pairs("$({})").is_empty());
    }

    #[test]
    fn parse_body_pairs_handles_the_bare_newline_separated_form() {
        // Verbatim from pilots/gitea/gitea.graphql's create_issue body: no
        // `$(...)`/`{ }` wrapper, one `key: expr` per line, no commas.
        let body = "\n        title: $args.title\n        body: $args.body\n        assignees: $args.assignees\n        labels: $args.labels\n        milestone: $args.milestone\n        due_date: $args.dueDate\n        ref: $args.ref\n        closed: $args.closed\n        ";
        let pairs = parse_body_pairs(body);
        assert_eq!(
            pairs,
            vec![
                ("title".to_string(), "$args.title".to_string()),
                ("body".to_string(), "$args.body".to_string()),
                ("assignees".to_string(), "$args.assignees".to_string()),
                ("labels".to_string(), "$args.labels".to_string()),
                ("milestone".to_string(), "$args.milestone".to_string()),
                ("due_date".to_string(), "$args.dueDate".to_string()),
                ("ref".to_string(), "$args.ref".to_string()),
                ("closed".to_string(), "$args.closed".to_string()),
            ]
        );
    }

    #[test]
    fn parse_body_pairs_bare_form_keeps_a_multiline_nested_object_literal_as_one_pair() {
        let body = "\n        foo: {\n          a: $args.a,\n          b: $args.b\n        }\n        bar: $args.bar\n        ";
        let pairs = parse_body_pairs(body);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "foo");
        assert!(pairs[0].1.contains("a: $args.a"));
        assert!(pairs[0].1.contains("b: $args.b"));
        assert_eq!(pairs[1], ("bar".to_string(), "$args.bar".to_string()));
    }

    // ── classify(): depth limit, recursion guard, mixed unions ────────────────

    fn shapes_from(pairs: Vec<(&str, Value)>) -> Object {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    #[test]
    fn depth_limit_marker_is_unresolved_not_a_plain_leaf() {
        // Step 0's own finding: none of the measured vendor's shapes hit this, so this
        // case is necessarily synthetic — a shape exactly matching the fixed
        // object `openapi.rs:361` writes in place of a truncated conversion.
        let shapes = shapes_from(vec![(
            "Deep",
            serde_json::json!({"type": "object", "properties": {
                "leaf": {"description": "depth limit reached during inventory conversion"}
            }}),
        )]);
        let root = serde_json::json!({"$ref": "#/shapes/Deep"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let (_, detail) = rows.iter().find(|(p, _)| p == "leaf").unwrap();
        assert_eq!(detail.as_deref(), Some("depth limit at inventory build"));
    }

    #[test]
    fn a_self_referential_shape_stops_recursion_rather_than_looping_forever() {
        let shapes = shapes_from(vec![(
            "Node",
            serde_json::json!({"type": "object", "properties": {
                "id": {"type": "string"},
                "child": {"$ref": "#/shapes/Node"}
            }}),
        )]);
        let root = serde_json::json!({"$ref": "#/shapes/Node"});
        // No infinite loop: `id` offers normally, and `child`, reappearing
        // while `Node` is already open on this branch, is offered as one row
        // and not expanded again. (Changed in review: it used to emit no row
        // at all, which hid a wire field from the accounting.)
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let paths: Vec<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["child", "id"]);
    }

    // HubSpot's CRM Lists filter, reduced: three branch shapes, each holding
    // an array whose items are a `oneOf` of all three. No single `$ref` sits
    // in the items position, so the walk met a different branch on every lap
    // and recursed until the stack overflowed (ADR 0102).
    fn filter_branch_shapes() -> Object {
        let branches = serde_json::json!({"type": "array", "items": {"oneOf": [
            {"$ref": "#/shapes/OrBranch"},
            {"$ref": "#/shapes/AndBranch"},
            {"$ref": "#/shapes/NotBranch"}
        ]}});
        shapes_from(vec![
            (
                "OrBranch",
                serde_json::json!({"type": "object", "properties": {
                    "filterBranchType": {"type": "string"},
                    "filterBranches": branches.clone()
                }}),
            ),
            (
                "AndBranch",
                serde_json::json!({"type": "object", "properties": {
                    "filterBranchType": {"type": "string"},
                    "filterBranches": branches.clone(),
                    "filters": {"type": "array", "items": {"type": "string"}}
                }}),
            ),
            (
                "NotBranch",
                serde_json::json!({"type": "object", "properties": {
                    "filterBranchType": {"type": "string"},
                    "filterBranches": branches
                }}),
            ),
        ])
    }

    #[test]
    fn a_oneof_of_several_refs_that_cycles_back_stops_recursion() {
        let shapes = filter_branch_shapes();
        let root = serde_json::json!({"$ref": "#/shapes/OrBranch"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let paths: Vec<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        // The first lap expands the merged variants once; on the second
        // every branch is open, so the walk offers one row for the subtree
        // (`filterBranches[].filterBranches[]`) and stops.
        assert_eq!(
            paths,
            vec![
                "filterBranchType",
                "filterBranches[].filterBranchType",
                "filterBranches[].filterBranches[]",
                "filterBranches[].filters[]"
            ]
        );
    }

    #[test]
    fn a_oneof_of_several_refs_that_does_not_cycle_is_still_expanded() {
        // Guarding a shape reached through a `oneOf` must not suppress a
        // sibling branch that merely reuses one of the variants.
        let shapes = shapes_from(vec![
            (
                "Cat",
                serde_json::json!({"type": "object", "properties": {"lives": {"type": "integer"}}}),
            ),
            (
                "Dog",
                serde_json::json!({"type": "object", "properties": {"bark": {"type": "string"}}}),
            ),
            (
                "Owner",
                serde_json::json!({"type": "object", "properties": {
                    "pet": {"oneOf": [{"$ref": "#/shapes/Cat"}, {"$ref": "#/shapes/Dog"}]},
                    "backup": {"oneOf": [{"$ref": "#/shapes/Cat"}, {"$ref": "#/shapes/Dog"}]}
                }}),
            ),
        ]);
        let root = serde_json::json!({"$ref": "#/shapes/Owner"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let mut paths: Vec<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec!["backup.bark", "backup.lives", "pet.bark", "pet.lives"]
        );
    }

    #[test]
    fn a_two_hop_alias_chain_resolves_to_the_end_shapes_properties() {
        // Asana's real shape: JobResponse -> JobBase -> JobCompact, none of
        // which is itself an object — each is nothing but a further $ref.
        let shapes = shapes_from(vec![
            ("A", serde_json::json!({"$ref": "#/shapes/B"})),
            ("B", serde_json::json!({"$ref": "#/shapes/C"})),
            (
                "C",
                serde_json::json!({"type": "object", "properties": {
                    "x": {"type": "string"}
                }}),
            ),
        ]);
        let root = serde_json::json!({"$ref": "#/shapes/A"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let paths: Vec<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec!["x"],
            "a two-hop alias must not read as a bare leaf"
        );
    }

    #[test]
    fn a_three_hop_alias_chain_resolves_to_the_end_shapes_properties() {
        let shapes = shapes_from(vec![
            ("A", serde_json::json!({"$ref": "#/shapes/B"})),
            ("B", serde_json::json!({"$ref": "#/shapes/C"})),
            ("C", serde_json::json!({"$ref": "#/shapes/D"})),
            (
                "D",
                serde_json::json!({"type": "object", "properties": {
                    "y": {"type": "string"}
                }}),
            ),
        ]);
        let root = serde_json::json!({"$ref": "#/shapes/A"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let paths: Vec<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            vec!["y"],
            "a three-hop alias must not read as a bare leaf"
        );
    }

    #[test]
    fn a_self_referencing_alias_chain_reports_recursive_not_a_leaf() {
        // A and B alias each other with no object ever in the chain — a
        // pure-alias cycle, distinct from the property-level self-reference
        // above. Must not overflow the stack, and must not read as a leaf.
        let shapes = shapes_from(vec![
            ("A", serde_json::json!({"$ref": "#/shapes/B"})),
            ("B", serde_json::json!({"$ref": "#/shapes/A"})),
        ]);
        let root = serde_json::json!({"$ref": "#/shapes/A"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        assert!(
            rows.is_empty(),
            "a pure alias cycle offers nothing: {:?}",
            rows
        );
    }

    #[test]
    fn mixed_union_reports_the_parent_leaf_and_the_object_variants_children() {
        // The real `customFields[].value` shape: a union of scalar branches
        // and one object branch — both the field itself and the object
        // branch's own fields are offered.
        let shapes = shapes_from(vec![]);
        let root = serde_json::json!({
            "oneOf": [
                {"type": "boolean"},
                {"type": "object", "properties": {"currencyCode": {"type": "string"}}},
                {"type": "null"}
            ]
        });
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let paths: Vec<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        assert!(
            paths.contains(&""),
            "the mixed union's own scalar branches keep the parent as a leaf too: {:?}",
            paths
        );
        assert!(paths.contains(&"currencyCode"));
    }

    #[test]
    fn a_bodyless_in_practice_empty_object_shape_offers_nothing() {
        // Several of a vendor's operations declare a request body as bare
        // `{"type": "object"}`, no `properties` key at all (its
        // `AssessmentListRequest`) — the walker already refuses to invent a
        // leaf for a null shape (no request_body at all); an explicitly
        // empty object must bottom out the same way, not read as one
        // unaccounted leaf at the empty root path.
        let shapes = shapes_from(vec![("EmptyBody", serde_json::json!({"type": "object"}))]);
        let root = serde_json::json!({"$ref": "#/shapes/EmptyBody"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        assert!(
            rows.is_empty(),
            "an empty-object shape offers no leaves, same as a null shape: {:?}",
            rows
        );
    }

    #[test]
    fn an_object_shape_with_an_empty_properties_map_also_offers_nothing() {
        // The same vendor's `ApiKeyInfoRequest`: `{"type": "object", "properties":
        // {}}` — the other spelling of "no properties", a present but empty
        // map rather than an absent key. Must bottom out the same way.
        let shapes = shapes_from(vec![(
            "EmptyBody",
            serde_json::json!({"type": "object", "properties": {}}),
        )]);
        let root = serde_json::json!({"$ref": "#/shapes/EmptyBody"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        assert!(
            rows.is_empty(),
            "an empty `properties: {{}}` offers no leaves either: {:?}",
            rows
        );
    }

    #[test]
    fn a_nested_empty_object_still_offers_its_one_opaque_leaf() {
        // The root-only rule cuts the other way for a vendor's
        // `errorInfo.meta` and Asana's `BatchResponse.body`/`.headers`: a
        // bare `{"type":"object"}` reached through a property is a real
        // value the wire passes through (arbitrary JSON, mapped or omitted
        // with a reason like any other leaf), not an empty body — it must
        // still report its one opaque leaf exactly as before the root-level
        // fix existed.
        let shapes = shapes_from(vec![(
            "Envelope",
            serde_json::json!({"type": "object", "properties": {
                "code": {"type": "string"},
                "meta": {"type": "object"}
            }}),
        )]);
        let root = serde_json::json!({"$ref": "#/shapes/Envelope"});
        let rows = offered_rows(&root, &shapes, &no_dropped());
        let paths: Vec<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        assert!(
            paths.contains(&"meta"),
            "a nested empty object is still one leaf, not zero: {:?}",
            paths
        );
        assert!(paths.contains(&"code"));
    }

    // ── Response selection walk: a method transform is unresolved, not mapped ─

    #[test]
    fn a_quoted_selection_segment_is_followed_to_its_mapped_field() {
        // A vendor's real jobPosting `linkedData` selection, verbatim: `context: $."@context"` next to
        // plain, unquoted sibling keys on the same object.
        let shapes = shapes_from(vec![]);
        let root = serde_json::json!({"type": "object", "properties": {
            "@context": {"type": "string"},
            "@type": {"type": "string"},
            "title": {"type": "string"}
        }});
        let nodes = reconcile::parse_selection(
            r#"context: $."@context" type: $."@type" title description datePosted employmentType"#,
        );
        let mut coverage = SelectionCoverage {
            exact: BTreeMap::new(),
            unresolved_prefixes: Vec::new(),
            open_spreads: Vec::new(),
        };
        let mut seen = Vec::new();
        walk_selection(
            "",
            &nodes,
            &root,
            &shapes,
            &no_dropped(),
            &mut seen,
            &mut coverage,
        );
        assert!(
            coverage.exact.contains_key("@context"),
            "a quoted key must resolve like a plain one: {:?}",
            coverage.exact
        );
        assert!(
            coverage.exact.contains_key("@type"),
            "a second quoted key on the same object must also resolve: {:?}",
            coverage.exact
        );
        assert!(
            coverage.exact.contains_key("title"),
            "an unquoted sibling key must keep resolving too: {:?}",
            coverage.exact
        );
    }

    #[test]
    fn a_quoted_selection_segment_with_an_embedded_dot_is_one_key_not_two() {
        // Synthetic OData shape: the quoted segment's own `.` must not be
        // read as a further path step.
        let shapes = shapes_from(vec![]);
        let root = serde_json::json!({"type": "object", "properties": {
            "@odata.nextLink": {"type": "string"}
        }});
        let nodes = reconcile::parse_selection(r#"nextLink: $."@odata.nextLink""#);
        let mut coverage = SelectionCoverage {
            exact: BTreeMap::new(),
            unresolved_prefixes: Vec::new(),
            open_spreads: Vec::new(),
        };
        let mut seen = Vec::new();
        walk_selection(
            "",
            &nodes,
            &root,
            &shapes,
            &no_dropped(),
            &mut seen,
            &mut coverage,
        );
        assert!(
            coverage.exact.contains_key("@odata.nextLink"),
            "the embedded dot inside quotes must not split into two path segments: {:?}",
            coverage.exact
        );
    }

    #[test]
    fn a_selection_method_the_walk_does_not_know_marks_the_subtree_unresolved() {
        let shapes = shapes_from(vec![]);
        let root = serde_json::json!({"type": "object", "properties": {
            "results": {"type": "object", "properties": {"weird": {"type": "string"}}}
        }});
        let nodes = reconcile::parse_selection("$.results { weird: weird->someUnknownMethod }");
        let mut coverage = SelectionCoverage {
            exact: BTreeMap::new(),
            unresolved_prefixes: Vec::new(),
            open_spreads: Vec::new(),
        };
        let mut seen = Vec::new();
        walk_selection(
            "",
            &nodes,
            &root,
            &shapes,
            &no_dropped(),
            &mut seen,
            &mut coverage,
        );
        assert!(
            coverage
                .unresolved_prefixes
                .iter()
                .any(|(p, _)| p == "results.weird"),
            "expected results.weird to be flagged unresolved, got {:?}",
            coverage.unresolved_prefixes
        );
        assert!(
            coverage.exact.is_empty(),
            "a method-transformed node must not also be reported mapped"
        );
    }

    // ── Request mapped: an expression outside the small grammar is unresolved ─

    #[test]
    fn a_request_body_expression_outside_the_grammar_is_unresolved_not_mapped() {
        let sdl = r#"
type Query {
  op(a: Int!, b: Int!): Widget
}
type Widget {
  sum: Int
}
"#;
        let schema = closure::build_schema(sdl, None).unwrap();
        let top_level_props: std::collections::BTreeSet<String> =
            ["sum".to_string()].into_iter().collect();
        let mut out = RequestMapping {
            mapped: BTreeMap::new(),
            unresolved_prefixes: Vec::new(),
        };
        let request_root =
            serde_json::json!({"type": "object", "properties": {"sum": {"type": "integer"}}});
        let shapes = shapes_from(vec![]);
        request_mapped_with_prefix(
            &top_level_props,
            "$({sum: $args.a + $args.b})",
            &request_root,
            &shapes,
            &no_dropped(),
            &schema,
            "Query.op",
            "",
            &mut out,
        );
        assert!(out.mapped.is_empty());
        assert_eq!(
            out.unresolved_prefixes,
            vec![(
                "sum".to_string(),
                "request expression not parsed: arithmetic".to_string()
            )]
        );
    }

    #[test]
    fn a_request_field_missing_from_the_forwarded_input_type_is_unaccounted() {
        // A synthetic nested request omission: the wire schema wants
        // `filter.range.{min,max}`, but the declared input type only has
        // `min` — `max` must classify unaccounted at the nested path,
        // independently of anything on the response side.
        let sdl = r#"
type Query {
  op(filter: RangeFilterInput!): Widget
}
input RangeFilterInput {
  range: RangeInput!
}
input RangeInput {
  min: Int!
}
type Widget {
  sum: Int
}
"#;
        let schema = closure::build_schema(sdl, None).unwrap();
        let shapes = shapes_from(vec![]);
        let wire_filter = serde_json::json!({"type": "object", "properties": {
            "range": {"type": "object", "properties": {
                "min": {"type": "integer"},
                "max": {"type": "integer"}
            }}
        }});
        let mut out: BTreeMap<String, bool> = BTreeMap::new();
        let mut seen = Vec::new();
        mark_forward(
            "filter".to_string(),
            &wire_filter,
            &shapes,
            &no_dropped(),
            "RangeFilterInput",
            &schema,
            "",
            &mut seen,
            &mut out,
        );
        assert_eq!(out.get("filter.range.min"), Some(&false));
        assert!(out.get("filter.range.max").is_none(), "max has no matching GraphQL field and must be left for the caller to classify unaccounted");
    }

    // ── Dropped-construct resolution: exact reference, never a same-name guess ─

    fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    struct TempWorkspace(PathBuf);
    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "obligations-test-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempWorkspace(dir)
        }
    }
    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn colliding_component_names_and_a_generalised_const_prefix_items_case() {
        let ws = TempWorkspace::new("collide");
        let dir = &ws.0;
        // `Foo.Bar` and `Foo_Bar` both sanitise to `Foo_Bar`; the second
        // component must take the collision suffix `Foo_Bar2`, and a
        // by-name lookup must resolve each to its own reference, not a
        // same-name guess.
        // `ShapeSet::name_for` only names a component when an operation's
        // `$ref` actually resolves to it — declaring both under
        // `components.schemas` is not enough by itself, so each is
        // referenced from its own operation, in the order that must
        // produce the collision (`Foo.Bar` first, so it keeps the bare
        // `Foo_Bar`; `Foo_Bar` second, so it takes the `Foo_Bar2` suffix).
        fn op_returning(schema_ref: &str) -> Value {
            serde_json::json!({
                "get": {
                    "operationId": schema_ref,
                    "responses": {"200": {"description": "ok", "content": {"application/json": {
                        "schema": {"$ref": format!("#/components/schemas/{}", schema_ref)}
                    }}}}
                }
            })
        }
        let spec = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "t", "version": "1"},
            "paths": {
                "/foo": op_returning("Foo.Bar"),
                "/bar": op_returning("Foo_Bar"),
            },
            "components": {"schemas": {
                "Foo.Bar": {"type": "string", "const": "tagged"},
                "Foo_Bar": {"type": "array", "prefixItems": [{"type": "string"}]},
            }}
        });
        write(dir, "openapi.json", &spec.to_string());
        write(
            dir,
            ".factory/sources.lock.yaml",
            "contract_version: 1\nsources:\n  - kind: openapi\n    version: \"3.1.0\"\n    path: openapi.json\n",
        );
        let dropped = build_dropped_constructs(dir);
        assert_eq!(dropped.status, "ran");
        assert_eq!(
            dropped.check("Foo_Bar"),
            Some("const".to_string()),
            "the first-registered name keeps the bare suffix"
        );
        assert_eq!(dropped.check("Foo_Bar2"), Some("prefixItems".to_string()), "the collision-suffixed name resolves to its own, different schema — a same-name lookup would have found the wrong one or nothing");
    }

    /// `.factory` reads go through custody (ADR 0025): a symlinked
    /// `.factory/workspace.yaml`, or a source document pinned at a symlinked
    /// `.factory` path, is refused rather than followed to its target.
    /// A one-operation workspace whose response is `#/shapes/Store` (from
    /// `shapes`) and whose connector selection is `selection`.
    fn one_op_workspace(
        tag: &str,
        shapes: Value,
        selection: &str,
    ) -> (TempWorkspace, &'static str) {
        let ws = TempWorkspace::new(tag);
        let dir = &ws.0;
        let op = "get:/stores/{id}";
        write(dir, ".factory/workspace.yaml", "contract_version: 1\nservice: shop\ndirectory: shop\ntype_prefix: Shop\nfield_prefix: shop\n");
        write(
            dir,
            "shop.graphql",
            &format!(
                "extend schema\n  @link(url: \"https://specs.apollo.dev/connect/v0.3\", import: [\"@source\", \"@connect\"])\n\n@source(name: \"shop\", http: {{ baseURL: \"{{{{BASE_URL}}}}\" }})\n\ntype Query {{\n  shop_store(id: ID!): Shop_Store\n    @connect(source: \"shop\", http: {{ GET: \"/stores/{{$args.id}}\" }}, selection: \"{}\")\n}}\n",
                selection
            ),
        );
        write(
            dir,
            ".factory/inventory.json",
            &serde_json::json!({
                "contract_version": 1,
                "api": {"title": "Shop", "base_urls": ["https://shop.test"]},
                "operations": [{"key": op, "operation_id": "getStore", "method": "GET", "path": "/stores/{id}",
                    "response": {"status": "200", "shape_ref": "#/shapes/Store"}}],
                "shapes": shapes
            })
            .to_string(),
        );
        (ws, op)
    }

    fn class_of(report: &Report, path: &str) -> Option<String> {
        report
            .response
            .iter()
            .find(|r| r.path == path)
            .map(|r| r.class.label())
    }

    /// A dictionary is one offered row. A selection that reaches into it
    /// (`labels { a }`) used to leave that row `unaccounted`: the walk found
    /// no `a` property on the map and recorded nothing.
    #[test]
    fn a_selection_inside_a_dictionary_maps_its_row() {
        let shapes = serde_json::json!({"Store": {"type": "object", "properties": {
            "id": {"type": "string"},
            "labels": {"type": "object", "additionalProperties": {"type": "string"}}
        }}});
        let (ws, op) = one_op_workspace("dict", shapes.clone(), "id labels { a }");
        let report = build(&ws.0, op).unwrap();
        assert_eq!(class_of(&report, "labels").as_deref(), Some("mapped"));
        // Not selected at all, it is still unaccounted.
        let (ws, op) = one_op_workspace("dict-unselected", shapes, "id");
        assert_eq!(
            class_of(&build(&ws.0, op).unwrap(), "labels").as_deref(),
            Some("unaccounted")
        );
    }

    /// A bare `$` selection returns the whole response (ADR 0105). The walk
    /// skipped every node with no path step, so AppWorld's `[String]`
    /// fields over a root array of strings (spotify `get:/spotify/genres`,
    /// D-0015) reported `[]` unaccounted, and amazon recorded an omit to
    /// quiet it.
    #[test]
    fn a_bare_dollar_selection_maps_a_root_array_of_scalars() {
        let shapes = serde_json::json!({"Store": {"type": "array", "items": {"type": "string"}}});
        let (ws, op) = one_op_workspace("bare-dollar-scalars", shapes.clone(), "$");
        let report = build(&ws.0, op).unwrap();
        assert_eq!(class_of(&report, "[]").as_deref(), Some("mapped (json)"));
        assert_eq!(report.response_counts().unaccounted, 0);
        // A selection that reads nothing still leaves the row unaccounted.
        let (ws, op) = one_op_workspace("bare-dollar-scalars-none", shapes, "");
        assert_eq!(
            class_of(&build(&ws.0, op).unwrap(), "[]").as_deref(),
            Some("unaccounted")
        );
    }

    /// The same over a root array of objects: every item path is inside the
    /// value `$` returns whole.
    #[test]
    fn a_bare_dollar_selection_maps_a_root_array_of_objects() {
        let shapes = serde_json::json!({
            "Store": {"type": "array", "items": {"$ref": "#/shapes/Item"}},
            "Item": {"type": "object", "properties": {
                "id": {"type": "string"},
                "tags": {"type": "array", "items": {"type": "string"}}
            }}
        });
        let (ws, op) = one_op_workspace("bare-dollar-objects", shapes, "$");
        let report = build(&ws.0, op).unwrap();
        assert_eq!(class_of(&report, "[].id").as_deref(), Some("mapped (json)"));
        assert_eq!(
            class_of(&report, "[].tags[]").as_deref(),
            Some("mapped (json)")
        );
        assert_eq!(report.response_counts().unaccounted, 0);
    }

    /// A bare `$` over a root object covers its top-level properties too:
    /// the empty path is a prefix of every path, not only of `[…]` ones.
    #[test]
    fn a_bare_dollar_selection_maps_a_root_object() {
        let shapes = serde_json::json!({"Store": {"type": "object", "properties": {
            "id": {"type": "string"},
            "owner": {"type": "object", "properties": {"name": {"type": "string"}}}
        }}});
        let (ws, op) = one_op_workspace("bare-dollar-object", shapes.clone(), "$");
        let report = build(&ws.0, op).unwrap();
        assert_eq!(class_of(&report, "id").as_deref(), Some("mapped (json)"));
        assert_eq!(
            class_of(&report, "owner.name").as_deref(),
            Some("mapped (json)")
        );
        // The unresolved-prefix test takes the same empty prefix: a method
        // on the bare `$` leaves a root object's own properties unresolved,
        // not unaccounted.
        let (ws, op) = one_op_workspace("bare-dollar-object-method", shapes, "$->first");
        let report = build(&ws.0, op).unwrap();
        for path in ["id", "owner.name"] {
            assert!(
                class_of(&report, path)
                    .as_deref()
                    .is_some_and(|c| c.starts_with("unresolved")),
                "{} is {:?}",
                path,
                class_of(&report, path)
            );
        }
    }

    /// `$ { … }` walks its sub-selection from the root: what it names is
    /// mapped, what it leaves out stays unaccounted; and a method on the
    /// bare `$` marks the whole response unresolved, never mapped.
    #[test]
    fn a_rooted_dollar_with_a_sub_selection_or_a_method_is_not_a_blanket_map() {
        let shapes = serde_json::json!({
            "Store": {"type": "array", "items": {"$ref": "#/shapes/Item"}},
            "Item": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"}
            }}
        });
        let (ws, op) = one_op_workspace("rooted-sub", shapes.clone(), "$ { id }");
        let report = build(&ws.0, op).unwrap();
        assert_eq!(class_of(&report, "[].id").as_deref(), Some("mapped"));
        assert_eq!(class_of(&report, "[].name").as_deref(), Some("unaccounted"));
        let (ws, op) = one_op_workspace("rooted-method", shapes, "$->first");
        let report = build(&ws.0, op).unwrap();
        for path in ["[].id", "[].name"] {
            assert!(
                class_of(&report, path)
                    .as_deref()
                    .is_some_and(|c| c.starts_with("unresolved")),
                "{} is {:?}",
                path,
                class_of(&report, path)
            );
        }
    }

    /// A self-referential field was dropped from the offered surface
    /// entirely: `parent: Store` inside `Store` produced no row, so a schema
    /// that drops it passed and one that maps it was never checked. It is now
    /// one row, mapped when the selection reaches it or beneath it.
    #[test]
    fn a_self_referential_field_is_offered_as_one_row() {
        let shapes = serde_json::json!({"Store": {"type": "object", "properties": {
            "id": {"type": "string"},
            "parent": {"$ref": "#/shapes/Store"}
        }}});
        let (ws, op) = one_op_workspace("recursive-unmapped", shapes.clone(), "id");
        assert_eq!(
            class_of(&build(&ws.0, op).unwrap(), "parent").as_deref(),
            Some("unaccounted")
        );
        let (ws, op) = one_op_workspace("recursive-mapped", shapes, "id parent { id }");
        assert_eq!(
            class_of(&build(&ws.0, op).unwrap(), "parent").as_deref(),
            Some("mapped")
        );
    }

    /// The request-side fallback: a body property the connector never sends
    /// is `unaccounted`. Changing that final branch to `Mapped` must fail the
    /// suite (PR 65 review, T5: today all 590 tests stayed green).
    #[test]
    fn a_request_property_no_body_sends_is_unaccounted() {
        let ws = TempWorkspace::new("req-unaccounted");
        let dir = &ws.0;
        let op = "post:/notes";
        write(dir, ".factory/workspace.yaml", "contract_version: 1\nservice: shop\ndirectory: shop\ntype_prefix: Shop\nfield_prefix: shop\n");
        write(
            dir,
            ".factory/inventory.json",
            &serde_json::json!({
                "contract_version": 1,
                "api": {"title": "Shop", "base_urls": ["https://shop.test"]},
                "operations": [{"key": op, "operation_id": "createNote", "method": "POST", "path": "/notes",
                    "request_body": {"shape_ref": "#/shapes/NoteBody"},
                    "response": {"status": "200", "shape_ref": "#/shapes/Note"}}],
                "shapes": {
                    "NoteBody": {"type": "object", "properties": {"text": {"type": "string"}, "pinned": {"type": "boolean"}}},
                    "Note": {"type": "object", "properties": {"id": {"type": "string"}}}
                }
            })
            .to_string(),
        );
        write(
            dir,
            "shop.graphql",
            r#"extend schema
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "shop", http: { baseURL: "{{BASE_URL}}" })

type Shop_Note { id: ID }

type Mutation {
  shop_createNote(text: String): Shop_Note
    @connect(source: "shop", http: { POST: "/notes", body: "text: $args.text" }, selection: "id")
}
"#,
        );
        let report = build(dir, op).unwrap();
        let class = |path: &str| {
            report
                .request
                .iter()
                .find(|r| r.path == path)
                .map(|r| r.class.clone())
                .unwrap()
        };
        assert_eq!(class("text"), Class::Mapped { json: false });
        assert_eq!(class("pinned"), Class::Unaccounted);
    }

    /// Two root fields serve one POST. Each body must be read with its own
    /// field's argument types: `.first()` paired `alpha`'s `AlphaInput { x y }`
    /// with `zeta`'s body, so `payload.y` read as mapped though no body sends
    /// it (PR 65 review, T4).
    #[test]
    fn two_root_fields_on_one_op_key_each_read_their_own_body_with_their_own_types() {
        let ws = TempWorkspace::new("two-fields");
        let dir = &ws.0;
        let op = "post:/things";
        write(dir, ".factory/workspace.yaml", "contract_version: 1\nservice: shop\ndirectory: shop\ntype_prefix: Shop\nfield_prefix: shop\n");
        write(
            dir,
            ".factory/inventory.json",
            &serde_json::json!({
                "contract_version": 1,
                "api": {"title": "Shop", "base_urls": ["https://shop.test"]},
                "operations": [{"key": op, "operation_id": "createThing", "method": "POST", "path": "/things",
                    "request_body": {"shape_ref": "#/shapes/ThingBody"},
                    "response": {"status": "200", "shape_ref": "#/shapes/Thing"}}],
                "shapes": {
                    "ThingBody": {"type": "object", "properties": {"payload": {"type": "object", "properties": {
                        "x": {"type": "string"}, "y": {"type": "string"}}}}},
                    "Thing": {"type": "object", "properties": {"id": {"type": "string"}}}
                }
            })
            .to_string(),
        );
        write(
            dir,
            "shop.graphql",
            r#"extend schema
  @link(url: "https://specs.apollo.dev/connect/v0.3", import: ["@source", "@connect"])

@source(name: "shop", http: { baseURL: "{{BASE_URL}}" })

input Shop_ZetaInput { x: String }
input Shop_AlphaInput { x: String y: String }
type Shop_Thing { id: ID }

type Mutation {
  shop_zeta(input: Shop_ZetaInput!): Shop_Thing
    @connect(source: "shop", http: { POST: "/things", body: "payload: $args.input" }, selection: "id")
  shop_alpha(input: Shop_AlphaInput!): Shop_Thing
    @connect(source: "shop", http: { POST: "/things", body: "unrelated: $args.input" }, selection: "id")
}
"#,
        );
        let report = build(dir, op).unwrap();
        let class = |path: &str| {
            report
                .request
                .iter()
                .find(|r| r.path == path)
                .map(|r| r.class.label())
                .unwrap_or_else(|| panic!("no request row {}", path))
        };
        assert_eq!(class("payload.x"), "mapped");
        assert_eq!(class("payload.y"), "unaccounted", "no body sends payload.y");
    }

    /// Directive arguments may carry non-ASCII text outside a string (a
    /// JSONSelection key, a comment). The scanner stepped byte by byte and
    /// sliced at a non-boundary, which panicked; it now finds the value.
    #[test]
    fn find_value_reads_past_non_ascii_text_outside_strings() {
        // `é` and `café` sit at depth 0, outside any string, before the key.
        let block = " message: $.erreur_détail  # café\n extensions: \"$.code\" ";
        assert_eq!(find_value(block, "extensions").as_deref(), Some("$.code"));
        let args = format!("errors: {{{}}}", block);
        assert_eq!(find_value(&args, "errors").as_deref(), Some(block));
        assert_eq!(
            find_value(
                "description: \"Ünïcödé — ok\", isSuccess: \"$.ok\"",
                "isSuccess"
            )
            .as_deref(),
            Some("$.ok")
        );
    }

    /// An EmptyResponse 204 offers one row at the empty root path, which no
    /// `omits` entry could name (the schema requires a non-blank path). `.`
    /// names the root: with it the operation reaches `--check` pass.
    #[test]
    fn a_root_omit_accounts_for_an_empty_response_204() {
        let ws = TempWorkspace::new("root-omit");
        let dir = &ws.0;
        let op = "delete:/stores/{store_id}";
        write(dir, ".factory/workspace.yaml", "contract_version: 1\nservice: shop\ndirectory: shop\ntype_prefix: Shop\nfield_prefix: shop\n");
        write(dir, "shop.graphql", "type Query { shop_ping: String }\n");
        write(
            dir,
            ".factory/inventory.json",
            &serde_json::json!({
                "contract_version": 1,
                "api": {"title": "Shop", "base_urls": ["https://shop.test"]},
                "operations": [{"key": op, "operation_id": "deleteStore", "method": "DELETE", "path": "/stores/{store_id}",
                    "response": {"status": "204", "shape_ref": "#/shapes/EmptyResponse"}}],
                "shapes": {"EmptyResponse": {"description": "Empty Response"}}
            })
            .to_string(),
        );
        let decisions = |omits: Value| {
            serde_json::json!({"contract_version": 1, "decisions": [{
                "id": "D-0001", "title": "No body to map", "status": "resolved", "date": "2026-09-23",
                "resolution": {"decision": "a 204 returns nothing"}, "omits": omits
            }]})
            .to_string()
        };
        write(
            dir,
            ".factory/decisions.json",
            &decisions(serde_json::json!([])),
        );
        let before = build(dir, op).unwrap();
        assert_eq!(before.response.len(), 1);
        assert_eq!(before.response[0].path, "");
        assert_eq!(
            before.check_failure().as_deref(),
            Some("response unaccounted 1")
        );

        write(
            dir,
            ".factory/decisions.json",
            &decisions(
                serde_json::json!([{"operation": op, "direction": "response", "path": ".", "reason": "editorial"}]),
            ),
        );
        let after = build(dir, op).unwrap();
        assert!(matches!(
            after.response[0].class,
            Class::OmittedDecided { .. }
        ));
        assert_eq!(after.check_failure(), None);
    }

    /// A source pinned as `./.factory/spec.json` is still a `.factory` path:
    /// the leading `./` used to skip the custody check. And a sources.lock
    /// that cannot be read is reported as unreadable, not as absent.
    #[cfg(unix)]
    #[test]
    fn a_dot_slash_factory_path_goes_through_custody_and_an_unreadable_lock_says_so() {
        let ws = TempWorkspace::new("dot-slash");
        let dir = &ws.0;
        let outside = TempWorkspace::new("dot-slash-outside");
        let spec = serde_json::json!({
            "openapi": "3.1.0", "info": {"title": "t", "version": "1"},
            "paths": {"/foo": {"get": {"operationId": "foo", "responses": {"200": {"description": "ok",
                "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Foo"}}}}}}}},
            "components": {"schemas": {"Foo": {"type": "string", "const": "x"}}}
        });
        write(&outside.0, "spec.json", &spec.to_string());
        std::fs::create_dir_all(dir.join(".factory")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("spec.json"), dir.join(".factory/spec.json"))
            .unwrap();
        write(
            dir,
            ".factory/sources.lock.yaml",
            "contract_version: 1\nsources:\n  - kind: openapi\n    version: \"3.1.0\"\n    path: ./.factory/spec.json\n",
        );
        // The helpers themselves: `not_followed` happens to reject this
        // working-copy path today, but the custody decision is theirs.
        assert!(
            read_workspace_text(dir, "./.factory/spec.json").is_err(),
            "./.factory/spec.json must not be followed"
        );
        assert!(!workspace_file_exists(dir, "./.factory/spec.json"));
        assert!(read_workspace_text(dir, ".factory/spec.json").is_err());

        std::fs::remove_file(dir.join(".factory/sources.lock.yaml")).unwrap();
        std::fs::create_dir_all(dir.join(".factory/sources.lock.yaml")).unwrap();
        assert_eq!(
            build_dropped_constructs(dir).status,
            "skipped (sources.lock.yaml unreadable)"
        );
    }

    /// `--check` fails closed: one unresolved row with nothing unaccounted
    /// is a failure, and the reason names the direction and the count.
    #[test]
    fn check_fails_on_an_unresolved_row_even_with_nothing_unaccounted() {
        let row = |class: Class, direction: Direction| Row {
            path: "a".into(),
            direction,
            class,
            structurally_consumed: false,
            note: None,
        };
        let mut report = Report {
            op_key: "get:/a".into(),
            response: vec![row(Class::Mapped { json: false }, Direction::Response)],
            request: vec![row(
                Class::Unresolved("nested body".into()),
                Direction::Request,
            )],
            behaviour: Vec::new(),
            dropped_status: DroppedStatus::Ran,
            stale: Vec::new(),
        };
        assert_eq!(report.unaccounted_total(), 0);
        assert_eq!(
            report.check_failure().as_deref(),
            Some("request unresolved 1")
        );
        report.request = vec![row(Class::Mapped { json: false }, Direction::Request)];
        assert_eq!(report.check_failure(), None);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_factory_inputs_are_refused_not_followed() {
        let ws = TempWorkspace::new("custody");
        let dir = &ws.0;
        let outside = TempWorkspace::new("custody-outside");
        write(&outside.0, "workspace.yaml", "type_prefix: Stolen\n");
        let spec = serde_json::json!({
            "openapi": "3.1.0", "info": {"title": "t", "version": "1"},
            "paths": {"/foo": {"get": {"operationId": "foo", "responses": {"200": {"description": "ok",
                "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Foo"}}}}}}}},
            "components": {"schemas": {"Foo": {"type": "string", "const": "x"}}}
        });
        write(&outside.0, "spec.json", &spec.to_string());
        std::fs::create_dir_all(dir.join(".factory")).unwrap();
        std::os::unix::fs::symlink(
            outside.0.join("workspace.yaml"),
            dir.join(".factory/workspace.yaml"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.0.join("spec.json"), dir.join(".factory/spec.json"))
            .unwrap();
        write(
            dir,
            ".factory/sources.lock.yaml",
            "contract_version: 1\nsources:\n  - kind: openapi\n    version: \"3.1.0\"\n    path: .factory/spec.json\n",
        );
        assert_eq!(
            type_prefix(dir),
            None,
            "a symlinked workspace.yaml must not be read"
        );
        let dropped = build_dropped_constructs(dir);
        assert_ne!(
            dropped.status, "ran",
            "a symlinked .factory source must not be read"
        );
        assert!(dropped.by_name.is_empty());
    }

    /// A workspace whose sources are docs pages and a probe pins no
    /// description document: the sub-check is skipped and says why.
    #[test]
    fn a_docs_only_workspace_has_no_source_document_and_the_skip_is_reported_without_erroring() {
        let ws = TempWorkspace::new("docs-only");
        write(
            &ws.0,
            ".factory/sources.lock.yaml",
            "contract_version: 1\nsources:\n  - kind: docs\n    url: \"https://api.example.test/docs\"\n    retrieved_at: \"2026-09-08T20:05:00Z\"\n  - kind: probe\n    base_url: \"https://api.example.test\"\n    retrieved_at: \"2026-09-10T21:07:08Z\"\n",
        );
        let dropped = build_dropped_constructs(&ws.0);
        assert_eq!(
            dropped.status,
            "skipped (no openapi/swagger source document)"
        );
        assert!(dropped.by_name.is_empty());
    }

    #[test]
    fn gitea_swagger_resolves_issue_through_the_normalised_reference_space() {
        let dropped = build_dropped_constructs(&pilot("gitea"));
        assert_eq!(dropped.status, "ran");
        let issue = dropped.by_name.get("Issue").expect(
            "Issue must resolve via spec::read (Swagger-normalised), not sources::load_document (raw #/definitions/...)",
        );
        assert_eq!(
            get_str(issue, "description"),
            Some("Issue represents an issue in a repository")
        );
        assert!(
            dropped.check("Issue").is_none(),
            "Issue itself carries none of the dropped constructs"
        );
    }

    // The full pipeline against the target-owned pilots (a docs-only one,
    // and one with a real deficit) is pinned in that target's suite.

    // ── Stale omits (ADR 0103) ────────────────────────────────────────────────

    fn stale_row(path: &str, class: Class, consumed: bool) -> Row {
        Row {
            path: path.into(),
            direction: Direction::Response,
            class,
            structurally_consumed: consumed,
            note: None,
        }
    }

    fn entry(path: &str, status: &str, reason: Reason) -> OmitEntry {
        OmitEntry {
            decision: "D-0001".into(),
            status: status.into(),
            operation: "get:/a".into(),
            direction: Direction::Response,
            path: path.into(),
            reason,
        }
    }

    fn whys(omits: &[OmitEntry], rows: &[Row]) -> Vec<Staleness> {
        stale_for(omits, "get:/a", Direction::Response, rows)
            .into_iter()
            .map(|s| s.why)
            .collect()
    }

    #[test]
    fn stale_for_reports_each_way_an_entry_goes_stale() {
        let mapped = Class::Mapped { json: false };
        let omitted = Class::OmittedDecided {
            reason: Reason::Editorial,
        };
        let rows = vec![
            stale_row("a.x", mapped.clone(), false),
            stale_row("a.y", mapped.clone(), false),
            stale_row("b", omitted.clone(), true),
            stale_row("c", omitted.clone(), false),
        ];
        let e = |p: &str| entry(p, "resolved", Reason::Editorial);
        assert_eq!(whys(&[e("a")], &rows), vec![Staleness::Mapped]);
        assert_eq!(whys(&[e("a.x")], &rows), vec![Staleness::Mapped]);
        assert_eq!(whys(&[e("b")], &rows), vec![Staleness::Consumed]);
        assert_eq!(whys(&[e("zz")], &rows), vec![Staleness::NotOffered]);
        // In use: c needs it; the root covers c too.
        assert!(whys(&[e("c")], &rows).is_empty());
        assert!(whys(&[e(ROOT_OMIT)], &rows).is_empty());
        // A reviewed `consumed` entry on a row the envelope reads is not stale.
        assert!(whys(&[entry("b", "resolved", Reason::Consumed)], &rows).is_empty());
        // Only a resolved entry is checked, and only on its own operation.
        assert!(whys(&[entry("a", "open", Reason::Editorial)], &rows).is_empty());
        assert!(whys(&[entry("a", "superseded", Reason::Editorial)], &rows).is_empty());
        let mut other = e("zz");
        other.operation = "get:/b".into();
        assert!(whys(&[other], &rows).is_empty());
    }

    #[test]
    fn stale_for_counts_a_row_it_cannot_settle_as_needing_the_entry() {
        // An unresolved or unverified row might really be unmapped, so the
        // entry is not known to be stale; a transport-expansion finding is
        // not an offered row, so it covers nothing.
        let rows = vec![
            stale_row("u", Class::Unresolved("x".into()), false),
            stale_row("v", Class::UnverifiedDefault { target: "T".into() }, false),
            stale_row(
                "t",
                Class::TransportExpansionMissing {
                    boundary: "t".into(),
                    sent: vec![],
                },
                false,
            ),
        ];
        let e = |p: &str| entry(p, "resolved", Reason::Editorial);
        assert!(whys(&[e("u")], &rows).is_empty());
        assert!(whys(&[e("v")], &rows).is_empty());
        assert_eq!(whys(&[e("t")], &rows), vec![Staleness::NotOffered]);
    }
}

//! Conformance waivers — `waivers[]` in selection.yaml.
//!
//! `graphos-factory-core validate` reports a body it could not judge as
//! `unchecked` (the oracle had nothing to compare against: no documented
//! shape, no JSON body) or `unmatched` (it could not even find the operation
//! or read the body). Both are honest states; neither is a pass. When the
//! engineer has looked and accepted one — "Gitea documents no 404 body; the
//! fixture carries the one observed live" — a waiver records that in the
//! same grammar as `overrides[]`: a target, the status being accepted, a
//! reason, an optional context and decision (ADR 0113) and an optional expiry, so the acceptance survives the
//! session that made it and the next agent can read why.
//!
//! ```yaml
//! waivers:
//!   - where: "tests/fixtures/mappings/repo_not_found.json"   # one body
//!     status: unchecked
//!     reason: "…"
//!     decision: D-0010
//!   - operation: "get:/repos/{owner}/{repo}/issues"           # every such body of one operation
//!     status: unchecked
//!     reason: "…"
//!     decision: D-0010
//!     until: "the vendor documents the error schema"
//! ```
//!
//! A unit-suite body is named `tests/<suite>.connector.yaml#<entry name>`
//! (validate prints it with ` › `; both spellings match). `status` says which
//! state is accepted, so a waiver written for `unchecked` never hides a later
//! `unmatched`.

use crate::json::{get_arr, get_str};
use serde_json::Value;

/// The statuses a waiver may accept.
pub const STATUSES: &[&str] = &["unchecked", "unmatched"];

#[derive(Clone, Debug, PartialEq)]
pub struct Waiver {
    pub where_: Option<String>,
    pub operation: Option<String>,
    pub status: String,
    pub reason: Option<String>,
    pub decision: Option<String>,
    /// Background beyond the reason (`codify --context`, ADR 0113).
    pub context: Option<String>,
    pub until: Option<String>,
    pub expires: Option<String>,
}

impl Waiver {
    /// The `where` or `operation` the waiver names, for messages.
    pub fn target(&self) -> String {
        self.where_
            .clone()
            .or_else(|| self.operation.clone())
            .unwrap_or_default()
    }

    /// Does this waiver accept a result at `where_` (a fixture path or
    /// `suite › entry`) for `operation` with `status`?
    pub fn matches(&self, where_: &str, operation: Option<&str>, status: &str) -> bool {
        if self.status != status {
            return false;
        }
        match (&self.where_, &self.operation) {
            (Some(w), _) => normalize_where(w) == normalize_where(where_),
            (None, Some(op)) => operation == Some(op.as_str()),
            (None, None) => false,
        }
    }
}

/// `tests/x.connector.yaml#entry` and `tests/x.connector.yaml › entry` name
/// the same body; a leading `./` is noise.
pub fn normalize_where(w: &str) -> String {
    let w = w.trim().trim_start_matches("./");
    match w.split_once(" › ") {
        Some((file, entry)) => format!("{}#{}", file.trim(), entry.trim()),
        None => w.to_string(),
    }
}

pub fn read_waivers(selection: &Value) -> Vec<Waiver> {
    get_arr(selection, "waivers")
        .into_iter()
        .flatten()
        .map(|w| Waiver {
            where_: get_str(w, "where").map(str::to_string),
            operation: get_str(w, "operation").map(str::to_string),
            status: get_str(w, "status").unwrap_or("").to_string(),
            reason: get_str(w, "reason").map(str::to_string),
            decision: get_str(w, "decision").map(str::to_string),
            context: get_str(w, "context").map(str::to_string),
            until: get_str(w, "until").map(str::to_string),
            expires: get_str(w, "expires").map(str::to_string),
        })
        .collect()
}

/// Is a waiver past its `expires` date (YYYY-MM-DD, compared to today)?
pub fn expired(w: &Waiver, today: &str) -> bool {
    w.expires.as_deref().map(|d| d < today).unwrap_or(false)
}

/// The waiver as it is written back to selection.yaml.
pub fn to_value(w: &Waiver) -> Value {
    let mut pairs: Vec<(&str, Value)> = Vec::new();
    if let Some(x) = &w.where_ {
        pairs.push(("where", Value::from(x.as_str())));
    }
    if let Some(x) = &w.operation {
        pairs.push(("operation", Value::from(x.as_str())));
    }
    pairs.push(("status", Value::from(w.status.as_str())));
    if let Some(x) = &w.reason {
        pairs.push(("reason", Value::from(x.as_str())));
    }
    if let Some(x) = &w.decision {
        pairs.push(("decision", Value::from(x.as_str())));
    }
    if let Some(x) = &w.context {
        pairs.push(("context", Value::from(x.as_str())));
    }
    if let Some(x) = &w.until {
        pairs.push(("until", Value::from(x.as_str())));
    }
    if let Some(x) = &w.expires {
        pairs.push(("expires", Value::from(x.as_str())));
    }
    crate::json::object(pairs)
}

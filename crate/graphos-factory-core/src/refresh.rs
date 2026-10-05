//! Replaying a pinned document's patches over a newer vendor document.
//!
//! A refresh (`sources refresh`) replaces the upstream copy with the
//! vendor's new bytes and rebuilds the working copy from them plus the
//! recorded `patches[]`. Each patch is replayed in order against the
//! evolving document and given one of three fates:
//!
//! - **re-applied** — the target is as the patch left it (an `add` has no
//!   target yet, a `replace` or `remove` still finds its `was`), so the
//!   patch is applied and kept on the entry with its reason, decision and
//!   verification;
//! - **obsolete** — the vendor's document already says what the patch
//!   produced (an `add` whose value is present, a `replace` whose target
//!   already holds `value`, a `remove` whose target is gone): nothing is
//!   applied and the patch is dropped, its reason recorded in the decision;
//! - **conflict** — the vendor changed the target in a way the patch did
//!   not anticipate (a `replace` or `remove` whose `was` no longer matches,
//!   an `add` the vendor now defines differently, a target or parent that
//!   moved): nothing is applied, the patch is dropped, and the refresh exits
//!   3 so the engineer re-applies the intent by hand and codifies it.
//!
//! A patch with no `was` key (hand-written) is re-applied when its target
//! exists — there is nothing to compare — and the report says so. An
//! explicit `null` in `value` or `was` is a value like any other (the spec
//! says `example: null`), never a missing key.
//!
//! An `add` at an array position (`/required/-`, `/required/1`) has no
//! member to compare, so it is obsolete when the array already contains the
//! value and re-applied otherwise.

use crate::json::{compact, field, get, get_str};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    Reapplied,
    Obsolete,
    Conflict,
}

impl Fate {
    pub fn as_str(&self) -> &'static str {
        match self {
            Fate::Reapplied => "reapplied",
            Fate::Obsolete => "obsolete",
            Fate::Conflict => "conflict",
        }
    }
}

/// One recorded patch after replay.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub index: usize,
    pub patch: Value,
    pub fate: Fate,
    pub why: String,
}

impl Outcome {
    pub fn to_value(&self) -> Value {
        crate::json::object(vec![
            ("index", Value::from(self.index)),
            ("op", Value::from(get_str(&self.patch, "op").unwrap_or("?"))),
            (
                "path",
                Value::from(get_str(&self.patch, "path").unwrap_or("")),
            ),
            ("fate", Value::from(self.fate.as_str())),
            ("why", Value::from(self.why.as_str())),
            (
                "reason",
                get(&self.patch, "reason").cloned().unwrap_or(Value::Null),
            ),
            (
                "decision",
                get(&self.patch, "decision").cloned().unwrap_or(Value::Null),
            ),
        ])
    }
}

/// The replayed document and every patch's fate.
#[derive(Debug, Clone)]
pub struct Replay {
    pub document: Value,
    pub outcomes: Vec<Outcome>,
}

impl Replay {
    pub fn count(&self, fate: Fate) -> usize {
        self.outcomes.iter().filter(|o| o.fate == fate).count()
    }
    /// The patches to keep on the entry, verbatim (reason, decision and
    /// verification included), in their recorded order.
    pub fn kept(&self) -> Vec<Value> {
        self.outcomes
            .iter()
            .filter(|o| o.fate == Fate::Reapplied)
            .map(|o| o.patch.clone())
            .collect()
    }
}

/// A value for a report line: compact JSON, cut when long.
fn short(v: &Value) -> String {
    let s = compact(v);
    if s.chars().count() > 72 {
        let cut: String = s.chars().take(69).collect();
        format!("{}…", cut)
    } else {
        s
    }
}

/// Replay `patches` (the entry's `patches[]`, in order) over `upstream`.
pub fn replay(upstream: &Value, patches: &[Value]) -> Replay {
    let mut doc = upstream.clone();
    let mut outcomes = Vec::with_capacity(patches.len());
    for (index, patch) in patches.iter().enumerate() {
        let (fate, why) = classify(&doc, patch);
        let (fate, why) = if fate == Fate::Reapplied {
            match crate::patch::apply(&doc, std::slice::from_ref(patch)) {
                Ok(next) => {
                    doc = next;
                    (fate, why)
                }
                Err(e) => (Fate::Conflict, format!("does not apply: {}", e)),
            }
        } else {
            (fate, why)
        };
        outcomes.push(Outcome {
            index,
            patch: patch.clone(),
            fate,
            why,
        });
    }
    Replay {
        document: doc,
        outcomes,
    }
}

/// The fate of one patch against the document as it stands, without
/// applying it.
fn classify(doc: &Value, patch: &Value) -> (Fate, String) {
    let op = get_str(patch, "op").unwrap_or("");
    let path = get_str(patch, "path").unwrap_or("");
    let toks = match crate::patch::tokens(path) {
        Ok(t) => t,
        Err(e) => return (Fate::Conflict, e),
    };
    let current = crate::patch::resolve(doc, &toks);
    // `field`, not `get`: an explicit null is a value the patch carries.
    let value = field(patch, "value");
    let was = field(patch, "was");
    match op {
        "add" => {
            let value = match value {
                Some(v) => v,
                None => return (Fate::Conflict, "the patch has no `value`".into()),
            };
            // An array index or `-` names a position, not a member: a present
            // element there is the one being pushed aside, not "the same
            // value", so an array is asked whether it contains the value.
            let parent = if toks.is_empty() {
                Some(doc)
            } else {
                crate::patch::resolve(doc, &toks[..toks.len() - 1])
            };
            match parent {
                Some(Value::Array(items)) => {
                    if items.contains(value) {
                        (
                            Fate::Obsolete,
                            "the vendor's array already contains this value".into(),
                        )
                    } else {
                        (
                            Fate::Reapplied,
                            "the vendor's array does not contain this value".into(),
                        )
                    }
                }
                Some(Value::Object(_)) => match current {
                    Some(cur) if cur == value => (
                        Fate::Obsolete,
                        "the vendor's document now carries this value".into(),
                    ),
                    Some(cur) => (
                        Fate::Conflict,
                        format!(
                            "the vendor's document now defines this pointer as {} (the patch adds {})",
                            short(cur),
                            short(value)
                        ),
                    ),
                    None => (Fate::Reapplied, "target still absent".into()),
                },
                _ => (
                    Fate::Conflict,
                    "the parent no longer exists in the vendor's document".into(),
                ),
            }
        }
        "replace" => {
            let value = match value {
                Some(v) => v,
                None => return (Fate::Conflict, "the patch has no `value`".into()),
            };
            match current {
                None => (
                    Fate::Conflict,
                    "the target no longer exists in the vendor's document".into(),
                ),
                Some(cur) if cur == value => (
                    Fate::Obsolete,
                    "the vendor's document now holds the patched value".into(),
                ),
                Some(cur) => match was {
                    Some(w) if w == cur => (Fate::Reapplied, "`was` still matches".into()),
                    Some(w) => (
                        Fate::Conflict,
                        format!(
                            "the vendor changed the target to {} (the patch replaced {} with {})",
                            short(cur),
                            short(w),
                            short(value)
                        ),
                    ),
                    None => (
                        Fate::Reapplied,
                        format!(
                            "no `was` recorded; re-applied over the vendor's current {} unchecked",
                            short(cur)
                        ),
                    ),
                },
            }
        }
        "remove" => match current {
            None => (
                Fate::Obsolete,
                "the vendor's document no longer has the target".into(),
            ),
            Some(cur) => match was {
                Some(w) if w == cur => (Fate::Reapplied, "`was` still matches".into()),
                Some(w) => (
                    Fate::Conflict,
                    format!(
                        "the vendor changed the target to {} (the patch removed {})",
                        short(cur),
                        short(w)
                    ),
                ),
                None => (
                    Fate::Reapplied,
                    format!(
                        "no `was` recorded; removes the vendor's current {} unchecked",
                        short(cur)
                    ),
                ),
            },
        },
        // `test` and anything else: let apply decide.
        _ => (Fate::Reapplied, "checked by apply".into()),
    }
}

/// Lines for a text report, one per outcome, grouped by fate in the
/// order re-applied, obsolete, conflict.
pub fn report_lines(replay: &Replay) -> Vec<String> {
    let mut lines = Vec::new();
    for fate in [Fate::Reapplied, Fate::Obsolete, Fate::Conflict] {
        for o in replay.outcomes.iter().filter(|o| o.fate == fate) {
            let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
            let cite = match (get_str(&o.patch, "decision"), get_str(&o.patch, "reason")) {
                (Some(d), Some(r)) => format!(" [{}: {}]", d, one_line(r)),
                (Some(d), None) => format!(" [{}]", d),
                (None, Some(r)) => format!(" [{}]", one_line(r)),
                (None, None) => String::new(),
            };
            lines.push(format!(
                "{:<11}{} — {}{}",
                format!("{}:", fate.as_str()),
                crate::patch::describe(&o.patch),
                one_line(&o.why),
                cite
            ));
        }
    }
    lines
}

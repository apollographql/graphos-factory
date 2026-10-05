//! Which branch of a two-branch success/error `oneOf`/`anyOf` response is
//! the actual payload -- decided from unambiguous SOURCE evidence only,
//! never a property's name (ADR 0080; item 6 of the specialist-pipeline
//! leftovers plan).
//!
//! The specialist-pipeline copy's `resolve_success_branch`
//! (`envelope.rs` on `phase-7a-specialist-pipeline` at 0c47033) picked the
//! non-error branch whenever exactly one of the two looked like an error --
//! "looked like" meaning "has a property literally named `error` or
//! `errors`". That is a naming heuristic, not failure semantics: a
//! legitimate success variant that reports partial per-item failures via
//! an `errors` field on an otherwise-successful 2xx response would be
//! misclassified as the error branch and its real shape discarded. This
//! module never inspects a property's name to decide which branch is
//! which. Two kinds of evidence count, both checkable against the source
//! document alone:
//!
//! - [`Evidence::StatusCorrelated`] -- one branch's shape is also the shape
//!   the same operation documents at an explicit non-2xx status. The
//!   spec's own status-code semantics say so; no guessing.
//! - [`Evidence::Discriminated`] -- both branches require the same
//!   property, typed boolean, each pinning it (`const`, or a single-value
//!   `enum`) to the opposite literal. A boolean flag pinned to `true` in
//!   one branch and `false` in the other is a genuine, wire-checkable
//!   discriminator regardless of what the property is called -- unlike
//!   the retired heuristic, this reads a literal *value* pinned by the
//!   spec, not a property's *name*. `true` is the success branch by the
//!   universal boolean convention, not by reading English into a name.
//!
//! Neither kind reads a property's name to decide direction. When neither
//! applies, [`resolve`] returns `None` and the union is left exactly as
//! found -- `inventory build` writes no fact, matching ADR 0018's
//! requirement that inventory be reproducible from the source document
//! alone with nothing promoted from a human's or an agent's review.

use crate::json::{get, get_arr, get_obj, get_str, Object};
use serde_json::Value;
use std::collections::HashSet;

fn deref(shape: Option<&Value>, shapes: &Object, seen: &mut HashSet<String>) -> Option<Value> {
    let s = shape?;
    match get_str(s, "$ref") {
        Some(r) => {
            let name = r.rsplit('/').next().unwrap_or(r).to_string();
            if !seen.insert(name.clone()) {
                return None;
            }
            deref(shapes.get(&name), shapes, seen)
        }
        None => Some(s.clone()),
    }
}

/// Why one branch was promoted to the success shape. Carried only for
/// reporting (the ADR, `inventory describe`); `inventory.json` itself
/// records just the resolved `referenced_shape` pointer, not the reason --
/// the reason is re-derivable from the source document at any time, and
/// ADR 0073's own "no cached verdict" rule applies here too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// The error branch's shape is also documented at this explicit
    /// non-2xx status on the same operation.
    StatusCorrelated { status: String },
    /// Both branches require `property`, typed boolean, pinned to the
    /// opposite literal in each.
    Discriminated { property: String },
}

pub struct Resolution {
    /// The success branch, exactly as the source wrote it: a `{"$ref":
    /// ...}` object when the branch is a named component, the inline
    /// shape otherwise.
    pub branch: Value,
    pub evidence: Evidence,
}

/// Does `shape` (already deref'd) have `properties` and, if `required` is
/// given, does it require `key`?
fn requires(shape: &Value, key: &str) -> bool {
    get_arr(shape, "required")
        .into_iter()
        .flatten()
        .any(|r| r.as_str() == Some(key))
}

/// The single literal value a property is pinned to: `const`, or a
/// one-element `enum`. `None` for anything else, including an absent
/// property or one with more than one allowed value -- an unpinned
/// property is not a discriminator.
fn pinned_literal(property: &Value) -> Option<&Value> {
    if let Some(c) = property.get("const") {
        return Some(c);
    }
    match get_arr(property, "enum") {
        Some(e) if e.len() == 1 => e.first(),
        _ => None,
    }
}

fn is_boolean_typed(property: &Value) -> bool {
    match get(property, "type") {
        Some(Value::String(t)) => t == "boolean",
        Some(Value::Array(ts)) => ts.iter().any(|t| t.as_str() == Some("boolean")),
        // A pinned boolean literal is unambiguous even with no declared type.
        None => matches!(pinned_literal(property), Some(Value::Bool(_))),
        _ => false,
    }
}

/// Two branches sharing a required property, boolean-typed, each pinned to
/// the opposite literal boolean. Returns the (success index, property
/// name) when so -- the branch pinned to `true` is success, by the
/// boolean convention itself, never by the property's name.
fn boolean_discriminator(branches: &[Value; 2]) -> Option<(usize, String)> {
    let props: Vec<&Object> = branches
        .iter()
        .map(|b| get_obj(b, "properties"))
        .collect::<Option<Vec<_>>>()?;
    for name in props[0].keys() {
        let Some(p1) = props[1].get(name) else {
            continue;
        };
        let p0 = &props[0][name];
        if !requires(&branches[0], name) || !requires(&branches[1], name) {
            continue;
        }
        if !is_boolean_typed(p0) || !is_boolean_typed(p1) {
            continue;
        }
        let (Some(Value::Bool(v0)), Some(Value::Bool(v1))) =
            (pinned_literal(p0), pinned_literal(p1))
        else {
            continue;
        };
        if v0 == v1 {
            continue;
        }
        let success_index = if *v0 { 0 } else { 1 };
        return Some((success_index, name.clone()));
    }
    None
}

/// Structural equality after both sides are deref'd -- enough to recognize
/// "the same named component schema" (the common case: both sides are
/// `$ref`s to the same target) without a full recursive deep-equal that
/// would also match two unrelated but coincidentally identical inline
/// shapes.
fn same_shape(a: &Value, b: &Value, shapes: &Object) -> bool {
    let ra = deref(Some(a), shapes, &mut HashSet::new());
    let rb = deref(Some(b), shapes, &mut HashSet::new());
    ra.is_some() && ra == rb
}

/// Resolve which of `response_shape`'s two branches is the success shape,
/// only on unambiguous evidence (ADR 0080). `error_shapes` are the same
/// operation's own explicitly documented non-2xx responses, as `(status,
/// shape)` pairs -- `crate::openapi`'s own error-response reader already
/// extracts them per operation; the shape may be a `$ref` or already
/// resolved, both are deref'd the same way a branch is. `None` whenever
/// `response_shape` is not a two-branch `oneOf`/`anyOf`, or the branches
/// cannot be told apart by either evidence kind -- the union is then left
/// exactly as found, and the caller must not fall back to a naming guess.
pub fn resolve(
    response_shape: &Value,
    shapes: &Object,
    error_shapes: &[(String, Value)],
) -> Option<Resolution> {
    let resolved_root = deref(Some(response_shape), shapes, &mut HashSet::new())?;
    for key in ["oneOf", "anyOf"] {
        let Some(raw_branches) = get_arr(&resolved_root, key) else {
            continue;
        };
        if raw_branches.len() != 2 {
            continue;
        }
        let (Some(b0), Some(b1)) = (
            deref(Some(&raw_branches[0]), shapes, &mut HashSet::new()),
            deref(Some(&raw_branches[1]), shapes, &mut HashSet::new()),
        ) else {
            continue;
        };

        // Evidence 1: one branch's shape is also documented at an explicit
        // non-2xx status on the same operation.
        for (i, branch) in [&b0, &b1].into_iter().enumerate() {
            if let Some((status, _)) = error_shapes
                .iter()
                .find(|(_, e)| same_shape(branch, e, shapes))
            {
                let success_index = 1 - i;
                return Some(Resolution {
                    branch: raw_branches[success_index].clone(),
                    evidence: Evidence::StatusCorrelated {
                        status: status.clone(),
                    },
                });
            }
        }

        // Evidence 2: a shared required boolean property pinned to the
        // opposite literal in each branch.
        if let Some((success_index, property)) = boolean_discriminator(&[b0, b1]) {
            return Some(Resolution {
                branch: raw_branches[success_index].clone(),
                evidence: Evidence::Discriminated { property },
            });
        }
    }
    None
}

/// Whether `response_shape` is a genuine two-branch `oneOf`/`anyOf`
/// candidate for this item at all, resolved or not -- used by callers that
/// need to tell "not this kind of shape" (leave everything alone) apart
/// from "this kind of shape, but ambiguous" (fall back to JSON, open
/// finding).
pub fn is_candidate(response_shape: &Value, shapes: &Object) -> bool {
    let Some(resolved) = deref(Some(response_shape), shapes, &mut HashSet::new()) else {
        return false;
    };
    ["oneOf", "anyOf"]
        .iter()
        .any(|k| get_arr(&resolved, k).map(|b| b.len() == 2).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shapes(pairs: Vec<(&str, Value)>) -> Object {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn error_shape(status: &str, shape: Value) -> (String, Value) {
        (status.to_string(), shape)
    }

    // ── A measured envelope: a boolean `success` pinned true/false ──────

    #[test]
    fn a_boolean_success_discriminator_resolves_to_the_true_branch() {
        let shapes = shapes(vec![
            (
                "Success",
                json!({"type": "object", "required": ["success", "results"], "properties": {
                    "success": {"type": "boolean", "const": true},
                    "results": {"type": "object"}
                }}),
            ),
            (
                "Error",
                json!({"type": "object", "required": ["success", "errors"], "properties": {
                    "success": {"type": "boolean", "const": false},
                    "errors": {"type": "array", "items": {"type": "string"}}
                }}),
            ),
        ]);
        let response = json!({"oneOf": [{"$ref": "#/shapes/Success"}, {"$ref": "#/shapes/Error"}]});
        let res = resolve(&response, &shapes, &[]).expect("should resolve");
        assert_eq!(res.branch, json!({"$ref": "#/shapes/Success"}));
        assert_eq!(
            res.evidence,
            Evidence::Discriminated {
                property: "success".to_string()
            }
        );
    }

    #[test]
    fn the_discriminator_property_name_is_irrelevant_only_its_pinned_value_is() {
        // Same shapes as above, but the shared property is called `ok`, not
        // `success` -- and the error branch also carries a property
        // literally named `error`, which must NOT be what resolves this.
        let shapes = shapes(vec![
            (
                "Success",
                json!({"type": "object", "required": ["ok", "results"], "properties": {
                    "ok": {"type": "boolean", "const": true},
                    "results": {"type": "object"}
                }}),
            ),
            (
                "Failure",
                json!({"type": "object", "required": ["ok", "error"], "properties": {
                    "ok": {"type": "boolean", "const": false},
                    "error": {"type": "string"}
                }}),
            ),
        ]);
        let response =
            json!({"oneOf": [{"$ref": "#/shapes/Success"}, {"$ref": "#/shapes/Failure"}]});
        let res = resolve(&response, &shapes, &[]).expect("should resolve");
        assert_eq!(res.branch, json!({"$ref": "#/shapes/Success"}));
    }

    // ── Status-code correlation ─────────────────────────────────────────

    #[test]
    fn a_branch_matching_a_documented_non_2xx_shape_is_the_error_branch() {
        let shapes = shapes(vec![
            (
                "Widget",
                json!({"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}}),
            ),
            (
                "Problem",
                json!({"type": "object", "properties": {"detail": {"type": "string"}}}),
            ),
        ]);
        let response =
            json!({"oneOf": [{"$ref": "#/shapes/Widget"}, {"$ref": "#/shapes/Problem"}]});
        let error_shapes = vec![error_shape("404", json!({"$ref": "#/shapes/Problem"}))];
        let res = resolve(&response, &shapes, &error_shapes).expect("should resolve");
        assert_eq!(res.branch, json!({"$ref": "#/shapes/Widget"}));
        assert_eq!(
            res.evidence,
            Evidence::StatusCorrelated {
                status: "404".to_string()
            }
        );
    }

    // ── The exact false positive the retired heuristic produced ────────

    #[test]
    fn a_legitimate_success_variant_with_an_errors_field_is_left_ambiguous() {
        // Partial-failure bulk response: genuinely successful (2xx), no
        // boolean discriminator, no status correlation -- and the retired
        // heuristic would have misclassified it as the error branch purely
        // because it has a property named `errors`.
        let shapes = shapes(vec![
            (
                "BulkResult",
                json!({"type": "object", "required": ["created", "errors"], "properties": {
                    "created": {"type": "array", "items": {"type": "string"}},
                    "errors": {"type": "array", "items": {"type": "object"}}
                }}),
            ),
            (
                "ErrorEnvelope",
                json!({"type": "object", "required": ["message"], "properties": {
                    "message": {"type": "string"}
                }}),
            ),
        ]);
        let response =
            json!({"oneOf": [{"$ref": "#/shapes/BulkResult"}, {"$ref": "#/shapes/ErrorEnvelope"}]});
        assert!(resolve(&response, &shapes, &[]).is_none());
        assert!(is_candidate(&response, &shapes));
    }

    #[test]
    fn a_single_branch_union_is_not_a_candidate() {
        let shapes = shapes(vec![]);
        let response = json!({"oneOf": [{"type": "string"}]});
        assert!(!is_candidate(&response, &shapes));
        assert!(resolve(&response, &shapes, &[]).is_none());
    }

    #[test]
    fn a_three_branch_union_is_not_a_candidate() {
        let shapes = shapes(vec![]);
        let response =
            json!({"oneOf": [{"type": "string"}, {"type": "integer"}, {"type": "boolean"}]});
        assert!(!is_candidate(&response, &shapes));
        assert!(resolve(&response, &shapes, &[]).is_none());
    }

    #[test]
    fn a_shared_non_boolean_pinned_property_does_not_resolve() {
        // A shared discriminator-shaped property that is not boolean must
        // not resolve -- there is no universal convention for which string
        // literal means success, so treating this as the same signal would
        // reopen the naming-heuristic hole one type up.
        let shapes = shapes(vec![
            (
                "A",
                json!({"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string", "const": "ok"}}}),
            ),
            (
                "B",
                json!({"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string", "const": "failed"}}}),
            ),
        ]);
        let response = json!({"oneOf": [{"$ref": "#/shapes/A"}, {"$ref": "#/shapes/B"}]});
        assert!(resolve(&response, &shapes, &[]).is_none());
    }

    #[test]
    fn a_non_required_boolean_discriminator_does_not_resolve() {
        let shapes = shapes(vec![
            (
                "A",
                json!({"type": "object", "properties": {"success": {"type": "boolean", "const": true}}}),
            ),
            (
                "B",
                json!({"type": "object", "properties": {"success": {"type": "boolean", "const": false}}}),
            ),
        ]);
        let response = json!({"oneOf": [{"$ref": "#/shapes/A"}, {"$ref": "#/shapes/B"}]});
        assert!(resolve(&response, &shapes, &[]).is_none());
    }

    #[test]
    fn anyof_is_read_the_same_as_oneof() {
        let shapes = shapes(vec![
            (
                "A",
                json!({"type": "object", "required": ["success"], "properties": {"success": {"type": "boolean", "const": true}}}),
            ),
            (
                "B",
                json!({"type": "object", "required": ["success"], "properties": {"success": {"type": "boolean", "const": false}}}),
            ),
        ]);
        let response = json!({"anyOf": [{"$ref": "#/shapes/A"}, {"$ref": "#/shapes/B"}]});
        assert!(resolve(&response, &shapes, &[]).is_some());
    }
}

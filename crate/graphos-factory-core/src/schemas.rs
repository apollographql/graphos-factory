//! The contract schemas, embedded at build time. The binary is
//! installed into a cache directory rather than run from the repository, so
//! it cannot find `schemas/` on disk; it carries the exact versions it was
//! built against instead. `--schemas DIR` on lint/inventory/infer/evidence
//! overrides them for development. A target embeds its own beside these
//! (`Target::embedded_schemas`).
//!
//! Each core schema's `$id` names the public repository the core is
//! mirrored to (`https://github.com/apollographql/graphos-factory/schemas/<file>`),
//! so it reads the same in every tree; it is a name, never fetched. The
//! `selection review` input token hashes these texts, so an `$id` change is
//! a token change.

use serde_json::Value;
use std::path::Path;

pub const CONTEXT: &str = include_str!("../../../schemas/context.schema.json");
pub const DECISIONS: &str = include_str!("../../../schemas/decisions.schema.json");
pub const FINDINGS: &str = include_str!("../../../schemas/findings.schema.json");
pub const WORKSPACE: &str = include_str!("../../../schemas/workspace.schema.json");
pub const INVENTORY: &str = include_str!("../../../schemas/inventory.schema.json");
pub const SELECTION: &str = include_str!("../../../schemas/selection.schema.json");
pub const EVIDENCE: &str = include_str!("../../../schemas/evidence.schema.json");
pub const APPLIED_LOCK: &str = include_str!("../../../schemas/applied-lock.schema.json");
pub const SOURCES_LOCK: &str = include_str!("../../../schemas/sources-lock.schema.json");

/// Load a contract schema by file name, from `dir` when given (and the file
/// exists there), otherwise from the embedded copy: the core's, then the
/// active target's. Returns None only for an unknown name.
pub fn load(name: &str, dir: Option<&Path>) -> Option<Value> {
    load_from(name, dir, crate::target::active().embedded_schemas)
}

/// [`load`] with the target's embedded schemas named explicitly.
pub fn load_from(name: &str, dir: Option<&Path>, embedded: &[(&str, &str)]) -> Option<Value> {
    if let Some(d) = dir {
        let file = d.join(name);
        if file.exists() {
            if let Ok(text) = std::fs::read_to_string(&file) {
                return crate::json::parse(&text).ok();
            }
        }
    }
    let text = match name {
        "workspace.schema.json" => WORKSPACE,
        "context.schema.json" => CONTEXT,
        "decisions.schema.json" => DECISIONS,
        "findings.schema.json" => FINDINGS,
        "inventory.schema.json" => INVENTORY,
        "selection.schema.json" => SELECTION,
        "evidence.schema.json" => EVIDENCE,
        "applied-lock.schema.json" => APPLIED_LOCK,
        "sources-lock.schema.json" => SOURCES_LOCK,
        _ => embedded.iter().find(|(n, _)| *n == name)?.1,
    };
    crate::json::parse(text).ok()
}

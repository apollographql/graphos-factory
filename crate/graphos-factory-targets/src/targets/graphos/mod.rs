//! The graphos target: connector subgraphs for a GraphOS supergraph the
//! user already runs (factory-split proposal §5, §5a; ADR 0114, Phase 8c).
//!
//! A skeleton. It adds no command, file, lint rule or tag vocabulary; it
//! relaxes the two core rules about `@source` that only a single-source
//! renderer needs, names the Federation directives a subgraph composes with,
//! and registers `supergraph_check`, an evidence layer that reports
//! `not_run` until the next pass decides how a subgraph is composed against
//! the user's supergraph. The questions it leaves open are written as
//! questions in `skills/graphos-factory/references/` (proposal §9).

use graphos_factory_core::lint::Findings;
use graphos_factory_core::target::{
    ComposeConfig, EvidenceLayer, InitInput, LayerInput, LintInput, Override, Target,
};
use serde_json::Value;
use std::path::PathBuf;

/// The target's name, and its binary's: what a workspace this target
/// writes records in `workspace.yaml` `skill.name`, the value `init
/// --target` takes, and what `src/bin/graphos-factory.rs` hands the core
/// as the binary's name.
pub const NAME: &str = "graphos-factory";

/// The placeholders a schema may use: the two the core's layers render.
///
/// Open (proposal §9, question 1): keep `{{BASE_URL}}` / `{{AUTH_EXPR}}`
/// and `template.yaml`, rendering literals at export, or author literals
/// and set this to `&[]` (and [`TARGET`]'s `variables_file_required` to
/// `false`), which takes `render` out of this target's path. Until that is
/// decided the target keeps the core's pair and requires the variables
/// file, so a workspace copied from the other target validates unchanged.
pub const PLACEHOLDERS: &[&str] = &["BASE_URL", "AUTH_EXPR"];

/// The Federation directives a connector subgraph composes with: what its
/// schema may import from the Federation `@link`. `federation-drift`
/// reports one the schema applies without importing; an unused import is
/// not drift (`ComposeConfig::link_imports`).
pub const LINK_IMPORTS: &[&str] = &["@key", "@shareable", "@requires", "@provides", "@external"];

/// Why `supergraph_check` does not run yet.
pub const SUPERGRAPH_CHECK_REASON: &str = "not built: composing against the user's supergraph is decided in the next pass (skills/graphos-factory/references/verification.md)";

fn no_lint(_: &LintInput, _: &mut Findings) {}

fn no_init_files(_: &InitInput) -> Vec<(PathBuf, String)> {
    Vec::new()
}

/// A subgraph composes with others, so a second `@source` is a shape this
/// target has not modelled yet, not one it forbids (proposal §9, question 6).
fn multiple_sources(message: &str) -> String {
    message.replace(
        "this target renders exactly one",
        "this target composes with other subgraphs; a second @source is unmodelled, not forbidden",
    )
}

/// `supergraph_check`: compose the subgraph against the user's existing
/// supergraph (`rover subgraph check` against a GraphOS variant, or a
/// compose with their other subgraphs; proposal §9, question 3). Not built,
/// so it says so: `not_run` with the reason, never a pass.
fn supergraph_check(_: &LayerInput) -> Value {
    serde_json::json!({
        "status": "not_run",
        "reason": SUPERGRAPH_CHECK_REASON,
    })
}

pub const TARGET: Target = Target {
    name: NAME,
    placeholders: PLACEHOLDERS,
    variables_file_required: true,
    required_files: &[],
    output_files: &[],
    commands: &[],
    lint: no_lint,
    lint_rules: &[],
    // No tag-based policy reads this target's schemas: every name is allowed.
    tag_vocabulary: Vec::new,
    rule_overrides: &[
        ("multiple-sources", Override::Severity("warn")),
        ("multiple-sources", Override::Message(multiple_sources)),
        // No renderer here counts raw `@source(` text.
        ("commented-source", Override::Off),
    ],
    evidence_layers: &[EvidenceLayer {
        name: "supergraph_check",
        run: supergraph_check,
        gating: false,
    }],
    compose: ComposeConfig {
        federation_spec_version: None,
        link_imports: LINK_IMPORTS,
    },
    init_files: no_init_files,
    export_gate: None,
    embedded_schemas: &[],
};

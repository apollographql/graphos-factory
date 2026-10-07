//! The extension point a binary's composition root fills (ADR 0114).
//!
//! The core is everything that does not know what it is building for: the
//! workspace contract, inventory, selection, reconcile, lock, codify,
//! scaffold and the six core evidence layers. A [`Target`] is the data a
//! composition root hands [`crate::run`]: the commands, lint rules,
//! placeholders, files and evidence layers only that target knows about. The
//! core reads a target only through these fields and never names one.
//!
//! A binary may register several targets; each product binary registers
//! exactly one (ADR 0114, Phase 8f). A command on a workspace runs
//! against the one whose `name` the workspace's `skill.name` records; a
//! command with no workspace runs against the first; `init --target NAME`
//! picks the one a new workspace records (ADR 0114, Phase 8c amendment).

use crate::args::Flags;
use crate::cmd::Entry;
use crate::lint::Findings;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What a target adds to the core.
pub struct Target {
    /// Written to `workspace.yaml` `skill.name` by `init --target`, and how
    /// a command picks its target: a workspace runs against the registered
    /// target whose name its `skill.name` records. The core compares it
    /// only with that recorded string (or `--target`'s value), never with
    /// a literal.
    pub name: &'static str,
    /// The `{{NAME}}` placeholders a schema may use. The core has no default
    /// set: a placeholder outside this list is `unknown-placeholder`.
    pub placeholders: &'static [&'static str],
    /// Whether `template.yaml` (the workspace's variables and their local
    /// test values) must exist: `missing-template` when it does not.
    pub variables_file_required: bool,
    /// Files the target adds to a workspace, workspace-relative.
    pub required_files: &'static [&'static str],
    /// Of the workspace's authored files, the ones the lock's provenance
    /// hashes beyond the core's (the schema, `supergraph.yaml`,
    /// `template.yaml`, `README.md`, `tests/`).
    pub output_files: &'static [&'static str],
    /// Commands registered after the core's; `--help` lists them.
    pub commands: &'static [TargetCommand],
    /// Target rules, run after every core rule with the same parsed inputs.
    pub lint: fn(&LintInput, &mut Findings),
    /// The rule names `lint` can report, listed by `lint --json` so a
    /// reviewer sees which target rules ran.
    pub lint_rules: &'static [&'static str],
    /// The `@tag` names a tag-based policy reads; `unknown-tag` warns on
    /// any other. Empty restricts no name.
    pub tag_vocabulary: fn() -> Vec<String>,
    /// A core rule's severity or message changed, or the rule turned off,
    /// by rule name.
    pub rule_overrides: &'static [(&'static str, Override)],
    /// Whether this target honours the types a resolved decision declares
    /// owned by another subgraph (a record's `foreign_types`, read by
    /// [`crate::decisions::foreign_types`]). When it does, such a type may
    /// carry the owner's exact name, so `type-prefix` does not hold it, and
    /// the entity rules read it as an extension of the owner's entity rather
    /// than one this subgraph resolves ([`crate::entity::check_with`]). When
    /// it does not, the records are kept and no rule reads them.
    pub foreign_types: bool,
    /// Evidence layers run after the core's, recorded under
    /// `target_evidence_layers` in `evidence/latest.json`.
    pub evidence_layers: &'static [EvidenceLayer],
    /// What composition and `federation-drift` hold the schema to.
    pub compose: ComposeConfig,
    /// Files `init` writes beside the core's, workspace-relative, with their
    /// contents. Created when absent; one that already exists is left alone
    /// and reported, never overwritten (where a core file would refuse init).
    pub init_files: fn(&InitInput) -> Vec<(PathBuf, String)>,
    /// The publication gate: whether `evidence/latest.json` lets the
    /// workspace leave for this target's destination.
    pub export_gate: Option<fn(&Value) -> Gate>,
    /// Contract schemas the target embeds, by file name, beside the core's.
    pub embedded_schemas: &'static [(&'static str, &'static str)],
}

/// One command a target registers.
pub struct TargetCommand {
    pub name: &'static str,
    pub entry: Entry,
    /// The command's rows in the top-level usage: the first after the
    /// command name, each further one a continuation line.
    pub summary: &'static [&'static str],
    /// The text `<command> --help` prints, when the command has its own;
    /// otherwise the rows of `summary` are printed.
    pub usage: Option<&'static str>,
    /// The command's sub-verbs, in usage order; empty for none.
    pub verbs: &'static [&'static str],
    /// The flags the command (with a verb, for a command that has verbs)
    /// accepts; `None` for a verb it does not have.
    pub flags: fn(Option<&str>) -> Option<&'static Flags>,
}

/// How a target adjusts one core rule.
#[derive(Clone, Copy)]
pub enum Override {
    /// The rule does not run for this target.
    Off,
    /// The rule's findings carry this severity (`error` or `warn`).
    Severity(&'static str),
    /// The rule's message, rewritten from the core's.
    Message(fn(&str) -> String),
}

/// The parsed inputs every lint rule reads, handed to [`Target::lint`] so a
/// target rule costs no extra parsing.
pub struct LintInput<'a> {
    pub dir: &'a Path,
    pub target: &'a Target,
    pub schemas_dir: Option<&'a Path>,
    pub workspace: &'a Value,
    pub selection: Option<&'a Value>,
    pub inventory: Option<&'a Value>,
    pub schema_file: &'a str,
    pub sdl: &'a str,
    /// `decisions.json`, read and validated once per run (ADR 0113); `None`
    /// when it does not load.
    pub decisions: Option<&'a Value>,
    /// Its union with `findings.json`, the record every omit reads.
    pub decisions_and_findings: Option<&'a Value>,
    /// The types a resolved decision declares another subgraph's, when
    /// [`Target::foreign_types`] is set; empty otherwise.
    pub foreign_types: &'a std::collections::BTreeSet<String>,
}

/// One evidence layer a target adds.
pub struct EvidenceLayer {
    /// The layer's key under `target_evidence_layers`.
    pub name: &'static str,
    /// The layer's row (`status`, and the `reason`, `findings`, `cases`,
    /// `failed` the core layers carry), from the run so far: the core layers'
    /// rows, the per-operation table and the workspace directory.
    pub run: fn(&LayerInput) -> Value,
    /// Whether a `fail` fails the `evidence` command's exit code.
    pub gating: bool,
}

/// What a target evidence layer reads.
pub struct LayerInput<'a> {
    pub dir: &'a Path,
    /// The evidence so far, in `latest.json`'s shape: `layers`,
    /// `operations`, and the target layers already run.
    pub evidence: &'a Value,
    /// The directory of the core's wrapper scripts this run uses
    /// (`--scripts`, else `GRAPHOS_FACTORY_CORE_SCRIPTS`): a layer that runs
    /// a script of its own finds it from here, and hands it the same core.
    pub scripts: &'a Path,
}

/// What composition holds the schema to, per target.
pub struct ComposeConfig {
    /// The Federation spec version the schema's `@link` must name; `None`
    /// keeps the workspace's own pin (`federation_spec_version`, else
    /// `federation_version`).
    pub federation_spec_version: Option<&'static str>,
    /// The Federation directives this target composes with: the set a
    /// schema *may* import from the Federation `@link`, not a set it must.
    /// `federation-drift` reports a directive of this set that the schema
    /// applies (`@key(...)`, `@shareable`: outside `#` comments and the
    /// import list itself) while the Federation `@link` does not import it,
    /// since composition would refuse the schema. An import the schema
    /// never applies is not drift, and neither is an import outside this
    /// set (`@tag`): the rule holds only the directives listed. Empty checks
    /// no import.
    pub link_imports: &'static [&'static str],
}

/// What `init` hands [`Target::init_files`].
pub struct InitInput<'a> {
    /// The new `workspace.yaml`, parsed.
    pub workspace: &'a Value,
    /// The inventory `init` built.
    pub inventory: &'a Value,
}

/// The publication gate's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub pass: bool,
    /// Why it refused, one entry per layer or operation; empty on a pass.
    pub reasons: Vec<String>,
}

fn no_lint(_: &LintInput, _: &mut Findings) {}

fn no_init_files(_: &InitInput) -> Vec<(PathBuf, String)> {
    Vec::new()
}

/// The core alone: the two placeholders the core's own layers render, a
/// required variables file, and nothing else. The core suite and the
/// `graphos-factory-bare` binary (which registers no target) run against it.
pub const BARE: Target = Target {
    name: "core",
    placeholders: &["BASE_URL", "AUTH_EXPR"],
    variables_file_required: true,
    required_files: &[],
    output_files: &[],
    commands: &[],
    lint: no_lint,
    lint_rules: &[],
    tag_vocabulary: Vec::new,
    rule_overrides: &[],
    foreign_types: false,
    evidence_layers: &[],
    compose: ComposeConfig {
        federation_spec_version: None,
        link_imports: &[],
    },
    init_files: no_init_files,
    export_gate: None,
    embedded_schemas: &[],
};

static REGISTERED: OnceLock<&'static [Target]> = OnceLock::new();
static ACTIVE: OnceLock<&'static Target> = OnceLock::new();

/// Register the targets a binary serves (`crate::run` does, before any
/// command). Every one is kept; [`crate::cmd::dispatch`] fixes the one a
/// command runs against with [`select`]. Only the first registration in a
/// process counts.
pub fn register(targets: &'static [Target]) {
    let _ = REGISTERED.set(targets);
}

/// Every registered target, in the composition root's order; empty for the
/// core alone.
pub fn registered() -> &'static [Target] {
    REGISTERED.get().copied().unwrap_or(&[])
}

/// Fix the target this process runs against. Dispatch calls it once, before
/// the command runs; a later call changes nothing.
pub fn select(target: &'static Target) {
    let _ = ACTIVE.set(target);
}

/// The target this process runs against: the one [`select`] fixed, else the
/// first registered, else [`BARE`] (the core alone, no target registered).
pub fn active() -> &'static Target {
    ACTIVE
        .get()
        .copied()
        .or_else(|| registered().first())
        .unwrap_or(&BARE)
}

/// The target among `targets` whose `name` is `name`: the string a
/// workspace recorded in `skill.name`, or the value `init --target` was
/// given. This is the only comparison the core makes with a target's name.
pub fn by_name<'a>(targets: &'a [Target], name: &str) -> Option<&'a Target> {
    targets.iter().find(|t| t.name == name)
}

/// The names of `targets`, comma-separated, for a message.
pub fn names(targets: &[Target]) -> String {
    targets
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A workspace that records a `skill.name` no registered target carries
/// (`target-unknown`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown {
    /// The name the workspace recorded.
    pub recorded: String,
    /// The registered names, comma-separated.
    pub registered: String,
}

impl std::fmt::Display for Unknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "workspace.yaml skill.name is {:?}, which names no target this binary serves (registered: {})",
            self.recorded, self.registered
        )
    }
}

/// The `skill.name` the workspace at `dir` records. `None` when `dir` holds
/// no readable `.factory/workspace.yaml` (read through custody, ADR 0025) or
/// the file records no name: a workspace file that does not load is the
/// command's own error to report, not the selection's.
pub fn recorded_name(dir: &Path) -> Option<String> {
    let text = crate::factory_io::read_to_string_optional(dir, ".factory/workspace.yaml")
        .ok()
        .flatten()?;
    let workspace = crate::yaml::parse(&text).ok()?;
    workspace
        .get("skill")
        .and_then(|s| s.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The target among `targets` a command on the workspace at `dir` runs
/// against. `Ok(None)` when there is nothing to choose by: no workspace at
/// `dir`, one that records no `skill.name`, or no target registered at all
/// (the core alone runs every workspace against [`BARE`]). `Err` when the
/// workspace records a name none of `targets` carries.
pub fn for_workspace<'a>(targets: &'a [Target], dir: &Path) -> Result<Option<&'a Target>, Unknown> {
    if targets.is_empty() {
        return Ok(None);
    }
    let Some(recorded) = recorded_name(dir) else {
        return Ok(None);
    };
    match by_name(targets, &recorded) {
        Some(t) => Ok(Some(t)),
        None => Err(Unknown {
            recorded,
            registered: names(targets),
        }),
    }
}

/// The target `init --target NAME` creates a workspace for, among
/// `targets`. The flag may be left out only when at most one target is
/// registered (with none, the core alone writes a [`BARE`] workspace). `Err`
/// is the usage error: its code (`target-required`, `target-unknown`) and
/// message, which lists the registered names.
pub fn for_init<'a>(
    targets: &'a [Target],
    requested: Option<&str>,
) -> Result<&'a Target, (&'static str, String)> {
    let listed = || {
        if targets.is_empty() {
            BARE.name.to_string()
        } else {
            names(targets)
        }
    };
    match (requested, targets) {
        (None, []) => Ok(&BARE),
        (None, [only]) => Ok(only),
        (None, _) => Err((
            "target-required",
            format!(
                "--target NAME is required: this binary serves more than one target ({})",
                listed()
            ),
        )),
        (Some(name), []) if name == BARE.name => Ok(&BARE),
        (Some(name), _) => by_name(targets, name).ok_or_else(|| {
            (
                "target-unknown",
                // A product binary is named for its one target, so the name
                // asked for is the binary that writes such a workspace.
                format!(
                    "--target {:?} names no target this binary serves ({}); a {} workspace is made by `{} init`",
                    name,
                    listed(),
                    name,
                    name
                ),
            )
        }),
    }
}

/// The target command `name` and the target that registers it: the active
/// target's first, then every other registered target's in order, so
/// `--help`, the flag check and dispatch find a command whichever target
/// owns it.
pub fn command(name: &str) -> Option<(&'static Target, &'static TargetCommand)> {
    let active = active();
    std::iter::once(active)
        .chain(registered().iter().filter(|t| !std::ptr::eq(*t, active)))
        .find_map(|t| t.commands.iter().find(|c| c.name == name).map(|c| (t, c)))
}

impl Target {
    /// The first override this target sets on `rule`, if any.
    pub fn rule_override(&self, rule: &str) -> Option<Override> {
        self.rule_overrides_for(rule).next()
    }

    /// Every override this target sets on `rule`, in the order listed: a
    /// target may change a rule's severity and its message with two
    /// entries.
    pub fn rule_overrides_for<'a>(&'a self, rule: &'a str) -> impl Iterator<Item = Override> + 'a {
        self.rule_overrides
            .iter()
            .filter(move |(r, _)| *r == rule)
            .map(|(_, o)| *o)
    }

    /// The core rules this target overrides, each named once, in order.
    pub fn overridden_rules(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for (rule, _) in self.rule_overrides {
            if !out.contains(rule) {
                out.push(rule);
            }
        }
        out
    }
}

//! Provenance for an authored/applied workspace state.
//!
//! The applied lock is the right-sized home: it already means "the agent has
//! seen this state", is internal to the workspace, and is refreshed after an
//! apply.  This module records exact byte hashes without claiming that an LLM
//! run can be reproduced bit for bit.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// The core scripts' `toolchain.sh`, which owns every version pin; the build
/// script finds it (`GRAPHOS_FACTORY_CORE_SCRIPTS_DIR`).
const TOOLCHAIN_SH: &str = include_str!(concat!(
    env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR"),
    "/toolchain.sh"
));

fn authoring_model(explicit: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| {
            crate::env::var("LLM_MODEL")
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn bytes_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn file_record(dir: &Path, rel: &str) -> Result<Value, String> {
    let path = Path::new(rel);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("{}: expected a workspace-relative file", rel));
    }
    // `.factory/*` is custody's, and a link there is refused outright rather
    // than resolved and hashed; the authored files beside it (the schema, the
    // suite, the spec) keep the canonicalize-and-contain check they had, so a
    // link the user made inside their own workspace still records (ADR 0025).
    let bytes = if rel.starts_with(&format!("{}/", crate::factory_io::STATE_DIR)) {
        crate::factory_io::read(dir, rel).map_err(String::from)?
    } else {
        let root = dir.canonicalize().map_err(|e| e.to_string())?;
        let file = dir
            .join(path)
            .canonicalize()
            .map_err(|e| format!("{}: {}", rel, e))?;
        if !file.starts_with(root) {
            return Err(format!("{}: file resolves outside the workspace", rel));
        }
        std::fs::read(&file).map_err(|e| format!("{}: {}", rel, e))?
    };
    Ok(crate::json::object(vec![
        ("sha256", Value::from(bytes_hash(&bytes))),
        ("bytes", Value::from(bytes.len() as u64)),
    ]))
}

fn insert_existing(dir: &Path, paths: impl IntoIterator<Item = String>) -> Result<Value, String> {
    let mut map = crate::json::obj();
    for rel in paths.into_iter().collect::<BTreeSet<_>>() {
        if dir.join(&rel).is_file() {
            map.insert(rel.clone(), file_record(dir, &rel)?);
        }
    }
    Ok(Value::Object(map))
}

fn collect_snapshot_paths(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_snapshot_paths(item, out)),
        Value::Object(map) => {
            if map.get("sha256").and_then(Value::as_str).is_some() {
                if let Some(path) = map.get("path").and_then(Value::as_str) {
                    out.insert(path.to_string());
                }
            }
            map.values()
                .for_each(|item| collect_snapshot_paths(item, out));
        }
        _ => {}
    }
}

fn input_paths(dir: &Path) -> Result<BTreeSet<String>, String> {
    let mut paths: BTreeSet<String> = [
        ".factory/workspace.yaml",
        ".factory/inventory.json",
        ".factory/selection.yaml",
        ".factory/context.yaml",
        ".factory/sources.lock.yaml",
        crate::decisions::FILE,
        crate::findings::FILE,
        ".factory/memory.md",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();

    if let Some(lock) = crate::sources::read_sources_lock(dir)? {
        for source in crate::sources::document_entries(&lock) {
            paths.insert(source.path);
            if let Some(upstream) = source.upstream {
                paths.insert(upstream);
            }
        }
    }
    if let Some(text) = crate::factory_io::read_to_string_optional(dir, crate::context::FILE)? {
        let value =
            crate::yaml::parse(&text).map_err(|e| format!("{}: {}", crate::context::FILE, e))?;
        collect_snapshot_paths(&value, &mut paths);
    }
    Ok(paths)
}

/// A dotfile or a file under a dot-directory (`.DS_Store`, an editor's swap
/// file) is local noise, never part of the suite.
fn is_hidden(rel: &Path) -> bool {
    rel.components().any(|c| match c {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

/// The files git would commit under `rel`: tracked, plus untracked ones no
/// ignore rule excludes, as paths relative to `root` (`git -C root` makes git
/// strip the prefix of a workspace nested in a larger repository). Untracked
/// files stay in because `lock` runs before an apply's commit, when the
/// cases it scaffolded are not tracked yet. `None` when `root` is not inside
/// a git work tree, or git is unavailable.
fn git_listed_files(root: &Path, rel: &Path) -> Option<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .arg(rel)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listed: BTreeSet<PathBuf> = output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(String::from_utf8_lossy(p).to_string()))
        .collect();
    Some(listed.into_iter().collect())
}

/// Record the files under `rel` that belong to the workspace. In a git work
/// tree that is what git would commit (tracked or not ignored), so a local
/// file the checkout will never have is not recorded and later reported
/// missing by `lock --check --provenance`; outside one, every file on disk.
/// Dotfiles are skipped either way, and a symlink is listed in `omitted`,
/// never followed.
fn collect_files(
    root: &Path,
    rel: &Path,
    out: &mut BTreeSet<String>,
    omitted: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    if !root.join(rel).is_dir() {
        return Ok(());
    }
    match git_listed_files(root, rel) {
        Some(listed) => {
            for next in listed {
                if is_hidden(&next) {
                    continue;
                }
                // A tracked file deleted from disk is simply not recorded.
                let Ok(meta) = std::fs::symlink_metadata(root.join(&next)) else {
                    continue;
                };
                if meta.file_type().is_symlink() {
                    omitted.insert(
                        next.to_string_lossy().to_string(),
                        "symlink not followed".to_string(),
                    );
                } else if meta.is_file() {
                    out.insert(next.to_string_lossy().to_string());
                }
            }
            Ok(())
        }
        None => walk_files(root, rel, out, omitted),
    }
}

fn walk_files(
    root: &Path,
    rel: &Path,
    out: &mut BTreeSet<String>,
    omitted: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let dir = root.join(rel);
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {}", dir.display(), e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let next = rel.join(entry.file_name());
        if is_hidden(Path::new(&entry.file_name())) {
            continue;
        }
        if ty.is_symlink() {
            omitted.insert(
                next.to_string_lossy().to_string(),
                "symlink not followed".to_string(),
            );
            continue;
        }
        if ty.is_dir() {
            walk_files(root, &next, out, omitted)?;
        } else if ty.is_file() {
            out.insert(next.to_string_lossy().to_string());
        }
    }
    Ok(())
}

fn output_paths(
    dir: &Path,
    schema_file: &str,
) -> Result<(BTreeSet<String>, BTreeMap<String, String>), String> {
    let mut paths: BTreeSet<String> =
        [schema_file, "supergraph.yaml", "template.yaml", "README.md"]
            .into_iter()
            .chain(crate::target::active().output_files.iter().copied())
            .map(str::to_string)
            .collect();
    let mut omitted = BTreeMap::new();
    collect_files(dir, Path::new("tests"), &mut paths, &mut omitted)?;
    Ok((paths, omitted))
}

fn command(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Where the environment points the skill checkout, before Git is asked.
/// An empty variable counts as unset, and a root that resolves to the empty
/// path is no root: `git -C ""` runs in the current directory, which is the
/// workspace, and would record the workspace's own commit as the skill's.
enum EnvSkillRoot {
    /// The root, and the variable it was read from.
    SkillRoot(PathBuf, String),
    Scripts {
        value: String,
        root: Option<PathBuf>,
        name: String,
    },
    Unset,
}

fn env_skill_root() -> EnvSkillRoot {
    if let Some((name, root)) =
        crate::env::var_os_named("SKILL_ROOT").filter(|(_, v)| !v.is_empty())
    {
        return EnvSkillRoot::SkillRoot(PathBuf::from(root), name);
    }
    let Some((name, value)) = crate::env::var_os_named("SCRIPTS") else {
        return EnvSkillRoot::Unset;
    };
    let scripts = PathBuf::from(&value);
    let scripts = scripts.canonicalize().unwrap_or(scripts);
    // `<root>/<dir>/scripts`: the core's `graphos-factory-core/scripts` and a target's
    // `skills/<name>/scripts`'s parent alike sit two levels below the root.
    let root = scripts
        .ancestors()
        .nth(2)
        .filter(|root| !root.as_os_str().is_empty())
        .map(Path::to_path_buf);
    EnvSkillRoot::Scripts {
        value: value.to_string_lossy().into_owned(),
        root,
        name,
    }
}

fn inferred_skill_root(explicit: Option<&Path>) -> Option<(PathBuf, String)> {
    if let Some(root) = explicit {
        return Some((root.to_path_buf(), "--skill-dir".to_string()));
    }
    match env_skill_root() {
        EnvSkillRoot::SkillRoot(root, name) => Some((root, name)),
        EnvSkillRoot::Scripts {
            root: Some(root),
            name,
            ..
        } => Some((root, name)),
        _ => None,
    }
}

fn skill_state(explicit: Option<&Path>) -> Result<Value, String> {
    if let Some((root, source)) = inferred_skill_root(explicit) {
        let revision = command(&root, &["rev-parse", "HEAD"]);
        let status = command(&root, &["status", "--porcelain", "--untracked-files=all"]);
        match (revision, status) {
            (Some(revision), Some(status)) => {
                return Ok(crate::json::object(vec![
                    ("revision", Value::from(revision)),
                    ("dirty", Value::Bool(!status.is_empty())),
                    ("source", Value::from(source)),
                ]));
            }
            _ if explicit.is_some() => {
                return Err(format!(
                    "{} is not a readable Git checkout for --skill-dir",
                    root.display()
                ));
            }
            _ => {}
        }
    }
    Ok(crate::json::object(vec![
        (
            "revision",
            Value::from(env!("GRAPHOS_FACTORY_CORE_BUILD_REVISION")),
        ),
        (
            "dirty",
            Value::Bool(env!("GRAPHOS_FACTORY_CORE_BUILD_DIRTY") == "true"),
        ),
        ("source", Value::from("binary-build")),
    ]))
}

fn assignment_value(text: &str) -> Option<&str> {
    let text = text.trim();
    let first = text.chars().next()?;
    if first == '\'' || first == '"' {
        let rest = &text[first.len_utf8()..];
        let end = rest.find(first)?;
        let tail = rest[end + first.len_utf8()..].trim();
        if !tail.is_empty() && !tail.starts_with('#') {
            return None;
        }
        return Some(&rest[..end]);
    }
    text.split(|c: char| c.is_whitespace() || c == ';')
        .next()
        .filter(|value| !value.is_empty())
}

fn toolchain_pin(script: &str, name: &str) -> Option<String> {
    script.lines().find_map(|line| {
        let mut assignment = line.trim_start();
        if let Some(rest) = assignment.strip_prefix("export") {
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            assignment = rest.trim_start();
        }
        let (assigned_name, value) = assignment.split_once('=')?;
        if assigned_name != name {
            return None;
        }
        let value = assignment_value(value)?;
        let prefix = format!("${{{name}:-");
        let value = value
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix('}'))
            .unwrap_or(value);
        (!value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+')))
        .then(|| value.to_string())
    })
}

fn pin(name: &str) -> Result<String, String> {
    toolchain_pin(TOOLCHAIN_SH, name)
        .ok_or_else(|| format!("toolchain.sh has no readable {} pin", name))
}

/// Authoring provenance for the exact state being acknowledged by `lock`.
pub fn record(
    dir: &Path,
    schema_file: &str,
    skill_dir: Option<&Path>,
    model: Option<&str>,
) -> Result<Value, String> {
    let workspace = crate::yaml::parse(&crate::factory_io::read_to_string(
        dir,
        ".factory/workspace.yaml",
    )?)?;
    let inputs = insert_existing(dir, input_paths(dir)?)?;
    let (output_paths, omitted_outputs) = output_paths(dir, schema_file)?;
    let outputs = insert_existing(dir, output_paths)?;
    Ok(crate::json::object(vec![
        ("recorded_at", Value::from(crate::now_iso())),
        ("skill", skill_state(skill_dir)?),
        (
            "authoring_agent",
            crate::json::object(vec![
                ("model", Value::from(authoring_model(model))),
                ("attested", Value::Bool(true)),
            ]),
        ),
        (
            "binary",
            crate::json::object(vec![
                ("name", Value::from(crate::bin_name())),
                ("version", Value::from(env!("CARGO_PKG_VERSION"))),
                (
                    "revision",
                    Value::from(env!("GRAPHOS_FACTORY_CORE_BUILD_REVISION")),
                ),
                (
                    "dirty",
                    Value::Bool(env!("GRAPHOS_FACTORY_CORE_BUILD_DIRTY") == "true"),
                ),
                ("rustc", Value::from(env!("GRAPHOS_FACTORY_CORE_BUILD_RUSTC"))),
                ("target", Value::from(env!("GRAPHOS_FACTORY_CORE_BUILD_TARGET"))),
            ]),
        ),
        (
            "toolchain",
            crate::json::object(vec![
                (
                    "connect_spec",
                    Value::from(
                        crate::json::get_str(&workspace, "connect_spec").unwrap_or("unknown"),
                    ),
                ),
                (
                    "federation",
                    Value::from(
                        crate::json::get_str(&workspace, "federation_version").unwrap_or("unknown"),
                    ),
                ),
                ("rover_pin", Value::from(pin("ROVER_VERSION")?)),
                ("router_pin", Value::from(pin("ROUTER_VERSION")?)),
                ("wiremock_pin", Value::from(pin("WIREMOCK_VERSION")?)),
            ]),
        ),
        ("inputs", inputs),
        ("outputs", outputs),
        (
            "omitted_outputs",
            Value::Object(omitted_outputs.into_iter().map(|(k, v)| (k, Value::from(v))).collect()),
        ),
        (
            "reproducibility",
            Value::from(
                "records the authored state and tools; the model is self-reported and does not prove model identity, full session history, or bit-for-bit LLM replay",
            ),
        ),
    ]))
}

/// One recorded provenance file whose bytes on disk no longer hash to what
/// the lock recorded.
pub struct Drift {
    /// `inputs` or `outputs`.
    pub section: &'static str,
    pub path: String,
    /// `changed`, or `missing` when the file is gone or unreadable.
    pub change: &'static str,
}

/// Rehash every file the lock's provenance recorded and return the ones that
/// differ. A lock without a provenance block has nothing to compare. Files
/// added since the lock are not reported: provenance records what the lock
/// saw, not what the workspace must contain.
pub fn drift(dir: &Path, lock: &Value) -> Vec<Drift> {
    let mut out = Vec::new();
    for section in ["inputs", "outputs"] {
        let Some(entries) = lock
            .get("provenance")
            .and_then(|p| p.get(section))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (rel, recorded) in entries {
            let recorded = recorded.get("sha256").and_then(Value::as_str);
            let change = match file_record(dir, rel) {
                Ok(now) if now.get("sha256").and_then(Value::as_str) == recorded => continue,
                Ok(_) => "changed",
                Err(_) => "missing",
            };
            out.push(Drift {
                section,
                path: rel.clone(),
                change,
            });
        }
    }
    out
}

/// Record provenance after a command creates or updates an applied lock.
pub fn refresh(
    dir: &Path,
    lock: &mut Value,
    skill_dir: Option<&Path>,
    model: Option<&str>,
) -> Result<(), String> {
    let schema_file = crate::json::get_str(lock, "schema")
        .ok_or_else(|| "applied lock has no schema path".to_string())?
        .to_string();
    crate::json::set(lock, "written_by", Value::from(crate::written_by()));
    crate::json::set(
        lock,
        "provenance",
        record(dir, &schema_file, skill_dir, model)?,
    );
    Ok(())
}

/// One line per provenance value that fell back because nothing named it:
/// the model recorded as `unknown`, or the skill recorded from the binary's
/// build revision instead of the checkout the layers ran from. `lock` prints
/// these to stderr; the lock itself is unchanged (ADR 0090). Eight of ten
/// first locks in the AppWorld regeneration recorded one or the other.
pub fn fallback_warnings(provenance: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let model = provenance
        .get("authoring_agent")
        .and_then(|a| a.get("model"))
        .and_then(Value::as_str);
    if model == Some("unknown") {
        out.push(
            "warning: provenance.authoring_agent.model recorded as unknown — pass --model MODEL or set GRAPHOS_FACTORY_CORE_LLM_MODEL to the identifier the host supplies (never a guess), then rerun `graphos-factory-core lock`"
                .to_string(),
        );
    }
    let source = provenance
        .get("skill")
        .and_then(|s| s.get("source"))
        .and_then(Value::as_str);
    if source == Some("binary-build") {
        // `--skill-dir` never falls back (a non-checkout is an error), so
        // the fallback means no flag, and an environment that is unset or
        // points outside a Git checkout. The reason and the remedy follow
        // the variable that actually decided: GRAPHOS_FACTORY_CORE_SKILL_ROOT
        // takes precedence, so advising GRAPHOS_FACTORY_CORE_SCRIPTS while it is
        // set would change nothing.
        let scripts_fix = "pass --skill-dir DIR (the skill's Git checkout) or set GRAPHOS_FACTORY_CORE_SCRIPTS to the scripts/ directory inside that checkout".to_string();
        let (why, fix) = match env_skill_root() {
            EnvSkillRoot::SkillRoot(root, name) => (
                format!(
                    "{} is set to {}, which is not a Git checkout, and it takes precedence over GRAPHOS_FACTORY_CORE_SCRIPTS",
                    name,
                    root.display()
                ),
                format!(
                    "pass --skill-dir DIR (the skill's Git checkout), or point {} at that checkout or unset it",
                    name
                ),
            ),
            EnvSkillRoot::Scripts { value, name, .. } if value.is_empty() => {
                (format!("{} is set but empty", name), scripts_fix)
            }
            EnvSkillRoot::Scripts {
                value,
                root: Some(root),
                name,
            } => (
                format!(
                    "{} is set to '{}', so the skill root is taken as {} (two levels up), which is not a Git checkout",
                    name,
                    value,
                    root.display()
                ),
                scripts_fix,
            ),
            EnvSkillRoot::Scripts {
                value,
                root: None,
                name,
            } => (
                format!(
                    "{} is set to '{}', which is not inside a Git checkout",
                    name, value
                ),
                scripts_fix,
            ),
            EnvSkillRoot::Unset => (
                "neither --skill-dir nor GRAPHOS_FACTORY_CORE_SCRIPTS is set".to_string(),
                scripts_fix,
            ),
        };
        out.push(format!(
            "warning: provenance.skill.source recorded as binary-build (the revision this binary was built from, not the skill the layers ran from): {} — {}, then rerun `graphos-factory-core lock`",
            why, fix
        ));
    }
    out
}

/// Try to record provenance for a partial writer. If collection fails, omit
/// the old record so it cannot appear to describe the new applied state.
pub fn refresh_partial(
    dir: &Path,
    lock: &mut Value,
    skill_dir: Option<&Path>,
    model: Option<&str>,
) -> Option<String> {
    match refresh(dir, lock, skill_dir, model) {
        Ok(()) => None,
        Err(error) => {
            crate::json::remove(lock, "provenance");
            Some(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::toolchain_pin;

    #[test]
    fn toolchain_pins_allow_shell_formatting_without_evaluating_shell() {
        let script = r#"
            export ROVER_VERSION="${ROVER_VERSION:-0.41.0}" # pinned
	ROUTER_VERSION=${ROUTER_VERSION:-2.17.0}
WIREMOCK_VERSION="3.13.2"
        "#;
        assert_eq!(
            toolchain_pin(script, "ROVER_VERSION").as_deref(),
            Some("0.41.0")
        );
        assert_eq!(
            toolchain_pin(script, "ROUTER_VERSION").as_deref(),
            Some("2.17.0")
        );
        assert_eq!(
            toolchain_pin(script, "WIREMOCK_VERSION").as_deref(),
            Some("3.13.2")
        );
    }

    #[test]
    fn toolchain_pins_reject_dynamic_or_mismatched_values() {
        assert_eq!(
            toolchain_pin(
                "ROVER_VERSION=\"${OTHER_VERSION:-0.41.0}\"",
                "ROVER_VERSION"
            ),
            None
        );
        assert_eq!(
            toolchain_pin("ROVER_VERSION=\"$(latest-rover)\"", "ROVER_VERSION"),
            None
        );
        assert_eq!(toolchain_pin("ROVER_VERSION=", "ROVER_VERSION"), None);
    }
}

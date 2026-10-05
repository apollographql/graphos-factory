//! init — create a new service workspace from a description document.
//!
//!   init [workspace] --name N --spec PATH [--target NAME] [--url U] [--retrieved-at T]
//!        [--created-at T] [--context-mode generic|specialized|undecided] [--dry-run] [--json]
//!
//! `--target NAME` is the registered target the workspace is for: `init`
//! writes its name to `skill.name` and adds its `init_files`, and every
//! later command on the workspace runs against it. A target file that
//! already exists is left alone and reported (`left_alone`), never
//! overwritten: it is the caller's, and init does not refuse over it.
//! `--target` is optional with one target registered (each product binary
//! registers exactly one, so it writes that one); required when a binary
//! serves more than one (exit 1, `target-required`, listing them).
//!
//! Writes the files a spec-backed workspace starts with (ADR 0056):
//! `.factory/workspace.yaml`, the working copy (`openapi.<ext>` or
//! `swagger.<ext>`, the document's bytes verbatim), its vendor copy under
//! `.factory/sources/`, `.factory/sources.lock.yaml` and
//! `.factory/inventory.json`. The lock entry is written by the same function
//! `sources pin` uses, and the inventory by the same builder `inventory build`
//! uses, so the result is exactly what pinning and building by hand would
//! give: a later `inventory build` reproduces the inventory byte for byte.
//!
//! The judgement calls stay with the caller: the service name, the
//! `context_mode` assessment (omitted when not given — `context check` then
//! reads it as generic, ADR 0081), and git. `init` never runs git and
//! never commits: the caller decides between a standalone `git init` and a
//! folder of an enclosing repository.
//!
//! Every file is computed before anything is written, so `--dry-run` writes
//! nothing and reports the very bytes a real run writes. With `--json` both
//! print `files: [{path, content, sha256, bytes}]` (workspace-relative paths,
//! exact UTF-8 content — the document must be UTF-8 text, and every other
//! file is generated text) and `left_alone: [path]`, the target files that
//! already existed.
//!
//! Exit codes: 0 created (with --dry-run: would create); 1 usage (a missing
//! or invalid flag, an invalid name; an unknown flag never reaches init —
//! dispatch refuses it with exit 2, ADR 0097); 2 refused, nothing written (the
//! target already holds a workspace or a file init would create, or the
//! document is unreadable, not OpenAPI 3.x / Swagger 2.0, or yields no valid
//! inventory); 4 a write failed after the first file was written (the
//! message lists what was written). With --json a failure prints
//! `{error, code, exit}` (plus `written` for exit 4).

use crate::args::{Args, Flags};
use crate::json::{get, get_arr, get_str, pretty};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The composition plugin pin a new workspace records. Must equal
/// `FEDERATION_VERSION` in `scripts/toolchain.sh` and the `workspace.yaml`
/// template in `references/workspace-contract.md` (tests check both), so a
/// new workspace composes with the toolchain CI and the session hook install.
/// No `federation_spec_version` is written: a new workspace links the
/// plugin's own minor (`federation/v2.15`, ADR 0049).
pub const FEDERATION_VERSION: &str = "2.15.2";
/// The connect spec a new workspace links (`workspace.yaml` `connect_spec`);
/// the template's default since ADR 0049 (a test checks).
pub const CONNECT_SPEC: &str = "v0.4";
pub const WORKSPACE_FILE: &str = ".factory/workspace.yaml";
const CONTEXT_MODES: [&str; 3] = ["generic", "specialized", "undecided"];
pub const USAGE: &str = "usage: init [workspace] --name N --spec PATH [--target NAME] [--url U] [--retrieved-at T] [--created-at T] [--context-mode generic|specialized|undecided] [--dry-run] [--json]
  --target NAME  the target the workspace is for, written to skill.name; defaults to the one target a product binary serves (required only when a binary serves more than one; `--help` lists them)";

/// The four names of one service (references/naming.md).
#[derive(Debug, Clone, PartialEq)]
pub struct Names {
    pub service: String,
    pub directory: String,
    pub type_prefix: String,
    pub field_prefix: String,
}

/// The four names from one service name, given in snake_case (`widget_co`)
/// or kebab-case (`widget-co`). Every word is lowercase letters and digits
/// and starts with a letter: the type prefix capitalises each word
/// (`Widget_Co`), and a word that starts with a digit cannot be capitalised.
pub fn names(name: &str) -> Result<Names, String> {
    let rule = "a service name is lowercase snake_case or kebab-case — words of a-z and 0-9, each starting with a letter, joined by `_` or `-` (not both), e.g. widget_co or widget-co (references/naming.md)";
    if name.contains('-') && name.contains('_') {
        return Err(format!("{:?} mixes `-` and `_`: {}", name, rule));
    }
    let words: Vec<&str> = name.split(['-', '_']).collect();
    let valid_word = |w: &&str| {
        w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && w.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    };
    if name.is_empty() || !words.iter().all(valid_word) {
        return Err(format!("{:?} is not a valid service name: {}", name, rule));
    }
    let type_prefix = words
        .iter()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("_");
    Ok(Names {
        service: words.join("_"),
        directory: words.join("-"),
        type_prefix,
        field_prefix: words.join("_"),
    })
}

/// One file init creates: a workspace-relative path and its exact bytes.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// Why init stopped. `exit` is the process exit code; `code` is a stable
/// slug for a caller that branches on the kind of failure.
#[derive(Debug)]
struct Failure {
    exit: i32,
    code: &'static str,
    message: String,
    written: Option<Vec<String>>,
}

fn fail(exit: i32, code: &'static str, message: String) -> Failure {
    Failure {
        exit,
        code,
        message,
        written: None,
    }
}

/// Everything a run creates and reports, computed before any write.
struct Plan {
    names: Names,
    context_mode: Option<String>,
    kind: &'static str,
    version: String,
    working: String,
    upstream: String,
    upstream_sha256: String,
    content_sha256: String,
    inventory: Value,
    files: Vec<PlannedFile>,
    /// Target files that already exist: not written, not refused.
    left_alone: Vec<String>,
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Whether a target file's place is taken: the file itself exists (a dangling
/// symlink counts), or a directory above it, below `target`, is not a plain
/// directory (a symlink would carry the write wherever it points; a file
/// would fail it). Such a file is left alone, never written through.
fn place_taken(target: &Path, rel: &str) -> bool {
    let full = target.join(rel);
    if exists(&full) {
        return true;
    }
    let mut dir = full.parent();
    while let Some(d) = dir {
        if d == target {
            break;
        }
        match std::fs::symlink_metadata(d) {
            Ok(m) if !m.is_dir() => return true,
            _ => {}
        }
        dir = d.parent();
    }
    false
}

fn plan(args: &Args, target: &Path) -> Result<Plan, Failure> {
    if args.positional.len() > 1 {
        return Err(fail(
            1,
            "usage",
            format!(
                "one workspace directory at most (got {})",
                args.positional.join(" ")
            ),
        ));
    }
    // The target the new workspace records in `skill.name` and whose
    // `init_files` it gets: `--target`, required when the binary serves
    // more than one (ADR 0114, Phase 8c).
    let registered = crate::target::registered();
    if args.has("target") && args.get("target").is_none() {
        return Err(fail(
            1,
            "usage",
            format!(
                "--target needs a value: one of {}",
                crate::target::names(registered)
            ),
        ));
    }
    let chosen = crate::target::for_init(registered, args.get("target"))
        .map_err(|(code, message)| fail(1, code, message))?;
    crate::target::select(chosen);
    let name = args.get("name").ok_or_else(|| {
        fail(
            1,
            "usage",
            "--name N is required: the service name (snake_case or kebab-case)".into(),
        )
    })?;
    let spec = args.get("spec").ok_or_else(|| {
        fail(
            1,
            "usage",
            "--spec PATH is required: the description document (OpenAPI 3.x or Swagger 2.0), already fetched — init never fetches".into(),
        )
    })?;
    let names = names(name).map_err(|e| fail(1, "invalid-name", e))?;
    let context_mode = match args.get("context-mode") {
        None if args.has("context-mode") => {
            return Err(fail(
                1,
                "usage",
                "--context-mode needs a value: generic, specialized or undecided".into(),
            ))
        }
        None => None,
        Some(m) if CONTEXT_MODES.contains(&m) => Some(m.to_string()),
        Some(m) => {
            return Err(fail(
                1,
                "usage",
                format!(
                    "--context-mode {:?} is not one of generic, specialized, undecided",
                    m
                ),
            ))
        }
    };

    // The target: absent (created on a real run) or a directory that holds
    // no workspace yet.
    match std::fs::metadata(target) {
        Ok(m) if !m.is_dir() => {
            return Err(fail(
                2,
                "not-a-directory",
                format!("{} exists and is not a directory", target.display()),
            ))
        }
        _ => {}
    }
    for marker in [".factory", "workspace.yaml"] {
        if exists(&target.join(marker)) {
            return Err(fail(
                2,
                "workspace-exists",
                format!(
                    "{} already holds a workspace ({} exists); init creates a new one and never overwrites or adopts files — open the existing workspace, or choose an empty directory",
                    target.display(),
                    marker
                ),
            ));
        }
    }

    // The document: readable UTF-8 text that declares OpenAPI 3.x or
    // Swagger 2.0 and yields a valid inventory.
    let bytes = std::fs::read(spec)
        .map_err(|e| fail(2, "spec-unreadable", format!("--spec {}: {}", spec, e)))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        fail(
            2,
            "spec-unreadable",
            format!(
                "--spec {}: not UTF-8 text; the working copy is edited and patched as text",
                spec
            ),
        )
    })?;
    let doc = crate::spec::load(text, spec).map_err(|e| {
        fail(
            2,
            "spec-invalid",
            format!("--spec {} does not parse as JSON or YAML: {}", spec, e),
        )
    })?;
    let (kind, version) = crate::sources::detect(&doc);
    let supported = match kind {
        "openapi" => version.starts_with("3."),
        "swagger" => version == "2.0",
        _ => false,
    };
    if !supported {
        return Err(fail(
            2,
            "spec-unsupported",
            if kind == "unknown" {
                format!(
                    "--spec {} declares neither `openapi` nor `swagger`; init reads OpenAPI 3.x and Swagger 2.0 description documents only",
                    spec
                )
            } else {
                format!(
                    "--spec {} declares {} {}; init reads OpenAPI 3.x and Swagger 2.0 only",
                    spec, kind, version
                )
            },
        ));
    }

    // The working copy is named for its dialect, in its own format.
    let ext = if text.trim_start().starts_with('{') {
        "json"
    } else if spec.to_ascii_lowercase().ends_with(".yml") {
        "yml"
    } else {
        "yaml"
    };
    let working = format!("{}.{}", kind, ext);
    let upstream = crate::sources::default_upstream(&working);
    let candidate = crate::sources::SourceEntry {
        kind: kind.to_string(),
        version: Some(version.clone()),
        path: working.clone(),
        upstream: Some(upstream.clone()),
        upstream_sha256: None,
        patches: vec![],
    };
    if let Some(fault) = crate::sources::not_followed(&candidate, &[]) {
        return Err(fail(
            2,
            "internal",
            format!(
                "internal error — the new entry would not be followed: {}",
                fault.describe()
            ),
        ));
    }

    let (built, format) = crate::cmd::inventory::build_from_text(text, &working).map_err(|e| {
        fail(
            2,
            "spec-invalid",
            format!(
                "--spec {}: the inventory cannot be built from it: {}",
                spec, e
            ),
        )
    })?;
    let contract = crate::cmd::inventory::schema_errors(&built.inventory, None);
    if !contract.is_empty() {
        return Err(fail(
            2,
            "spec-invalid",
            format!(
                "--spec {}: the built inventory does not satisfy schemas/inventory.schema.json ({} errors; first: {})",
                spec,
                contract.len(),
                contract[0]
            ),
        ));
    }

    let now = crate::now_iso();
    let created_at = args.get("created-at").unwrap_or(&now).to_string();
    let retrieved_at = args.get("retrieved-at").unwrap_or(&now).to_string();
    let upstream_sha256 = crate::patch::bytes_sha256(&bytes);
    let content_sha256 = crate::patch::canonical_sha256(&doc);

    let workspace_text = workspace_yaml(&names, &created_at, context_mode.as_deref());
    let workspace_errors = crate::yaml::parse(&workspace_text)
        .map(|v| {
            crate::jsonschema::validate(
                &v,
                &crate::schemas::load("workspace.schema.json", None).unwrap_or(Value::Null),
            )
        })
        .unwrap_or_else(|e| vec![e]);
    if !workspace_errors.is_empty() {
        return Err(fail(
            1,
            "usage",
            format!(
                "workspace.yaml would not satisfy schemas/workspace.schema.json: {} (check --created-at: an RFC 3339 date-time)",
                workspace_errors.join("; ")
            ),
        ));
    }

    let lock_text = crate::cmd::sources::pinned_lock_text(
        "",
        &crate::cmd::sources::PinRecord {
            kind,
            version: &version,
            url: args.get("url"),
            retrieved_at: Some(&retrieved_at),
            path: &working,
            upstream: &upstream,
            upstream_sha256: &upstream_sha256,
            legacy_sha256: false,
        },
    )
    .map_err(|e| fail(2, "internal", format!("internal error — {}", e)))?;

    // The files the target adds. One that already exists is the caller's:
    // left alone and reported, never overwritten, and not a refusal.
    let target_files: Vec<PlannedFile> = {
        let workspace = crate::yaml::parse(&workspace_text).unwrap_or(Value::Null);
        (crate::target::active().init_files)(&crate::target::InitInput {
            workspace: &workspace,
            inventory: &built.inventory,
        })
        .into_iter()
        .map(|(path, text)| PlannedFile {
            path: path.to_string_lossy().into_owned(),
            bytes: text.into_bytes(),
        })
        .collect()
    };
    let mut files = vec![
        PlannedFile {
            path: WORKSPACE_FILE.to_string(),
            bytes: workspace_text.into_bytes(),
        },
        PlannedFile {
            path: working.clone(),
            bytes: bytes.clone(),
        },
        PlannedFile {
            path: upstream.clone(),
            bytes,
        },
        PlannedFile {
            path: crate::sources::SOURCES_LOCK.to_string(),
            bytes: lock_text.into_bytes(),
        },
        PlannedFile {
            path: crate::cmd::inventory::DEFAULT_INVENTORY_REL.to_string(),
            bytes: pretty(&built.inventory).into_bytes(),
        },
    ];
    // Created, never overwritten: the markers above cover `.factory/`, and
    // this covers the working copy.
    if let Some(f) = files.iter().find(|f| exists(&target.join(&f.path))) {
        return Err(fail(
            2,
            "file-exists",
            format!(
                "{} already exists in {}; init creates every file it lists and never overwrites or adopts one — move it aside (pass it as --spec from elsewhere) and run init again",
                f.path,
                target.display()
            ),
        ));
    }

    let (kept, target_files): (Vec<PlannedFile>, Vec<PlannedFile>) = target_files
        .into_iter()
        .partition(|f| place_taken(target, &f.path));
    let left_alone: Vec<String> = kept.into_iter().map(|f| f.path).collect();
    files.extend(target_files);

    let ops = get_arr(&built.inventory, "operations")
        .cloned()
        .unwrap_or_default();
    let (supported_ops, needs_review, unsupported) = crate::cmd::inventory::tally(&ops);
    let inventory = crate::json::object(vec![
        (
            "title",
            Value::from(
                get(&built.inventory, "api")
                    .and_then(|a| get_str(a, "title"))
                    .unwrap_or(""),
            ),
        ),
        ("format", Value::from(format.to_string())),
        ("operations", Value::from(ops.len())),
        ("supported", Value::from(supported_ops)),
        ("needs_review", Value::from(needs_review)),
        ("unsupported", Value::from(unsupported)),
        (
            "shapes",
            Value::from(
                get(&built.inventory, "shapes")
                    .and_then(Value::as_object)
                    .map(|o| o.len())
                    .unwrap_or(0),
            ),
        ),
        (
            "unresolved",
            Value::from(
                get_arr(&built.inventory, "unresolved")
                    .map(|a| a.len())
                    .unwrap_or(0),
            ),
        ),
        (
            "warnings",
            Value::Array(
                built
                    .warnings
                    .iter()
                    .map(|w| Value::from(w.as_str()))
                    .collect(),
            ),
        ),
    ]);

    Ok(Plan {
        names,
        context_mode,
        kind,
        version,
        working,
        upstream,
        upstream_sha256,
        content_sha256,
        inventory,
        files,
        left_alone,
    })
}

/// `.factory/workspace.yaml` for a new spec-backed REST workspace, in the
/// pilots' layout.
fn workspace_yaml(names: &Names, created_at: &str, context_mode: Option<&str>) -> String {
    let s = crate::yaml::scalar;
    let mut out = String::new();
    out.push_str("contract_version: 1\n");
    out.push_str(&format!("service: {}\n", s(&names.service)));
    out.push_str(&format!("directory: {}\n", s(&names.directory)));
    out.push_str(&format!("type_prefix: {}\n", s(&names.type_prefix)));
    out.push_str(&format!("field_prefix: {}\n", s(&names.field_prefix)));
    out.push_str(&format!(
        "skill:\n  name: {}\n",
        s(crate::target::active().name)
    ));
    out.push_str(&format!("  version: {}\n", env!("CARGO_PKG_VERSION")));
    out.push_str("source_kind: rest\n");
    out.push_str(&format!("connect_spec: {}\n", CONNECT_SPEC));
    out.push_str(&format!("federation_version: \"{}\"\n", FEDERATION_VERSION));
    out.push_str("intake: spec\n");
    // Plain, as the pilots write it, when it is timestamp-shaped (YAML reads
    // it back as the same string); quoted otherwise, so no value can add a key.
    let plain = !created_at.is_empty()
        && created_at
            .chars()
            .all(|c| c.is_ascii_digit() || "TZtz:.+-".contains(c));
    out.push_str(&format!(
        "created_at: {}\n",
        if plain {
            created_at.to_string()
        } else {
            s(created_at)
        }
    ));
    if let Some(mode) = context_mode {
        out.push_str(&format!("context_mode: {}\n", mode));
    }
    out
}

/// Write every planned file, in order, creating each (never overwriting).
/// `.factory/*` goes through custody (ADR 0025); the working copy is an
/// ordinary workspace file.
fn write_all(target: &Path, files: &[PlannedFile]) -> Result<(), Failure> {
    std::fs::create_dir_all(target)
        .map_err(|e| fail(2, "write-failed", format!("{}: {}", target.display(), e)))?;
    let mut written: Vec<String> = Vec::new();
    for f in files {
        // A target file whose place is taken is refused below, not written;
        // it must not be reported as written.
        let blocked = !f.path.starts_with(".factory/") && place_taken(target, &f.path);
        let result = if f.path.starts_with(".factory/") {
            crate::factory_io::create_parent_dir(target, &f.path)
                .and_then(|_| crate::factory_io::create_new(target, &f.path, &f.bytes))
                .map_err(|e| e.to_string())
        } else {
            // A target file may sit in a subdirectory (`tests/`).
            // Planning leaves a symlinked directory's files alone; this is
            // the same check again at write time, so a link made since
            // refuses the write instead of carrying it elsewhere.
            let path = target.join(&f.path);
            if blocked {
                Err(format!(
                    "{}: it, or a directory above it, already exists or is a symlink",
                    f.path
                ))
            } else {
                path.parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&path)
                    })
                    .and_then(|mut file| file.write_all(&f.bytes))
                    .map_err(|e| format!("{}: {}", f.path, e))
            }
        };
        if let Err(e) = result {
            // A create_new that failed may still have left the file behind.
            if !blocked && exists(&target.join(&f.path)) {
                written.push(f.path.clone());
            }
            return Err(if written.is_empty() {
                fail(2, "write-failed", e)
            } else {
                Failure {
                    exit: 4,
                    code: "incomplete",
                    message: format!(
                        "INCOMPLETE — {}; written so far: {}. Remove them and run init again",
                        e,
                        written.join(", ")
                    ),
                    written: Some(written),
                }
            });
        }
        written.push(f.path.clone());
    }
    Ok(())
}

fn file_value(f: &PlannedFile) -> Value {
    // Every planned file is UTF-8: the document was refused otherwise, and
    // the rest is generated text.
    crate::json::object(vec![
        ("path", Value::from(f.path.as_str())),
        (
            "content",
            Value::from(String::from_utf8_lossy(&f.bytes).as_ref()),
        ),
        ("sha256", Value::from(crate::patch::bytes_sha256(&f.bytes))),
        ("bytes", Value::from(f.bytes.len())),
    ])
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const FLAGS: Flags = Flags {
    boolean: &["dry-run", "json"],
    valued: &[
        "name",
        "spec",
        "url",
        "retrieved-at",
        "created-at",
        "context-mode",
        "target",
    ],
};

pub fn main(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &FLAGS);
    if args.has("help") || args.positional.iter().any(|p| p == "-h") {
        println!("{}", USAGE);
        return 0;
    }
    let json = args.has("json");
    let target: PathBuf = PathBuf::from(args.dir());
    let dry_run = args.has("dry-run");
    let result = plan(&args, &target).and_then(|p| {
        if !dry_run {
            write_all(&target, &p.files)?;
        }
        Ok(p)
    });
    let p = match result {
        Ok(p) => p,
        Err(f) => {
            eprintln!("init: {}", f.message);
            if f.exit == 1 {
                eprintln!("{}", USAGE);
            }
            if json {
                let mut pairs = vec![
                    ("error", Value::from(f.message.as_str())),
                    ("code", Value::from(f.code)),
                    ("exit", Value::from(f.exit)),
                ];
                if let Some(w) = &f.written {
                    pairs.push((
                        "written",
                        Value::Array(w.iter().map(|p| Value::from(p.as_str())).collect()),
                    ));
                }
                print!("{}", pretty(&crate::json::object(pairs)));
            }
            return f.exit;
        }
    };

    if json {
        let summary = crate::json::object(vec![
            ("workspace", Value::from(args.dir())),
            ("service", Value::from(p.names.service.as_str())),
            ("directory", Value::from(p.names.directory.as_str())),
            ("type_prefix", Value::from(p.names.type_prefix.as_str())),
            ("field_prefix", Value::from(p.names.field_prefix.as_str())),
            (
                "context_mode",
                p.context_mode
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("path", Value::from(p.working.as_str())),
            ("kind", Value::from(p.kind)),
            ("version", Value::from(p.version.as_str())),
            ("upstream", Value::from(p.upstream.as_str())),
            ("upstream_sha256", Value::from(p.upstream_sha256.as_str())),
            ("content_sha256", Value::from(p.content_sha256.as_str())),
            ("inventory", p.inventory.clone()),
            ("dry_run", Value::Bool(dry_run)),
            (
                "files",
                Value::Array(p.files.iter().map(file_value).collect()),
            ),
            (
                "left_alone",
                Value::Array(
                    p.left_alone
                        .iter()
                        .map(|s| Value::from(s.as_str()))
                        .collect(),
                ),
            ),
            ("exit", Value::from(0)),
        ]);
        print!("{}", pretty(&summary));
        return 0;
    }

    println!(
        "init{}: {} workspace {} (service {}) in {}",
        if dry_run { " (dry run)" } else { "" },
        if dry_run { "would create" } else { "created" },
        p.names.directory,
        p.names.service,
        target.display()
    );
    let inv = &p.inventory;
    let n = |k: &str| get(inv, k).and_then(Value::as_u64).unwrap_or(0);
    for f in &p.files {
        let note = if f.path == p.working {
            format!("  (working copy, {} {})", p.kind, p.version)
        } else if f.path == p.upstream {
            "  (vendor copy; never edited)".to_string()
        } else if f.path == crate::cmd::inventory::DEFAULT_INVENTORY_REL {
            format!(
                "  ({} operations: {} supported, {} needs_review, {} unsupported; {} shapes, {} unresolved)",
                n("operations"),
                n("supported"),
                n("needs_review"),
                n("unsupported"),
                n("shapes"),
                n("unresolved")
            )
        } else {
            String::new()
        };
        println!("  {}{}", f.path, note);
    }
    for path in &p.left_alone {
        println!("  {}  (exists, left alone)", path);
    }
    for w in get_arr(inv, "warnings").into_iter().flatten() {
        println!("  warning: {}", w.as_str().unwrap_or(""));
    }
    match p.context_mode.as_deref() {
        Some(mode) => println!("  context_mode: {}", mode),
        None => println!(
            "  context_mode: not recorded — read as generic; if the user wants customer-specific fields, set it in {} (SKILL.md step 3)",
            WORKSPACE_FILE
        ),
    }
    if dry_run {
        println!("  nothing written (--dry-run)");
    } else {
        println!(
            "  init never runs git: standalone, `git init` the directory; inside a repository, let it track the folder. Then commit."
        );
    }
    0
}

//! Customer-context readiness, independent of schema generation or network access.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub const FILE: &str = ".factory/context.yaml";

#[derive(Debug, Deserialize)]
struct Snapshot {
    path: String,
    sha256: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    byte_count: Option<u64>,
    #[serde(default)]
    representation: Option<String>,
    #[serde(default)]
    refresh_by: Option<String>,
    #[serde(default)]
    derived_from: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Requirement {
    id: String,
    phase: String,
    affects: Vec<String>,
    reason: String,
    resolve_with: String,
    status: String,
    #[serde(default)]
    evidence: Vec<Snapshot>,
}

#[derive(Debug, Deserialize)]
struct Context {
    inputs: Vec<Snapshot>,
    requirements: Vec<Requirement>,
}

#[derive(Debug, Serialize)]
pub struct Blocker {
    pub id: String,
    pub phase: String,
    pub affects: Vec<String>,
    pub reason: String,
    pub resolve_with: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub contract_version: u8,
    pub mode: Option<String>,
    pub build_ready: bool,
    pub live_ready: bool,
    pub errors: Vec<String>,
    pub blockers: Vec<Blocker>,
}

impl Report {
    pub fn exit_code(&self, phase: &str) -> i32 {
        if !self.errors.is_empty() {
            1
        } else if if phase == "live" {
            self.live_ready
        } else {
            self.build_ready
        } {
            0
        } else {
            2
        }
    }

    fn block(&mut self, id: &str, phase: &str, reason: String, resolve_with: &str) {
        self.blockers.push(Blocker {
            id: id.into(),
            phase: phase.into(),
            affects: vec!["declared service scope".into()],
            reason,
            resolve_with: resolve_with.into(),
        });
    }
}

fn snapshot_issues(dir: &Path, snapshot: &Snapshot) -> Vec<String> {
    let mut issues = Vec::new();
    let path = Path::new(&snapshot.path);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return vec![format!(
            "{}: expected a workspace-relative file",
            snapshot.path
        )];
    }
    let checked: Result<(), String> = (|| {
        // A captured artifact lives under `.factory/context-artifacts/`, so
        // custody owns it; a snapshot of an authored file beside it keeps the
        // canonicalize-and-contain check (ADR 0025).
        let bytes = if snapshot
            .path
            .starts_with(&format!("{}/", crate::factory_io::STATE_DIR))
        {
            crate::factory_io::read(dir, &snapshot.path).map_err(|e| {
                // The message already names the relative path; the caller
                // prefixes it again, so hand back only the reason.
                e.to_string()
                    .strip_prefix(&format!("{}: ", snapshot.path))
                    .map(str::to_string)
                    .unwrap_or_else(|| e.to_string())
            })?
        } else {
            let root = dir.canonicalize().map_err(|e| e.to_string())?;
            let file = dir.join(path).canonicalize().map_err(|e| e.to_string())?;
            if !file.starts_with(root) {
                return Err("file resolves outside the workspace".into());
            }
            std::fs::read(file).map_err(|e| e.to_string())?
        };
        if let Some(expected) = snapshot.byte_count {
            if bytes.len() as u64 != expected {
                issues.push(format!(
                    "{}: byte count changed from {} to {}; reassess the dependent context before recapturing",
                    snapshot.path,
                    expected,
                    bytes.len()
                ));
            }
        }
        if format!("{:x}", Sha256::digest(&bytes)) != snapshot.sha256 {
            issues.push(format!(
                "{}: content changed (SHA-256 mismatch); reassess the dependent context before recapturing",
                snapshot.path
            ));
        }
        Ok(())
    })();
    if let Err(e) = checked {
        issues.push(format!("{}: {}", snapshot.path, e));
    }
    if snapshot
        .refresh_by
        .as_deref()
        .map(|date| crate::today().as_str() > date)
        .unwrap_or(false)
    {
        issues.push(format!(
            "{}: refresh was due {}; recapture from the declared source and reassess",
            snapshot.path,
            snapshot.refresh_by.as_deref().unwrap_or("")
        ));
    }
    issues
}

/// The assessment result lives in workspace.yaml, not the companion file, so a
/// generic wrapper needs no context.yaml. A readable workspace.yaml with no
/// `context_mode` key is `Ok(None)`, which `check` reads as generic (ADR 0081).
/// A workspace.yaml that is missing, refused, unreadable or unparseable, or a
/// marker that is not a string, is an error: only an absent key is generic.
fn workspace_mode(dir: &Path) -> Result<Option<String>, String> {
    let text = crate::factory_io::read_to_string(dir, ".factory/workspace.yaml")
        .map_err(|e| e.to_string())?;
    let value = crate::yaml::parse(&text).map_err(|e| format!(".factory/workspace.yaml: {}", e))?;
    match value.get("context_mode") {
        None => Ok(None),
        Some(mode) => mode.as_str().map(|m| Some(m.to_string())).ok_or_else(|| {
            "workspace.yaml records a context_mode that is not a string; set it to generic, specialized, or undecided".to_string()
        }),
    }
}

/// Is there a `.factory/context.yaml` at all? A symlinked one counts as
/// present and is refused when read: "there is a file here that the factory
/// will not follow" is a different report from "there is no file here"
/// (ADR 0025).
pub fn file_present(dir: &Path) -> bool {
    match crate::factory_io::symlink_metadata(dir, FILE) {
        Ok(found) => found.is_some(),
        Err(e) => e.is_refusal(),
    }
}

/// Check declared requirements only. Discovering an undeclared dependency is
/// the agent's job; a passing report does not establish API correctness.
pub fn check(dir: &Path, schemas_dir: Option<&Path>) -> Report {
    let mut report = Report {
        contract_version: 1,
        mode: None,
        build_ready: false,
        live_ready: false,
        errors: vec![],
        blockers: vec![],
    };
    let marker = match workspace_mode(dir) {
        Ok(marker) => marker,
        Err(e) => {
            report.errors.push(e);
            return report;
        }
    };
    report.mode = marker.clone();
    let file_present = file_present(dir);

    // An unrecorded mode is a generic wrapper. The marker is self-attested, so
    // blocking on its absence proved only that nobody had typed a word; the
    // declared requirements below are what carry a real gap (ADR 0081).
    let specialized = match marker.as_deref() {
        None | Some("generic") => false,
        Some("undecided") => {
            report.block(
                "context-scope",
                "build",
                "The intended generic or specialized scope is unresolved".into(),
                "Reuse the user's stated intent; ask only if the scope remains ambiguous.",
            );
            return report;
        }
        Some("specialized") => true,
        Some(other) => {
            report.errors.push(format!(
                "workspace.yaml records an unknown context_mode: {}",
                other
            ));
            return report;
        }
    };

    // Generic wrapping with no tracked requirements needs no companion file.
    if !file_present {
        if specialized {
            report.errors.push("specialized mode requires a .factory/context.yaml recording the scope or metadata that governs the schema".into());
        } else {
            report.build_ready = true;
            report.live_ready = true;
        }
        return report;
    }

    let value = match crate::factory_io::read_to_string(dir, FILE)
        .map_err(String::from)
        .and_then(|text| crate::yaml::parse(&text).map_err(|e| format!("{}: {}", FILE, e)))
    {
        Ok(value) => value,
        Err(e) => {
            report.errors.push(e);
            return report;
        }
    };
    let Some(schema) = crate::schemas::load("context.schema.json", schemas_dir) else {
        report.errors.push("cannot load context.schema.json".into());
        return report;
    };
    report.errors = crate::jsonschema::validate(&value, &schema);
    if !report.errors.is_empty() {
        return report;
    }
    let context: Context = match serde_json::from_value(value) {
        Ok(context) => context,
        Err(e) => {
            report.errors.push(e.to_string());
            return report;
        }
    };
    if specialized && !context.requirements.iter().any(|r| r.phase == "build") {
        report.errors.push("specialized mode requires at least one build requirement recording the scope or metadata that governs the schema".into());
    }
    let snapshots: Vec<&Snapshot> = context
        .inputs
        .iter()
        .chain(context.requirements.iter().flat_map(|r| r.evidence.iter()))
        .collect();
    let mut artifact_ids: HashMap<String, &Snapshot> = HashMap::new();
    for snapshot in &snapshots {
        if let Some(id) = &snapshot.id {
            if artifact_ids.insert(id.clone(), snapshot).is_some() {
                report
                    .errors
                    .push(format!("duplicate context artifact id: {}", id));
            }
        }
    }
    for snapshot in &snapshots {
        for source in &snapshot.derived_from {
            if snapshot.id.as_deref() == Some(source) {
                report.errors.push(format!(
                    "context artifact {} cannot derive from itself",
                    snapshot.id.as_deref().unwrap_or(&snapshot.path)
                ));
                continue;
            }
            if !artifact_ids.contains_key(source) {
                report.errors.push(format!(
                    "context artifact {} names unknown derived_from artifact {}",
                    snapshot.id.as_deref().unwrap_or(&snapshot.path),
                    source
                ));
            }
        }
        if snapshot.representation.as_deref() == Some("raw") && !snapshot.derived_from.is_empty() {
            report.errors.push(format!(
                "raw context artifact {} cannot declare derived_from",
                snapshot.id.as_deref().unwrap_or(&snapshot.path)
            ));
        }
    }
    for snapshot in &context.inputs {
        for reason in snapshot_issues(dir, snapshot) {
            report.block("context-input", "build", reason, "Review the changed input and reassess requirements before recording its current hash.");
        }
    }
    let mut seen = HashSet::new();
    for requirement in context.requirements {
        if !seen.insert(requirement.id.clone()) {
            report
                .errors
                .push(format!("duplicate requirement id: {}", requirement.id));
        }
        let mut reasons = vec![];
        if requirement.status != "resolved" {
            reasons.push(format!("{}: {}", requirement.status, requirement.reason));
        } else {
            for snapshot in &requirement.evidence {
                for issue in snapshot_issues(dir, snapshot) {
                    reasons.push(issue);
                }
            }
        }
        if !reasons.is_empty() {
            report.blockers.push(Blocker {
                id: requirement.id,
                phase: requirement.phase,
                affects: requirement.affects,
                reason: reasons.join("; "),
                resolve_with: requirement.resolve_with,
            });
        }
    }
    report.build_ready =
        report.errors.is_empty() && !report.blockers.iter().any(|b| b.phase == "build");
    report.live_ready = report.build_ready && report.blockers.is_empty();
    report
}

#[derive(Debug)]
pub struct CaptureOptions {
    pub requirement: Option<String>,
    pub input: bool,
    pub id: String,
    pub from: PathBuf,
    pub representation: String,
    pub authority: String,
    pub format: String,
    pub captured_at: String,
    pub capture_method: String,
    pub source: String,
    pub scope: Vec<String>,
    pub refresh_by: Option<String>,
    pub derived_from: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CaptureReport {
    pub id: String,
    pub path: String,
    pub sha256: String,
    pub byte_count: u64,
    pub requirement: Option<String>,
    pub input: bool,
}

fn artifact_destination(id: &str, from: &Path) -> String {
    let extension = from
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.chars().all(|c| c.is_ascii_alphanumeric()))
        .map(|e| format!(".{}", e))
        .unwrap_or_default();
    format!(".factory/context-artifacts/{}/artifact{}", id, extension)
}

fn validate_format(bytes: &[u8], format: &str) -> Result<(), String> {
    match format {
        "opaque" => Ok(()),
        "json" => {
            let text = std::str::from_utf8(bytes)
                .map_err(|e| format!("captured JSON is not UTF-8: {}", e))?;
            crate::json::parse(text)
                .map(|_| ())
                .map_err(|e| format!("captured JSON does not parse: {}", e))
        }
        "yaml" => {
            let text = std::str::from_utf8(bytes)
                .map_err(|e| format!("captured YAML is not UTF-8: {}", e))?;
            crate::yaml::parse(text)
                .map(|_| ())
                .map_err(|e| format!("captured YAML does not parse: {}", e))
        }
        other => Err(format!(
            "--format must be opaque, json, or yaml (got {})",
            other
        )),
    }
}

fn credential_issue(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let patterns = [
        ("private key", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
        (
            "bearer credential",
            r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{16,}",
        ),
        ("basic credential", r"(?i)\bBasic\s+[A-Za-z0-9+/=]{16,}"),
        (
            "credential-valued field",
            r#"(?im)^\s*[\"']?(authorization|access[-_]?token|refresh[-_]?token|id[-_]?token|client[-_]?secret|api[-_]?key|password)[\"']?\s*[:=]\s*[\"']?[A-Za-z0-9._~+/=-]{8,}"#,
        ),
    ];
    patterns.iter().find_map(|(name, pattern)| {
        regex::Regex::new(pattern)
            .ok()
            .filter(|re| re.is_match(&text))
            .map(|_| (*name).to_string())
    })
}

/// Copy one already-local artifact byte for byte and record it in the
/// conditional context contract. This function performs no network access and
/// has no credential input. Hash and byte count are computed before optional
/// JSON/YAML parsing, so parser normalization cannot change the evidence.
pub fn capture(
    dir: &Path,
    options: CaptureOptions,
    schemas_dir: Option<&Path>,
) -> Result<CaptureReport, String> {
    if options.input == options.requirement.is_some() {
        return Err("choose exactly one of --input or --requirement ID".into());
    }
    if !regex::Regex::new(r"^[a-z][a-z0-9-]*$")
        .unwrap()
        .is_match(&options.id)
    {
        return Err("--id must match ^[a-z][a-z0-9-]*$".into());
    }
    if !matches!(
        options.representation.as_str(),
        "raw" | "derived" | "transcribed"
    ) {
        return Err("--representation must be raw, derived, or transcribed".into());
    }
    if !matches!(options.authority.as_str(), "authoritative" | "supporting") {
        return Err("--authority must be authoritative or supporting".into());
    }
    if options.authority == "authoritative" && options.representation != "raw" {
        return Err("only a raw artifact can be authoritative".into());
    }
    if options.representation == "raw" && !options.derived_from.is_empty() {
        return Err("a raw artifact cannot use --derived-from".into());
    }
    if options.representation != "raw" && options.derived_from.is_empty() {
        return Err("derived and transcribed artifacts require --derived-from ID".into());
    }
    if options.scope.is_empty() {
        return Err("at least one --scope is required".into());
    }
    if options.capture_method.trim().is_empty() || options.source.trim().is_empty() {
        return Err("--capture-method and --source must be nonempty".into());
    }

    // Read and fingerprint exact bytes before any parser sees the artifact.
    let bytes =
        std::fs::read(&options.from).map_err(|e| format!("{}: {}", options.from.display(), e))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let byte_count = bytes.len() as u64;
    if let Some(kind) = credential_issue(&bytes) {
        return Err(format!(
            "refusing an artifact that appears to contain a {}; capture non-secret metadata or administrative attestation instead",
            kind
        ));
    }
    validate_format(&bytes, &options.format)?;

    let mut context = crate::yaml::parse(&crate::factory_io::read_to_string(dir, FILE)?)
        .map_err(|e| format!("{}: {}", FILE, e))?;
    let destination = artifact_destination(&options.id, &options.from);
    let mut record = crate::json::obj();
    record.insert("id".into(), serde_json::Value::from(options.id.as_str()));
    record.insert("path".into(), serde_json::Value::from(destination.as_str()));
    record.insert("sha256".into(), serde_json::Value::from(sha256.as_str()));
    record.insert("byte_count".into(), serde_json::Value::from(byte_count));
    record.insert(
        "representation".into(),
        serde_json::Value::from(options.representation.as_str()),
    );
    record.insert(
        "authority".into(),
        serde_json::Value::from(options.authority.as_str()),
    );
    record.insert(
        "format".into(),
        serde_json::Value::from(options.format.as_str()),
    );
    record.insert(
        "captured_at".into(),
        serde_json::Value::from(options.captured_at.as_str()),
    );
    record.insert(
        "capture_method".into(),
        serde_json::Value::from(options.capture_method.as_str()),
    );
    record.insert(
        "source".into(),
        serde_json::Value::from(options.source.as_str()),
    );
    record.insert(
        "scope".into(),
        serde_json::Value::Array(
            options
                .scope
                .iter()
                .map(|s| Value::from(s.as_str()))
                .collect(),
        ),
    );
    if let Some(refresh_by) = &options.refresh_by {
        record.insert("refresh_by".into(), Value::from(refresh_by.as_str()));
    }
    if !options.derived_from.is_empty() {
        record.insert(
            "derived_from".into(),
            Value::Array(
                options
                    .derived_from
                    .iter()
                    .map(|s| Value::from(s.as_str()))
                    .collect(),
            ),
        );
    }
    let record = Value::Object(record);

    let target = if options.input {
        context
            .get_mut("inputs")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "context.yaml has no inputs array".to_string())?
    } else {
        let id = options.requirement.as_deref().unwrap_or("");
        let requirements = context
            .get_mut("requirements")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "context.yaml has no requirements array".to_string())?;
        let requirement = requirements
            .iter_mut()
            .find(|r| crate::json::get_str(r, "id") == Some(id))
            .ok_or_else(|| format!("unknown requirement id: {}", id))?;
        if requirement.get("evidence").is_none() {
            crate::json::set(requirement, "evidence", Value::Array(vec![]));
        }
        requirement
            .get_mut("evidence")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| format!("requirement {} has no evidence array", id))?
    };
    if let Some(existing) = target
        .iter_mut()
        .find(|item| crate::json::get_str(item, "id") == Some(options.id.as_str()))
    {
        *existing = record;
    } else {
        target.push(record);
    }

    let schema = crate::schemas::load("context.schema.json", schemas_dir)
        .ok_or_else(|| "cannot load context.schema.json".to_string())?;
    let errors = crate::jsonschema::validate(&context, &schema);
    if !errors.is_empty() {
        return Err(format!("context.yaml after capture: {}", errors.join("; ")));
    }
    // Cross-record checks (duplicate ids and derived_from references) run on
    // the proposed document before either file is replaced.
    let proposed: Context = serde_json::from_value(context.clone()).map_err(|e| e.to_string())?;
    let all: Vec<&Snapshot> = proposed
        .inputs
        .iter()
        .chain(proposed.requirements.iter().flat_map(|r| r.evidence.iter()))
        .collect();
    let ids: HashSet<&str> = all.iter().filter_map(|s| s.id.as_deref()).collect();
    if ids.len() != all.iter().filter(|s| s.id.is_some()).count() {
        return Err("context.yaml would contain duplicate context artifact ids".into());
    }
    for source in &options.derived_from {
        if source == &options.id {
            return Err(format!(
                "context artifact {} cannot derive from itself",
                source
            ));
        }
        if !ids.contains(source.as_str()) {
            return Err(format!(
                "--derived-from names unknown artifact id: {}",
                source
            ));
        }
    }

    // `capture` is the one writer that needs two files to land together, so
    // it keeps temp+rename instead of writing in place — but both
    // destinations are proven regular files inside the workspace first, and
    // both temporaries are custody's own (ADR 0025).
    let artifact_tmp_rel = format!("{}.capture.tmp", destination);
    let context_tmp_rel = format!("{}.tmp", FILE);
    crate::factory_io::create_parent_dir(dir, &destination)?;
    let artifact_tmp = crate::factory_io::checked_path(dir, &artifact_tmp_rel)?;
    let context_tmp = crate::factory_io::checked_path(dir, &context_tmp_rel)?;
    crate::factory_io::write_in_place(dir, &artifact_tmp_rel, &bytes)?;
    crate::factory_io::write_in_place(
        dir,
        &context_tmp_rel,
        crate::yaml::stringify(&context, 0).as_bytes(),
    )?;
    crate::factory_io::rename_into_place(dir, &destination, &artifact_tmp)?;
    crate::factory_io::rename_into_place(dir, FILE, &context_tmp)?;

    Ok(CaptureReport {
        id: options.id,
        path: destination,
        sha256,
        byte_count,
        requirement: options.requirement,
        input: options.input,
    })
}

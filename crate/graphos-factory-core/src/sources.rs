//! Pinned sources: the description documents a workspace was built from.
//!
//! `.factory/sources.lock.yaml` names every input. A *document* entry
//! (`kind: openapi` for OpenAPI 3.x, `kind: swagger` for Swagger 2.0) pins two
//! copies: `upstream`, the vendor's bytes exactly as retrieved and never
//! edited, and `path`, the working copy the reader reads and people edit. The
//! difference between them is the entry's `patches` (JSON Patch, each with a
//! reason). `applied.lock.yaml` records the working copy's content hash as
//! last acknowledged, so an edit nobody has codified is detected the same way
//! a schema hand edit is.
//!
//! This module reads the lock, detects a document's kind, computes each
//! source's status, and edits the lock text in place (keys and the `patches`
//! block of one entry) so the agent's notes and folded scalars survive.

use crate::json::{get, get_arr, get_str, obj};
use serde_json::Value;
use std::path::Path;

pub const SOURCES_LOCK: &str = ".factory/sources.lock.yaml";
pub const DOCUMENT_KINDS: [&str; 2] = ["openapi", "swagger"];

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub kind: String,
    pub version: Option<String>,
    pub path: String,
    pub upstream: Option<String>,
    pub upstream_sha256: Option<String>,
    pub patches: Vec<Value>,
}

pub fn read_sources_lock(dir: &Path) -> Result<Option<Value>, String> {
    let text = match crate::factory_io::read_to_string_optional(dir, SOURCES_LOCK)? {
        Some(text) => text,
        None => return Ok(None),
    };
    crate::yaml::parse(&text).map(Some).map_err(|e| {
        if e.starts_with(SOURCES_LOCK) || e.contains("sources.lock.yaml") {
            e
        } else {
            format!("{}: {}", SOURCES_LOCK, e)
        }
    })
}

/// A workspace-relative path that stays inside the workspace: not absolute,
/// no `..` component. Both copies of a pinned document must satisfy it, or
/// the workspace would not carry them.
pub fn inside_workspace(rel: &str) -> bool {
    let p = Path::new(rel);
    !p.is_absolute()
        && !p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Where a vendor copy may live: inside the workspace and directly under
/// `.factory/sources/`, a directory reserved for vendor copies (a working
/// copy is never pinned from there), so `sources pin --force` can never
/// overwrite a working copy, the schema, or any other tracked file through a
/// hand-written `upstream:`.
pub const UPSTREAM_DIR: &str = ".factory/sources/";

pub fn valid_upstream(rel: &str) -> bool {
    inside_workspace(rel)
        && rel.starts_with(UPSTREAM_DIR)
        && rel.len() > UPSTREAM_DIR.len()
        && !rel[UPSTREAM_DIR.len()..].contains('/')
}

/// The vendor copy an entry names, or the default when it records none.
pub fn resolved_upstream(e: &SourceEntry) -> String {
    e.upstream
        .clone()
        .unwrap_or_else(|| default_upstream(&e.path))
}

/// Why an entry is not acted on: nothing is read or written through it until
/// the lock is fixed. `.factory/sources/` is reserved for vendor copies, and
/// the two copies of every entry must be distinct files, so that
/// `sources pin --force` can never overwrite a document someone edits.
#[derive(Debug, Clone, PartialEq)]
pub enum NotFollowed {
    /// `path` or `upstream` is absolute or climbs out of the workspace.
    OutsideWorkspace(String),
    /// `path` is under `.factory/sources/`, the vendor copies' directory.
    PathInReservedDir,
    /// `upstream` is not a file directly under `.factory/sources/`.
    UpstreamMisplaced(String),
    /// `path` and `upstream` name one file.
    SameFile,
    /// Another entry pins the same `path`.
    DuplicatePath,
    /// `upstream` is another entry's working copy.
    UpstreamIsWorkingCopyOf(String),
    /// Another entry resolves to the same vendor copy.
    SharesUpstreamWith(String),
}

impl NotFollowed {
    pub fn is_outside(&self) -> bool {
        matches!(self, NotFollowed::OutsideWorkspace(_))
    }

    /// Whether the fault is an overwrite hazard for `sources pin --force`
    /// (a duplicate `path` is a bookkeeping fault, not one).
    pub fn is_overwrite_hazard(&self) -> bool {
        !matches!(self, NotFollowed::DuplicatePath)
    }

    /// One sentence naming the fault and the key to fix.
    pub fn describe(&self) -> String {
        match self {
            NotFollowed::OutsideWorkspace(p) => format!(
                "{} is outside the workspace — both copies are committed with the workspace; use paths relative to its root with no `..`",
                p
            ),
            NotFollowed::PathInReservedDir => format!(
                "its `path` is under {}, which is reserved for vendor copies — move the working copy elsewhere in the workspace",
                UPSTREAM_DIR
            ),
            NotFollowed::UpstreamMisplaced(u) => format!(
                "its `upstream` {} is not a file directly under {} — a vendor copy lives there, never at a working copy, the schema or any other file",
                u, UPSTREAM_DIR
            ),
            NotFollowed::SameFile => {
                "its `path` and `upstream` name one file — the two copies must be distinct".to_string()
            }
            NotFollowed::DuplicatePath => {
                "another entry pins the same `path` — one entry per document; remove the duplicate"
                    .to_string()
            }
            NotFollowed::UpstreamIsWorkingCopyOf(other) => format!(
                "its `upstream` is the working copy of {} — a vendor copy is never a working copy",
                other
            ),
            NotFollowed::SharesUpstreamWith(other) => format!(
                "it resolves to the same vendor copy as {} — every pinned document keeps its own (record a distinct `upstream`)",
                other
            ),
        }
    }
}

/// Whether `entry` may be acted on, given the lock's other entries.
pub fn not_followed(entry: &SourceEntry, siblings: &[SourceEntry]) -> Option<NotFollowed> {
    let upstream = resolved_upstream(entry);
    if !inside_workspace(&entry.path) {
        return Some(NotFollowed::OutsideWorkspace(entry.path.clone()));
    }
    if !inside_workspace(&upstream) {
        return Some(NotFollowed::OutsideWorkspace(upstream));
    }
    if entry.path == upstream {
        return Some(NotFollowed::SameFile);
    }
    if entry.path.starts_with(UPSTREAM_DIR) {
        return Some(NotFollowed::PathInReservedDir);
    }
    if !valid_upstream(&upstream) {
        return Some(NotFollowed::UpstreamMisplaced(upstream));
    }
    // `siblings` may include the entry itself (the whole lock is passed in);
    // a caller checking a candidate that is not yet in the lock passes the
    // lock as is, so one same-path record is the entry being re-pinned.
    if siblings.iter().filter(|o| o.path == entry.path).count() > 1 {
        return Some(NotFollowed::DuplicatePath);
    }
    for o in siblings.iter().filter(|o| o.path != entry.path) {
        if o.path == upstream {
            return Some(NotFollowed::UpstreamIsWorkingCopyOf(o.path.clone()));
        }
        if resolved_upstream(o) == upstream {
            return Some(NotFollowed::SharesUpstreamWith(o.path.clone()));
        }
    }
    None
}

/// The document entries (openapi / swagger) of a sources lock, in order.
pub fn document_entries(lock: &Value) -> Vec<SourceEntry> {
    get_arr(lock, "sources")
        .into_iter()
        .flatten()
        .filter(|s| {
            get_str(s, "kind")
                .map(|k| DOCUMENT_KINDS.contains(&k))
                .unwrap_or(false)
        })
        .filter_map(|s| {
            Some(SourceEntry {
                kind: get_str(s, "kind")?.to_string(),
                version: get(s, "version").map(crate::spec::value_to_string),
                path: get_str(s, "path")?.trim_start_matches("./").to_string(),
                upstream: get_str(s, "upstream").map(|u| u.trim_start_matches("./").to_string()),
                upstream_sha256: get_str(s, "upstream_sha256").map(str::to_string),
                patches: get_arr(s, "patches").cloned().unwrap_or_default(),
            })
        })
        .collect()
}

/// A lock entry that is not a pinned document: a `docs` page read, a
/// `probe` of the live API, or a kind this binary does not model yet
/// (`postman`, `har`). It is reported as recorded — where it came from, when,
/// its hash when one was recorded, the operations it informed and its note —
/// and never followed: nothing is read or written through it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedSource {
    pub kind: String,
    /// `url` (a docs page), or `base_url` (a probe), as `shown_url` reports it.
    pub url: Option<String>,
    pub retrieved_at: Option<String>,
    /// The page's hash as retrieved, when the agent recorded one.
    pub sha256: Option<String>,
    /// The operation keys it informed: `used_for` (docs), or `operations` (a probe).
    pub operations: Vec<String>,
    pub note: Option<String>,
    /// A probe's credential: the environment variable's name, never its value.
    pub credential: Option<String>,
}

impl RecordedSource {
    pub fn to_value(&self) -> Value {
        let opt = |v: &Option<String>| v.clone().map(Value::from).unwrap_or(Value::Null);
        crate::json::object(vec![
            ("kind", Value::from(self.kind.as_str())),
            ("url", opt(&self.url)),
            ("retrieved_at", opt(&self.retrieved_at)),
            ("sha256", opt(&self.sha256)),
            (
                "operations",
                Value::Array(
                    self.operations
                        .iter()
                        .map(|o| Value::from(o.as_str()))
                        .collect(),
                ),
            ),
            ("note", opt(&self.note)),
            ("credential", opt(&self.credential)),
        ])
    }
}

/// Every entry of a sources lock that `document_entries` does not pin, in
/// lock order: docs pages, probes, kinds not modelled yet, and a document
/// entry with no `path` (which lint reports; here it is still a source the
/// workspace names). Together with `document_entries` this is every entry.
pub fn recorded_entries(lock: &Value) -> Vec<RecordedSource> {
    let text = |s: &Value, key: &str| get(s, key).map(crate::spec::value_to_string);
    let strings = |s: &Value, key: &str| -> Option<Vec<String>> {
        get_arr(s, key).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
    };
    get_arr(lock, "sources")
        .into_iter()
        .flatten()
        .filter(|s| {
            let pinned = get_str(s, "kind")
                .map(|k| DOCUMENT_KINDS.contains(&k))
                .unwrap_or(false)
                && get_str(s, "path").is_some();
            !pinned
        })
        .map(|s| RecordedSource {
            kind: get_str(s, "kind").unwrap_or("unknown").to_string(),
            url: text(s, "url")
                .or_else(|| text(s, "base_url"))
                .map(|u| shown_url(&u)),
            retrieved_at: text(s, "retrieved_at"),
            sha256: get_str(s, "sha256").map(str::to_string),
            operations: strings(s, "used_for")
                .or_else(|| strings(s, "operations"))
                .unwrap_or_default(),
            note: get_str(s, "note").map(str::to_string),
            credential: get_str(s, "credential").map(str::to_string),
        })
        .collect()
}

/// A recorded URL as it may be reported. A docs `url` is written by hand, so
/// it can carry what must not leave the lock: userinfo
/// (`https://user:pass@host/`) is dropped, and the value of a query
/// parameter whose name says it is a credential (`api_key`, `token`, ...,
/// or `key`) becomes `REDACTED`. Anything else, and any text that is not an
/// absolute URL, is returned exactly as recorded.
pub fn shown_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    let mut changed = false;
    if !url.username().is_empty() || url.password().is_some() {
        // Only a URL with a host has userinfo, so neither setter can fail here.
        let _ = url.set_username("");
        let _ = url.set_password(None);
        changed = true;
    }
    let secret = |k: &str| crate::scrub::is_sensitive_key(k) || k.eq_ignore_ascii_case("key");
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    if pairs.iter().any(|(k, _)| secret(k)) {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &pairs {
            query.append_pair(k, if secret(k) { "REDACTED" } else { v });
        }
        url.set_query(Some(&query.finish()));
        changed = true;
    }
    if changed {
        url.to_string()
    } else {
        raw.to_string()
    }
}

/// The recorded (non-document) sources of the workspace; empty when there is
/// no sources lock. An unreadable lock is an error, never "nothing recorded".
pub fn recorded(dir: &Path) -> Result<Vec<RecordedSource>, String> {
    Ok(read_sources_lock(dir)?
        .as_ref()
        .map(recorded_entries)
        .unwrap_or_default())
}

/// What a description document is: ("openapi", "3.0.2"), ("swagger", "2.0"),
/// or ("unknown", "") when it declares neither key.
pub fn detect(doc: &Value) -> (&'static str, String) {
    if let Some(v) = get(doc, "swagger") {
        return ("swagger", crate::spec::value_to_string(v));
    }
    if let Some(v) = get(doc, "openapi") {
        return ("openapi", crate::spec::value_to_string(v));
    }
    ("unknown", String::new())
}

/// `.factory/sources/<name>.upstream.<ext>` for a working copy path, where
/// `<name>` is the whole relative path with its separators turned into `-`
/// (`openapi.json` -> `openapi`, `vendor/v2/openapi.json` ->
/// `vendor-v2-openapi`), so two pinned documents never share one upstream.
pub fn default_upstream(path: &str) -> String {
    let file = Path::new(path);
    let stem = {
        let without_ext = match file.extension() {
            Some(e) => &path[..path.len() - e.len() - 1],
            None => path,
        };
        let joined: String = without_ext
            .trim_start_matches("./")
            .replace(['/', '\\'], "-");
        if joined.is_empty() {
            "source".to_string()
        } else {
            joined
        }
    };
    let ext = file
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    format!(".factory/sources/{}.upstream{}", stem, ext)
}

/// Read a JSON or YAML document relative to the workspace. A vendor copy
/// under `.factory/sources/` goes through custody (ADR 0025); a working copy
/// is an ordinary workspace file the user edits.
pub fn load_document(dir: &Path, rel: &str) -> Result<Value, String> {
    let text = if rel.starts_with(UPSTREAM_DIR) {
        crate::factory_io::read_to_string(dir, rel)?
    } else {
        std::fs::read_to_string(dir.join(rel)).map_err(|e| format!("{}: {}", rel, e))?
    };
    crate::spec::load(&text, rel)
}

/// Everything `lock --check`, `reconcile` and `lint` want to say about one
/// pinned document.
#[derive(Debug, Clone)]
pub struct SourceStatus {
    pub path: String,
    pub declared_kind: String,
    pub detected_kind: String,
    /// The version recorded on the entry (or, when none is, the declared one).
    pub version: String,
    /// The version the document itself declares.
    pub detected_version: String,
    pub upstream: String,
    /// There is an upstream file there, as seen through the entry (see
    /// `working_present`). A file custody refused to read counts as present
    /// and reports itself through `upstream_error`: "not allowed" is never
    /// downgraded to "not there" (ADR 0025).
    pub upstream_present: bool,
    /// Some(false) when the upstream file's bytes no longer hash to `upstream_sha256`.
    pub upstream_ok: Option<bool>,
    /// `path` or `upstream` is absolute or climbs out of the workspace: the
    /// entry is not acted on (nothing is read or written through it).
    pub outside_workspace: bool,
    /// `upstream` is not a file directly under `.factory/sources/`, the working
    /// copy is, or the two copies alias each other or another entry's: the
    /// entry is not acted on either, since `--force` would write there.
    pub upstream_misplaced: bool,
    /// The sentence naming why the entry is not followed (either flag above).
    pub misplaced_reason: Option<String>,
    /// The fault itself, for callers that branch on it.
    pub fault: Option<NotFollowed>,
    /// The upstream exists but could not be read as a document — it does not
    /// parse as JSON or YAML, or custody refused the path: the error.
    pub upstream_error: Option<String>,
    /// The entry records an `upstream_sha256`, i.e. claims a vendor baseline.
    /// Decides the remedy for a missing upstream: restore it (recorded) or
    /// pin it (not yet recorded, the migration path).
    pub upstream_recorded: bool,
    /// The working copy exists on disk, as seen through the entry: false for
    /// an entry that is not followed (any `misplaced_reason`), whatever is on disk.
    pub working_present: bool,
    /// The working copy exists but does not parse as JSON or YAML: the error.
    pub working_error: Option<String>,
    pub content_sha256: Option<String>,
    pub locked_sha256: Option<String>,
    /// The working copy changed since the applied lock acknowledged it.
    pub hand_edit: bool,
    pub patches: usize,
    /// Some(false) when applying `patches` to the upstream does not give the working copy.
    pub patches_ok: Option<bool>,
    pub patches_error: Option<String>,
}

impl SourceStatus {
    /// No applied-lock entry for this source yet.
    pub fn unlocked(&self) -> bool {
        self.locked_sha256.is_none()
    }

    /// What blocks an `apply`: `lock --check` exits 3, `reconcile` is not
    /// clean and `lint` errors on exactly these. `locked` is whether the
    /// workspace has an applied lock at all; without one, nothing is
    /// "unlocked" yet.
    pub fn blocks_apply(&self, locked: bool) -> bool {
        self.outside_workspace
            || self.upstream_misplaced
            || self.upstream_error.is_some()
            || (locked && self.unlocked())
            || self.hand_edit
            || !self.working_present
            || self.working_error.is_some()
            || !self.upstream_present
            || self.upstream_ok == Some(false)
            || self.patches_ok == Some(false)
    }

    pub fn to_value(&self) -> Value {
        crate::json::object(vec![
            ("path", Value::from(self.path.as_str())),
            ("kind", Value::from(self.declared_kind.as_str())),
            ("detected_kind", Value::from(self.detected_kind.as_str())),
            ("version", Value::from(self.version.as_str())),
            (
                "detected_version",
                Value::from(self.detected_version.as_str()),
            ),
            ("upstream", Value::from(self.upstream.as_str())),
            ("outside_workspace", Value::Bool(self.outside_workspace)),
            ("upstream_misplaced", Value::Bool(self.upstream_misplaced)),
            (
                "misplaced_reason",
                self.misplaced_reason
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            (
                "upstream_error",
                self.upstream_error
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("upstream_recorded", Value::Bool(self.upstream_recorded)),
            ("upstream_present", Value::Bool(self.upstream_present)),
            ("working_present", Value::Bool(self.working_present)),
            (
                "upstream_ok",
                self.upstream_ok.map(Value::Bool).unwrap_or(Value::Null),
            ),
            (
                "working_error",
                self.working_error
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
            ("locked", Value::Bool(!self.unlocked())),
            ("hand_edit", Value::Bool(self.hand_edit)),
            ("patches", Value::from(self.patches)),
            (
                "patches_ok",
                self.patches_ok.map(Value::Bool).unwrap_or(Value::Null),
            ),
            (
                "patches_error",
                self.patches_error
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            ),
        ])
    }
}

/// `siblings` are the lock's other document entries: a vendor copy that names
/// another entry's working copy is misplaced too.
pub fn status(
    dir: &Path,
    entry: &SourceEntry,
    siblings: &[SourceEntry],
    applied_lock: Option<&Value>,
) -> SourceStatus {
    let upstream = resolved_upstream(entry);
    let fault = self::not_followed(entry, siblings);
    let outside_workspace = fault.as_ref().map(NotFollowed::is_outside).unwrap_or(false);
    let upstream_misplaced = fault.is_some() && !outside_workspace;
    let misplaced_reason = fault.as_ref().map(NotFollowed::describe);
    let fault_kind = fault.clone();
    // A faulted entry is reported, never followed.
    let not_followed = fault.is_some();
    let working_present = !not_followed && dir.join(&entry.path).exists();
    let (working, working_error) = if not_followed {
        (None, None)
    } else {
        match load_document(dir, &entry.path) {
            Ok(d) => (Some(d), None),
            Err(e) => (None, working_present.then_some(e)),
        }
    };
    let (detected_kind, detected_version) = working
        .as_ref()
        .map(detect)
        .unwrap_or(("unknown", String::new()));
    let content_sha256 = working.as_ref().map(crate::patch::canonical_sha256);
    let locked_sha256 = applied_lock
        .and_then(|l| get(l, "sources"))
        .and_then(|s| {
            get_str(s, &entry.path).or_else(|| {
                // A hand-edited lock may spell the key `./x.json`.
                s.as_object().and_then(|m| {
                    m.iter()
                        .find(|(k, _)| k.trim_start_matches("./") == entry.path)
                        .and_then(|(_, v)| v.as_str())
                })
            })
        })
        .map(str::to_string);
    let hand_edit = match (&content_sha256, &locked_sha256) {
        (Some(c), Some(l)) => c != l,
        _ => false,
    };
    // Not followed means the entry's `upstream` was refused as a path
    // before any of this; otherwise it is under `.factory/sources/` and
    // custody owns it (ADR 0025).
    // Only "there is no file there" is an absence. A refusal — a symlinked
    // `.factory/sources/<name>.upstream.json`, the case ADR 0025 exists to
    // make loud — is a report of its own: collapsing it into `None` would
    // print "no upstream copy … restore it" for a file that is right there
    // and was deliberately not followed.
    let (upstream_bytes, upstream_refused) = if not_followed {
        (None, None)
    } else {
        match crate::factory_io::read_optional(dir, &upstream) {
            Ok(bytes) => (bytes, None),
            Err(e) if e.is_not_found() => (None, None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    let upstream_present = upstream_bytes.is_some() || upstream_refused.is_some();
    let upstream_ok = match (&entry.upstream_sha256, &upstream_bytes) {
        (Some(expected), Some(bytes)) => Some(&crate::patch::bytes_sha256(bytes) == expected),
        _ => None,
    };
    let loaded_upstream = match upstream_refused {
        // Refused at the read: never opened again to produce a second,
        // weaker diagnosis.
        Some(e) => Some(Err(e)),
        None => upstream_bytes
            .is_some()
            .then(|| load_document(dir, &upstream)),
    };
    let upstream_error = match &loaded_upstream {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    };
    let (patches_ok, patches_error) = match (loaded_upstream, &working) {
        (Some(Ok(up)), Some(w)) => match crate::patch::apply(&up, &entry.patches) {
            Ok(patched) => (
                Some(&patched == w),
                if &patched == w {
                    None
                } else {
                    Some(
                        "applying the recorded patches to the upstream does not give the working copy"
                            .to_string(),
                    )
                },
            ),
            Err(e) => (Some(false), Some(e)),
        },
        // An unreadable upstream is its own report; the patches were not evaluated.
        (Some(Err(_)), _) => (None, None),
        _ => (None, None),
    };
    SourceStatus {
        path: entry.path.clone(),
        declared_kind: entry.kind.clone(),
        detected_kind: detected_kind.to_string(),
        version: entry
            .version
            .clone()
            .unwrap_or_else(|| detected_version.clone()),
        detected_version,
        upstream,
        upstream_present,
        upstream_ok,
        outside_workspace,
        upstream_misplaced,
        misplaced_reason,
        fault: fault_kind,
        upstream_error,
        upstream_recorded: entry.upstream_sha256.is_some(),
        working_present,
        working_error,
        content_sha256,
        locked_sha256,
        hand_edit,
        patches: entry.patches.len(),
        patches_ok,
        patches_error,
    }
}

/// Working copies the applied lock acknowledges that no entry pins any
/// more: an entry removed from sources.lock.yaml would otherwise unpin a
/// document silently, its hand edits unwatched.
pub fn unpinned(dir: &Path, applied_lock: Option<&Value>) -> Vec<String> {
    let acknowledged: Vec<String> = applied_lock
        .and_then(|l| get(l, "sources"))
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    if acknowledged.is_empty() {
        return vec![];
    }
    let pinned: Vec<String> = match read_sources_lock(dir) {
        Ok(Some(l)) => document_entries(&l).into_iter().map(|e| e.path).collect(),
        // No lock at all: every acknowledged copy really is unpinned.
        Ok(None) => vec![],
        // An unreadable or refused lock is not "nothing is pinned" — that
        // would report every acknowledged document as silently unpinned.
        // `statuses` runs first in every caller and reports the read error.
        Err(_) => return vec![],
    };
    acknowledged
        .into_iter()
        .filter(|a| {
            let a = a.trim_start_matches("./");
            !pinned.iter().any(|p| p == a)
        })
        .collect()
}

/// Status of every pinned document in the workspace; empty when there is no
/// sources lock or it pins none.
pub fn statuses(dir: &Path, applied_lock: Option<&Value>) -> Result<Vec<SourceStatus>, String> {
    let lock = match read_sources_lock(dir)? {
        Some(l) => l,
        None => return Ok(vec![]),
    };
    let entries = document_entries(&lock);
    Ok(entries
        .iter()
        .map(|e| status(dir, e, &entries, applied_lock))
        .collect())
}

/// `sources: { <path>: <sha256> }` for the applied lock, from the current
/// working copies.
pub fn applied_hashes(dir: &Path, entries: &[SourceEntry]) -> Value {
    let mut m = obj();
    for e in entries {
        // A faulted entry is never acted on, not even to acknowledge it.
        if not_followed(e, entries).is_some() {
            continue;
        }
        if let Ok(doc) = load_document(dir, &e.path) {
            m.insert(
                e.path.clone(),
                Value::from(crate::patch::canonical_sha256(&doc)),
            );
        }
    }
    Value::Object(m)
}

// ── Editing the lock text in place ──────────────────────────────────────────

struct Block {
    start: usize, // the `- ` line
    end: usize,   // exclusive
    indent: usize,
}

/// The top-level `sources:` key, with or without a trailing comment.
fn is_sources_key(line: &str) -> bool {
    let t = line.trim_end();
    t == "sources:" || t.starts_with("sources: #") || t.starts_with("sources:  #") || {
        let before = t.split(" #").next().unwrap_or(t).trim_end();
        before == "sources:"
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The `- ` item blocks of the top-level `sources:` list.
fn blocks(lines: &[&str]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let sources_at = match lines.iter().position(|l| is_sources_key(l)) {
        Some(i) => i,
        None => return out,
    };
    let mut item_indent: Option<usize> = None;
    let mut i = sources_at + 1;
    while i < lines.len() {
        let l = lines[i];
        let t = l.trim_end();
        let is_item = t.trim_start().starts_with("- ") || t.trim() == "-";
        if !t.trim().is_empty() && indent_of(l) == 0 && !is_item {
            break; // next top-level key (a flush-left `- ` is still an item of `sources:`)
        }
        if is_item {
            let ind = indent_of(l);
            match item_indent {
                None => item_indent = Some(ind),
                Some(x) if x != ind => {
                    i += 1;
                    continue;
                }
                _ => {}
            }
            if let Some(last) = out.last_mut() {
                last.end = i;
            }
            out.push(Block {
                start: i,
                end: lines.len(),
                indent: ind,
            });
        }
        i += 1;
    }
    if let Some(last) = out.last_mut() {
        last.end = i;
        // Trailing blank lines belong to what follows.
        while last.end > last.start + 1 && lines[last.end - 1].trim().is_empty() {
            last.end -= 1;
        }
    }
    out
}

fn unquote(v: &str) -> String {
    let t = v.trim();
    if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')) {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// The value of `key:` inside a block (first line's `- key:` counts).
fn block_key_value(lines: &[&str], b: &Block, key: &str) -> Option<String> {
    let key_indent = b.indent + 2;
    for (n, l) in lines[b.start..b.end].iter().enumerate() {
        let body = if n == 0 {
            l.trim_start().strip_prefix("- ").unwrap_or("").to_string()
        } else if indent_of(l) == key_indent {
            l.trim_start().to_string()
        } else {
            continue;
        };
        if let Some(rest) = body.strip_prefix(&format!("{}:", key)) {
            let rest = rest.trim();
            if rest.is_empty() || rest.starts_with('>') || rest.starts_with('|') {
                return Some(String::new());
            }
            return Some(unquote(rest.split(" #").next().unwrap_or(rest)));
        }
    }
    None
}

fn find_block(lines: &[&str], path: &str) -> Option<Block> {
    let want = path.trim_start_matches("./");
    blocks(lines).into_iter().find(|b| {
        block_key_value(lines, b, "path")
            .as_deref()
            .map(|p| p.trim_start_matches("./"))
            == Some(want)
    })
}

/// Where `key:` sits in a block: its line, and the exclusive end of its
/// continuation (folded scalars, nested lists).
fn key_span(lines: &[&str], b: &Block, key: &str) -> Option<(usize, usize)> {
    let key_indent = b.indent + 2;
    let mut at: Option<usize> = None;
    for i in b.start..b.end {
        let l = lines[i];
        let body = if i == b.start {
            l.trim_start().strip_prefix("- ").unwrap_or("")
        } else if indent_of(l) == key_indent {
            l.trim_start()
        } else {
            continue;
        };
        if body.starts_with(&format!("{}:", key)) {
            at = Some(i);
            break;
        }
    }
    let at = at?;
    let mut end = at + 1;
    while end < b.end {
        let l = lines[end];
        if l.trim().is_empty() || indent_of(l) > key_indent {
            end += 1;
        } else {
            break;
        }
    }
    // Blank lines at the end of the span stay with what follows.
    while end > at + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    Some((at, end))
}

fn scalar(v: &str) -> String {
    let plain_ok = !v.is_empty()
        && !v.chars().all(|c| c.is_ascii_digit() || c == '.')
        && !v.contains(": ")
        && !v.contains(" #")
        && !v.starts_with([
            '"', '\'', '&', '*', '!', '|', '>', '%', '@', '`', '[', ']', '{', '}', '#', '-', '?',
            ':', ',',
        ])
        && !matches!(v, "true" | "false" | "null" | "yes" | "no" | "~");
    if plain_ok {
        v.to_string()
    } else {
        serde_json::to_string(v).unwrap_or_else(|_| format!("\"{}\"", v))
    }
}

/// `contract_version: 1` at the top of a lock that lacks it (an agent's
/// hand-written `init` lock, or one from before the key was required).
pub fn ensure_contract_version(text: &str) -> String {
    if text.lines().any(|l| l.starts_with("contract_version:")) {
        return text.to_string();
    }
    // After a leading document marker and leading comments, so the file
    // stays one YAML document with its header intact.
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut at = 0;
    while at < lines.len() {
        let t = lines[at].trim();
        if t == "---" || t.starts_with('#') || t.is_empty() {
            at += 1;
        } else {
            break;
        }
    }
    let mut out = String::new();
    out.push_str(&lines[..at].concat());
    out.push_str("contract_version: 1\n");
    out.push_str(&lines[at..].concat());
    out
}

/// A flow-style `sources: [...]` line cannot be edited in place. An empty
/// one is rewritten to the block form; a non-empty one is an error naming
/// the fix.
fn block_form(text: &str) -> Result<String, String> {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let t = line.trim_end();
        if let Some(rest) = t.strip_prefix("sources:") {
            let rest = rest.split(" #").next().unwrap_or(rest).trim();
            if rest.starts_with('[') {
                if rest.trim_end_matches(',') == "[]" {
                    out.push_str("sources:\n");
                    continue;
                }
                return Err(format!(
                    "{} keeps its `sources` list in flow style (`sources: [...]`); rewrite it as a block list (one `- kind: …` item per line) so entries can be edited in place",
                    SOURCES_LOCK
                ));
            }
        }
        out.push_str(line);
    }
    Ok(out)
}

/// Set scalar keys on the entry whose `path` matches, replacing existing
/// lines (and their continuations) and adding missing ones after the `- `
/// line. When no entry matches, a new one is appended to the list, `kind`
/// first. Every other line of the file is left as it was, except that a
/// missing `contract_version: 1` is added and an empty flow-style list is
/// made a block list.
pub fn upsert_entry(text: &str, path: &str, pairs: &[(&str, &str)]) -> Result<String, String> {
    let text = ensure_contract_version(&block_form(text)?);
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    if !lines.is_empty() && !lines.last().unwrap().ends_with('\n') {
        lines.last_mut().unwrap().push('\n');
    }
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    match find_block(&refs, path) {
        Some(b) => {
            let key_indent = " ".repeat(b.indent + 2);
            // Apply from the bottom so earlier indices stay valid.
            let mut ops: Vec<(usize, usize, String)> = Vec::new();
            // New keys go after `path:` (every matched entry has one), so the
            // record reads kind, url, …, path, upstream, upstream_sha256.
            let insert_at = key_span(&refs, &b, "path")
                .map(|(_, end)| end)
                .unwrap_or(b.start + 1);
            for (k, v) in pairs {
                match key_span(&refs, &b, k) {
                    Some((at, end)) => {
                        let line = if at == b.start {
                            format!("{}- {}: {}\n", " ".repeat(b.indent), k, scalar(v))
                        } else {
                            format!("{}{}: {}\n", key_indent, k, scalar(v))
                        };
                        ops.push((at, end, line));
                    }
                    None => {
                        ops.push((
                            insert_at,
                            insert_at,
                            format!("{}{}: {}\n", key_indent, k, scalar(v)),
                        ));
                    }
                }
            }
            // Inserts at the same index must keep pair order: stable sort by (start, insertion order).
            // Replacements before inserts at the same index, so an insert at the
            // end of `path:`'s span is not spliced over by a rewrite of the key
            // that sits there.
            ops.sort_by_key(|(s, e, _)| (*s, *e > *s));
            for (start, end, line) in ops.into_iter().rev() {
                lines.splice(start..end, std::iter::once(line));
            }
            Ok(lines.concat())
        }
        None => {
            let mut out = lines.concat();
            let has_sources = refs.iter().any(|l| is_sources_key(l));
            let indent = blocks(&refs).first().map(|b| b.indent).unwrap_or(2);
            if !has_sources {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("sources:\n");
            }
            let pad = " ".repeat(indent);
            let mut entry = String::new();
            let mut ordered: Vec<(&str, &str)> = pairs.to_vec();
            if let Some(pos) = ordered.iter().position(|(k, _)| *k == "kind") {
                let kind = ordered.remove(pos);
                ordered.insert(0, kind);
            }
            if !ordered.iter().any(|(k, _)| *k == "path") {
                ordered.push(("path", path));
            }
            for (n, (k, v)) in ordered.iter().enumerate() {
                if n == 0 {
                    entry.push_str(&format!("{}- {}: {}\n", pad, k, scalar(v)));
                } else {
                    entry.push_str(&format!("{}  {}: {}\n", pad, k, scalar(v)));
                }
            }
            // Append after the last block of the list (or right after `sources:`).
            let refs2: Vec<&str> = out.split_inclusive('\n').collect();
            let at = blocks(&refs2).last().map(|b| b.end).unwrap_or_else(|| {
                refs2
                    .iter()
                    .position(|l| is_sources_key(l))
                    .map(|i| i + 1)
                    .unwrap_or(refs2.len())
            });
            let mut parts: Vec<String> = refs2.iter().map(|s| s.to_string()).collect();
            parts.insert(at, entry);
            Ok(parts.concat())
        }
    }
}

/// Replace (or add) the `patches:` block of the entry whose `path` matches.
pub fn splice_patches(text: &str, path: &str, patches: &[Value]) -> Result<String, String> {
    let text = ensure_contract_version(&block_form(text)?);
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    if !lines.is_empty() && !lines.last().unwrap().ends_with('\n') {
        lines.last_mut().unwrap().push('\n');
    }
    let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let b = find_block(&refs, path)
        .ok_or_else(|| format!("{} has no entry with path: {}", SOURCES_LOCK, path))?;
    let key_indent = b.indent + 2;
    let (start, end) = key_span(&refs, &b, "patches").unwrap_or((b.end, b.end));
    // On the `- ` line the key keeps its item marker.
    let lead = if start == b.start {
        format!("{}- ", " ".repeat(b.indent))
    } else {
        " ".repeat(key_indent)
    };
    let block = if patches.is_empty() {
        format!("{}patches: []\n", lead)
    } else {
        format!(
            "{}patches:\n{}",
            lead,
            crate::yaml::stringify(&Value::Array(patches.to_vec()), key_indent + 2)
        )
    };
    lines.splice(start..end, std::iter::once(block));
    Ok(lines.concat())
}

//! Where the decision log and the findings live on disk (ADR 0118), shared
//! by both.
//!
//! - **The single file**, `.factory/decisions.json` (`findings.json`),
//!   `contract_version` 1, holds the records written before ADR 0118, with
//!   their numbered ids. It is never migrated and never gains a record or a
//!   field: a verb that changes one of its records (resolve, reopen,
//!   supersede) writes it back in place, and every other byte stays as it
//!   was, so every older binary still reads it.
//! - **The directory**, `.factory/decisions/` (`findings/`), holds every
//!   record added since: one file per record, `{"contract_version": 2,
//!   "decision": {…}}`, named `<id>-<slug>.json`, with a random id. Two
//!   branches that each add a record add two files and merge without
//!   conflict.
//!
//! A workspace may hold either or both. In memory they are one document,
//! `{contract_version, <array>: […]}`: the file's records in file order, then
//! the directory's. [`write`] puts each record back where it lives, deciding
//! by which ids the file on disk holds, so no caller tracks where a record
//! came from. Every read and write goes through custody (`crate::factory_io`,
//! ADR 0025): the file and a changed record file are truncated in place, a
//! new record file is created with `O_EXCL`, and nothing is ever renamed or
//! removed.

use crate::json;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Where one log lives and what its records are called.
pub struct Log {
    /// The single file of records written before ADR 0118.
    pub legacy: &'static str,
    /// The directory of record files.
    pub dir: &'static str,
    /// The array key of the file and of the in-memory document.
    pub array: &'static str,
    /// The key that holds the record inside a record file.
    pub key: &'static str,
    /// The id prefix, `D-` or `F-`.
    pub prefix: &'static str,
    /// The schema the file and the record files validate against.
    pub schema: &'static str,
}

pub const DECISIONS: Log = Log {
    legacy: ".factory/decisions.json",
    dir: ".factory/decisions",
    array: "decisions",
    key: "decision",
    prefix: "D-",
    schema: "decisions.schema.json",
};

pub const FINDINGS: Log = Log {
    legacy: ".factory/findings.json",
    dir: ".factory/findings",
    array: "findings",
    key: "finding",
    prefix: "F-",
    schema: "findings.schema.json",
};

/// The contract version of every record file.
pub const RECORD_VERSION: u64 = 2;

/// Whether either log has a record directory.
pub fn has_directory(root: &Path) -> bool {
    crate::factory_io::is_dir(root, DECISIONS.dir) || crate::factory_io::is_dir(root, FINDINGS.dir)
}

fn schema(log: &Log, schemas_dir: Option<&Path>) -> Result<Value, String> {
    crate::schemas::load(log.schema, schemas_dir)
        .ok_or_else(|| format!("cannot load {}", log.schema))
}

/// The single file, parsed and validated, or `None` when it is absent.
fn read_legacy(root: &Path, log: &Log, schema: &Value) -> Result<Option<Value>, String> {
    let Some(text) = crate::factory_io::read_to_string_optional(root, log.legacy)? else {
        return Ok(None);
    };
    let value = json::parse(&text).map_err(|e| format!("{}: {}", log.legacy, e))?;
    let errors = crate::jsonschema::validate(&value, schema);
    if !errors.is_empty() {
        return Err(format!("{}: {}", log.legacy, errors.join("; ")));
    }
    if json::get(&value, "contract_version").and_then(Value::as_u64) != Some(1) {
        return Err(format!(
            "{}: the single file is contract_version 1; records written since ADR 0118 live in {}/, one file each",
            log.legacy, log.dir
        ));
    }
    Ok(Some(value))
}

/// Read the log, validated: the single file's records, then the directory's.
/// `None` when neither exists.
pub fn read(root: &Path, log: &Log, schemas_dir: Option<&Path>) -> Result<Option<Value>, String> {
    let schema = schema(log, schemas_dir)?;
    let legacy = read_legacy(root, log, &schema)?;
    let dir_present = crate::factory_io::symlink_metadata(root, log.dir)?.is_some();
    if legacy.is_none() && !dir_present {
        return Ok(None);
    }
    let mut records: Vec<Value> = legacy
        .as_ref()
        .and_then(|l| json::get_arr(l, log.array))
        .cloned()
        .unwrap_or_default();
    let added = read_directory(root, log, &schema)?;
    let version = if added.is_empty() { 1 } else { RECORD_VERSION };
    records.extend(added);
    Ok(Some(json::object(vec![
        ("contract_version", Value::from(version)),
        (log.array, Value::Array(records)),
    ])))
}

/// The record files of a directory, by file name. An absent directory is
/// empty. A dot-file is ignored; anything else that is not a `.json` file
/// is refused, so a stray file is seen rather than skipped.
fn entries(root: &Path, log: &Log) -> Result<Vec<String>, String> {
    if !crate::factory_io::is_dir(root, log.dir) {
        if crate::factory_io::symlink_metadata(root, log.dir)?.is_some() {
            return Err(format!("{} is not a directory", log.dir));
        }
        return Ok(vec![]);
    }
    let mut names = Vec::new();
    for entry in crate::factory_io::read_dir(root, log.dir)? {
        let entry = entry.map_err(|e| format!("{}: {}", log.dir, e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if !name.ends_with(".json") {
            return Err(format!(
                "{}/{}: not a record file (every entry is <id>-<slug>.json), so the whole log is unread until it goes: a merge tool's leftover (.orig, .rej) or an editor's backup? Settle the merge in the .json file it came from, then remove this one",
                log.dir, name
            ));
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// The file a [`read`] error names, when it names one of this log's (such
/// a message begins `<path>: `), else the single file: where lint pins the
/// finding that reports the log unreadable.
pub fn error_path<'a>(log: &Log, error: &'a str) -> &'a str {
    match error.split_once(": ") {
        Some((path, _)) if path == log.legacy || path.starts_with(&format!("{}/", log.dir)) => path,
        _ => log.legacy,
    }
}

/// Every record file of both logs, as `.factory/…` paths, sorted: inputs of
/// the lock's provenance and of `selection review`, beside the single files.
pub fn record_paths(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for log in [&DECISIONS, &FINDINGS] {
        for name in entries(root, log)? {
            out.push(format!("{}/{}", log.dir, name));
        }
    }
    out.sort();
    Ok(out)
}

/// The workspace-relative path of the record file holding `id`, or `None`
/// when no record file does (a numbered record lives in the single file).
/// `add --json` prints it, so a caller stages the file without globbing.
pub fn path_of(root: &Path, log: &Log, id: &str) -> Result<Option<String>, String> {
    Ok(entries(root, log)?
        .into_iter()
        .find(|name| names_record(name, id))
        .map(|name| format!("{}/{}", log.dir, name)))
}

/// Whether `name` is the file of the record with this id: `<id>.json` or
/// `<id>-<anything>.json`.
fn names_record(name: &str, id: &str) -> bool {
    name.strip_prefix(id)
        .is_some_and(|rest| rest == ".json" || rest.starts_with('-'))
}

/// The directory's records, each validated as a record file and checked
/// against its file name, in [`order`].
fn read_directory(root: &Path, log: &Log, schema: &Value) -> Result<Vec<Value>, String> {
    let mut records = Vec::new();
    for name in entries(root, log)? {
        let rel = format!("{}/{}", log.dir, name);
        let text = crate::factory_io::read_to_string(root, &rel)?;
        let file = json::parse(&text).map_err(|e| format!("{}: {}", rel, e))?;
        let errors = crate::jsonschema::validate_ref(&file, schema, "#/$defs/file");
        if !errors.is_empty() {
            return Err(format!("{}: {}", rel, errors.join("; ")));
        }
        let Some(rec) = json::get(&file, log.key).cloned() else {
            return Err(format!("{}: no {} in the file", rel, log.key));
        };
        let id = json::get_str(&rec, "id").unwrap_or("");
        if !names_record(&name, id) {
            return Err(format!(
                "{}: holds {}, but its name does not begin with that id; a record file is named <id>-<slug>.json",
                rel, id
            ));
        }
        records.push(rec);
    }
    order(&mut records);
    Ok(records)
}

/// A numbered id's number (`D-0012` → 12), or `None` for a random one. An
/// id of digits only is numbered: [`new_id`] never draws one, so a random
/// id always holds a letter.
pub fn number(id: &str) -> Option<u64> {
    let digits = id.get(2..)?;
    if digits.len() >= 4 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

/// Whether an id is a random one (ADR 0118) rather than a number.
pub fn is_random(id: &str) -> bool {
    number(id).is_none()
}

/// The grammar of an id after its prefix: a number of four or more digits
/// (`0019`) or six lower-case base36 characters (`k7m2qx`). Every schema
/// field that holds or cites a decision or finding id carries exactly this
/// alternation after its `D-`/`F-`/`[DF]-` (a test holds them to it), and
/// every verb flag that cites one checks it through [`is_id`]. It is the
/// schema-level superset of what [`new_id`] draws: six digits match both
/// branches and read as a number, so "at least one letter" is [`number`]'s
/// call, not the pattern's.
pub const ID_TAIL: &str = "([0-9]{4,}|[0-9a-z]{6})";

/// Whether `s` is an id with one of `prefixes` (`"D-"`, `"F-"`), numbered or
/// random, by [`ID_TAIL`].
pub fn is_id(s: &str, prefixes: &[&str]) -> bool {
    let re = regex::Regex::new(&format!("^{}$", ID_TAIL)).expect("static regex");
    prefixes
        .iter()
        .any(|p| s.strip_prefix(p).is_some_and(|tail| re.is_match(tail)))
}

/// Whether `s` is a decision id: `D-0019` or `D-k7m2qx`.
pub fn is_decision_id(s: &str) -> bool {
    is_id(s, &[DECISIONS.prefix])
}

/// The order of the directory's records: by `date`, then id. A numbered id
/// in the directory (hand-copied there) sorts first, by number.
pub fn order(records: &mut [Value]) {
    records.sort_by(|a, b| {
        let ia = json::get_str(a, "id").unwrap_or("");
        let ib = json::get_str(b, "id").unwrap_or("");
        match (number(ia), number(ib)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                let da = json::get_str(a, "date").unwrap_or("");
                let db = json::get_str(b, "date").unwrap_or("");
                da.cmp(db).then(ia.cmp(ib))
            }
        }
    });
}

/// Ids that appear on more than one record, each once, in order of first
/// appearance.
pub fn duplicate_ids(doc: &Value, array: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut dups = Vec::new();
    for rec in json::get_arr(doc, array).into_iter().flatten() {
        if let Some(id) = json::get_str(rec, "id") {
            if !seen.insert(id) && !dups.iter().any(|d: &String| d == id) {
                dups.push(id.to_string());
            }
        }
    }
    dups
}

/// Write the document back: each record the single file on disk holds goes
/// back into it (the file is rewritten only when one of them changed, and
/// stays contract_version 1), and every other record to its own file under
/// the directory. Refuses a log that holds an id twice, an old record that
/// gained a field only a record file may carry, and anything the schema
/// rejects; nothing is written unless everything validates.
pub fn write(
    root: &Path,
    log: &Log,
    doc: &Value,
    schemas_dir: Option<&Path>,
) -> Result<(), String> {
    let schema = schema(log, schemas_dir)?;
    let dups = duplicate_ids(doc, log.array);
    if !dups.is_empty() {
        return Err(format!(
            "refusing to write a log that holds {} twice: a merge kept both records; settle it as the merge conflict it is: keep one of the two, or, if both are real, remove the later one and `decisions add` it again so it gets a fresh id, then update what cites it",
            dups.join(", ")
        ));
    }
    let on_disk = crate::factory_io::read_to_string_optional(root, log.legacy)?;
    let on_disk_value = match &on_disk {
        Some(text) => Some(json::parse(text).map_err(|e| format!("{}: {}", log.legacy, e))?),
        None => None,
    };
    let legacy_ids: HashSet<String> = on_disk_value
        .as_ref()
        .map(|v| ids(v, log.array))
        .unwrap_or_default();
    let records = json::get_arr(doc, log.array).cloned().unwrap_or_default();
    let (old, new): (Vec<Value>, Vec<Value>) = records
        .into_iter()
        .partition(|r| json::get_str(r, "id").is_some_and(|id| legacy_ids.contains(id)));
    // The single file's records in its own order, each as the caller holds
    // it now: the file never gains or loses a record (ADR 0118).
    let by_id: HashMap<&str, &Value> = old
        .iter()
        .filter_map(|r| json::get_str(r, "id").map(|id| (id, r)))
        .collect();
    let ordered: Vec<Value> = on_disk_value
        .as_ref()
        .and_then(|v| json::get_arr(v, log.array))
        .into_iter()
        .flatten()
        .map(|r| {
            json::get_str(r, "id")
                .and_then(|id| by_id.get(id))
                .map(|now| (*now).clone())
                .unwrap_or_else(|| r.clone())
        })
        .collect();

    // Validate everything before the first byte is written.
    let legacy_doc = json::object(vec![
        ("contract_version", Value::from(1)),
        (log.array, Value::Array(ordered)),
    ]);
    let legacy_bytes = json::pretty(&legacy_doc);
    if on_disk.is_some() {
        let errors = crate::jsonschema::validate(&legacy_doc, &schema);
        if !errors.is_empty() {
            return Err(format!(
                "{}: refusing to write an invalid document: {}. A record written before ADR 0118 keeps the fields it had; put a new edge or slug on a new record",
                log.legacy,
                errors.join("; ")
            ));
        }
    }
    let mut files = Vec::new();
    for rec in &new {
        let bytes = record_bytes(log, rec);
        let file = json::parse(&bytes).unwrap_or(Value::Null);
        let errors = crate::jsonschema::validate_ref(&file, &schema, "#/$defs/file");
        if !errors.is_empty() {
            let id = json::get_str(rec, "id").unwrap_or("?");
            return Err(format!(
                "{}/: refusing to write an invalid record {}: {}",
                log.dir,
                id,
                errors.join("; ")
            ));
        }
        files.push((
            json::get_str(rec, "id").unwrap_or("").to_string(),
            rec,
            bytes,
        ));
    }

    // A file is rewritten only when one of its records changed (ADR 0118
    // §4), compared as parsed JSON: a file that is merely not in the
    // binary's own layout (re-indented, CRLF, reformatted on save) is left
    // as it is, so a verb on one record changes exactly one file.
    if let Some(value) = &on_disk_value {
        if value != &legacy_doc {
            // In place through custody, never renamed over (ADR 0025).
            crate::factory_io::write_in_place(root, log.legacy, legacy_bytes.as_bytes())?;
        }
    }
    if files.is_empty() {
        return Ok(());
    }
    crate::factory_io::create_dir_all(root, log.dir)?;
    let mut by_id: HashMap<String, String> = HashMap::new();
    for name in entries(root, log)? {
        let stem = name.trim_end_matches(".json");
        let id = match stem.get(2..).and_then(|rest| rest.find('-')) {
            Some(i) => &stem[..i + 2],
            None => stem,
        };
        by_id.entry(id.to_string()).or_insert(name);
    }
    for (id, rec, bytes) in files {
        match by_id.get(&id) {
            Some(name) => {
                let rel = format!("{}/{}", log.dir, name);
                let before = crate::factory_io::read(root, &rel)?;
                let unchanged = std::str::from_utf8(&before)
                    .ok()
                    .and_then(|t| json::parse(t).ok())
                    .is_some_and(|v| Some(v) == json::parse(&bytes).ok());
                if !unchanged {
                    crate::factory_io::write_in_place(root, &rel, bytes.as_bytes())?;
                }
            }
            None => {
                let rel = format!("{}/{}", log.dir, file_name(rec));
                crate::factory_io::create_new(root, &rel, bytes.as_bytes())?;
            }
        }
    }
    Ok(())
}

/// Write the whole document as the single file, exactly as before ADR 0118:
/// for the two migrations that produce one (`decisions migrate` from
/// `decisions.md`, and ADR 0113's `--split`), which read and write only the
/// single-file shape. Refuses when the log has a record directory, since
/// those records would be folded into the file.
pub fn write_single(
    root: &Path,
    log: &Log,
    doc: &Value,
    schemas_dir: Option<&Path>,
) -> Result<(), String> {
    if crate::factory_io::symlink_metadata(root, log.dir)?.is_some() {
        return Err(format!(
            "{}/ exists: this migration writes the single file only, and would fold the records added since ADR 0118 into it",
            log.dir
        ));
    }
    let schema = schema(log, schemas_dir)?;
    let errors = crate::jsonschema::validate(doc, &schema);
    if !errors.is_empty() {
        return Err(format!(
            "{}: refusing to write an invalid document: {}",
            log.legacy,
            errors.join("; ")
        ));
    }
    let dups = duplicate_ids(doc, log.array);
    if !dups.is_empty() {
        return Err(format!(
            "{}: refusing to write a log that holds {} twice",
            log.legacy,
            dups.join(", ")
        ));
    }
    crate::factory_io::create_parent_dir(root, log.legacy)?;
    crate::factory_io::write_in_place(root, log.legacy, json::pretty(doc).as_bytes())
        .map_err(String::from)
}

/// The bytes of one record's file.
pub fn record_bytes(log: &Log, rec: &Value) -> String {
    json::pretty(&json::object(vec![
        ("contract_version", Value::from(RECORD_VERSION)),
        (log.key, rec.clone()),
    ]))
}

/// A record's file name: `<id>-<slug>.json`, or `<id>.json` with no slug.
pub fn file_name(rec: &Value) -> String {
    let id = json::get_str(rec, "id").unwrap_or("");
    match json::get_str(rec, "slug") {
        Some(slug) if !slug.is_empty() => format!("{}-{}.json", id, slug),
        _ => format!("{}.json", id),
    }
}

/// The most characters a slug carries.
pub const SLUG_MAX: usize = 40;

/// A file-name slug from a title: lower-case ASCII letters and digits, runs
/// of anything else as one hyphen, cut at a hyphen to at most [`SLUG_MAX`]
/// characters. Empty when the title has no letter or digit.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        // An apostrophe joins its word rather than splitting it:
        // `PagerDuty's` is `pagerdutys`, not `pagerduty-s`.
        if c == '\'' || c == '\u{2019}' {
            continue;
        }
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut slug = out.trim_end_matches('-').to_string();
    if slug.len() > SLUG_MAX {
        let cut = slug[..=SLUG_MAX].rfind('-').unwrap_or(SLUG_MAX);
        slug.truncate(cut);
        slug = slug.trim_end_matches('-').to_string();
    }
    slug
}

/// Whether a slug is one [`slugify`] could have produced: lower-case ASCII
/// letters and digits in hyphen-separated runs, at most [`SLUG_MAX`] long.
pub fn is_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= SLUG_MAX
        && s.split('-').all(|run| {
            !run.is_empty()
                && run
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

const BASE36: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Sixty-four bits from the operating system's random source, through the
/// standard library's randomly keyed hasher (no extra crate). Not for
/// secrets; an id only has to be unlikely to repeat, and the writer refuses
/// a repeat anyway.
fn random_u64(salt: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(salt);
    if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(t.as_nanos());
    }
    h.write_u32(std::process::id());
    h.finish()
}

/// A new random id with `prefix`, six base36 characters with at least one
/// letter (so it never reads as a number), that `taken` does not already
/// hold. Draws again on a collision.
pub fn new_id(prefix: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut salt = 0u64;
    loop {
        let mut n = random_u64(salt);
        let mut id = String::from(prefix);
        for _ in 0..6 {
            id.push(BASE36[(n % 36) as usize] as char);
            n /= 36;
        }
        if number(&id).is_none() && !taken(&id) {
            return id;
        }
        salt += 1;
    }
}

/// The ids every record in `doc` carries, for [`new_id`]'s `taken`.
pub fn ids(doc: &Value, array: &str) -> HashSet<String> {
    json::get_arr(doc, array)
        .into_iter()
        .flatten()
        .filter_map(|r| json::get_str(r, "id").map(str::to_string))
        .collect()
}

/// The record with `slug` placed right after its `id`, where `add` puts it.
pub fn with_slug(rec: &Value, slug: &str) -> Value {
    let Value::Object(m) = rec else {
        return rec.clone();
    };
    let mut out = json::obj();
    for (k, v) in m {
        if k == "slug" {
            continue;
        }
        out.insert(k.clone(), v.clone());
        if k == "id" {
            out.insert("slug".into(), Value::from(slug));
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lower_case_hyphenated_and_cut_at_a_hyphen() {
        assert_eq!(
            slugify("Paginate by cursor, not page"),
            "paginate-by-cursor-not-page"
        );
        assert_eq!(slugify("  --Ünïcode & co. "), "n-code-co");
        assert_eq!(slugify("..."), "");
        assert_eq!(
            slugify("Enums keep PagerDuty's wire casing"),
            "enums-keep-pagerdutys-wire-casing"
        );
        let long = slugify("one two three four five six seven eight nine ten eleven");
        assert!(long.len() <= SLUG_MAX, "{}", long);
        assert!(!long.ends_with('-'), "{}", long);
        assert_eq!(long, "one-two-three-four-five-six-seven-eight");
        assert!(is_slug(&long));
        assert!(!is_slug("Upper"));
        assert!(!is_slug("a--b"));
        assert!(!is_slug(""));
    }

    #[test]
    fn numbered_and_random_ids_are_told_apart() {
        assert_eq!(number("D-0012"), Some(12));
        assert_eq!(number("D-10000"), Some(10000));
        assert_eq!(number("D-k7m2qx"), None);
        assert_eq!(number("D-123456"), Some(123456));
        assert_eq!(number("D-12"), None);
        assert!(is_random("F-9k2wde"));
        assert!(!is_random("F-0001"));
    }

    #[test]
    fn both_id_shapes_are_ids_and_nothing_else_is() {
        for ok in ["D-0019", "D-12345", "D-k7m2qx", "D-123456"] {
            assert!(is_decision_id(ok), "{}", ok);
        }
        for bad in [
            "D-019",
            "D-K7M2QX",
            "D-k7m2q",
            "D-k7m2qxy",
            "F-k7m2qx",
            "D-k7m2-x",
        ] {
            assert!(!is_decision_id(bad), "{}", bad);
        }
        assert!(is_id("F-9k2wde", &["D-", "F-"]));
        assert!(!is_id("X-9k2wde", &["D-", "F-"]));
    }

    /// Every schema pattern that holds or cites a decision or finding id is
    /// [`ID_TAIL`] after its prefix, so a verb that checks a flag with
    /// [`is_id`] and the schema that validates the file it writes agree.
    #[test]
    fn every_schema_id_pattern_is_the_shared_grammar() {
        fn patterns(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::Object(m) => {
                    if let Some(Value::String(p)) = m.get("pattern") {
                        out.push(p.clone());
                    }
                    m.values().for_each(|c| patterns(c, out));
                }
                Value::Array(a) => a.iter().for_each(|c| patterns(c, out)),
                _ => {}
            }
        }
        let mut cited = 0;
        for name in [
            "decisions.schema.json",
            "findings.schema.json",
            "selection.schema.json",
            "sources-lock.schema.json",
        ] {
            let schema = crate::schemas::load(name, None).expect(name);
            let mut all = Vec::new();
            patterns(&schema, &mut all);
            for p in all
                .iter()
                .filter(|p| p.starts_with("^D-") || p.starts_with("^F-") || p.starts_with("^[DF]-"))
            {
                let prefix = &p[1..p.find('-').unwrap() + 1];
                assert_eq!(
                    *p,
                    format!("^{}{}$", prefix, ID_TAIL),
                    "{}: an id pattern that is not the shared grammar",
                    name
                );
                cited += 1;
            }
        }
        // decisions: id, after, amends; findings: id, related; selection:
        // waivers, overrides, links; sources-lock: patches.
        assert_eq!(cited, 9);
    }

    #[test]
    fn new_ids_are_six_base36_characters_and_avoid_taken_ones() {
        let first = new_id("D-", |_| false);
        assert_eq!(first.len(), 8);
        assert!(first[2..].bytes().all(|b| BASE36.contains(&b)), "{}", first);
        let again = new_id("D-", |id| id == first);
        assert_ne!(again, first);
        let mut seen = HashSet::new();
        for _ in 0..200 {
            let id = new_id("D-", |_| false);
            assert!(is_random(&id), "{}", id);
            assert!(seen.insert(id));
        }
    }

    #[test]
    fn order_puts_numbers_first_then_dates() {
        let mut recs: Vec<Value> = vec![
            serde_json::json!({"id": "D-zzzzz1", "date": "2026-10-01"}),
            serde_json::json!({"id": "D-0002", "date": "2026-11-01"}),
            serde_json::json!({"id": "D-aaaaa1", "date": "2026-10-02"}),
            serde_json::json!({"id": "D-0001", "date": "2026-12-01"}),
        ];
        order(&mut recs);
        let ids: Vec<&str> = recs
            .iter()
            .map(|r| json::get_str(r, "id").unwrap())
            .collect();
        assert_eq!(ids, ["D-0001", "D-0002", "D-zzzzz1", "D-aaaaa1"]);
    }

    #[test]
    fn a_record_file_is_named_by_its_id() {
        assert!(names_record("D-0001-unchecked-errors.json", "D-0001"));
        assert!(names_record("D-0001.json", "D-0001"));
        assert!(!names_record("D-00011-x.json", "D-0001"));
        assert!(!names_record("D-0002-x.json", "D-0001"));
    }
}

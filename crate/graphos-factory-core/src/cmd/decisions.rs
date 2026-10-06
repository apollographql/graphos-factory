//! `decisions <list|add|resolve|reopen|supersede|migrate>`: the workspace decision log —
//! judgement calls recorded and open questions still awaiting the user. The
//! only writer of `.factory/decisions.json` and of `.factory/decisions/`, where
//! every decision added since ADR 0118 is its own file with a random id, so a headless agent and a host
//! UI record resolutions through the same command (ADR 0026). `reopen` clears
//! a recorded answer so the user can revise it (ADR 0060); `supersede` marks
//! a resolved decision replaced, keeping its answer (ADR 0103).
//!
//! `add` refuses a record that names no alternative — no `--question`,
//! fewer than two `--choice`s and no `editorial` omit — with code
//! `no-alternative` (ADR 0113): a fact a reference settles is a finding
//! (`findings add`), not a decision.
//! `add --resolved` and `resolve` record `resolution.by: agent` unless
//! `--by` says otherwise. `migrate --split` (ADR 0113 §5) leaves the log
//! holding decisions only (`crate::split`).
//!
//! Exit codes: 0 done · 1 usage / unreadable / unknown id / invalid / refused.
//! `reopen --json` prints `{error, code, exit}` on a refusal; its codes are
//! `usage`, `decisions-missing`, `decisions-invalid`, `unknown-decision`,
//! `already-open` and `write-failed`; `supersede --json` the same shape, with
//! `not-resolved` and `already-superseded` in place of `already-open`.

use crate::args::{Args, Flags};
use crate::decisions::{
    self, Choice, JsonReason, NewDecision, NullHandling, Omit, ReopenRefusal, Resolution,
    SecretField,
};
use crate::json;
use serde_json::Value;
use std::path::Path;

pub const USAGE: &str = "usage: graphos-factory-core decisions <list|add|resolve|reopen|supersede|link|migrate> [workspace] …
  list    [workspace] [--open] [--causal] [--json]   --causal: in the order decisions presume one another (after, amends, shared affects)
  add     [workspace] --title T (--question Q | --choice LABEL --choice LABEL…) [--context C] [--phase P]
          [--choice-detail N:text]… [--multiple]
          each --choice LABEL is numbered 1, 2, 3… in flag order, and add prints `N  label` per choice;
          ids are kept only when every --choice is id:label (each id matching ^[a-z0-9][a-z0-9-]*$, a label
          after the colon); otherwise every value is a whole label (`2:1 split`, `v1:beta` stay labels);
          a duplicate id, or an invalid one when every --choice looks like id:label, is refused at the flag
          [--affects PATH]… [--requested-by WHO]
          [--slug S] [--after D-id]… [--amends D-id]…   the record file's name; decisions this one presumes or changes
          [--resolved [--chosen N]… [--note TEXT] [--decision TEXT] [--by user|agent]]   --by defaults to agent
          [--json]
          a record with no --question, fewer than two --choice and no editorial --omit is refused (no-alternative):
          a fact a reference or the wire settles is `findings add`
          [--omit 'operation|direction|path|reason']…   (counted only on a resolved record; direction response|request|behaviour)
          [--json-reason 'Type.field|reason']…   (read only on a resolved record)
          [--null-handling 'operation|argument|behavior']…   (behavior: send_null or omit; read only on a resolved record)
          [--secret-field 'Type.field|disposition|reason']…   (disposition: expose or exclude; secret-field-exposed)
          [--foreign-type NAME]…   (a type another subgraph owns, declared here under its owner's name; read only
          on a resolved record, and only by a target that lets a schema declare one)
  resolve [workspace] --id D-id [--chosen N]… [--note TEXT] [--decision TEXT] [--by user|agent] [--force]   --by defaults to agent
          --chosen takes a choice's id, or its exact label when exactly one choice has that label
  reopen  [workspace] --id D-id [--json]   clear a resolved or superseded decision's answer; status back to open
  supersede [workspace] --id D-id [--json] mark a resolved decision replaced; its answer is kept, its omits, json_reasons and null_handling stop counting
  link    [workspace] --id D-id (--after D-id | --amends D-id)… [--json]   a decision recorded as its own file presumes or changes another
          (how decision-overlap is answered); an old D-nnnn record never gains a field and is refused
  migrate [workspace] [--keep-md] [--force] [--dry-run] [--json]   import a legacy .factory/decisions.md into decisions.json
  migrate [workspace] --split [--sorted FILE] [--dry-run] [--json]   split an older-format log, leaving decisions.json holding decisions only
          kept: a record with a question, choices or an editorial omit stays a decision, id and all
          moved: `Hand edit codified:` / `Source patch:` / `Conformance waiver:` move onto the override, patch or
            waiver whose decision: names them (non-template context -> the entry's context:), then drop;
            one no entry cites becomes a finding (source: codify)
          `Upstream replaced:` / `Upstream refreshed:` become findings (source: sources)
          every other record is sorted by hand in FILE (YAML or JSON), a mapping from each D-nnnn to:
            {as: decision, question: Q, choices: [{id, label, detail?}…], chosen: [id…], by: user|agent, note: TEXT}
            {as: finding, cites: C, body: TEXT, title: T, affects: […], evidence: […], related: […]}   (all optional)
            {as: memory, line: TEXT}   a line under memory.md's `## Tried and rejected`
            {as: drop}
          a kept decision may be listed `as: decision` to gain its question and choices;
          without --sorted, a record still unassigned is listed and nothing is written (exit 1, code unsorted)";

fn usage(msg: &str) -> i32 {
    if !msg.is_empty() {
        eprintln!("decisions: {}", msg);
    }
    eprintln!("{}", USAGE);
    1
}

fn fail(msg: &str) -> i32 {
    eprintln!("decisions: {}", msg);
    1
}

pub fn main(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("list") => list(&argv[1..]),
        Some("add") => add(&argv[1..]),
        Some("resolve") => resolve(&argv[1..]),
        Some("reopen") => reopen(&argv[1..]),
        Some("supersede") => supersede(&argv[1..]),
        Some("link") => link(&argv[1..]),
        Some("migrate") => migrate(&argv[1..]),
        _ => {
            usage("expected a subcommand (list, add, resolve, reopen, supersede, link, or migrate)")
        }
    }
}

fn schemas_dir(args: &Args) -> Option<&Path> {
    args.get("schemas").map(Path::new)
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const LIST_FLAGS: Flags = Flags {
    boolean: &["open", "json", "causal"],
    valued: &["schemas"],
};

fn list(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &LIST_FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    let doc = match decisions::load(&dir, schemas_dir(&args)) {
        Ok(doc) => doc,
        Err(e) => return fail(&e),
    };
    let only_open = args.has("open");
    if args.has("causal") {
        return list_causal(&doc, only_open, args.has("json"));
    }
    let items: Vec<&Value> = json::get_arr(&doc, "decisions")
        .map(|a| {
            a.iter()
                .filter(|d| !only_open || json::get_str(d, "status") == Some("open"))
                .collect()
        })
        .unwrap_or_default();
    if args.has("json") {
        let arr = Value::Array(items.into_iter().cloned().collect());
        print!("{}", json::pretty(&json::object(vec![("decisions", arr)])));
        return 0;
    }
    if items.is_empty() {
        println!(
            "{}",
            if only_open {
                "No open decisions."
            } else {
                "No decisions recorded."
            }
        );
        return 0;
    }
    for d in items {
        print_decision(d);
    }
    0
}

/// `list --causal` (ADR 0118): the log in the order its decisions presume
/// one another, each with the edges that place it.
fn list_causal(doc: &Value, only_open: bool, json_out: bool) -> i32 {
    let ordered = match decisions::causal_order(doc) {
        Ok(o) => o,
        Err(ids) => {
            return fail(&format!(
                "list --causal: no order exists: {}. Either after/amends form a cycle, or they run against the age order of a span a third decision shares with both; fix the edge on one of them",
                ids.join(" -> ")
            ))
        }
    };
    let keep = |d: &Value| !only_open || json::get_str(d, "status") == Some("open");
    if json_out {
        let arr: Vec<Value> = ordered
            .iter()
            .filter(|(d, _)| keep(d))
            .map(|(d, edges)| {
                let mut v = d.clone();
                let via: Vec<Value> = edges
                    .iter()
                    .map(|e| {
                        json::object(vec![
                            ("before", Value::from(e.before.as_str())),
                            ("via", Value::from(e.via.as_str())),
                        ])
                    })
                    .collect();
                json::set(&mut v, "placed_after", Value::Array(via));
                v
            })
            .collect();
        print!(
            "{}",
            json::pretty(&json::object(vec![("decisions", Value::Array(arr))]))
        );
        return 0;
    }
    let mut any = false;
    for (d, edges) in &ordered {
        if !keep(d) {
            continue;
        }
        any = true;
        print_decision(d);
        for e in edges {
            println!("             via: {} {}", e.via, e.before);
        }
    }
    if !any {
        println!(
            "{}",
            if only_open {
                "No open decisions."
            } else {
                "No decisions recorded."
            }
        );
    }
    0
}

fn print_decision(d: &Value) {
    let status = json::get_str(d, "status").unwrap_or("open");
    let id = json::get_str(d, "id").unwrap_or("D-????");
    let title = json::get_str(d, "title").unwrap_or("");
    println!("[{:<9}] {}  {}", status, id, title);
    if let Some(q) = json::get_str(d, "question") {
        println!("             ? {}", q);
    }
    if status == "open" {
        for c in json::get_arr(d, "choices").into_iter().flatten() {
            let cid = json::get_str(c, "id").unwrap_or("");
            let label = json::get_str(c, "label").unwrap_or("");
            println!("             - {}: {}", cid, label);
        }
    } else if let Some(res) = json::get(d, "resolution") {
        let chosen = json::strings(json::field(res, "chosen"));
        if !chosen.is_empty() {
            // Each chosen id with its label, so a numbered id reads.
            let labels = recorded_pairs(d);
            let named: Vec<String> = chosen
                .iter()
                .map(|c| match labels.iter().find(|(id, _)| id == c) {
                    Some((_, label)) => format!("{}  {}", c, label),
                    None => c.clone(),
                })
                .collect();
            println!("             → {}", named.join(", "));
        }
        if let Some(note) = json::get_str(res, "note") {
            println!("             → {}", note);
        }
    }
}

fn build_resolution(args: &Args) -> Resolution {
    Resolution {
        chosen: args.all("chosen"),
        note: args.get("note").map(str::to_string),
        decision: args.get("decision").map(str::to_string),
        // ADR 0113: an answer nobody attributes is the agent's; the plugin
        // passes `--by user` on every user path.
        by: Some(args.get("by").unwrap_or("agent").to_string()),
        at: Some(
            args.get("at")
                .map(str::to_string)
                .unwrap_or_else(crate::today),
        ),
    }
}

/// The pattern a choice id must match: `decisions.schema.json`'s own, read
/// from the embedded schema so the flag and the file never disagree.
pub fn choice_id_pattern() -> String {
    crate::schemas::load("decisions.schema.json", None)
        .and_then(|s| {
            s.pointer("/$defs/choice/properties/id/pattern")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "^[a-z0-9][a-z0-9-]*$".to_string())
}

/// The `--choice` values of one call, as `(id, label)` pairs. All or
/// nothing: when every value is `id:label` (the text before its first `:`
/// matches the schema's id pattern and a label follows), the ids are kept,
/// the older calling form; otherwise every value is a whole label, numbered
/// "1", "2", "3" in flag order, so `2:1 split`, `v1:beta` or a URL is never
/// cut at its colon. A duplicate explicit id is refused, naming both flags.
/// When every value has the `word:label` shape (one word of letters, digits,
/// `_` or `-`, then a label with no space after the colon) but a word is not
/// a valid id, the caller meant the older form: refused at the flag, naming
/// the value and the pattern.
pub fn parse_choices(specs: &[String]) -> Result<Vec<(String, String)>, String> {
    let pattern = choice_id_pattern();
    let valid = regex::Regex::new(&pattern).map_err(|e| e.to_string())?;
    let explicit: Vec<Option<(String, String)>> = specs
        .iter()
        .map(|spec| {
            spec.split_once(':')
                // `https://…` is a URL, never an id and a label.
                .filter(|(id, label)| {
                    valid.is_match(id) && !label.trim().is_empty() && !label.starts_with("//")
                })
                .map(|(id, label)| (id.to_string(), label.trim().to_string()))
        })
        .collect();
    if !specs.is_empty() && explicit.iter().all(Option::is_some) {
        let mut out: Vec<(String, String)> = Vec::new();
        for (i, (id, label)) in explicit.into_iter().flatten().enumerate() {
            if let Some(j) = out.iter().position(|(seen, _)| *seen == id) {
                return Err(format!(
                    "--choice {:?}: the id {:?} is already taken by the earlier --choice {:?}; ids are unique in a record (drop every id to have the choices numbered)",
                    specs[i], id, specs[j]
                ));
            }
            out.push((id, label));
        }
        return Ok(out);
    }
    // The older form attempted: every value is a valid `id:label` or has
    // its shape, and at least one word is not a valid id.
    let shaped = |spec: &str| {
        spec.split_once(':').is_some_and(|(id, label)| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && !label.trim().is_empty()
                && !label.starts_with(char::is_whitespace)
                && !label.starts_with("//")
        })
    };
    if !specs.is_empty()
        && specs
            .iter()
            .zip(&explicit)
            .all(|(spec, e)| e.is_some() || shaped(spec))
    {
        if let Some((spec, _)) = specs.iter().zip(&explicit).find(|(_, e)| e.is_none()) {
            let id = spec.split_once(':').map(|(id, _)| id).unwrap_or_default();
            return Err(format!(
                "--choice {:?}: every --choice in this call looks like id:label but {:?} does not match {}; drop the ids to have the choices numbered, or fix the id",
                spec, id, pattern
            ));
        }
    }
    let mut out = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let label = spec.trim();
        if label.is_empty() {
            return Err(format!("--choice {:?}: the label is empty", spec));
        }
        out.push(((i + 1).to_string(), label.to_string()));
    }
    Ok(out)
}

/// `--chosen` values as choice ids: an id the record's choices hold, or the
/// exact label of exactly one of them. A record with no choices takes the
/// value as given, held to the id pattern.
pub fn chosen_ids(values: &[String], choices: &[(String, String)]) -> Result<Vec<String>, String> {
    let pattern = choice_id_pattern();
    let valid = regex::Regex::new(&pattern).map_err(|e| e.to_string())?;
    let listed = || {
        choices
            .iter()
            .map(|(id, label)| format!("{}  {}", id, label))
            .collect::<Vec<_>>()
            .join("; ")
    };
    values
        .iter()
        .map(|v| {
            if choices.iter().any(|(id, _)| id == v) {
                return Ok(v.clone());
            }
            let by_label: Vec<&String> = choices
                .iter()
                .filter(|(_, label)| label == v.trim())
                .map(|(id, _)| id)
                .collect();
            match by_label.len() {
                1 => Ok(by_label[0].clone()),
                0 if choices.is_empty() && valid.is_match(v) => Ok(v.clone()),
                0 if choices.is_empty() => {
                    Err(format!("--chosen {:?}: not a choice id ({})", v, pattern))
                }
                0 => Err(format!(
                    "--chosen {:?} names no choice id or label; the choices are: {}",
                    v,
                    listed()
                )),
                _ => Err(format!(
                    "--chosen {:?} is the label of {} choices; pass the id: {}",
                    v,
                    by_label.len(),
                    listed()
                )),
            }
        })
        .collect()
}

fn collect_choices(args: &Args) -> Result<Vec<Choice>, String> {
    let parsed = parse_choices(&args.all("choice"))?;
    let mut choices: Vec<Choice> = parsed
        .into_iter()
        .map(|(id, label)| Choice {
            id,
            label,
            detail: None,
        })
        .collect();
    for spec in args.all("choice-detail") {
        let Some((id, detail)) = spec.split_once(':') else {
            return Err(format!("--choice-detail expects id:text, got {:?}", spec));
        };
        match choices.iter_mut().find(|c| c.id == id.trim()) {
            Some(c) => c.detail = Some(detail.trim().to_string()),
            None => {
                return Err(format!(
                    "--choice-detail names an unknown choice id {:?}",
                    id.trim()
                ))
            }
        }
    }
    Ok(choices)
}

/// `--omit operation|direction|path|reason`, repeatable — the minimum a
/// caller needs to record one structured omission alongside a decision
/// (ADR 0036). `|`, not `:`, since `operation` is itself `method:path`
/// (`post:/candidate.info`) and a JSON-array path segment can carry `:`
/// too; `direction`/`reason` are validated against decisions.schema.json's
/// own vocabulary, same as `decisions migrate`'s fenced-block importer.
fn collect_omits(args: &Args) -> Result<Vec<Omit>, String> {
    let mut omits = Vec::new();
    for spec in args.all("omit") {
        let parts: Vec<&str> = spec.splitn(4, '|').collect();
        let [operation, direction, path, reason] = parts[..] else {
            return Err(format!(
                "--omit expects operation|direction|path|reason, got {:?}",
                spec
            ));
        };
        crate::decisions::check_omit_vocabulary(direction, reason)
            .map_err(|e| format!("--omit {:?}: {}", spec, e))?;
        omits.push(Omit {
            operation: operation.to_string(),
            direction: direction.to_string(),
            path: path.to_string(),
            reason: reason.to_string(),
        });
    }
    Ok(omits)
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const ADD_FLAGS: Flags = Flags {
    boolean: &["multiple", "resolved", "json"],
    valued: &[
        "title",
        "date",
        "phase",
        "question",
        "context",
        "requested-by",
        "affects",
        "choice",
        "choice-detail",
        "omit",
        "secret-field",
        "json-reason",
        "null-handling",
        "foreign-type",
        "slug",
        "after",
        "amends",
        "chosen",
        "note",
        "decision",
        "by",
        "at",
        "schemas",
    ],
};

/// `--secret-field Type.field|disposition|reason`, repeatable — records that
/// a credential-shaped response field's exposure was reviewed (ADR 0078).
/// `|`, not `:`, matching `--omit`'s convention; `Type.field` splits on the
/// first `.`, since neither a GraphQL type nor a field name ever contains
/// one. `disposition` is validated against decisions.schema.json's own
/// vocabulary (`expose`/`exclude`), same as `--omit`'s `direction`/`reason`.
fn collect_secret_fields(args: &Args) -> Result<Vec<SecretField>, String> {
    let mut fields = Vec::new();
    for spec in args.all("secret-field") {
        let parts: Vec<&str> = spec.splitn(3, '|').collect();
        let [type_field, disposition, reason] = parts[..] else {
            return Err(format!(
                "--secret-field expects Type.field|disposition|reason, got {:?}",
                spec
            ));
        };
        let Some((type_name, field)) = type_field.split_once('.') else {
            return Err(format!(
                "--secret-field expects Type.field|disposition|reason, got {:?} (no . in {:?})",
                spec, type_field
            ));
        };
        if !["expose", "exclude"].contains(&disposition) {
            return Err(format!(
                "--secret-field {:?}: disposition must be expose or exclude, got {:?}",
                spec, disposition
            ));
        }
        fields.push(SecretField {
            type_name: type_name.to_string(),
            field: field.to_string(),
            disposition: disposition.to_string(),
            reason: reason.to_string(),
        });
    }
    Ok(fields)
}

/// `--json-reason Type.field|reason`, repeatable: why a response field is
/// left typed as the workspace's own JSON scalar (ADR 0073). `|` separates
/// the reason from the field's own key the same way `spans json-accounting`
/// keys it; the key's single `.` separates the GraphQL type name from the
/// field name, neither of which can itself contain one.
fn collect_json_reasons(args: &Args) -> Result<Vec<JsonReason>, String> {
    let mut out = Vec::new();
    for spec in args.all("json-reason") {
        let parts: Vec<&str> = spec.splitn(2, '|').collect();
        let [key, reason] = parts[..] else {
            return Err(format!(
                "--json-reason expects Type.field|reason, got {:?}",
                spec
            ));
        };
        // A GraphQL field is one level under its type: `A.b.c` would split
        // into field `b.c`, which no SDL field can ever match.
        let Some((type_name, field)) = key.split_once('.').filter(|(_, f)| !f.contains('.')) else {
            return Err(format!(
                "--json-reason {:?}: expected Type.field before the '|', got {:?}",
                spec, key
            ));
        };
        if !crate::json_accounting::REASONS.contains(&reason) {
            return Err(format!(
                "--json-reason {:?}: reason must be one of {:?}, got {:?}",
                spec,
                crate::json_accounting::REASONS,
                reason
            ));
        }
        out.push(JsonReason {
            type_name: type_name.to_string(),
            field: field.to_string(),
            reason: reason.to_string(),
        });
    }
    Ok(out)
}

/// `--null-handling operation|argument|behavior`, repeatable: what the
/// connector sends when the caller passes the argument as an explicit
/// `null` (ADR 0079). `|` for the same reason as `--omit`.
fn collect_null_handling(args: &Args) -> Result<Vec<NullHandling>, String> {
    let mut out = Vec::new();
    for spec in args.all("null-handling") {
        let parts: Vec<&str> = spec.splitn(3, '|').collect();
        let [operation, argument, behavior] = parts[..] else {
            return Err(format!(
                "--null-handling expects operation|argument|send_null or omit, got {:?}",
                spec
            ));
        };
        if !["send_null", "omit"].contains(&behavior) {
            return Err(format!(
                "--null-handling {:?}: behavior must be send_null or omit, got {:?}",
                spec, behavior
            ));
        }
        out.push(NullHandling {
            operation: operation.to_string(),
            argument: argument.to_string(),
            behavior: behavior.to_string(),
        });
    }
    Ok(out)
}

/// A request-direction omit on a member the source marks required is
/// accepted (it may be a real vendor fact) but contradicts the source, so say
/// so at write time; the write-body proof keeps the member a gap regardless.
fn warn_required_omits(dir: &Path, omits: &[Omit]) {
    if !omits.iter().any(|o| o.direction == "request") {
        return;
    }
    let Some(inventory) =
        crate::factory_io::read_to_string_optional(dir, ".factory/inventory.json")
            .ok()
            .flatten()
            .and_then(|t| json::parse(&t).ok())
    else {
        return;
    };
    let shapes = json::get_obj(&inventory, "shapes")
        .cloned()
        .unwrap_or_default();
    for o in omits.iter().filter(|o| o.direction == "request") {
        let Some(op) = json::get_arr(&inventory, "operations")
            .into_iter()
            .flatten()
            .find(|op| json::get_str(op, "key") == Some(o.operation.as_str()))
        else {
            continue;
        };
        let required = crate::request_serialization::required_request_members(op, &shapes);
        let first = o.path.split(['.', '[']).next().unwrap_or("");
        let hit: Vec<&String> = if o.path == crate::obligations::ROOT_OMIT {
            required.iter().collect()
        } else {
            required.iter().filter(|r| r.as_str() == first).collect()
        };
        if !hit.is_empty() {
            eprintln!(
                "decisions: warning: --omit {} request {} covers {}, which the source marks required; the write-body proof keeps a required member a gap (required_member_omitted)",
                o.operation,
                o.path,
                hit.iter().map(|s| format!("`{}`", s)).collect::<Vec<_>>().join(", ")
            );
        }
    }
}

fn add(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &ADD_FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    let Some(title) = args.get("title") else {
        return usage("add: --title is required");
    };
    let choices = match collect_choices(&args) {
        Ok(choices) => choices,
        Err(e) => return fail(&e),
    };
    let omits = match collect_omits(&args) {
        Ok(omits) => omits,
        Err(e) => return fail(&e),
    };
    // ADR 0113 §1: a decision carries its alternative. A question or two
    // choices name it; an `editorial` omit carries it implicitly ("expose
    // it"), which is why that reason lives only on a decision. Without any
    // of them the record is a fact (a finding) or narrative (memory.md, the
    // commit).
    let has_question = args.get("question").is_some_and(|q| !q.trim().is_empty());
    let has_editorial_omit = omits.iter().any(|o| o.reason == "editorial");
    if !has_question && choices.len() < 2 && !has_editorial_omit {
        return refuse(
            args.has("json"),
            ReopenRefusal {
                code: "no-alternative",
                message: "add: refused (no-alternative) — a decision names what else could have been done: give it a --question, two or more --choice LABEL, or an --omit whose reason is editorial (the alternative is to expose the path). A fact a reference or the wire settles is `graphos-factory-core findings add --cites …`; a vendor quirk or a negative result is a memory.md line".into(),
            },
        );
    }
    let secret_fields = match collect_secret_fields(&args) {
        Ok(fields) => fields,
        Err(e) => return fail(&e),
    };
    let json_reasons = match collect_json_reasons(&args) {
        Ok(j) => j,
        Err(e) => return fail(&e),
    };
    let null_handling = match collect_null_handling(&args) {
        Ok(n) => n,
        Err(e) => return fail(&e),
    };
    let foreign_types = args.all("foreign-type");
    for name in &foreign_types {
        if let Err(e) = decisions::check_foreign_type(name) {
            return fail(&e);
        }
    }
    let resolving =
        args.has("resolved") || args.has("chosen") || args.has("note") || args.has("decision");
    let resolution = if resolving {
        let mut r = build_resolution(&args);
        if r.chosen.is_empty() && r.note.is_none() && r.decision.is_none() {
            return usage("add --resolved: give at least one of --chosen, --note, or --decision");
        }
        r.chosen = match chosen_ids(&r.chosen, &pairs(&choices)) {
            Ok(ids) => ids,
            Err(e) => return fail(&e),
        };
        Some(r)
    } else {
        None
    };
    let recorded_choices = pairs(&choices);
    let new = NewDecision {
        title: title.to_string(),
        date: args
            .get("date")
            .map(str::to_string)
            .unwrap_or_else(crate::today),
        phase: args.get("phase").map(str::to_string),
        question: args.get("question").map(str::to_string),
        context: args.get("context").map(str::to_string),
        requested_by: args.get("requested-by").map(str::to_string),
        multiple: args.has("multiple"),
        affects: args.all("affects"),
        choices,
        resolution,
        omits,
        secret_fields,
        json_reasons,
        null_handling,
        foreign_types,
        slug: args.get("slug").map(str::to_string),
        after: args.all("after"),
        amends: args.all("amends"),
    };
    if new.resolution.is_none()
        && !(new.omits.is_empty()
            && new.json_reasons.is_empty()
            && new.null_handling.is_empty()
            && new.foreign_types.is_empty())
    {
        eprintln!(
            "decisions: warning: this record is open, so its --omit, --json-reason, \
             --null-handling and --foreign-type entries do not count until `decisions resolve` settles it"
        );
    }
    // Recorded whatever the target, since the log is the target's to read;
    // said aloud where nothing will, so the prefix error that follows in
    // lint is no surprise.
    if !new.foreign_types.is_empty() && !crate::target::active().foreign_types {
        eprintln!(
            "decisions: note: this workspace's target gives every type the workspace's prefix, so no rule reads --foreign-type here; the record is kept"
        );
    }
    warn_required_omits(&dir, &new.omits);
    let mut doc = match decisions::load(&dir, schemas_dir(&args)) {
        Ok(doc) => doc,
        Err(e) => return fail(&e),
    };
    let id = match decisions::add(&mut doc, new) {
        Ok(id) => id,
        Err(e) => return fail(&e),
    };
    if let Err(e) = decisions::save(&dir, &doc, schemas_dir(&args)) {
        return fail(&e);
    }
    if args.has("json") {
        // The record file the add wrote (ADR 0118), so a caller that stages
        // or copies it does not glob for it.
        let path = match crate::record_log::path_of(&dir, &crate::record_log::DECISIONS, &id) {
            Ok(path) => path.map(Value::from).unwrap_or(Value::Null),
            Err(e) => return fail(&e),
        };
        let choices = recorded_choices
            .iter()
            .map(|(id, label)| {
                json::object(vec![
                    ("id", Value::from(id.as_str())),
                    ("label", Value::from(label.as_str())),
                ])
            })
            .collect();
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("id", Value::from(id)),
                ("path", path),
                ("choices", Value::Array(choices)),
            ]))
        );
    } else {
        println!("recorded {}", id);
        print_choices(&recorded_choices);
    }
    0
}

/// Choices as `(id, label)` pairs.
fn pairs(choices: &[Choice]) -> Vec<(String, String)> {
    choices
        .iter()
        .map(|c| (c.id.clone(), c.label.clone()))
        .collect()
}

/// A record's choices as `(id, label)` pairs, from the log.
fn recorded_pairs(record: &Value) -> Vec<(String, String)> {
    json::get_arr(record, "choices")
        .into_iter()
        .flatten()
        .map(|c| {
            (
                json::get_str(c, "id").unwrap_or("").to_string(),
                json::get_str(c, "label").unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// One `id  label` line per choice: what `--chosen` takes.
fn print_choices(choices: &[(String, String)]) {
    for (id, label) in choices {
        println!("  {}  {}", id, label);
    }
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const RESOLVE_FLAGS: Flags = Flags {
    boolean: &["force", "json"],
    valued: &["id", "chosen", "note", "decision", "by", "at", "schemas"],
};

fn resolve(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &RESOLVE_FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    let Some(id) = args.get("id") else {
        return usage("resolve: --id D-id is required");
    };
    if !(args.has("chosen") || args.has("note") || args.has("decision")) {
        return usage("resolve: give at least one of --chosen, --note, or --decision");
    }
    let mut doc = match decisions::load(&dir, schemas_dir(&args)) {
        Ok(doc) => doc,
        Err(e) => return fail(&e),
    };
    let choices = decisions::find(&doc, id)
        .map(recorded_pairs)
        .unwrap_or_default();
    let mut resolution = build_resolution(&args);
    resolution.chosen = match chosen_ids(&resolution.chosen, &choices) {
        Ok(ids) => ids,
        Err(e) => return fail(&format!("resolve {}: {}", id, e)),
    };
    let chosen = resolution.chosen.clone();
    if let Err(e) = decisions::resolve(&mut doc, id, resolution, args.has("force")) {
        return fail(&e);
    }
    if let Err(e) = decisions::save(&dir, &doc, schemas_dir(&args)) {
        return fail(&e);
    }
    println!("resolved {}", id);
    print_choices(
        &choices
            .into_iter()
            .filter(|(c, _)| chosen.contains(c))
            .collect::<Vec<_>>(),
    );
    0
}

/// A `reopen` refusal: the message on stderr (with the usage text for a usage
/// error, like every other verb), and with `--json` an `{error, code, exit}`
/// object on stdout, the shape `init` uses. Always exit 1, the code every
/// `decisions` verb refuses with.
fn refuse(json_out: bool, refusal: ReopenRefusal) -> i32 {
    eprintln!("decisions: {}", refusal.message);
    if refusal.code == "usage" {
        eprintln!("{}", USAGE);
    }
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("error", Value::from(refusal.message)),
                ("code", Value::from(refusal.code)),
                ("exit", Value::from(1)),
            ]))
        );
    }
    1
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const REOPEN_FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["id", "schemas"],
};

/// The log a status change acts on. It acts on a recorded decision: no log
/// means no decision, and an unreadable or invalid one is never silently
/// replaced by an empty log the way `add` would start one.
fn load_recorded(dir: &Path, args: &Args, verb: &str) -> Result<Value, ReopenRefusal> {
    match decisions::load_present(dir, schemas_dir(args)) {
        Ok(Some(doc)) => Ok(doc),
        Ok(None) => Err(ReopenRefusal {
            code: "decisions-missing",
            message: format!(
                "the workspace has no decision log ({} nor a record under {}/); there is no decision to {}",
                decisions::FILE,
                crate::record_log::DECISIONS.dir,
                verb
            ),
        }),
        Err(e) => Err(ReopenRefusal {
            code: "decisions-invalid",
            message: if e.starts_with(decisions::FILE) {
                e
            } else {
                format!("{}: {}", decisions::FILE, e)
            },
        }),
    }
}

fn reopen(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &REOPEN_FLAGS);
    let json_out = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    let Some(id) = args.get("id") else {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "usage",
                message: "reopen: --id D-id is required".into(),
            },
        );
    };
    let mut doc = match load_recorded(&dir, &args, "reopen") {
        Ok(doc) => doc,
        Err(refusal) => return refuse(json_out, refusal),
    };
    let reopened = match decisions::reopen(&mut doc, id) {
        Ok(r) => r,
        Err(refusal) => return refuse(json_out, refusal),
    };
    if let Err(e) = decisions::save(&dir, &doc, schemas_dir(&args)) {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "write-failed",
                message: e,
            },
        );
    }
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("id", Value::from(id)),
                ("status", Value::from("open")),
                (
                    "previous_status",
                    Value::from(reopened.previous_status.as_str())
                ),
                ("cleared", reopened.cleared.unwrap_or(Value::Null)),
                ("exit", Value::from(0)),
            ]))
        );
    } else {
        println!("reopened {} (was {})", id, reopened.previous_status);
    }
    0
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const SUPERSEDE_FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["id", "schemas"],
};

/// `supersede --id D-nnnn`: a resolved decision a later one, or a change to
/// the service, replaced (ADR 0103). The fix `lint`'s `stale-omit` names.
fn supersede(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &SUPERSEDE_FLAGS);
    let json_out = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    let Some(id) = args.get("id") else {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "usage",
                message: "supersede: --id D-id is required".into(),
            },
        );
    };
    let mut doc = match load_recorded(&dir, &args, "supersede") {
        Ok(doc) => doc,
        Err(refusal) => return refuse(json_out, refusal),
    };
    if let Err(refusal) = decisions::supersede(&mut doc, id) {
        return refuse(json_out, refusal);
    }
    if let Err(e) = decisions::save(&dir, &doc, schemas_dir(&args)) {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "write-failed",
                message: e,
            },
        );
    }
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("id", Value::from(id)),
                ("status", Value::from("superseded")),
                ("previous_status", Value::from("resolved")),
                ("exit", Value::from(0)),
            ]))
        );
    } else {
        println!("superseded {} (was resolved)", id);
    }
    0
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const LINK_FLAGS: Flags = Flags {
    boolean: &["json"],
    valued: &["id", "after", "amends", "schemas"],
};

/// `link --id D-id --after D-id… --amends D-id…` (ADR 0118): edges on a
/// record added since the single file stopped growing.
fn link(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &LINK_FLAGS);
    let json_out = args.has("json");
    let dir = Path::new(&args.dir()).to_path_buf();
    let Some(id) = args.get("id") else {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "usage",
                message: "link: --id is required".into(),
            },
        );
    };
    let (after, amends) = (args.all("after"), args.all("amends"));
    if after.is_empty() && amends.is_empty() {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "usage",
                message: "link: give at least one --after or --amends".into(),
            },
        );
    }
    let mut doc = match load_recorded(&dir, &args, "link") {
        Ok(doc) => doc,
        Err(refusal) => return refuse(json_out, refusal),
    };
    if let Err(refusal) = decisions::link(&mut doc, id, after, amends) {
        return refuse(json_out, refusal);
    }
    if let Err(e) = decisions::save(&dir, &doc, schemas_dir(&args)) {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "write-failed",
                message: e,
            },
        );
    }
    let rec = decisions::find(&doc, id).cloned().unwrap_or(Value::Null);
    if json_out {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("id", Value::from(id)),
                (
                    "after",
                    rec.get("after").cloned().unwrap_or(Value::Array(vec![]))
                ),
                (
                    "amends",
                    rec.get("amends").cloned().unwrap_or(Value::Array(vec![]))
                ),
                ("exit", Value::from(0)),
            ]))
        );
    } else {
        println!("linked {}", id);
    }
    0
}

/// The flags this verb accepts; dispatch refuses any other (ADR 0097).
pub const MIGRATE_FLAGS: Flags = Flags {
    boolean: &["keep-md", "force", "dry-run", "json", "split"],
    valued: &["schemas", "sorted"],
};

fn migrate(argv: &[String]) -> i32 {
    let args = Args::parse(argv, &MIGRATE_FLAGS);
    let dir = Path::new(&args.dir()).to_path_buf();
    if args.has("split") {
        return migrate_split(&args, &dir);
    }
    if args.has("sorted") {
        return usage("migrate: --sorted belongs to --split");
    }
    // Both legacy and current logs live under `.factory/`, so both are
    // custody's: a symlinked log is refused, never migrated through (ADR 0025).
    const LEGACY_MD: &str = ".factory/decisions.md";
    let md_present = match crate::factory_io::symlink_metadata(&dir, LEGACY_MD) {
        Ok(found) => found.is_some(),
        Err(e) => {
            return fail(&e.to_string());
        }
    };
    let json_present = match crate::factory_io::symlink_metadata(&dir, decisions::FILE) {
        Ok(found) => found.is_some(),
        Err(e) => {
            return fail(&e.to_string());
        }
    };
    if !md_present {
        if json_present {
            println!(
                "decisions: already migrated — {} exists and there is no decisions.md.",
                decisions::FILE
            );
            return 0;
        }
        return fail("no .factory/decisions.md to migrate");
    }
    if json_present && !args.has("force") {
        return fail(&format!(
            "{} already exists; pass --force to overwrite it from decisions.md",
            decisions::FILE
        ));
    }
    let md = match crate::factory_io::read_to_string(&dir, LEGACY_MD) {
        Ok(t) => t,
        Err(e) => return fail(&e.to_string()),
    };
    let (records, warnings) = match decisions::parse_markdown(&md) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    for w in &warnings {
        eprintln!("decisions: warning: {}", w);
    }
    if records.is_empty() {
        return fail(
            "no D-nnnn decision blocks found in decisions.md; nothing to migrate — convert it by hand or leave it in place",
        );
    }
    let n = records.len();
    let plural = if n == 1 { "" } else { "s" };
    if args.has("dry-run") {
        if args.has("json") {
            print!(
                "{}",
                json::pretty(&json::object(vec![
                    ("migrated", Value::from(n)),
                    ("decisions", Value::Array(records)),
                ]))
            );
        } else {
            println!(
                "decisions (dry run): would migrate {} record{} from decisions.md to {}",
                n,
                plural,
                decisions::FILE
            );
        }
        return 0;
    }
    let doc = json::object(vec![
        ("contract_version", Value::from(1)),
        ("decisions", Value::Array(records)),
    ]);
    if let Err(e) = decisions::save_single(&dir, &doc, schemas_dir(&args)) {
        return fail(&e);
    }
    let removed = if args.has("keep-md") {
        false
    } else {
        match crate::factory_io::remove_file(&dir, LEGACY_MD) {
            Ok(()) => true,
            Err(e) => {
                eprintln!(
                    "decisions: warning: wrote {} but could not remove decisions.md: {}",
                    decisions::FILE,
                    e
                );
                false
            }
        }
    };
    if args.has("json") {
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("migrated", Value::from(n)),
                (
                    "warnings",
                    Value::Array(warnings.iter().map(|w| Value::from(w.as_str())).collect()),
                ),
                ("removed_md", Value::from(removed)),
            ]))
        );
    } else {
        println!(
            "decisions: migrated {} record{} to {} ({})",
            n,
            plural,
            decisions::FILE,
            if removed {
                "removed decisions.md"
            } else {
                "kept decisions.md"
            }
        );
    }
    0
}

/// `migrate --split [--sorted FILE] [--dry-run] [--json]` (ADR 0113 §5):
/// compute the split with `crate::split::plan`, list what only a person can
/// sort when it is unassigned, and write nothing until every output
/// validates.
fn migrate_split(args: &Args, dir: &Path) -> i32 {
    let json_out = args.has("json");
    if args.has("keep-md") || args.has("force") {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "usage",
                message: "migrate --split takes only --sorted, --dry-run and --json".into(),
            },
        );
    }
    let doc = match load_recorded(dir, args, "split") {
        Ok(doc) => doc,
        Err(refusal) => return refuse(json_out, refusal),
    };
    if crate::record_log::has_directory(dir) {
        return refuse(
            json_out,
            ReopenRefusal {
                code: "split-refused",
                message: "migrate --split reads and writes decisions.json and findings.json only, and this workspace already records decisions or findings one file each under .factory/; the split is for a log written before them".into(),
            },
        );
    }
    let sorted = match args.get("sorted") {
        Some(path) => {
            let text = match crate::factory_io::read_named_path(Path::new(path))
                .and_then(|b| String::from_utf8(b).map_err(|_| format!("{}: not UTF-8", path)))
            {
                Ok(t) => t,
                Err(e) => {
                    return refuse(
                        json_out,
                        ReopenRefusal {
                            code: "sorted-invalid",
                            message: format!("--sorted {}: {}", path, e),
                        },
                    )
                }
            };
            match crate::split::parse_sorted(&text) {
                Ok(v) => Some(v),
                Err(e) => {
                    return refuse(
                        json_out,
                        ReopenRefusal {
                            code: "sorted-invalid",
                            message: format!("--sorted {}: {}", path, e),
                        },
                    )
                }
            }
        }
        None => None,
    };
    let plan = match crate::split::plan(dir, &doc, sorted.as_ref(), schemas_dir(args)) {
        Ok(p) => p,
        Err(crate::split::Refusal::Unsorted(records)) => {
            eprintln!(
                "decisions: migrate --split: {} record{} only the agent running the migration can sort; nothing was written. Assign each in a --sorted FILE (decision with its question and choices, finding, memory, or drop: `decisions --help`):",
                records.len(),
                if records.len() == 1 { "" } else { "s" }
            );
            for (id, title) in &records {
                eprintln!("  {}  {}", id, title);
            }
            if json_out {
                let unsorted: Vec<Value> = records
                    .iter()
                    .map(|(id, title)| {
                        json::object(vec![
                            ("id", Value::from(id.as_str())),
                            ("title", Value::from(title.as_str())),
                        ])
                    })
                    .collect();
                print!(
                    "{}",
                    json::pretty(&json::object(vec![
                        (
                            "error",
                            Value::from(format!(
                                "{} record(s) unassigned; pass --sorted FILE",
                                records.len()
                            )),
                        ),
                        ("code", Value::from("unsorted")),
                        ("unsorted", Value::Array(unsorted)),
                        ("exit", Value::from(1)),
                    ]))
                );
            }
            return 1;
        }
        Err(crate::split::Refusal::Invalid(e)) => {
            return refuse(
                json_out,
                ReopenRefusal {
                    code: "split-refused",
                    message: format!("migrate --split: {}; nothing was written", e),
                },
            )
        }
    };
    for w in &plan.warnings {
        eprintln!("decisions: warning: {}", w);
    }
    let dry_run = args.has("dry-run");
    if !dry_run {
        if let Err(e) = crate::split::write(dir, &plan, schemas_dir(args)) {
            return refuse(
                json_out,
                ReopenRefusal {
                    code: "write-failed",
                    message: e,
                },
            );
        }
    }
    const COUNTS: [&str; 6] = ["kept", "decision", "moved", "finding", "memory", "dropped"];
    if json_out {
        let records: Vec<Value> = plan
            .outcomes
            .iter()
            .map(|o| {
                let mut v = json::object(vec![
                    ("id", Value::from(o.id.as_str())),
                    ("title", Value::from(o.title.as_str())),
                    ("class", Value::from(o.class.label())),
                    ("outcome", Value::from(o.outcome)),
                ]);
                if let Some(f) = &o.finding {
                    json::set(&mut v, "finding", Value::from(f.as_str()));
                }
                if let Some(d) = &o.detail {
                    json::set(&mut v, "detail", Value::from(d.as_str()));
                }
                v
            })
            .collect();
        let counts = json::object(
            COUNTS
                .iter()
                .map(|c| (*c, Value::from(plan.count(c))))
                .collect(),
        );
        print!(
            "{}",
            json::pretty(&json::object(vec![
                ("split", Value::Bool(true)),
                ("dry_run", Value::Bool(dry_run)),
                ("counts", counts),
                ("records", Value::Array(records)),
                (
                    "warnings",
                    Value::Array(
                        plan.warnings
                            .iter()
                            .map(|w| Value::from(w.as_str()))
                            .collect()
                    ),
                ),
                (
                    "writes",
                    Value::Array(plan.writes().into_iter().map(Value::from).collect()),
                ),
                ("exit", Value::from(0)),
            ]))
        );
        return 0;
    }
    println!(
        "decisions{}: split {} record{}: {} kept, {} re-recorded as decisions, {} moved onto entries, {} to findings, {} to memory.md, {} dropped",
        if dry_run { " (dry run)" } else { "" },
        plan.outcomes.len(),
        if plan.outcomes.len() == 1 { "" } else { "s" },
        plan.count("kept"),
        plan.count("decision"),
        plan.count("moved"),
        plan.count("finding"),
        plan.count("memory"),
        plan.count("dropped"),
    );
    for o in &plan.outcomes {
        println!(
            "  {}  {:<8} {}{}{}",
            o.id,
            o.outcome,
            o.title,
            o.finding
                .as_ref()
                .map(|f| format!(" -> {}", f))
                .unwrap_or_default(),
            o.detail
                .as_ref()
                .map(|d| format!(" ({})", d))
                .unwrap_or_default()
        );
    }
    if !dry_run {
        println!("  wrote {}", plan.writes().join(", "));
    }
    0
}

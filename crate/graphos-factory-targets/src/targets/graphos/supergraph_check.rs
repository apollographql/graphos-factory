//! `supergraph_check`: the subgraph checked against the user's published
//! supergraph with `rover subgraph check` (ADR 0133).
//!
//! The layer runs `skills/graphos-factory/scripts/supergraph-check.sh`, the
//! way the core runs its wrappers: `bash <script> <workspace>`, with this
//! binary handed to it as `GRAPHOS_FACTORY_CORE_BIN` unless the caller named
//! one. The script, not the binary, talks to GraphOS, and only with
//! `APOLLO_KEY` in the environment and a graph ref given for the run
//! (`GRAPHOS_FACTORY_GRAPH_REF`, mode `explicit`) or by the user's own switch
//! (`GRAPHOS_FACTORY_SUPERGRAPH_CHECK=auto` with `APOLLO_GRAPH_REF`, mode
//! `auto`); otherwise it exits 3 and the layer is `not_run`. The binary never reads the key; the script removes
//! its value from everything it prints and from rover's JSON, should
//! anything have echoed it.
//!
//! Exit codes map as the core's do: 0 pass, 3 not_run, 127 skipped, anything
//! else fail. The row records the command and exit code, the reason when not
//! `pass`, the failing build errors and checks as findings (at most 20), and
//! `details`: the graph ref (never the key), the mode that chose it, rover's
//! version, the subgraph
//! name, the composition result, every check task's status, the operation
//! check's counts and the Studio URL rover printed, read from rover's
//! `--format json` output.

use graphos_factory_core::json::{get, get_arr, get_obj, get_str};
use graphos_factory_core::target::LayerInput;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The script's file name, in the target's `scripts/`.
pub const SCRIPT: &str = "supergraph-check.sh";

/// The variable naming the target's `scripts/` directory. The session-start
/// hook and `env.sh` set it; without it the directory is found from the
/// core's (`script_dir`).
pub const SCRIPTS_ENV: &str = "GRAPHOS_FACTORY_TARGET_SCRIPTS";

/// The variable the script writes rover's JSON to when set.
pub const JSON_ENV: &str = "SUPERGRAPH_CHECK_JSON";

/// At most this many findings are recorded.
const MAX_FINDINGS: usize = 20;

/// The directory holding [`SCRIPT`]: `named` (the value of
/// `$GRAPHOS_FACTORY_TARGET_SCRIPTS`) when set, and only it, else beside the core's scripts in either layout: an
/// installed skill carries its core inside it
/// (`skills/graphos-factory/graphos-factory-core/scripts`, the target's
/// scripts two levels up), a checkout or plugin keeps it at the root
/// (`graphos-factory-core/scripts`, the target's under
/// `skills/graphos-factory/scripts`). `Err` names where it looked.
pub fn script_dir(core_scripts: &Path, named: Option<&OsStr>) -> Result<PathBuf, String> {
    if let Some(dir) = named {
        let dir = PathBuf::from(dir);
        return if dir.join(SCRIPT).is_file() {
            Ok(dir)
        } else {
            Err(format!(
                "supergraph_check: {} is not found in {} ({}) — not installed",
                SCRIPT,
                dir.display(),
                SCRIPTS_ENV
            ))
        };
    }
    let candidates = [
        core_scripts.join("../../scripts"),
        core_scripts.join("../../skills/graphos-factory/scripts"),
    ];
    candidates
        .iter()
        .find(|d| d.join(SCRIPT).is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "supergraph_check: {} is not installed beside the core scripts at {}; set {} to the skill's scripts/ directory",
                SCRIPT,
                core_scripts.display(),
                SCRIPTS_ENV
            )
        })
}

/// A new directory under the system temp directory, mode 0700, created by
/// this call: `create` fails when the path already exists, so a directory
/// someone else placed at a predictable name is never used; a few names are
/// tried.
fn private_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let base = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut last = String::new();
    for attempt in 0..16u32 {
        let dir = base.join(format!(
            "supergraph-check-{}-{}-{}",
            std::process::id(),
            nanos,
            attempt
        ));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) => last = format!("{}: {}", dir.display(), e),
        }
    }
    Err(last)
}

/// The layer: run the script, read rover's JSON, build the row.
pub fn run(input: &LayerInput) -> Value {
    let named = std::env::var_os(SCRIPTS_ENV);
    let dir = match script_dir(input.scripts, named.as_deref()) {
        Ok(d) => d,
        Err(reason) => return row(127, &reason, None),
    };
    let script = dir.join(SCRIPT);
    // rover's (redacted) JSON goes to a file in a directory this layer
    // creates, read and removed here.
    let scratch = match private_dir() {
        Ok(d) => d,
        Err(e) => return row(1, &format!("supergraph_check: FAIL — {}", e), None),
    };
    let json_file = scratch.join("check.json");
    let mut command = Command::new("bash");
    command
        .arg(&script)
        .arg(input.dir)
        .env("NO_COLOR", "1")
        .env(JSON_ENV, &json_file)
        .env(graphos_factory_core::env::name("SCRIPTS"), input.scripts)
        .stdin(Stdio::null());
    if graphos_factory_core::env::is_unset("BIN") {
        if let Ok(exe) = std::env::current_exe() {
            command.env(graphos_factory_core::env::name("BIN"), exe);
        }
    }
    let (code, output) = match command.output() {
        Ok(o) => (
            o.status.code().unwrap_or(1),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        ),
        Err(e) => (
            1,
            format!("supergraph_check: bash {}: {}\n", script.display(), e),
        ),
    };
    let rover_json = std::fs::read_to_string(&json_file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let _ = std::fs::remove_dir_all(&scratch);
    let mut out = row(code, &output, rover_json.as_ref());
    if let Some(o) = out.as_object_mut() {
        o.insert(
            "command".into(),
            Value::from(format!("{} {}", SCRIPT, input.dir.display())),
        );
    }
    out
}

fn status_for(code: i32) -> &'static str {
    match code {
        0 => "pass",
        3 => "not_run",
        127 => "skipped",
        _ => "fail",
    }
}

/// The row for a run that exited `code` and printed `output`, with rover's
/// JSON when the script got that far.
pub fn row(code: i32, output: &str, rover: Option<&Value>) -> Value {
    let lines: Vec<&str> = output.lines().collect();
    let status = status_for(code);
    let mut r = Map::new();
    r.insert("status".into(), Value::from(status));
    r.insert("exit_code".into(), Value::from(code));

    let findings: Vec<Value> = if status == "fail" {
        lines
            .iter()
            .filter(|l| l.starts_with("  "))
            .map(|l| Value::from(l.trim()))
            .take(MAX_FINDINGS)
            .collect()
    } else {
        Vec::new()
    };
    r.insert("findings".into(), Value::Array(findings));

    if status != "pass" {
        let find = |re: &str| {
            let re = Regex::new(re).unwrap();
            lines
                .iter()
                .find(|l| re.is_match(l))
                .map(|l| l.trim().to_string())
        };
        let reason = match status {
            "not_run" => find(r"not_run|not set").unwrap_or_else(|| "layer did not run".into()),
            "skipped" => find(r"not installed|not found").unwrap_or_else(|| "tool missing".into()),
            _ => find(r"^supergraph_check: FAIL")
                .or_else(|| find(r"FAIL"))
                .unwrap_or_else(|| format!("exit {}", code)),
        };
        r.insert("reason".into(), Value::from(reason));
    }

    let head = Regex::new(
        r"^supergraph_check: rover (\S+), subgraph (\S+), graph (\S+), mode (explicit|auto)$",
    )
    .unwrap();
    let head = lines.iter().find_map(|l| head.captures(l));
    if head.is_some() || rover.is_some() {
        let mut d = Map::new();
        let cap = |i: usize| {
            head.as_ref()
                .and_then(|c| c.get(i))
                .map_or(Value::Null, |m| Value::from(m.as_str()))
        };
        d.insert("graph_ref".into(), cap(3));
        d.insert("mode".into(), cap(4));
        d.insert("rover".into(), cap(1));
        d.insert("subgraph".into(), cap(2));
        if let Some(j) = rover {
            let build_errors = get(j, "error")
                .and_then(|e| get(e, "details"))
                .and_then(|e| get_arr(e, "build_errors"))
                .map_or(0, Vec::len);
            let code = get(j, "error").and_then(|e| get_str(e, "code"));
            let tasks = get(j, "data").and_then(|d| get_obj(d, "tasks"));
            let composition = if build_errors > 0 || code == Some("E029") {
                "fail"
            } else if tasks.is_some() {
                "pass"
            } else {
                "not_run"
            };
            d.insert("composition".into(), Value::from(composition));
            d.insert("build_errors".into(), Value::from(build_errors));
            let mut checks = Map::new();
            let mut url = Value::Null;
            for (name, task) in tasks.into_iter().flatten() {
                if let Some(s) = get_str(task, "task_status") {
                    checks.insert(name.clone(), Value::from(s));
                }
                if url.is_null() {
                    if let Some(u) = get_str(task, "target_url") {
                        url = Value::from(u);
                    }
                }
            }
            d.insert("checks".into(), Value::Object(checks));
            if let Some(ops) = tasks.and_then(|t| t.get("operations")) {
                d.insert(
                    "operations".into(),
                    json!({
                        "status": get_str(ops, "task_status"),
                        "checked": get(ops, "operation_check_count").cloned().unwrap_or(Value::Null),
                        "failing_changes": get(ops, "failure_count").cloned().unwrap_or(Value::Null),
                    }),
                );
                if let Some(u) = get_str(ops, "target_url") {
                    url = Value::from(u);
                }
            }
            d.insert("target_url".into(), url);
            if let Some(c) = code {
                d.insert("rover_error_code".into(), Value::from(c));
            }
        }
        r.insert("details".into(), Value::Object(d));
    }
    Value::Object(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_map_as_the_cores() {
        assert_eq!(row(0, "", None)["status"], "pass");
        assert_eq!(row(1, "", None)["status"], "fail");
        assert_eq!(row(78, "", None)["status"], "fail");
        assert_eq!(row(3, "", None)["status"], "not_run");
        assert_eq!(row(127, "", None)["status"], "skipped");
        assert!(row(0, "", None).get("reason").is_none());
    }

    #[test]
    fn the_scratch_directory_is_new_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let a = private_dir().unwrap();
        let b = private_dir().unwrap();
        assert_ne!(a, b);
        let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{:o}", mode);
        std::fs::remove_dir(&a).unwrap();
        std::fs::remove_dir(&b).unwrap();
    }
}

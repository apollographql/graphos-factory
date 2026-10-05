//! `init` end to end: a new spec-backed workspace from one description
//! document, exactly as `sources pin` and `inventory build` would leave it; a
//! dry run that writes nothing and reports the bytes a real run writes; and
//! every refusal (an existing workspace, a bad document, a bad name, a bad
//! flag) exits non-zero with nothing written.

use graphos_factory_core::cmd::init::names;
use graphos_factory_core::json::{get_arr, get_str};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const OPENAPI: &str = r##"{
  "openapi": "3.0.3",
  "info": {"title": "Widget Co", "version": "1.0.0"},
  "servers": [{"url": "https://api.widgets.test"}],
  "components": {"schemas": {
    "Widget": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}, "name": {"type": "string"}}}
  }},
  "paths": {
    "/widgets": {"get": {"operationId": "listWidgets",
      "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
        "type": "object", "properties": {"widgets": {"type": "array", "items": {"$ref": "#/components/schemas/Widget"}}}}}}}}}},
    "/widgets/{id}": {"get": {"operationId": "getWidget",
      "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
      "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Widget"}}}}}}}
  }
}
"##;

const SWAGGER: &str = "swagger: \"2.0\"\ninfo:\n  title: Gadget\n  version: \"1\"\nhost: api.gadget.test\nbasePath: /v1\nschemes: [https]\npaths:\n  /gadgets:\n    get:\n      operationId: listGadgets\n      produces: [application/json]\n      responses:\n        \"200\":\n          description: ok\n          schema:\n            type: array\n            items:\n              type: object\n              properties:\n                id: { type: integer }\n";

const T: &str = "2026-09-24T12:00:00Z";

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
}

/// Run `init` with `--json`; (exit code, parsed stdout, stderr).
fn init_json(target: &Path, args: &[&str]) -> (i32, Value, String) {
    let out = bin()
        .arg("init")
        .arg(target)
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let json = graphos_factory_core::json::parse(&text)
        .unwrap_or_else(|e| panic!("stdout not JSON ({}): {:?}\nstderr: {}", e, text, stderr));
    (out.status.code().unwrap(), json, stderr)
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) -> String {
    let f = dir.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, bytes).unwrap();
    f.to_string_lossy().to_string()
}

/// Every file under `dir`, workspace-relative and sorted, with its bytes.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    if dir.exists() {
        walk(dir, dir, &mut out);
    }
    out.sort();
    out
}

fn files(report: &Value) -> Vec<(String, String, String)> {
    get_arr(report, "files")
        .unwrap()
        .iter()
        .map(|f| {
            (
                get_str(f, "path").unwrap().to_string(),
                get_str(f, "content").unwrap().to_string(),
                get_str(f, "sha256").unwrap().to_string(),
            )
        })
        .collect()
}

fn sha256(bytes: &[u8]) -> String {
    graphos_factory_core::patch::bytes_sha256(bytes)
}

#[test]
fn names_derive_the_four_names_of_a_service() {
    let n = names("widget-co").unwrap();
    assert_eq!(n.service, "widget_co");
    assert_eq!(n.directory, "widget-co");
    assert_eq!(n.type_prefix, "Widget_Co");
    assert_eq!(n.field_prefix, "widget_co");
    assert_eq!(
        names("widget_co").unwrap(),
        n,
        "snake and kebab are one name"
    );
    let d = names("deck_of_cards").unwrap();
    assert_eq!(
        (d.directory.as_str(), d.type_prefix.as_str()),
        ("deck-of-cards", "Deck_Of_Cards")
    );
    let p = names("pagerduty").unwrap();
    assert_eq!(
        (p.service.as_str(), p.type_prefix.as_str()),
        ("pagerduty", "Pagerduty")
    );
    assert_eq!(names("web3_api").unwrap().type_prefix, "Web3_Api");
    for bad in [
        "",
        "WidgetCo",
        "widgetCo",
        "widget-co_x",
        "3d",
        "widget__co",
        "widget-",
        "_widget",
        "widget co",
        "widget.co",
        "widget_3d",
        "wídget",
    ] {
        assert!(names(bad).is_err(), "{:?} must be refused", bad);
    }
}

#[test]
fn init_creates_the_workspace_pin_and_inventory_would() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = write(tmp.path(), "download/widgets.json", OPENAPI.as_bytes());
    let target = tmp.path().join("widget-co");
    let (code, report, stderr) = init_json(
        &target,
        &[
            "--name",
            "widget-co",
            "--spec",
            &spec,
            "--url",
            "https://api.widgets.test/openapi.json",
            "--retrieved-at",
            T,
            "--created-at",
            T,
            "--context-mode",
            "generic",
        ],
    );
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert_eq!(report["dry_run"], Value::Bool(false));
    assert_eq!(report["exit"], Value::from(0));
    assert_eq!(get_str(&report, "service"), Some("widget_co"));
    assert_eq!(get_str(&report, "directory"), Some("widget-co"));
    assert_eq!(get_str(&report, "type_prefix"), Some("Widget_Co"));
    assert_eq!(get_str(&report, "context_mode"), Some("generic"));
    assert_eq!(get_str(&report, "path"), Some("openapi.json"));
    assert_eq!(get_str(&report, "kind"), Some("openapi"));
    assert_eq!(get_str(&report, "version"), Some("3.0.3"));
    assert_eq!(
        get_str(&report, "upstream"),
        Some(".factory/sources/openapi.upstream.json")
    );
    assert_eq!(report["inventory"]["operations"], Value::from(2));
    assert_eq!(report["inventory"]["supported"], Value::from(2));
    assert_eq!(get_str(&report["inventory"], "title"), Some("Widget Co"));

    // The files, in write order, and exactly what is on disk — nothing else.
    let listed = files(&report);
    let paths: Vec<&str> = listed.iter().map(|(p, _, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            ".factory/workspace.yaml",
            "openapi.json",
            ".factory/sources/openapi.upstream.json",
            ".factory/sources.lock.yaml",
            ".factory/inventory.json",
        ]
    );
    let on_disk = tree(&target);
    assert_eq!(
        on_disk.len(),
        listed.len(),
        "init writes only what it lists"
    );
    for (path, content, sum) in &listed {
        let bytes = std::fs::read(target.join(path)).unwrap();
        assert_eq!(&bytes, content.as_bytes(), "{} content", path);
        assert_eq!(sum, &sha256(&bytes), "{} sha256", path);
    }
    assert!(!target.join(".git").exists(), "init never runs git");

    // Both copies are the vendor's bytes, verbatim.
    assert_eq!(
        std::fs::read(target.join("openapi.json")).unwrap(),
        OPENAPI.as_bytes()
    );
    assert_eq!(
        get_str(&report, "upstream_sha256"),
        Some(sha256(OPENAPI.as_bytes()).as_str())
    );

    // workspace.yaml satisfies its schema and records the caller's calls.
    let ws =
        graphos_factory_core::yaml::parse_file(&target.join(".factory/workspace.yaml")).unwrap();
    let schema = graphos_factory_core::schemas::load("workspace.schema.json", None).unwrap();
    assert_eq!(
        graphos_factory_core::jsonschema::validate(&ws, &schema),
        Vec::<String>::new()
    );
    assert_eq!(get_str(&ws, "intake"), Some("spec"));
    assert_eq!(get_str(&ws, "source_kind"), Some("rest"));
    assert_eq!(get_str(&ws, "created_at"), Some(T));
    assert_eq!(get_str(&ws, "context_mode"), Some("generic"));
    assert_eq!(
        get_str(&ws, "federation_version"),
        Some(graphos_factory_core::cmd::init::FEDERATION_VERSION)
    );

    // sources.lock.yaml satisfies its schema and pins what `sources pin` pins.
    let lock =
        graphos_factory_core::yaml::parse_file(&target.join(".factory/sources.lock.yaml")).unwrap();
    let schema = graphos_factory_core::schemas::load("sources-lock.schema.json", None).unwrap();
    assert_eq!(
        graphos_factory_core::jsonschema::validate(&lock, &schema),
        Vec::<String>::new()
    );
    let entry = &get_arr(&lock, "sources").unwrap()[0];
    assert_eq!(
        get_str(entry, "url"),
        Some("https://api.widgets.test/openapi.json")
    );
    assert_eq!(get_str(entry, "retrieved_at"), Some(T));
    assert_eq!(get_str(entry, "path"), Some("openapi.json"));

    // A hand-run `sources pin` of the same document writes the same lock
    // and the same vendor copy, byte for byte.
    let by_hand = tmp.path().join("by-hand");
    write(&by_hand, "openapi.json", OPENAPI.as_bytes());
    let out = bin()
        .args(["sources", "pin"])
        .arg(&by_hand)
        .args([
            "--path",
            "openapi.json",
            "--url",
            "https://api.widgets.test/openapi.json",
            "--retrieved-at",
            T,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for rel in [
        ".factory/sources.lock.yaml",
        ".factory/sources/openapi.upstream.json",
    ] {
        assert_eq!(
            std::fs::read(by_hand.join(rel)).unwrap(),
            std::fs::read(target.join(rel)).unwrap(),
            "{} as `sources pin` writes it",
            rel
        );
    }

    // `inventory build` on the working copy reproduces the inventory byte
    // for byte — the check CI makes on every spec-backed pilot.
    let rebuilt = tmp.path().join("rebuilt.json");
    let out = bin()
        .args(["inventory", "build", "openapi.json", "--out"])
        .arg(&rebuilt)
        .current_dir(&target)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(&rebuilt).unwrap(),
        std::fs::read(target.join(".factory/inventory.json")).unwrap()
    );

    // `sources status` reads the pin as intact: kind, upstream, no hand edit.
    let out = bin()
        .args(["sources", "status"])
        .arg(&target)
        .arg("--json")
        .output()
        .unwrap();
    let status =
        graphos_factory_core::json::parse(&String::from_utf8(out.stdout).unwrap()).unwrap();
    let s = &get_arr(&status, "sources").unwrap()[0];
    assert_eq!(get_str(s, "path"), Some("openapi.json"));
    assert_eq!(s["upstream_ok"], Value::Bool(true), "{}", status);
    assert_eq!(s["hand_edit"], Value::Bool(false), "{}", status);

    // A generic workspace is build-ready without a context.yaml.
    let out = bin()
        .args(["context", "check"])
        .arg(&target)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn dry_run_writes_nothing_and_matches_a_real_run_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = write(tmp.path(), "widgets.json", OPENAPI.as_bytes());
    let target = tmp.path().join("svc");
    let args = [
        "--name",
        "widget_co",
        "--spec",
        spec.as_str(),
        "--retrieved-at",
        T,
        "--created-at",
        T,
    ];
    let mut dry_args = args.to_vec();
    dry_args.push("--dry-run");
    let (code, dry, stderr) = init_json(&target, &dry_args);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert_eq!(dry["dry_run"], Value::Bool(true));
    assert!(
        !target.exists(),
        "a dry run does not even create the directory"
    );
    assert_eq!(dry["context_mode"], Value::Null, "not given, not recorded");

    // An existing, empty target stays empty too.
    std::fs::create_dir_all(&target).unwrap();
    let (code, again, _) = init_json(&target, &dry_args);
    assert_eq!(code, 0);
    assert!(tree(&target).is_empty());
    assert_eq!(files(&again), files(&dry));

    let (code, real, stderr) = init_json(&target, &args);
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert_eq!(files(&real), files(&dry), "same paths, contents and hashes");
    let on_disk: Vec<(String, Vec<u8>)> = tree(&target);
    let mut planned: Vec<(String, Vec<u8>)> = files(&dry)
        .into_iter()
        .map(|(p, c, _)| (p, c.into_bytes()))
        .collect();
    planned.sort();
    assert_eq!(
        on_disk, planned,
        "the dry run's bytes are the real run's bytes"
    );
    // Every other key agrees too.
    let strip = |mut v: Value| {
        v.as_object_mut().unwrap().remove("dry_run");
        v
    };
    assert_eq!(strip(dry), strip(real));

    // Without context_mode, init records nothing and context check reads the
    // workspace as generic (ADR 0081).
    let ws = std::fs::read_to_string(target.join(".factory/workspace.yaml")).unwrap();
    assert!(!ws.contains("context_mode"), "{}", ws);
    let out = bin()
        .args(["context", "check"])
        .arg(&target)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("not recorded; read as generic"));
}

#[test]
fn a_swagger_yaml_document_becomes_a_swagger_yaml_working_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = write(tmp.path(), "gadget.yml", SWAGGER.as_bytes());
    let target = tmp.path().join("gadget");
    let (code, report, stderr) = init_json(
        &target,
        &[
            "--name",
            "gadget",
            "--spec",
            &spec,
            "--context-mode",
            "specialized",
        ],
    );
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert_eq!(get_str(&report, "kind"), Some("swagger"));
    assert_eq!(get_str(&report, "version"), Some("2.0"));
    assert_eq!(get_str(&report, "path"), Some("swagger.yml"));
    assert_eq!(
        get_str(&report, "upstream"),
        Some(".factory/sources/swagger.upstream.yml")
    );
    assert_eq!(
        std::fs::read(target.join("swagger.yml")).unwrap(),
        SWAGGER.as_bytes()
    );
    let ws =
        graphos_factory_core::yaml::parse_file(&target.join(".factory/workspace.yaml")).unwrap();
    assert_eq!(get_str(&ws, "context_mode"), Some("specialized"));
    let rebuilt = tmp.path().join("rebuilt.json");
    let out = bin()
        .args(["inventory", "build", "swagger.yml", "--out"])
        .arg(&rebuilt)
        .current_dir(&target)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        std::fs::read(&rebuilt).unwrap(),
        std::fs::read(target.join(".factory/inventory.json")).unwrap()
    );
}

#[test]
fn inside_a_repository_init_makes_a_folder_and_never_nests_a_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("artifacts");
    std::fs::create_dir_all(&repo).unwrap();
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("git unavailable; skipping");
        return;
    }
    let spec = write(tmp.path(), "widgets.json", OPENAPI.as_bytes());
    let (code, _, stderr) = init_json(
        &repo.join("widget-co"),
        &["--name", "widget-co", "--spec", &spec],
    );
    assert_eq!(code, 0, "stderr: {}", stderr);
    assert!(!repo.join("widget-co/.git").exists());
    assert!(repo.join("widget-co/.factory/workspace.yaml").is_file());
    // Nothing was staged or committed in the enclosing repository.
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "?? widget-co/");
}

#[test]
fn an_existing_workspace_or_file_is_refused_and_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = write(tmp.path(), "widgets.json", OPENAPI.as_bytes());

    for marker in [
        ".factory/workspace.yaml",
        ".factory/selection.yaml",
        "workspace.yaml",
    ] {
        let target = tmp
            .path()
            .join(format!("ws-{}", marker.replace(['/', '.'], "_")));
        write(&target, marker, b"contract_version: 1\n");
        let before = tree(&target);
        let (code, report, _) = init_json(&target, &["--name", "widget", "--spec", &spec]);
        assert_eq!(code, 2, "{}", marker);
        assert_eq!(
            get_str(&report, "code"),
            Some("workspace-exists"),
            "{}",
            marker
        );
        assert_eq!(report["exit"], Value::from(2));
        assert!(get_str(&report, "error")
            .unwrap()
            .contains("already holds a workspace"));
        assert_eq!(tree(&target), before, "{} untouched", marker);
    }

    // A working copy already at the path init would create is not adopted.
    let target = tmp.path().join("has-spec");
    write(&target, "openapi.json", b"{\"openapi\": \"3.0.0\"}");
    let (code, report, _) = init_json(&target, &["--name", "widget", "--spec", &spec]);
    assert_eq!(code, 2);
    assert_eq!(get_str(&report, "code"), Some("file-exists"));
    assert!(!target.join(".factory").exists());

    // A target that is a file.
    let file = tmp.path().join("a-file");
    std::fs::write(&file, "x").unwrap();
    let (code, report, _) = init_json(&file, &["--name", "widget", "--spec", &spec]);
    assert_eq!(
        (code, get_str(&report, "code")),
        (2, Some("not-a-directory"))
    );
}

#[test]
fn a_bad_document_is_refused_with_nothing_written() {
    let tmp = tempfile::tempdir().unwrap();
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("missing.json", vec![], "spec-unreadable"),
        (
            "latin1.yaml",
            b"openapi: 3.0.0\ninfo: {title: caf\xe9}\n".to_vec(),
            "spec-unreadable",
        ),
        (
            "broken.json",
            b"{\"openapi\": \"3.0.0\",".to_vec(),
            "spec-invalid",
        ),
        ("list.yaml", b"- a\n- b\n".to_vec(), "spec-invalid"),
        (
            "plain.json",
            b"{\"hello\": \"world\"}".to_vec(),
            "spec-unsupported",
        ),
        (
            "old.json",
            b"{\"swagger\": \"1.2\", \"apis\": []}".to_vec(),
            "spec-unsupported",
        ),
        (
            "two.yaml",
            b"openapi: 2.0.0\npaths: {}\n".to_vec(),
            "spec-unsupported",
        ),
    ];
    for (name, bytes, want) in cases {
        let spec = if name == "missing.json" {
            tmp.path().join(name).to_string_lossy().to_string()
        } else {
            write(tmp.path(), name, &bytes)
        };
        let target = tmp.path().join(format!("ws-{}", name));
        for dry in [false, true] {
            let mut args = vec!["--name", "widget", "--spec", spec.as_str()];
            if dry {
                args.push("--dry-run");
            }
            let (code, report, stderr) = init_json(&target, &args);
            assert_eq!(code, 2, "{}: {} / {}", name, report, stderr);
            assert_eq!(get_str(&report, "code"), Some(want), "{}: {}", name, report);
            assert!(stderr.starts_with("init: "), "{}: {}", name, stderr);
            assert!(!target.exists(), "{}: nothing written", name);
        }
    }
}

#[test]
fn a_bad_name_or_flag_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = write(tmp.path(), "widgets.json", OPENAPI.as_bytes());
    let target = tmp.path().join("ws");
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["--name", "WidgetCo", "--spec", &spec], "invalid-name"),
        (
            vec!["--name", "widget-co_x", "--spec", &spec],
            "invalid-name",
        ),
        (vec!["--name", "3d", "--spec", &spec], "invalid-name"),
        (vec!["--spec", &spec], "usage"),
        (vec!["--name", "widget"], "usage"),
        (
            vec![
                "--name",
                "widget",
                "--spec",
                &spec,
                "--context-mode",
                "tenant",
            ],
            "usage",
        ),
        (
            vec![
                "--name",
                "widget",
                "--spec",
                &spec,
                "--created-at",
                "yesterday",
            ],
            "usage",
        ),
        (
            vec![
                "--name",
                "widget",
                "--spec",
                &spec,
                "--created-at",
                "2026-09-24T00:00:00Z\ncontext_mode: generic",
            ],
            "usage",
        ),
    ];
    for (args, want) in cases {
        let (code, report, _) = init_json(&target, &args);
        assert_eq!(code, 1, "{:?}: {}", args, report);
        assert_eq!(
            get_str(&report, "code"),
            Some(want),
            "{:?}: {}",
            args,
            report
        );
        assert!(!target.exists(), "{:?}: nothing written", args);
    }
    // An unknown flag never reaches init: dispatch refuses it, exit 2,
    // with the same JSON failure shape (ADR 0097).
    let args = [
        "--name",
        "widget",
        "--spec",
        &spec,
        "--context_mode",
        "generic",
    ];
    let (code, report, stderr) = init_json(&target, &args);
    assert_eq!(code, 2, "{}", stderr);
    assert_eq!(get_str(&report, "code"), Some("unknown-flag"), "{}", report);
    assert_eq!(report.get("exit").and_then(|e| e.as_i64()), Some(2));
    assert!(stderr.contains("--context_mode"), "{}", stderr);
    assert!(!target.exists(), "nothing written");
}

#[test]
fn text_mode_reports_the_files_and_leaves_git_to_the_caller() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = write(tmp.path(), "widgets.json", OPENAPI.as_bytes());
    let target = tmp.path().join("ws");
    let out = bin()
        .arg("init")
        .arg(&target)
        .args(["--name", "widget", "--spec", &spec, "--dry-run"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("would create workspace widget"), "{}", text);
    assert!(
        text.contains(".factory/inventory.json  (2 operations"),
        "{}",
        text
    );
    assert!(text.contains("nothing written (--dry-run)"), "{}", text);
    assert!(!target.exists());

    let out = bin().args(["init", "--help"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--dry-run"));
}

/// A new workspace composes with the toolchain CI and the session hook
/// install: init's pin is toolchain.sh's.
#[test]
fn the_federation_pin_is_the_toolchain_pin() {
    let script = Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR")).join("toolchain.sh");
    let text = std::fs::read_to_string(script).unwrap();
    let pin = text
        .lines()
        .find_map(|l| l.strip_prefix("FEDERATION_VERSION=\"${FEDERATION_VERSION:-"))
        .and_then(|rest| rest.strip_suffix("}\""))
        .expect("toolchain.sh pins FEDERATION_VERSION");
    assert_eq!(pin, graphos_factory_core::cmd::init::FEDERATION_VERSION);
}

/// `init` writes what the `workspace.yaml` template in workspace-contract.md
/// says a new workspace records: its `connect_spec` and `federation_version`,
/// and no `federation_spec_version` (the template leaves it commented out).
#[test]
fn the_workspace_pins_are_the_template_pins() {
    let doc =
        Path::new(env!("GRAPHOS_FACTORY_CORE_SKILL_DIR")).join("references/workspace-contract.md");
    let text = std::fs::read_to_string(doc).unwrap();
    let template = text
        .split("## `workspace.yaml`")
        .nth(1)
        .and_then(|rest| rest.split("```yaml\n").nth(1))
        .and_then(|rest| rest.split("```").next())
        .expect("workspace-contract.md has a workspace.yaml template");
    let value = |key: &str| {
        template
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{}: ", key)))
            .map(|rest| rest.split('#').next().unwrap().trim().trim_matches('"'))
            .unwrap_or_else(|| panic!("the template sets {}", key))
            .to_string()
    };
    assert_eq!(
        value("connect_spec"),
        graphos_factory_core::cmd::init::CONNECT_SPEC
    );
    assert_eq!(
        value("federation_version"),
        graphos_factory_core::cmd::init::FEDERATION_VERSION
    );
    assert!(
        !template
            .lines()
            .any(|l| l.starts_with("federation_spec_version:")),
        "the template now sets federation_spec_version; init must write it too"
    );

    let dir = tempfile::tempdir().unwrap();
    let spec = dir.path().join("spec.json");
    std::fs::write(&spec, OPENAPI).unwrap();
    let target = dir.path().join("widget");
    let out = bin()
        .args(["init", target.to_str().unwrap(), "--name", "widget"])
        .args(["--spec", spec.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{:?}", out);
    let written = std::fs::read_to_string(target.join(".factory/workspace.yaml")).unwrap();
    assert!(written.contains(&format!("connect_spec: {}\n", value("connect_spec"))));
    assert!(written.contains(&format!(
        "federation_version: \"{}\"\n",
        value("federation_version")
    )));
    assert!(!written.contains("federation_spec_version"), "{}", written);
}

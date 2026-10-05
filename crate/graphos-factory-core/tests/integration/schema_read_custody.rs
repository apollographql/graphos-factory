//! Phase 7az / ADR 0025: a hand-edited `workspace.yaml` naming a `directory`
//! that resolves back inside `.factory/` (or outside the workspace) must
//! never make `lock`, `reconcile` or `lint` read whatever that path happens
//! to be a symlink to. Before this fix, `schema_file_of` built
//! `<directory>.graphql` and every caller read it with a plain
//! `std::fs::read_to_string`, bypassing `factory_io`'s custody entirely.

use graphos_factory_core::lint::{lint_workspace, LintOptions};
use std::path::Path;

const MARKER: &str = "SECRET_MARKER_OUTSIDE_THE_WORKSPACE_0f3c9a";

/// A workspace whose `directory` points at `.factory/probe`, with
/// `.factory/probe.graphql` symlinked to a file outside the workspace that
/// carries a distinctive marker. Reading the "schema" through the old,
/// unguarded path would return the marker text.
fn poisoned_workspace() -> (tempfile::TempDir, tempfile::TempDir) {
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("not-a-schema.txt");
    std::fs::write(&secret, MARKER).unwrap();

    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".factory")).unwrap();
    std::fs::write(
        ws.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: .factory/probe\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\ncontext_mode: generic\n",
    )
    .unwrap();
    std::fs::write(
        ws.path().join(".factory/selection.yaml"),
        "contract_version: 1\ndefaults:\n  fields: all\n  max_depth: 6\n  opaque_json_policy: forbid\noperations: {}\n",
    )
    .unwrap();
    std::fs::write(
        ws.path().join(".factory/inventory.json"),
        r#"{"contract_version":1,"api":{"title":"Widget Co","base_urls":["https://api.widgets.test"]},"operations":[],"shapes":{},"unresolved":[]}"#,
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, ws.path().join(".factory/probe.graphql")).unwrap();
    (ws, outside)
}

#[cfg(unix)]
#[test]
fn lock_load_refuses_a_directory_that_points_into_factory() {
    let (ws, _outside) = poisoned_workspace();
    let err = match graphos_factory_core::cmd::lock::load(ws.path()) {
        Err(e) => e,
        Ok(_) => panic!("expected the poisoned directory to be refused"),
    };
    assert!(
        !err.contains(MARKER),
        "the marker from outside the workspace must never appear in the error: {err}"
    );
}

#[cfg(unix)]
#[test]
fn reconcile_workspace_refuses_a_directory_that_points_into_factory() {
    let (ws, _outside) = poisoned_workspace();
    let err = graphos_factory_core::reconcile::reconcile_workspace(ws.path(), None).unwrap_err();
    assert!(
        !err.contains(MARKER),
        "the marker from outside the workspace must never appear in the error: {err}"
    );
}

#[cfg(unix)]
#[test]
fn lint_workspace_refuses_a_directory_that_points_into_factory() {
    let (ws, _outside) = poisoned_workspace();
    let result = lint_workspace(
        ws.path(),
        &LintOptions {
            schemas_dir: None,
            skip_evidence: true,
            target: &graphos_factory_core::target::BARE,
        },
    );
    // Specifically the refusal, not incidentally some other error: before
    // the fix the symlink target "exists" (it is a real file, just outside
    // the workspace), so the old `missing-schema` check never fired and the
    // marker text was read as the schema's SDL instead.
    assert!(
        result.findings.iter().any(|f| f.rule == "missing-schema"),
        "expected a missing-schema finding (the custody refusal), got: {:?}",
        result.findings
    );
    let rendered = format!("{:?}", result.findings);
    assert!(
        !rendered.contains(MARKER),
        "the marker from outside the workspace must never appear in a finding: {rendered}"
    );
}

/// A `directory` that walks out of the workspace with `..` is caught by the
/// same pattern check, independent of whether the resulting path happens to
/// cross `.factory/` at all.
#[test]
fn schema_file_of_refuses_a_directory_outside_the_pattern() {
    let cases = [
        ".factory/probe",
        "../../etc/passwd",
        "",
        "Weird_Case",
        "trailing-",
    ];
    for directory in cases {
        let workspace = serde_json::json!({ "directory": directory });
        assert!(
            graphos_factory_core::reconcile::schema_file_of(&workspace).is_err(),
            "{directory:?} should not pass workspace.schema.json's directory pattern"
        );
    }
}

#[test]
fn schema_file_of_still_accepts_an_ordinary_directory() {
    let workspace = serde_json::json!({ "directory": "widget-co" });
    assert_eq!(
        graphos_factory_core::reconcile::schema_file_of(&workspace).unwrap(),
        "widget-co.graphql"
    );
}

/// B1 (Codex's review of #116): `validate` and `render` each built
/// `<directory>.graphql` and read it with a plain `std::fs::read_to_string`,
/// independently of the fix above -- neither goes through `cmd::lock::load`
/// or shares a call site with `reconcile`/`lint`. `render` additionally
/// names its *output* file from the same unchecked `directory`, so an
/// absolute `directory` could overwrite a file outside `--out` entirely,
/// not just read one.
///
/// `validate`'s `sdl` only feeds `OpHints`' entity-type disambiguation
/// (`crate::op_match`), consulted only for a path several operations match
/// equally. Checked empirically against this minimal fixture and a full
/// `pilots/gitea` copy with only `directory` and the schema symlink changed
/// (Codex's own richer-probe technique): the marker never reaches
/// `report.json` in either case, refused or not -- there is no observable
/// content-based difference through this command's output for a realistic
/// workspace. A FIFO in place of an ordinary secret file proves the
/// underlying fact instead: a blocking read on a FIFO with no writer
/// returns only if something opens it, so an unfixed raw
/// `std::fs::read_to_string` hangs here, and the fixed, validated,
/// refused-before-any-open path returns immediately.
#[cfg(unix)]
#[test]
fn validate_workspace_refuses_a_directory_that_points_into_factory() {
    let outside = tempfile::tempdir().unwrap();
    let fifo = outside.path().join("schema.fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success(),
        "mkfifo must be available to build this probe"
    );

    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".factory")).unwrap();
    std::fs::write(
        ws.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: .factory/probe\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\ncontext_mode: generic\n",
    )
    .unwrap();
    std::fs::write(
        ws.path().join(".factory/selection.yaml"),
        "contract_version: 1\ndefaults:\n  fields: all\n  max_depth: 6\n  opaque_json_policy: forbid\noperations: {}\n",
    )
    .unwrap();
    std::fs::write(
        ws.path().join(".factory/inventory.json"),
        r#"{"contract_version":1,"api":{"title":"Widget Co","base_urls":["https://api.widgets.test"]},"operations":[],"shapes":{},"unresolved":[]}"#,
    )
    .unwrap();
    std::os::unix::fs::symlink(&fifo, ws.path().join(".factory/probe.graphql")).unwrap();

    let dir = ws.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = graphos_factory_core::cmd::validate::validate_workspace(&dir);
        let _ = tx.send(result.is_ok());
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(3)).is_ok(),
        "validate_workspace hung reading the outside FIFO -- it opened a path custody must refuse before any open() at all"
    );
}

#[cfg(unix)]
#[test]
fn render_refuses_a_directory_that_points_into_factory() {
    let (ws, _outside) = poisoned_workspace();
    let out = tempfile::tempdir().unwrap();
    // Pre-create the nested output directory the vulnerable code's
    // `out.join(".factory/probe.rendered.graphql")` needs: without it,
    // `std::fs::write` fails on a missing parent regardless of whether the
    // read was ever refused, which would make this test pass for the wrong
    // reason (verified: it did, against the pre-fix source, before this
    // directory was added).
    std::fs::create_dir_all(out.path().join(".factory")).unwrap();
    let argv = vec![
        ws.path().to_string_lossy().to_string(),
        "--out".to_string(),
        out.path().to_string_lossy().to_string(),
    ];
    let code = graphos_factory_core::cmd::render::main(&argv);
    assert_ne!(
        code, 0,
        "render must refuse a poisoned directory, not silently render it"
    );
    for entry in walk(out.path()) {
        let content = std::fs::read_to_string(&entry).unwrap_or_default();
        assert!(
            !content.contains(MARKER),
            "{entry:?} carries the outside marker; the read must never have happened"
        );
    }
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// Codex's P2: an absolute `directory` made `render`'s *output* filename
/// (`out.join(format!("{directory}.rendered.graphql"))`) itself absolute --
/// `PathBuf::join` with an absolute operand discards the base entirely --
/// so `--out` a safe directory did not save the outside file it named.
#[cfg(unix)]
#[test]
fn render_refuses_an_absolute_directory_and_never_touches_its_target() {
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("render-victim.rendered.graphql");
    std::fs::write(&victim, "DO_NOT_OVERWRITE").unwrap();
    let directory = outside.path().join("render-victim");
    // The *input* side of the same bug: render's schema read is
    // `{directory}.graphql`, so without this file the read fails first and
    // the write path this test targets is never reached at all -- verified:
    // that is exactly what happened before this file was added.
    std::fs::write(
        outside.path().join("render-victim.graphql"),
        "type Query { ping: String }\n",
    )
    .unwrap();

    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".factory")).unwrap();
    std::fs::write(
        ws.path().join(".factory/workspace.yaml"),
        format!(
            "contract_version: 1\nservice: widget_co\ndirectory: \"{}\"\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\ncontext_mode: generic\n",
            directory.display()
        ),
    )
    .unwrap();
    std::fs::write(
        ws.path().join(".factory/selection.yaml"),
        "contract_version: 1\ndefaults:\n  fields: all\n  max_depth: 6\n  opaque_json_policy: forbid\noperations: {}\n",
    )
    .unwrap();

    let out = tempfile::tempdir().unwrap();
    let argv = vec![
        ws.path().to_string_lossy().to_string(),
        "--out".to_string(),
        out.path().to_string_lossy().to_string(),
    ];
    let code = graphos_factory_core::cmd::render::main(&argv);
    assert_ne!(code, 0, "render must refuse an absolute directory");
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "DO_NOT_OVERWRITE",
        "the outside file the absolute directory names must never be touched, safe --out or not"
    );
}

#[cfg(unix)]
#[test]
fn lock_load_still_works_for_a_normal_workspace() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".factory")).unwrap();
    std::fs::write(
        ws.path().join(".factory/workspace.yaml"),
        "contract_version: 1\nservice: widget_co\ndirectory: widget-co\ntype_prefix: Widget_Co\nfield_prefix: widget_co\nskill:\n  name: example\n  version: 0.1.0\nsource_kind: rest\nconnect_spec: v0.3\nfederation_version: \"2.12.0\"\nintake: spec\ncreated_at: 2026-09-08T00:00:00Z\ncontext_mode: generic\n",
    )
    .unwrap();
    std::fs::write(
        ws.path().join("widget-co.graphql"),
        "type Query { widget_co_ping: String }\n",
    )
    .unwrap();
    let loaded = graphos_factory_core::cmd::lock::load(ws.path()).unwrap();
    assert_eq!(loaded.schema_file, "widget-co.graphql");
    assert_eq!(loaded.sdl, "type Query { widget_co_ping: String }\n");
    let _ = Path::new(ws.path()); // keep `Path` import used across cfg(unix) variants
}

/// Codex's M2 finding (P19): `schema_file_of`'s pattern makes
/// `<directory>.graphql` always relative with no `/`, so for an ordinary
/// workspace root it can never resolve inside `.factory/` -- meaning
/// `read_named_path`, the second mechanism ADR 0075 adds alongside the
/// pattern check, is unreachable by every other test in this file, and
/// replacing it with a plain `std::fs::read` leaves all of them green. A
/// workspace whose own root sits below a literal `.factory` *ancestor*
/// directory closes that gap: `directory: widget-co` is perfectly valid,
/// but the ancestor path itself carries a `.factory` component the pattern
/// check never inspects, and `read_named_path`'s custody -- which finds the
/// rightmost `.factory` component anywhere in the path, not only an
/// immediate parent -- still refuses the symlink.
#[cfg(unix)]
#[test]
fn read_schema_file_still_refuses_a_symlink_below_a_literal_factory_ancestor() {
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("not-a-schema.txt");
    std::fs::write(&secret, MARKER).unwrap();

    let root = tempfile::tempdir().unwrap();
    let ws = root.path().join(".factory").join("nested-workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::os::unix::fs::symlink(&secret, ws.join("widget-co.graphql")).unwrap();

    let workspace = serde_json::json!({ "directory": "widget-co" });
    let err = graphos_factory_core::reconcile::read_schema_file(&ws, &workspace).unwrap_err();
    assert!(
        !err.contains(MARKER),
        "the marker from outside must never appear in the error: {err}"
    );
}

/// The positive control: the same nested-`.factory`-ancestor shape, but an
/// ordinary file rather than a symlink, still reads normally -- the second
/// mechanism does not turn every such workspace into a refusal, only a
/// symlinked one.
#[cfg(unix)]
#[test]
fn read_schema_file_still_works_below_a_literal_factory_ancestor_for_an_ordinary_file() {
    let root = tempfile::tempdir().unwrap();
    let ws = root.path().join(".factory").join("nested-workspace");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("widget-co.graphql"),
        "type Query { widget_co_ping: String }\n",
    )
    .unwrap();

    let workspace = serde_json::json!({ "directory": "widget-co" });
    let (schema_file, sdl) =
        graphos_factory_core::reconcile::read_schema_file(&ws, &workspace).unwrap();
    assert_eq!(schema_file, "widget-co.graphql");
    assert_eq!(sdl, "type Query { widget_co_ping: String }\n");
}

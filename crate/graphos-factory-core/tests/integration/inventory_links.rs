//! `inventory links` — every candidate_entity_link fact, flat, with its
//! path in the fields grammar and the operations returning its host shape
//! (ADR 0069). The fixture is built from an OpenAPI 3 document only because
//! that is the shortest way to get an inventory with nested facts.

use graphos_factory_core::cmd::inventory_links::{candidate_links, link_field_name, mark_selected};
use graphos_factory_core::openapi::build_inventory;
use serde_json::{json, Value};
use std::path::Path;

/// The path item of a canonical GET-by-id operation; the caller keys it
/// under the path it belongs to.
fn by_id(id: &str, op: &str, schema: &str) -> Value {
    json!({
        "get": {
            "operationId": op,
            "parameters": [{"name": id, "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{}", schema)}}}}}
        }
    })
}

/// Albums and songs: a by-id GET each, a bare-array song list whose items
/// carry `album_id`, and a `post:/songs` returning the same `Song`
/// component the by-id GET returns.
fn spec() -> Value {
    let mut s = json!({
        "openapi": "3.0.3",
        "info": {"title": "Music API", "version": "1.0.0"},
        "servers": [{"url": "https://api.music.test/v1"}],
        "components": {"schemas": {
            "Album": {"type": "object", "properties": {"id": {"type": "string"}, "title": {"type": "string"}}},
            "Song": {"type": "object", "properties": {"id": {"type": "string"}, "album_id": {"type": "string"}}}
        }},
        "paths": {
            "/songs": {
                "get": {
                    "operationId": "listSongs",
                    "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                        "type": "array",
                        "items": {"type": "object", "properties": {"id": {"type": "string"}, "album_id": {"type": "string"}}}
                    }}}}}
                },
                "post": {
                    "operationId": "createSong",
                    "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Song"}}}},
                    "responses": {"201": {"description": "created", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Song"}}}}}
                }
            }
        }
    });
    s["paths"]["/albums/{albumId}"] = by_id("albumId", "getAlbum", "Album");
    s["paths"]["/songs/{songId}"] = by_id("songId", "getSong", "Song");
    s
}

fn build(s: &Value) -> Value {
    build_inventory(s).unwrap().inventory
}

fn workspace(inventory: &Value, selection: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let factory = dir.path().join(".factory");
    std::fs::create_dir_all(&factory).unwrap();
    std::fs::write(
        factory.join("inventory.json"),
        graphos_factory_core::json::pretty(inventory),
    )
    .unwrap();
    if let Some(text) = selection {
        std::fs::write(factory.join("selection.yaml"), text).unwrap();
    }
    dir
}

fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .arg("inventory")
        .args(args)
        .arg(dir.to_str().unwrap())
        .output()
        .unwrap()
}

#[test]
fn candidate_links_lists_every_fact_with_its_path_and_the_operations_returning_the_shape() {
    let inv = build(&spec());
    let links = candidate_links(&inv);
    let rows: Vec<(String, String, String, bool, Vec<String>)> = links
        .iter()
        .map(|l| {
            (
                l.shape.clone(),
                l.path.clone(),
                l.operation.clone(),
                l.host_is_list_item,
                l.returned_by.clone(),
            )
        })
        .collect();
    // Song is returned by two operations (the POST and the by-id GET), and
    // the list's inline item shape by one; shapes come out in inventory
    // order, and a root-array item path leads with `[]>`.
    assert_eq!(
        rows,
        vec![
            (
                "Song".to_string(),
                "album_id".to_string(),
                "get:/albums/{albumId}".to_string(),
                false,
                vec!["post:/songs".to_string(), "get:/songs/{songId}".to_string()],
            ),
            (
                "ListSongsResponse".to_string(),
                "[]>album_id".to_string(),
                "get:/albums/{albumId}".to_string(),
                true,
                vec!["get:/songs".to_string()],
            ),
        ]
    );
    assert!(links
        .iter()
        .all(|l| l.parameter == "albumId" && !l.list_context));
    // No selection was read: nothing is known about the target's inclusion.
    assert!(links.iter().all(|l| l.target_selected.is_none()));

    // With a selection, the by-id operation's include flag is reported:
    // listed with `include: false`, it is not selected. (An operation the
    // selection does not list at all counts as not selected too; the
    // `--json` test below, whose selection lists only get:/songs, is the
    // case for that.)
    let mut links = candidate_links(&inv);
    let selection = graphos_factory_core::yaml::parse(
        "contract_version: 1\noperations:\n  \"get:/songs\": { include: true }\n  \"get:/albums/{albumId}\": { include: false }\n",
    )
    .unwrap();
    mark_selected(&mut links, &selection);
    assert!(links.iter().all(|l| l.target_selected == Some(false)));
    let selection = graphos_factory_core::yaml::parse(
        "contract_version: 1\noperations:\n  \"get:/albums/{albumId}\": { include: true }\n",
    )
    .unwrap();
    mark_selected(&mut links, &selection);
    assert!(links.iter().all(|l| l.target_selected == Some(true)));
}

#[test]
fn inventory_links_json_reports_target_selected_from_the_selection() {
    let inv = build(&spec());
    let dir = workspace(
        &inv,
        Some("contract_version: 1\noperations:\n  \"get:/songs\": { include: true }\n"),
    );
    let out = run(dir.path(), &["links", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        report["candidate_links"],
        json!([
            {
                "shape": "Song",
                "path": "album_id",
                "operation": "get:/albums/{albumId}",
                "parameter": "albumId",
                "list_context": false,
                "host_is_list_item": false,
                "returned_by": ["post:/songs", "get:/songs/{songId}"],
                "target_selected": false
            },
            {
                "shape": "ListSongsResponse",
                "path": "[]>album_id",
                "operation": "get:/albums/{albumId}",
                "parameter": "albumId",
                "list_context": false,
                "host_is_list_item": true,
                "returned_by": ["get:/songs"],
                "target_selected": false
            }
        ])
    );

    // The text form: one line per fact, both annotations spelled out.
    let out = run(dir.path(), &["links"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("ListSongsResponse > []>album_id -> get:/albums/{albumId} (albumId) (list item) (target is not selected)\n"),
        "{}",
        stdout
    );
    assert!(
        stdout.contains(
            "Song > album_id -> get:/albums/{albumId} (albumId) (target is not selected)\n"
        ),
        "{}",
        stdout
    );
    assert!(
        stdout.contains("    returned by: post:/songs, get:/songs/{songId}\n"),
        "{}",
        stdout
    );
    assert!(
        stdout.contains("2 candidate links across 2 shapes\n"),
        "{}",
        stdout
    );
}

#[test]
fn inventory_links_without_an_inventory_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["links"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("no inventory at"), "{}", stderr);
    assert!(
        stderr.contains("run `inventory build <document>` first"),
        "{}",
        stderr
    );
}

#[test]
fn a_dotdot_workspace_argument_still_finds_the_selection_but_a_dotdot_inventory_path_is_refused() {
    // ADR 0069 fix round 1, finding 2: a `..` component must never make a
    // selection that exists look absent.
    let inv = build(&spec());
    let dir = workspace(
        &inv,
        Some("contract_version: 1\noperations:\n  \"get:/songs\": { include: true }\n"),
    );
    let name = dir.path().file_name().unwrap();
    // The same directory, spelled by descending into it, back out, and back
    // in again: sound to the OS (the parent exists, trivially, because
    // `dir.path()` itself does), but a path `split_workspace_path` refuses
    // to derive a workspace root from, because it carries a `..` (ADR
    // 0025).
    let dotdot = dir.path().join("..").join(name);

    // The default (workspace) route already holds this positional argument
    // as its trusted custody root, so it reads the selection directly from
    // it and still finds it despite the `..` spelling.
    let out = run(&dotdot, &["links"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("(target is not selected)"), "{}", stdout);

    // The --inventory route re-derives the workspace from the named file's
    // own path; a `..` there is refused outright, never silently read as
    // "no selection".
    let inventory_dotdot = dotdot.join(".factory").join("inventory.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "inventory",
            "links",
            "--inventory",
            inventory_dotdot.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains(".."), "{}", stderr);
}

#[test]
fn inventory_list_prints_links_notes_capped_at_five() {
    // Six foreign keys on one shape, each with its own by-id GET, so the
    // sixth note has to fall behind the cap.
    let mut s = spec();
    let mut props = serde_json::Map::new();
    props.insert("id".to_string(), json!({"type": "string"}));
    for r in [
        "artist",
        "label",
        "genre",
        "producer",
        "studio",
        "publisher",
    ] {
        props.insert(format!("{}_id", r), json!({"type": "string"}));
        s["components"]["schemas"][format!("{}Record", r)] =
            json!({"type": "object", "properties": {"id": {"type": "string"}}});
        s["paths"][format!("/{}s/{{{}Id}}", r, r)] = by_id(
            &format!("{}Id", r),
            &format!("get{}", r),
            &format!("{}Record", r),
        );
    }
    s["components"]["schemas"]["Release"] = json!({"type": "object", "properties": props});
    s["paths"]["/releases/{releaseId}"] = by_id("releaseId", "getRelease", "Release");
    let inv = build(&s);
    let dir = workspace(
        &inv,
        Some(
            "contract_version: 1\noperations:\n  \"get:/artists/{artistId}\": { include: true }\n",
        ),
    );
    let inventory = dir.path().join(".factory/inventory.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "inventory",
            "list",
            "--inventory",
            inventory.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    // Under get:/releases/{releaseId}: five notes, then the overflow line.
    assert!(
        stdout.contains("      links: artist_id -> get:/artists/{artistId} (artistId)\n"),
        "{}",
        stdout
    );
    assert!(
        stdout.contains(
            "      links: label_id -> get:/labels/{labelId} (labelId) (target is not selected)\n"
        ),
        "{}",
        stdout
    );
    // Five under get:/releases/{releaseId} (the cap), plus one under each of
    // the three operations returning Song or the song list's items.
    assert_eq!(stdout.matches("      links: ").count(), 8, "{}", stdout);
    assert!(
        stdout.contains("      … and 1 more (inventory links)\n"),
        "{}",
        stdout
    );
    // The list op's inline items are annotated as a list item.
    assert!(
        stdout.contains("      links: []>album_id -> get:/albums/{albumId} (albumId) (list item) (target is not selected)\n"),
        "{}",
        stdout
    );

    // --json carries the page's rows under `candidate_links`.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args([
            "inventory",
            "list",
            "--inventory",
            inventory.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = report["candidate_links"].as_array().unwrap();
    // 6 on Release + 1 on Song + 1 on the song list's items.
    assert_eq!(rows.len(), 8, "{}", report["candidate_links"]);
    assert!(rows.iter().any(|r| r["shape"] == "Release"
        && r["path"] == "publisher_id"
        && r["target_selected"] == false));
}

#[test]
fn inventory_list_notes_links_unannotated_when_the_selection_cannot_be_read() {
    // ADR 0069 whole-branch review, finding A: `inventory list` is a pager,
    // not a selection reader. A `..` in `--inventory` or an unparsable
    // selection.yaml beside the inventory costs the notes their
    // `(target is not selected)` annotation, never the listing itself —
    // `inventory describe` on the same path exits 0. Only `inventory links
    // --inventory` refuses a `..` outright (the test above).
    let inv = build(&spec());
    // A selection that would annotate the list operation's note: it does
    // not list get:/albums/{albumId}, so a read selection marks the target
    // not selected.
    let dir = workspace(
        &inv,
        Some("contract_version: 1\noperations:\n  \"get:/songs\": { include: true }\n"),
    );
    let name = dir.path().file_name().unwrap();
    let dotdot = dir.path().join("..").join(name);
    let list = |inventory: &Path, json: bool| {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"));
        cmd.args([
            "inventory",
            "list",
            "--inventory",
            inventory.to_str().unwrap(),
            "--limit",
            "1",
        ]);
        if json {
            cmd.arg("--json");
        }
        cmd.output().unwrap()
    };

    // The page's one operation is get:/songs; its note prints without the
    // annotation, and stderr says why.
    let unread = "      links: []>album_id -> get:/albums/{albumId} (albumId) (list item)\n";
    let out = list(&dotdot.join(".factory").join("inventory.json"), false);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{}", stderr);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("get:/songs"), "{}", stdout);
    assert!(stdout.contains(unread), "{}", stdout);
    assert!(!stdout.contains("(target is not selected)"), "{}", stdout);
    assert!(
        stderr.contains("inventory list: selection not read:")
            && stderr.contains("contains `..`; link targets unannotated"),
        "{}",
        stderr
    );
    let out = list(&dotdot.join(".factory").join("inventory.json"), true);
    assert_eq!(out.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = report["candidate_links"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{}", report);
    assert_eq!(rows[0]["target_selected"], Value::Null);

    // A selection.yaml that does not parse: the same, and the same note.
    let broken = workspace(&inv, Some("operations: [unclosed\n"));
    let out = list(
        &broken.path().join(".factory").join("inventory.json"),
        false,
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(out.status.code(), Some(0), "{}", stderr);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains(unread), "{}", stdout);
    assert!(
        stderr.contains("inventory list: .factory/selection.yaml:")
            && stderr.contains("; link targets unannotated"),
        "{}",
        stderr
    );

    // And a readable selection beside a sound path still annotates.
    let out = list(&dir.path().join(".factory").join("inventory.json"), false);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("      links: []>album_id -> get:/albums/{albumId} (albumId) (list item) (target is not selected)\n"),
        "{}",
        stdout
    );
}

#[test]
fn inventory_links_help_prints_the_usage_and_exits_0() {
    // Run where no inventory exists: `--help` must not read one.
    let dir = tempfile::tempdir().unwrap();
    for flag in ["--help", "-h"] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
            .args(["inventory", "links", flag])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{} {}",
            flag,
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            stdout.contains("usage: inventory links [workspace] [--inventory FILE] [--json]"),
            "{} {}",
            flag,
            stdout
        );
    }
}

#[test]
fn inventory_links_says_when_no_operation_returns_the_host_directly() {
    // ADR 0069, R30: pass 2 walks every named shape a response reaches
    // through `$ref`, so a fact can sit on a shape no operation returns
    // (Track, reached only through the list's `items.$ref`).
    let mut s = spec();
    s["components"]["schemas"]["Track"] = json!({"type": "object", "properties": {
        "title": {"type": "string"},
        "album_id": {"type": "string"}
    }});
    s["paths"]["/tracks"] = json!({
        "get": {
            "operationId": "listTracks",
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "array", "items": {"$ref": "#/components/schemas/Track"}
            }}}}}
        }
    });
    let inv = build(&s);
    let track: Vec<_> = candidate_links(&inv)
        .into_iter()
        .filter(|l| l.shape == "Track")
        .collect();
    assert_eq!(track.len(), 1);
    assert_eq!(track[0].path, "album_id");
    assert!(track[0].returned_by.is_empty());
    let dir = workspace(&inv, None);
    let out = run(dir.path(), &["links"]);
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains(
            "Track > album_id -> get:/albums/{albumId} (albumId)\n    returned by: no operation directly (reached through a `$ref`)\n"
        ),
        "{}",
        stdout
    );
}

#[test]
fn link_field_name_singularises_and_camel_cases_the_last_static_segment() {
    // The one derivation of a link's proposed field name (ADR 0069, R17):
    // selection draft, reconcile, lint, links apply and scaffold all call
    // this, so its rules are pinned once, here.
    assert_eq!(link_field_name("/albums/{album_id}"), "album");
    assert_eq!(link_field_name("/spotify/songs/{song_id}"), "song");
    assert_eq!(link_field_name("/payment_cards/{id}"), "paymentCard");
    assert_eq!(
        link_field_name("/payment-cards/{id}"),
        "paymentCard",
        "a hyphen is a separator too"
    );
    assert_eq!(link_field_name("/categories/{id}"), "category");
    assert_eq!(link_field_name("/statuses/{id}"), "status");
    assert_eq!(
        link_field_name("/glass/{id}"),
        "glass",
        "a double s is not a plural"
    );
    assert_eq!(
        link_field_name("/users/{username}/orgs"),
        "org",
        "the last static segment, not the one before the parameter"
    );
    assert_eq!(
        link_field_name("/{id}"),
        "item",
        "a path with no static segment falls back to `item`"
    );
    assert_eq!(
        link_field_name("/albums/{album_id}/"),
        "album",
        "a trailing slash is not a segment"
    );
    // A bare `ses` is not enough to mean "drop the `es`": these all end in
    // `es` but are not one of the six endings that lose it (ADR 0069 fix
    // round 1, finding 1). gitea already has `get:/licenses/{name}`.
    assert_eq!(
        link_field_name("/releases/{id}"),
        "release",
        "a plain trailing `s` comes off; `es` does not"
    );
    assert_eq!(link_field_name("/databases/{id}"), "database");
    assert_eq!(link_field_name("/licenses/{id}"), "license");
    // These do end in one of the six endings that lose `es`, including the
    // `-ch`/`-x` families that keep their own final consonant.
    assert_eq!(
        link_field_name("/branches/{id}"),
        "branch",
        "`ches` loses `es`, keeping the `ch`"
    );
    assert_eq!(
        link_field_name("/boxes/{id}"),
        "box",
        "`xes` loses `es`, keeping the `x`"
    );
    assert_eq!(
        link_field_name("/statuses/{id}"),
        "status",
        "`uses` loses `es`"
    );

    // A derivation that is empty or not a legal GraphQL name falls back to
    // `item`, and the first letter of a legal one is always lowercased
    // (ADR 0069 fix round 1, finding 3).
    assert_eq!(
        link_field_name("/s/{id}"),
        "item",
        "singularising the whole segment away leaves nothing"
    );
    assert_eq!(
        link_field_name("/2fa/{id}"),
        "item",
        "a leading digit is never a legal GraphQL name"
    );
    assert_eq!(
        link_field_name("/Users/{id}"),
        "user",
        "the first letter is lowercased, never left capitalised"
    );
}

#[test]
fn inventory_links_records_a_get_by_id_whose_response_does_not_carry_its_key() {
    // ADR 0085: `get:/balances/{email}` is a canonical GET-by-id, but its
    // response is a balance, not the record `email` names, so `Song.email`
    // gets no fact and the refusal is printed where the facts are.
    let mut s = spec();
    s["components"]["schemas"]["Song"]["properties"]["email"] = json!({"type": "string"});
    s["components"]["schemas"]["Balance"] =
        json!({"type": "object", "properties": {"total": {"type": "number"}}});
    s["paths"]["/balances/{email}"] = json!({
        "get": {
            "operationId": "getBalance",
            "parameters": [{"name": "email", "in": "path", "required": true, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Balance"}}}}}
        }
    });
    let inv = build(&s);
    let dir = workspace(&inv, None);
    let out = run(dir.path(), &["links", "--json"]);
    assert!(out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(report["candidate_links"]
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["path"] != "email"));
    assert_eq!(
        report["refused_targets"],
        json!([{
            "operation": "get:/balances/{email}",
            "parameter": "email",
            "reason": "its response carries no `email` of the parameter's type: it describes something other than the record the key identifies"
        }])
    );
    let out = run(dir.path(), &["links"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("\n1 GET-by-id operation(s) refused as link targets:\n  get:/balances/{email} (email): its response carries no `email`"),
        "{}",
        stdout
    );
}

//! The Elastic License v2 gate (`scripts/elv2.sh`): rover's supergraph
//! composition plugin is ELv2-licensed, and only the user accepts it, by
//! setting APOLLO_ELV2_LICENSE=accept. Unset, every wrapper that composes
//! (compose, unit, e2e, live) prints the instruction and exits 3, which
//! `evidence` records as `not_run` with that reason: never `pass`, never
//! `fail`. Set, the wrappers run as before and pass the user's variable to
//! rover untouched, never a flag of their own. rover is a stub on PATH that
//! records its arguments and the variable it was given.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const WRAPPERS: [&str; 4] = ["compose", "unit", "e2e", "live"];

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

struct Env {
    root: tempfile::TempDir,
    ws: PathBuf,
    tools: PathBuf,
}

impl Env {
    /// A copy of the public gitea pilot and stubs for every tool a wrapper
    /// checks before the gate. The stub rover appends one line per call to
    /// `rover.calls`: the variable it saw, then its arguments.
    fn new() -> Env {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("gitea");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea"),
            &ws,
        );
        let tools = root.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        executable(
            &tools.join("rover"),
            "#!/usr/bin/env bash\n\
             echo \"APOLLO_ELV2_LICENSE=${APOLLO_ELV2_LICENSE:-<unset>} $*\" >> \"$(dirname \"$0\")/rover.calls\"\n\
             case \"$1\" in --version) echo 'Rover 0.41.0' ;; esac\n",
        );
        for tool in ["java", "curl", "jq"] {
            executable(&tools.join(tool), "#!/usr/bin/env bash\nexit 0\n");
        }
        Env { root, ws, tools }
    }

    fn rover_calls(&self) -> String {
        std::fs::read_to_string(self.tools.join("rover.calls")).unwrap_or_default()
    }

    fn command(&self, program: &str, licence: Option<&str>) -> Command {
        let mut c = Command::new(program);
        c.env(
            "PATH",
            format!(
                "{}:{}",
                self.tools.display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("HOME", self.root.path().join("home"))
        .env("GRAPHOS_FACTORY_CORE_CACHE", self.root.path().join("cache"))
        .env(
            "GRAPHOS_FACTORY_CORE_BIN",
            env!("CARGO_BIN_EXE_graphos-factory-bare"),
        )
        .env_remove("GRAPHOS_FACTORY_CORE_VERSION")
        .env_remove("GRAPHOS_FACTORY_CORE_LIVE_ENV")
        // live.sh reaches the gate only once its credential is set.
        .env("GITEA_TOKEN", "stand-in");
        match licence {
            Some(v) => c.env("APOLLO_ELV2_LICENSE", v),
            None => c.env_remove("APOLLO_ELV2_LICENSE"),
        };
        c
    }

    fn wrapper(&self, name: &str, licence: Option<&str>) -> Output {
        let script =
            Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR")).join(format!("{}.sh", name));
        self.command("bash", licence)
            .arg(script)
            .arg(&self.ws)
            .output()
            .unwrap()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn assert_instruction(name: &str, err: &str) {
    for needle in [
        format!(
            "{}: APOLLO_ELV2_LICENSE is not set to accept, and only you can accept the Elastic License v2 (not_run)",
            name
        ),
        "https://www.elastic.co/licensing/elastic-license".to_string(),
        "rover supergraph compose".to_string(),
        "and the Apollo Router".to_string(),
        "set `APOLLO_ELV2_LICENSE=accept`".to_string(),
        "an agent sets it only after you say yes".to_string(),
    ] {
        assert!(err.contains(&needle), "{}.sh lacks {:?}: {}", name, needle, err);
    }
}

#[test]
fn every_wrapper_unset_exits_not_run_with_the_instruction_before_rover_composes() {
    for licence in [None, Some(""), Some("yes")] {
        for name in WRAPPERS {
            let env = Env::new();
            let out = env.wrapper(name, licence);
            let err = stderr(&out);
            assert_eq!(
                out.status.code(),
                Some(3),
                "{}.sh ({:?}): {}",
                name,
                licence,
                err
            );
            assert_instruction(name, &err);
            // At most `rover --version` ran (e2e and live check it is
            // installed); nothing that needs the plugin did.
            let calls = env.rover_calls();
            assert!(
                calls.lines().all(|l| l.ends_with(" --version")),
                "{}.sh ({:?}) called rover past the gate: {}",
                name,
                licence,
                calls
            );
        }
    }
}

#[test]
fn compose_with_the_variable_set_passes_it_through_and_adds_no_flag() {
    let env = Env::new();
    let out = env.wrapper("compose", Some("accept"));
    let err = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{}", err);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("compose: pass"),
        "{}",
        err
    );
    assert!(!err.contains("Elastic License"), "{}", err);
    let calls = env.rover_calls();
    let compose = calls
        .lines()
        .find(|l| l.contains("supergraph compose"))
        .unwrap_or_else(|| panic!("compose.sh never composed: {}", calls));
    assert!(
        compose.starts_with("APOLLO_ELV2_LICENSE=accept "),
        "{}",
        compose
    );
    assert!(!calls.contains("--elv2-license"), "{}", calls);
}

#[test]
fn no_wrapper_originates_the_acceptance() {
    let scripts = Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR"));
    for entry in std::fs::read_dir(scripts).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("sh") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            let code = line.split(" echo ").next().unwrap_or(line);
            assert!(
                !code.contains("--elv2-license") && !code.contains("APOLLO_ELV2_LICENSE=accept"),
                "{} accepts the ELv2 itself: {}",
                path.display(),
                line
            );
        }
    }
}

#[test]
fn evidence_records_the_composing_layers_not_run_with_the_reason() {
    let env = Env::new();
    let out = env
        .command(env!("CARGO_BIN_EXE_graphos-factory-bare"), None)
        .args(["evidence", env.ws.to_str().unwrap(), "--scripts"])
        .arg(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR"))
        .args(["--only", "compose,unit,e2e,live"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let latest: Value = serde_json::from_str(
        &std::fs::read_to_string(env.ws.join(".factory/evidence/latest.json")).unwrap(),
    )
    .unwrap_or_else(|e| panic!("{}: {}{}", e, stdout, stderr(&out)));
    for (key, layer) in [
        ("compose", "compose"),
        ("connector_unit", "unit"),
        ("wiremock_e2e", "e2e"),
        ("live", "live"),
    ] {
        let row = &latest["layers"][key];
        assert_eq!(row["status"], "not_run", "{}: {}", key, row);
        assert_eq!(row["exit_code"], 3, "{}: {}", key, row);
        assert_eq!(
            row["reason"],
            format!(
                "{}: APOLLO_ELV2_LICENSE is not set to accept, and only you can accept the Elastic License v2 (not_run)",
                layer
            ),
            "{}: {}",
            key,
            row
        );
    }
    // Not the zero-case run: no case count, so no gate reads it as one.
    assert!(
        latest["layers"]["connector_unit"].get("cases").is_none(),
        "{}",
        latest["layers"]["connector_unit"]
    );
    // No operation is credited, and the report says none is proven.
    for (op, columns) in latest["operations"].as_object().unwrap() {
        assert!(
            !columns.as_object().unwrap().values().any(|v| v == "pass"),
            "{}: {}",
            op,
            columns
        );
    }
    assert!(
        stdout.contains(
            "not validated: no executed layer ran (compose: APOLLO_ELV2_LICENSE is not set to accept"
        ),
        "{}",
        stdout
    );
    assert!(
        stdout.contains(&format!(
            "not validated: {} selected operation(s) have no executed evidence: ",
            latest["operations"].as_object().unwrap().len()
        )),
        "{}",
        stdout
    );
}

/// `toolchain.sh` against a scratch HOME and cache: rover (a stub at the
/// pinned version) and WireMock are in place; the plugin and the Router are
/// whatever `plugin` and `router` say. curl is a stub that records any call
/// and fails, so a download attempt shows up instead of reaching the network.
struct Toolchain {
    root: tempfile::TempDir,
}

impl Toolchain {
    fn new(plugin: bool, router: bool) -> Toolchain {
        let root = tempfile::tempdir().unwrap();
        let rover_bin = root.path().join("home/.rover/bin");
        std::fs::create_dir_all(&rover_bin).unwrap();
        executable(
            &rover_bin.join("rover"),
            "#!/usr/bin/env bash\n[ \"$1\" = --version ] && echo 'Rover 0.41.0'\n",
        );
        if plugin {
            executable(&rover_bin.join("supergraph-v2.15.2"), "#!/bin/sh\n");
        }
        let cache = root.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("wiremock-standalone-3.13.2.jar"), "").unwrap();
        if router {
            executable(&cache.join("router-2.17.0"), "#!/bin/sh\n");
        }
        let tools = root.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        executable(
            &tools.join("curl"),
            "#!/usr/bin/env bash\necho \"$*\" >> \"$(dirname \"$0\")/curl.calls\"\nexit 22\n",
        );
        Toolchain { root }
    }

    fn run(&self, args: &[&str], licence: Option<&str>) -> (i32, String) {
        let script = Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR")).join("toolchain.sh");
        let mut c = Command::new("bash");
        c.arg(script)
            .args(args)
            .env("HOME", self.root.path().join("home"))
            .env("GRAPHOS_FACTORY_CORE_CACHE", self.root.path().join("cache"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.path().join("tools").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("ROVER_VERSION", "0.41.0")
            .env("FEDERATION_VERSION", "2.15.2")
            .env("ROUTER_VERSION", "2.17.0")
            .env("WIREMOCK_VERSION", "3.13.2");
        match licence {
            Some(v) => c.env("APOLLO_ELV2_LICENSE", v),
            None => c.env_remove("APOLLO_ELV2_LICENSE"),
        };
        let out = c.output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    }

    fn curl_calls(&self) -> String {
        std::fs::read_to_string(self.root.path().join("tools/curl.calls")).unwrap_or_default()
    }

    fn router_cached(&self) -> bool {
        self.root.path().join("cache/router-2.17.0").exists()
    }
}

fn assert_toolchain_instruction(out: &str) {
    for needle in [
        "toolchain: APOLLO_ELV2_LICENSE is not set to accept, and only you can accept the Elastic License v2",
        "and the Apollo Router, which e2e and live run, are licensed under the Elastic License v2",
        "set `APOLLO_ELV2_LICENSE=accept`",
    ] {
        assert!(out.contains(needle), "toolchain.sh lacks {:?}: {}", needle, out);
    }
    assert_eq!(
        out.matches("only you can accept").count(),
        1,
        "the instruction is printed once: {}",
        out
    );
}

#[test]
fn toolchain_unset_downloads_neither_the_plugin_nor_the_router() {
    let t = Toolchain::new(false, false);
    let (code, out) = t.run(&[], None);
    assert_eq!(code, 0, "{}", out);
    assert_toolchain_instruction(&out);
    assert!(
        out.contains(
            "toolchain: not downloading the supergraph plugin v2.15.2 and the Apollo Router 2.17.0"
        ),
        "{}",
        out
    );
    assert!(
        out.contains("no supergraph plugin, no router, wiremock 3.13.2 ready (APOLLO_ELV2_LICENSE not set to accept)"),
        "{}",
        out
    );
    assert_eq!(t.curl_calls(), "", "toolchain.sh downloaded: {}", out);
    assert!(!t.router_cached(), "{}", out);
}

#[test]
fn toolchain_check_unset_with_the_router_missing_fails_with_the_instruction() {
    let t = Toolchain::new(true, false);
    let (code, out) = t.run(&["--check"], None);
    assert_eq!(code, 1, "{}", out);
    assert!(
        out.contains("toolchain: Apollo Router 2.17.0 is not at"),
        "{}",
        out
    );
    assert_toolchain_instruction(&out);
    assert_eq!(t.curl_calls(), "", "{}", out);
}

#[test]
fn toolchain_check_unset_with_everything_present_is_a_note_not_a_failure() {
    let t = Toolchain::new(true, true);
    let (code, out) = t.run(&["--check"], None);
    assert_eq!(code, 0, "{}", out);
    assert!(
        out.contains("toolchain: note — the supergraph plugin and the Apollo Router are installed, but APOLLO_ELV2_LICENSE is not set to accept"),
        "{}",
        out
    );
    assert_toolchain_instruction(&out);
    let (code, out) = t.run(&["--check"], Some("accept"));
    assert_eq!(code, 0, "{}", out);
    assert!(!out.contains("Elastic License"), "{}", out);
}

#[test]
fn toolchain_accepted_tries_to_download_the_router() {
    // curl is the failing stub: the attempt is what is under test.
    let t = Toolchain::new(true, false);
    let (code, out) = t.run(&[], Some("accept"));
    assert_ne!(code, 0, "{}", out);
    assert!(
        t.curl_calls()
            .contains("apollographql/router/releases/download/v2.17.0"),
        "{}\n{}",
        t.curl_calls(),
        out
    );
    assert!(!out.contains("Elastic License"), "{}", out);
}

//! Which product binary the wrappers run (ADR 0087). Every wrapper
//! sources `scripts/resolve-bin.sh`: $GRAPHOS_FACTORY_CORE_BIN, then the bootstrap
//! cache's `graphos-factory-core` link, then `graphos-factory-core` on PATH, and a binary reporting a version older than the pin
//! (crate/Cargo.toml, release.env in a copy of the core with no crate/ above
//! it, or $GRAPHOS_FACTORY_CORE_VERSION) is refused with exit 78 —
//! never used silently. The binaries here are stubs that print a version and
//! drop a marker file when asked to do anything else, so a test can tell
//! "refused before use" from "used". The other tools each wrapper checks for
//! first (rover, java, curl, jq, git) are stubs on PATH, and are never reached
//! by a refusal.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const PIN: &str = env!("CARGO_PKG_VERSION");
// A target's own scripts source the same resolver and are its suite's.
const WRAPPERS: [&str; 4] = ["compose", "unit", "e2e", "live"];

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Env {
    root: tempfile::TempDir,
    /// PATH stubs for the tools the wrappers check before the binary.
    tools: PathBuf,
    /// Empty cache unless a test puts a binary in it.
    cache: PathBuf,
}

impl Env {
    fn new() -> Env {
        let root = tempfile::tempdir().unwrap();
        let tools = root.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        for tool in ["rover", "java", "curl", "jq", "git"] {
            executable(&tools.join(tool), "#!/usr/bin/env bash\nexit 0\n");
        }
        let cache = root.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        Env { root, tools, cache }
    }

    /// A binary that reports `version` as given (None: prints nothing) and
    /// touches `<path>.used` for any other subcommand.
    fn stub(&self, path: &Path, version: Option<&str>) -> PathBuf {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let report = match version {
            Some(v) => format!("echo 'graphos-factory-core {}'", v),
            None => ":".to_string(),
        };
        executable(
            path,
            &format!(
                "#!/usr/bin/env bash\ncase \"$1\" in\n  version) {} ;;\n  *) touch \"$0.used\"; exit 1 ;;\nesac\n",
                report
            ),
        );
        path.to_path_buf()
    }

    fn used(&self, bin: &Path) -> bool {
        PathBuf::from(format!("{}.used", bin.display())).exists()
    }

    /// Run a wrapper with PATH = tool stubs (+ `extra_path` first) + the
    /// system PATH, the cache pointed at this test's, and `bin` (if any) as
    /// GRAPHOS_FACTORY_CORE_BIN.
    fn wrapper(
        &self,
        name: &str,
        bin: Option<&Path>,
        extra_path: Option<&Path>,
        version: Option<&str>,
    ) -> (i32, String) {
        self.wrapper_in(&scripts(), name, bin, extra_path, version)
    }

    /// `wrapper`, run from the scripts directory `dir`.
    fn wrapper_in(
        &self,
        dir: &Path,
        name: &str,
        bin: Option<&Path>,
        extra_path: Option<&Path>,
        version: Option<&str>,
    ) -> (i32, String) {
        let mut path = format!("{}:{}", self.tools.display(), system_path());
        if let Some(p) = extra_path {
            path = format!("{}:{}", p.display(), path);
        }
        let ws = self.root.path().join("ws");
        std::fs::create_dir_all(ws.join(".factory")).unwrap();
        let mut c = Command::new("bash");
        c.arg(dir.join(format!("{}.sh", name))).arg(&ws);
        c.env("PATH", path)
            .env("GRAPHOS_FACTORY_CORE_CACHE", &self.cache)
            // The user's own acceptance of the plugin's ELv2 (elv2.sh); the
            // gate itself is tested in elv2.rs.
            .env("APOLLO_ELV2_LICENSE", "accept")
            .env_remove("GRAPHOS_FACTORY_CORE_BIN")
            .env_remove("GRAPHOS_FACTORY_CORE_VERSION");
        if let Some(b) = bin {
            c.env("GRAPHOS_FACTORY_CORE_BIN", b);
        }
        if let Some(v) = version {
            c.env("GRAPHOS_FACTORY_CORE_VERSION", v);
        }
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
}

/// The system PATH without any directory holding a `graphos-factory-core`, so the
/// PATH step of the resolution only ever finds a stub a test put there.
fn system_path() -> String {
    std::env::var("PATH")
        .unwrap()
        .split(':')
        .filter(|d| !Path::new(d).join("graphos-factory-core").exists())
        .collect::<Vec<_>>()
        .join(":")
}

fn scripts() -> PathBuf {
    Path::new(env!("GRAPHOS_FACTORY_CORE_SCRIPTS_DIR")).to_path_buf()
}

/// A version one patch below the pin: the smallest step that must be refused.
fn one_below() -> String {
    let mut parts: Vec<u64> = PIN.split('.').map(|p| p.parse().unwrap()).collect();
    if parts[2] > 0 {
        parts[2] -= 1;
    } else {
        parts[1] -= 1;
        parts[2] = 99;
    }
    format!("{}.{}.{}", parts[0], parts[1], parts[2])
}

#[test]
fn every_wrapper_refuses_a_stale_binary_before_using_it() {
    for name in WRAPPERS {
        let env = Env::new();
        let stale = env.stub(&env.root.path().join("stale/factory"), Some("0.5.1"));
        let (code, out) = env.wrapper(name, Some(&stale), None, None);
        assert_eq!(code, 78, "{}.sh: {}", name, out);
        assert!(
            !env.used(&stale),
            "{}.sh ran the stale binary: {}",
            name,
            out
        );
        assert!(
            out.contains(&format!("{}: FAIL", name))
                && out.contains("version 0.5.1, older than the")
                && out.contains(PIN)
                && out.contains("bootstrap.sh --build")
                && out.contains("GRAPHOS_FACTORY_CORE_BIN"),
            "{}.sh: {}",
            name,
            out
        );
    }
}

#[test]
fn one_patch_below_the_pin_is_refused() {
    let env = Env::new();
    let stale = env.stub(&env.root.path().join("stale/factory"), Some(&one_below()));
    let (code, out) = env.wrapper("compose", Some(&stale), None, None);
    assert_eq!(code, 78, "{}", out);
    assert!(!env.used(&stale), "{}", out);
}

#[test]
fn a_stale_cache_shadowing_a_current_binary_on_path_is_refused() {
    // The field failure: ~/.cache held 0.5.1, PATH held the current binary,
    // and the cache won. The order stays; the stale binary is now refused
    // and the message names where it came from.
    let env = Env::new();
    let stale = env.stub(&env.cache.join("bin/graphos-factory-core"), Some("0.5.1"));
    let on_path = env.root.path().join("path");
    let current = env.stub(&on_path.join("graphos-factory-core"), Some(PIN));
    let (code, out) = env.wrapper("compose", None, Some(&on_path), None);
    assert_eq!(code, 78, "{}", out);
    assert!(!env.used(&stale) && !env.used(&current), "{}", out);
    assert!(
        out.contains(&stale.display().to_string()) && out.contains("the bootstrap cache"),
        "{}",
        out
    );
}

#[test]
fn a_binary_at_the_pin_is_accepted_and_named() {
    for name in WRAPPERS {
        let env = Env::new();
        let bin = env.stub(&env.root.path().join("ok/factory"), Some(PIN));
        let (code, out) = env.wrapper(name, Some(&bin), None, None);
        assert_ne!(code, 78, "{}.sh: {}", name, out);
        // Past the resolver each wrapper has its own preconditions (a suite,
        // a live.yaml); compose reaches `render` on an
        // empty workspace, so it is the one that proves the binary is run.
        if name == "compose" {
            assert!(env.used(&bin), "compose.sh never ran the binary: {}", out);
        }
        assert!(
            out.contains(&format!(
                "graphos-factory-core: {} runs {} (from GRAPHOS_FACTORY_CORE_BIN), version {} (expected >= {})",
                name,
                bin.display(),
                PIN,
                PIN
            )),
            "{}.sh: {}",
            name,
            out
        );
    }
}

#[test]
fn a_newer_binary_is_used() {
    // The edge pre-release is built from main's head and may be ahead of a
    // checkout's pin; ahead is fine, behind is not.
    let env = Env::new();
    let bin = env.stub(&env.root.path().join("edge/factory"), Some("99.0.0-edge"));
    let (code, out) = env.wrapper("compose", Some(&bin), None, None);
    assert_ne!(code, 78, "{}", out);
    assert!(env.used(&bin), "{}", out);
}

#[test]
fn a_binary_that_reports_no_version_is_refused() {
    let env = Env::new();
    let bin = env.stub(&env.root.path().join("mute/factory"), None);
    let (code, out) = env.wrapper("unit", Some(&bin), None, None);
    assert_eq!(code, 78, "{}", out);
    assert!(!env.used(&bin), "{}", out);
    assert!(out.contains("does not report a version"), "{}", out);
}

#[test]
fn the_version_variable_sets_the_pin() {
    // The same variable bootstrap.sh installs by: with it lowered, the older
    // binary is what was asked for; with it raised, the current one is stale.
    let env = Env::new();
    let old = env.stub(&env.root.path().join("old/factory"), Some("0.5.1"));
    let (code, out) = env.wrapper("compose", Some(&old), None, Some("0.5.1"));
    assert_ne!(code, 78, "{}", out);
    assert!(env.used(&old), "{}", out);
    let cur = env.stub(&env.root.path().join("cur/factory"), Some(PIN));
    let (code, out) = env.wrapper("compose", Some(&cur), None, Some("99.0.0"));
    assert_eq!(code, 78, "{}", out);
    assert!(out.contains("(GRAPHOS_FACTORY_CORE_VERSION)"), "{}", out);
}

/// The core's scripts copied the way the public tree copies them into an
/// installed skill: `<skill>/graphos-factory-core/scripts`, with no crate/
/// two levels up, and `release` (when given) as its release.env.
fn detached_scripts(env: &Env, release: Option<&str>) -> PathBuf {
    let core = env.root.path().join("skill/graphos-factory-core");
    copy_dir(&scripts(), &core.join("scripts"));
    if let Some(text) = release {
        std::fs::write(core.join("release.env"), text).unwrap();
    }
    core.join("scripts")
}

#[test]
fn a_copy_of_the_core_reads_its_pin_from_release_env() {
    // npx skills and gh skill copy a skill directory alone: its copy of the
    // core has no crate/Cargo.toml above it, so the pin comes from the
    // release.env written beside scripts/ when the copy was made.
    let env = Env::new();
    let dir = detached_scripts(
        &env,
        Some("# a comment\nversion=99.0.0\nrepository=owner/repo\n"),
    );
    let cur = env.stub(&env.root.path().join("cur/factory"), Some(PIN));
    let (code, out) = env.wrapper_in(&dir, "compose", Some(&cur), None, None);
    assert_eq!(code, 78, "{}", out);
    assert!(out.contains("older than the 99.0.0"), "{}", out);
    assert!(out.contains("(release.env)"), "{}", out);
    assert!(!env.used(&cur), "{}", out);
    std::fs::write(dir.join("../release.env"), format!("version={}\n", PIN)).unwrap();
    let (code, out) = env.wrapper_in(&dir, "compose", Some(&cur), None, None);
    assert_ne!(code, 78, "{}", out);
    assert!(out.contains(&format!("(expected >= {})", PIN)), "{}", out);
    // The variable still overrides the file.
    let (code, out) = env.wrapper_in(&dir, "compose", Some(&cur), None, Some("99.0.0"));
    assert_eq!(code, 78, "{}", out);
    assert!(out.contains("(GRAPHOS_FACTORY_CORE_VERSION)"), "{}", out);
}

#[test]
fn a_copy_with_no_pin_warns_and_names_both_sources() {
    let env = Env::new();
    let dir = detached_scripts(&env, None);
    let cur = env.stub(&env.root.path().join("cur/factory"), Some(PIN));
    let (code, out) = env.wrapper_in(&dir, "compose", Some(&cur), None, None);
    assert_ne!(code, 78, "{}", out);
    assert!(
        out.contains("WARNING: no pin to check it against (no crate/Cargo.toml or release.env beside the scripts"),
        "{}",
        out
    );
}

#[test]
fn no_binary_anywhere_is_127() {
    let env = Env::new();
    let (code, out) = env.wrapper("compose", None, None, None);
    assert_eq!(code, 127, "{}", out);
    assert!(out.contains("is not installed"), "{}", out);
}

#[test]
fn evidence_hands_the_wrappers_its_own_binary() {
    // `graphos-factory-core evidence` runs the wrappers; without GRAPHOS_FACTORY_CORE_BIN
    // they would resolve the cache first and find the stale binary there.
    // evidence names itself instead, so the layer runs on the binary the user
    // invoked, and the log says which one.
    let env = Env::new();
    let stale = env.stub(&env.cache.join("bin/graphos-factory-core"), Some("0.5.1"));
    let ws = env.root.path().join("gitea");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea"),
        &ws,
    );
    let exe = env!("CARGO_BIN_EXE_graphos-factory-bare");
    let out = Command::new(exe)
        .args(["evidence", ws.to_str().unwrap(), "--scripts"])
        .arg(scripts())
        .args(["--only", "compose"])
        .env("PATH", format!("{}:{}", env.tools.display(), system_path()))
        .env("GRAPHOS_FACTORY_CORE_CACHE", &env.cache)
        .env_remove("GRAPHOS_FACTORY_CORE_BIN")
        .env_remove("GRAPHOS_FACTORY_CORE_VERSION")
        .output()
        .unwrap();
    let latest: Value = serde_json::from_str(
        &std::fs::read_to_string(ws.join(".factory/evidence/latest.json")).unwrap(),
    )
    .unwrap_or_else(|e| panic!("{}: {}", e, String::from_utf8_lossy(&out.stderr)));
    let compose = &latest["layers"]["compose"];
    let log = std::fs::read_to_string(ws.join(compose["log"].as_str().unwrap())).unwrap();
    assert_ne!(compose["exit_code"], 78, "{}", log);
    assert!(!env.used(&stale), "{}", log);
    assert!(
        log.contains(&format!(
            "graphos-factory-core: compose runs {} (from GRAPHOS_FACTORY_CORE_BIN)",
            exe
        )),
        "{}",
        log
    );
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

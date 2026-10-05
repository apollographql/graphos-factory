use std::process::Command;
use std::{path::Path, path::PathBuf};

fn output(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR"),
    );
    let root = manifest_dir.parent().unwrap_or(Path::new(&manifest_dir));
    let root_arg = root.to_string_lossy();
    let revision = output(Command::new("git").args(["-C", &root_arg, "rev-parse", "HEAD"]));
    let status = output(Command::new("git").args([
        "-C",
        &root_arg,
        "status",
        "--porcelain",
        "--untracked-files=all",
    ]));
    // A missing revision or unreadable status is not evidence of a clean tree.
    let dirty = match (&revision, status) {
        (Some(_), Some(status)) => !status.is_empty(),
        _ => true,
    };
    let revision = revision.unwrap_or_else(|| "unknown".into());
    let rustc = output(Command::new("rustc").arg("--version")).unwrap_or_else(|| "unknown".into());
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".into());

    println!("cargo:rustc-env=GRAPHOS_FACTORY_CORE_BUILD_REVISION={revision}");
    println!("cargo:rustc-env=GRAPHOS_FACTORY_CORE_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=GRAPHOS_FACTORY_CORE_BUILD_RUSTC={rustc}");
    println!("cargo:rustc-env=GRAPHOS_FACTORY_CORE_BUILD_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");

    // The core scripts directory, `graphos-factory-core/scripts/` at the repository root:
    // the core's own path, never a target's. `provenance` embeds the
    // toolchain pins from it; the core suite runs the wrappers from it.
    let repo = root.parent().unwrap_or(root);
    let scripts = repo.join("graphos-factory-core/scripts");
    assert!(
        scripts.join("toolchain.sh").is_file(),
        "expected the core scripts at {}, with toolchain.sh",
        scripts.display()
    );
    println!(
        "cargo:rustc-env=GRAPHOS_FACTORY_CORE_SCRIPTS_DIR={}",
        scripts.display()
    );
    println!(
        "cargo:rustc-env=GRAPHOS_FACTORY_CORE_SKILL_DIR={}",
        scripts.parent().unwrap_or(&scripts).display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        scripts.join("toolchain.sh").display()
    );
    // Cargo does not otherwise rerun this script for every dirty-tree change.
    // This intentionally missing path makes the cheap build script run once
    // per Cargo invocation, including after tracked or untracked edits.
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join(".provenance-always-rerun").display()
    );
    if let Some(head) =
        output(Command::new("git").args(["-C", &root_arg, "rev-parse", "--git-path", "HEAD"]))
    {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(reference) =
        output(Command::new("git").args(["-C", &root_arg, "symbolic-ref", "-q", "HEAD"]))
    {
        if let Some(path) = output(Command::new("git").args([
            "-C",
            &root_arg,
            "rev-parse",
            "--git-path",
            &reference,
        ])) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

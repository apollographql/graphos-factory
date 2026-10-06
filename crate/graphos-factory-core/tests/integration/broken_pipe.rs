//! A reader that closes stdout early (`| head -1`) ends the binary quietly:
//! no `failed printing to stdout` panic on stderr and no panic's exit 101.
//! The output is the public pilot's whole inventory as JSON (~900 KB), far
//! more than any pipe buffer holds, so the binary is always still writing
//! when the reader goes away.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

#[test]
fn a_reader_closing_stdout_early_is_no_panic() {
    let pilot = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../pilots/graphos/gitea");
    let mut child = Command::new(env!("CARGO_BIN_EXE_graphos-factory-bare"))
        .args(["inventory", "list", "--json", "--limit", "100000"])
        .current_dir(&pilot)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 1];
    stdout.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"{");
    drop(stdout);
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "{}",
        stderr
    );
    assert_ne!(out.status.code(), Some(101), "{:?}: {}", out.status, stderr);
}

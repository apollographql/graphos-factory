//! Custody for the files under a workspace's `.factory/` directory.
//!
//! A host application may run this binary against a workspace the user
//! names, so the one filesystem guarantee no host can give us is the one
//! we have to give ourselves: a `.factory/*` path this binary reads, or
//! writes in place, is a regular file *inside that workspace*. A
//! `.factory/selection.yaml` symlinked to `~/.aws/credentials` is refused on
//! read — so the secret never reaches a report, a lock hash or an error
//! message — and refused on write, so an edit never lands outside the
//! workspace.
//!
//! Two mechanisms, because neither is enough on its own:
//!
//! * every component of the relative path is `lstat`ed on the way down and a
//!   symlink at any of them — `.factory` itself, an intermediate directory,
//!   or the file — is refused. This is what makes the refusal *legible*: the
//!   error names the relative path and the offending component, never the
//!   link's target.
//! * the final `open` carries `O_NOFOLLOW` and `O_CLOEXEC`, so the last
//!   component cannot be swapped for a symlink between the `lstat` and the
//!   `open`, and the descriptor is not inherited by the wrappers `evidence`
//!   shells out to.
//!
//! Writes truncate the destination in place. They never rename a temporary
//! over it: a rename replaces a symlink with a regular file *silently*, and
//! the refusal is the point. `context.rs` keeps its two-file temp+rename —
//! it needs both-or-neither — and validates both destinations here first.
//!
//! `root` is the workspace directory as the caller was given it. Custody
//! starts *below* it: a workspace reached through a symlinked parent is the
//! user's own choice of path, not an escape from the workspace.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

/// The one directory this module guards. Every `relative` starts here.
pub const STATE_DIR: &str = ".factory";

/// Why a `.factory` path was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A component of the path is a symbolic link.
    SymlinkRefused,
    /// The relative path is empty, absolute, escapes with `..`, or is not
    /// under `.factory/`.
    OutsideWorkspace,
    /// The path exists but is a directory, a device, a socket — not a file.
    NotARegularFile,
    /// Everything else the filesystem said.
    Io(std::io::ErrorKind),
}

/// A refused or failed `.factory` access. `path` is always workspace-relative
/// and `/`-separated; the target of a refused link never appears anywhere in
/// this value.
#[derive(Debug, Clone)]
pub struct FactoryIoError {
    pub path: String,
    pub reason: Reason,
    /// The component that was refused (symlink), or the OS message (io).
    detail: Option<String>,
}

impl FactoryIoError {
    /// True when the path simply is not there — the common, benign case for
    /// an optional workspace file.
    pub fn is_not_found(&self) -> bool {
        self.reason == Reason::Io(std::io::ErrorKind::NotFound)
    }

    /// True when custody refused the path (as opposed to the filesystem
    /// failing the operation).
    pub fn is_refusal(&self) -> bool {
        !matches!(self.reason, Reason::Io(_))
    }

    fn refused(path: &str, reason: Reason, detail: Option<String>) -> Self {
        FactoryIoError {
            path: path.to_string(),
            reason,
            detail,
        }
    }

    fn io(path: &str, e: &std::io::Error) -> Self {
        FactoryIoError {
            path: path.to_string(),
            reason: Reason::Io(e.kind()),
            detail: Some(e.to_string()),
        }
    }
}

impl std::fmt::Display for FactoryIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            // Naming the refused component, and what it should have been,
            // is the whole diagnostic: the link's target never appears.
            Reason::SymlinkRefused => {
                let component = self.detail.as_deref().unwrap_or(&self.path);
                if component == self.path {
                    write!(
                        f,
                        "{}: refused — a symlink, not a regular file; the factory refuses it rather than follow it outside the workspace",
                        self.path
                    )
                } else {
                    write!(
                        f,
                        "{}: refused — {} is a symlink, not a directory; the factory refuses it rather than follow it outside the workspace",
                        self.path, component
                    )
                }
            }
            Reason::OutsideWorkspace => write!(
                f,
                "{}: refused — not a path under {}/ in the workspace",
                self.path, STATE_DIR
            ),
            Reason::NotARegularFile => {
                write!(f, "{}: refused — not a regular file", self.path)
            }
            Reason::Io(_) => write!(
                f,
                "{}: {}",
                self.path,
                self.detail.as_deref().unwrap_or("I/O error")
            ),
        }
    }
}

impl std::error::Error for FactoryIoError {}

impl From<FactoryIoError> for String {
    fn from(e: FactoryIoError) -> String {
        e.to_string()
    }
}

/// The relative path split into plain components, `.factory` first. `None`
/// for anything absolute, empty, non-UTF-8, `..`-bearing or rooted elsewhere.
fn components(relative: &str) -> Option<Vec<String>> {
    let mut parts: Vec<String> = Vec::new();
    for c in Path::new(relative).components() {
        match c {
            Component::Normal(os) => parts.push(os.to_str()?.to_string()),
            // RootDir, Prefix, ParentDir and CurDir are all refusals; CurDir
            // only survives `components()` for a bare ".".
            _ => return None,
        }
    }
    if parts.first().map(String::as_str) != Some(STATE_DIR) {
        return None;
    }
    Some(parts)
}

/// The path of `relative` inside `root`, with every component that exists
/// proven not to be a symlink.
///
/// The final component may be missing — that is a create. Intermediate
/// components that are missing are reported by the operation that needs
/// them, not here.
pub fn checked_path(root: &Path, relative: &str) -> Result<PathBuf, FactoryIoError> {
    let parts = match components(relative) {
        Some(p) => p,
        None => {
            return Err(FactoryIoError::refused(
                relative,
                Reason::OutsideWorkspace,
                None,
            ))
        }
    };
    let rel = parts.join("/");
    let mut cur = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        cur.push(part);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(FactoryIoError::refused(
                        &rel,
                        Reason::SymlinkRefused,
                        Some(parts[..=i].join("/")),
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Nothing below a missing component can exist, so nothing
                // below it can be a link either.
                for rest in &parts[i + 1..] {
                    cur.push(rest);
                }
                return Ok(cur);
            }
            Err(e) => return Err(FactoryIoError::io(&rel, &e)),
        }
    }
    Ok(cur)
}

/// The canonical `/`-separated spelling of `relative`, for messages.
fn rel_of(relative: &str) -> String {
    components(relative)
        .map(|p| p.join("/"))
        .unwrap_or_else(|| relative.to_string())
}

#[cfg(unix)]
fn read_flags() -> i32 {
    libc::O_NOFOLLOW | libc::O_CLOEXEC
}

/// `O_NOFOLLOW` reports `ELOOP` on Linux and macOS, `EMLINK` on FreeBSD.
#[cfg(unix)]
fn is_nofollow_error(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ELOOP) | Some(libc::EMLINK))
}

#[cfg(not(unix))]
fn is_nofollow_error(_e: &std::io::Error) -> bool {
    false
}

fn open_checked(path: &Path, rel: &str, options: &mut OpenOptions) -> Result<File, FactoryIoError> {
    #[cfg(unix)]
    options.custom_flags(read_flags());
    let file = options.open(path).map_err(|e| {
        if is_nofollow_error(&e) {
            FactoryIoError::refused(rel, Reason::SymlinkRefused, Some(rel.to_string()))
        } else {
            FactoryIoError::io(rel, &e)
        }
    })?;
    // The open succeeded on something; `fstat` says what, with no second
    // lookup by name. A FIFO would have blocked, which is why the lstat
    // pre-check runs first.
    let meta = file.metadata().map_err(|e| FactoryIoError::io(rel, &e))?;
    if !meta.is_file() {
        return Err(FactoryIoError::refused(rel, Reason::NotARegularFile, None));
    }
    Ok(file)
}

/// Open a `.factory` file for reading. `O_NOFOLLOW`, and a regular file.
pub fn open_for_read(root: &Path, relative: &str) -> Result<File, FactoryIoError> {
    let path = checked_path(root, relative)?;
    let rel = rel_of(relative);
    open_checked(&path, &rel, OpenOptions::new().read(true))
}

/// The bytes of a `.factory` file.
pub fn read(root: &Path, relative: &str) -> Result<Vec<u8>, FactoryIoError> {
    let rel = rel_of(relative);
    let mut file = open_for_read(root, relative)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| FactoryIoError::io(&rel, &e))?;
    Ok(bytes)
}

/// The text of a `.factory` file.
pub fn read_to_string(root: &Path, relative: &str) -> Result<String, FactoryIoError> {
    let rel = rel_of(relative);
    let bytes = read(root, relative)?;
    String::from_utf8(bytes).map_err(|_| {
        FactoryIoError::io(
            &rel,
            &std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            ),
        )
    })
}

/// The text of a `.factory` file, or `None` when there is none.
///
/// A missing file is the only absence this reports: a symlinked one is an
/// error, never an empty default.
pub fn read_to_string_optional(
    root: &Path,
    relative: &str,
) -> Result<Option<String>, FactoryIoError> {
    match read_to_string(root, relative) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.is_not_found() => Ok(None),
        Err(e) => Err(e),
    }
}

/// The bytes of a `.factory` file, or `None` when there is none.
pub fn read_optional(root: &Path, relative: &str) -> Result<Option<Vec<u8>>, FactoryIoError> {
    match read(root, relative) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.is_not_found() => Ok(None),
        Err(e) => Err(e),
    }
}

/// `lstat` of a `.factory` path: `None` when it does not exist, an error when
/// a component is a link or the path is not ours to touch.
pub fn symlink_metadata(root: &Path, relative: &str) -> Result<Option<Metadata>, FactoryIoError> {
    let path = checked_path(root, relative)?;
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(FactoryIoError::io(&rel_of(relative), &e)),
    }
}

/// Is this a regular file the factory may read? A symlink, a directory or a
/// refused path all answer `false` — callers that need the reason read it.
pub fn is_file(root: &Path, relative: &str) -> bool {
    matches!(symlink_metadata(root, relative), Ok(Some(m)) if m.is_file())
}

/// Is this a real directory under `.factory/`?
pub fn is_dir(root: &Path, relative: &str) -> bool {
    matches!(symlink_metadata(root, relative), Ok(Some(m)) if m.is_dir())
}

/// Refuse now if the destination exists and is not a regular file we own.
fn check_destination(root: &Path, relative: &str) -> Result<PathBuf, FactoryIoError> {
    let path = checked_path(root, relative)?;
    let rel = rel_of(relative);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                Err(FactoryIoError::refused(
                    &rel,
                    Reason::SymlinkRefused,
                    Some(rel.clone()),
                ))
            } else if !meta.is_file() {
                Err(FactoryIoError::refused(&rel, Reason::NotARegularFile, None))
            } else {
                Ok(path)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(e) => Err(FactoryIoError::io(&rel, &e)),
    }
}

/// Write `bytes` to a `.factory` file, truncating it where it stands.
///
/// Creates the file when it is absent; never renames a temporary over it, so
/// a symlink is refused rather than quietly replaced.
pub fn write_in_place(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), FactoryIoError> {
    let path = check_destination(root, relative)?;
    let rel = rel_of(relative);
    let mut file = open_checked(
        &path,
        &rel,
        OpenOptions::new().write(true).create(true).truncate(true),
    )?;
    file.write_all(bytes)
        .map_err(|e| FactoryIoError::io(&rel, &e))?;
    file.flush().map_err(|e| FactoryIoError::io(&rel, &e))
}

/// Write a `.factory` file that must not exist yet (`O_EXCL`).
pub fn create_new(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), FactoryIoError> {
    let path = check_destination(root, relative)?;
    let rel = rel_of(relative);
    let mut file = open_checked(&path, &rel, OpenOptions::new().write(true).create_new(true))?;
    file.write_all(bytes)
        .map_err(|e| FactoryIoError::io(&rel, &e))?;
    file.flush().map_err(|e| FactoryIoError::io(&rel, &e))
}

/// Create a directory under `.factory/` and every missing parent, refusing a
/// symlink anywhere on the way.
pub fn create_dir_all(root: &Path, relative: &str) -> Result<PathBuf, FactoryIoError> {
    let path = checked_path(root, relative)?;
    std::fs::create_dir_all(&path)
        .map_err(|e| FactoryIoError::io(&rel_of(relative), &e))
        .map(|_| path)
}

/// The parent directory of a `.factory` file, created if missing.
pub fn create_parent_dir(root: &Path, relative: &str) -> Result<(), FactoryIoError> {
    let parts = match components(relative) {
        Some(p) => p,
        None => {
            return Err(FactoryIoError::refused(
                relative,
                Reason::OutsideWorkspace,
                None,
            ))
        }
    };
    if parts.len() < 2 {
        return Ok(());
    }
    create_dir_all(root, &parts[..parts.len() - 1].join("/")).map(|_| ())
}

/// List a directory under `.factory/`.
pub fn read_dir(root: &Path, relative: &str) -> Result<std::fs::ReadDir, FactoryIoError> {
    let path = checked_path(root, relative)?;
    std::fs::read_dir(&path).map_err(|e| FactoryIoError::io(&rel_of(relative), &e))
}

/// Remove a directory tree under `.factory/`. A symlink at the root of the
/// tree is refused rather than unlinked.
pub fn remove_dir_all(root: &Path, relative: &str) -> Result<(), FactoryIoError> {
    let path = checked_path(root, relative)?;
    let rel = rel_of(relative);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(FactoryIoError::refused(
            &rel,
            Reason::SymlinkRefused,
            Some(rel.clone()),
        )),
        Ok(_) => std::fs::remove_dir_all(&path).map_err(|e| FactoryIoError::io(&rel, &e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(FactoryIoError::io(&rel, &e)),
    }
}

/// Unlink a regular file under `.factory/`. A symlink is refused rather than
/// unlinked: removing the link and leaving the target is a half-answer, and
/// the caller that is about to delete a file has just read it through
/// custody, which would already have refused.
pub fn remove_file(root: &Path, relative: &str) -> Result<(), FactoryIoError> {
    let path = check_destination(root, relative)?;
    let rel = rel_of(relative);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(FactoryIoError::io(&rel, &e)),
    }
}

/// Move `from` onto a `.factory` destination, after proving the destination
/// is not a symlink.
///
/// Only for the writer that needs two files to land together
/// (`context capture`). Everything else writes in place.
pub fn rename_into_place(root: &Path, relative: &str, from: &Path) -> Result<(), FactoryIoError> {
    let path = check_destination(root, relative)?;
    std::fs::rename(from, &path).map_err(|e| FactoryIoError::io(&rel_of(relative), &e))
}

/// Split a path the caller named on the command line into the workspace it
/// belongs to and its `.factory/…` relative, when it points inside one.
///
/// `inventory build --out .factory/inventory.json` and `--out
/// pilots/gitea/.factory/inventory.json` both land in custody; `--out
/// /tmp/scratch.json` is an ordinary file the caller asked for and does not.
pub fn split_workspace_path(path: &Path) -> Option<(PathBuf, String)> {
    let parts: Vec<Component> = path.components().collect();
    // A `..` anywhere makes the split unsound: the root would not be the
    // workspace the `.factory` component sits in.
    if parts.iter().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let at = parts.iter().rposition(
        |c| matches!(c, Component::Normal(os) if *os == std::ffi::OsStr::new(STATE_DIR)),
    )?;
    let relative = parts[at..]
        .iter()
        .map(|c| match c {
            Component::Normal(os) => os.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?
        .join("/");
    // The prefix keeps its own shape — absolute, `./`-anchored or bare.
    let mut root = PathBuf::new();
    for c in &parts[..at] {
        root.push(c.as_os_str());
    }
    Some((
        if root.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            root
        },
        relative,
    ))
}

/// Read a path the caller named: through custody when it points inside a
/// workspace's `.factory/`, as an ordinary file when it does not.
pub fn read_named_path(path: &Path) -> Result<Vec<u8>, String> {
    match split_workspace_path(path) {
        Some((root, relative)) => read(&root, &relative).map_err(String::from),
        None => std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e)),
    }
}

/// Write a path the caller named, in place, creating its parent directory.
pub fn write_named_path(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match split_workspace_path(path) {
        Some((root, relative)) => {
            create_parent_dir(&root, &relative)?;
            write_in_place(&root, &relative, bytes).map_err(String::from)
        }
        None => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, bytes).map_err(|e| format!("{}: {}", path.display(), e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".factory")).expect("mkdir");
        dir
    }

    #[test]
    fn reads_and_writes_a_regular_file() {
        let ws = workspace();
        write_in_place(ws.path(), ".factory/selection.yaml", b"include: true\n").unwrap();
        assert_eq!(
            read_to_string(ws.path(), ".factory/selection.yaml").unwrap(),
            "include: true\n"
        );
        write_in_place(ws.path(), ".factory/selection.yaml", b"x\n").unwrap();
        assert_eq!(read(ws.path(), ".factory/selection.yaml").unwrap(), b"x\n");
    }

    #[test]
    fn refuses_a_relative_that_is_not_under_factory() {
        let ws = workspace();
        for bad in [
            "",
            "/etc/passwd",
            "selection.yaml",
            ".factory/../selection.yaml",
            "../.factory/selection.yaml",
            ".",
        ] {
            let err = read(ws.path(), bad).unwrap_err();
            assert_eq!(err.reason, Reason::OutsideWorkspace, "{bad}");
        }
    }

    #[test]
    fn reports_a_missing_file_as_not_found() {
        let ws = workspace();
        let err = read(ws.path(), ".factory/nothing.json").unwrap_err();
        assert!(err.is_not_found());
        assert!(!err.is_refusal());
        assert_eq!(
            read_to_string_optional(ws.path(), ".factory/nothing.json").unwrap(),
            None
        );
    }

    #[test]
    fn refuses_a_directory_as_a_file() {
        let ws = workspace();
        create_dir_all(ws.path(), ".factory/evidence").unwrap();
        let err = read(ws.path(), ".factory/evidence").unwrap_err();
        assert_eq!(err.reason, Reason::NotARegularFile);
        assert!(err.to_string().contains(".factory/evidence"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_file_on_read_and_on_write() {
        let ws = workspace();
        let outside = ws.path().join("outside-secret");
        std::fs::write(&outside, "AWS_SECRET_ACCESS_KEY=hunter2\n").unwrap();
        std::os::unix::fs::symlink(&outside, ws.path().join(".factory/selection.yaml")).unwrap();

        let err = read(ws.path(), ".factory/selection.yaml").unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        let message = err.to_string();
        assert!(message.contains(".factory/selection.yaml"), "{message}");
        assert!(
            message.contains("a symlink, not a regular file"),
            "{message}"
        );
        assert!(!message.contains("outside-secret"), "{message}");

        let err = write_in_place(ws.path(), ".factory/selection.yaml", b"clobbered").unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "AWS_SECRET_ACCESS_KEY=hunter2\n",
            "the link target must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_directory_component() {
        let ws = workspace();
        let elsewhere = ws.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("selection.yaml"), "include: false\n").unwrap();
        std::fs::remove_dir_all(ws.path().join(".factory")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, ws.path().join(".factory")).unwrap();

        let err = read(ws.path(), ".factory/selection.yaml").unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        let message = err.to_string();
        assert!(message.contains(".factory/selection.yaml"), "{message}");
        assert!(
            message.contains(".factory is a symlink, not a directory"),
            "{message}"
        );
        assert!(!message.contains("elsewhere"), "{message}");

        assert!(create_dir_all(ws.path(), ".factory/samples").is_err());
        assert!(read_dir(ws.path(), ".factory").is_err());
        assert!(!is_file(ws.path(), ".factory/selection.yaml"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_nested_below_factory() {
        let ws = workspace();
        let outside = ws.path().join("vendor");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("swagger.upstream.json"), "{}").unwrap();
        std::os::unix::fs::symlink(&outside, ws.path().join(".factory/sources")).unwrap();

        let err = read(ws.path(), ".factory/sources/swagger.upstream.json").unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        assert!(err
            .to_string()
            .contains(".factory/sources is a symlink, not a directory"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_destination_for_rename_and_remove() {
        let ws = workspace();
        let outside = ws.path().join("outside.yaml");
        std::fs::write(&outside, "keep me\n").unwrap();
        std::os::unix::fs::symlink(&outside, ws.path().join(".factory/context.yaml")).unwrap();
        let tmp = ws.path().join(".factory/context.yaml.tmp");
        std::fs::write(&tmp, "new\n").unwrap();

        let err = rename_into_place(ws.path(), ".factory/context.yaml", &tmp).unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me\n");

        let dir_outside = ws.path().join("outside-dir");
        std::fs::create_dir_all(&dir_outside).unwrap();
        std::fs::write(dir_outside.join("keep"), "x").unwrap();
        std::os::unix::fs::symlink(&dir_outside, ws.path().join(".factory/samples")).unwrap();
        let err = remove_dir_all(ws.path(), ".factory/samples").unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        assert!(dir_outside.join("keep").exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_file_unlinks_a_regular_file_and_refuses_a_symlink() {
        let ws = workspace();
        write_in_place(ws.path(), ".factory/decisions.md", b"# Decisions\n").unwrap();
        remove_file(ws.path(), ".factory/decisions.md").unwrap();
        assert!(!is_file(ws.path(), ".factory/decisions.md"));
        // Removing what is not there is not an error: the caller's job is done.
        remove_file(ws.path(), ".factory/decisions.md").unwrap();

        let outside = ws.path().join("outside.md");
        std::fs::write(&outside, "keep me\n").unwrap();
        std::os::unix::fs::symlink(&outside, ws.path().join(".factory/decisions.md")).unwrap();
        let err = remove_file(ws.path(), ".factory/decisions.md").unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        assert!(ws.path().join(".factory/decisions.md").is_symlink());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me\n");
    }

    #[cfg(unix)]
    #[test]
    fn create_new_refuses_an_existing_symlink() {
        let ws = workspace();
        let outside = ws.path().join("outside.json");
        std::fs::write(&outside, "{}").unwrap();
        std::os::unix::fs::symlink(&outside, ws.path().join(".factory/inventory.json")).unwrap();
        let err = create_new(ws.path(), ".factory/inventory.json", b"[]").unwrap_err();
        assert_eq!(err.reason, Reason::SymlinkRefused);
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "{}");
    }

    #[test]
    fn splits_a_named_path_into_workspace_and_relative() {
        for (given, root, rel) in [
            (".factory/inventory.json", ".", ".factory/inventory.json"),
            (
                "pilots/gitea/.factory/inventory.json",
                "pilots/gitea",
                ".factory/inventory.json",
            ),
            (
                "ws/.factory/evidence/latest.json",
                "ws",
                ".factory/evidence/latest.json",
            ),
        ] {
            let (got_root, got_rel) = split_workspace_path(Path::new(given)).expect(given);
            assert_eq!(got_root, PathBuf::from(root), "{given}");
            assert_eq!(got_rel, rel, "{given}");
        }
        for plain in ["-", "/tmp/scratch.json", "out/inventory.json"] {
            assert!(split_workspace_path(Path::new(plain)).is_none(), "{plain}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_named_path_inside_factory_is_still_refused() {
        let ws = workspace();
        let outside = ws.path().join("outside.json");
        std::fs::write(&outside, "{}").unwrap();
        std::os::unix::fs::symlink(&outside, ws.path().join(".factory/inventory.json")).unwrap();
        let named = ws.path().join(".factory/inventory.json");
        assert!(read_named_path(&named)
            .unwrap_err()
            .contains(".factory/inventory.json"));
        assert!(write_named_path(&named, b"[]").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "{}");
    }

    #[test]
    fn creates_parents_and_nested_files() {
        let ws = workspace();
        create_parent_dir(ws.path(), ".factory/evidence/runs/1/log.txt").unwrap();
        write_in_place(ws.path(), ".factory/evidence/runs/1/log.txt", b"ok").unwrap();
        assert!(is_dir(ws.path(), ".factory/evidence/runs/1"));
        assert!(is_file(ws.path(), ".factory/evidence/runs/1/log.txt"));
        assert_eq!(
            read_to_string(ws.path(), ".factory/evidence/runs/1/log.txt").unwrap(),
            "ok"
        );
        remove_dir_all(ws.path(), ".factory/evidence/runs/1").unwrap();
        assert!(!is_dir(ws.path(), ".factory/evidence/runs/1"));
    }
}

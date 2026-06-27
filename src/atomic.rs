//! Atomic file writes: write to a sibling temp file, flush it to disk, then
//! rename it over the target. An interrupted or failed write can never leave a
//! truncated or half-written file — the previous good version stays in place
//! until the rename atomically swaps in the complete new one.
//!
//! This backs Quill's "never lose an author's words" guarantee (#15/#262):
//! every write of manuscript content — a chapter, the continuous-view window,
//! the book manifest — goes through here instead of a plain truncate-then-write
//! (`fs::write`), which exposes a window where a crash or disk-full mid-write
//! destroys the file outright.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Atomically write `bytes` to `path`.
///
/// Writes a sibling temp file in the **same directory** (so the final rename
/// stays on one filesystem and is therefore atomic), `sync_all`s it so the
/// bytes are durable before the swap, then renames it over `path`. On any
/// failure before the rename, the temp file is removed and `path` is left
/// untouched, so the previous good version always survives a failed write.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = temp_path(path);

    // Stage into the temp file, flush it durable, then swap it in — all in one
    // fallible step so a failure anywhere leaves a single cleanup path. The
    // inner scope drops (closes) the file before the rename.
    let staged = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        // Test-only seam (#265): simulate a crash *after* the temp is fully
        // written but *before* the rename — the case that distinguishes an
        // atomic save (target untouched) from a truncate-then-write (target
        // already destroyed). Production builds compile this out entirely.
        #[cfg(test)]
        if fail_point::should_fail_before_rename() {
            return Err(io::Error::other("injected pre-rename failure"));
        }
        fs::rename(&tmp, path)
    })();
    if let Err(e) = staged {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // Best-effort: fsync the directory so the rename itself is durable across a
    // power loss. Not every platform lets you open a directory as a file, so a
    // failure here is ignored — the bytes are already safely on disk.
    if let Ok(dir) = File::open(parent(path)) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The directory holding `path`, defaulting to the current directory for a bare
/// filename (whose `parent()` is the empty path).
fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// A hidden sibling temp path next to `path`, keyed by process id so concurrent
/// processes (e.g. parallel tests) never collide on the same temp file. Hidden
/// (dot-prefixed) so a leftover never shows up as a stray chapter file.
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "quill".into());
    parent(path).join(format!(".{name}.{}.tmp", std::process::id()))
}

/// Test-only fault injection for [`write`] (#265). One-shot and thread-local, so
/// the data-path suite can simulate a crash mid-save without touching the
/// filesystem's permissions, and parallel tests can't perturb each other.
/// Compiled out of production builds.
#[cfg(test)]
pub(crate) mod fail_point {
    use std::cell::Cell;

    thread_local! {
        static FAIL_BEFORE_RENAME: Cell<bool> = const { Cell::new(false) };
    }

    /// Arm a one-shot failure that fires inside the next [`super::write`] on this
    /// thread, after the temp file is written but before the rename.
    pub(crate) fn arm_pre_rename_failure() {
        FAIL_BEFORE_RENAME.with(|f| f.set(true));
    }

    /// Whether the current write should fail before its rename, consuming the
    /// armed flag (so it fires exactly once).
    pub(crate) fn should_fail_before_rename() -> bool {
        FAIL_BEFORE_RENAME.with(|f| f.replace(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::scratch_dir("atomic", name)
    }

    #[test]
    fn writes_and_reads_back() {
        let dir = tmpdir("read_back");
        let path = dir.join("chapter.md");
        write(&path, b"hello\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn overwrites_in_place() {
        let dir = tmpdir("overwrite");
        let path = dir.join("over.md");
        write(&path, b"first").unwrap();
        write(&path, b"second").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn leaves_no_temp_files_behind() {
        let dir = tmpdir("clean");
        let path = dir.join("clean.md");
        write(&path, b"body").unwrap();
        let strays: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "temp file left behind: {strays:?}");
    }

    // The core guarantee (#262): a write that fails leaves the previous good
    // version intact. We force the failure by making the target's directory
    // read-only so the temp file can't be created — then confirm the existing
    // file is byte-for-byte unchanged. Unix-only (relies on POSIX perms).
    #[cfg(unix)]
    #[test]
    fn failed_write_preserves_previous_version() {

        let parent = tmpdir("readonly").join("ro");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("chapter.md");
        write(&path, b"the good version\n").unwrap();

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
        let result = write(&path, b"a doomed half-written version");
        // Restore write perms so the temp dir can be cleaned up later.
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();

        // Under root the read-only bit is bypassed, so the write succeeds and
        // there's nothing to assert — skip rather than false-pass.
        if result.is_ok() {
            return;
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "the good version\n",
            "the previous good version must survive a failed write"
        );
    }
}

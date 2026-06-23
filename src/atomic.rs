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

    // Scope the file so it is flushed and closed before the rename.
    let written = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp, path) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // A unique directory per test, so parallel tests never observe each other's
    // transient temp files.
    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-atomic-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
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
    // file is byte-for-byte unchanged. Unix-only (relies on POSIX perms) and
    // skipped under root, which bypasses permission bits.
    #[cfg(unix)]
    #[test]
    fn failed_write_preserves_previous_version() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tmpdir("readonly").join("ro");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("chapter.md");
        write(&path, b"the good version\n").unwrap();

        // Running as root ignores the read-only bit, so the write would succeed
        // and there'd be nothing to assert — skip rather than false-pass.
        let is_root = unsafe { libc_getuid() } == 0;
        if is_root {
            return;
        }

        let readonly = fs::Permissions::from_mode(0o555);
        fs::set_permissions(&parent, readonly).unwrap();

        let result = write(&path, b"a doomed half-written version");

        // Restore write perms so the temp dir can be cleaned up later.
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(result.is_err(), "write into a read-only dir should fail");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "the good version\n",
            "the previous good version must survive a failed write"
        );
    }

    #[cfg(unix)]
    extern "C" {
        #[link_name = "getuid"]
        fn libc_getuid() -> u32;
    }
}

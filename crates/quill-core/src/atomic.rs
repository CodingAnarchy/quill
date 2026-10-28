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
/// untouched, so the previous good version always survives a failed write. The
/// parent directory must already exist.
///
/// When `path` is a symlink (a chapter symlinked into a synced/shared folder),
/// the write follows it: the temp is staged beside — and the rename lands on —
/// the link's **real target**, so the target's content updates and the link
/// itself is preserved. A plain rename onto the link path would instead replace
/// the link with a regular file and orphan the target, silently stale (#871).
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with(path, |f| f.write_all(bytes))
}

/// Atomically produce `path`'s contents by *streaming* into it, for a payload
/// too large to want buffered in memory first (e.g. a project archive zip,
/// #658).
///
/// Same guarantee as [`write`] — a sibling temp file in the same directory,
/// `sync_all`ed and renamed over `path`, so a failure anywhere leaves the
/// previous good version untouched — but instead of taking a `&[u8]`, it hands
/// `fill` the open temp [`File`] (which is `Write + Seek`, as a zip writer
/// wants) to write directly. The parent directory must already exist.
///
/// [`write`] is a thin `&[u8]` front for it.
pub fn write_with<F>(path: &Path, fill: F) -> io::Result<()>
where
    F: FnOnce(&mut File) -> io::Result<()>,
{
    stage_and_swap(path, Durability::Synced, fill)
}

/// Atomically replace a **best-effort hint** file — one whose loss costs only a
/// convenience, never the author's words: the temp + rename still means a reader
/// sees either the old record or the new one (never a torn one, barring a power
/// cut), but neither the temp file nor the directory is `fsync`ed.
///
/// Those two `fsync`s are the bulk of an [`write`]'s cost — on macOS each is an
/// `F_FULLFSYNC`, several milliseconds — and a hint written on every save (the
/// resume position, #588) would pay them every time for a record that is
/// rewritten moments later anyway. After a power loss the file may be missing,
/// stale, or empty; a hint's reader must treat all three as "no record" (as
/// [`crate::lastpos::read`] does). **Never** use this for manuscript content, the
/// manifest, annotations, or any other source of truth — those go through
/// [`write`].
pub fn write_hint(path: &Path, bytes: &[u8]) -> io::Result<()> {
    stage_and_swap(path, Durability::Hint, |f| f.write_all(bytes))
}

/// Whether [`stage_and_swap`] flushes the temp file and the directory to disk.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Durability {
    /// `fsync` the temp before the rename and the directory after it.
    Synced,
    /// Rename only — atomic against a crash of this process, not a power loss.
    Hint,
}

/// The single staging / (optional) fsync / rename sequence behind [`write`],
/// [`write_with`], and [`write_hint`].
fn stage_and_swap<F>(path: &Path, durability: Durability, fill: F) -> io::Result<()>
where
    F: FnOnce(&mut File) -> io::Result<()>,
{
    // Test-only seam: count this thread's atomic writes, so a guard can prove
    // the typing/idle path never does one on the UI thread.
    #[cfg(any(test, feature = "test-support"))]
    write_count::bump();
    let synced = durability == Durability::Synced;
    let path = &resolve_symlink(path)?;
    let tmp = temp_path(path);

    // Stage into the temp file, flush it durable, then swap it in — all in one
    // fallible step so a failure anywhere leaves a single cleanup path. The
    // inner scope drops (closes) the file before the rename.
    let staged = (|| {
        let mut f = File::create(&tmp)?;
        fill(&mut f)?;
        if synced {
            f.sync_all()?;
        }
        // Test-only seam (#265): simulate a crash *after* the temp is fully
        // written but *before* the rename — the case that distinguishes an
        // atomic save (target untouched) from a truncate-then-write (target
        // already destroyed). Production builds compile this out entirely.
        // Every entry point reaches it here, so a `write`, a `write_with`, and a
        // `write_hint` each count as exactly one armed step of the countdown.
        #[cfg(any(test, feature = "test-support"))]
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
    if synced {
        if let Ok(dir) = File::open(parent(path)) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// Create the directory `dir` itself — never its parents — when it's missing.
/// For the project's `.quill/` state folder, written from background writers
/// that may run a moment after the project folder was renamed or deleted: a
/// late write must then fail rather than resurrect the folder (as
/// `create_dir_all` would).
pub fn create_leaf_dir(dir: &Path) -> io::Result<()> {
    match fs::create_dir(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

/// Resolve `path` to the real file the write should land on: if `path` is a
/// symlink, follow it to its target so the temp is staged beside — and the
/// rename replaces — the target rather than the link (#871). A non-symlink (or a
/// missing path, the create-new case) is returned unchanged.
///
/// Only the final component is a link here (a chapter file symlinked into a
/// synced folder); [`fs::read_link`] gives that target, resolved relative to the
/// link's own directory so a relative link (`../shared/ch1.md`) points where it
/// should. A dangling link (target's directory is gone) surfaces as the write's
/// own error at `File::create`, never a silent replacement of the link.
pub fn resolve_symlink(path: &Path) -> io::Result<PathBuf> {
    // Follow a chain of links to the real file (each hop staged+renamed on the
    // final target's filesystem). Bounded so a self-referential link cycle
    // errors out instead of looping forever.
    resolve_symlink_depth(path.to_path_buf(), 32)
}

/// [`resolve_symlink`] with a remaining-hops budget, so a link cycle terminates
/// with an error rather than recursing without bound.
fn resolve_symlink_depth(path: PathBuf, hops: u32) -> io::Result<PathBuf> {
    if hops == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many levels of symbolic links",
        ));
    }
    // `symlink_metadata` does not follow the link, so this distinguishes a
    // symlink from a regular file; a `NotFound` (nothing there yet) is the
    // ordinary create case and leaves the path unchanged.
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = fs::read_link(&path)?;
            let resolved = if target.is_absolute() {
                target
            } else {
                parent(&path).join(target)
            };
            resolve_symlink_depth(resolved, hops - 1)
        }
        _ => Ok(path),
    }
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
        .map_or_else(|| "quill".into(), |n| n.to_string_lossy().into_owned());
    parent(path).join(format!(".{name}.{}.tmp", std::process::id()))
}

/// Test-only fault injection for [`write`] (#265). One-shot and thread-local, so
/// the data-path suite can simulate a crash mid-save without touching the
/// filesystem's permissions, and parallel tests can't perturb each other.
/// Compiled out of production builds.
#[cfg(any(test, feature = "test-support"))]
pub mod fail_point {
    use std::cell::Cell;

    thread_local! {
        static FAIL_BEFORE_RENAME: Cell<bool> = const { Cell::new(false) };
        // Countdown to a one-shot failure on the Nth write: each write decrements
        // it, and it fires (once) when it reaches 1. `0` means disarmed, so a
        // multi-write operation can be failed at a chosen step without arming
        // between the internal writes it doesn't expose.
        static FAIL_ON_NTH: Cell<usize> = const { Cell::new(0) };
        // Countdown to a one-shot failure on the Nth *swap rename* of
        // `Project::write_chapters_atomic` (#262/#974), independent of the
        // staging-write countdowns above. This targets the bare-rename swap loop
        // that runs *after* every temp is durably staged — the window where a
        // later rename can fail with an earlier one already applied — which the
        // staging fail-points never reach. `0` means disarmed.
        static FAIL_SWAP_ON_NTH: Cell<usize> = const { Cell::new(0) };
    }

    /// Arm a one-shot failure that fires inside the next [`super::write`] on this
    /// thread, after the temp file is written but before the rename.
    pub fn arm_pre_rename_failure() {
        FAIL_BEFORE_RENAME.with(|f| f.set(true));
    }

    /// Arm a one-shot failure that fires on the `n`th [`super::write`] on this
    /// thread (1-based), after that write's temp file is written but before its
    /// rename. Lets a test fault a specific step of a multi-write operation
    /// (e.g. [`Project::import_chapters`](crate::project::Project::import_chapters))
    /// without arming between the writes it performs internally.
    pub fn arm_pre_rename_failure_on_nth(n: usize) {
        FAIL_ON_NTH.with(|f| f.set(n));
    }

    /// Whether the current write should fail before its rename, consuming the
    /// one-shot arming (either the next-write flag or the Nth-write countdown) so
    /// it fires exactly once.
    pub fn should_fail_before_rename() -> bool {
        if FAIL_BEFORE_RENAME.with(|f| f.replace(false)) {
            return true;
        }
        FAIL_ON_NTH.with(|f| match f.get() {
            0 => false,
            1 => {
                f.set(0);
                true
            }
            n => {
                f.set(n - 1);
                false
            }
        })
    }

    /// Arm a one-shot failure that fires on the `n`th swap rename (1-based) of the
    /// next [`Project::write_chapters_atomic`](crate::project::Project::write_chapters_atomic)
    /// on this thread — the bare-rename swap loop that runs after every temp is
    /// staged. Lets a test drive the partial-swap window (an earlier rename applied,
    /// a later one failing) that the staging fail-points can't reach.
    pub fn arm_swap_failure_on_nth(n: usize) {
        FAIL_SWAP_ON_NTH.with(|f| f.set(n));
    }

    /// Whether the current swap rename should fail, consuming the one-shot
    /// countdown so it fires exactly once on the armed step.
    pub fn should_fail_swap() -> bool {
        FAIL_SWAP_ON_NTH.with(|f| match f.get() {
            0 => false,
            1 => {
                f.set(0);
                true
            }
            n => {
                f.set(n - 1);
                false
            }
        })
    }
}

/// Test-only count of atomic writes (any entry point — [`write`], [`write_with`],
/// [`write_hint`]) attempted on **this thread**. The UI-thread guard drives the
/// typing and idle path and asserts this stays put: a durable write there blocks
/// typing for as long as the disk takes, so it must happen on a worker thread
/// (whose writes count on that thread, not the caller's). Compiled out of
/// production builds.
#[cfg(any(test, feature = "test-support"))]
pub mod write_count {
    use std::cell::Cell;

    thread_local! {
        static WRITES: Cell<usize> = const { Cell::new(0) };
    }

    /// Record one atomic write on this thread.
    pub(super) fn bump() {
        WRITES.with(|w| w.set(w.get() + 1));
    }

    /// How many atomic writes this thread has attempted.
    pub fn get() -> usize {
        WRITES.with(Cell::get)
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
            .filter_map(std::result::Result::ok)
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

    // #871: a chapter symlinked into a synced/shared folder must have its *target*
    // updated by a save, and the link itself preserved — never replaced with a
    // regular file that orphans the target as silently stale. Unix-only (relies on
    // `std::os::unix::fs::symlink`); the CLAUDE.md platforms (macOS/Linux) have it.
    #[cfg(unix)]
    #[test]
    fn writing_through_a_symlink_updates_the_target_not_the_link() {
        use std::os::unix::fs::symlink;

        let dir = tmpdir("symlink");
        let target = dir.join("real-chapter.md");
        let link = dir.join("chapter.md");
        write(&target, b"original\n").unwrap();
        symlink(&target, &link).unwrap();

        write(&link, b"revised\n").unwrap();

        // The link is still a link, and the real target carries the new content.
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the save must not replace the symlink with a regular file"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "revised\n");
        assert_eq!(fs::read_to_string(&link).unwrap(), "revised\n");
        // And no temp file is orphaned beside the target.
        let strays: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "temp file left behind: {strays:?}");
    }

    // A relative symlink target resolves against the link's own directory, not the
    // process cwd — otherwise a save through it would write to the wrong place.
    #[cfg(unix)]
    #[test]
    fn writing_through_a_relative_symlink_resolves_against_the_link_dir() {
        use std::os::unix::fs::symlink;

        let dir = tmpdir("symlink_rel");
        let sub = dir.join("synced");
        fs::create_dir_all(&sub).unwrap();
        let target = sub.join("real.md");
        write(&target, b"before\n").unwrap();
        let link = dir.join("chapter.md");
        // A link relative to `dir` (the link's parent), pointing into `synced/`.
        symlink(Path::new("synced/real.md"), &link).unwrap();

        write(&link, b"after\n").unwrap();

        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "after\n");
    }
}

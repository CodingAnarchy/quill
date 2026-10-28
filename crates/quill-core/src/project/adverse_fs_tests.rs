//! Graceful degradation under adverse filesystem conditions (#1101).
//!
//! Several adverse conditions are *believed* handled — by the atomic-write layer
//! (`atomic.rs`) for saves and by the read-error distinction in `io.rs` for reads
//! — but had no explicit test at the [`Project`] layer. This module exercises them
//! end-to-end on real, hermetic filesystem conditions in a temp dir:
//!
//! - A **save under a read-only project directory** fails with a clear error and
//!   leaves the PRIOR on-disk version byte-for-byte intact (the atomic layer stages
//!   to a temp file + rename, so a failed write must never truncate the original).
//!   Read-only stands in for a true disk-full (ENOSPC) mid-save: ENOSPC can't be
//!   simulated portably, but it exercises the *same* "save fails, prior version
//!   intact" guarantee — a `File::create`/write that fails before the rename.
//! - Opening a project whose **manifest references a missing chapter file**
//!   degrades gracefully: no panic, the missing file reads as an empty chapter
//!   (the legitimate "starts empty" case), and a *hard* read error on that path is
//!   distinguished from empty via `try_chapter_body`.
//! - An **unreadable project directory** (chmod 0) is *reported* on open — a clear
//!   `Err` naming the manifest path, never a panic.
//!
//! The permission-based tests are Unix-only (POSIX perms) and skipped under root,
//! which bypasses the read-only/permission bits and would false-pass. Kept in their
//! own file so `tests.rs` / `hardening_tests.rs` stay under the ~1,000-line
//! convention.

use std::os::unix::fs::PermissionsExt;

use super::*;

fn temp_dir(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("proj-adverse", name)
}

/// Whether the test process is root (euid 0). Root bypasses the read-only and
/// permission bits the chmod-based tests rely on, so those tests skip-with-note
/// under root rather than false-passing on a write/read that unexpectedly succeeds.
fn is_root() -> bool {
    // SAFETY: `geteuid` is always safe to call and never fails.
    unsafe { libc::geteuid() == 0 }
}

/// Seed a minimal one-chapter project on disk. The chapter carries real prose so a
/// truncation or corruption of the original is detectable byte-for-byte.
fn seed_project(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        Project::manifest_path(dir),
        "title = \"Adverse\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("one.md"),
        "# One\n\nThe night was dark and the road was long indeed and true.\n",
    )
    .unwrap();
}

// --- Save under an adverse (read-only / ENOSPC-analog) condition -------------

// A chapter save under a read-only project directory must fail with a clear error
// and leave the PRIOR on-disk version byte-for-byte intact. The atomic layer stages
// to a sibling temp file and renames, so a `File::create` that can't create the temp
// (read-only dir, or a full disk) aborts before the rename — the original is never
// truncated. This stands in for a true disk-full (ENOSPC) mid-save, which can't be
// simulated portably but hits the same "fails before the rename" path.
#[cfg(unix)]
#[test]
fn a_save_under_a_read_only_dir_fails_and_keeps_the_prior_version() {
    if is_root() {
        eprintln!("skipping: root bypasses the read-only bit; can't force the write to fail");
        return;
    }
    let dir = temp_dir("save-readonly-chapter");
    seed_project(&dir);
    let path = dir.join("one.md");
    let prior = fs::read(&path).unwrap();

    // Make the project directory read-only so the atomic temp file can't be created.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = atomic::write(&path, b"a doomed half-written revision");
    // Restore write perms so the scratch dir can be cleaned up.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

    let err = result.expect_err("a save into a read-only dir must fail, not silently succeed");
    // The error names a real problem the author can act on (permission / read-only),
    // not an opaque success.
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied,
        "a read-only-dir save reports a permission failure: {err}"
    );
    // The core guarantee: the previous good version survives byte-for-byte — the
    // failed write neither truncated nor corrupted the original.
    assert_eq!(
        fs::read(&path).unwrap(),
        prior,
        "the prior on-disk version must survive a failed save untouched"
    );

    fs::remove_dir_all(&dir).ok();
}

// The same guarantee routed through the real save path: `save_manifest` under a
// read-only project dir fails and leaves the prior `book.toml` intact. This proves
// the Project layer (not just `atomic::write` in isolation) degrades gracefully —
// the author's on-disk manifest is never lost to a failed save.
#[cfg(unix)]
#[test]
fn a_manifest_save_under_a_read_only_dir_keeps_the_prior_manifest() {
    if is_root() {
        eprintln!("skipping: root bypasses the read-only bit; can't force the write to fail");
        return;
    }
    let dir = temp_dir("save-readonly-manifest");
    seed_project(&dir);
    let manifest_path = Project::manifest_path(&dir);
    let prior = fs::read(&manifest_path).unwrap();

    let mut project = Project::open(&dir).unwrap();
    project.manifest.title = "Renamed In Memory".to_string();

    // Read-only dir: the atomic manifest write can't stage its temp file.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = project.save_manifest();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

    let err = result.expect_err("a manifest save into a read-only dir must fail");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied,
        "the failed manifest save reports a permission failure: {err}"
    );
    // The prior manifest on disk is untouched — the in-memory rename never reached it.
    assert_eq!(
        fs::read(&manifest_path).unwrap(),
        prior,
        "the prior book.toml must survive a failed save byte-for-byte"
    );

    // Reopening reads the prior title, not the un-saved in-memory rename.
    drop(project);
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.title, "Adverse");

    fs::remove_dir_all(&dir).ok();
}

// --- Opening a project whose manifest references a missing chapter -----------

// A manifest that references a chapter file which does not exist on disk (deleted,
// renamed, or never written) must open without a panic, and the missing chapter
// reads as an empty body — the legitimate "starts empty" case, degrading gracefully
// rather than crashing or reporting phantom content.
#[test]
fn opening_with_a_missing_chapter_file_degrades_gracefully() {
    let dir = temp_dir("missing-chapter-open");
    fs::create_dir_all(&dir).unwrap();
    // Manifest references two chapters but only the first exists on disk; `two.md`
    // was never written (a deleted/renamed chapter the manifest still points at).
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Adverse\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"missing-two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "# One\n\nReal prose lives here.\n").unwrap();

    // Open must not panic despite the dangling manifest reference.
    let project = Project::open(&dir).unwrap();
    assert_eq!(project.manifest.chapters.len(), 2, "both entries loaded");

    // The present chapter reads its real prose; the missing one degrades to empty.
    assert!(project.chapter_body(0).contains("Real prose"));
    assert_eq!(
        project.chapter_body(1),
        "",
        "a manifest-referenced missing chapter reads as an empty body, not a panic"
    );
    // A missing file is the "starts empty" case (Ok), distinct from a hard read error.
    assert_eq!(
        project.try_chapter_body(1).unwrap(),
        "",
        "a missing chapter is Ok(empty), not an Err"
    );
    // Its cached word count is 0 — the missing file didn't corrupt the rollup.
    assert_eq!(project.chapter_words(1), 0);

    fs::remove_dir_all(&dir).ok();
}

// A chapter whose path is *unreadable* (a genuine hard read error, not a missing
// file) surfaces as an `Err` on access — distinguished from the empty/missing case —
// so a caller never mistakes an unreadable chapter for real emptiness. Unix-only
// (uses a directory in the file's place to force a hard read error).
#[cfg(unix)]
#[test]
fn an_unreadable_chapter_file_surfaces_an_error_on_access() {
    let dir = temp_dir("unreadable-chapter");
    seed_project(&dir);
    let project = Project::open(&dir).unwrap();

    // Replace the chapter file with a directory: reading it is a hard error, not a
    // NotFound, so it must not collapse to an empty "starts empty" body.
    let chapter_path = dir.join("one.md");
    fs::remove_file(&chapter_path).unwrap();
    fs::create_dir(&chapter_path).unwrap();

    let err = project
        .try_chapter_body(0)
        .expect_err("an unreadable chapter is a hard error, distinct from empty");
    assert_ne!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "the hard read error is distinguished from a missing file: {err}"
    );

    fs::remove_dir(&chapter_path).ok();
    fs::remove_dir_all(&dir).ok();
}

// --- Opening an unreadable project directory ---------------------------------

// An unreadable project directory (chmod 0) is reported on open — a clear `Err`
// whose message names the manifest path — never a panic. Under the read-error fix
// (#1101) the bare `io::Error` from `read_to_string` is wrapped so the author can
// see *which* file couldn't be read. Unix-only; skipped under root (which bypasses
// the permission bits).
#[cfg(unix)]
#[test]
fn opening_an_unreadable_project_dir_is_reported() {
    if is_root() {
        eprintln!("skipping: root bypasses directory permissions; the dir stays readable");
        return;
    }
    let dir = temp_dir("unreadable-dir");
    seed_project(&dir);

    // Make the whole project dir unreadable/untraversable.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let result = Project::open(&dir);
    // Restore perms so the scratch dir can be inspected and cleaned up.
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

    // `Project` isn't `Debug`, so destructure rather than `expect_err`.
    let Err(err) = result else {
        panic!("an unreadable project dir must be reported, not opened");
    };
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied,
        "the report is a permission failure: {err}"
    );
    // The message names the manifest path so the author knows what couldn't be read.
    let msg = err.to_string();
    assert!(
        msg.contains("book.toml"),
        "the error names the unreadable file: {msg}"
    );

    fs::remove_dir_all(&dir).ok();
}

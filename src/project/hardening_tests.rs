//! Write-layer hardening tests (#871): the instance lock and the read-error
//! distinction, at the [`Project`] layer. Kept in their own file so `tests.rs`
//! stays under the ~1,000-line convention. The atomic-write symlink coverage
//! lives with `atomic.rs`; the recovery-offer coverage with the app tests.

use super::lock::{self, LockOutcome};
use super::*;

fn temp_dir(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("proj-hardening", name)
}

/// Seed a minimal one-chapter project on disk and return its dir. The chapter has
/// real prose so its cached word count is non-zero.
fn seed_project(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        Project::manifest_path(dir),
        "title = \"Locked\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("one.md"),
        "# One\n\nThe night was dark and the road was long indeed and true.\n",
    )
    .unwrap();
}

// --- Gap 2: instance lock ---------------------------------------------------

// A project opened while a *live other-process* lock is present opens read-only
// and reports the holder, rather than racing that instance's writes. We stand in
// a lockfile owned by pid 1 (init/launchd, always alive on Unix) — the same path
// a real second instance's lockfile drives — since a unit test can't fork a
// second Quill.
#[cfg(unix)]
#[test]
fn opening_an_already_locked_project_is_read_only() {
    let dir = temp_dir("busy-open");
    seed_project(&dir);
    // A live, other-process lock already in place (pid 1 on this host).
    fs::create_dir_all(dir.join(".quill")).unwrap();
    let body = format!("1\n{}\n0\n", host());
    fs::write(dir.join(".quill").join("lock"), body).unwrap();

    let project = Project::open(&dir).unwrap();
    assert!(
        project.is_read_only(),
        "a project held by another live instance opens read-only"
    );
    assert!(
        project.busy_holder().is_some_and(|h| h.contains("pid 1")),
        "the read-only project names the holder for a warning"
    );

    // And a save is refused rather than clobbering the other instance's files.
    let err = project.save_manifest().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    fs::remove_dir_all(&dir).ok();
}

// The ordinary case: opening a free project takes the lock, writes are allowed,
// and dropping the project releases the lock so the next open succeeds.
#[test]
fn opening_a_free_project_takes_the_lock_and_can_write() {
    let dir = temp_dir("free-open");
    seed_project(&dir);

    let lockfile = dir.join(".quill").join("lock");
    {
        let mut project = Project::open(&dir).unwrap();
        assert!(!project.is_read_only(), "a free project opens writable");
        assert!(lockfile.exists(), "the lock is held while open");
        project.manifest.title = "Renamed".to_string();
        project.save_manifest().unwrap(); // writable
    }
    // Dropping the project released the lock.
    assert!(
        !lockfile.exists(),
        "dropping the project removes the instance lock"
    );
    // So a fresh open holds it again and reads the saved title.
    let reopened = Project::open(&dir).unwrap();
    assert!(!reopened.is_read_only());
    assert_eq!(reopened.manifest.title, "Renamed");

    fs::remove_dir_all(&dir).ok();
}

/// This host's name, matching the lock module's own derivation, so a seeded
/// lockfile is recognized as same-host.
fn host() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "unknown-host".to_string())
}

// A same-process reopen (the shape of an in-place project rename, which moves the
// folder with its lockfile and reopens it) must not be blocked by our own lock —
// otherwise the renamed project would open read-only.
#[test]
fn a_same_process_reopen_is_not_blocked_by_our_own_lock() {
    let dir = temp_dir("self-reopen");
    seed_project(&dir);
    let held = match lock::acquire(&dir) {
        LockOutcome::Held(l) => l,
        LockOutcome::Busy { .. } => panic!("first acquire should hold the lock"),
    };
    // Reopening the same project in the same process reclaims rather than blocks.
    let project = Project::open(&dir).unwrap();
    assert!(
        !project.is_read_only(),
        "an in-process reopen is us, not a competitor — not read-only"
    );
    project.save_manifest().unwrap();
    drop(held);
    fs::remove_dir_all(&dir).ok();
}

// --- Gap 3: hard read error vs. empty/NotFound ------------------------------

// A hard chapter-read failure must not zero-word-cache the chapter or drop its
// metadata (which would corrupt the word/TODO rollups). The last good cache is
// kept until the next successful read.
#[test]
fn a_hard_chapter_read_error_keeps_the_last_good_cache() {
    let dir = temp_dir("cache-read-fail");
    seed_project(&dir);
    let mut project = Project::open(&dir).unwrap();
    let good_words = project.chapter_words(0);
    let good_total = project.words_total();
    assert!(good_words > 0, "the chapter cached real words at open");

    // Make the chapter file unreadable (swap it for a directory — a hard read
    // error, distinct from a missing file).
    let chapter_path = dir.join("one.md");
    fs::remove_file(&chapter_path).unwrap();
    fs::create_dir(&chapter_path).unwrap();

    project.read_chapter_cache(0);
    assert_eq!(
        project.chapter_words(0),
        good_words,
        "a hard read error keeps the last good word count, not 0"
    );
    assert_eq!(
        project.words_total(),
        good_total,
        "the running total is not corrupted by a failed read"
    );

    fs::remove_dir(&chapter_path).ok();
    fs::remove_dir_all(&dir).ok();
}

// A legitimately empty (or missing) chapter caches as 0 words — the read-error
// handling must not confuse "empty" with "unreadable".
#[test]
fn an_empty_chapter_caches_as_zero_words() {
    let dir = temp_dir("cache-empty");
    seed_project(&dir);
    let mut project = Project::open(&dir).unwrap();

    // Truncate the chapter to empty (a legitimate empty chapter, readable).
    fs::write(dir.join("one.md"), "").unwrap();
    project.read_chapter_cache(0);
    assert_eq!(
        project.chapter_words(0),
        0,
        "an empty chapter legitimately caches 0 words"
    );

    // And a missing file (NotFound) is likewise empty, not an error.
    fs::remove_file(dir.join("one.md")).unwrap();
    project.read_chapter_cache(0);
    assert_eq!(
        project.chapter_words(0),
        0,
        "a missing chapter caches 0 words"
    );

    fs::remove_dir_all(&dir).ok();
}

// `try_chapter_body` distinguishes a hard read error (`Err`) from an empty or
// missing file (`Ok("")`) — the distinction the recovery offer relies on.
#[test]
fn try_chapter_body_distinguishes_error_from_empty() {
    let dir = temp_dir("try-body");
    seed_project(&dir);
    let project = Project::open(&dir).unwrap();

    // Readable, non-empty: Ok with the body.
    assert!(project.try_chapter_body(0).unwrap().contains("dark"));

    // Empty file: Ok(""), not an error.
    fs::write(dir.join("one.md"), "").unwrap();
    assert_eq!(project.try_chapter_body(0).unwrap(), "");

    // Unreadable (a directory in its place): a hard error.
    fs::remove_file(dir.join("one.md")).unwrap();
    fs::create_dir(dir.join("one.md")).unwrap();
    assert!(
        project.try_chapter_body(0).is_err(),
        "a hard read error is distinguished from an empty file"
    );

    fs::remove_dir(dir.join("one.md")).ok();
    fs::remove_dir_all(&dir).ok();
}

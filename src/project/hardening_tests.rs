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

// --- #890: idle re-check reclaims a lock that goes stale after open ----------

// A project opened read-only against a live holder that later crashes must
// reclaim the now-stale lock on the idle re-check and become writable — the
// transparent recovery the ticket asks for. We open against a live-looking lock
// (pid 1), overwrite it with a dead pid to model the holder crashing, then
// re-check: the project flips to writable and can save.
#[cfg(unix)]
#[test]
fn a_stale_lock_is_reclaimed_on_idle_recheck() {
    let dir = temp_dir("recheck-reclaim");
    seed_project(&dir);
    fs::create_dir_all(dir.join(".quill")).unwrap();
    // A live holder at open (pid 1, always alive on Unix): opens read-only.
    fs::write(
        dir.join(".quill").join("lock"),
        format!("1\n{}\n0\n", host()),
    )
    .unwrap();

    let mut project = Project::open(&dir).unwrap();
    assert!(
        project.is_read_only(),
        "opens read-only against a live holder"
    );
    // The holder crashes: its lock is now a dead same-host pid (2^30 is far above
    // any live pid). The re-check must reclaim it.
    fs::write(
        dir.join(".quill").join("lock"),
        format!("{}\n{}\n0\n", 1 << 30, host()),
    )
    .unwrap();

    assert!(
        project.recheck_lock(),
        "a now-dead same-host lock is reclaimed on re-check"
    );
    assert!(
        !project.is_read_only(),
        "the reclaimed project is writable again"
    );
    assert!(
        project.busy_holder().is_none(),
        "the busy-holder note is cleared once reclaimed"
    );
    project.save_manifest().unwrap(); // writable now
                                      // And the lock now records our pid, not the crashed holder's.
    assert_eq!(
        lock::recorded_pid(&dir),
        Some(std::process::id() as i32),
        "the reclaimed lock records our pid"
    );

    fs::remove_dir_all(&dir).ok();
}

// The idle re-check must NOT steal a lock still held by a live instance: no
// regression to the #871 read-only protection.
#[cfg(unix)]
#[test]
fn a_live_holder_is_respected_on_idle_recheck() {
    let dir = temp_dir("recheck-live");
    seed_project(&dir);
    fs::create_dir_all(dir.join(".quill")).unwrap();
    fs::write(
        dir.join(".quill").join("lock"),
        format!("1\n{}\n0\n", host()),
    )
    .unwrap();

    let mut project = Project::open(&dir).unwrap();
    assert!(project.is_read_only());
    assert!(
        !project.recheck_lock(),
        "a live holder must keep the project read-only on re-check"
    );
    assert!(project.is_read_only(), "still read-only after the re-check");
    assert_eq!(
        lock::recorded_pid(&dir),
        Some(1),
        "the live holder's lock is untouched"
    );

    fs::remove_dir_all(&dir).ok();
}

// A different-host lock stays honored on re-check: we can't probe another
// machine's processes, so it's never assumed stale.
#[test]
fn a_different_host_lock_stays_read_only_on_idle_recheck() {
    let dir = temp_dir("recheck-otherhost");
    seed_project(&dir);
    fs::create_dir_all(dir.join(".quill")).unwrap();
    // Even a "live-looking" pid is honored when the host isn't ours.
    fs::write(
        dir.join(".quill").join("lock"),
        format!("{}\nsome-other-machine\n0\n", std::process::id()),
    )
    .unwrap();

    let mut project = Project::open(&dir).unwrap();
    assert!(project.is_read_only());
    assert!(
        !project.recheck_lock(),
        "a different-host lock must stay honored on re-check"
    );
    assert!(project.is_read_only(), "still read-only after the re-check");

    fs::remove_dir_all(&dir).ok();
}

// The re-check is a no-op on a project we already own writably: nothing to
// reclaim, and it never touches the lock we hold.
#[test]
fn recheck_is_a_no_op_on_a_writable_project() {
    let dir = temp_dir("recheck-noop");
    seed_project(&dir);

    let mut project = Project::open(&dir).unwrap();
    assert!(!project.is_read_only(), "opens writable");
    assert!(
        !project.recheck_lock(),
        "a writable project has nothing to reclaim"
    );
    assert!(!project.is_read_only(), "still writable");

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

// --- write_chapters_atomic swap loop (#262/#974) ----------------------------

/// Seed a minimal two-chapter project so the multi-file atomic write has two
/// destinations to swap. Each chapter has distinct prose so a half-updated state
/// is detectable.
fn seed_two_chapter_project(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        Project::manifest_path(dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "# One\n\nOriginal one prose here.\n").unwrap();
    fs::write(dir.join("two.md"), "# Two\n\nOriginal two prose here.\n").unwrap();
}

// The final swap of `write_chapters_atomic` must be all-or-nothing: if a *later*
// rename fails with an earlier one already applied, the applied one is rolled back
// so no chapter is left half-updated (#262). Before this fix the loop was a bare
// `fs::rename` per file with no rollback, so a mid-loop failure durably rewrote the
// first chapter and left the second stale — the split state the feature promises to
// prevent. We arm the new swap fail-point on the second rename.
#[test]
fn write_chapters_atomic_rolls_back_a_partial_swap() {
    let dir = temp_dir("chapters-atomic-partial-swap");
    seed_two_chapter_project(&dir);
    let one_before = fs::read_to_string(dir.join("one.md")).unwrap();
    let two_before = fs::read_to_string(dir.join("two.md")).unwrap();
    let mut project = Project::open(&dir).unwrap();

    // Fail the *second* swap rename, after the first has already been applied.
    crate::atomic::fail_point::arm_swap_failure_on_nth(2);
    let result = project.write_chapters_atomic(&[
        (0, "# One\n\nREWRITTEN one.\n".to_string()),
        (1, "# Two\n\nREWRITTEN two.\n".to_string()),
    ]);

    assert!(result.is_err(), "the partial swap surfaces as an error");
    // Neither chapter is left in the rewritten state — the first swap was rolled
    // back, the second never happened. All-or-nothing.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        one_before,
        "the first chapter's swap was rolled back to its original"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        two_before,
        "the second chapter was never rewritten"
    );
    // No stray temp files were orphaned.
    let strays: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(strays.is_empty(), "temp file left behind: {strays:?}");

    fs::remove_dir_all(&dir).ok();
}

// A chapter file symlinked into a synced/shared folder must have the multi-file
// atomic write follow the link to its *target* and preserve the link itself —
// never replace it with a regular file that orphans the target as silently stale
// (#871). Before this fix the swap was a bare rename onto the link path, which
// replaced the link. Unix-only (relies on `std::os::unix::fs::symlink`).
#[cfg(unix)]
#[test]
fn write_chapters_atomic_writes_through_a_symlinked_chapter() {
    // Scoped to this `#[cfg(unix)]` test: the fn doesn't exist on other platforms.
    use std::os::unix::fs::symlink;

    let dir = temp_dir("chapters-atomic-symlink");
    fs::create_dir_all(&dir).unwrap();
    let synced = dir.join("synced");
    fs::create_dir_all(&synced).unwrap();
    // The real chapter file lives in the synced folder; `two.md` is a link to it.
    let real_target = synced.join("real-two.md");
    fs::write(&real_target, "# Two\n\nOriginal two prose here.\n").unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "# One\n\nOriginal one prose here.\n").unwrap();
    symlink(&real_target, dir.join("two.md")).unwrap();

    let mut project = Project::open(&dir).unwrap();
    project
        .write_chapters_atomic(&[
            (0, "# One\n\nREWRITTEN one.\n".to_string()),
            (1, "# Two\n\nREWRITTEN two.\n".to_string()),
        ])
        .unwrap();

    // The link is still a link, and the real target carries the new content.
    assert!(
        fs::symlink_metadata(dir.join("two.md"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the atomic write must not replace the symlink with a regular file"
    );
    assert_eq!(
        fs::read_to_string(&real_target).unwrap(),
        "# Two\n\nREWRITTEN two.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "# One\n\nREWRITTEN one.\n"
    );

    fs::remove_dir_all(&dir).ok();
}

// --- #1144: refuse a manifest from a newer build ----------------------------

// A manifest whose `format_version` is *newer* than this build must fail to open
// rather than load a lossy view (whose next save would strip the future fields).
// The error is `InvalidData` — not `NotFound` — so the re-create path callers
// switch on stays intact, and the message names both versions.
#[test]
fn opening_a_future_format_version_is_refused() {
    let dir = temp_dir("future-format-version");
    fs::create_dir_all(&dir).unwrap();
    let future = manifest::CURRENT_FORMAT_VERSION + 1;
    fs::write(
        Project::manifest_path(&dir),
        format!(
            "format_version = {future}\ntitle = \"From The Future\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n"
        ),
    )
    .unwrap();
    fs::write(dir.join("one.md"), "# One\n\nProse from a newer Quill.\n").unwrap();

    let err = Project::open(&dir)
        .err()
        .expect("a newer format version must not open");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::InvalidData,
        "the ceiling check reports InvalidData, not NotFound — the re-create path is preserved"
    );
    let msg = err.to_string();
    assert!(
        msg.contains(&future.to_string())
            && msg.contains(&manifest::CURRENT_FORMAT_VERSION.to_string()),
        "the message names both the found and supported versions: {msg}"
    );

    fs::remove_dir_all(&dir).ok();
}

// The current version — and an older/versionless one — still opens and migrates
// exactly as before: the ceiling guard only trips strictly *above* the current
// version.
#[test]
fn opening_the_current_format_version_still_succeeds() {
    let dir = temp_dir("current-format-version");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        format!(
            "format_version = {}\ntitle = \"Current\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
            manifest::CURRENT_FORMAT_VERSION
        ),
    )
    .unwrap();
    fs::write(
        dir.join("one.md"),
        "# One\n\nProse at the current version.\n",
    )
    .unwrap();

    let project = Project::open(&dir).expect("the current version opens");
    assert_eq!(project.manifest.title, "Current");

    fs::remove_dir_all(&dir).ok();
}

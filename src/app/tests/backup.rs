//! Tests for automatic off-site backup to the Git remote (#887): the opt-in
//! cadence (on-save + timed interval), the enabled / remote gating, and the
//! failure-surfaces-its-cause-and-reschedules path.
//!
//! Hermetic and offline (#531): none of these push to a real remote. The cadence
//! and gating are driven with an injected clock, and the push outcome is a fake
//! (a plain `Ok`/`Err`) spawned through `spawn_backup_with_outcome_for_test`, so
//! `cargo test` never touches the network.

use super::*;
use crate::config::{self, BackupConfig};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

/// A backup config with the given remote-push knobs, saving a few lines per test.
/// The local-archive fields (#888) stay at their defaults (off), so these #887
/// tests exercise the remote push alone.
fn cfg(enabled: bool, on_save: bool, interval_mins: u64) -> BackupConfig {
    BackupConfig {
        enabled,
        on_save,
        interval_mins,
        ..Default::default()
    }
}

/// A backup config that turns *local* archive backups on with the given hourly
/// interval and retention count (#888); the remote push stays off.
fn local_cfg(interval_hours: u64, keep: usize) -> BackupConfig {
    BackupConfig {
        local_enabled: true,
        local_interval_hours: interval_hours,
        local_keep: keep,
        ..Default::default()
    }
}

#[test]
fn disabled_backup_never_marks_or_fires() {
    // The default: backup off. A save marks nothing, and the cadence hook is a
    // no-op — no push is ever spawned, no behavior change (#887).
    let (mut app, _dir) = two_chapter_app("backup-disabled");
    app.backup = cfg(false, true, 5);
    app.mark_backup_on_save();
    assert!(
        !app.backup_pending_for_test(),
        "a disabled backup doesn't even flag an on-save push"
    );
    app.tick_backup();
    assert!(
        !app.backup_in_flight_for_test(),
        "the cadence hook spawns nothing while disabled"
    );
}

#[test]
fn on_save_marks_a_pending_backup_only_when_enabled() {
    let (mut app, _dir) = two_chapter_app("backup-on-save-mark");
    app.backup = cfg(true, true, 0);
    app.mark_backup_on_save();
    assert!(
        app.backup_pending_for_test(),
        "an enabled on-save backup flags a pending push"
    );

    // With on_save opted out, a save owes nothing — only the timed interval drives it.
    app.backup = cfg(true, false, 30);
    app.backup_pending = false;
    app.mark_backup_on_save();
    assert!(
        !app.backup_pending_for_test(),
        "on_save = false: a save marks no backup"
    );
}

#[test]
fn a_pending_backup_is_due_regardless_of_the_clock() {
    let (mut app, _dir) = two_chapter_app("backup-pending-due");
    app.backup = cfg(true, true, 0); // no timed interval
    app.mark_backup_pending_for_test();
    assert!(
        app.backup_due_for_test(Instant::now()),
        "the on-save flag makes a backup due immediately"
    );
}

#[test]
fn the_timed_interval_becomes_due_only_after_it_elapses() {
    let (mut app, _dir) = two_chapter_app("backup-interval-due");
    app.backup = cfg(true, false, 10); // 10-minute cadence, no on-save

    // Freshly attempted: not yet due.
    app.age_backup_clock_for_test(0);
    assert!(
        !app.backup_due_for_test(Instant::now()),
        "not due right after an attempt"
    );

    // Just short of the interval: still not due.
    app.age_backup_clock_for_test(10 * 60 - 5);
    assert!(
        !app.backup_due_for_test(Instant::now()),
        "not due just before the interval"
    );

    // Past the interval: due.
    app.age_backup_clock_for_test(10 * 60 + 1);
    assert!(
        app.backup_due_for_test(Instant::now()),
        "due once the interval has elapsed"
    );
}

#[test]
fn a_zero_interval_is_never_due_on_the_clock_alone() {
    let (mut app, _dir) = two_chapter_app("backup-no-interval");
    app.backup = cfg(true, true, 0); // on-save only, timed trigger off
    app.age_backup_clock_for_test(60 * 60 * 24); // a day idle
    assert!(
        !app.backup_due_for_test(Instant::now()),
        "with interval_mins = 0 the clock never makes a backup due"
    );
}

#[test]
fn a_due_backup_without_a_remote_is_a_no_op_and_clears_the_flag() {
    // Enabled + a pending save, but the project has no remote (it isn't even under
    // version control in this fixture): the hook fires no push and clears the flag
    // so it doesn't spuriously fire the instant a remote appears (#887).
    let (mut app, _dir) = two_chapter_app("backup-no-remote");
    app.backup = cfg(true, true, 0);
    app.mark_backup_pending_for_test();
    app.tick_backup();
    assert!(
        !app.backup_in_flight_for_test(),
        "no remote → no push spawned"
    );
    assert!(
        !app.backup_pending_for_test(),
        "the pending flag is cleared rather than left stuck on"
    );
}

#[test]
fn a_successful_backup_stamps_the_last_backup_time() {
    let (mut app, _dir) = two_chapter_app("backup-success");
    app.backup = cfg(true, true, 0);
    assert!(
        !app.has_backed_up_for_test(),
        "no backup yet at session start"
    );
    app.spawn_backup_with_outcome_for_test(Ok("main".to_string()));
    app.settle_backup_for_test();
    assert!(
        app.has_backed_up_for_test(),
        "a successful push records the last-backup time"
    );
    assert!(app.last_backup_age().is_some(), "the age is now observable");
    assert!(
        app.message.as_deref().unwrap_or("").contains("Backed up"),
        "{:?}",
        app.message
    );
}

#[test]
fn a_failed_backup_surfaces_its_cause_and_leaves_last_backup_unset() {
    // A rejected / offline push comes back as an Err carrying the surfaced cause
    // (as `crate::vcs::push` produces). It must reach the author, never be lost,
    // and must not claim the work was backed up (#887).
    let (mut app, _dir) = two_chapter_app("backup-failure");
    app.backup = cfg(true, false, 5);
    app.spawn_backup_with_outcome_for_test(Err(
        "Authentication failed — set up an SSH key.".to_string()
    ));
    app.settle_backup_for_test();
    assert!(
        !app.has_backed_up_for_test(),
        "a failed push does not stamp last_backup"
    );
    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("Authentication failed"),
        "the failure's cause reaches the author: {msg:?}"
    );
    assert!(
        msg.contains("retry"),
        "the author is told it will retry: {msg:?}"
    );
}

#[test]
fn a_failed_backup_reschedules_on_the_next_cadence() {
    // After a failure the attempt clock advanced (so it doesn't hammer the remote),
    // but nothing is stuck: once the interval elapses again the backup is due, and a
    // second attempt — succeeding this time — records the backup. Failure isn't
    // terminal (#887).
    let (mut app, _dir) = two_chapter_app("backup-retry");
    app.backup = cfg(true, false, 10);

    app.spawn_backup_with_outcome_for_test(Err("Remote operation failed: offline".to_string()));
    app.settle_backup_for_test();
    assert!(!app.has_backed_up_for_test(), "first attempt failed");

    // Right after the failure it's not due again (the clock advanced) …
    assert!(
        !app.backup_due_for_test(Instant::now()),
        "a failure doesn't immediately re-fire — it waits out the interval"
    );
    // … but once the interval elapses, it's due, and a retry that succeeds lands.
    app.age_backup_clock_for_test(10 * 60 + 1);
    assert!(
        app.backup_due_for_test(Instant::now()),
        "the interval makes the retry due"
    );
    app.spawn_backup_with_outcome_for_test(Ok("main".to_string()));
    app.settle_backup_for_test();
    assert!(
        app.has_backed_up_for_test(),
        "the retry succeeds and records the backup"
    );
}

#[test]
fn a_dead_backup_worker_surfaces_its_cause() {
    // The push worker panicking (rather than returning an Err) must still surface a
    // cause via #855's dead-worker cause-carrying, and still promise a retry (#887).
    let (mut app, _dir) = two_chapter_app("backup-died");
    app.backup = cfg(true, true, 0);
    // Silence the panic hook so the deliberate panic doesn't print to the test's
    // stderr; the cause is still carried across the worker boundary.
    let prior = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    app.backup_task.spawn(|| panic!("worker exploded"));
    app.settle_backup_for_test();
    std::panic::set_hook(prior);
    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("worker exploded") && msg.contains("retry"),
        "a dead worker's cause reaches the author with a retry note: {msg:?}"
    );
    assert!(
        !app.has_backed_up_for_test(),
        "a dead worker is not a success"
    );
}

#[test]
fn the_status_bar_shows_the_last_backup_once_one_succeeds() {
    // The author can see how current the off-site copy is (#887): the vc indicator
    // gains a "backed up N ago" note after a successful backup, and doesn't claim
    // one before any has happened.
    let (mut app, _dir) = two_chapter_app("backup-statusbar");
    app.under_vc = true; // the vc indicator only shows under version control
    let before = crate::ui::render_to_string(&app, 100, 10);
    assert!(
        !before.contains("backed up"),
        "no backup note before one succeeds:\n{before}"
    );
    app.backup = cfg(true, true, 0);
    app.spawn_backup_with_outcome_for_test(Ok("main".to_string()));
    app.settle_backup_for_test();
    let after = crate::ui::render_to_string(&app, 100, 10);
    assert!(
        after.contains("backed up"),
        "a succeeded backup shows in the status bar:\n{after}"
    );
}

// --- Scheduled local archive backups (#888) -------------------------------
//
// Hermetic (#531): the data dir is pointed at a scratch folder via `set_data_dir`,
// so archives land there and never the developer's real `~/.local/share`. The
// cadence is driven with an injected clock; a run writes a real (small) zip so the
// archive + prune path is exercised end to end, no wall-clock and no network.

/// Count the `.zip` archives directly under `dir` (an absent dir is zero).
fn zip_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "zip"))
        .count()
}

#[test]
fn local_backup_off_by_default_never_fires() {
    // The default config leaves local backups off: the cadence hook spawns nothing
    // and nothing is written, exactly as before (#888).
    let (mut app, _dir) = two_chapter_app("local-off");
    let data = app_scratch_dir("local-off-data");
    config::set_data_dir(Some(data.clone()));

    app.age_local_backup_clock_for_test(60 * 60 * 24 * 7); // a week idle
    assert!(
        !app.local_backup_due_for_test(Instant::now()),
        "no local cadence while disabled"
    );
    app.tick_local_backup();
    let bdir = config::backups_dir(&app.project.root).unwrap();
    assert!(!bdir.exists() || zip_count(&bdir) == 0, "nothing archived");

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn the_local_interval_becomes_due_only_after_it_elapses() {
    let (mut app, _dir) = two_chapter_app("local-due");
    app.backup = local_cfg(6, 5); // every 6 hours

    app.age_local_backup_clock_for_test(0);
    assert!(
        !app.local_backup_due_for_test(Instant::now()),
        "not due right after an attempt"
    );
    app.age_local_backup_clock_for_test(6 * 3600 - 60);
    assert!(
        !app.local_backup_due_for_test(Instant::now()),
        "not due just before the interval"
    );
    app.age_local_backup_clock_for_test(6 * 3600 + 60);
    assert!(
        app.local_backup_due_for_test(Instant::now()),
        "due once the interval has elapsed"
    );
}

#[test]
fn a_due_local_backup_writes_a_zip_and_stamps_the_time() {
    let (mut app, _dir) = two_chapter_app("local-writes");
    app.backup = local_cfg(6, 5);
    let data = app_scratch_dir("local-writes-data");
    config::set_data_dir(Some(data.clone()));
    let bdir = config::backups_dir(&app.project.root).unwrap();

    assert!(
        !app.has_backed_up_locally_for_test(),
        "no local backup yet at session start"
    );
    let out = app.run_local_backup_for_test(&bdir, Some(5));
    let path = out.expect("the archive is written");
    assert!(path.exists(), "the zip is on disk: {}", path.display());
    assert_eq!(zip_count(&bdir), 1, "exactly one archive");
    assert!(
        app.has_backed_up_locally_for_test(),
        "a successful archive stamps the last-local-backup time"
    );
    assert!(app.last_local_backup_age().is_some());

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn retention_prunes_older_archives_to_the_keep_count() {
    // Several archives already in the backups dir, then a new backup: pruning keeps
    // only the newest `keep` (#888). Names sort by timestamp, so seeding numbered
    // names stands in for successive backups.
    let (mut app, _dir) = two_chapter_app("local-prune");
    app.backup = local_cfg(6, 3); // keep 3
    let data = app_scratch_dir("local-prune-data");
    config::set_data_dir(Some(data.clone()));
    let bdir = config::backups_dir(&app.project.root).unwrap();
    fs::create_dir_all(&bdir).unwrap();
    // Four older archives; their names sort before the fresh timestamped one.
    for name in [
        "Book-2000-01-01-000000.zip",
        "Book-2000-01-02-000000.zip",
        "Book-2000-01-03-000000.zip",
        "Book-2000-01-04-000000.zip",
    ] {
        fs::write(bdir.join(name), b"old").unwrap();
    }
    assert_eq!(zip_count(&bdir), 4, "seeded four old archives");

    app.run_local_backup_for_test(&bdir, Some(3))
        .expect("archived");
    assert_eq!(
        zip_count(&bdir),
        3,
        "pruned to the retention count of 3 (the newest kept)"
    );
    // The newest seeded archive survives; the oldest is gone.
    assert!(
        !bdir.join("Book-2000-01-01-000000.zip").exists(),
        "the oldest was pruned"
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn zero_retention_keeps_every_archive() {
    // `local_keep = 0` disables pruning — every archive is kept rather than all
    // being deleted (#888).
    let (mut app, _dir) = two_chapter_app("local-keep-all");
    app.backup = local_cfg(6, 0);
    let data = app_scratch_dir("local-keep-all-data");
    config::set_data_dir(Some(data.clone()));
    let bdir = config::backups_dir(&app.project.root).unwrap();
    fs::create_dir_all(&bdir).unwrap();
    for name in ["Book-2000-01-01-000000.zip", "Book-2000-01-02-000000.zip"] {
        fs::write(bdir.join(name), b"old").unwrap();
    }

    app.run_local_backup_for_test(&bdir, app.backup.local_retention())
        .expect("archived");
    assert_eq!(
        zip_count(&bdir),
        3,
        "the two seeded plus the new one — nothing pruned"
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn a_stale_last_backup_warns_on_open() {
    // Local backups on, and the newest archive is older than the stale window: the
    // warning fires. A recent archive is silent (#888). Driven by backdating the
    // archive's mtime so there's no wall-clock wait.
    let (app, dir) = two_chapter_app("local-stale");
    let data = app_scratch_dir("local-stale-data");
    config::set_data_dir(Some(data.clone()));
    let bdir = config::backups_dir(&app.project.root).unwrap();
    fs::create_dir_all(&bdir).unwrap();
    let archive = bdir.join("Book-2000-01-01-000000.zip");
    fs::write(&archive, b"old").unwrap();
    // Backdate the archive's mtime two days into the past.
    let two_days_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 3600);
    filetime_set(&archive, two_days_ago);

    // Rebuild the app so it re-reads the (now stale) backups dir on open. Local
    // backups on with the default 24h stale window.
    let mut app = open_app(&dir);
    app.backup = local_cfg(6, 5); // local_enabled, default stale_warn_hours = 24
    let warn = app.stale_local_backup_warning();
    assert!(
        warn.as_deref().unwrap_or("").contains("stale"),
        "a two-day-old archive past the 24h window warns: {warn:?}"
    );

    // A fresh archive (now) is not stale.
    filetime_set(&archive, std::time::SystemTime::now());
    assert!(
        app.stale_local_backup_warning().is_none(),
        "a just-written archive doesn't warn"
    );
    // And with local backups off, no warning regardless.
    app.backup = BackupConfig::default();
    assert!(
        app.stale_local_backup_warning().is_none(),
        "no warning when local backups are off"
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

/// Set a file's mtime by rewriting it and touching via a fresh handle — the test
/// backdates an archive to exercise the staleness window without waiting real days.
/// Uses `filetime` if available, else a best-effort re-open; here we set it through
/// the standard library's `set_modified` (stable since 1.75).
fn filetime_set(path: &Path, when: std::time::SystemTime) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(when).unwrap();
}

// --- Crash mid restore-swap: startup reconciliation (#1179) ----------------

#[test]
fn a_crash_mid_restore_swap_recovers_the_project_on_open() {
    // Simulate a crash inside the restore swap's rename window: the first rename
    // (root -> <name>.quill-restore-old) ran, but the process died before the second
    // (staging -> root). `root` is now gone and the real project is stranded in the
    // hidden sibling. Without reconciliation the missing `root` reads as a brand-new
    // directory and startup scaffolds a fresh *empty* project over the book; with it,
    // the original is moved back and the author's prose is intact (#1179).
    let (app, temp) = two_chapter_app("restore-crash-reconcile");
    let root = app.project.root.clone();
    let chapter = root.join("one.md");
    assert_eq!(fs::read_to_string(&chapter).unwrap(), "first chapter\n");
    // Release the open buffer/handles before moving the directory out from under it.
    drop(app);

    // The swap's first rename, then the crash — `root` is missing, the project sits
    // in `<name>.quill-restore-old` (the hidden sibling `restore_over` moves it to).
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    let old = root
        .parent()
        .unwrap()
        .join(format!("{name}.quill-restore-old"));
    fs::rename(&root, &old).unwrap();
    assert!(!root.exists(), "mid-swap: root is gone (the crash window)");
    assert!(
        !Project::manifest_path(&root).exists(),
        "so a naive open-vs-create would scaffold a fresh empty project here"
    );

    // Startup reconciliation completes/undoes the interrupted swap ...
    App::reconcile_interrupted_restore(&root);

    // ... putting the *original* project back at `root`, prose and all — not empty.
    assert!(
        Project::manifest_path(&root).exists(),
        "the manifest is restored to root"
    );
    assert_eq!(
        fs::read_to_string(&chapter).unwrap(),
        "first chapter\n",
        "the author's chapter prose is back, not a fresh empty project"
    );
    assert!(
        !old.exists(),
        "the leftover .quill-restore-old temp dir is cleaned up"
    );

    // And the actual startup decision now opens the recovered project rather than
    // creating a new one: reopening reads the real manifest and both chapters.
    let reopened = open_app(&root);
    assert_eq!(
        reopened.project.manifest.title, "Book",
        "the recovered project opens (title intact), not a fresh scaffold"
    );
    assert_eq!(
        fs::read_to_string(root.join("two.md")).unwrap(),
        "second chapter\n",
        "the second chapter survived too"
    );

    drop(temp);
}

#[test]
fn reconciliation_is_a_no_op_without_leftovers() {
    // The normal launch: `root` holds a manifest and there are no restore temp dirs.
    // Reconciliation must not touch anything (#1179).
    let (app, _temp) = two_chapter_app("restore-noop");
    let root = app.project.root.clone();
    drop(app);

    App::reconcile_interrupted_restore(&root);
    assert!(
        Project::manifest_path(&root).exists(),
        "an intact project is left exactly as it was"
    );
    assert_eq!(
        fs::read_to_string(root.join("one.md")).unwrap(),
        "first chapter\n",
        "no prose disturbed"
    );
}

//! Tests for automatic off-site backup to the Git remote (#887): the opt-in
//! cadence (on-save + timed interval), the enabled / remote gating, and the
//! failure-surfaces-its-cause-and-reschedules path.
//!
//! Hermetic and offline (#531): none of these push to a real remote. The cadence
//! and gating are driven with an injected clock, and the push outcome is a fake
//! (a plain `Ok`/`Err`) spawned through `spawn_backup_with_outcome_for_test`, so
//! `cargo test` never touches the network.

use super::*;
use crate::config::BackupConfig;
use std::time::Instant;

/// A backup config with the given knobs, saving a few lines per test.
fn cfg(enabled: bool, on_save: bool, interval_mins: u64) -> BackupConfig {
    BackupConfig {
        enabled,
        on_save,
        interval_mins,
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

//! Crash recovery off the UI thread: the idle capture hands the sidecar write
//! to a background writer, so a disk that stalls a durable write can't stall
//! typing. These pin the hand-off (the UI path never waits on the write), the
//! latest-wins queue, clear-after-write ordering, the retry of a failed write,
//! and the guard that the typing/idle path performs no atomic write on the UI
//! thread at all.

use super::*;
use crate::app::persistence::RECOVERY_MIN_INTERVAL;
use crate::atomic;
use crate::recovery;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long a UI-path call may take while the writer is parked on a write. The
/// call does no I/O, so it should take microseconds; a blocking call would take
/// [`STUCK_WRITE`], the time the test's stand-in for a stuck disk holds the
/// write. Far apart, so a slow machine can't confuse the two.
const PROMPT: Duration = Duration::from_millis(500);
const STUCK_WRITE: Duration = Duration::from_secs(3);

/// Park the app's recovery writer on its next job — a disk that's stuck — and
/// return a sender that releases it. If the test never sends (because the UI
/// path blocked on the write), the stand-in disk recovers after
/// [`STUCK_WRITE`] by itself, so a regression fails the timing assertion
/// instead of hanging the suite.
fn stall_writer(app: &App) -> mpsc::Sender<()> {
    app.recovery_writer.hold();
    let release = app.recovery_writer.releaser();
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let _ = rx.recv_timeout(STUCK_WRITE);
        release();
    });
    tx
}

#[test]
fn the_capture_returns_without_waiting_for_the_write() {
    let (mut app, dir) = two_chapter_app("recw-async");
    let unstall = stall_writer(&app);
    type_str(&mut app, "typed while the disk is stuck ");

    let t = Instant::now();
    app.tick_recovery(); // submits the snapshot; the writer parks on it
    app.recovery_writer.wait_in_flight();
    type_str(&mut app, "and more");
    app.on_input_settled();
    app.on_quiet(); // the idle pass's (throttled) capture
    app.tick_recovery();
    let spent = t.elapsed();
    assert!(
        spent < PROMPT,
        "the UI path waited on the stuck write ({spent:?})"
    );
    assert!(
        recovery::read(&dir).is_none(),
        "the write is still parked, so nothing has landed yet"
    );

    unstall.send(()).unwrap();
    app.flush_recovery();
    let rec = recovery::read(&dir).expect("the write lands once the disk recovers");
    assert!(
        rec.body.contains("typed while the disk is stuck"),
        "{rec:?}"
    );
}

#[test]
fn only_the_newest_snapshot_is_written_after_a_stall() {
    let (mut app, dir) = two_chapter_app("recw-latest");
    let unstall = stall_writer(&app);
    type_str(&mut app, "A");
    app.capture_recovery(); // in flight, parked
    app.recovery_writer.wait_in_flight();
    type_str(&mut app, "B");
    app.capture_recovery(); // waiting
    type_str(&mut app, "C");
    app.capture_recovery(); // replaces the waiting "AB"

    unstall.send(()).unwrap();
    app.flush_recovery();
    assert!(recovery::read(&dir).unwrap().body.contains("ABC"));
    assert_eq!(
        app.recovery_writer.writes(),
        2,
        "the in-flight write and the newest one — never a backlog"
    );
}

#[test]
fn a_clear_after_a_pending_write_leaves_no_sidecar() {
    // A save while a capture is in flight (and another waiting) must not be
    // undone by the late write: the clear runs after it and drops the waiting one.
    let (mut app, dir) = two_chapter_app("recw-clear");
    let unstall = stall_writer(&app);
    type_str(&mut app, "first");
    app.capture_recovery(); // in flight, parked
    app.recovery_writer.wait_in_flight();
    type_str(&mut app, " second");
    app.capture_recovery(); // waiting
    app.on_key(ctrl('s')); // a clean save queues the clear

    unstall.send(()).unwrap();
    app.flush_recovery();
    assert!(
        recovery::read(&dir).is_none(),
        "the clear landed after the in-flight write"
    );
    assert_eq!(
        app.recovery_writer.writes(),
        1,
        "the waiting write was superseded"
    );
}

#[test]
fn a_failed_write_is_retried_on_the_next_tick() {
    let (mut app, dir) = two_chapter_app("recw-retry");
    app.recovery_writer.fail_next_write();
    type_str(&mut app, "keep me");
    app.tick_recovery();
    app.flush_recovery(); // picks up the failure
    assert!(recovery::read(&dir).is_none(), "the first write failed");

    // Well within the throttle interval, and no new edit: still retried.
    app.tick_recovery();
    app.flush_recovery();
    let rec = recovery::read(&dir).expect("the next tick retried the write");
    assert!(rec.body.contains("keep me"));

    // And once it has landed, an unchanged tick doesn't write again.
    let writes = app.recovery_writer.writes();
    app.recovery_captured_at = Instant::now().checked_sub(RECOVERY_MIN_INTERVAL);
    app.tick_recovery();
    app.flush_recovery();
    assert_eq!(app.recovery_writer.writes(), writes);
}

/// Drive the event loop's typing and idle path — keys, the per-batch settled
/// pass, the per-frame recovery tick (past its throttle), the debounced idle
/// pass — and return how many atomic writes the UI (this) thread performed.
fn ui_thread_writes_while_typing(app: &mut App) -> usize {
    let before = atomic::write_count::get();
    for round in 0..3 {
        for c in "words ".chars() {
            app.on_key(key(KeyCode::Char(c)));
            app.on_input_settled();
            app.tick_recovery();
        }
        app.on_quiet();
        // Let the next round's tick capture again, as two seconds of typing would.
        app.recovery_captured_at = Instant::now().checked_sub(RECOVERY_MIN_INTERVAL);
        assert!(app.flush_recovery(), "round {round}: the writer settled");
    }
    atomic::write_count::get() - before
}

// The guard against the stutter this fixed: a durable write (temp, fsync,
// rename, dir fsync) on the UI thread stalls typing whenever the disk is busy —
// measured at up to seconds under contention. The typing/idle path must hand
// every sidecar write to the background writer. (Explicit saves and autosave are
// a separate path.)
#[test]
fn the_typing_and_idle_path_never_writes_on_the_ui_thread() {
    let (mut app, dir) = two_chapter_app("recw-guard-single");
    assert_eq!(ui_thread_writes_while_typing(&mut app), 0);
    assert!(
        app.recovery_writer.writes() >= 3,
        "the captures happened — on the writer thread"
    );
    assert!(recovery::read(&dir).unwrap().body.contains("words"));

    let (mut app, dir) = three_chapter_app("recw-guard-continuous");
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_some());
    assert_eq!(ui_thread_writes_while_typing(&mut app), 0);
    assert!(app.recovery_writer.writes() >= 3);
    assert!(recovery::read(&dir).is_some());
}

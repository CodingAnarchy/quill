//! Saving off the UI thread: autosave and Ctrl-S hand the write to the
//! project files writer and return, so a disk that stalls a durable write can't
//! stall typing. These pin the hand-off (the save path never waits on the
//! write), the dirty/revision state machine (typing during a write keeps the
//! buffer dirty and the recovery sidecar), failure reporting, the operations
//! that must wait for the save (chapter switch, export, quit), and the guard
//! that an autosave cycle performs no atomic write on the UI thread.

use super::*;
use crate::atomic;
use crate::mode::{Action, ExportFormat};
use crate::project::annotation_writes;
use crate::recovery;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long a save-path call may take while the writer is parked on a write.
/// The call does no I/O, so it should take microseconds; a blocking call would
/// take [`STUCK_WRITE`]. Far apart, so a slow machine can't confuse the two.
const PROMPT: Duration = Duration::from_millis(500);
const STUCK_WRITE: Duration = Duration::from_secs(3);
/// How long the stand-in stuck disk holds a write that an operation must wait
/// for — long enough that an operation which didn't wait would finish first.
const BRIEF_STALL: Duration = Duration::from_millis(150);

/// An editor whose saves really are asynchronous (tests settle eagerly by
/// default) with a one-second autosave interval.
fn async_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.settle_saves_eagerly = false;
    app.autosave_interval_secs = 1;
    (app, dir)
}

/// Pretend the author has been idle past the autosave interval, then tick it.
fn autosave_now(app: &mut App) {
    app.last_activity = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    app.tick_autosave();
}

/// Park the save writer on its next job — a disk that's stuck — and return a
/// sender that releases it. If the test never sends (because the save path
/// blocked on the write), the stand-in disk recovers after [`STUCK_WRITE`] by
/// itself, so a regression fails a timing assertion instead of hanging.
fn stall_writer(app: &App) -> mpsc::Sender<()> {
    app.file_writer.hold();
    let release = app.file_writer.releaser();
    let (tx, rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        let _ = rx.recv_timeout(STUCK_WRITE);
        release();
    });
    tx
}

/// Park the save writer on its next job and release it after [`BRIEF_STALL`]
/// from another thread — for an operation that must wait out the write.
fn stall_briefly(app: &App) {
    app.file_writer.hold();
    let release = app.file_writer.releaser();
    thread::spawn(move || {
        thread::sleep(BRIEF_STALL);
        release();
    });
}

fn on_disk(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap()
}

/// Capture the crash-recovery sidecar now and let it land.
fn capture(app: &mut App) {
    app.capture_recovery();
    app.flush_recovery();
}

#[test]
fn autosave_returns_without_waiting_for_the_write() {
    let (mut app, dir) = async_app("save-async");
    type_str(&mut app, "X");
    capture(&mut app);
    let unstall = stall_writer(&app);

    let t = Instant::now();
    autosave_now(&mut app);
    app.file_writer.wait_in_flight();
    type_str(&mut app, "Y"); // typing carries on while the write is stuck
    app.on_input_settled();
    app.tick_autosave(); // a second tick doesn't queue another save
    let spent = t.elapsed();
    assert!(
        spent < PROMPT,
        "the save path waited on the write ({spent:?})"
    );
    assert_eq!(
        on_disk(&dir, "one.md"),
        "first chapter\n",
        "nothing landed yet"
    );
    assert!(app.buffer.dirty);
    assert_eq!(app.pending_saves.len(), 1);

    unstall.send(()).unwrap();
    assert!(app.settle_saves());
    assert_eq!(
        on_disk(&dir, "one.md"),
        "Xfirst chapter\n",
        "the snapshot landed"
    );
}

#[test]
fn a_landed_autosave_cleans_the_buffer_and_clears_the_sidecar() {
    let (mut app, dir) = async_app("save-clean");
    type_str(&mut app, "X");
    capture(&mut app);
    assert!(recovery::read(&dir).is_some());

    autosave_now(&mut app);
    assert!(app.settle_saves());
    app.flush_recovery();
    assert!(!app.buffer.dirty);
    assert!(app.message.is_none(), "autosave is quiet on success");
    assert_eq!(on_disk(&dir, "one.md"), "Xfirst chapter\n");
    assert!(recovery::read(&dir).is_none(), "nothing left to recover");
}

#[test]
fn typing_during_a_save_keeps_the_buffer_dirty_and_the_sidecar() {
    let (mut app, dir) = async_app("save-typing");
    type_str(&mut app, "A");
    capture(&mut app);
    let unstall = stall_writer(&app);
    autosave_now(&mut app);
    app.file_writer.wait_in_flight();
    type_str(&mut app, "B"); // after the snapshot
    capture(&mut app); // the sidecar now holds the newer text

    unstall.send(()).unwrap();
    assert!(app.settle_saves());
    app.flush_recovery();
    assert_eq!(on_disk(&dir, "one.md"), "Afirst chapter\n", "the snapshot");
    assert!(app.buffer.dirty, "the newer text isn't on disk yet");
    let rec = recovery::read(&dir).expect("the sidecar survives the older save");
    assert!(rec.body.starts_with("ABfirst"), "{rec:?}");

    // The next save catches up and only then clears it.
    app.on_key(ctrl('s'));
    assert!(app.settle_saves());
    app.flush_recovery();
    assert!(!app.buffer.dirty);
    assert_eq!(on_disk(&dir, "one.md"), "ABfirst chapter\n");
    assert!(recovery::read(&dir).is_none());
}

#[test]
fn a_failed_background_save_reports_and_keeps_everything() {
    let (mut app, dir) = async_app("save-fail");
    type_str(&mut app, "keep me ");
    capture(&mut app);

    app.file_writer.fail_next();
    autosave_now(&mut app);
    assert!(app.settle_saves());
    app.flush_recovery();
    let msg = app.message.clone().unwrap_or_default();
    assert!(msg.starts_with("Autosave failed: Save failed"), "{msg}");
    assert!(app.buffer.dirty, "a failed save leaves the buffer dirty");
    assert_eq!(on_disk(&dir, "one.md"), "first chapter\n");
    assert!(
        recovery::read(&dir).unwrap().body.contains("keep me"),
        "the sidecar is intact"
    );

    // An explicit save reports it in its own words.
    app.file_writer.fail_next();
    app.on_key(ctrl('s'));
    assert!(app.settle_saves());
    let msg = app.message.clone().unwrap_or_default();
    assert!(msg.starts_with("Save failed"), "{msg}");
    assert!(app.buffer.dirty);
}

#[test]
fn ctrl_s_is_asynchronous_and_reports_when_it_lands() {
    let (mut app, dir) = async_app("save-ctrl-s");
    type_str(&mut app, "X");
    let unstall = stall_writer(&app);
    let t = Instant::now();
    app.on_key(ctrl('s'));
    assert!(t.elapsed() < PROMPT);
    assert_eq!(app.message.as_deref(), Some("Saving…"));
    unstall.send(()).unwrap();
    assert!(app.settle_saves());
    assert_eq!(app.message.as_deref(), Some("Saved"));
    assert_eq!(on_disk(&dir, "one.md"), "Xfirst chapter\n");
}

#[test]
fn a_chapter_switch_waits_for_the_save_in_flight() {
    let (mut app, dir) = async_app("save-switch");
    type_str(&mut app, "X");
    stall_briefly(&app);
    autosave_now(&mut app);
    app.file_writer.wait_in_flight();

    app.on_key(ctrl('n')); // waits for the write, then loads the next chapter
    assert_eq!(app.project.active_index(), 1, "{:?}", app.message);
    assert_eq!(app.buffer.text(), "second chapter\n");
    assert_eq!(on_disk(&dir, "one.md"), "Xfirst chapter\n");
    assert!(app.pending_saves.is_empty());
}

#[test]
fn a_chapter_switch_after_a_failed_save_is_refused() {
    let (mut app, dir) = async_app("save-switch-fail");
    type_str(&mut app, "X");
    app.file_writer.fail_next();
    autosave_now(&mut app);
    // And the switch's own write fails too: the disk is really refusing.
    atomic::fail_point::arm_pre_rename_failure();
    app.on_key(ctrl('n'));
    assert_eq!(app.project.active_index(), 0, "the switch was refused");
    assert!(app.buffer.dirty, "the edits are still here");
    assert!(app.buffer.text().starts_with("Xfirst"));
    assert!(app.message.as_deref().unwrap_or("").contains("Save failed"));
    assert_eq!(on_disk(&dir, "one.md"), "first chapter\n");
}

#[test]
fn an_export_right_after_ctrl_s_sees_the_saved_text() {
    let (mut app, dir) = async_app("save-export");
    type_str(&mut app, "Freshly typed. ");
    stall_briefly(&app);
    app.on_key(ctrl('s'));
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    assert_eq!(
        on_disk(&dir, "one.md"),
        "Freshly typed. first chapter\n",
        "the export waited for the save"
    );
    let exported = fs::read_to_string(dir.join("export").join("book.md")).unwrap();
    assert!(exported.contains("Freshly typed."), "{exported}");
    assert!(!app.buffer.dirty);
}

#[test]
fn quitting_waits_for_the_save_in_flight() {
    let (mut app, dir) = async_app("save-quit");
    type_str(&mut app, "X");
    stall_briefly(&app);
    app.on_key(ctrl('s'));
    app.on_key(ctrl('q'));
    assert!(app.should_quit, "a landed save leaves nothing to ask about");
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    assert_eq!(on_disk(&dir, "one.md"), "Xfirst chapter\n");
    assert!(app.finish_saves().is_none());
}

#[test]
fn quitting_after_a_failed_save_still_asks() {
    let (mut app, _dir) = async_app("save-quit-fail");
    type_str(&mut app, "X");
    app.file_writer.fail_next();
    app.on_key(ctrl('s'));
    app.on_key(ctrl('q'));
    assert!(!app.should_quit);
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the unsaved prompt opens"
    );
}

/// Type, settle the per-batch pass, run the idle pass, autosave, and let the
/// writer drain — returning how many atomic writes (and annotations side-file
/// writes performed here) the UI (this) thread did along the way.
fn ui_thread_writes_across_an_autosave(app: &mut App) -> usize {
    let before = atomic::write_count::get();
    for c in "words ".chars() {
        app.on_key(key(KeyCode::Char(c)));
        app.on_input_settled();
        app.tick_recovery();
    }
    app.on_quiet();
    autosave_now(app);
    app.poll_saves();
    assert!(app.file_writer.flush(STUCK_WRITE), "the writer settled");
    app.poll_saves(); // the completion: clean, recovery clear, history, position
    assert!(app.file_writer.flush(STUCK_WRITE));
    app.poll_saves();
    assert!(!app.edit_dirty(), "the autosave landed: {:?}", app.message);
    atomic::write_count::get() - before
}

// The guard against the stall this fixed: a chapter write (temp, fsync,
// rename, dir fsync) on the UI thread blocks typing whenever the disk is busy —
// measured at up to ~1s under contention as autosave fired. Autosave must hand
// every write of its cycle — the chapter(s), the annotations side file, the
// history log, the resume position — to the background writer.
#[test]
fn an_autosave_cycle_never_writes_on_the_ui_thread() {
    let (mut app, dir) = async_app("save-guard-single");
    // A note after the typing point, so the save also re-writes the side file.
    app.create_annotation(6, 13, "a note".into());
    let notes = annotation_writes::get();
    assert_eq!(ui_thread_writes_across_an_autosave(&mut app), 0);
    assert!(
        app.file_writer.writes() >= 1,
        "the save ran — on the writer"
    );
    assert!(on_disk(&dir, "one.md").starts_with("words "));
    assert_eq!(
        annotation_writes::get() - notes,
        1,
        "the shifted note was saved"
    );
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!((live[0].start, live[0].end), (12, 19));

    let (mut app, dir) = three_chapter_app("save-guard-continuous");
    app.settle_saves_eagerly = false;
    app.autosave_interval_secs = 1;
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_some());
    assert_eq!(ui_thread_writes_across_an_autosave(&mut app), 0);
    assert!(app.file_writer.writes() >= 1);
    assert!(on_disk(&dir, "c0.md").starts_with("words "));
    assert!(
        app.project.chapter_words(0) >= 2,
        "the window's counts refreshed"
    );
}

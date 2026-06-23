//! Crash recovery (#263): the editor writes unsaved work to a sidecar on the
//! idle cadence, offers it back at launch after an unclean exit, and clears it
//! on a clean save or an explicit discard.

use super::*;
use crate::mode::Pending;
use crate::recovery;

/// True while the recovery prompt is open.
fn at_recovery_prompt(app: &App) -> bool {
    matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ConfirmRecovery))
}

#[test]
fn idle_writes_a_sidecar_while_the_buffer_is_dirty() {
    let (mut app, dir) = two_chapter_app("rec-write");
    type_str(&mut app, "new words");
    app.on_idle();

    let rec = recovery::read(&dir).expect("a dirty buffer should leave a sidecar");
    assert_eq!(rec.chapter_file, "one.md");
    assert!(rec.body.contains("new words"));
}

#[test]
fn a_clean_save_clears_the_sidecar() {
    let (mut app, dir) = two_chapter_app("rec-save");
    type_str(&mut app, "more");
    app.on_idle();
    assert!(recovery::read(&dir).is_some());

    app.on_key(ctrl('s')); // save
    assert!(recovery::read(&dir).is_none(), "saving leaves nothing to recover");
}

#[test]
fn offer_restores_unsaved_work_into_the_buffer() {
    let (mut app, dir) = two_chapter_app("rec-restore");
    // Simulate a previous session that crashed with unsaved edits to chapter one.
    recovery::write(&dir, &recovery::Recovery { chapter_file: "one.md".into(), body: "rescued prose\n".into() }).unwrap();

    app.offer_recovery();
    assert!(at_recovery_prompt(&app), "a fresh sidecar should be offered");

    app.on_key(key(KeyCode::Char('y'))); // restore
    assert!(app.edit_buffer().text().contains("rescued prose"));
    assert!(app.edit_dirty(), "restored work is unsaved until the author saves");
    assert!(matches!(app.mode, Mode::Insert), "the prompt closes after restoring");
}

#[test]
fn discard_clears_the_sidecar_without_touching_the_buffer() {
    let (mut app, dir) = two_chapter_app("rec-discard");
    recovery::write(&dir, &recovery::Recovery { chapter_file: "one.md".into(), body: "rescued prose\n".into() }).unwrap();

    app.offer_recovery();
    let before = app.edit_buffer().text();
    app.on_key(key(KeyCode::Char('d'))); // discard

    assert!(recovery::read(&dir).is_none(), "discard removes the sidecar");
    assert_eq!(app.edit_buffer().text(), before, "discard leaves the buffer untouched");
    assert!(!app.edit_dirty());
}

#[test]
fn keep_for_later_leaves_the_sidecar_for_the_next_launch() {
    let (mut app, dir) = two_chapter_app("rec-later");
    recovery::write(&dir, &recovery::Recovery { chapter_file: "one.md".into(), body: "rescued prose\n".into() }).unwrap();

    app.offer_recovery();
    app.on_key(key(KeyCode::Esc)); // keep for later

    assert!(recovery::read(&dir).is_some(), "Esc keeps the sidecar to decide later");
    assert!(matches!(app.mode, Mode::Insert));
}

#[test]
fn a_sidecar_matching_disk_is_stale_and_silently_cleared() {
    let (mut app, dir) = two_chapter_app("rec-stale");
    // The recorded body is exactly what's already on disk — nothing was lost.
    recovery::write(&dir, &recovery::Recovery { chapter_file: "one.md".into(), body: "first chapter\n".into() }).unwrap();

    app.offer_recovery();
    assert!(!at_recovery_prompt(&app), "no prompt when nothing was actually lost");
    assert!(recovery::read(&dir).is_none(), "the stale sidecar is cleared");
}

#[test]
fn a_sidecar_for_a_missing_chapter_is_cleared() {
    let (mut app, dir) = two_chapter_app("rec-missing");
    recovery::write(&dir, &recovery::Recovery { chapter_file: "ghost.md".into(), body: "orphaned\n".into() }).unwrap();

    app.offer_recovery();
    assert!(!at_recovery_prompt(&app));
    assert!(recovery::read(&dir).is_none());
}

#[test]
fn quitting_despite_unsaved_changes_discards_the_sidecar() {
    let (mut app, dir) = two_chapter_app("rec-quit");
    type_str(&mut app, "abandon me");
    app.on_idle();
    assert!(recovery::read(&dir).is_some());

    app.on_key(ctrl('q')); // unsaved → quit confirm prompt
    app.on_key(key(KeyCode::Char('y'))); // confirm quit
    assert!(app.should_quit);
    assert!(recovery::read(&dir).is_none(), "an explicit quit-with-unsaved discards recovery");
}

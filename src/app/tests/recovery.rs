//! Crash recovery (#263): the editor writes unsaved work to a sidecar on the
//! idle cadence, offers it back at launch after an unclean exit, and clears it
//! on a clean save or an explicit discard.

use super::*;
use crate::app::persistence::RECOVERY_MIN_INTERVAL;
use crate::mode::Pending;
use crate::recovery;
use std::fmt::Write;
use std::time::Instant;

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
    app.flush_recovery(); // the clear is queued on the background writer
    assert!(
        recovery::read(&dir).is_none(),
        "saving leaves nothing to recover"
    );
}

#[test]
fn offer_restores_unsaved_work_into_the_buffer() {
    let (mut app, dir) = two_chapter_app("rec-restore");
    // Simulate a previous session that crashed with unsaved edits to chapter one.
    recovery::write(
        &dir,
        &recovery::Recovery::single("one.md".into(), "rescued prose\n".into()),
    )
    .unwrap();

    app.offer_recovery();
    assert!(
        at_recovery_prompt(&app),
        "a fresh sidecar should be offered"
    );

    app.on_key(key(KeyCode::Char('y'))); // restore
    assert!(app.edit_buffer().text().contains("rescued prose"));
    assert!(
        app.edit_dirty(),
        "restored work is unsaved until the author saves"
    );
    assert!(
        matches!(app.mode, Mode::Insert),
        "the prompt closes after restoring"
    );
}

#[test]
fn discard_clears_the_sidecar_without_touching_the_buffer() {
    let (mut app, dir) = two_chapter_app("rec-discard");
    recovery::write(
        &dir,
        &recovery::Recovery::single("one.md".into(), "rescued prose\n".into()),
    )
    .unwrap();

    app.offer_recovery();
    let before = app.edit_buffer().text();
    app.on_key(key(KeyCode::Char('d'))); // discard
    app.flush_recovery();

    assert!(
        recovery::read(&dir).is_none(),
        "discard removes the sidecar"
    );
    assert_eq!(
        app.edit_buffer().text(),
        before,
        "discard leaves the buffer untouched"
    );
    assert!(!app.edit_dirty());
}

#[test]
fn keep_for_later_leaves_the_sidecar_for_the_next_launch() {
    let (mut app, dir) = two_chapter_app("rec-later");
    recovery::write(
        &dir,
        &recovery::Recovery::single("one.md".into(), "rescued prose\n".into()),
    )
    .unwrap();

    app.offer_recovery();
    app.on_key(key(KeyCode::Esc)); // keep for later

    assert!(
        recovery::read(&dir).is_some(),
        "Esc keeps the sidecar to decide later"
    );
    assert!(matches!(app.mode, Mode::Insert));
}

#[test]
fn a_sidecar_matching_disk_is_stale_and_silently_cleared() {
    let (mut app, dir) = two_chapter_app("rec-stale");
    // The recorded body is exactly what's already on disk — nothing was lost.
    recovery::write(
        &dir,
        &recovery::Recovery::single("one.md".into(), "first chapter\n".into()),
    )
    .unwrap();

    app.offer_recovery();
    assert!(
        !at_recovery_prompt(&app),
        "no prompt when nothing was actually lost"
    );
    assert!(
        recovery::read(&dir).is_none(),
        "the stale sidecar is cleared"
    );
}

#[test]
fn a_sidecar_for_a_missing_chapter_is_cleared() {
    let (mut app, dir) = two_chapter_app("rec-missing");
    recovery::write(
        &dir,
        &recovery::Recovery::single("ghost.md".into(), "orphaned\n".into()),
    )
    .unwrap();

    app.offer_recovery();
    assert!(!at_recovery_prompt(&app));
    assert!(recovery::read(&dir).is_none());
}

// #871: a *hard* read error on a captured chapter (permissions / an I/O fault)
// must not be mistaken for "the chapter differs from disk" and over-offer a
// restore of content that was never actually lost. The sidecar is kept for a
// later launch (when the file may read again), not spuriously offered now.
#[cfg(unix)]
#[test]
fn a_hard_read_error_does_not_over_offer_recovery() {
    let (mut app, dir) = two_chapter_app("rec-read-fail");
    recovery::write(
        &dir,
        &recovery::Recovery::single("one.md".into(), "captured prose\n".into()),
    )
    .unwrap();

    // Make chapter one unreadable — a directory in its place is a hard read error,
    // distinct from a missing file (which is the legitimate "gone" case above).
    let chapter = dir.join("one.md");
    fs::remove_file(&chapter).unwrap();
    fs::create_dir(&chapter).unwrap();

    app.offer_recovery();
    assert!(
        !at_recovery_prompt(&app),
        "a hard read error must not spuriously offer recovery"
    );
    assert!(
        recovery::read(&dir).is_some(),
        "the sidecar is kept for a later launch, not cleared or offered"
    );

    fs::remove_dir(&chapter).ok();
}

// --- Continuous view (#566) -----------------------------------------------

#[test]
fn idle_captures_every_dirty_continuous_segment() {
    let (mut app, dir) = three_chapter_app("rec-cont-write");
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());

    // Edit chapter 0 ("alpha"), then move into chapter 1 ("beta") and edit it.
    app.on_key(key(KeyCode::End));
    type_str(&mut app, " one");
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::End));
    type_str(&mut app, " two");
    app.on_idle();

    let rec = recovery::read(&dir).expect("dirty continuous segments leave a sidecar");
    let captured: Vec<(String, String)> =
        rec.chapters().map(|c| (c.chapter_file, c.body)).collect();
    assert_eq!(
        captured.len(),
        2,
        "both edited chapters captured: {captured:?}"
    );
    assert!(captured
        .iter()
        .any(|(f, b)| f == "c0.md" && b.contains("alpha one")));
    assert!(captured
        .iter()
        .any(|(f, b)| f == "c1.md" && b.contains("beta two")));
}

#[test]
fn an_unedited_chapter_is_not_captured_in_the_continuous_sidecar() {
    let (mut app, dir) = three_chapter_app("rec-cont-partial");
    app.on_key(ctrl('l'));
    app.on_key(key(KeyCode::End));
    type_str(&mut app, " edit");
    app.on_idle();

    let rec = recovery::read(&dir).expect("a dirty segment leaves a sidecar");
    let files: Vec<_> = rec.chapters().map(|c| c.chapter_file).collect();
    assert_eq!(files, vec!["c0.md"], "only the edited chapter is captured");
}

#[test]
fn saving_the_continuous_view_clears_the_sidecar() {
    let (mut app, dir) = three_chapter_app("rec-cont-save");
    app.on_key(ctrl('l'));
    app.on_key(key(KeyCode::End));
    type_str(&mut app, " edit");
    app.on_idle();
    assert!(recovery::read(&dir).is_some());

    app.on_key(ctrl('s')); // save the window
    app.on_idle(); // the now-clean window clears the sidecar
    assert!(
        recovery::read(&dir).is_none(),
        "saving the continuous view leaves nothing to recover"
    );
}

#[test]
fn a_continuous_crash_is_offered_and_restored_at_relaunch() {
    let dir = app_scratch_dir("rec-cont-relaunch");
    {
        let mut manifest = String::from("title = \"Book\"\n\n");
        for (i, body) in ["alpha\n", "beta\n", "gamma\n"].iter().enumerate() {
            write!(
                manifest,
                "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
            )
            .unwrap();
            fs::write(dir.join(format!("c{i}.md")), body).unwrap();
        }
        fs::write(Project::manifest_path(&dir), manifest).unwrap();
    }

    // First session: edit two chapters in the continuous view, idle to capture,
    // then drop the app without saving — a stand-in for a crash/kill.
    {
        let mut app = open_app(&dir);
        app.on_key(ctrl('l'));
        app.on_key(key(KeyCode::End));
        type_str(&mut app, " ONE");
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::End));
        type_str(&mut app, " TWO");
        app.on_idle();
        assert!(recovery::read(&dir).is_some(), "the crash left a sidecar");
        // The files on disk are still the originals — nothing was saved.
        assert_eq!(fs::read_to_string(dir.join("c0.md")).unwrap(), "alpha\n");
    }

    // Relaunch: the sidecar is offered, names both chapters, and restores them.
    let mut app = open_app(&dir);
    app.offer_recovery();
    assert!(
        at_recovery_prompt(&app),
        "the continuous sidecar is offered"
    );
    if let Mode::Prompt(s) = &app.mode {
        assert!(
            s.label.contains("2 chapters"),
            "prompt names the count: {}",
            s.label
        );
    }

    app.on_key(key(KeyCode::Char('y'))); // restore
    assert!(
        app.manuscript.is_some(),
        "restoring a multi-chapter record opens the continuous view"
    );
    let flow = app.edit_buffer().text();
    assert!(flow.contains("alpha ONE"), "first edit recovered: {flow}");
    assert!(flow.contains("beta TWO"), "second edit recovered: {flow}");
    assert!(
        app.edit_dirty(),
        "recovered work is unsaved until the author saves"
    );

    // A save persists every recovered chapter and clears the sidecar.
    app.on_key(ctrl('s'));
    assert_eq!(
        fs::read_to_string(dir.join("c0.md")).unwrap(),
        "alpha ONE\n"
    );
    assert_eq!(fs::read_to_string(dir.join("c1.md")).unwrap(), "beta TWO\n");
    app.on_idle();
    assert!(
        recovery::read(&dir).is_none(),
        "the save cleared the sidecar"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn single_chapter_recovery_is_unchanged() {
    // The original single-chapter flow still writes a one-chapter sidecar with no
    // `extra` chapters — the continuous work didn't disturb it (#566).
    let (mut app, dir) = two_chapter_app("rec-single-unchanged");
    type_str(&mut app, "more");
    app.on_idle();
    let rec = recovery::read(&dir).expect("single-chapter dirty buffer leaves a sidecar");
    assert_eq!(rec.chapter_file, "one.md");
    assert!(
        rec.extra.is_empty(),
        "single-chapter records carry no extras"
    );
    assert_eq!(rec.chapters().count(), 1);
}

// #165 follow-up: a focused reference pane (a story-bible entry / loose note
// opened beside the prose) is also `App::buffer`, but its text is NOT the active
// chapter's. The idle recovery snapshot must never label that text with the
// active chapter's filename — on restore the crash-recovery offer would overwrite
// the active chapter with the reference file's content (data corruption).
#[test]
fn a_focused_reference_pane_is_not_captured_as_the_active_chapter() {
    let (mut app, dir) = two_chapter_app("rec-ref-focus"); // active = "One" / one.md
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        dir.join("bible").join("mara.md"),
        "# Mara\n\n## Physical\n\ntall\n",
    )
    .unwrap();

    // Open the bible entry in a reference pane and focus it (Ctrl-w w).
    app.on_key(ctrl('g')); // chapter browser (reveals the reference-files menu)
    app.on_key(key(KeyCode::Char('f'))); // reference files
    app.on_key(key(KeyCode::Enter)); // open the (only) entry in a pane
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char('w'))); // focus the bible pane
    assert!(
        !app.focus_shows_active_chapter(),
        "the focused buffer should be the reference pane, not the active chapter"
    );

    // Edit the focused bible pane, then run the idle cadence that captures recovery.
    type_str(&mut app, "unsaved bible note");
    app.on_idle();

    // The core safety property: nothing captured may pair the active chapter's
    // file with the reference pane's text.
    let rec = recovery::read(&dir);
    if let Some(rec) = &rec {
        for c in rec.chapters() {
            assert_ne!(
                c.chapter_file, "one.md",
                "recovery mis-labeled the bible's text as the active chapter: {c:?}"
            );
        }
    }
    // This fix skips the snapshot entirely while a reference pane is focused
    // (the restore path can only resolve manifest chapters, not a bible file).
    assert!(
        rec.is_none(),
        "no sidecar is written for a focused reference pane: {rec:?}"
    );

    // And the active chapter's file on disk is untouched by any of this.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "first chapter\n"
    );
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
    app.flush_recovery(); // as `main` does once the loop exits
    assert!(
        recovery::read(&dir).is_none(),
        "an explicit quit-with-unsaved discards recovery"
    );
}

#[test]
fn an_unchanged_buffer_does_not_rewrite_the_sidecar() {
    // Each capture is a durable (fsync'd) write; an idle pass with no new text —
    // after an arrow key, a mouse move, a pause — must not repeat it.
    let (mut app, dir) = two_chapter_app("rec-unchanged");
    type_str(&mut app, "new words");
    app.on_idle();
    assert!(recovery::read(&dir).is_some(), "the edit was captured");

    // Remove the sidecar behind the app's back: a rewrite would bring it back.
    recovery::clear(&dir);
    app.on_key(key(KeyCode::Left)); // a motion, no text change
    app.on_idle();
    assert!(
        recovery::read(&dir).is_none(),
        "nothing changed, so nothing was rewritten"
    );

    type_str(&mut app, "!");
    app.on_idle();
    let rec = recovery::read(&dir).expect("a change is captured again");
    assert!(rec.body.contains("new word!s"), "{}", rec.body);
}

#[test]
fn an_undo_is_captured_on_the_next_idle_pass() {
    // Undo swaps the text without an edit; the capture's change key must still
    // see it, or the sidecar would keep the pre-undo text.
    let (mut app, dir) = two_chapter_app("rec-undo");
    type_str(&mut app, " extra");
    app.on_idle();
    type_str(&mut app, " more");
    app.on_idle();
    assert!(recovery::read(&dir).unwrap().body.contains("extra more"));

    app.on_key(ctrl('z')); // drop " more"
    app.on_idle();
    let rec = recovery::read(&dir).expect("still dirty, still captured");
    assert!(rec.body.contains("extra"), "{}", rec.body);
    assert!(
        !rec.body.contains("more"),
        "the sidecar holds the post-undo text: {}",
        rec.body
    );
}

#[test]
fn a_throttled_capture_is_deferred_not_dropped() {
    // The event loop's capture is rate-limited while edits keep coming; a write
    // the throttle holds back lands on a later tick, so the last edit of a burst
    // is never left uncaptured.
    let (mut app, dir) = two_chapter_app("rec-throttle");
    type_str(&mut app, "A");
    app.tick_recovery();
    app.flush_recovery();
    assert!(recovery::read(&dir).unwrap().body.contains('A'));

    type_str(&mut app, "B");
    app.tick_recovery(); // within the interval: held back
    app.flush_recovery();
    assert!(!recovery::read(&dir).unwrap().body.contains('B'));

    // Once the interval has passed, the next tick writes the latest text.
    app.recovery_captured_at = Instant::now().checked_sub(RECOVERY_MIN_INTERVAL);
    app.tick_recovery();
    app.flush_recovery();
    assert!(recovery::read(&dir).unwrap().body.contains("AB"));
}

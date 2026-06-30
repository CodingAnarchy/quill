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
            manifest.push_str(&format!(
                "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
            ));
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

#[test]
fn quitting_despite_unsaved_changes_discards_the_sidecar() {
    let (mut app, dir) = two_chapter_app("rec-quit");
    type_str(&mut app, "abandon me");
    app.on_idle();
    assert!(recovery::read(&dir).is_some());

    app.on_key(ctrl('q')); // unsaved → quit confirm prompt
    app.on_key(key(KeyCode::Char('y'))); // confirm quit
    assert!(app.should_quit);
    assert!(
        recovery::read(&dir).is_none(),
        "an explicit quit-with-unsaved discards recovery"
    );
}

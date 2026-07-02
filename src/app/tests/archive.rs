//! Offline project archive & restore flow (#594): the run-actions that write a
//! timestamped `.zip` of the whole project and restore one into a fresh folder.
//! The zip/unzip round-trip is unit-tested in `crate::archive`; these cover the
//! prompt wiring — archiving to a chosen path, then restoring it and opening the
//! result.

use super::*;
use crate::mode::Action;

/// Drive the archive run-action to write the open project to `dest`.
fn archive_to(app: &mut App, dest: &Path) {
    app.run_action(Action::ArchiveProject, false);
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ArchiveProjectDest)),
        "the archive prompt opened"
    );
    // Replace the prefilled default with the test's destination.
    if let Mode::Prompt(state) = &mut app.mode {
        state.clear();
    }
    type_str(app, &dest.to_string_lossy());
    app.on_key(key(KeyCode::Enter));
}

#[test]
fn archiving_writes_a_zip_to_the_chosen_path() {
    isolate("archive-write");
    let (mut app, _dir) = two_chapter_app("archive-write");
    let out = app_scratch_dir("archive-write-out").join("backup.zip");

    archive_to(&mut app, &out);

    assert!(out.is_file(), "the archive was written");
    let msg = app.message.clone().unwrap_or_default();
    assert!(msg.contains("Archived"), "reported the archive: {msg}");
    let _ = fs::remove_dir_all(out.parent().unwrap());
}

#[test]
fn restoring_reproduces_and_opens_the_project() {
    isolate("archive-restore");
    let (mut app, _dir) = two_chapter_app("archive-restore");
    let out = app_scratch_dir("archive-restore-out").join("backup.zip");
    archive_to(&mut app, &out);

    // Restore into a fresh folder.
    let dest = app_scratch_dir("archive-restore-dest").join("Restored");
    app.run_action(Action::RestoreArchive, false);
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::RestoreArchiveSource)),
        "the restore source prompt opened"
    );
    type_str(&mut app, &out.to_string_lossy());
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::RestoreArchiveDest)),
        "the restore destination prompt opened"
    );
    if let Mode::Prompt(state) = &mut app.mode {
        state.clear();
    }
    type_str(&mut app, &dest.to_string_lossy());
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.root, dest, "the restored project is now open");
    assert!(Project::manifest_path(&dest).exists());
    assert_eq!(
        fs::read_to_string(dest.join("one.md")).unwrap(),
        "first chapter\n",
        "the chapter bytes round-tripped"
    );

    let _ = fs::remove_dir_all(out.parent().unwrap());
    let _ = fs::remove_dir_all(dest.parent().unwrap());
}

#[test]
fn restoring_refuses_a_non_archive_file() {
    isolate("archive-badsrc");
    let (mut app, _dir) = two_chapter_app("archive-badsrc");
    let bogus = app_scratch_dir("archive-badsrc-src").join("not.zip");
    fs::write(&bogus, "not a zip").unwrap();

    app.run_action(Action::RestoreArchive, false);
    type_str(&mut app, &bogus.to_string_lossy());
    app.on_key(key(KeyCode::Enter));

    // A readable non-archive advances to the destination prompt, then fails on
    // unpack; an unreadable one is rejected up front. Either way the project is
    // unchanged and a message is set.
    if matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::RestoreArchiveDest)) {
        let dest = app_scratch_dir("archive-badsrc-dest").join("X");
        if let Mode::Prompt(state) = &mut app.mode {
            state.clear();
        }
        type_str(&mut app, &dest.to_string_lossy());
        app.on_key(key(KeyCode::Enter));
        assert!(app
            .message
            .clone()
            .unwrap_or_default()
            .contains("Restore failed"));
        let _ = fs::remove_dir_all(dest.parent().unwrap());
    }
    assert_ne!(
        app.project.root,
        app_scratch_dir("archive-badsrc-dest").join("X")
    );
    let _ = fs::remove_dir_all(bogus.parent().unwrap());
}

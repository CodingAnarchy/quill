//! A book written on more than one device (#1298): the open chapter picks up a
//! change made elsewhere, unsaved words are never overwritten, and conflict
//! copies a sync leaves are named.

use std::fs;

use super::*;

/// The open chapter's file, rewritten as another device would, with its
/// modification time moved on so the change is visible however fast the test.
fn change_elsewhere(app: &App, text: &str) {
    let path = app.project.active_path();
    fs::write(&path, text).expect("write as another device");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|f| f.set_modified(later))
        .expect("move the time on");
}

#[test]
fn a_change_from_another_device_shows_in_the_open_chapter() {
    let (mut app, _dir) = two_chapter_app("synced-reload");
    app.notice_changes_on_disk(); // the chapter as opened
    change_elsewhere(&app, "first chapter, edited on the phone\n");
    app.notice_changes_on_disk();
    assert_eq!(app.buffer.text(), "first chapter, edited on the phone\n");
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("another device")),
        "{:?}",
        app.message
    );
}

#[test]
fn unsaved_words_stay_and_the_other_version_is_kept_beside_them() {
    let (mut app, dir) = two_chapter_app("synced-dirty");
    app.notice_changes_on_disk();
    type_str(&mut app, "Mine. ");
    change_elsewhere(&app, "first chapter, edited on the phone\n");
    app.notice_changes_on_disk();
    assert!(
        app.buffer.text().starts_with("Mine. "),
        "the author's words stay"
    );
    let copy = dir.join("one 2.md");
    assert_eq!(
        fs::read_to_string(&copy).expect("the other version is kept"),
        "first chapter, edited on the phone\n"
    );
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("one 2.md")),
        "{:?}",
        app.message
    );
}

#[test]
fn the_editors_own_save_is_not_a_change_from_elsewhere() {
    let (mut app, _dir) = two_chapter_app("synced-own");
    app.notice_changes_on_disk();
    type_str(&mut app, "Mine. ");
    app.flush_active().expect("save");
    app.settle_saves();
    app.message = None;
    app.notice_changes_on_disk();
    assert_eq!(app.message, None, "nothing to say about our own save");
    assert!(app.buffer.text().starts_with("Mine. "));
}

#[test]
fn a_conflict_copy_in_a_synced_book_is_named_once() {
    let (mut app, dir) = two_chapter_app("synced-conflict");
    crate::synced::mark_synced_for_test(&dir);
    fs::write(dir.join("two 2.md"), "The phone's version.\n").expect("copy");
    app.notice_changes_on_disk();
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("\u{201c}Two\u{201d}") && m.contains("two 2.md")),
        "{:?}",
        app.message
    );
    app.message = None;
    app.last_conflict_scan = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(60))
        .expect("an instant a minute ago");
    app.notice_changes_on_disk();
    assert_eq!(app.message, None, "named once");
}

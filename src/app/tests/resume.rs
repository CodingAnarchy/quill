//! Resume the last editing position (#588): a clean save/quit records the active
//! chapter and cursor to `.quill/`, and the next open restores them — separate
//! from crash recovery, which restores unsaved *text*, not the last *location*.

use super::*;
use crate::lastpos;

#[test]
fn a_save_records_the_active_chapter_and_cursor() {
    let (mut app, dir) = two_chapter_app("resume-save");
    // Move to chapter two and put the cursor a few bytes in.
    app.on_key(ctrl('n'));
    assert_eq!(app.project.active_index(), 1);
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    let cursor = app.buffer.cursor_byte();
    app.on_key(ctrl('s')); // clean save persists the position

    let pos = lastpos::read(&dir).expect("a save records the position");
    assert_eq!(pos.chapter_file, "two.md");
    assert_eq!(pos.cursor, cursor);
}

#[test]
fn reopening_restores_the_active_chapter_and_cursor() {
    let dir = app_scratch_dir("resume-restore");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();

    // First session: land on chapter two, move the cursor, quit cleanly.
    {
        let mut app = open_test_app(&dir);
        app.on_key(ctrl('n'));
        for _ in 0..7 {
            app.on_key(key(KeyCode::Right));
        }
        let cursor = app.buffer.cursor_byte();
        app.on_key(ctrl('s'));
        assert_eq!(lastpos::read(&dir).unwrap().cursor, cursor);
    }

    // Next launch resumes there rather than at chapter 0, top of file — loading
    // chapter two directly.
    let app = launch_test_app(&dir);
    assert_eq!(app.project.active_index(), 1, "resumed on chapter two");
    assert_eq!(
        app.buffer.text(),
        "second chapter\n",
        "chapter two is loaded"
    );
    assert_eq!(app.buffer.cursor_byte(), 7, "resumed at the saved offset");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_stale_chapter_reference_falls_back_to_the_top() {
    let dir = two_chapter_dir("resume-stale");
    // A record for a chapter that no longer exists.
    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "ghost.md".into(),
            cursor: 5,
        },
    )
    .unwrap();

    let app = launch_test_app(&dir);
    assert_eq!(
        app.project.active_index(),
        0,
        "a missing chapter starts at the top"
    );
    assert_eq!(app.buffer.text(), "first chapter\n");
    assert_eq!(app.buffer.cursor_byte(), 0);
}

#[test]
fn an_offset_past_the_end_is_clamped() {
    let dir = two_chapter_dir("resume-clamp");
    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "one.md".into(),
            cursor: 9_999, // far past the short chapter
        },
    )
    .unwrap();

    let app = launch_test_app(&dir);
    assert_eq!(app.project.active_index(), 0);
    assert!(
        app.buffer.cursor_byte() <= app.buffer.len_bytes(),
        "the cursor is clamped into the buffer"
    );
}

#[test]
fn no_record_leaves_the_open_at_the_top() {
    let dir = two_chapter_dir("resume-none");
    let app = launch_test_app(&dir); // nothing on disk
    assert_eq!(app.project.active_index(), 0);
    assert_eq!(app.buffer.cursor_byte(), 0);
}

#[test]
fn a_mid_codepoint_offset_does_not_panic_at_startup() {
    // The persisted offset only clamps past-the-end, not mid-codepoint. If the
    // chapter's bytes shifted since it was saved — an external edit, an older
    // version, or smart typography rewriting "--" into a 3-byte em-dash — the
    // saved offset can fall inside a multi-byte grapheme. Before the boundary
    // snap in `cursor_at_byte`, restoring it panicked ("byte index N is not a
    // char boundary") before the first draw, bricking reopen until the sidecar
    // was cleared. It must now floor to a valid boundary instead.
    let dir = app_scratch_dir("resume-mid-codepoint");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    // "He paused—then ran." — the em-dash is a 3-byte grapheme.
    let body = "He paused\u{2014}then ran.\n";
    fs::write(dir.join("one.md"), body).unwrap();
    let dash = body.find('\u{2014}').unwrap();

    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "one.md".into(),
            cursor: dash + 1, // inside the em-dash, not on a boundary
        },
    )
    .unwrap();

    let app = launch_test_app(&dir); // panicked here before the fix
    assert_eq!(app.project.active_index(), 0);
    // Floored back onto the em-dash's boundary rather than crashing.
    assert_eq!(app.buffer.cursor_byte(), dash);

    fs::remove_dir_all(&dir).ok();
}

/// Mark the position sidecar with trailing whitespace (still valid JSON): any
/// rewrite drops it, so its survival proves the save skipped the write.
fn mark_position(dir: &Path) -> String {
    let path = dir.join(".quill").join("position.json");
    let marked = format!("{} ", fs::read_to_string(&path).unwrap());
    fs::write(&path, &marked).unwrap();
    marked
}

#[test]
fn a_save_with_the_cursor_unmoved_does_not_rewrite_the_position() {
    let (mut app, dir) = two_chapter_app("resume-unchanged");
    app.on_key(key(KeyCode::Right));
    app.on_key(ctrl('s'));
    let marked = mark_position(&dir);
    let read_pos = || fs::read_to_string(dir.join(".quill").join("position.json")).unwrap();

    // Saving again with the cursor where it was leaves the record untouched.
    app.on_key(ctrl('s'));
    app.on_key(ctrl('s'));
    assert_eq!(read_pos(), marked, "unchanged position, no rewrite");

    // Moving the cursor and saving writes the new spot.
    app.on_key(key(KeyCode::Right));
    let cursor = app.buffer.cursor_byte();
    app.on_key(ctrl('s'));
    assert_ne!(read_pos(), marked);
    assert_eq!(lastpos::read(&dir).unwrap().cursor, cursor);
}

#[test]
fn a_position_read_at_launch_is_not_rewritten_by_a_save_there() {
    let dir = app_scratch_dir("resume-read-seed");
    let dir = TempProject::new(dir);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "one.md".into(),
            cursor: 3,
        },
    )
    .unwrap();
    let marked = mark_position(&dir);

    let mut app = launch_test_app(&dir);
    app.on_key(ctrl('s'));
    assert_eq!(
        fs::read_to_string(dir.join(".quill").join("position.json")).unwrap(),
        marked
    );
}

#[test]
fn an_unreadable_remembered_chapter_falls_back_to_the_first() {
    // The remembered chapter won't read (a directory where its file should be):
    // the launch opens on the first chapter instead rather than refusing to open
    // the book, and the on-open damage check flags the unreadable chapter.
    let dir = two_chapter_dir("resume-unreadable");
    fs::remove_file(dir.join("two.md")).unwrap();
    fs::create_dir(dir.join("two.md")).unwrap();
    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "two.md".into(),
            cursor: 3,
        },
    )
    .unwrap();

    let app = launch_test_app(&dir);
    assert_eq!(app.project.active_index(), 0, "fell back to chapter one");
    assert_eq!(app.buffer.text(), "first chapter\n");
    assert_eq!(
        app.buffer.cursor_byte(),
        0,
        "the saved offset isn't applied"
    );
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("looks damaged")),
        "the launch reports the damage: {:?}",
        app.message
    );
}

#[test]
fn a_chapter_in_another_book_of_a_series_starts_at_the_top() {
    // The record names a chapter of a book that isn't the active one: it isn't in
    // the active manifest, so the launch starts at the active book's first chapter.
    let dir = TempProject::new(app_scratch_dir("resume-other-book"));
    for sub in ["b1", "b2"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("b1/one.md"), "book one\n").unwrap();
    fs::write(dir.join("b2/one.md"), "book two\n").unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Series\"\n\n\
         [[books]]\ntitle = \"One\"\n\n[[books.chapters]]\ntitle = \"A\"\nfile = \"b1/one.md\"\n\n\
         [[books]]\ntitle = \"Two\"\n\n[[books.chapters]]\ntitle = \"B\"\nfile = \"b2/one.md\"\n",
    )
    .unwrap();
    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "b2/one.md".into(),
            cursor: 2,
        },
    )
    .unwrap();

    let app = launch_test_app(&dir);
    assert_eq!(app.project.active_book(), 0);
    assert_eq!(app.project.active_file(), "b1/one.md");
    assert_eq!(app.buffer.text(), "book one\n");
    assert_eq!(app.buffer.cursor_byte(), 0);
}

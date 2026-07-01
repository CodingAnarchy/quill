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

    // Next open resumes there rather than at chapter 0, top of file.
    let mut app = open_test_app(&dir);
    app.restore_position();
    assert_eq!(app.project.active_index(), 1, "resumed on chapter two");
    assert_eq!(app.buffer.cursor_byte(), 7, "resumed at the saved offset");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_stale_chapter_reference_falls_back_to_the_top() {
    let (mut app, dir) = two_chapter_app("resume-stale");
    // A record for a chapter that no longer exists.
    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "ghost.md".into(),
            cursor: 5,
        },
    )
    .unwrap();

    app.restore_position();
    assert_eq!(
        app.project.active_index(),
        0,
        "a missing chapter starts at the top"
    );
}

#[test]
fn an_offset_past_the_end_is_clamped() {
    let (mut app, dir) = two_chapter_app("resume-clamp");
    lastpos::write(
        &dir,
        &lastpos::Position {
            chapter_file: "one.md".into(),
            cursor: 9_999, // far past the short chapter
        },
    )
    .unwrap();

    app.restore_position();
    assert_eq!(app.project.active_index(), 0);
    assert!(
        app.buffer.cursor_byte() <= app.buffer.len_bytes(),
        "the cursor is clamped into the buffer"
    );
}

#[test]
fn no_record_leaves_the_open_at_the_top() {
    let (mut app, _dir) = two_chapter_app("resume-none");
    app.restore_position(); // nothing on disk
    assert_eq!(app.project.active_index(), 0);
    assert_eq!(app.buffer.cursor_byte(), 0);
}

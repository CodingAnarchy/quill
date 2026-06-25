//! Inline author annotations (#302): a note anchored to a chapter byte range,
//! marked in the editor, re-anchored as the prose is edited, listed/navigable,
//! and kept out of the chapter `.md` (so exports stay clean).

use super::*;
use crate::buffer::Cursor;
use crate::mode::Mode;
use crate::project::Project;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[test]
fn annotation_persists_in_a_side_file_and_round_trips() {
    let (mut app, dir) = two_chapter_app("anno-roundtrip");
    // "first chapter\n" — annotate "first".
    app.create_annotation(0, 5, "check the opening".into());
    assert_eq!(app.annotations.len(), 1);

    app.on_key(ctrl('s')); // save the chapter

    // The chapter file stays byte-for-byte clean — the note never enters the
    // prose, which is why exports (they read the chapter bodies) never carry it.
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), "first chapter\n");
    // The note lives in the side file instead, keyed by chapter.
    let side = fs::read_to_string(dir.join("annotations.toml")).unwrap();
    assert!(side.contains("check the opening") && side.contains("one.md"), "{side}");

    // It survives a reload of the project.
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live.len(), 1);
    assert_eq!((live[0].start, live[0].end, live[0].note.as_str()), (0, 5, "check the opening"));
}

#[test]
fn annotating_via_the_prompt_anchors_the_word_at_the_cursor() {
    let (mut app, _dir) = two_chapter_app("anno-prompt");
    // Cursor inside "chapter" (bytes 6..13 of "first chapter").
    app.buffer.cursor = Cursor { line: 0, col: 8 };
    app.start_annotation();
    assert!(matches!(app.mode, Mode::Prompt(_)), "the note prompt opened");

    type_str(&mut app, "is this the right word");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.annotations.len(), 1);
    let a = &app.annotations[0];
    assert_eq!((a.start, a.end), (6, 13), "the whole word under the cursor is anchored");
    assert_eq!(a.note, "is this the right word");
}

#[test]
fn an_empty_note_creates_nothing() {
    let (mut app, _dir) = two_chapter_app("anno-empty");
    app.start_annotation_range(0, 5);
    app.on_key(key(KeyCode::Enter)); // confirm with no text
    assert!(app.annotations.is_empty(), "a blank note is discarded");
}

#[test]
fn anchor_slides_when_text_is_inserted_before_it() {
    let (mut app, _dir) = two_chapter_app("anno-shift");
    app.create_annotation(6, 13, "note on chapter".into()); // "chapter"

    // Type three characters at the very start, before the anchored span.
    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle(); // re-anchoring runs off the keystroke path

    let a = &app.annotations[0];
    assert_eq!((a.start, a.end), (9, 16), "the note slid right by the inserted text");
}

#[test]
fn deleting_the_anchored_text_drops_the_note() {
    let (mut app, _dir) = two_chapter_app("anno-drop");
    app.create_annotation(0, 5, "note".into()); // "first"

    // Erase the whole anchored word.
    app.buffer.cursor = Cursor { line: 0, col: 5 };
    for _ in 0..5 {
        app.on_key(key(KeyCode::Backspace));
    }
    app.on_idle();

    assert!(app.annotations.is_empty(), "a note whose text is gone is dropped");
}

#[test]
fn review_list_jumps_to_and_deletes_annotations() {
    let (mut app, _dir) = two_chapter_app("anno-review");
    app.create_annotation(0, 5, "first note".into());
    app.create_annotation(6, 13, "second note".into());

    app.open_annotation_review();
    assert!(matches!(app.mode, Mode::Annotations { selected: 0 }));

    // Move to the second annotation and jump — the cursor lands on its start.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.cursor_byte(), 6, "Enter jumps the cursor to the annotation");

    // Reopen and delete the first; one remains.
    app.open_annotation_review();
    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.annotations.len(), 1);
    assert_eq!(app.annotations[0].note, "second note");
}

#[test]
fn annotations_are_not_offered_in_the_continuous_view() {
    let (mut app, _dir) = two_chapter_app("anno-continuous");
    app.toggle_continuous();
    assert!(app.manuscript.is_some(), "in the continuous view");

    app.start_annotation_range(0, 5);
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no note prompt in the continuous view");
    assert!(app.message.as_deref().unwrap_or("").contains("single-chapter view"));
}

#[test]
fn undo_and_redo_move_annotations_with_the_text() {
    let (mut app, _dir) = two_chapter_app("anno-undo");
    // "first chapter\n" — annotate "chapter" (bytes 6..13).
    app.create_annotation(6, 13, "note on chapter".into());

    // Type three chars at the very start; on idle the note re-anchors right.
    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();
    assert_eq!((app.annotations[0].start, app.annotations[0].end), (9, 16), "slid right by the insert");

    // Undo the insertion → the note returns to its original span (#370).
    app.on_key(ctrl('z'));
    assert_eq!((app.annotations[0].start, app.annotations[0].end), (6, 13), "undo restores the anchor");

    // Redo → it follows the text forward again.
    app.on_key(ctrl('y'));
    assert_eq!((app.annotations[0].start, app.annotations[0].end), (9, 16), "redo re-applies");
}

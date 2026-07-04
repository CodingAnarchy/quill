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
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "first chapter\n"
    );
    // The note lives in the side file instead, keyed by chapter.
    let side = fs::read_to_string(dir.join("annotations.toml")).unwrap();
    assert!(
        side.contains("check the opening") && side.contains("one.md"),
        "{side}"
    );

    // It survives a reload of the project.
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live.len(), 1);
    assert_eq!(
        (live[0].start, live[0].end, live[0].note.as_str()),
        (0, 5, "check the opening")
    );
}

#[test]
fn annotating_via_the_prompt_anchors_the_word_at_the_cursor() {
    let (mut app, _dir) = two_chapter_app("anno-prompt");
    // Cursor inside "chapter" (bytes 6..13 of "first chapter").
    app.buffer.cursor = Cursor { line: 0, col: 8 };
    app.start_annotation();
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the note prompt opened"
    );

    type_str(&mut app, "is this the right word");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.annotations.len(), 1);
    let a = &app.annotations[0];
    assert_eq!(
        (a.start, a.end),
        (6, 13),
        "the whole word under the cursor is anchored"
    );
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
    assert_eq!(
        (a.start, a.end),
        (9, 16),
        "the note slid right by the inserted text"
    );
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

    assert!(
        app.annotations.is_empty(),
        "a note whose text is gone is dropped"
    );
}

#[test]
fn review_list_jumps_to_and_deletes_annotations() {
    let (mut app, _dir) = two_chapter_app("anno-review");
    app.create_annotation(0, 5, "first note".into());
    app.create_annotation(6, 13, "second note".into());

    app.open_annotation_review();
    assert!(matches!(app.mode, Mode::Annotations { selected: 0, .. }));

    // Move to the second annotation and jump — the cursor lands on its start.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.cursor_byte(),
        6,
        "Enter jumps the cursor to the annotation"
    );

    // Reopen and delete the first; one remains.
    app.open_annotation_review();
    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.annotations.len(), 1);
    assert_eq!(app.annotations[0].note, "second note");
}

#[test]
fn annotations_can_be_created_and_anchored_in_the_continuous_view() {
    let (mut app, dir) = three_chapter_app("anno-continuous");
    app.toggle_continuous();
    assert!(app.manuscript.is_some(), "in the continuous view");

    // Annotate "alpha" (buffer bytes 0..5) — the first loaded chapter.
    app.start_annotation_range(0, 5);
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the note prompt opens in continuous view"
    );
    type_str(&mut app, "first note");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.annotations.len(), 1);
    assert_eq!(
        app.annotations[0].chapter, "c0.md",
        "tagged to the chapter that owns the span"
    );

    // Saving writes it to that chapter's side file, translated chapter-local.
    app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("c0.md");
    assert_eq!(live.len(), 1);
    assert_eq!(
        (live[0].start, live[0].end),
        (0, 5),
        "stored chapter-local, not buffer-absolute"
    );
}

#[test]
fn an_edit_re_anchors_a_later_chapters_note_in_the_continuous_view() {
    let (mut app, dir) = three_chapter_app("anno-continuous-edit");
    app.toggle_continuous();
    // "beta" is the second loaded chapter, after "alpha\n" (6 bytes).
    let beta_start = app.manuscript.as_ref().unwrap().segment_spans()[1].1;
    app.start_annotation_range(beta_start, beta_start + 4); // "beta"
    type_str(&mut app, "n");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.annotations[0].chapter, "c1.md");

    // Type three chars at the very start (chapter 0); the note in chapter 1 slides.
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();
    assert_eq!(
        app.annotations[0].start,
        beta_start + 3,
        "the later note re-anchored"
    );

    // It still persists chapter-local (unshifted) to its own chapter on save.
    app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    let reopened = Project::open(&dir).unwrap();
    let c1 = reopened.annotations_for("c1.md");
    assert_eq!(
        (c1[0].start, c1[0].end),
        (0, 4),
        "chapter-local offset unchanged by ch 0's edit"
    );
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
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (9, 16),
        "slid right by the insert"
    );

    // Undo the insertion → the note returns to its original span (#370).
    app.on_key(ctrl('z'));
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (6, 13),
        "undo restores the anchor"
    );

    // Redo → it follows the text forward again.
    app.on_key(ctrl('y'));
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (9, 16),
        "redo re-applies"
    );
}

/// The annotation list's `(selected, filter, typing)` state.
fn anno_state(app: &App) -> (usize, String, bool) {
    match &app.mode {
        Mode::Annotations {
            selected,
            filter,
            typing,
        } => (*selected, filter.clone(), *typing),
        _ => panic!("expected the annotation list"),
    }
}

#[test]
fn slash_filters_annotations_by_note_text_and_enter_jumps() {
    // "first chapter\n": "first" = 0..5, "chapter" = 6..13.
    let (mut app, _dir) = two_chapter_app("anno-filter-jump");
    app.create_annotation(0, 5, "opening beat".into());
    app.create_annotation(6, 13, "closing beat".into());
    app.open_annotation_review();
    assert!(matches!(app.mode, Mode::Annotations { selected: 0, .. }));

    // `/` then "closing" narrows to the second note; the selection snaps to it.
    app.on_key(key(KeyCode::Char('/')));
    assert!(anno_state(&app).2);
    type_str(&mut app, "closing");
    assert_eq!(anno_state(&app).0, 1, "selection follows the only match");

    // Enter leaves the input sub-mode, then acts on the match: jump to byte 6.
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.buffer.cursor_byte(), 6);
}

#[test]
fn annotation_filter_matches_the_anchored_snippet_too() {
    // The snippet ("chapter") should match even when the note text doesn't.
    let (mut app, _dir) = two_chapter_app("anno-filter-snippet");
    app.create_annotation(0, 5, "note A".into()); // anchors "first"
    app.create_annotation(6, 13, "note B".into()); // anchors "chapter"
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "chapter"); // only the second note's snippet matches
    assert_eq!(anno_state(&app).0, 1);
}

#[test]
fn deleting_a_filtered_annotation_removes_the_right_one() {
    let (mut app, _dir) = two_chapter_app("anno-filter-delete");
    app.create_annotation(0, 5, "keep me".into());
    app.create_annotation(6, 13, "drop me".into());
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "drop");
    app.on_key(key(KeyCode::Enter)); // leave input, keep the filter
    app.on_key(key(KeyCode::Char('d'))); // delete the filtered match

    assert_eq!(app.annotations.len(), 1);
    assert_eq!(app.annotations[0].note, "keep me");
}

#[test]
fn annotation_esc_clears_the_filter_then_closes() {
    let (mut app, _dir) = two_chapter_app("anno-filter-esc");
    app.create_annotation(0, 5, "alpha".into());
    app.create_annotation(6, 13, "beta".into());
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "beta");
    app.on_key(key(KeyCode::Esc)); // leave input, keep filter
    assert_eq!(anno_state(&app).1, "beta");

    app.on_key(key(KeyCode::Esc)); // clear filter; list restored
    assert_eq!(anno_state(&app).1, "");

    app.on_key(key(KeyCode::Esc)); // close
    assert!(matches!(app.mode, Mode::Insert));
}

//! Removing annotations from the editor: the note under the cursor, the notes a
//! Visual selection overlaps, and every note after a y/N confirm — each persisted
//! to the side file, unlike the prose, which never changes.

use super::*;
use crate::buffer::Cursor;
use crate::mode::{Action, Mode};
use crate::project::Project;

/// "first chapter\n" with a note on "first" (0..5) and one on "chapter" (6..13).
fn two_notes(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.create_annotation(0, 5, "opening".into());
    app.create_annotation(6, 13, "noun".into());
    (app, dir)
}

fn notes(app: &App) -> Vec<&str> {
    app.annotations.iter().map(|a| a.note.as_str()).collect()
}

#[test]
fn remove_at_cursor_drops_only_the_note_under_it_and_persists() {
    let (mut app, dir) = two_notes("anno-remove-cursor");
    app.buffer.cursor = Cursor { line: 0, col: 8 }; // inside "chapter"
    app.run_action(Action::RemoveAnnotation, false);

    assert_eq!(notes(&app), ["opening"]);
    assert_eq!(app.message.as_deref(), Some("Annotation removed."));
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.annotations_for("one.md").len(), 1);
    // The prose is untouched.
    assert_eq!(app.buffer.text(), "first chapter\n");
}

#[test]
fn remove_at_cursor_counts_the_spot_just_past_the_word() {
    let (mut app, _dir) = two_notes("anno-remove-edge");
    app.buffer.cursor = Cursor { line: 0, col: 13 }; // right after "chapter"
    app.run_action(Action::RemoveAnnotation, false);
    assert_eq!(notes(&app), ["opening"]);
}

#[test]
fn remove_with_nothing_under_the_cursor_says_so() {
    let (mut app, _dir) = two_chapter_app("anno-remove-none");
    app.create_annotation(0, 5, "opening".into());
    app.buffer.cursor = Cursor { line: 0, col: 9 };
    app.run_action(Action::RemoveAnnotation, false);
    assert_eq!(notes(&app), ["opening"]);
    assert_eq!(app.message.as_deref(), Some("No annotation at the cursor."));
}

#[test]
fn remove_drops_every_note_a_visual_selection_overlaps() {
    let (mut app, _dir) = two_notes("anno-remove-visual");
    // Select from "rst" into "cha" — overlapping both notes.
    app.buffer.cursor = Cursor { line: 0, col: 8 };
    app.mode = Mode::Visual {
        anchor: 2,
        linewise: false,
    };
    app.remove_annotation_at_cursor();
    assert!(app.annotations.is_empty());
    assert_eq!(app.message.as_deref(), Some("2 annotations removed."));
}

#[test]
fn clearing_asks_first_and_only_y_removes() {
    let (mut app, dir) = two_notes("anno-clear");
    app.run_action(Action::ClearAnnotations, false);
    assert!(matches!(app.mode, Mode::Prompt(_)), "a y/N confirm opens");
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(app.annotations.len(), 2, "declining keeps every note");

    app.run_action(Action::ClearAnnotations, false);
    app.on_key(key(KeyCode::Char('y')));
    assert!(app.annotations.is_empty());
    assert!(!matches!(app.mode, Mode::Prompt(_)), "the prompt closes");
    let reopened = Project::open(&dir).unwrap();
    assert!(reopened.annotations_for("one.md").is_empty());
}

#[test]
fn clearing_with_no_notes_needs_no_confirm() {
    let (mut app, _dir) = two_chapter_app("anno-clear-none");
    app.run_action(Action::ClearAnnotations, false);
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    assert_eq!(app.message.as_deref(), Some("No annotations here."));
}

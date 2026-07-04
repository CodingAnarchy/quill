//! Author-defined text snippets (#411): the picker and verbatim insertion.

use super::*;
use crate::config::Snippet;
use crate::mode::Action;

fn snip(name: &str, text: &str) -> Snippet {
    Snippet {
        name: name.into(),
        text: text.into(),
    }
}

#[test]
fn insert_snippet_inserts_the_literal_text_at_the_cursor() {
    let (mut app, _dir) = two_chapter_app("snippet-insert");
    app.snippets = vec![snip("scene", "* * *")];
    // The cursor rests at the start of "first chapter\n".
    app.insert_snippet(0);
    assert!(
        app.buffer.line(0).starts_with("* * *first chapter"),
        "got: {:?}",
        app.buffer.line(0)
    );
}

#[test]
fn an_inserted_snippet_is_one_undo() {
    let (mut app, _dir) = two_chapter_app("snippet-undo");
    app.snippets = vec![snip("scene", "* * *")];
    app.insert_snippet(0);
    assert!(app.buffer.line(0).starts_with("* * *"));
    // A single undo removes the whole snippet, not one character at a time.
    app.on_key(ctrl('z'));
    assert!(
        app.buffer.line(0).starts_with("first chapter"),
        "got: {:?}",
        app.buffer.line(0)
    );
}

#[test]
fn snippet_insertion_is_verbatim_ignoring_smart_typography() {
    let (mut app, _dir) = two_chapter_app("snippet-literal");
    app.smart_typography = true; // would curl quotes / dash on the typing path
    app.snippets = vec![snip("quote", "\"--\"")];
    app.insert_snippet(0);
    // The configured text lands exactly — straight quotes, double hyphen.
    assert!(
        app.buffer.line(0).starts_with("\"--\""),
        "got: {:?}",
        app.buffer.line(0)
    );
}

#[test]
fn the_picker_lists_snippets_and_enter_inserts_the_selected_one() {
    let (mut app, _dir) = two_chapter_app("snippet-picker");
    app.snippets = vec![snip("break", "* * *"), snip("name", "Aelindra")];
    app.open_snippet_picker();
    assert!(matches!(app.mode, Mode::Palette(_)), "the picker opened");
    // Select the second snippet and insert it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.mode, Mode::Insert),
        "the picker closed back to the editor"
    );
    assert!(
        app.buffer.line(0).starts_with("Aelindra"),
        "got: {:?}",
        app.buffer.line(0)
    );
}

#[test]
fn the_run_action_menu_marks_snippets_unavailable_when_none_are_defined() {
    let (mut app, _dir) = two_chapter_app("snippet-none");
    app.snippets = vec![];
    assert!(app.action_status(Action::InsertSnippet).is_some());
    // Opening it anyway is a no-op with a hint, not an empty palette.
    app.open_snippet_picker();
    assert!(matches!(app.mode, Mode::Insert));
    assert!(
        app.message.as_deref().unwrap_or("").contains("No snippets"),
        "{:?}",
        app.message
    );
}

#[test]
fn a_caret_marker_lands_the_cursor_at_its_position() {
    let (mut app, _dir) = two_chapter_app("snippet-caret");
    // The caret `$0` sits between the quotes; it's consumed, not inserted.
    app.snippets = vec![snip("quote", "\u{201c}$0\u{201d}")];
    app.insert_snippet(0);
    // The marker text is gone — only the two curly quotes reach the buffer.
    assert!(
        app.buffer
            .line(0)
            .starts_with("\u{201c}\u{201d}first chapter"),
        "got: {:?}",
        app.buffer.line(0)
    );
    // The cursor rests between the quotes (one grapheme in, not at the end).
    assert_eq!(app.buffer.cursor.line, 0);
    assert_eq!(app.buffer.cursor.col, 1);
}

#[test]
fn a_snippet_without_a_caret_leaves_the_cursor_at_the_end() {
    let (mut app, _dir) = two_chapter_app("snippet-no-caret");
    app.snippets = vec![snip("scene", "* * *")];
    app.insert_snippet(0);
    // Unchanged behavior: text inserted verbatim, cursor after the last char.
    assert!(
        app.buffer.line(0).starts_with("* * *first chapter"),
        "got: {:?}",
        app.buffer.line(0)
    );
    assert_eq!(app.buffer.cursor.line, 0);
    assert_eq!(app.buffer.cursor.col, "* * *".chars().count());
}

#[test]
fn an_escaped_caret_inserts_a_literal_dollar_zero() {
    let (mut app, _dir) = two_chapter_app("snippet-escaped-caret");
    // `$$0` is the escape: a literal `$0` with no caret, cursor at the end.
    app.snippets = vec![snip("cost", "costs $$0 today")];
    app.insert_snippet(0);
    assert!(
        app.buffer.line(0).starts_with("costs $0 today"),
        "got: {:?}",
        app.buffer.line(0)
    );
    assert_eq!(app.buffer.cursor.col, "costs $0 today".chars().count());
}

#[test]
fn only_the_first_caret_is_consumed() {
    let (mut app, _dir) = two_chapter_app("snippet-second-caret");
    // A second `$0` is not a caret — it inserts literally after the cursor slot.
    app.snippets = vec![snip("pair", "a$0b$0c")];
    app.insert_snippet(0);
    // Only the first `$0` is stripped; the second stays as text.
    assert!(
        app.buffer.line(0).starts_with("ab$0cfirst chapter"),
        "got: {:?}",
        app.buffer.line(0)
    );
    // Cursor lands after "a", before "b$0c" (the four-char tail).
    assert_eq!(app.buffer.cursor.col, 1);
}

#[test]
fn a_caret_insertion_is_one_undo() {
    let (mut app, _dir) = two_chapter_app("snippet-caret-undo");
    app.snippets = vec![snip("quote", "\u{201c}$0\u{201d}")];
    app.insert_snippet(0);
    assert!(app.buffer.line(0).starts_with("\u{201c}\u{201d}"));
    // One undo removes the whole snippet — the trailing cursor step-back is part
    // of the same sealed group, not a separate edit.
    app.on_key(ctrl('z'));
    assert!(
        app.buffer.line(0).starts_with("first chapter"),
        "got: {:?}",
        app.buffer.line(0)
    );
}

#[test]
fn a_caret_lands_the_cursor_in_the_continuous_view_too() {
    let (mut app, _dir) = three_chapter_app("snippet-caret-continuous");
    app.snippets = vec![snip("wrap", "[$0]")];
    app.toggle_continuous();
    assert!(app.manuscript.is_some(), "in the continuous view");
    app.insert_snippet(0);
    let flow = app.manuscript.as_ref().unwrap().buffer();
    assert!(flow.line(0).starts_with("[]"), "got: {:?}", flow.line(0));
    // Cursor rests between the brackets.
    assert_eq!(flow.cursor.col, 1);
}

#[test]
fn snippets_insert_in_the_continuous_view_too() {
    let (mut app, _dir) = three_chapter_app("snippet-continuous");
    app.snippets = vec![snip("x", "ZZ")];
    app.toggle_continuous();
    assert!(app.manuscript.is_some(), "in the continuous view");
    app.insert_snippet(0);
    // The text reaches the manuscript window's buffer at the cursor (chapter start).
    let flow = app.manuscript.as_ref().unwrap().buffer();
    assert!(flow.line(0).starts_with("ZZ"), "got: {:?}", flow.line(0));
}

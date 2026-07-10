//! Tests for join-lines (`J`) and reflow-paragraph (`gq`) (#1005): merging the
//! current line with the next with a normalized space, and re-wrapping a paragraph
//! by collapsing its hard breaks — both single undoable edits that never cross a
//! paragraph or scene boundary and never touch a word.

use super::*;

/// A Vim app in Normal mode over `text`, cursor at byte 0 — a controlled buffer
/// for the join/reflow tests.
fn jr_app(name: &str, text: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    app.buffer = Buffer::from_text(text);
    (app, dir)
}

/// The words of a buffer, in order, ignoring all whitespace — used to assert that
/// join/reflow change only the whitespace, never a word.
fn words(app: &App) -> Vec<String> {
    app.buffer
        .text()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[test]
fn j_joins_the_next_line_with_a_single_space() {
    let (mut app, _dir) = jr_app("jr-join-basic", "the cat\nsat down\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "the cat sat down\n",
        "J merged the two lines with one space"
    );
}

#[test]
fn j_normalizes_ragged_whitespace_between_the_lines() {
    let (mut app, _dir) = jr_app("jr-join-ws", "the cat   \n   sat down\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "the cat sat down\n",
        "trailing and leading whitespace collapse to one space"
    );
}

#[test]
fn count_j_joins_several_lines() {
    let (mut app, _dir) = jr_app("jr-join-count", "a\nb\nc\nd\n");
    // `3J` folds three lines (a, b, c) into one.
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.buffer.text(), "a b c\nd\n", "3J joined three lines");
}

#[test]
fn j_never_merges_across_a_blank_line() {
    let (mut app, _dir) = jr_app("jr-join-para", "end of one\n\nstart of two\n");
    // Cursor on the last line of the first paragraph; J must not pull the blank
    // line (a paragraph boundary) or the next paragraph up.
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "end of one\n\nstart of two\n",
        "J respects the paragraph boundary (blank line)"
    );
}

#[test]
fn j_never_merges_across_a_scene_break() {
    let (mut app, _dir) = jr_app("jr-join-scene", "last line\n* * *\nnext scene\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "last line\n* * *\nnext scene\n",
        "J never folds a scene break (thematic break) up"
    );
}

#[test]
fn j_is_one_undo_step() {
    let (mut app, _dir) = jr_app("jr-join-undo", "the cat\nsat down\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.buffer.text(), "the cat sat down\n");
    app.on_key(key(KeyCode::Char('u'))); // one undo
    assert_eq!(
        app.buffer.text(),
        "the cat\nsat down\n",
        "a single undo restores the pre-join text"
    );
}

#[test]
fn dot_repeats_a_join() {
    // The dot-repeat cross-check (#1004): `J` flows through the change-dispatch
    // path and mutates via the standard edit primitive, so `.` repeats it with no
    // extra wiring.
    let (mut app, _dir) = jr_app("jr-join-dot", "one\ntwo\nthree\nfour\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.buffer.text(), "one two\nthree\nfour\n", "J joined once");
    // `.` repeats the join at the cursor, folding the next line up again.
    app.on_key(key(KeyCode::Char('.')));
    assert_eq!(
        app.buffer.text(),
        "one two three\nfour\n",
        ". repeated the join"
    );
}

#[test]
fn gq_reflows_collapsing_hard_breaks() {
    // A paragraph broken into short ragged lines re-wraps to the wrap column,
    // collapsing every internal hard break.
    let (mut app, _dir) = jr_app(
        "jr-reflow-collapse",
        "the quick\nbrown fox\njumps over\nthe lazy dog\n",
    );
    app.wrap_width.set(72); // a wide column, so the whole paragraph fits on one line
    let before = words(&app);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "the quick brown fox jumps over the lazy dog\n",
        "gq collapsed the hard breaks into one flowed line"
    );
    assert_eq!(before, words(&app), "reflow changed no word");
}

#[test]
fn gq_rewraps_to_the_configured_column_without_changing_words() {
    let (mut app, _dir) = jr_app(
        "jr-reflow-column",
        "alpha beta gamma delta epsilon zeta eta\n",
    );
    app.wrap_width.set(12); // a narrow column forces re-breaking
    let before = words(&app);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    // Every re-wrapped line stays within the column, and no line is blank.
    for line in app.buffer.text().lines() {
        assert!(!line.is_empty(), "reflow left no blank line");
        assert!(
            unicode_width::UnicodeWidthStr::width(line) <= 12,
            "line {line:?} exceeds the wrap column"
        );
    }
    assert_eq!(
        before,
        words(&app),
        "reflow preserved every word, byte-for-byte and in order"
    );
}

#[test]
fn gq_does_not_reach_across_a_blank_line() {
    let (mut app, _dir) = jr_app(
        "jr-reflow-para",
        "first para line one\nfirst para line two\n\nsecond para\n",
    );
    app.wrap_width.set(72);
    // Cursor in the first paragraph; reflow must only touch it.
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "first para line one first para line two\n\nsecond para\n",
        "reflow stayed inside the paragraph, leaving the blank line and next paragraph"
    );
}

#[test]
fn gq_does_not_reach_across_a_scene_break() {
    let (mut app, _dir) = jr_app(
        "jr-reflow-scene",
        "scene one\nline two\n* * *\nscene two\nline two\n",
    );
    app.wrap_width.set(72);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "scene one line two\n* * *\nscene two\nline two\n",
        "reflow never crosses a scene break"
    );
}

#[test]
fn gq_is_one_undo_step() {
    let (mut app, _dir) = jr_app("jr-reflow-undo", "the quick\nbrown fox\n");
    app.wrap_width.set(72);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(app.buffer.text(), "the quick brown fox\n");
    app.on_key(key(KeyCode::Char('u'))); // one undo
    assert_eq!(
        app.buffer.text(),
        "the quick\nbrown fox\n",
        "a single undo restores the pre-reflow paragraph"
    );
}

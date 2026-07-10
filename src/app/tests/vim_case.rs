//! Tests for the transpose and case-toggle affordances (#1006): `gs`/`gS`
//! transpose characters/words, `~` toggles the char under the cursor, the `g~` /
//! `gu` / `gU` operator case-maps a motion or text object, each is one undo step,
//! and `.` repeats a case-toggle. All hermetic — driven through the key dispatch.

use super::*;

/// A Vim app in Normal mode over `text`, cursor at byte 0.
fn case_app(name: &str, text: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    app.buffer = Buffer::from_text(text);
    (app, dir)
}

/// Move the cursor to a byte offset in the active buffer.
fn seek(app: &mut App, byte: usize) {
    app.buffer.cursor = app.buffer.cursor_at_byte(byte);
}

#[test]
fn transpose_chars_swaps_the_two_around_the_cursor() {
    let (mut app, _dir) = case_app("case-xp", "teh\n");
    seek(&mut app, 2); // on 'h'
    type_str(&mut app, "gs");
    assert_eq!(app.buffer.text(), "the\n", "gs swapped e and h");
}

#[test]
fn transpose_words_swaps_adjacent_words() {
    let (mut app, _dir) = case_app("case-gS", "quick brown fox\n");
    seek(&mut app, 2); // inside "quick"
    type_str(&mut app, "gS");
    assert_eq!(
        app.buffer.text(),
        "brown quick fox\n",
        "gS swapped the two adjacent words"
    );
}

#[test]
fn tilde_toggles_the_case_under_the_cursor_and_advances() {
    let (mut app, _dir) = case_app("case-tilde", "hello\n");
    type_str(&mut app, "~");
    assert_eq!(app.buffer.text(), "Hello\n", "~ upper-cased the h");
    // The cursor advanced, so a second ~ toggles the next char.
    type_str(&mut app, "~");
    assert_eq!(app.buffer.text(), "HEllo\n", "~ advanced to the next char");
}

#[test]
fn tilde_respects_the_line_end() {
    let (mut app, _dir) = case_app("case-tilde-end", "ab\ncd\n");
    seek(&mut app, 2); // past "ab", at the line end
    type_str(&mut app, "~");
    assert_eq!(
        app.buffer.text(),
        "ab\ncd\n",
        "~ past the line end is a no-op"
    );
}

#[test]
fn g_tilde_operator_toggles_a_word() {
    let (mut app, _dir) = case_app("case-g-tilde-w", "Hello world\n");
    type_str(&mut app, "g~w"); // toggle over the "Hello" word motion
    assert_eq!(app.buffer.text(), "hELLO world\n", "g~w toggled the word");
}

#[test]
fn g_tilde_operator_over_a_text_object() {
    let (mut app, _dir) = case_app("case-g-tilde-iw", "one TWO three\n");
    seek(&mut app, 4); // inside "TWO"
    type_str(&mut app, "g~iw");
    assert_eq!(
        app.buffer.text(),
        "one two three\n",
        "g~iw toggled the inner word"
    );
}

#[test]
fn upper_and_lower_case_a_motion() {
    let (mut app, _dir) = case_app("case-gU-w", "hello world\n");
    type_str(&mut app, "gUw"); // upper-case "hello"
    assert_eq!(app.buffer.text(), "HELLO world\n");
    type_str(&mut app, "guiw"); // lower-case the inner word again
    assert_eq!(app.buffer.text(), "hello world\n");
}

#[test]
fn doubled_operator_upper_cases_the_whole_line() {
    let (mut app, _dir) = case_app("case-gUU", "make me loud\n");
    type_str(&mut app, "gUU");
    assert_eq!(app.buffer.text(), "MAKE ME LOUD\n", "gUU raised the line");
}

#[test]
fn each_case_op_is_a_single_undo_step() {
    let (mut app, _dir) = case_app("case-undo", "Hello World\n");
    type_str(&mut app, "g~$"); // toggle the whole line's content
    assert_eq!(app.buffer.text(), "hELLO wORLD\n");
    // One `u` reverts the entire operator edit — proof it was one group.
    app.on_key(key(KeyCode::Char('u')));
    assert_eq!(
        app.buffer.text(),
        "Hello World\n",
        "a single undo reverts the whole g~ edit"
    );
}

#[test]
fn transpose_is_a_single_undo_step() {
    let (mut app, _dir) = case_app("case-xp-undo", "teh\n");
    seek(&mut app, 2);
    type_str(&mut app, "gs");
    assert_eq!(app.buffer.text(), "the\n");
    app.on_key(key(KeyCode::Char('u')));
    assert_eq!(app.buffer.text(), "teh\n", "one undo reverts the transpose");
}

#[test]
fn dot_repeats_a_case_toggle() {
    // The dot-repeat cross-check (#1004 × #1006): `~` flows through the standard
    // change-dispatch path, so `.` repeats it with no per-command bookkeeping.
    let (mut app, _dir) = case_app("case-dot-tilde", "abcd\n");
    type_str(&mut app, "~"); // A -> toggle first char, cursor advances
    assert_eq!(app.buffer.text(), "Abcd\n", "~ toggled the first char");
    // Move the cursor and repeat with `.`; it should toggle the char there.
    seek(&mut app, 2); // on 'c'
    app.on_key(key(KeyCode::Char('.')));
    assert_eq!(app.buffer.text(), "AbCd\n", ". repeated the case-toggle");
    // And again on the next char.
    app.on_key(key(KeyCode::Char('.')));
    assert_eq!(app.buffer.text(), "AbCD\n", ". repeats the toggle again");
}

#[test]
fn dot_repeats_a_g_tilde_word_operator() {
    let (mut app, _dir) = case_app("case-dot-g-tilde", "Foo Bar Baz\n");
    type_str(&mut app, "g~w"); // toggle "Foo"
    assert_eq!(app.buffer.text(), "fOO Bar Baz\n");
    // Advance to "Bar" and repeat the whole operator with `.`.
    seek(&mut app, 4);
    app.on_key(key(KeyCode::Char('.')));
    assert_eq!(
        app.buffer.text(),
        "fOO bAR Baz\n",
        ". repeated the g~ word operator"
    );
}

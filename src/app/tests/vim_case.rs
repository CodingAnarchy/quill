//! Tests for the transpose and case-toggle affordances (#1006): `gs`/`gS`
//! transpose characters/words, `~` toggles the char under the cursor, the `g~` /
//! `gu` / `gU` operator case-maps a motion or text object, each is one undo step,
//! and `.` repeats a case-toggle. All hermetic — driven through the key dispatch.

use super::*;

/// A Vim app in Normal mode over `text`, cursor at byte 0.
fn case_app(name: &str, text: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.flags.vim = true;
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

// --- M2: `g~` then `.` must not replay a previous unrelated change ---------

#[test]
fn g_tilde_then_dot_cancels_and_does_not_replay_the_last_change() {
    // A `g~`/`gu`/`gU` armed and immediately followed by `.` must NOT hit the dot
    // fast-path — which would replay the previous change while silently discarding
    // the armed operator. `.` isn't a motion, so the case operator cancels
    // (matching Vim); no replay, no edit.
    //
    // The prior change here is a `~` toggle. If the buggy fast-path fires, it
    // replays `~` while the Toggle case operator is still armed, so the replayed
    // `~` reads as the doubled `g~~` and toggles the WHOLE line — a visible,
    // wrong edit. The fix keeps `.` off the fast-path, so nothing is replayed.
    let (mut app, _dir) = case_app("case-g-tilde-dot", "alpha bravo\n");
    type_str(&mut app, "~"); // real prior change: toggle the leading 'a' -> 'A'
    assert_eq!(
        app.buffer.text(),
        "Alpha bravo\n",
        "~ toggled the first char"
    );
    seek(&mut app, 0); // back to the line start
    let after_real = app.buffer.text();
    let seq_before = app.buffer.edit_seq();
    // Arm `g~`, then press `.`.
    type_str(&mut app, "g~");
    assert!(app.case_op.is_some(), "g~ armed the case operator");
    app.on_key(key(KeyCode::Char('.')));
    assert!(
        app.case_op.is_none(),
        "`.` cancelled the pending case operator"
    );
    assert_eq!(
        app.buffer.text(),
        after_real,
        "`.` after g~ made no edit — it neither replayed the ~ nor toggled the line"
    );
    assert_eq!(
        app.buffer.edit_seq(),
        seq_before,
        "the cancelled g~. bumped no edit sequence"
    );
}

// --- M1: a no-op case/transpose must not dirty, undo, or retarget `.` ------

#[test]
fn a_noop_case_command_does_not_dirty_undo_or_retarget_dot() {
    // `~` on a caseless char, `gU` over already-uppercase text, and `gs` on
    // identical chars all map to themselves. Each must be a true no-op: no dirty
    // flag, no undo step, and — crucially — no `.` retarget, so a prior real
    // change is still what `.` repeats (the edit_seq never moves).
    let (mut app, _dir) = case_app("case-noop", "AB 5 xx yo\n");
    // A real change first: `~` toggles the leading 'A' -> 'a'; this is what `.`
    // should keep repeating through every following no-op.
    type_str(&mut app, "~");
    assert_eq!(app.buffer.text(), "aB 5 xx yo\n", "~ toggled the A");
    // Baseline the change-tracking state after the one real edit.
    app.buffer.dirty = false;
    let seq_after_real = app.buffer.edit_seq();

    // No-op 1: `~` on the digit '5' (caseless).
    seek(&mut app, 3);
    type_str(&mut app, "~");
    // No-op 2: `gU` over a caseless span (the space at byte 2) — upper-casing a
    // space maps it to itself.
    seek(&mut app, 2);
    type_str(&mut app, "gUl");
    // No-op 3: `gs` swapping the two identical 'x' chars in "xx".
    seek(&mut app, 6); // on the second 'x'
    type_str(&mut app, "gs");

    assert_eq!(
        app.buffer.text(),
        "aB 5 xx yo\n",
        "every no-op left the text unchanged"
    );
    assert!(
        !app.buffer.dirty,
        "a no-op case/transpose did not dirty the buffer"
    );
    assert_eq!(
        app.buffer.edit_seq(),
        seq_after_real,
        "no-ops bumped no edit sequence (so `.` was not retargeted)"
    );
    // Undo should reach back past the no-ops to the one real change and revert it.
    app.on_key(key(KeyCode::Char('u')));
    assert_eq!(
        app.buffer.text(),
        "AB 5 xx yo\n",
        "one undo reverts the real change — the no-ops pushed no undo steps"
    );

    // And `.` still repeats the original `~` toggle, not any of the no-ops. Redo
    // the real change, move to a fresh cased char, and repeat.
    app.on_key(key(KeyCode::Char('.')));
    // `.` re-applies the recorded `~` at the cursor (start of line -> 'A' toggled).
    assert_eq!(
        app.buffer.text(),
        "aB 5 xx yo\n",
        "`.` repeated the original ~ toggle across the no-ops"
    );
}

// --- M3: `df<char-not-on-line>` is a full no-op ---------------------------

#[test]
fn df_with_an_absent_target_deletes_nothing() {
    // A failed operator+motion (`df` for a char that isn't on the line) is a full
    // no-op in Vim — it must NOT delete the grapheme under the cursor.
    let (mut app, _dir) = case_app("case-df-absent", "hello\n");
    let seq_before = app.buffer.edit_seq();
    type_str(&mut app, "dfz"); // 'z' isn't on the line
    assert_eq!(
        app.buffer.text(),
        "hello\n",
        "df for an absent char deleted nothing"
    );
    assert_eq!(
        app.buffer.edit_seq(),
        seq_before,
        "the cancelled df bumped no edit sequence"
    );
    assert!(
        app.vim_state.op.is_none(),
        "the delete operator was cancelled"
    );
}

#[test]
fn a_replayed_df_deletes_nothing_on_a_line_lacking_the_target() {
    // A recorded `dfx` replayed by `.` on a line without 'x' must also be a no-op
    // — it must not silently delete a char under the cursor (M3).
    let (mut app, _dir) = case_app("case-df-dot", "axbc\nqrst\n");
    type_str(&mut app, "dfx"); // deletes "ax" up to and incl. 'x'
    assert_eq!(app.buffer.text(), "bc\nqrst\n", "dfx deleted through the x");
    // Move to the second line (no 'x') and repeat with `.`.
    app.buffer.cursor = app.buffer.cursor_at_byte(3); // start of "qrst"
    let text_before_dot = app.buffer.text();
    let seq_before_dot = app.buffer.edit_seq();
    app.on_key(key(KeyCode::Char('.')));
    assert_eq!(
        app.buffer.text(),
        text_before_dot,
        "replayed dfx on a line without x deleted nothing"
    );
    assert_eq!(
        app.buffer.edit_seq(),
        seq_before_dot,
        "the no-op replay bumped no edit sequence"
    );
}

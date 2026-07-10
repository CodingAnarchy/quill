//! Tests for Vim `.` (repeat last change) (#1004): a change command replays at
//! the cursor as a single undo step, while pure motions / mode switches never
//! become the repeat target.

use super::*;

/// A Vim app in Normal mode over `text`, cursor at byte 0 — a controlled buffer
/// for the repeat tests (the shared fixture's "first chapter\n" has no repeated
/// word to exercise `.` against).
fn dot_app(name: &str, text: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    app.buffer = Buffer::from_text(text);
    (app, dir)
}

/// Send `.` once (Normal mode).
fn dot(app: &mut App) {
    app.on_key(key(KeyCode::Char('.')));
}

#[test]
fn dot_repeats_delete_word() {
    let (mut app, _dir) = dot_app("dot-dw", "one two three\n");
    // `dw` deletes "one " (word plus trailing space), leaving the cursor on "two".
    type_str(&mut app, "dw");
    assert_eq!(
        app.buffer.text(),
        "two three\n",
        "dw removed the first word"
    );
    // `.` repeats `dw` at the cursor: deletes "two ".
    dot(&mut app);
    assert_eq!(app.buffer.text(), "three\n", ". repeated the word delete");
}

#[test]
fn dot_repeats_change_inside_quotes() {
    let (mut app, _dir) = dot_app("dot-ciq", "say \"foo\" and \"bar\" now\n");
    // Move onto the first quoted span, then `ci"` + type replacement + Esc.
    app.buffer.cursor = app.buffer.cursor_at_byte(5); // inside the first "foo"
    type_str(&mut app, "ci\"");
    assert!(matches!(app.mode, Mode::Insert), "ci\" enters Insert");
    type_str(&mut app, "X");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        app.buffer.text(),
        "say \"X\" and \"bar\" now\n",
        "ci\" replaced the first quoted span"
    );
    // Move into the second quoted span and repeat the whole change.
    let at = app.buffer.text().find("bar").unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(at);
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "say \"X\" and \"X\" now\n",
        ". repeated the change-inside-quotes"
    );
    assert!(matches!(app.mode, Mode::Normal), ". lands back in Normal");
}

#[test]
fn dot_repeats_insert_then_escape() {
    let (mut app, _dir) = dot_app("dot-insert", "abc\n");
    // `i` + type + Esc is a recordable change: insert "-> " before the cursor.
    type_str(&mut app, "i");
    type_str(&mut app, "X");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.buffer.text(), "Xabc\n", "insert-then-escape typed X");
    let ones = app.buffer.text().matches('X').count();
    assert_eq!(ones, 1, "one X so far");
    // `.` replays the whole insert session, inserting a second X at the cursor.
    dot(&mut app);
    assert_eq!(
        app.buffer.text().matches('X').count(),
        2,
        ". replayed the insert-then-escape change: {}",
        app.buffer.text()
    );
    assert!(matches!(app.mode, Mode::Normal), ". lands back in Normal");
}

#[test]
fn a_pure_motion_is_not_repeated() {
    let (mut app, _dir) = dot_app("dot-motion", "one two three\n");
    // A real change first, so there IS a last change to (not) be clobbered.
    type_str(&mut app, "dw"); // "two three\n"
    assert_eq!(app.buffer.text(), "two three\n");
    // Now a pure motion: `w` moves the cursor (onto "three") and edits nothing.
    let before = app.buffer.text();
    type_str(&mut app, "w");
    assert_eq!(app.buffer.text(), before, "w only moved the cursor");
    // `.` must still repeat the *delete* (at the moved cursor), not the motion.
    // A recorded motion would leave the text unchanged; a recorded delete removes
    // the word under the cursor.
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "two ",
        ". repeated the delete at the cursor, not the intervening motion"
    );
}

#[test]
fn dot_with_no_prior_change_is_a_no_op() {
    let (mut app, _dir) = dot_app("dot-none", "unchanged\n");
    let before = app.buffer.text();
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        before,
        ". with nothing recorded does nothing"
    );
}

#[test]
fn repeated_edit_is_one_undo_step() {
    let (mut app, _dir) = dot_app("dot-undo", "one two three\n");
    type_str(&mut app, "dw"); // "two three\n"
    app.buffer.seal_undo_group(); // as an idle tick would, closing the original edit
    dot(&mut app); // repeat -> "three\n"
    assert_eq!(app.buffer.text(), "three\n", "repeat applied");
    // One undo reverses the whole repeated change in a single step.
    app.on_key(key(KeyCode::Char('u')));
    assert_eq!(
        app.buffer.text(),
        "two three\n",
        "a single undo reverses the repeat"
    );
}

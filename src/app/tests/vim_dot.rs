//! Tests for Vim `.` (repeat last change) (#1004): a change command replays at
//! the cursor as a single undo step, while pure motions / mode switches never
//! become the repeat target.

use super::*;

/// A Vim app in Normal mode over `text`, cursor at byte 0 — a controlled buffer
/// for the repeat tests (the shared fixture's "first chapter\n" has no repeated
/// word to exercise `.` against).
fn dot_app(name: &str, text: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.flags.vim = true;
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
fn dot_repeats_a_counted_delete_char() {
    // #1142: `3x` deletes three chars; `.` must repeat with the count, deleting
    // three more — not one (the bug dropped the count prefix from the recording).
    let (mut app, _dir) = dot_app("dot-count-x", "abcdefgh\n");
    type_str(&mut app, "3x");
    assert_eq!(app.buffer.text(), "defgh\n", "3x deleted three chars");
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "gh\n",
        ". repeated the counted delete (three)"
    );
}

#[test]
fn dot_repeats_a_counted_operator_motion() {
    // #1142: `2dw` deletes two words; `.` must repeat with the count.
    let (mut app, _dir) = dot_app("dot-count-dw", "one two three four five\n");
    type_str(&mut app, "2dw");
    assert_eq!(
        app.buffer.text(),
        "three four five\n",
        "2dw deleted two words"
    );
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "five\n",
        ". repeated the two-word delete"
    );
}

#[test]
fn dot_repeats_a_counted_replace() {
    // #1142: `3rX` replaces three chars; `.` must repeat with the count.
    let (mut app, _dir) = dot_app("dot-count-r", "aaaaaaaa\n");
    type_str(&mut app, "3rX");
    assert_eq!(app.buffer.text(), "XXXaaaaa\n", "3rX replaced three chars");
    // Move onto the first untouched 'a' and repeat the counted replace.
    app.buffer.cursor = app.buffer.cursor_at_byte(3);
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "XXXXXXaa\n",
        ". repeated the three-char replace"
    );
}

#[test]
fn dot_of_an_uncounted_change_is_unchanged() {
    // #1142 regression: an uncounted change still repeats exactly once, so the
    // count-composing fix hasn't leaked a stale count into a bare `.`.
    let (mut app, _dir) = dot_app("dot-uncounted", "aaaa\n");
    type_str(&mut app, "x"); // delete one 'a'
    assert_eq!(app.buffer.text(), "aaa\n", "x deleted one char");
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "aa\n",
        ". repeated the single delete once"
    );
}

#[test]
fn outer_count_multiplies_a_counted_repeat() {
    // #1142: `{count}.` (the outer count consumed in dispatch) replays the whole
    // recorded command — count included — that many times. `2x` then `2.` deletes
    // the two-char change twice (four total), never double-applying the inner count.
    let (mut app, _dir) = dot_app("dot-outer-count", "abcdefgh\n");
    type_str(&mut app, "2x"); // -> "cdefgh\n"
    assert_eq!(app.buffer.text(), "cdefgh\n", "2x deleted two chars");
    type_str(&mut app, "2."); // repeat the 2-char delete twice -> deletes four
    assert_eq!(
        app.buffer.text(),
        "gh\n",
        "2. repeated the two-char delete twice (four chars)"
    );
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
fn dot_repeats_a_visual_delete() {
    // #1034: a change made in Visual mode (`v{motion}d`) is recorded so `.`
    // repeats it, re-entering Visual, re-selecting an equivalent span, deleting.
    let (mut app, _dir) = dot_app("dot-visual-d", "abcdefgh\n");
    // `vll` selects "abc" (inclusive of the landing char); `d` deletes it.
    type_str(&mut app, "vlld");
    assert_eq!(app.buffer.text(), "defgh\n", "visual delete removed abc");
    assert!(matches!(app.mode, Mode::Normal), "delete left Visual");
    // `.` repeats the whole visual delete at the cursor: selects "def", deletes.
    dot(&mut app);
    assert_eq!(app.buffer.text(), "gh\n", ". repeated the visual delete");
    assert!(matches!(app.mode, Mode::Normal), ". lands back in Normal");
}

#[test]
fn dot_repeats_a_visual_case_toggle() {
    // #1034: `~` over a Visual selection case-maps the span; `.` repeats it at the
    // cursor. Visual case leaves the cursor at the selection's start (Vim's rule),
    // so `.` re-selects a same-width span from wherever the cursor then sits — here
    // moved onto "def" first.
    let (mut app, _dir) = dot_app("dot-visual-tilde", "abcdefgh\n");
    type_str(&mut app, "vll~"); // toggle-case "abc" -> "ABC"
    assert_eq!(app.buffer.text(), "ABCdefgh\n", "visual ~ upcased abc");
    assert!(matches!(app.mode, Mode::Normal), "~ left Visual");
    app.buffer.cursor = app.buffer.cursor_at_byte(3); // onto "def"
    dot(&mut app); // repeat the 3-char toggle over "def"
    assert_eq!(
        app.buffer.text(),
        "ABCDEFgh\n",
        ". repeated the visual toggle"
    );
}

#[test]
fn dot_repeats_a_visual_lowercase() {
    // #1034: `gu` (via `u`) over a Visual selection lowercases it; `.` repeats.
    let (mut app, _dir) = dot_app("dot-visual-gu", "ABCDEFGH\n");
    type_str(&mut app, "vllu"); // lowercase "ABC" -> "abc"
    assert_eq!(app.buffer.text(), "abcDEFGH\n", "visual u lowercased ABC");
    app.buffer.cursor = app.buffer.cursor_at_byte(3); // onto "DEF"
    dot(&mut app); // repeat the 3-char lowercase over "DEF"
    assert_eq!(
        app.buffer.text(),
        "abcdefGH\n",
        ". repeated the visual lowercase"
    );
}

#[test]
fn repeated_visual_change_is_one_undo_step() {
    // #1034 acceptance: the repeated visual change is a single undo step.
    let (mut app, _dir) = dot_app("dot-visual-undo", "abcdefgh\n");
    type_str(&mut app, "vlld"); // delete "abc" -> "defgh\n"
    app.buffer.seal_undo_group(); // as an idle tick would, closing the original
    dot(&mut app); // repeat -> "gh\n"
    assert_eq!(app.buffer.text(), "gh\n", "repeat applied");
    app.on_key(key(KeyCode::Char('u')));
    assert_eq!(
        app.buffer.text(),
        "defgh\n",
        "a single undo reverses the whole repeated visual delete"
    );
}

#[test]
fn a_normal_change_after_a_visual_one_still_repeats() {
    // #1034 acceptance: no cross-contamination — a Normal-mode change made after a
    // visual one becomes the new `.` target, and repeats correctly.
    let (mut app, _dir) = dot_app("dot-visual-then-normal", "one two three four\n");
    type_str(&mut app, "vld"); // visual-delete "on" -> "e two three four\n"
    assert_eq!(app.buffer.text(), "e two three four\n");
    // A Normal-mode `dw` now: it must become the last change, not the visual one.
    type_str(&mut app, "dw"); // delete "e " -> "two three four\n"
    assert_eq!(app.buffer.text(), "two three four\n");
    // `.` repeats the *Normal* `dw` (deletes "two "), not the earlier visual delete.
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "three four\n",
        ". repeated the later Normal change, not the visual one"
    );
}

#[test]
fn a_cancelled_visual_selection_does_not_become_the_repeat() {
    // #1034: entering Visual and pressing Esc edits nothing, so it must not clobber
    // the previous `.` target (mirrors a bare Normal-mode motion).
    let (mut app, _dir) = dot_app("dot-visual-esc", "one two three\n");
    type_str(&mut app, "dw"); // real prior change -> "two three\n"
    assert_eq!(app.buffer.text(), "two three\n");
    // Enter Visual, extend, then cancel with Esc — a pure selection, no edit.
    type_str(&mut app, "vll");
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Normal), "Esc cancelled Visual");
    assert_eq!(
        app.buffer.text(),
        "two three\n",
        "no edit from the selection"
    );
    // Put the cursor back at the start so the check isolates *which* change `.`
    // repeats (the cancelled selection left the cursor at its end).
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    // `.` still repeats the `dw` delete, not the cancelled selection.
    dot(&mut app);
    assert_eq!(
        app.buffer.text(),
        "three\n",
        ". repeated the delete, not the cancelled visual selection"
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

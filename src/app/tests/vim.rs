//! Tests for the opt-in Vim modal-editing foundation (#201): Normal / Insert
//! modes, entry/exit keys, basic motion, and that the modeless default and the
//! shared `Ctrl-` shortcuts are unaffected.

use super::*;

/// A two-chapter app with Vim mode forced on and resting in Normal — independent
/// of whatever is in the developer's real config file.
fn vim_app(name: &str) -> (App, PathBuf) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    (app, dir)
}

#[test]
fn normal_mode_keys_do_not_insert_text() {
    let (mut app, dir) = vim_app("vim-normal-no-insert");
    let original = app.buffer.text();
    // An unmapped letter is a no-op, never typed input.
    app.on_key(key(KeyCode::Char('z')));
    assert_eq!(app.buffer.text(), original, "letters are commands, not input");
    assert!(matches!(app.mode, Mode::Normal));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn i_enters_insert_then_esc_returns_to_normal() {
    let (mut app, dir) = vim_app("vim-i-esc");
    app.on_key(key(KeyCode::Char('i')));
    assert!(matches!(app.mode, Mode::Insert), "i enters Insert");
    type_str(&mut app, "Hi");
    assert!(app.buffer.text().starts_with("Hifirst chapter"), "{}", app.buffer.text());
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Normal), "Esc returns to Normal");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_appends_after_the_cursor() {
    let (mut app, dir) = vim_app("vim-append");
    // Cursor starts at col 0; `a` enters Insert one grapheme to the right.
    app.on_key(key(KeyCode::Char('a')));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.buffer.cursor.col, 1, "append moves past the first grapheme");
    type_str(&mut app, "X");
    assert!(app.buffer.text().starts_with("fXirst chapter"), "{}", app.buffer.text());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn capital_a_appends_at_line_end() {
    let (mut app, dir) = vim_app("vim-A");
    app.on_key(key(KeyCode::Char('A')));
    assert!(matches!(app.mode, Mode::Insert));
    type_str(&mut app, "!");
    assert!(app.buffer.text().starts_with("first chapter!"), "{}", app.buffer.text());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn o_opens_a_line_below_and_capital_o_above() {
    let (mut app, dir) = vim_app("vim-open");
    app.on_key(key(KeyCode::Char('o')));
    assert!(matches!(app.mode, Mode::Insert), "o enters Insert");
    type_str(&mut app, "new");
    assert_eq!(app.buffer.text(), "first chapter\nnew\n", "o opens below");

    // Back to Normal, then O opens above the current (now second) line.
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('O')));
    type_str(&mut app, "top");
    assert_eq!(app.buffer.text(), "first chapter\ntop\nnew\n", "{}", app.buffer.text());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn hjkl_moves_the_cursor_in_normal_mode() {
    let (mut app, dir) = vim_app("vim-motion");
    assert_eq!(app.buffer.cursor.col, 0);
    app.on_key(key(KeyCode::Char('l')));
    assert_eq!(app.buffer.cursor.col, 1, "l moves right");
    app.on_key(key(KeyCode::Char('h')));
    assert_eq!(app.buffer.cursor.col, 0, "h moves left");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_shortcuts_still_work_in_normal_mode() {
    let (mut app, dir) = vim_app("vim-ctrl");
    // Ctrl-g focuses the chapter browser, the same as in Insert mode.
    app.on_key(ctrl('g'));
    assert!(matches!(app.mode, Mode::Nav { .. }), "Ctrl-g reaches the browser from Normal");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn returning_from_the_browser_lands_in_normal_when_vim_is_on() {
    let (mut app, dir) = vim_app("vim-return");
    app.on_key(ctrl('g')); // into the browser
    app.on_key(key(KeyCode::Enter)); // open the selected chapter
    assert!(matches!(app.mode, Mode::Normal), "Vim returns to Normal, not Insert");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn word_motion_and_count_move_the_cursor() {
    // Active chapter is "first chapter\n".
    let (mut app, dir) = vim_app("vim-word");
    app.on_key(key(KeyCode::Char('w'))); // to "chapter"
    assert_eq!(app.buffer.cursor.col, 6, "w jumps to the next word");
    app.on_key(key(KeyCode::Char('0'))); // back to line start
    assert_eq!(app.buffer.cursor.col, 0, "0 goes to the line start");
    app.on_key(key(KeyCode::Char('$'))); // to line end
    assert_eq!(app.buffer.cursor.col, app.buffer.line_len(), "$ goes to the line end");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_count_prefix_repeats_a_motion() {
    let (mut app, dir) = vim_app("vim-count");
    // "3l" moves right three graphemes.
    type_str(&mut app, "3l");
    assert_eq!(app.buffer.cursor.col, 3, "3l moves three to the right");
    // The count is consumed — a lone "l" now moves one.
    app.on_key(key(KeyCode::Char('l')));
    assert_eq!(app.buffer.cursor.col, 4, "count resets after the motion");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn gg_and_capital_g_jump_between_lines() {
    let (mut app, dir) = vim_app("vim-gg");
    // Give the chapter a few lines via Insert, then return to Normal.
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond\nthird");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.buffer.cursor.line, 2, "cursor on the last line after typing");
    type_str(&mut app, "gg");
    assert_eq!(app.buffer.cursor.line, 0, "gg jumps to the first line");
    app.on_key(key(KeyCode::Char('G')));
    assert_eq!(app.buffer.cursor.line, 2, "G jumps to the last line");
    type_str(&mut app, "2G");
    assert_eq!(app.buffer.cursor.line, 1, "a count before G jumps to that line");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn f_finds_a_char_and_semicolon_repeats() {
    let (mut app, dir) = vim_app("vim-find");
    // "first chapter": 't' at col 4 (first) and col 10 (chapter).
    type_str(&mut app, "ft");
    assert_eq!(app.buffer.cursor.col, 4, "ft jumps to the first 't'");
    app.on_key(key(KeyCode::Char(';')));
    assert_eq!(app.buffer.cursor.col, 10, "; repeats the find");
    app.on_key(key(KeyCode::Char(',')));
    assert_eq!(app.buffer.cursor.col, 4, ", repeats the find backwards");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn dw_deletes_a_word_and_stays_in_normal() {
    let (mut app, dir) = vim_app("vim-dw");
    type_str(&mut app, "dw"); // delete "first " (word + trailing space)
    assert_eq!(app.buffer.text(), "chapter\n", "{}", app.buffer.text());
    assert!(matches!(app.mode, Mode::Normal), "d stays in Normal");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cw_changes_to_word_end_and_enters_insert() {
    let (mut app, dir) = vim_app("vim-cw");
    // cw acts like ce: changes "first" (not the trailing space), then Insert.
    type_str(&mut app, "cw");
    assert!(matches!(app.mode, Mode::Insert), "c enters Insert");
    type_str(&mut app, "last");
    assert_eq!(app.buffer.text(), "last chapter\n", "{}", app.buffer.text());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn dd_deletes_the_line_with_a_count() {
    let (mut app, dir) = vim_app("vim-dd");
    // Build three lines, return to Normal at the top.
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond\nthird");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "gg"); // to line 0
    type_str(&mut app, "2dd"); // delete the first two lines
    assert_eq!(app.buffer.text(), "third\n", "{}", app.buffer.text());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn x_deletes_the_char_under_the_cursor() {
    let (mut app, dir) = vim_app("vim-x");
    type_str(&mut app, "x");
    assert_eq!(app.buffer.text(), "irst chapter\n", "{}", app.buffer.text());
    type_str(&mut app, "2x");
    assert_eq!(app.buffer.text(), "st chapter\n", "2x deletes two: {}", app.buffer.text());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ci_quote_changes_inside_quotes() {
    let (mut app, dir) = vim_app("vim-ciq");
    // Replace the chapter with a quoted line.
    type_str(&mut app, "cc");
    type_str(&mut app, "say \"hi\" now");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "0"); // line start
    type_str(&mut app, "ci\""); // change inside the quotes
    assert!(matches!(app.mode, Mode::Insert));
    type_str(&mut app, "bye");
    assert_eq!(app.buffer.text(), "say \"bye\" now\n", "{}", app.buffer.text());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn capital_d_deletes_to_end_of_line_and_u_undoes() {
    let (mut app, dir) = vim_app("vim-D");
    type_str(&mut app, "fc"); // jump to the 'c' of "chapter"
    type_str(&mut app, "D"); // delete to end of line
    assert_eq!(app.buffer.text(), "first \n", "{}", app.buffer.text());
    app.on_idle(); // seal the edit into an undo group
    type_str(&mut app, "u"); // undo
    assert_eq!(app.buffer.text(), "first chapter\n", "u restores the line");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn modeless_default_is_unchanged_when_vim_is_off() {
    let (mut app, dir) = two_chapter_app("vim-off");
    app.vim = false;
    app.mode = Mode::Insert;
    // Letters type straight into the buffer, and Esc is inert.
    type_str(&mut app, "Z");
    assert!(app.buffer.text().starts_with("Zfirst chapter"), "{}", app.buffer.text());
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert), "Esc does nothing without Vim mode");
    fs::remove_dir_all(&dir).ok();
}

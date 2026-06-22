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
    // `x` (delete, a later child) is unhandled here — it must not type a letter.
    app.on_key(key(KeyCode::Char('x')));
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

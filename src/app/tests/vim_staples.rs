//! Tests for the day-one Vim staples (#1106): `r` (replace char, incl. `{count}r`
//! and multibyte), `%` (jump to the matching bracket), `S` (substitute line),
//! `>>` / `<<` (indent / dedent), and `.`-repeat of `r`.

use super::*;

/// A Vim app in Normal mode over `text`, cursor at byte 0.
fn vim_app(name: &str, text: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    app.buffer = Buffer::from_text(text);
    (app, dir)
}

#[test]
fn r_replaces_the_char_under_the_cursor() {
    let (mut app, _dir) = vim_app("vim-r", "cat\n");
    type_str(&mut app, "rb");
    assert_eq!(app.buffer.text(), "bat\n", "'rb' replaced the first char");
    assert_eq!(
        app.buffer.cursor.col, 0,
        "cursor stays on the replaced char"
    );
}

#[test]
fn count_r_replaces_multiple_chars() {
    let (mut app, _dir) = vim_app("vim-count-r", "aaaa\n");
    // `3rx` replaces the first three chars with 'x'.
    type_str(&mut app, "3rx");
    assert_eq!(app.buffer.text(), "xxxa\n", "'3rx' replaced three chars");
    assert_eq!(
        app.buffer.cursor.col, 2,
        "cursor lands on the last replaced char"
    );
}

#[test]
fn r_never_reaches_past_the_line_end() {
    let (mut app, _dir) = vim_app("vim-r-clamp", "ab\ncd\n");
    // `5rx` on a two-char line replaces only the two chars, not into the newline.
    type_str(&mut app, "5rx");
    assert_eq!(
        app.buffer.text(),
        "xx\ncd\n",
        "'5rx' clamped to the line's two chars"
    );
}

#[test]
fn r_on_empty_line_is_a_noop() {
    let (mut app, _dir) = vim_app("vim-r-empty", "\nx\n");
    type_str(&mut app, "rz");
    assert_eq!(
        app.buffer.text(),
        "\nx\n",
        "'r' on an empty line changes nothing"
    );
}

#[test]
fn r_is_multibyte_safe() {
    // Replacing a multibyte grapheme, and replacing with one, must land on
    // grapheme boundaries — never split a UTF-8 sequence.
    let (mut app, _dir) = vim_app("vim-r-multibyte", "café\n");
    // Move onto the 'é' (grapheme index 3) and replace it with a plain 'e'.
    app.buffer.cursor = app.buffer.cursor_at_byte("caf".len());
    type_str(&mut app, "re");
    assert_eq!(app.buffer.text(), "cafe\n", "replaced é with e");

    // Replace a plain char with a multibyte one.
    let (mut app, _dir) = vim_app("vim-r-multibyte2", "abc\n");
    type_str(&mut app, "ré");
    assert_eq!(app.buffer.text(), "ébc\n", "replaced a with é");
}

#[test]
fn dot_repeats_r() {
    let (mut app, _dir) = vim_app("vim-r-dot", "aaaa\n");
    // `rx` replaces the first 'a'; `l` moves right; `.` repeats the replace.
    type_str(&mut app, "rx");
    assert_eq!(app.buffer.text(), "xaaa\n");
    type_str(&mut app, "l");
    app.on_key(key(KeyCode::Char('.')));
    assert_eq!(app.buffer.text(), "xxaa\n", "'.' repeated the 'rx'");
}

#[test]
fn percent_jumps_to_the_matching_bracket() {
    let (mut app, _dir) = vim_app("vim-percent", "(abc)\n");
    // From the opening '(' at col 0, '%' jumps to the ')'.
    app.on_key(key(KeyCode::Char('%')));
    assert_eq!(app.buffer.cursor.col, 4, "'%' jumped to the matching ')'");
    // And back again from the closer.
    app.on_key(key(KeyCode::Char('%')));
    assert_eq!(app.buffer.cursor.col, 0, "'%' jumped back to the '('");
}

#[test]
fn percent_finds_the_first_bracket_after_the_cursor() {
    let (mut app, _dir) = vim_app("vim-percent-scan", "a (b) c\n");
    // Cursor at col 0 ('a'); '%' scans forward to '(' then jumps to ')'.
    app.on_key(key(KeyCode::Char('%')));
    assert_eq!(
        app.buffer.cursor.col, 4,
        "'%' matched the '(' at col 2 to ')'"
    );
}

#[test]
fn percent_honors_nesting() {
    let (mut app, _dir) = vim_app("vim-percent-nest", "(a(b)c)\n");
    // From the outer '(' at col 0, '%' jumps to the outer ')' at col 6, not the
    // inner one.
    app.on_key(key(KeyCode::Char('%')));
    assert_eq!(app.buffer.cursor.col, 6, "'%' respected the nested pair");
}

#[test]
fn d_percent_deletes_the_whole_pair() {
    let (mut app, _dir) = vim_app("vim-d-percent", "x(abc)y\n");
    // Onto the '(' at col 1, `d%` deletes the '(' through the ')' inclusive.
    app.buffer.cursor = app.buffer.cursor_at_byte(1);
    type_str(&mut app, "d%");
    assert_eq!(app.buffer.text(), "xy\n", "'d%' cut the whole bracket pair");
}

#[test]
fn s_substitutes_the_whole_line() {
    let (mut app, _dir) = vim_app("vim-S", "one two\nkeep me\n");
    // `S` clears the line and enters Insert; type a replacement.
    type_str(&mut app, "S");
    assert!(matches!(app.mode, Mode::Insert), "'S' enters Insert");
    type_str(&mut app, "new");
    assert_eq!(
        app.buffer.text(),
        "new\nkeep me\n",
        "'S' replaced the whole first line"
    );
}

#[test]
fn indent_and_dedent_lines() {
    let (mut app, _dir) = vim_app("vim-indent", "line one\nline two\n");
    // `>>` indents the current line by one level (two spaces).
    type_str(&mut app, ">>");
    assert_eq!(
        app.buffer.text(),
        "  line one\nline two\n",
        "'>>' indented the current line"
    );
    // `<<` dedents it back.
    type_str(&mut app, "<<");
    assert_eq!(
        app.buffer.text(),
        "line one\nline two\n",
        "'<<' removed the indent"
    );
}

#[test]
fn count_indent_spans_multiple_lines() {
    let (mut app, _dir) = vim_app("vim-indent-count", "a\nb\nc\n");
    // `2>>` indents two lines.
    type_str(&mut app, "2>>");
    assert_eq!(
        app.buffer.text(),
        "  a\n  b\nc\n",
        "'2>>' indented the first two lines"
    );
}

#[test]
fn indent_leaves_blank_lines_alone() {
    let (mut app, _dir) = vim_app("vim-indent-blank", "a\n\nb\n");
    type_str(&mut app, "3>>");
    assert_eq!(
        app.buffer.text(),
        "  a\n\n  b\n",
        "'>>' skipped the blank line"
    );
}

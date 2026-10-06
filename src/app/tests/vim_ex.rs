//! Tests for Vim's Ex command line (#207): `:w`/`:q`/`:wq`/`:q!`, `:{line}`, and
//! `:%s///` substitution. Split out of `vim.rs` to keep the suite under the
//! ~1,000-line convention.

use super::*;

fn vim_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.flags.vim = true;
    app.mode = Mode::Normal;
    (app, dir)
}

/// Make the active chapter dirty (an unsaved edit), back in Normal mode.
fn dirty_it(app: &mut App) {
    app.on_key(key(KeyCode::Char('i')));
    type_str(app, "z");
    app.on_key(key(KeyCode::Esc));
    assert!(app.buffer.dirty, "buffer is now dirty");
}

#[test]
fn ex_write_saves_the_buffer() {
    let (mut app, _dir) = vim_app("vim-ex-w");
    dirty_it(&mut app);
    type_str(&mut app, ":w");
    app.on_key(key(KeyCode::Enter));
    assert!(!app.buffer.dirty, "':w' saved the buffer");
    assert!(matches!(app.mode, Mode::Normal), "back to Normal after :w");
}

#[test]
fn ex_quit_is_clean_or_prompts_when_dirty() {
    let (mut app, _dir) = vim_app("vim-ex-q");
    // Clean buffer: ':q' quits straight away.
    type_str(&mut app, ":q");
    app.on_key(key(KeyCode::Enter));
    assert!(app.should_quit, "':q' quits a clean buffer");

    // Dirty buffer: ':q' prompts instead of quitting.
    let (mut app, _dir2) = vim_app("vim-ex-q-dirty");
    dirty_it(&mut app);
    type_str(&mut app, ":q");
    app.on_key(key(KeyCode::Enter));
    assert!(!app.should_quit, "':q' on unsaved changes doesn't quit yet");
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "it opens the quit confirm"
    );
}

#[test]
fn ex_force_quit_discards_changes() {
    let (mut app, _dir) = vim_app("vim-ex-q-bang");
    dirty_it(&mut app);
    type_str(&mut app, ":q!");
    app.on_key(key(KeyCode::Enter));
    assert!(app.should_quit, "':q!' quits despite unsaved changes");
}

#[test]
fn ex_write_quit_saves_then_quits() {
    let (mut app, _dir) = vim_app("vim-ex-wq");
    dirty_it(&mut app);
    type_str(&mut app, ":wq");
    app.on_key(key(KeyCode::Enter));
    assert!(!app.buffer.dirty, "':wq' saved");
    assert!(app.should_quit, "':wq' quit");
}

#[test]
fn ex_line_number_jumps_to_that_line() {
    let (mut app, _dir) = vim_app("vim-ex-goto");
    // A five-line buffer so a mid-file jump has somewhere to land.
    app.buffer.replace_text("one\ntwo\nthree\nfour\nfive\n");
    app.mode = Mode::Normal;
    type_str(&mut app, ":3");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.cursor.line, 2, "':3' lands on the 3rd line");
    assert!(matches!(app.mode, Mode::Normal), "back to Normal after :3");
}

#[test]
fn ex_line_number_clamps_out_of_range() {
    let (mut app, _dir) = vim_app("vim-ex-goto-clamp");
    app.buffer.replace_text("one\ntwo\nthree\n");
    app.mode = Mode::Normal;
    // Way past the end clamps to the last content line (index 2).
    type_str(&mut app, ":999");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.cursor.line, 2,
        "an out-of-range line clamps to the last line"
    );
}

#[test]
fn ex_substitute_global_replaces_all_and_counts() {
    let (mut app, _dir) = vim_app("vim-ex-subst");
    app.buffer.replace_text("a a\naa\n");
    app.mode = Mode::Normal;
    type_str(&mut app, ":%s/a/b/g");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.text(),
        "b b\nbb\n",
        "'%s/a/b/g' replaced every 'a'"
    );
    assert_eq!(
        app.message.as_deref(),
        Some("Replaced 4 occurrences."),
        "it reports the substitution count"
    );
}

#[test]
fn ex_substitute_without_g_replaces_first_per_line() {
    let (mut app, _dir) = vim_app("vim-ex-subst-first");
    app.buffer.replace_text("a a\na a\n");
    app.mode = Mode::Normal;
    // No `g`: only the first match on each line is replaced (Vim's default).
    type_str(&mut app, ":%s/a/b/");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.text(),
        "b a\nb a\n",
        "without 'g', only the first per line is replaced"
    );
    assert_eq!(app.message.as_deref(), Some("Replaced 2 occurrences."));
}

#[test]
fn ex_substitute_tolerates_missing_trailing_delimiter() {
    let (mut app, _dir) = vim_app("vim-ex-subst-nodelim");
    app.buffer.replace_text("cat\n");
    app.mode = Mode::Normal;
    // No trailing '/': still parses pattern + replacement.
    type_str(&mut app, ":%s/cat/dog");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.text(),
        "dog\n",
        "'%s/cat/dog' (no trailing /) works"
    );
    assert_eq!(app.message.as_deref(), Some("Replaced 1 occurrence."));
}

#[test]
fn ex_substitute_reports_when_no_match() {
    let (mut app, _dir) = vim_app("vim-ex-subst-nomatch");
    app.buffer.replace_text("cat\n");
    app.mode = Mode::Normal;
    let before = app.buffer.text();
    type_str(&mut app, ":%s/zzz/qqq/g");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.text(),
        before,
        "no match leaves the buffer alone"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Pattern not found"),
        "{:?}",
        app.message
    );
}

#[test]
fn ex_substitute_current_line_only() {
    let (mut app, _dir) = vim_app("vim-ex-subst-cur");
    app.buffer.replace_text("a a\na a\n");
    app.mode = Mode::Normal;
    // Cursor on line 1 (index 0). A bare `:s` acts on the current line only.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    type_str(&mut app, ":s/a/b/g");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.text(),
        "b b\na a\n",
        "':s' substituted only the current line"
    );
    assert_eq!(app.message.as_deref(), Some("Replaced 2 occurrences."));
}

#[test]
fn ex_substitute_line_number_range() {
    let (mut app, _dir) = vim_app("vim-ex-subst-range");
    app.buffer.replace_text("a\na\na\na\n");
    app.mode = Mode::Normal;
    // `:2,3s/a/b/` substitutes only lines 2 and 3 (1-based).
    type_str(&mut app, ":2,3s/a/b/");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.text(),
        "a\nb\nb\na\n",
        "':2,3s' touched only lines 2 and 3"
    );
    assert_eq!(app.message.as_deref(), Some("Replaced 2 occurrences."));
}

#[test]
fn ex_substitute_dollar_end_range() {
    let (mut app, _dir) = vim_app("vim-ex-subst-dollar");
    app.buffer.replace_text("a\na\na\n");
    app.mode = Mode::Normal;
    // `.,$` from the current (first) line to the last — the whole buffer here.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    type_str(&mut app, ":.,$s/a/b/");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.text(),
        "b\nb\nb\n",
        "'.,$' spanned to the last line"
    );
}

#[test]
fn ex_percent_substitute_still_works() {
    // The whole-buffer `:%s` path must survive the range refactor (#826/#1106).
    let (mut app, _dir) = vim_app("vim-ex-subst-percent-still");
    app.buffer.replace_text("a a\naa\n");
    app.mode = Mode::Normal;
    type_str(&mut app, ":%s/a/b/g");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.text(), "b b\nbb\n");
    assert_eq!(app.message.as_deref(), Some("Replaced 4 occurrences."));
}

#[test]
fn ex_unknown_command_reports_and_does_nothing() {
    let (mut app, _dir) = vim_app("vim-ex-unknown");
    type_str(&mut app, ":frobnicate");
    app.on_key(key(KeyCode::Enter));
    assert!(!app.should_quit);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Not an editor command"),
        "{:?}",
        app.message
    );
}

#[test]
fn ex_command_can_be_cancelled() {
    let (mut app, _dir) = vim_app("vim-ex-esc");
    app.on_key(key(KeyCode::Char(':')));
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "':' opens the command line"
    );
    app.on_key(key(KeyCode::Esc));
    assert!(
        matches!(app.mode, Mode::Normal),
        "Esc cancels back to Normal"
    );
    assert!(!app.should_quit);
}

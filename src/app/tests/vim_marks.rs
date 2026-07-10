//! Tests for Vim's bracket navigation (#359), marks, and the jump-back list
//! (#825): `]s`/`[s` spell-flag hops, `m`/`` ` ``/`'` marks, and `` `` `` global
//! back-jumps across chapters. Split out of `vim.rs` (pure move).

use super::*;

fn vim_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    (app, dir)
}

#[test]
fn bracket_s_jumps_between_misspellings() {
    // "quikc" at byte 6, "wrnog" at byte 17; the rest are real words (#359).
    let (mut app, _dir) = vim_app("vim-spell-nav");
    app.buffer = Buffer::from_text("alpha quikc beta wrnog gamma");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 2, "{:?}", app.spell_flags);
    app.buffer.cursor = app.buffer.cursor_at_byte(0);

    app.on_key(key(KeyCode::Char(']')));
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(app.buffer.cursor_byte(), 6, "]s → next misspelling");
    app.on_key(key(KeyCode::Char(']')));
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(app.buffer.cursor_byte(), 17, "]s → next");
    app.on_key(key(KeyCode::Char('[')));
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(app.buffer.cursor_byte(), 6, "[s → previous");
}

#[test]
fn a_bracket_then_other_key_is_a_no_op() {
    let (mut app, _dir) = vim_app("vim-bracket-abort");
    app.buffer = Buffer::from_text("alpha quikc beta");
    app.on_idle();
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.on_key(key(KeyCode::Char(']')));
    app.on_key(key(KeyCode::Char('x'))); // not `s` — cancels, no jump, no edit
    assert_eq!(app.buffer.cursor_byte(), 0, "cursor unmoved");
    assert_eq!(app.buffer.text(), "alpha quikc beta", "no text change");
    assert!(matches!(app.mode, Mode::Normal));
}

// --- Marks and the jump-back list (#825) ---------------------------------

#[test]
fn set_a_mark_and_jump_back_to_it_with_backtick() {
    // Active chapter is "first chapter\n" (14 bytes incl. newline).
    let (mut app, _dir) = vim_app("vim-mark-set-jump");
    app.buffer.cursor = app.buffer.cursor_at_byte(6); // on "chapter"
    type_str(&mut app, "ma"); // set mark 'a' here
    app.buffer.cursor = app.buffer.cursor_at_byte(0); // move away
    assert_eq!(app.buffer.cursor_byte(), 0);
    type_str(&mut app, "`a"); // jump to mark 'a'
    assert_eq!(app.buffer.cursor_byte(), 6, "`a returns to the mark");
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn apostrophe_jumps_to_a_mark_too() {
    let (mut app, _dir) = vim_app("vim-mark-apostrophe");
    app.buffer.cursor = app.buffer.cursor_at_byte(6);
    type_str(&mut app, "mz");
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    type_str(&mut app, "'z"); // 'z is an alias for `z
    assert_eq!(app.buffer.cursor_byte(), 6, "'z returns to the mark");
}

#[test]
fn jumping_to_an_unset_mark_is_a_no_op_with_a_notice() {
    let (mut app, _dir) = vim_app("vim-mark-unset");
    app.buffer.cursor = app.buffer.cursor_at_byte(3);
    type_str(&mut app, "`q"); // 'q was never set
    assert_eq!(app.buffer.cursor_byte(), 3, "cursor unmoved");
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Mark not set"));
}

#[test]
fn the_outline_jump_pushes_the_jumplist_and_back_jump_returns() {
    // A one-chapter project with headings, so the focused outline can jump.
    let dir = app_scratch_dir("vim-outline-jumpback");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    // "# One\n\n" is 7 bytes; "## Later\n" heading starts at byte 7.
    fs::write(dir.join("one.md"), "# One\n\nlead\n\n## Later\n\ntail\n").unwrap();
    let mut app = open_test_app(&dir);
    app.vim = true;
    app.mode = Mode::Normal;
    app.on_idle(); // parse headings

    // Start the cursor at the top, focus the outline, select the second heading,
    // and jump to it.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.focus_outline();
    app.on_key(key(KeyCode::Down)); // to "## Later"
    app.on_key(key(KeyCode::Enter)); // jump
    let landed = app.buffer.cursor_byte();
    assert!(landed > 0, "the outline jumped forward to the heading");

    // Back-jump returns to where the author was (byte 0).
    type_str(&mut app, "``");
    assert_eq!(
        app.buffer.cursor_byte(),
        0,
        "`` returns to the pre-jump spot"
    );
    let _ = TempProject::new(dir);
}

#[test]
fn go_to_chapter_pushes_the_jumplist_and_back_jump_returns_across_chapters() {
    // Two chapters; start in chapter one at a known byte, go to chapter two, then
    // back-jump — which must re-open chapter one and restore the cursor.
    let (mut app, _dir) = vim_app("vim-goto-jumpback");
    app.buffer.cursor = app.buffer.cursor_at_byte(6); // in "first chapter"
    assert_eq!(app.project.active_index(), 0);

    // Drive the go-to-chapter prompt to chapter 2 (its jump pushes the jumplist).
    app.open_prompt(
        crate::mode::Pending::GoToChapter,
        "Go to chapter: ",
        "",
        Some(0),
    );
    type_str(&mut app, "2");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.project.active_index(), 1, "landed in chapter two");
    assert!(app.buffer.text().starts_with("second"));

    // Back-jump returns to chapter one, cursor at the recorded byte.
    type_str(&mut app, "``");
    assert_eq!(app.project.active_index(), 0, "`` re-opened chapter one");
    assert_eq!(
        app.buffer.cursor_byte(),
        6,
        "cursor restored to the pre-jump byte"
    );
}

#[test]
fn a_mark_is_global_and_jumps_back_across_chapters() {
    // Set a mark in chapter one, switch to chapter two, then jump to the mark —
    // it must re-open chapter one.
    let (mut app, _dir) = vim_app("vim-mark-global");
    app.buffer.cursor = app.buffer.cursor_at_byte(6);
    type_str(&mut app, "mb"); // mark 'b' in chapter one
    app.activate_chapter(1);
    assert_eq!(app.project.active_index(), 1);
    type_str(&mut app, "`b"); // jump to mark 'b'
    assert_eq!(
        app.project.active_index(),
        0,
        "the global mark re-opened chapter one"
    );
    assert_eq!(app.buffer.cursor_byte(), 6, "cursor at the marked byte");
}

#[test]
fn back_jump_with_an_empty_jumplist_is_a_no_op() {
    let (mut app, _dir) = vim_app("vim-jumpback-empty");
    app.buffer.cursor = app.buffer.cursor_at_byte(4);
    type_str(&mut app, "``"); // nothing pushed yet
    assert_eq!(app.buffer.cursor_byte(), 4, "cursor unmoved");
    assert!(app.message.as_deref().unwrap_or("").contains("No jump"));
}

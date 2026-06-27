//! Tests for undo/redo and find/replace over the active chapter.

use super::*;

#[test]
fn ctrl_z_undoes_and_ctrl_y_redoes_typing() {
    let (mut app, _dir) = two_chapter_app("undo");
    let original = app.buffer.text();
    type_str(&mut app, "ZZZ");
    app.on_idle(); // seal the typing burst into one undo group
    assert!(app.buffer.text().contains("ZZZ"), "the edit landed");

    app.on_key(ctrl('z'));
    assert_eq!(
        app.buffer.text(),
        original,
        "undo restores the original text"
    );
    app.on_key(ctrl('y'));
    assert!(app.buffer.text().contains("ZZZ"), "redo reapplies the edit");
}

#[test]
fn undo_is_guarded_in_the_continuous_view() {
    let (mut app, _dir) = two_chapter_app("undo-continuous");
    app.on_key(ctrl('l')); // enter the continuous-manuscript view
    assert!(app.manuscript.is_some());
    app.on_key(ctrl('z'));
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("continuous view"),
        "{:?}",
        app.message
    );
}

/// Match count and current index while in search mode.
fn search_state(app: &App) -> (usize, usize) {
    match &app.mode {
        Mode::Search(s) => (s.matches.len(), s.current),
        _ => panic!("expected to be in search mode"),
    }
}

#[test]
fn ctrl_f_finds_and_navigates_matches() {
    let (mut app, _dir) = two_chapter_app("find");
    // Active chapter is "first chapter" — "r" occurs in "first" and "chapter".
    app.on_key(ctrl('f'));
    assert!(matches!(app.mode, Mode::Search(_)), "Ctrl-f enters search");
    type_str(&mut app, "r");
    let (count, _) = search_state(&app);
    assert_eq!(count, 2, "two matches for 'r'");

    app.on_key(key(KeyCode::Enter)); // move to navigation
    let (_, before) = search_state(&app);
    app.on_key(key(KeyCode::Char('n')));
    let (_, after) = search_state(&app);
    assert_ne!(before, after, "n advances to the next match");
    // Wrap-around: from the last match, n returns to the first.
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(search_state(&app).1, before, "n wraps around");

    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert), "Esc leaves search");
}

#[test]
fn find_then_replace_all() {
    let (mut app, _dir) = two_chapter_app("replace-all");
    app.on_key(ctrl('f'));
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter)); // navigate
    app.on_key(key(KeyCode::Char('r'))); // promote to replace
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // begin stepping
    app.on_key(key(KeyCode::Char('a'))); // replace all

    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.buffer.text(), "first section\n");
    assert!(
        app.message.as_deref().unwrap_or("").contains("Replaced 1"),
        "{:?}",
        app.message
    );
}

#[test]
fn replace_current_match_only_with_y() {
    let (mut app, _dir) = two_chapter_app("replace-y");
    // Two matches for "r"; replace only the first, then bail.
    app.on_key(ctrl('f'));
    type_str(&mut app, "r");
    app.on_key(key(KeyCode::Enter)); // navigate
    app.on_key(key(KeyCode::Char('r'))); // promote to replace
    type_str(&mut app, "R");
    app.on_key(key(KeyCode::Enter)); // begin stepping at the first match
    app.on_key(key(KeyCode::Char('y'))); // replace this one, advance
    app.on_key(key(KeyCode::Esc)); // done

    assert_eq!(
        app.buffer.text(),
        "fiRst chapter\n",
        "only the first match was replaced"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Replaced 1"),
        "{:?}",
        app.message
    );
}

#[test]
fn replace_is_undoable() {
    let (mut app, _dir) = two_chapter_app("replace-undo");
    app.on_key(ctrl('f'));
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('a')));
    assert_eq!(app.buffer.text(), "first section\n");
    // The replace went through the normal edit path, so undo reverses it (#145).
    app.on_key(ctrl('z'));
    assert_eq!(
        app.buffer.text(),
        "first chapter\n",
        "undo restores the pre-replace text"
    );
}

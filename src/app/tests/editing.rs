//! Tests for undo/redo and find/replace over the active chapter.

use super::*;
use std::fmt::Write;

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
fn undo_works_in_the_continuous_view() {
    // #649: undo/redo used to be refused here; now it reverts the edit and never
    // shows the old "isn't available in the continuous view" message.
    let (mut app, _dir) = two_chapter_app("undo-continuous");
    app.on_key(ctrl('l')); // enter the continuous-manuscript view
    assert!(app.manuscript.is_some());

    let original = app.edit_buffer().line(0);
    type_str(&mut app, "ZZZ");
    app.on_idle(); // seal the typing burst into one undo group

    app.on_key(ctrl('z'));
    assert_eq!(
        app.edit_buffer().line(0),
        original,
        "undo reverted the edit"
    );
    assert!(
        !app.message
            .as_deref()
            .unwrap_or("")
            .contains("continuous view"),
        "undo was performed, not refused: {:?}",
        app.message
    );

    // With the history exhausted, a further undo reports the ordinary message.
    app.on_key(ctrl('z'));
    assert_eq!(app.message.as_deref(), Some("Nothing to undo."));
}

/// A key event carrying a modifier — for the Ctrl-←/→ / Ctrl-Backspace
/// modeless editing keys (#641).
fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

#[test]
fn delete_removes_the_char_ahead_in_insert_mode() {
    let (mut app, _dir) = two_chapter_app("modeless-delete");
    // Buffer is "first chapter"; cursor rests at the start.
    app.on_key(key(KeyCode::Delete));
    assert_eq!(
        app.buffer.text().trim_end(),
        "irst chapter",
        "Delete removes the char ahead of the cursor"
    );
}

#[test]
fn ctrl_arrows_move_by_word_in_insert_mode() {
    let (mut app, _dir) = two_chapter_app("modeless-word-motion");
    app.on_key(key_mod(KeyCode::Right, KeyModifiers::CONTROL));
    assert_eq!(
        app.buffer.cursor.col, 6,
        "Ctrl-→ moves to the start of the next word (\"chapter\")"
    );
    app.on_key(key_mod(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(
        app.buffer.cursor.col, 0,
        "Ctrl-← moves back to the start of the previous word"
    );
}

#[test]
fn ctrl_backspace_deletes_the_previous_word_in_insert_mode() {
    let (mut app, _dir) = two_chapter_app("modeless-word-delete");
    app.buffer.move_line_end(); // after "first chapter"
    app.on_key(key_mod(KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(
        app.buffer.text(),
        "first \n",
        "Ctrl-Backspace deletes the previous word, leaving the space before it"
    );
}

#[test]
fn page_keys_move_the_cursor_a_screen_in_insert_mode() {
    let dir = app_scratch_dir("modeless-paging");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    // 40 short lines, one word each, so paging moves whole logical lines.
    let body = (0..40).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    fs::write(dir.join("one.md"), body).unwrap();

    let mut app = open_test_app(&dir);
    app.viewport_height = 12; // a small screen: pages by ~9 rows
    assert_eq!(app.buffer.cursor.line, 0);

    app.on_key(key(KeyCode::PageDown));
    let after_down = app.buffer.cursor.line;
    assert!(
        after_down >= 8,
        "PageDown moves roughly a screenful down, got line {after_down}"
    );
    app.on_key(key(KeyCode::PageUp));
    assert_eq!(
        app.buffer.cursor.line, 0,
        "PageUp moves a screenful back up"
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

// --- #570: case-sensitive and whole-word toggles in the single-chapter find -

/// A one-chapter project whose only chapter holds `body`, with the same test
/// pins as the shared fixtures. For the toggle tests, which need known text
/// containing both a name and its lowercase / substring lookalikes.
fn one_chapter_app(name: &str, body: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), body).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn find_default_is_case_insensitive_substring() {
    // Both toggles off: "Will" matches Will, will, and the "will" in "willing".
    let (mut app, _dir) = one_chapter_app("find-default", "Will and will were willing.\n");
    app.on_key(ctrl('f'));
    type_str(&mut app, "Will");
    assert_eq!(
        search_state(&app).0,
        3,
        "case-insensitive substring (default)"
    );
}

#[test]
fn find_case_sensitive_toggle() {
    let (mut app, _dir) = one_chapter_app("find-case", "Will and will were willing.\n");
    app.on_key(ctrl('f'));
    type_str(&mut app, "Will");
    app.on_key(ctrl('c')); // case-sensitive
    assert_eq!(
        search_state(&app).0,
        1,
        "only the capitalized standalone 'Will'"
    );
    app.on_key(ctrl('c')); // back off — substring again
    assert_eq!(search_state(&app).0, 3);
}

#[test]
fn find_whole_word_toggle() {
    let (mut app, _dir) = one_chapter_app("find-word", "Will and will were willing.\n");
    app.on_key(ctrl('f'));
    type_str(&mut app, "will");
    app.on_key(ctrl('w')); // whole-word
    assert_eq!(
        search_state(&app).0,
        2,
        "'Will' and 'will', not the 'will' inside 'willing'"
    );
}

#[test]
fn find_case_sensitive_and_whole_word_together() {
    let (mut app, _dir) = one_chapter_app("find-both", "Will and will were willing.\n");
    app.on_key(ctrl('f'));
    type_str(&mut app, "Will");
    app.on_key(ctrl('c'));
    app.on_key(ctrl('w'));
    assert_eq!(search_state(&app).0, 1, "exact case AND standalone");
}

#[test]
fn replace_honors_whole_word_and_case() {
    // The headline rename: turn the character "Will" into "Bill" without touching
    // the verb "will" or "willing".
    let (mut app, _dir) = one_chapter_app("replace-toggles", "Will said he will be willing.\n");
    app.on_key(ctrl('f'));
    type_str(&mut app, "Will");
    app.on_key(ctrl('c')); // case-sensitive
    app.on_key(ctrl('w')); // whole-word
    app.on_key(key(KeyCode::Enter)); // navigate
    app.on_key(key(KeyCode::Char('r'))); // promote to replace
    type_str(&mut app, "Bill");
    app.on_key(key(KeyCode::Enter)); // step
    app.on_key(key(KeyCode::Char('a'))); // replace all

    assert_eq!(
        app.buffer.text(),
        "Bill said he will be willing.\n",
        "only the standalone, capitalized 'Will' was renamed"
    );
}

// --- #730: Ctrl-C quits, with the same unsaved-changes guard as Ctrl-Q -------

#[test]
fn ctrl_c_quits_when_there_are_no_unsaved_changes() {
    // A freshly-opened, unmodified chapter: Ctrl-C should quit straight away,
    // exactly as Ctrl-Q does, with no prompt in the way.
    let (mut app, _dir) = one_chapter_app("ctrl-c-clean", "settled prose\n");
    assert!(!app.edit_dirty(), "the fixture opens clean");

    app.on_key(ctrl('c'));

    assert!(
        app.should_quit,
        "Ctrl-C on a clean buffer quits immediately"
    );
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no confirm prompt when there's nothing to lose"
    );
}

#[test]
fn ctrl_c_prompts_before_quitting_with_unsaved_changes() {
    // With an unsaved edit, a reflexive Ctrl-C must not drop the work: it opens
    // the same quit-confirm prompt Ctrl-Q would, and doesn't quit yet.
    let (mut app, _dir) = one_chapter_app("ctrl-c-dirty", "settled prose\n");
    type_str(&mut app, "more");
    assert!(app.edit_dirty(), "the edit made the buffer dirty");

    app.on_key(ctrl('c'));

    assert!(
        !app.should_quit,
        "Ctrl-C with unsaved work doesn't quit outright"
    );
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::Quit)),
        "it opens the quit-confirm prompt, the same guard as Ctrl-Q"
    );
}

/// Quitting while a background task is in flight warns rather than silently
/// dropping it (#870): the `Task` threads are detached, so Ctrl-Q mid pass /
/// push-pull / download would lose the result (a Claude batch may be billing) or
/// leave a partial download. A busy task opens the quit-confirm prompt naming the
/// running work; confirming still quits.
#[test]
fn ctrl_q_warns_before_dropping_an_in_flight_task() {
    use crate::ai;
    let (mut app, _dir) = one_chapter_app("ctrl-q-busy", "settled prose\n");
    assert!(!app.edit_dirty(), "the fixture opens clean");
    // A spawned-but-unpolled task keeps `ai_task` busy — a request in flight.
    app.ai_task.spawn(|| {
        ai::Outcome::Error(ai::AiError::Network {
            status: None,
            message: "in flight".into(),
        })
    });

    app.on_key(ctrl('q'));

    assert!(
        !app.should_quit,
        "Ctrl-Q with a running task doesn't quit outright"
    );
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::Quit)),
        "it opens a quit-confirm prompt instead of dropping the task"
    );
    let label = match &app.mode {
        Mode::Prompt(s) => s.label.clone(),
        _ => String::new(),
    };
    assert!(
        label.contains("still running"),
        "the prompt names the in-flight work: {label:?}"
    );

    // Confirming quits despite the running task — the author chose to.
    app.on_key(key(KeyCode::Char('y')));
    assert!(app.should_quit, "confirming quits");
}

/// With nothing running and no unsaved edits, Ctrl-Q still quits straight away —
/// the busy-task warning (#870) doesn't add a prompt to the clean path.
#[test]
fn ctrl_q_quits_immediately_with_no_task_and_no_edits() {
    let (mut app, _dir) = one_chapter_app("ctrl-q-idle", "settled prose\n");
    assert!(!app.edit_dirty());

    app.on_key(ctrl('q'));

    assert!(app.should_quit, "a clean, idle editor quits at once");
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no prompt in the way");
}

//! Prompt line editing (#706): the shared text prompts accept caret movement
//! (arrows, Home/End), insertion/deletion at the caret, and Ctrl/Alt-Backspace
//! word-delete — not just append + Backspace-from-the-end. These drive one
//! representative text prompt (Rename) through `on_key`, which is the same code
//! path every shared text prompt runs.

use super::*;
use crate::mode::{Pending, PromptState};

/// A one-chapter project with a text prompt already open and prefilled, so a test
/// can drive the shared line editor. The Rename prompt is a plain text prompt;
/// every shared text prompt shares this key handler.
fn prompt_app(name: &str, prefill: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    let mut app = open_test_app(&dir);
    app.open_prompt(Pending::Rename { at: 0 }, "rename: ", prefill, Some(0));
    (app, TempProject::new(dir))
}

/// The open prompt's state, for asserting on its input and caret.
fn state(app: &App) -> &PromptState {
    match &app.mode {
        Mode::Prompt(s) => s,
        _ => panic!("expected a prompt to be open"),
    }
}

#[test]
fn a_fresh_prompt_puts_the_caret_at_the_end_of_the_prefill() {
    let (app, _dir) = prompt_app("prompt-caret-prefill", "Chapter One");
    let s = state(&app);
    assert_eq!(s.input, "Chapter One");
    assert_eq!(s.caret, s.input.len(), "caret starts past the prefill");
}

#[test]
fn typing_mid_string_inserts_at_the_caret() {
    let (mut app, _dir) = prompt_app("prompt-insert-mid", "ac");
    // Move left once so the caret sits between 'a' and 'c', then type 'b'.
    app.on_key(key(KeyCode::Left));
    type_str(&mut app, "b");
    let s = state(&app);
    assert_eq!(
        s.input, "abc",
        "the character landed at the caret, not the end"
    );
    assert_eq!(s.caret, 2, "caret advanced past the inserted character");
}

#[test]
fn left_and_right_move_the_caret_and_stop_at_the_ends() {
    let (mut app, _dir) = prompt_app("prompt-left-right", "ab");
    assert_eq!(state(&app).caret, 2);
    app.on_key(key(KeyCode::Left));
    assert_eq!(state(&app).caret, 1);
    app.on_key(key(KeyCode::Left));
    assert_eq!(state(&app).caret, 0);
    app.on_key(key(KeyCode::Left)); // clamps at the start
    assert_eq!(state(&app).caret, 0);
    app.on_key(key(KeyCode::Right));
    assert_eq!(state(&app).caret, 1);
    app.on_key(key(KeyCode::Right));
    assert_eq!(state(&app).caret, 2);
    app.on_key(key(KeyCode::Right)); // clamps at the end
    assert_eq!(state(&app).caret, 2);
}

#[test]
fn home_and_end_jump_to_the_ends() {
    let (mut app, _dir) = prompt_app("prompt-home-end", "hello");
    app.on_key(key(KeyCode::Home));
    assert_eq!(state(&app).caret, 0);
    app.on_key(key(KeyCode::End));
    assert_eq!(state(&app).caret, 5);
}

#[test]
fn backspace_deletes_before_the_caret_not_the_string_end() {
    let (mut app, _dir) = prompt_app("prompt-backspace-mid", "abc");
    app.on_key(key(KeyCode::Home));
    app.on_key(key(KeyCode::Right)); // caret between 'a' and 'b'
    app.on_key(key(KeyCode::Backspace));
    let s = state(&app);
    assert_eq!(s.input, "bc", "the character before the caret was removed");
    assert_eq!(s.caret, 0);
}

#[test]
fn forward_delete_removes_the_character_at_the_caret() {
    let (mut app, _dir) = prompt_app("prompt-delete-fwd", "abc");
    app.on_key(key(KeyCode::Home));
    app.on_key(key(KeyCode::Delete));
    let s = state(&app);
    assert_eq!(s.input, "bc", "the character at the caret was removed");
    assert_eq!(s.caret, 0, "caret stays put on forward delete");
}

#[test]
fn word_delete_removes_the_word_before_the_caret() {
    let (mut app, _dir) = prompt_app("prompt-word-delete", "hello world");
    // Caret at the end; Ctrl-Backspace removes "world" (and no more).
    app.on_key(ctrl_key(KeyCode::Backspace));
    assert_eq!(state(&app).input, "hello ");
    // Again removes the trailing space and "hello".
    app.on_key(ctrl_key(KeyCode::Backspace));
    assert_eq!(state(&app).input, "");
    assert_eq!(state(&app).caret, 0);
}

#[test]
fn word_delete_only_affects_text_before_the_caret() {
    let (mut app, _dir) = prompt_app("prompt-word-delete-mid", "alpha beta gamma");
    // Put the caret just after "beta" (before the space before "gamma").
    app.on_key(key(KeyCode::Home));
    for _ in 0.."alpha beta".len() {
        app.on_key(key(KeyCode::Right));
    }
    app.on_key(ctrl_key(KeyCode::Backspace));
    let s = state(&app);
    assert_eq!(s.input, "alpha  gamma", "only 'beta' was removed");
    assert_eq!(s.caret, "alpha ".len(), "caret sits where 'beta' began");
}

#[test]
fn multibyte_input_edits_on_char_boundaries() {
    // A non-ASCII prefill: caret math is byte-based, so this guards against a
    // mid-codepoint split panic on insert / delete / movement.
    let (mut app, _dir) = prompt_app("prompt-multibyte", "café");
    app.on_key(key(KeyCode::Left)); // between 'f' and 'é'
    type_str(&mut app, "x");
    assert_eq!(state(&app).input, "cafxé");
    app.on_key(key(KeyCode::End));
    app.on_key(key(KeyCode::Backspace)); // delete 'é'
    assert_eq!(state(&app).input, "cafx");
}

#[test]
fn enter_still_commits_the_edited_text() {
    let (mut app, _dir) = prompt_app("prompt-enter-commits", "Old Title");
    app.on_key(key(KeyCode::Home));
    type_str(&mut app, "New ");
    app.on_key(key(KeyCode::Enter));
    // Rename returns to the browser; the chapter title reflects the edited input.
    assert_eq!(app.project.active_title(), Some("New Old Title"));
}

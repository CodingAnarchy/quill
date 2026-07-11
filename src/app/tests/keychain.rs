//! Storing API keys in the OS keychain (#530): the always-available set-key
//! actions and their prompt, the hermetic no-op keychain in test builds (#531),
//! and stripping a plaintext key back out of the config.

use super::*;
use crate::ai;
use crate::ai::keychain::Key;
use crate::mode::{Action, Pending};

// --- Storing API keys in the OS keychain (#530) ----------------------------

/// The set-key actions are always available — they configure the key, so they
/// can't depend on a key already being set, and they don't require AI enabled.
#[test]
fn set_key_actions_are_always_available() {
    let (mut app, _dir) = two_chapter_app("action-set-key-avail");
    app.ai = ai::Settings::default(); // AI off
    assert!(app.action_status(Action::SetClaudeKey).is_none());
    assert!(app.action_status(Action::SetImageKey).is_none());
}

/// Running the set-key action opens a text prompt for the key — it never writes
/// anything until the author enters a value.
#[test]
fn set_claude_key_action_opens_a_prompt() {
    let (mut app, _dir) = two_chapter_app("action-set-key-prompt");
    app.run_action(Action::SetClaudeKey, false);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::SetApiKey(Key::Anthropic)));
            assert!(s.input.is_empty(), "no key prefilled");
        }
        _ => panic!("expected the set-key prompt to open"),
    }
}

/// A blank entry cancels — nothing is stored, no follow-up prompt.
#[test]
fn entering_a_blank_key_cancels() {
    let (mut app, _dir) = two_chapter_app("action-set-key-blank");
    app.run_action(Action::SetClaudeKey, false);
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.message.as_deref().unwrap_or("").contains("Cancelled"),
        "blank entry cancels, got: {:?}",
        app.message
    );
    assert!(!matches!(app.mode, Mode::Prompt(_)), "prompt closed");
}

/// In test builds the keychain is a no-op (#531), so entering a key reports the
/// failure and points the author at the env-var escape hatch rather than writing
/// to (or reading from) the developer's real keychain. This proves the suite is
/// hermetic against the OS store while still exercising the entry path.
#[test]
fn entering_a_key_in_tests_never_touches_the_real_keychain() {
    let (mut app, _dir) = two_chapter_app("action-set-key-hermetic");
    app.run_action(Action::SetImageKey, false);
    for c in "sk-test-123".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("FAL_KEY"),
        "points to the env-var escape hatch when the keychain can't be written, got: {msg:?}"
    );
    // The config file must not have been written with the secret either.
    if let Some(path) = crate::config::config_path() {
        if let Ok(text) = std::fs::read_to_string(path) {
            assert!(
                !text.contains("sk-test-123"),
                "secret never lands in config"
            );
        }
    }
}

/// Stripping the plaintext key removes the `api_key` line from the config section
/// and reloads the settings, so the running app stops reading the stale key.
#[test]
fn strip_plaintext_key_removes_it_from_the_config() {
    let (mut app, _dir) = two_chapter_app("action-strip-key");
    isolate("strip-key"); // point config resolution at a scratch dir for this test
    let path = crate::config::config_path().expect("config path in test");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[ai]\nenabled = true\napi_key = \"sk-plaintext\"\n").unwrap();
    app.ai = ai::Settings::load();
    assert_eq!(app.ai.api_key.as_deref(), Some("sk-plaintext"));

    app.strip_plaintext_key(Key::Anthropic);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("sk-plaintext"),
        "plaintext key stripped: {text}"
    );
    assert!(text.contains("enabled = true"), "other keys kept: {text}");
    assert!(
        app.ai.api_key.is_none(),
        "settings reloaded without the key"
    );
}

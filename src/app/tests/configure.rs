//! Describe-to-config (#303): the AI's proposed settings are validated, the bad
//! ones reported, and nothing is staged for writing until the author confirms.

use super::*;
use crate::ai::config_gen::{ConfigChange, GeneratedConfig};
use crate::mode::Pending;

fn change(key: &str, value: &str) -> ConfigChange {
    ConfigChange { key: key.to_string(), value: value.to_string() }
}

#[test]
fn a_valid_proposal_stages_changes_and_opens_a_confirm() {
    let (mut app, _dir) = two_chapter_app("cfg-valid");
    let gen = GeneratedConfig {
        changes: vec![
            change("editor.vim", "true"),
            change("editor.column_width", "64"),
            change("theme.heading", "#ff8800"),
        ],
        unmapped: vec![],
    };
    app.apply_generated_config(Ok(gen));

    // Nothing applied yet — staged behind a y/N confirm.
    assert!(matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ConfirmConfig)));
    let staged = app.pending_config.as_ref().expect("changes staged for confirm");
    assert_eq!(staged.len(), 3);
    // The editor is untouched until the author confirms.
    assert!(!app.vim);
    assert_eq!(app.focus_width, 72);
}

#[test]
fn invalid_values_are_rejected_and_the_rest_kept() {
    let (mut app, _dir) = two_chapter_app("cfg-mixed");
    let gen = GeneratedConfig {
        changes: vec![
            change("editor.vim", "maybe"),    // not a bool
            change("theme.heading", "banana"), // not a color
            change("theme.name", "dark"),      // valid
        ],
        unmapped: vec!["bigger font".into()],
    };
    app.apply_generated_config(Ok(gen));

    let staged = app.pending_config.as_ref().expect("the one valid change is staged");
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].0, "theme.name");
    // The confirm label surfaces what couldn't be mapped.
    let Mode::Prompt(s) = &app.mode else { panic!("expected a confirm prompt") };
    assert!(s.label.contains("couldn't map"), "{}", s.label);
    assert!(s.label.contains("bigger font"), "{}", s.label);
}

#[test]
fn nothing_mappable_reports_and_stages_nothing() {
    let (mut app, _dir) = two_chapter_app("cfg-none");
    let gen = GeneratedConfig {
        changes: vec![change("editor.vim", "perhaps")],
        unmapped: vec!["dark mode for my soul".into()],
    };
    app.apply_generated_config(Ok(gen));

    assert!(app.pending_config.is_none(), "nothing staged");
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm opened");
    assert!(app.message.as_deref().unwrap_or("").contains("couldn't map"), "{:?}", app.message);
}

#[test]
fn declining_the_confirm_changes_nothing() {
    use crossterm::event::KeyCode;
    let (mut app, _dir) = two_chapter_app("cfg-decline");
    app.apply_generated_config(Ok(GeneratedConfig {
        changes: vec![change("editor.column_width", "64")],
        unmapped: vec![],
    }));
    assert!(app.pending_config.is_some());
    // 'n' (anything but y) cancels.
    app.on_key(key(KeyCode::Char('n')));
    assert!(app.pending_config.is_none(), "staged changes cleared");
    assert_eq!(app.focus_width, 72, "column width untouched");
    assert!(!matches!(app.mode, Mode::Prompt(_)), "prompt closed");
}

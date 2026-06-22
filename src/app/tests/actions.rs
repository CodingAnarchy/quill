//! Tests for the unified action registry and the run-action menu (#225): the
//! registry enumerates every invokable action with a stable name, the menu lists
//! them, and `run_action` routes each through its existing code path while
//! surfacing why an unavailable action can't run.

use super::*;
use crate::ai;
use crate::mode::{Action, AiAction, ExportFormat};

/// Every registry action has a distinct stable name — workflows (#89) reference
/// these, so a collision would make one unaddressable.
#[test]
fn action_names_are_unique_and_stable() {
    let mut names: Vec<&str> = Action::ALL.iter().map(|a| a.name()).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "action names must be unique");
    // A couple of the names the ticket calls out by hand — these are the contract.
    assert!(names.contains(&"save"));
    assert!(names.contains(&"export:docx"));
    assert!(names.contains(&"report:progress"));
    assert!(names.contains(&"ai:characters"));
    assert!(names.contains(&"triage"));
}

#[test]
fn action_palette_lists_every_registry_action() {
    let (mut app, dir) = two_chapter_app("action-palette");
    app.open_action_palette();
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.items.len(), Action::ALL.len(), "all actions listed"),
        _ => panic!("expected the run-action palette to open"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_k_opens_the_action_palette() {
    let (mut app, dir) = two_chapter_app("action-ctrl-k");
    app.on_key(ctrl('k'));
    assert!(matches!(app.mode, Mode::Palette(_)), "Ctrl-k opens the run-action menu");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_save_action_writes_the_chapter() {
    let (mut app, dir) = two_chapter_app("action-save");
    app.on_key(key(KeyCode::Char('X'))); // edit chapter one
    assert!(app.buffer.dirty);
    app.run_action(Action::Save, false);
    assert!(!app.buffer.dirty, "save clears the dirty flag");
    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(saved.starts_with('X'), "edit saved, got: {saved:?}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_progress_report_action_opens_the_overlay() {
    let (mut app, dir) = two_chapter_app("action-report");
    app.run_action(Action::ProgressReport, false);
    assert!(app.overlay.is_some(), "the progress report overlay opens");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_export_action_compiles_the_manuscript() {
    let (mut app, dir) = two_chapter_app("action-export");
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    assert!(app.message.as_deref().unwrap_or("").contains("Exported to"), "{:?}", app.message);
    assert!(dir.join("export").join("book.md").exists(), "export written");
    fs::remove_dir_all(&dir).ok();
}

/// Export needs the per-chapter files; in the continuous view the window is
/// unsaved, so the action reports why instead of running.
#[test]
fn export_action_unavailable_in_the_continuous_view() {
    let (mut app, dir) = two_chapter_app("action-export-continuous");
    app.toggle_continuous();
    assert!(app.manuscript.is_some());
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    assert!(
        app.message.as_deref().unwrap_or("").contains("continuous"),
        "explains the continuous-view guard, got: {:?}",
        app.message
    );
    assert!(!dir.join("export").join("book.md").exists(), "nothing exported");
    fs::remove_dir_all(&dir).ok();
}

/// A network AI action with the backend off reports how to enable it (the same
/// guard the AI palette gates on), and sends nothing.
#[test]
fn ai_action_unavailable_when_ai_is_off() {
    let (mut app, dir) = two_chapter_app("action-ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.run_action(Action::Ai(AiAction::Characters), false);
    assert!(!app.ai_task.is_busy(), "nothing sent when AI is off");
    assert!(app.message.is_some(), "the reason is surfaced: {:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

/// Triage is local aggregation, so it stays available with the AI backend off —
/// it just reports an empty list rather than blocking on AI readiness.
#[test]
fn triage_action_available_when_ai_is_off() {
    let (mut app, dir) = two_chapter_app("action-triage-off");
    app.ai = ai::Settings::default();
    app.run_action(Action::Ai(AiAction::Triage), false);
    assert!(!app.ai_task.is_busy());
    assert!(
        app.message.as_deref().unwrap_or("").contains("No findings"),
        "triage runs locally and reports its empty state, got: {:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

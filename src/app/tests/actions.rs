//! Tests for the unified action registry and the run-action menu (#225): the
//! registry enumerates every invokable action with a stable name, the menu lists
//! them, and `run_action` routes each through its existing code path while
//! surfacing why an unavailable action can't run.

use super::*;
use crate::ai;
use crate::mode::{Action, AiAction, ExportFormat, PaletteRow};

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
    let (mut app, _dir) = two_chapter_app("action-palette");
    app.open_action_palette();
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.items.len(), Action::ALL.len(), "all actions listed"),
        _ => panic!("expected the run-action palette to open"),
    }
}

/// Each group's actions must be contiguous in `ALL` — otherwise the menu would
/// emit a second header for a re-opened group. Guards future `ALL` additions.
#[test]
fn action_groups_are_contiguous_in_the_registry() {
    let mut seen: Vec<&str> = Vec::new();
    let mut prev: Option<&str> = None;
    for a in Action::ALL {
        let g = a.group();
        if prev != Some(g) {
            assert!(
                !seen.contains(&g),
                "group '{g}' is split across the registry — keep each group's actions together"
            );
            seen.push(g);
            prev = Some(g);
        }
    }
}

#[test]
fn the_action_palette_groups_actions_under_collapsible_headers() {
    let (mut app, _dir) = two_chapter_app("palette-groups");
    app.open_action_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("expected the run-action palette");
    };
    let rows = p.rows();
    // The first visible row is a group header.
    assert!(
        matches!(
            rows[0],
            PaletteRow::Header {
                group: "Project",
                ..
            }
        ),
        "first row is the Project header"
    );
    // The AI passes gather under one heading; each group emits exactly one header.
    let headers: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            PaletteRow::Header { group, .. } => Some(*group),
            PaletteRow::Item(_) => None,
        })
        .collect();
    assert!(headers.contains(&"AI") && headers.contains(&"Export"));
    let mut unique = headers.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), headers.len(), "one header per group");
}

#[test]
fn collapsing_a_group_hides_its_items_but_keeps_the_header() {
    let (mut app, _dir) = two_chapter_app("palette-collapse");
    app.open_action_palette();
    let before = match &app.mode {
        Mode::Palette(p) => p.rows().len(),
        _ => unreachable!(),
    };
    // The Project header is selected first; ← collapses it.
    app.on_key(key(KeyCode::Left));
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(p.collapsed.contains("Project"), "group collapsed");
    assert!(p.rows().len() < before, "the group's items are hidden");
    assert!(
        matches!(
            p.rows()[p.selected],
            PaletteRow::Header {
                group: "Project",
                collapsed: true
            }
        ),
        "the header stays, still selected"
    );
}

#[test]
fn enter_on_a_header_toggles_it_rather_than_running_an_action() {
    let (mut app, _dir) = two_chapter_app("palette-toggle");
    app.open_action_palette();
    app.on_key(key(KeyCode::Enter)); // on the Project header
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Enter on a header keeps the palette open"
    );
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(p.collapsed.contains("Project"), "collapsed by Enter");
    app.on_key(key(KeyCode::Enter)); // toggles back open
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(!p.collapsed.contains("Project"), "Enter expands it again");
}

#[test]
fn a_flat_palette_has_no_group_headers() {
    let (mut app, _dir) = two_chapter_app("palette-flat");
    app.open_ai_palette(); // the AI palette is ungrouped
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(
        p.rows().iter().all(|r| matches!(r, PaletteRow::Item(_))),
        "a flat palette renders no headers"
    );
    assert_eq!(p.rows().len(), p.items.len());
}

#[test]
fn ctrl_k_opens_the_action_palette() {
    let (mut app, _dir) = two_chapter_app("action-ctrl-k");
    app.on_key(ctrl('k'));
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Ctrl-k opens the run-action menu"
    );
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
}

#[test]
fn run_progress_report_action_opens_the_overlay() {
    let (mut app, _dir) = two_chapter_app("action-report");
    app.run_action(Action::ProgressReport, false);
    assert!(app.overlay.is_some(), "the progress report overlay opens");
}

#[test]
fn run_export_action_compiles_the_manuscript() {
    let (mut app, dir) = two_chapter_app("action-export");
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    assert!(
        app.message.as_deref().unwrap_or("").contains("Exported to"),
        "{:?}",
        app.message
    );
    assert!(
        dir.join("export").join("book.md").exists(),
        "export written"
    );
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
    assert!(
        !dir.join("export").join("book.md").exists(),
        "nothing exported"
    );
}

/// A network AI action with the backend off reports how to enable it (the same
/// guard the AI palette gates on), and sends nothing.
#[test]
fn ai_action_unavailable_when_ai_is_off() {
    let (mut app, _dir) = two_chapter_app("action-ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.run_action(Action::Ai(AiAction::Characters), false);
    assert!(!app.ai_task.is_busy(), "nothing sent when AI is off");
    assert!(
        app.message.is_some(),
        "the reason is surfaced: {:?}",
        app.message
    );
}

/// Triage is local aggregation, so it stays available with the AI backend off —
/// it just reports an empty list rather than blocking on AI readiness.
#[test]
fn triage_action_available_when_ai_is_off() {
    let (mut app, _dir) = two_chapter_app("action-triage-off");
    app.ai = ai::Settings::default();
    app.run_action(Action::Ai(AiAction::Triage), false);
    assert!(!app.ai_task.is_busy());
    assert!(
        app.message.as_deref().unwrap_or("").contains("No findings"),
        "triage runs locally and reports its empty state, got: {:?}",
        app.message
    );
}

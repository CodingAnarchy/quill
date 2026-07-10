//! Tests for the version-control palette (init / commit / history / diff /
//! restore / remote), split by concern into submodules. The shared palette
//! drivers live here; each submodule pulls them in with `use super::*`.

use super::*;

mod filter;
mod history;
mod milestones;
mod pull;
mod status;
mod sync;

/// Open the Git palette (Ctrl-v), move to item `index`, and run it.
pub(super) fn git_menu(app: &mut App, index: usize) {
    app.on_key(ctrl('v'));
    for _ in 0..index {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
}

/// Open the Git palette and run the item with `label` (#833) — index-independent, so
/// adding a palette entry never renumbers the tests that pick a specific action.
pub(super) fn git_menu_label(app: &mut App, label: &str) {
    app.on_key(ctrl('v'));
    let index = palette_labels(app)
        .iter()
        .position(|l| *l == label)
        .unwrap_or_else(|| panic!("no Git-palette item labelled {label:?}"));
    for _ in 0..index {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
}

/// Run the Commit action, then confirm the pre-commit review (#704) — Enter on the
/// review overlay proceeds to the message prompt, type `message`, and submit. The
/// full author-facing commit flow, reused by the tests that just want a snapshot.
pub(super) fn commit_via_menu(app: &mut App, message: &str) {
    git_menu(app, 0); // Commit → the pre-commit review overlay
    app.on_key(key(KeyCode::Enter)); // confirm → the message prompt
    type_str(app, message);
    app.on_key(key(KeyCode::Enter));
}

pub(super) fn palette_labels(app: &App) -> Vec<&str> {
    match &app.mode {
        Mode::Palette(p) => p.items.iter().map(|it| it.label.as_str()).collect(),
        _ => panic!("expected a palette to be open"),
    }
}

pub(super) fn overlay_text(app: &App) -> String {
    // Join the spans *within* a line with no separator (a line may now carry
    // several spans — the word-level diff, #1013 — so a phrase that reads as one
    // line must reassemble as one line), then the lines with newlines.
    app.overlay_lines()
        .expect("an overlay should be open")
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The commit summaries shown in the open history view, newest first.
pub(super) fn history_summaries(app: &App) -> Vec<String> {
    match &app.mode {
        Mode::History(h) => h.commits.iter().map(|c| c.summary.clone()).collect(),
        _ => panic!("expected the history view to be open"),
    }
}

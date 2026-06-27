//! Tests for the version-control palette (init / commit / history / diff /
//! restore / remote).

use super::*;

/// Open the Git palette (Ctrl-v), move to item `index`, and run it.
fn git_menu(app: &mut App, index: usize) {
    app.on_key(ctrl('v'));
    for _ in 0..index {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
}

fn palette_labels(app: &App) -> Vec<&str> {
    match &app.mode {
        Mode::Palette(p) => p.items.iter().map(|it| it.label.as_str()).collect(),
        _ => panic!("expected a palette to be open"),
    }
}

fn overlay_text(app: &App) -> String {
    app.overlay_lines()
        .expect("an overlay should be open")
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn git_palette_initializes_version_control() {
    let (mut app, dir) = two_chapter_app("vcs-init");
    assert!(!app.under_vc, "fresh project isn't under version control");
    git_menu(&mut app, 0); // the only item before init is "Initialize"
    assert!(app.under_vc, "the project is now under version control");
    assert!(dir.join(".git").exists() && dir.join(".gitignore").exists());
    assert!(
        app.message.as_deref().unwrap_or("").contains("Initialized"),
        "{:?}",
        app.message
    );
}

#[test]
fn vc_status_reflects_uncommitted_changes() {
    let (mut app, _dir) = two_chapter_app("vc-status");
    git_menu(&mut app, 0); // Initialize → the project's files are uncommitted
    assert!(app.vc_changes > 0, "a fresh repo has uncommitted files");

    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "snapshot");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.vc_changes, 0, "clean working tree after a commit");

    // Edit a chapter and save → the tree is dirty again (refreshed on idle).
    app.on_key(key(KeyCode::Char('X')));
    app.on_key(ctrl('s'));
    app.on_idle();
    assert!(app.vc_changes > 0, "dirty after a saved edit");
}

#[test]
fn git_palette_shows_commit_history() {
    let (mut app, _dir) = two_chapter_app("vc-log");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 1); // History — before any commit
    assert!(overlay_text(&app).contains("No commits yet"));
    app.on_key(key(KeyCode::Esc)); // close the overlay

    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "first draft");
    app.on_key(key(KeyCode::Enter));

    git_menu(&mut app, 1); // History
    assert!(
        overlay_text(&app).contains("first draft"),
        "history lists the commit"
    );
}

#[test]
fn git_palette_commits_with_a_message() {
    let (mut app, _dir) = two_chapter_app("vcs-commit");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 0); // Commit
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "a commit-message prompt opens"
    );
    type_str(&mut app, "first draft");
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Committed"),
        "{:?}",
        app.message
    );
    // Everything was captured → the tree is now clean.
    assert_eq!(crate::vcs::commit(&app.project.root, "x").unwrap(), None);
}

#[test]
fn git_palette_shows_changes_diff() {
    let (mut app, _dir) = two_chapter_app("vc-diff");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "base");
    app.on_key(key(KeyCode::Enter));

    // Clean tree → a message, no overlay.
    git_menu(&mut app, 2); // Changes
    assert!(app.overlay.is_none());
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No uncommitted changes"));

    // Edit + save → the diff overlay shows the changed file.
    app.on_key(key(KeyCode::Char('Q')));
    app.on_key(ctrl('s'));
    git_menu(&mut app, 2); // Changes
    assert!(
        overlay_text(&app).contains("one.md"),
        "diff names the changed file"
    );
}

#[test]
fn git_palette_restores_a_chapter() {
    let (mut app, _dir) = two_chapter_app("vc-restore");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "base");
    app.on_key(key(KeyCode::Enter));

    // Edit + save, then restore.
    app.on_key(key(KeyCode::Char('Z')));
    app.on_key(ctrl('s'));
    assert!(app.buffer.text().contains('Z'), "the edit landed");
    git_menu(&mut app, 3); // Restore chapter
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "restore asks for confirmation"
    );
    app.on_key(key(KeyCode::Char('y')));
    assert!(
        !app.buffer.text().contains('Z'),
        "the edit was reverted: {:?}",
        app.buffer.text()
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Restored"),
        "{:?}",
        app.message
    );
}

#[test]
fn git_palette_configures_a_remote_then_offers_sync() {
    let (mut app, _dir) = two_chapter_app("vc-remote");
    git_menu(&mut app, 0); // Initialize

    // Before a remote: the menu offers "Set remote" but not Push/Pull.
    app.on_key(ctrl('v'));
    let labels = palette_labels(&app);
    assert!(labels.contains(&"Set remote"), "{labels:?}");
    assert!(
        !labels.contains(&"Push") && !labels.contains(&"Pull"),
        "{labels:?}"
    );
    app.on_key(key(KeyCode::Esc));

    // Set a remote through the palette (Commit, History, Changes, Restore, Set remote = idx 4).
    git_menu(&mut app, 4);
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "set-remote prompts for a URL"
    );
    type_str(&mut app, "https://example.com/book.git");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        crate::vcs::remote_url(&app.project.root).as_deref(),
        Some("https://example.com/book.git"),
    );

    // Now the menu offers Push and Pull, and "Change remote" instead of "Set remote".
    app.on_key(ctrl('v'));
    let labels = palette_labels(&app);
    assert!(
        labels.contains(&"Push") && labels.contains(&"Pull"),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"Change remote") && !labels.contains(&"Set remote"),
        "{labels:?}"
    );
}

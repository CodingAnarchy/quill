//! Heading/scene folding + distraction-free composition behavior tests (#975):
//! the fold keys and actions toggle fold state, folding leaves the file byte-for-
//! byte unchanged after a save, the composition-mode toggle round-trips, and a
//! search reveals a match inside a folded section.

use super::*;
use crate::buffer::Cursor;
use crate::mode::Action;

/// A one-chapter project whose body has two `##` scenes, each with a body line.
fn folding_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("one.md"),
        "## Scene one\n\nThe river ran cold.\n\n## Scene two\n\nThe city slept.\n",
    )
    .unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// The start byte of the heading whose title is `title`, in the active buffer.
fn heading_start(app: &App, title: &str) -> usize {
    app.edit_buffer()
        .syntax()
        .headings()
        .iter()
        .find(|h| h.title == title)
        .unwrap_or_else(|| panic!("heading {title:?} not parsed"))
        .start
}

#[test]
fn ctrl_space_folds_the_section_under_the_cursor() {
    let (mut app, _dir) = folding_app("fold-toggle");
    // Cursor into the first scene's body.
    let start = heading_start(&app, "Scene one");
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(start);

    assert!(app.folds.is_empty(), "nothing folded to start");
    app.on_key(ctrl(' '));
    assert!(app.folds.is_folded(start), "Ctrl-Space folds the section");
    app.on_key(ctrl(' '));
    assert!(
        !app.folds.is_folded(start),
        "a second Ctrl-Space expands it"
    );
}

#[test]
fn folding_leaves_the_file_bytes_unchanged_after_save() {
    let (mut app, dir) = folding_app("fold-bytes");
    let original = fs::read_to_string(dir.join("one.md")).unwrap();

    // Fold everything, then save — the fold is display-only, so the file must be
    // byte-for-byte the same as before (bar the save's usual normalization, which
    // the seed content already satisfies).
    app.fold_all();
    assert!(!app.folds.is_empty(), "fold-all folded the sections");
    app.run_action(Action::Save, false);

    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    assert_eq!(
        saved, original,
        "folding never wrote fold state into the file"
    );
    // And the live buffer text is unchanged too.
    assert!(
        app.edit_buffer().text().contains("The river ran cold."),
        "the folded body is still in the buffer, just not rendered"
    );
}

#[test]
fn fold_all_and_expand_all_actions_toggle_every_section() {
    let (mut app, _dir) = folding_app("fold-all");
    app.run_action(Action::FoldAll, false);
    assert!(
        app.folds.is_folded(heading_start(&app, "Scene one"))
            && app.folds.is_folded(heading_start(&app, "Scene two")),
        "fold-all folds every section"
    );
    app.run_action(Action::ExpandAll, false);
    assert!(app.folds.is_empty(), "expand-all clears every fold");
}

#[test]
fn composition_mode_toggles_and_esc_restores() {
    let (mut app, _dir) = folding_app("composition");
    assert!(!app.distraction_free, "off by default");
    app.on_key(ctrl('\\'));
    assert!(app.distraction_free, "Ctrl-\\ enters composition mode");
    // Esc restores the chrome from an editing mode.
    app.on_key(key(KeyCode::Esc));
    assert!(!app.distraction_free, "Esc exits composition mode");

    // The run-action toggle round-trips too.
    app.run_action(Action::ToggleComposition, false);
    assert!(app.distraction_free, "the action enters composition mode");
    app.run_action(Action::ToggleComposition, false);
    assert!(!app.distraction_free, "the action exits it");
}

#[test]
fn search_reveals_a_match_inside_a_folded_section() {
    let (mut app, _dir) = folding_app("fold-search-reveal");
    // Fold the first scene, whose body holds "river".
    let one = heading_start(&app, "Scene one");
    app.folds.toggle(one);
    assert!(app.folds.is_folded(one), "the first scene is folded");
    // Put the cursor at the top so the incremental find lands in the folded section.
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };

    // The incremental find (Ctrl-F, then typing) jumps the cursor to each match as
    // the query narrows — the match for "river" is inside the folded body, which
    // the reveal must unfold so the cursor isn't hidden away.
    app.on_key(ctrl('f'));
    type_str(&mut app, "river");

    assert!(
        !app.folds.is_folded(one),
        "the search reveal unfolded the section holding the match"
    );
    let cursor_byte = app.edit_buffer().cursor_byte();
    let text = app.edit_buffer().text();
    assert!(
        text[cursor_byte..].starts_with("river"),
        "the cursor landed on the match inside the (now revealed) section"
    );
}

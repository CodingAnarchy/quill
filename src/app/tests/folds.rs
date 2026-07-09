//! Heading/scene folding + distraction-free composition behavior tests (#975):
//! the fold keys and actions toggle fold state, folding leaves the file byte-for-
//! byte unchanged after a save, the composition-mode toggle round-trips, and a
//! search reveals a match inside a folded section.

use super::*;
use crate::buffer::Cursor;
use crate::mode::{Action, Mode};

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

/// A one-chapter project with a `#` title over two `##` scenes — so folding the
/// title hides the scene *heading* lines, letting a jump target one that's hidden.
fn titled_folding_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("one.md"),
        "# Title\n\n## Scene one\n\nThe river ran cold.\n\n## Scene two\n\nThe city slept.\n",
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

/// Whether the fold view hides the cursor's current line — the caret-hiding bug the
/// reveal guards against (#975 bug E).
fn cursor_is_hidden(app: &App) -> bool {
    let line = app.edit_buffer().cursor.line;
    app.fold_view().is_hidden(line)
}

#[test]
fn arrowing_down_from_a_folded_heading_reveals_the_fold() {
    // Bug E(ii): pressing Down from a folded heading moves the cursor into the
    // hidden body; the motion reveal must unfold it so the caret stays on a visible
    // line rather than a zero-row hidden one.
    let (mut app, _dir) = folding_app("fold-arrow-down");
    let one = heading_start(&app, "Scene one");
    app.folds.toggle(one);
    // Park the cursor on the folded heading line itself (visible).
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(one);
    assert!(!cursor_is_hidden(&app), "the heading line starts visible");

    // Down moves into the section body, which is hidden by the fold.
    app.on_key(key(KeyCode::Down));

    assert!(
        !app.folds.is_folded(one),
        "arrowing into the folded body reveals the section"
    );
    assert!(
        !cursor_is_hidden(&app),
        "the cursor is left on a visible line, not a zero-row hidden one"
    );
}

#[test]
fn quick_switch_jump_into_a_folded_heading_reveals_it() {
    // Bug E(i): a same-chapter quick-switch jump lands on a scene heading that a
    // higher-level fold has hidden. The jump must reveal the enclosing fold so the
    // landing line is visible.
    let (mut app, _dir) = titled_folding_app("fold-qs-jump");
    let title = heading_start(&app, "Title");
    app.folds.toggle(title); // hides both `##` scenes' heading lines
    let scene = heading_start(&app, "Scene two");
    assert!(
        app.fold_view()
            .is_hidden(app.edit_buffer().cursor_at_byte(scene).line),
        "the scene heading line is hidden under the title fold"
    );

    // Open the switcher and jump to "Scene two" (same chapter).
    app.on_key(ctrl('d'));
    type_str(&mut app, "scene two");
    app.on_key(key(KeyCode::Enter));

    assert!(matches!(app.mode, Mode::Insert), "the switcher closed");
    assert_eq!(
        app.edit_buffer().cursor_byte(),
        scene,
        "landed on the scene"
    );
    assert!(
        !cursor_is_hidden(&app),
        "the jump revealed the fold so the cursor is on a visible line"
    );
}

#[test]
fn project_search_jump_into_a_folded_section_reveals_it() {
    // Bug E(i): a same-chapter manuscript-find jump lands on a match inside a folded
    // body; the jump must reveal the fold so the caret isn't hidden.
    let (mut app, _dir) = folding_app("fold-projsearch-jump");
    let one = heading_start(&app, "Scene one");
    app.folds.toggle(one); // hides "The river ran cold."

    // Manuscript-wide find for "river", then jump to the (single) match.
    app.open_project_find();
    type_str(&mut app, "river");
    app.on_key(key(KeyCode::Enter)); // run search → Results
    app.on_key(key(KeyCode::Enter)); // jump to the selected match

    assert!(matches!(app.mode, Mode::Insert), "the overlay closed");
    let cursor_byte = app.edit_buffer().cursor_byte();
    assert!(
        app.edit_buffer().text()[cursor_byte..].starts_with("river"),
        "the cursor landed on the match"
    );
    assert!(
        !app.folds.is_folded(one) && !cursor_is_hidden(&app),
        "the jump revealed the fold so the caret is visible"
    );
}

#[test]
fn mark_restore_into_a_folded_section_reveals_it() {
    // Bug E(i): returning to a mark set inside a section that was later folded must
    // reveal the fold so the restored caret is on a visible line.
    let (mut app, _dir) = folding_app("fold-mark-restore");
    let one = heading_start(&app, "Scene one");
    // Set mark `a` inside the first scene's body.
    let body = app.edit_buffer().text().find("river").unwrap();
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(body);
    app.set_mark('a');
    // Move away, then fold the scene the mark lives in.
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    app.folds.toggle(one);
    assert!(app.folds.is_folded(one));

    // Jump back to the mark — it's inside the folded body.
    app.jump_to_mark('a');

    assert_eq!(
        app.edit_buffer().cursor_byte(),
        body,
        "restored to the mark"
    );
    assert!(
        !app.folds.is_folded(one) && !cursor_is_hidden(&app),
        "the restore revealed the fold so the caret is visible"
    );
}

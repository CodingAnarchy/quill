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

/// A one-chapter project with a long body so the galley pages more than once — many
/// short paragraphs, each its own line, so pagination has plenty of rows to walk.
fn galley_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    let mut body = String::new();
    for i in 1..=60 {
        let _ = writeln!(body, "Paragraph number {i} of the proofing pass.\n");
    }
    fs::write(dir.join("one.md"), body).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn galley_mode_toggles_on_and_off_and_is_read_only() {
    let (mut app, _dir) = galley_app("galley-toggle");
    assert!(!app.in_galley(), "off by default");

    app.toggle_galley();
    assert!(app.in_galley(), "the toggle enters proofreading mode");

    // Read-only: an ordinary character key does nothing to the buffer while proofing.
    let before = app.edit_buffer().text();
    app.on_key(key(KeyCode::Char('Z')));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(
        app.edit_buffer().text(),
        before,
        "no key edits the manuscript while proofing"
    );
    assert!(
        app.in_galley(),
        "and those keys don't leave the mode either"
    );

    // Esc exits back to editing.
    app.on_key(key(KeyCode::Esc));
    assert!(!app.in_galley(), "Esc leaves proofreading mode");

    // The run-action toggle round-trips too.
    app.run_action(Action::ToggleGalley, false);
    assert!(app.in_galley(), "the action enters proofreading mode");
    app.run_action(Action::ToggleGalley, false);
    assert!(!app.in_galley(), "the action exits it");
}

#[test]
fn galley_paging_moves_the_page_top_forward_and_back() {
    let (mut app, _dir) = galley_app("galley-paging");
    app.toggle_galley();
    // Give the paging its geometry (a frame would normally record this).
    app.galley_geometry.set((72, 20));
    let start = app.galley.expect("in galley").top_line;

    // Space pages forward — the page top advances into the chapter.
    app.on_key(key(KeyCode::Char(' ')));
    let after_fwd = app.galley.expect("in galley").top_line;
    assert!(after_fwd > start, "Space pages the galley forward");

    // Backspace pages back to where it started (one screenful each way).
    app.on_key(key(KeyCode::Backspace));
    let after_back = app.galley.expect("in galley").top_line;
    assert!(
        after_back <= start,
        "Backspace pages the galley back toward the top"
    );

    // Back-paging clamps at the very first line rather than underflowing.
    for _ in 0..20 {
        app.on_key(key(KeyCode::Backspace));
    }
    assert_eq!(
        app.galley.expect("in galley").top_line,
        0,
        "back-paging clamps at the start of the chapter"
    );
}

#[test]
fn galley_line_numbers_toggle_on() {
    let (mut app, _dir) = galley_app("galley-line-numbers");
    app.toggle_galley();
    assert!(
        !app.galley.expect("in galley").line_numbers,
        "line numbers off by default"
    );
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        app.galley.expect("in galley").line_numbers,
        "`n` turns line numbers on"
    );
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        !app.galley.expect("in galley").line_numbers,
        "`n` toggles them back off"
    );
}

#[test]
fn entering_and_leaving_galley_preserves_the_edit_cursor_and_scroll() {
    let (mut app, _dir) = galley_app("galley-preserve");
    // Park the edit cursor deep in the chapter and set a read-ahead scroll offset, as
    // the edit view would have while working.
    let body = app.edit_buffer().text();
    let at = body
        .find("Paragraph number 40")
        .expect("the marker paragraph");
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(at);
    let cursor_before = app.edit_buffer().cursor;
    app.scroll_offset.set(3);

    // Enter, page around, toggle line numbers, then leave.
    app.toggle_galley();
    app.galley_geometry.set((72, 20));
    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Char('n')));
    app.on_key(key(KeyCode::Esc));

    assert!(!app.in_galley(), "left the mode");
    assert_eq!(
        app.edit_buffer().cursor,
        cursor_before,
        "the edit cursor is exactly where it was before proofing"
    );
    assert_eq!(
        app.scroll_offset.get(),
        3,
        "the edit view's scroll offset is undisturbed by a proofing pass"
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

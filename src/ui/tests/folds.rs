//! Heading/scene folding + distraction-free composition render tests (#975):
//! a section collapses to its header and expands; fold-all / expand-all; the
//! buffer is left byte-for-byte unchanged; the cursor/click mapping stays correct
//! across a fold; composition mode hides the chrome and restores it. Modeled on
//! the scene-break display-only tests.

use super::*;
use crate::buffer::Cursor;

/// A three-section chapter: a title and two `##` scenes, each with a body line.
const CHAPTER: &str =
    "# Title\n\n## Scene one\n\nThe river ran cold.\n\n## Scene two\n\nThe city slept.";

#[test]
fn a_section_folds_to_its_header_and_expands() {
    let mut app = app_with(CHAPTER);
    let before = app.edit_buffer().text();
    // Nothing folded: the first scene's body is on screen.
    let open = render(&app, 60, 20);
    assert!(open.contains("The river ran cold."), "body shows unfolded");

    // Fold the first scene (its heading starts at the "## Scene one" byte).
    let start = app
        .edit_buffer()
        .syntax()
        .headings()
        .iter()
        .find(|h| h.title == "Scene one")
        .expect("the scene heading is parsed")
        .start;
    app.folds.toggle(start);

    let folded = render(&app, 60, 20);
    assert!(
        !folded.contains("The river ran cold."),
        "the folded section's body is hidden"
    );
    assert!(folded.contains("Scene one"), "the header stays visible");
    assert!(
        folded.contains('▸'),
        "the folded header carries a fold marker"
    );
    // A same-level scene after the fold is *not* hidden — the fold stops at it.
    assert!(
        folded.contains("The city slept."),
        "the next same-level section is unaffected"
    );
    assert_eq!(
        app.edit_buffer().text(),
        before,
        "folding never touches the buffer"
    );

    // Expand: the body returns and the marker is gone.
    app.folds.toggle(start);
    let reopened = render(&app, 60, 20);
    assert!(
        reopened.contains("The river ran cold."),
        "expanding restores the body"
    );
    assert!(!reopened.contains('▸'), "no fold marker once expanded");
}

#[test]
fn fold_all_collapses_every_section_and_expand_all_restores() {
    // Two sibling `##` scenes (no wrapping `#`), so fold-all leaves both headers.
    let scenes = "## Scene one\n\nThe river ran cold.\n\n## Scene two\n\nThe city slept.";
    let mut app = app_with(scenes);
    let headings = app.edit_buffer().syntax().headings().to_vec();
    app.folds.fold_all(&headings);
    // Park the cursor on the first (visible) header so it isn't on a hidden line.
    let first = headings[0].start;
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(first);

    let folded = render(&app, 60, 20);
    assert!(
        !folded.contains("The river ran cold."),
        "the first section's body is folded away"
    );
    assert!(
        !folded.contains("The city slept."),
        "the second section's body is folded away"
    );
    assert!(
        folded.contains("Scene one") && folded.contains("Scene two"),
        "both sibling headers remain"
    );

    app.folds.clear();
    let expanded = render(&app, 60, 20);
    assert!(
        expanded.contains("The river ran cold.") && expanded.contains("The city slept."),
        "expand-all brings every body back"
    );
}

#[test]
fn a_higher_level_fold_hides_its_nested_subsections() {
    // A `#` title folds everything under it, including the `##` scenes below.
    let mut app = app_with(CHAPTER);
    let title = app
        .edit_buffer()
        .syntax()
        .headings()
        .iter()
        .find(|h| h.title == "Title")
        .expect("title heading")
        .start;
    app.folds.toggle(title);
    let folded = render(&app, 60, 20);
    assert!(folded.contains("Title"), "the title itself stays visible");
    assert!(
        !folded.contains("Scene one") && !folded.contains("Scene two"),
        "the nested subsections fold away under the higher-level heading"
    );
}

#[test]
fn the_cursor_maps_correctly_across_a_fold() {
    // With the first scene folded, the cursor placed in the *second*, still-visible
    // scene must land on-screen — the fold's zero-row body shifts nothing off.
    let mut app = app_with(CHAPTER);
    let start = app
        .edit_buffer()
        .syntax()
        .headings()
        .iter()
        .find(|h| h.title == "Scene one")
        .unwrap()
        .start;
    app.folds.toggle(start);
    // Put the cursor on the "The city slept." line (the last logical line).
    let last = app.edit_buffer().line_count() - 1;
    app.edit_buffer_mut().cursor = Cursor { line: last, col: 0 };

    // The click-inversion layout records only drawn rows; a click on the row that
    // shows "The city slept." must invert back to that logical line, proving the
    // folded (zero-row) lines above didn't desync the screen→byte mapping.
    let screen = render(&app, 60, 20);
    let row = screen
        .lines()
        .position(|l| l.contains("The city slept."))
        .expect("the second scene body is on screen") as u16;
    let layout = app.editor_layout.borrow();
    let vrow = layout
        .rows
        .get(row as usize - layout.column.y as usize)
        .expect("a recorded row for the visible body line");
    assert_eq!(
        vrow.line, last,
        "the visible body row maps to its own logical line across the fold"
    );
}

#[test]
fn composition_mode_hides_the_chrome_and_restores_it() {
    let mut app = app_with_chapters(&["One", "Two"], CHAPTER);
    app.outline_visible = true;
    // With the outline pane on, its chapter list shows in the normal layout.
    let normal = render(&app, 80, 20);
    assert!(
        normal.contains("Two"),
        "the outline pane lists chapters normally"
    );

    app.distraction_free = true;
    let composed = render(&app, 80, 20);
    assert!(
        !composed.contains("Two"),
        "composition mode hides the outline pane chrome"
    );
    // The prose itself is still there — only the chrome is gone.
    assert!(
        composed.contains("The river ran cold."),
        "the prose still shows"
    );

    app.distraction_free = false;
    let restored = render(&app, 80, 20);
    assert!(
        restored.contains("Two"),
        "toggling composition mode off restores the layout"
    );
}

#[test]
fn composition_mode_composes_with_folding() {
    // Folds and distraction-free stack: chrome hidden *and* a section collapsed.
    let mut app = app_with(CHAPTER);
    let start = app
        .edit_buffer()
        .syntax()
        .headings()
        .iter()
        .find(|h| h.title == "Scene one")
        .unwrap()
        .start;
    app.folds.toggle(start);
    app.distraction_free = true;
    let screen = render(&app, 60, 20);
    assert!(
        !screen.contains("The river ran cold."),
        "the fold still hides its body"
    );
    assert!(screen.contains("Scene two"), "unfolded sections still show");
}

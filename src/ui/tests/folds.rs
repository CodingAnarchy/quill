//! Heading/scene folding + distraction-free composition render tests (#975):
//! a section collapses to its header and expands; fold-all / expand-all; the
//! buffer is left byte-for-byte unchanged; the cursor/click mapping stays correct
//! across a fold; composition mode hides the chrome and restores it. Modeled on
//! the scene-break display-only tests.

use std::fmt::Write;

use super::*;
use crate::buffer::Cursor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};

/// A plain (no-modifier) key event — the galley paging tests press Space, `n`, etc.
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

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
fn revealing_a_fold_at_the_cursor_places_the_terminal_caret() {
    // Bug E: a jump lands the cursor inside a collapsed section. Before the reveal
    // the caret's line is a zero-row hidden line, so the render never calls
    // `set_cursor_position` and the terminal caret sticks stale. After the reveal the
    // fold opens and the caret is placed on its (now visible) line.
    let mut app = app_with(CHAPTER);
    // Fold the `#` title, which hides the `##` scene heading lines below it.
    let title = app
        .edit_buffer()
        .syntax()
        .headings()
        .iter()
        .find(|h| h.title == "Title")
        .unwrap()
        .start;
    app.folds.toggle(title);
    // Jump the cursor onto the (hidden) "## Scene two" heading, as a jump path would.
    let scene = app
        .edit_buffer()
        .syntax()
        .headings()
        .iter()
        .find(|h| h.title == "Scene two")
        .unwrap()
        .start;
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(scene);
    assert!(
        app.fold_view().is_hidden(app.edit_buffer().cursor.line),
        "before the reveal the caret's line is hidden by the title fold"
    );

    // The centralized reveal (invoked on every jump path) opens the enclosing fold.
    assert!(app.reveal_fold_at_cursor(), "the reveal opened the fold");
    assert!(
        !app.fold_view().is_hidden(app.edit_buffer().cursor.line),
        "the caret's line is visible after the reveal"
    );

    // Render: the terminal caret is now placed, and it lands on the heading text.
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    let buf = terminal.backend().buffer().clone();
    assert_eq!(
        buf[(pos.x, pos.y)].symbol(),
        "#",
        "the caret sits on the heading's first character (its `#`) on a visible row"
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

/// A tall chapter with a foldable `## Alpha` subsection whose middle lines are
/// filled by `middle` (given a blank line above and below so a `---` parses as a
/// thematic break, not a setext underline). A deep pre-section forces scrolling so
/// the centering walk climbs through the folded region.
fn folded_middle_doc(middle: &str) -> String {
    let mut s = String::from("# Title\n\n");
    for i in 1..=30 {
        let _ = write!(s, "Pre line {i}.\n\n");
    }
    s.push_str("## Alpha\n\na-one\n\n");
    let _ = write!(s, "{middle}\n\n");
    s.push_str("a-two\n\n");
    let _ = write!(s, "{middle}\n\n");
    s.push_str("a-three\n\n## Beta\n\nb-one\n\nb-two\n\nb-three");
    s
}

#[test]
fn a_folded_scene_break_does_not_shift_the_viewport() {
    // #12: a `---` scene break inside a collapsed section keys a display-only insert
    // on a hidden line. The fill loop skips the hidden line before ever reaching that
    // insert, so it draws nothing — but the scroll/centering walks used to still
    // count its rows (unlike the body rows, which fold to zero), mis-anchoring the
    // viewport by those phantom rows. With the `folds.is_hidden` guard on the header
    // closure, a folded section renders identically whether its hidden middle holds
    // scene breaks or plain lines.
    let fold_alpha_and_render = |middle: &str| -> String {
        let mut app = app_with(&folded_middle_doc(middle));
        let start = app
            .edit_buffer()
            .syntax()
            .headings()
            .iter()
            .find(|h| h.title == "Alpha")
            .expect("the Alpha heading is parsed")
            .start;
        app.folds.toggle(start);
        // Cursor on the last line (in Beta, just past the fold), so the centering
        // walk climbs up through the collapsed Alpha section.
        let last = app.edit_buffer().line_count() - 1;
        app.edit_buffer_mut().cursor = Cursor { line: last, col: 0 };
        // The prose region only — the last row is the status bar, whose live word
        // count legitimately differs between the `---` and `plain` documents.
        let screen = render(&app, 60, 15);
        screen
            .lines()
            .take(screen.lines().count() - 1)
            .collect::<Vec<_>>()
            .join("\n")
    };

    let with_breaks = fold_alpha_and_render("---");
    let with_plain = fold_alpha_and_render("plain");
    assert_eq!(
        with_breaks, with_plain,
        "a folded section's hidden scene breaks add no phantom rows to the scroll math"
    );
}

/// A many-paragraph chapter whose lines are individually identifiable, so a render test
/// can tell which page is on screen. Each paragraph is one numbered line.
fn galley_chapter() -> String {
    let mut body = String::new();
    for i in 1..=60 {
        let _ = writeln!(body, "Line {i} of the galley proofing pass.\n");
    }
    body
}

#[test]
fn galley_mode_paginates_and_pages_forward() {
    let mut app = app_with(&galley_chapter());
    // Proof from the top of the chapter (the cursor rests at the buffer end after the
    // test buffer is typed in; a real pass usually starts at the beginning).
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    app.toggle_galley();

    // The first page shows the opening lines but not the far tail — it's paginated,
    // not the whole scroll.
    let page1 = render(&app, 60, 12);
    assert!(
        page1.contains("Line 1 of"),
        "the first page opens the chapter"
    );
    assert!(
        !page1.contains("Line 60 of"),
        "a page is one screenful, not the whole chapter"
    );

    // Page forward: the visible window advances to later lines.
    let top1 = app.galley.expect("in galley").top_line;
    app.on_key(key(KeyCode::Char(' ')));
    let top2 = app.galley.expect("in galley").top_line;
    assert!(top2 > top1, "Space advanced the page top");
    let page2 = render(&app, 60, 12);
    assert!(
        !page2.contains("Line 1 of"),
        "the opening line has paged off screen"
    );
    // The new page shows a later stretch of the chapter — some numbered line past the
    // first screenful, not the opening lines.
    assert!(
        page2.lines().any(|l| l.contains(" of the galley")),
        "the next screenful of prose is shown"
    );
    assert!(
        !page2.contains("Line 2 of"),
        "early lines that belong to page one are no longer shown"
    );
}

#[test]
fn galley_mode_shows_no_chrome() {
    // In the galley there is no status bar and no outline pane — just the page.
    let mut app = app_with_chapters(&["One", "Two"], &galley_chapter());
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    app.outline_visible = true;
    app.toggle_galley();
    let screen = render(&app, 80, 12);
    assert!(
        !screen.contains("Two"),
        "the outline pane chrome is hidden while proofing"
    );
    assert!(
        !screen.contains("words · ln"),
        "the editor status bar is hidden while proofing"
    );
    assert!(screen.contains("Line 1 of"), "the prose page shows");
}

#[test]
fn galley_line_numbers_render_a_gutter() {
    let mut app = app_with(&galley_chapter());
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    app.toggle_galley();
    let plain = render(&app, 60, 8);
    // With numbers off, the first prose row starts at the text, not a digit gutter.
    let first = plain.lines().find(|l| l.contains("Line 1 of")).unwrap();
    assert!(
        first.trim_start().starts_with("Line 1"),
        "no gutter when line numbers are off"
    );

    app.toggle_galley_line_numbers();
    let numbered = render(&app, 60, 8);
    let row = numbered.lines().find(|l| l.contains("Line 1 of")).unwrap();
    // The logical line number (1) prefixes the row in a numeric gutter.
    assert!(
        row.trim_start().starts_with('1'),
        "the line-number gutter prefixes the prose row: {row:?}"
    );
}

#[test]
fn a_collapsed_heading_row_maps_a_click_to_its_start_byte() {
    // Bug I: the collapsed-heading fold glyph "▸ " is a display-only prefix that
    // isn't in the buffer. A per-character click→byte walk would land the cursor a
    // couple of columns too far into the heading text. The row is recorded
    // non-editable, so a click on it maps to the heading's start byte instead — no
    // glyph-width desync.
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
    render(&app, 60, 20); // populate the click layout

    let hline = app.edit_buffer().cursor_at_byte(start).line;
    let layout = app.editor_layout.borrow();
    let vrow = layout
        .rows
        .iter()
        .find(|r| r.line == hline)
        .expect("the collapsed heading has a recorded row");
    assert!(
        !vrow.editable,
        "the collapsed heading row is non-editable so a click maps to its start"
    );
    // `click_target` (the click inverse) maps a non-editable row to byte 0 of its
    // line — the heading's start — regardless of where in the row the click fell.
    assert_eq!(
        vrow.line_start, start,
        "the row's line start is the heading's start byte"
    );
}

#[test]
fn the_caret_on_a_collapsed_heading_renders_past_the_fold_glyph() {
    // Bug I: the display-only fold glyph shifts the heading text right on screen, so
    // the caret placed on a collapsed heading must shift with it — otherwise it
    // renders a couple of cells left of the text, over the glyph instead of the `#`.
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
    // Park the caret at the collapsed heading's start (col 0) — where a fold snap or
    // a same-chapter jump leaves it.
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(start);

    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    let buf = terminal.backend().buffer().clone();
    // The glyph "▸ " precedes the heading; the caret must land on the heading's
    // first character (its `#`), not on the glyph two cells to its left.
    assert_eq!(
        buf[(pos.x, pos.y)].symbol(),
        "#",
        "the caret tracks the glyph-shifted heading text, landing on its `#`"
    );
    // And the cell two columns left is the fold glyph, confirming the shift is real.
    assert_eq!(
        buf[(pos.x - 2, pos.y)].symbol(),
        "▸",
        "the fold glyph sits two cells left of the caret"
    );
}

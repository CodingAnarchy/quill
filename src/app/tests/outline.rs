//! The focused heading outline (#657): the Ctrl-O outline pane made navigable.
//! Ctrl-O cycles hidden → shown → focused; while focused, the selection moves
//! over the active chapter's headings and Enter jumps the editor cursor to the
//! selected heading's byte offset. Hermetic — each test seeds its own project
//! dir (#531).

use super::*;
use crate::mode::Mode;
use crate::project::Project;
use std::time::{Duration, Instant};

/// A one-chapter project whose body has the given headings/prose, opened and
/// pinned like the other fixtures.
fn outline_app(name: &str, body: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), body).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn ctrl_o_cycles_hidden_then_shown_then_focused() {
    let (mut app, _dir) = outline_app(
        "outline-cycle",
        "# One\n\n## Scene A\n\nText.\n\n## Scene B\n\nMore.\n",
    );
    // Starts hidden.
    assert!(!app.outline_visible);

    // First Ctrl-O reveals the pane (still editing, display-only).
    app.on_key(ctrl('o'));
    assert!(app.outline_visible);
    assert!(matches!(app.mode, Mode::Insert));

    // Second Ctrl-O focuses the outline for navigation.
    app.on_key(ctrl('o'));
    assert!(app.outline_visible);
    assert!(matches!(app.mode, Mode::Outline { .. }));

    // A third Ctrl-O hides the pane and returns to editing.
    app.on_key(ctrl('o'));
    assert!(!app.outline_visible);
    assert!(matches!(app.mode, Mode::Insert));
}

#[test]
fn selecting_a_heading_moves_the_cursor_there() {
    let body = "# One\n\n## Scene A\n\nText.\n\n## Scene B\n\nMore.\n";
    let (mut app, _dir) = outline_app("outline-jump", body);

    // Show, then focus the outline.
    app.on_key(ctrl('o'));
    app.on_key(ctrl('o'));

    // Move the selection down to the third heading ("## Scene B") and jump.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // Back to editing, pane still up, cursor at the selected heading's offset.
    assert!(matches!(app.mode, Mode::Insert));
    assert!(app.outline_visible);
    let expected = body.find("## Scene B").unwrap();
    assert_eq!(app.buffer.cursor_byte(), expected);
}

#[test]
fn focus_starts_at_the_current_section() {
    let body = "# One\n\n## Scene A\n\nText here.\n\n## Scene B\n\nMore.\n";
    let (mut app, _dir) = outline_app("outline-current", body);

    // Put the cursor inside Scene A's body, then focus the outline.
    let inside_a = body.find("Text here.").unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(inside_a);
    app.on_key(ctrl('o')); // show
    app.on_key(ctrl('o')); // focus

    // Selection lands on Scene A (index 1), not the H1 title. Enter is a no-op
    // move to the same section start.
    let Mode::Outline { selected, .. } = app.mode else {
        panic!("outline should be focused");
    };
    assert_eq!(selected, 1);

    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.cursor_byte(), body.find("## Scene A").unwrap());
}

#[test]
fn esc_returns_to_editing_without_moving_the_cursor() {
    let body = "# One\n\n## Scene A\n\nText.\n\n## Scene B\n\nMore.\n";
    let (mut app, _dir) = outline_app("outline-esc", body);
    app.on_key(ctrl('o')); // show
    app.on_key(ctrl('o')); // focus
    let before = app.buffer.cursor_byte();

    // Move the selection, then Esc — the cursor must not have moved.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Esc));

    assert!(matches!(app.mode, Mode::Insert));
    assert!(app.outline_visible); // the pane stays up
    assert_eq!(app.buffer.cursor_byte(), before);
}

#[test]
fn selection_clamps_at_the_ends() {
    let body = "## Scene A\n\nText.\n\n## Scene B\n\nMore.\n";
    let (mut app, _dir) = outline_app("outline-clamp", body);
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.on_key(ctrl('o')); // show
    app.on_key(ctrl('o')); // focus — selection at Scene A (index 0)

    // Up at the top stays at 0.
    app.on_key(key(KeyCode::Up));
    assert!(matches!(app.mode, Mode::Outline { selected: 0, .. }));

    // Down past the end clamps to the last heading (index 1).
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert!(matches!(app.mode, Mode::Outline { selected: 1, .. }));
}

#[test]
fn focus_reports_when_there_are_no_headings() {
    let (mut app, _dir) = outline_app("outline-empty", "Just prose, no headings.\n");
    app.on_key(ctrl('o')); // show the (empty) pane
    app.on_key(ctrl('o')); // attempt to focus

    // No headings → stays in editing with a message rather than an empty focus.
    assert!(matches!(app.mode, Mode::Insert));
    assert!(app.message.as_deref().unwrap_or("").contains("No headings"));
}

/// #717: autosave (#409) fires on the idle tick regardless of mode, and the idle
/// tick reparses. Neither may shift the byte offset a focused outline jumps to.
/// The buffer is dirty and idle-elapsed while the outline is focused, so both a
/// `tick_autosave` and an `on_idle` reparse run before Enter; the jump must still
/// land on the selected heading's live offset.
#[test]
fn autosave_and_reparse_cant_shift_a_focused_outline_jump() {
    let body = "# One\n\n## Scene A\n\nText.\n\n## Scene B\n\nMore.\n";
    let (mut app, _dir) = outline_app("outline-autosave-jump", body);
    app.autosave_interval_secs = 1; // autosave on

    // Focus the outline and select "## Scene B".
    app.on_key(ctrl('o')); // show
    app.on_key(ctrl('o')); // focus
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert!(matches!(app.mode, Mode::Outline { selected: 2, .. }));

    // The buffer is dirty and has gone idle while the outline is up: the event
    // loop would run both the idle-timer autosave and an idle reparse here.
    app.buffer.dirty = true;
    app.last_activity = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    app.tick_autosave();
    app.on_idle();

    // Enter still lands on the live "## Scene B" offset — the save and reparse
    // left the in-memory offsets untouched.
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.buffer.cursor_byte(), body.find("## Scene B").unwrap());
    // The autosave did reach disk (proving it actually ran, not just no-op'd).
    assert!(!app.buffer.dirty, "autosave saved the dirty buffer");
}

#[test]
fn selection_clamps_when_the_heading_list_shrinks() {
    let body = "## Scene A\n\nText.\n\n## Scene B\n\nMore.\n\n## Scene C\n\nEnd.\n";
    let (mut app, _dir) = outline_app("outline-shrink", body);
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.on_key(ctrl('o')); // show
    app.on_key(ctrl('o')); // focus — selection at Scene A (index 0)

    // Select the last heading ("## Scene C", index 2).
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert!(matches!(app.mode, Mode::Outline { selected: 2, .. }));

    // The heading list shrinks under the focused outline: Scene B and Scene C are
    // gone, leaving only Scene A. A reparse makes the shorter list live.
    let shrunk = "Intro line.\n\n## Scene A\n\nText only.\n";
    app.buffer.replace_text(shrunk);
    app.buffer.refresh_syntax();
    assert_eq!(app.buffer.syntax().headings().len(), 1);
    // Park the cursor away from the heading so a failed jump would be observable.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);

    // The next keystroke re-clamps the stale index (2) to the last live heading
    // (0), so Enter lands on a real scene instead of silently no-op'ing.
    app.on_key(key(KeyCode::Up)); // clamps to 0, then a no-op move at the top
    assert!(matches!(app.mode, Mode::Outline { selected: 0, .. }));
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Insert));
    // Without the clamp, `selected` (2) would be out of range and Enter would
    // leave the cursor at 0; with it, the cursor jumps to the one live heading.
    assert_eq!(app.buffer.cursor_byte(), shrunk.find("## Scene A").unwrap());
    assert_ne!(app.buffer.cursor_byte(), 0);
}

/// Helper: the focused outline's `(selected, filter, typing)` state.
fn outline_state(app: &App) -> (usize, String, bool) {
    match &app.mode {
        Mode::Outline {
            selected,
            filter,
            typing,
        } => (*selected, filter.clone(), *typing),
        _ => panic!("expected the focused outline"),
    }
}

#[test]
fn slash_filters_the_outline_and_enter_jumps_to_the_match() {
    // Three scenes; only "Storm" should survive a "storm" filter.
    let body = "# One\n\n## Calm\n\nText.\n\n## The Storm\n\nText.\n\n## Aftermath\n\nText.\n";
    let (mut app, _dir) = outline_app("outline-filter-jump", body);
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.on_key(ctrl('o')); // show
    app.on_key(ctrl('o')); // focus — headings: One(0) Calm(1) Storm(2) Aftermath(3)

    // `/` enters the filter sub-mode; typing narrows to the storm heading.
    app.on_key(key(KeyCode::Char('/')));
    assert!(outline_state(&app).2, "typing after /");
    type_str(&mut app, "storm");
    let (selected, filter, typing) = outline_state(&app);
    assert_eq!(filter, "storm");
    assert!(typing);
    // The selection snapped onto the one heading that matches (index 2).
    assert_eq!(selected, 2);

    // Enter jumps the editor cursor to that heading and returns to editing.
    app.on_key(key(KeyCode::Enter)); // leaves the input sub-mode (keeps filter)
    assert!(matches!(app.mode, Mode::Outline { .. }));
    app.on_key(key(KeyCode::Enter)); // acts on the match
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.buffer.cursor_byte(), body.find("## The Storm").unwrap());
}

#[test]
fn outline_filter_movement_skips_hidden_headings() {
    let body = "## Alpha\n\nA.\n\n## Alpha Two\n\nB.\n\n## Beta\n\nC.\n";
    let (mut app, _dir) = outline_app("outline-filter-move", body);
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.on_key(ctrl('o'));
    app.on_key(ctrl('o')); // Alpha(0) Alpha Two(1) Beta(2)

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "alpha"); // matches 0 and 1, not Beta
    assert_eq!(outline_state(&app).0, 0);

    // Down within the input sub-mode steps to the next *matching* heading (1),
    // never landing on the hidden "Beta".
    app.on_key(key(KeyCode::Down));
    assert_eq!(outline_state(&app).0, 1);
    app.on_key(key(KeyCode::Down)); // no further match — stays on 1
    assert_eq!(outline_state(&app).0, 1);
}

#[test]
fn outline_esc_clears_the_filter_then_returns_to_editing() {
    let body = "## Calm\n\nA.\n\n## Storm\n\nB.\n";
    let (mut app, _dir) = outline_app("outline-filter-esc", body);
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.on_key(ctrl('o'));
    app.on_key(ctrl('o'));

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "storm");
    app.on_key(key(KeyCode::Esc)); // leave input, keep the filter
    assert_eq!(outline_state(&app).1, "storm");
    assert!(!outline_state(&app).2);

    // First command-mode Esc clears the filter (list restored, still focused).
    app.on_key(key(KeyCode::Esc));
    assert_eq!(outline_state(&app).1, "");
    assert!(matches!(app.mode, Mode::Outline { .. }));

    // Second Esc returns to editing.
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert));
}

// --- Restructure from the outline (#1008) --------------------------------

/// Focus the outline and step the selection to heading `n` (0-based over the live
/// heading list). Robust to the starting mode: parks the cursor at the top and
/// re-enters the focused outline from editing, so a prior gesture that left the
/// outline focused (or moved the selection) doesn't skew the result.
fn focus_and_select(app: &mut App, n: usize) {
    // Return to editing first (Ctrl-O from a focused outline hides the pane), then
    // put the cursor at the document top so focusing lands on the first heading.
    if matches!(app.mode, Mode::Outline { .. }) {
        app.on_key(key(KeyCode::Esc));
        if matches!(app.mode, Mode::Outline { .. }) {
            app.on_key(key(KeyCode::Esc)); // a second Esc clears a filter, then exits
        }
    }
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    if !app.outline_visible {
        app.on_key(ctrl('o')); // show
    }
    app.on_key(ctrl('o')); // focus — selection at the first heading (cursor at top)
                           // Step to `n` from a known selection of 0.
    for _ in 0..8 {
        if matches!(app.mode, Mode::Outline { selected: 0, .. }) {
            break;
        }
        app.on_key(key(KeyCode::Up));
    }
    for _ in 0..n {
        app.on_key(key(KeyCode::Down));
    }
}

#[test]
fn demote_then_promote_changes_the_heading_level() {
    let body = "## Scene A\n\nText.\n\n## Scene B\n\nMore.\n";
    let (mut app, _dir) = outline_app("outline-reheading", body);
    focus_and_select(&mut app, 0); // "## Scene A"

    // `>` demotes: one more `#`. Only the marker run changes; the title and body stay.
    app.on_key(key(KeyCode::Char('>')));
    assert_eq!(
        app.buffer.text(),
        "### Scene A\n\nText.\n\n## Scene B\n\nMore.\n"
    );
    assert_eq!(app.buffer.syntax().headings()[0].level, 3);

    // `<` promotes back to level 2, then again to level 1.
    app.on_key(key(KeyCode::Char('<')));
    assert_eq!(app.buffer.syntax().headings()[0].level, 2);
    app.on_key(key(KeyCode::Char('<')));
    assert_eq!(
        app.buffer.text(),
        "# Scene A\n\nText.\n\n## Scene B\n\nMore.\n"
    );
    assert_eq!(app.buffer.syntax().headings()[0].level, 1);

    // Promoting a level-1 heading is a no-op with a message — never an empty prefix.
    app.on_key(key(KeyCode::Char('<')));
    assert_eq!(app.buffer.syntax().headings()[0].level, 1);
    assert!(app.message.as_deref().unwrap_or("").contains("top-level"));
}

#[test]
fn demote_clamps_at_the_deepest_level() {
    let body = "###### Deep\n\nText.\n";
    let (mut app, _dir) = outline_app("outline-reheading-clamp", body);
    focus_and_select(&mut app, 0);
    app.on_key(key(KeyCode::Char('>'))); // already level 6 — no-op
    assert_eq!(app.buffer.text(), body);
    assert_eq!(app.buffer.syntax().headings()[0].level, 6);
    assert!(app.message.as_deref().unwrap_or("").contains("deepest"));
}

#[test]
fn reheading_is_one_undo_step() {
    let body = "## Scene A\n\nText.\n\n## Scene B\n\nMore.\n";
    let (mut app, _dir) = outline_app("outline-reheading-undo", body);
    focus_and_select(&mut app, 0);
    app.on_key(key(KeyCode::Char('>'))); // level 2 -> 3
    assert_eq!(app.buffer.syntax().headings()[0].level, 3);

    // A single undo restores the whole gesture — level and text back to the start.
    assert!(app.buffer.undo());
    app.buffer.refresh_syntax();
    assert_eq!(app.buffer.text(), body);
    assert_eq!(app.buffer.syntax().headings()[0].level, 2);
}

#[test]
fn move_section_down_carries_the_heading_and_its_body() {
    let body = "## Scene A\n\nAlpha body.\n\n## Scene B\n\nBeta body.\n";
    let (mut app, _dir) = outline_app("outline-move-down", body);
    focus_and_select(&mut app, 0); // "## Scene A"

    // `J` moves Scene A down past Scene B — the whole block travels, so the order
    // flips and every byte of both bodies is preserved (nothing dropped/split).
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "## Scene B\n\nBeta body.\n\n## Scene A\n\nAlpha body.\n"
    );
    // The outline followed the moved section: it's now the second heading (index 1).
    assert!(matches!(app.mode, Mode::Outline { selected: 1, .. }));
    let titles: Vec<_> = app
        .buffer
        .syntax()
        .headings()
        .iter()
        .map(|h| h.title.clone())
        .collect();
    assert_eq!(titles, vec!["Scene B", "Scene A"]);
}

#[test]
fn move_section_up_is_the_inverse_and_undoes_in_one_step() {
    let body = "## Scene A\n\nAlpha.\n\n## Scene B\n\nBeta.\n";
    let (mut app, _dir) = outline_app("outline-move-up", body);
    focus_and_select(&mut app, 1); // "## Scene B"

    app.on_key(key(KeyCode::Char('K'))); // move B up past A
    assert_eq!(
        app.buffer.text(),
        "## Scene B\n\nBeta.\n\n## Scene A\n\nAlpha.\n"
    );
    assert!(matches!(app.mode, Mode::Outline { selected: 0, .. }));

    // One undo restores the original order (a single gesture-scoped edit).
    assert!(app.buffer.undo());
    app.buffer.refresh_syntax();
    assert_eq!(app.buffer.text(), body);
    assert!(!app.buffer.undo(), "the move was exactly one undo step");
}

#[test]
fn move_section_carries_folded_in_subheadings() {
    // Scene A owns a "### Beat" sub-heading; moving A must carry the sub-beat and its
    // body as one block, never splitting the section at the deeper heading.
    let body = "## Scene A\n\nAlpha.\n\n### Beat\n\nBeat body.\n\n## Scene B\n\nBeta.\n";
    let (mut app, _dir) = outline_app("outline-move-subheading", body);
    focus_and_select(&mut app, 0); // "## Scene A" (its "### Beat" is a sub-beat)

    app.on_key(key(KeyCode::Char('J'))); // move A (with its ### Beat) past B
    assert_eq!(
        app.buffer.text(),
        "## Scene B\n\nBeta.\n\n## Scene A\n\nAlpha.\n\n### Beat\n\nBeat body.\n"
    );
    // Every heading survived, reordered as whole blocks.
    let titles: Vec<_> = app
        .buffer
        .syntax()
        .headings()
        .iter()
        .map(|h| h.title.clone())
        .collect();
    assert_eq!(titles, vec!["Scene B", "Scene A", "Beat"]);
}

#[test]
fn move_section_is_a_no_op_at_the_boundaries() {
    let body = "## Scene A\n\nAlpha.\n\n## Scene B\n\nBeta.\n";
    let (mut app, _dir) = outline_app("outline-move-bounds", body);

    // Up from the first sibling: no-op with a message, text untouched.
    focus_and_select(&mut app, 0);
    app.on_key(key(KeyCode::Char('K')));
    assert_eq!(app.buffer.text(), body);
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("first section"));

    // Down from the last sibling: likewise a no-op.
    focus_and_select(&mut app, 1);
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.buffer.text(), body);
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("last section"));
}

#[test]
fn move_section_only_reorders_among_siblings() {
    // A shallower "# Title" separates two level-2 scenes. Moving Scene A up must not
    // jump past the chapter title (its parent) — Scene A is the first sibling under
    // the title, so up is a no-op.
    let body = "# Title\n\n## Scene A\n\nAlpha.\n\n## Scene B\n\nBeta.\n";
    let (mut app, _dir) = outline_app("outline-move-siblings", body);
    focus_and_select(&mut app, 1); // "## Scene A" (index 1; "# Title" is 0)

    app.on_key(key(KeyCode::Char('K'))); // no sibling above — bounded by the title
    assert_eq!(app.buffer.text(), body);
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("first section"));

    // But down past its sibling Scene B works, carrying the body.
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "# Title\n\n## Scene B\n\nBeta.\n\n## Scene A\n\nAlpha.\n"
    );
}

#[test]
fn restructure_persists_to_disk() {
    let body = "## Scene A\n\nAlpha.\n\n## Scene B\n\nBeta.\n";
    let (mut app, dir) = outline_app("outline-restructure-persist", body);
    focus_and_select(&mut app, 0);
    app.on_key(key(KeyCode::Char('J'))); // move Scene A down; the gesture saves

    assert!(!app.buffer.dirty, "the gesture flushed the chapter to disk");
    let on_disk = fs::read_to_string(dir.join("one.md")).unwrap();
    assert_eq!(on_disk, "## Scene B\n\nBeta.\n\n## Scene A\n\nAlpha.\n");
}

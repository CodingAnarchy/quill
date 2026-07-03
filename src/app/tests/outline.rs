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
    let Mode::Outline { selected } = app.mode else {
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
    assert!(matches!(app.mode, Mode::Outline { selected: 0 }));

    // Down past the end clamps to the last heading (index 1).
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert!(matches!(app.mode, Mode::Outline { selected: 1 }));
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
    assert!(matches!(app.mode, Mode::Outline { selected: 2 }));

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
    assert!(matches!(app.mode, Mode::Outline { selected: 2 }));

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
    assert!(matches!(app.mode, Mode::Outline { selected: 0 }));
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Insert));
    // Without the clamp, `selected` (2) would be out of range and Enter would
    // leave the cursor at 0; with it, the cursor jumps to the one live heading.
    assert_eq!(app.buffer.cursor_byte(), shrunk.find("## Scene A").unwrap());
    assert_ne!(app.buffer.cursor_byte(), 0);
}

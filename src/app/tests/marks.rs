//! Tests for the go-to-line prompt and the recent-edit-locations ring (#1010):
//! a direct jump to a line number, the automatic breadcrumb of recent edit
//! spots cycled back through (bounded), the `gi`-style jump to the last
//! insertion, and that the ring survives ordinary navigation and coexists with
//! the manually-set Vim marks.

use super::*;
use crate::mode::Action;

/// A one-chapter project whose single chapter holds `body`, opened in the
/// modeless editor (Insert). Distinct known-length lines make line/byte
/// assertions easy.
fn line_app(name: &str, body: &str) -> (App, TempProject) {
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

// --- Go to line -----------------------------------------------------------

#[test]
fn go_to_line_moves_the_cursor_to_that_line() {
    // Five lines; line 3 (1-based) is "gamma".
    let (mut app, _dir) = line_app("goto-line-basic", "alpha\nbeta\ngamma\ndelta\nepsilon\n");
    app.open_go_to_line();
    assert!(matches!(app.mode, Mode::Prompt(_)), "prompt opens");
    // Replace the prefilled current line with "3".
    if let Mode::Prompt(state) = &mut app.mode {
        state.clear();
    }
    type_str(&mut app, "3");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.cursor.line, 2, "landed on 1-based line 3");
    assert!(!matches!(app.mode, Mode::Prompt(_)), "prompt closed");
}

#[test]
fn go_to_line_past_the_end_clamps_to_the_last_line() {
    let (mut app, _dir) = line_app("goto-line-clamp", "alpha\nbeta\ngamma\n");
    app.open_go_to_line();
    if let Mode::Prompt(state) = &mut app.mode {
        state.clear();
    }
    type_str(&mut app, "999");
    app.on_key(key(KeyCode::Enter));
    // "gamma" is the last content line (index 2); the trailing newline's empty
    // line isn't the target, like Vim's G.
    assert_eq!(app.buffer.cursor.line, 2, "clamped to last content line");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Past the end"),
        "reported the clamp: {:?}",
        app.message
    );
}

#[test]
fn go_to_line_rejects_a_non_number() {
    let (mut app, _dir) = line_app("goto-line-nan", "alpha\nbeta\ngamma\n");
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.open_go_to_line();
    if let Mode::Prompt(state) = &mut app.mode {
        state.clear();
    }
    type_str(&mut app, "abc");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.cursor.line, 0, "cursor unmoved on a bad entry");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Not a line number"),
        "{:?}",
        app.message
    );
}

#[test]
fn go_to_line_pushes_the_jumplist_so_back_jump_returns() {
    let (mut app, _dir) = line_app("goto-line-jumpback", "alpha\nbeta\ngamma\ndelta\n");
    // Start on line 2, byte at its start.
    let l2 = app.buffer.line_start_byte(1);
    app.buffer.cursor = app.buffer.cursor_at_byte(l2);
    app.open_go_to_line();
    if let Mode::Prompt(state) = &mut app.mode {
        state.clear();
    }
    type_str(&mut app, "4");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.cursor.line, 3, "moved to line 4");
    app.jump_back(); // `` `` ``
    assert_eq!(
        app.buffer.cursor.line, 1,
        "back-jump returns to where we left"
    );
}

#[test]
fn go_to_line_action_opens_the_prompt() {
    let (mut app, _dir) = line_app("goto-line-action", "alpha\nbeta\n");
    app.run_action(Action::GoToLine, false);
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::GoToLine)),
        "the run-action opens the go-to-line prompt"
    );
}

// --- Recent edit ring: recording & cycling --------------------------------

/// Type text on line `line` (0-based) by placing the cursor at its start first,
/// so each burst records a distinct edit spot.
fn edit_on_line(app: &mut App, line: usize, text: &str) {
    let start = app.buffer.line_start_byte(line);
    app.buffer.cursor = app.buffer.cursor_at_byte(start);
    type_str(app, text);
}

#[test]
fn edits_record_spots_and_cycling_back_walks_them() {
    let (mut app, _dir) = line_app("ring-record", "alpha\nbeta\ngamma\ndelta\n");
    // Edit three distinct lines in order: 0, 2, 3.
    edit_on_line(&mut app, 0, "X");
    edit_on_line(&mut app, 2, "Y");
    edit_on_line(&mut app, 3, "Z");
    // The ring holds three distinct spots (same-line keystrokes coalesced).
    assert_eq!(
        app.vim_state.edits.spots.len(),
        3,
        "one spot per edited line"
    );

    // Move somewhere unrelated, then cycle back: newest edit first (line 3),
    // then line 2, then line 0.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_older_edit();
    assert_eq!(app.buffer.cursor.line, 3, "first step → newest edit");
    app.jump_to_older_edit();
    assert_eq!(app.buffer.cursor.line, 2, "second step → older edit");
    app.jump_to_older_edit();
    assert_eq!(app.buffer.cursor.line, 0, "third step → oldest edit");
}

#[test]
fn cycling_back_is_bounded_at_the_oldest_spot() {
    let (mut app, _dir) = line_app("ring-bounded", "alpha\nbeta\ngamma\n");
    edit_on_line(&mut app, 0, "X");
    edit_on_line(&mut app, 2, "Y");
    app.jump_to_older_edit(); // line 2
    app.jump_to_older_edit(); // line 0
    app.jump_to_older_edit(); // no earlier — stays put
    assert_eq!(app.buffer.cursor.line, 0, "clamped at the oldest edit");
    assert!(
        app.message.as_deref().unwrap_or("").contains("Oldest edit"),
        "{:?}",
        app.message
    );
    // Then step forward again toward newer spots.
    app.jump_to_newer_edit();
    assert_eq!(
        app.buffer.cursor.line, 2,
        "forward returns toward the newest"
    );
}

#[test]
fn the_ring_is_bounded_and_drops_the_oldest() {
    // A tall buffer so we can edit more distinct lines than the ring holds.
    let mut body = String::new();
    for i in 0..40 {
        writeln!(body, "line{i}").unwrap();
    }
    let (mut app, _dir) = line_app("ring-cap", &body);
    for i in 0..40 {
        edit_on_line(&mut app, i, "*");
    }
    // The cap is 20, so the ring never grows past it.
    assert!(
        app.vim_state.edits.spots.len() <= 20,
        "ring is bounded: {}",
        app.vim_state.edits.spots.len()
    );
    // The oldest spots dropped: the very first edited line is no longer reachable
    // by cycling all the way back.
    let mut reached = Vec::new();
    for _ in 0..25 {
        app.jump_to_older_edit();
        reached.push(app.buffer.cursor.line);
    }
    assert!(
        !reached.contains(&0),
        "the oldest edit (line 0) was dropped from the bounded ring"
    );
}

#[test]
fn jump_to_last_insertion_returns_to_where_text_was_added() {
    let (mut app, _dir) = line_app("ring-gi", "alpha\nbeta\ngamma\n");
    // Insert on line 1 last.
    edit_on_line(&mut app, 1, "!");
    let inserted_line = app.buffer.cursor.line;
    // Wander off.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    assert_eq!(app.buffer.cursor.line, 0);
    app.jump_to_last_insertion();
    assert_eq!(
        app.buffer.cursor.line, inserted_line,
        "gi returns to the last insertion"
    );
}

#[test]
fn last_insertion_tracks_insertions_not_deletions() {
    let (mut app, _dir) = line_app("ring-gi-insert-only", "alpha\nbeta\ngamma\n");
    edit_on_line(&mut app, 0, "A"); // insertion on line 0
                                    // Delete on line 2 (a forward delete, not an insertion).
    let l2 = app.buffer.line_start_byte(2);
    app.buffer.cursor = app.buffer.cursor_at_byte(l2);
    app.edit_delete_forward();
    // The last-insertion target is still line 0, not the deletion on line 2.
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.line_start_byte(2));
    app.jump_to_last_insertion();
    assert_eq!(
        app.buffer.cursor.line, 0,
        "gi tracks the last insertion, not a later deletion"
    );
}

#[test]
fn same_line_edits_coalesce_into_one_spot() {
    let (mut app, _dir) = line_app("ring-coalesce", "alpha\nbeta\n");
    // Several keystrokes on the same line make just one ring entry.
    edit_on_line(&mut app, 0, "hello");
    assert_eq!(
        app.vim_state.edits.spots.len(),
        1,
        "a typed word is one edit spot, not one per keystroke"
    );
}

// --- Coexistence with navigation and manual marks -------------------------

#[test]
fn the_ring_survives_ordinary_navigation() {
    let (mut app, _dir) = line_app("ring-survives-nav", "alpha\nbeta\ngamma\ndelta\n");
    edit_on_line(&mut app, 0, "X");
    edit_on_line(&mut app, 3, "Y");
    let before = app.vim_state.edits.spots.len();
    // Ordinary cursor movement doesn't touch the ring.
    app.edit_move(Buffer::move_right);
    app.edit_move(Buffer::move_left);
    app.edit_move(|b| b.move_to_line(1));
    assert_eq!(
        app.vim_state.edits.spots.len(),
        before,
        "plain navigation leaves the ring untouched"
    );
    // And it's still cyclable afterward.
    app.jump_to_older_edit();
    assert_eq!(app.buffer.cursor.line, 3, "newest spot still reachable");
}

#[test]
fn the_ring_and_manual_marks_coexist() {
    let (mut app, dir) = line_app("ring-and-marks", "alpha\nbeta\ngamma\ndelta\n");
    // Turn on Vim so the mark keys are live.
    app.vim = true;
    app.mode = Mode::Normal;

    // Set a manual mark 'a' on line 1.
    let l1 = app.buffer.line_start_byte(1);
    app.buffer.cursor = app.buffer.cursor_at_byte(l1);
    type_str(&mut app, "ma");

    // Now make some edits (Insert then type), building the ring independently.
    app.mode = Mode::Insert;
    edit_on_line(&mut app, 3, "Z");

    // The ring recorded the edit; the manual mark is untouched.
    assert_eq!(app.vim_state.edits.spots.len(), 1, "ring recorded the edit");
    assert!(
        app.vim_state.marks.contains_key(&'a'),
        "manual mark survives"
    );

    // The mark still jumps to line 1.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.mode = Mode::Normal;
    type_str(&mut app, "`a");
    assert_eq!(app.buffer.cursor.line, 1, "manual mark still works");

    // And the ring still jumps to the edit on line 3 — independent of the mark.
    app.jump_to_older_edit();
    assert_eq!(
        app.buffer.cursor.line, 3,
        "the ring cycles independently of the mark"
    );
    drop(dir);
}

#[test]
fn empty_ring_and_no_insertion_are_graceful_no_ops() {
    let (mut app, _dir) = line_app("ring-empty", "alpha\nbeta\n");
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.line_start_byte(1));
    let line = app.buffer.cursor.line;
    app.jump_to_older_edit();
    assert_eq!(app.buffer.cursor.line, line, "no edits → cursor unmoved");
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No recent edits"));
    app.jump_to_last_insertion();
    assert_eq!(
        app.buffer.cursor.line, line,
        "no insertion → cursor unmoved"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No insertion"));
}

//! Tests for the go-to-line prompt and the recent-edit-locations ring (#1010):
//! a direct jump to a line number, the automatic breadcrumb of recent edit
//! spots cycled back through (bounded), the `gi`-style jump to the last
//! insertion, and that the ring survives ordinary navigation and coexists with
//! the manually-set Vim marks. Also the scene-heading motion (#1009): stepping
//! heading-to-heading within a chapter and crossing into adjacent chapters.

use super::*;
use crate::mode::Action;
use crate::project::Project;

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
    app.flags.vim = true;
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

// --- Scene-heading motion (#1009) -----------------------------------------

/// A project whose chapters hold the given bodies (`c0.md`, `c1.md`, …), opened
/// on chapter 0 in the modeless editor. Each body's headings drive the
/// scene-heading motion.
fn scene_app(name: &str, bodies: &[&str]) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    for (i, body) in bodies.iter().enumerate() {
        write!(
            manifest,
            "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
        )
        .unwrap();
        fs::write(dir.join(format!("c{i}.md")), body).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn next_scene_heading_lands_on_each_heading_in_turn() {
    // A lead-in line, then three headings with prose between them — so the first
    // heading isn't at byte 0 and "step forward from the top" lands on it.
    let body = "lead-in prose\n\n# One\n\nalpha prose\n\n# Two\n\nbeta prose\n\n# Three\n\ngamma\n";
    let (mut app, _dir) = scene_app("scene-next", &[body]);
    // Start at the very top (before the first heading).
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    let headings: Vec<usize> = app
        .buffer
        .syntax()
        .headings()
        .iter()
        .map(|h| h.start)
        .collect();
    assert_eq!(headings.len(), 3, "three headings parsed");

    // Each `]]` lands on the next heading in document order.
    app.jump_to_scene_heading(true);
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[0],
        "first `]]` → heading 1"
    );
    app.jump_to_scene_heading(true);
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[1],
        "second `]]` → heading 2"
    );
    app.jump_to_scene_heading(true);
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[2],
        "third `]]` → heading 3"
    );
}

#[test]
fn previous_scene_heading_walks_back_up() {
    let body = "# One\n\nalpha\n\n# Two\n\nbeta\n\n# Three\n\ngamma\n";
    let (mut app, _dir) = scene_app("scene-prev", &[body]);
    let headings: Vec<usize> = app
        .buffer
        .syntax()
        .headings()
        .iter()
        .map(|h| h.start)
        .collect();
    // Start at the end of the buffer.
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_scene_heading(false);
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[2],
        "first `[[` → heading 3"
    );
    app.jump_to_scene_heading(false);
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[1],
        "second `[[` → heading 2"
    );
    app.jump_to_scene_heading(false);
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[0],
        "third `[[` → heading 1"
    );
}

#[test]
fn next_scene_heading_crosses_into_the_next_chapter() {
    // Chapter 0 has one heading; the author is past it. Chapter 1 has a heading.
    let (mut app, _dir) = scene_app(
        "scene-cross-next",
        &[
            "# Only\n\nalpha prose here\n",
            "# Next chapter head\n\nbeta\n",
        ],
    );
    // Move past chapter 0's single heading, so there's nothing left forward here.
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_scene_heading(true);
    // Crossed into chapter 1, landed on its first heading (byte 0).
    assert_eq!(app.project.active_index(), 1, "crossed into chapter 1");
    assert_eq!(app.buffer.cursor_byte(), 0, "landed on chapter 1's heading");
}

#[test]
fn previous_scene_heading_crosses_back_and_lands_on_the_last_heading() {
    // Chapter 0 has two headings; chapter 1 has one. From chapter 1 above its
    // heading, `[[` crosses back and lands on chapter 0's *last* heading.
    let ch0 = "# A\n\none\n\n# B\n\ntwo\n";
    let (mut app, _dir) = scene_app("scene-cross-prev", &[ch0, "# C\n\nthree\n"]);
    // Open chapter 1 and sit at its top (before its only heading).
    app.activate_chapter(1);
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_scene_heading(false);
    assert_eq!(app.project.active_index(), 0, "crossed back into chapter 0");
    // Chapter 0's last heading is "# B", not "# A".
    let last = app.buffer.syntax().headings().last().unwrap().start;
    assert_eq!(
        app.buffer.cursor_byte(),
        last,
        "landed on chapter 0's last heading"
    );
}

#[test]
fn next_scene_heading_skips_a_chapter_with_no_headings() {
    // Chapter 1 is prose-only; the forward crossing should skip it to chapter 2.
    let (mut app, _dir) = scene_app(
        "scene-skip-empty",
        &[
            "# Start\n\nalpha\n",
            "just prose, no heading\n",
            "# Far\n\ngamma\n",
        ],
    );
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_scene_heading(true);
    assert_eq!(
        app.project.active_index(),
        2,
        "skipped heading-less chapter 1"
    );
    assert_eq!(app.buffer.cursor_byte(), 0, "landed on chapter 2's heading");
}

#[test]
fn next_scene_heading_reports_the_end_of_the_manuscript() {
    let (mut app, _dir) = scene_app("scene-end-fwd", &["# Only\n\nalpha\n"]);
    // Past the single heading, with no later chapter.
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    let before = app.buffer.cursor_byte();
    app.jump_to_scene_heading(true);
    assert_eq!(
        app.buffer.cursor_byte(),
        before,
        "cursor unmoved at the end"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No next scene heading"),
        "reported the end: {:?}",
        app.message
    );
}

#[test]
fn previous_scene_heading_reports_the_start_of_the_manuscript() {
    let (mut app, _dir) = scene_app("scene-end-back", &["# Only\n\nalpha\n"]);
    // Above the only heading, in the first chapter.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_scene_heading(false);
    assert_eq!(app.buffer.cursor_byte(), 0, "cursor unmoved at the start");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No previous scene heading"),
        "reported the start: {:?}",
        app.message
    );
}

#[test]
fn crossing_scene_heading_pushes_the_jumplist_so_back_jump_returns() {
    let (mut app, _dir) = scene_app(
        "scene-jumpback",
        &["# Only\n\nalpha prose\n", "# Next\n\nbeta\n"],
    );
    // Sit at a known spot in chapter 0, past its heading.
    let spot = app.buffer.len_bytes();
    app.buffer.cursor = app.buffer.cursor_at_byte(spot);
    app.jump_to_scene_heading(true); // crosses to chapter 1
    assert_eq!(app.project.active_index(), 1, "crossed forward");
    app.jump_back(); // `` `` ``
    assert_eq!(
        app.project.active_index(),
        0,
        "back-jump returns to chapter 0"
    );
}

#[test]
fn scene_heading_motion_works_without_opening_the_outline() {
    // The motion runs straight from the editor — the mode never becomes the outline.
    // A lead-in line keeps the first heading off byte 0 so stepping lands on it.
    let body = "lead\n\n# One\n\nalpha\n\n# Two\n\nbeta\n";
    let (mut app, _dir) = scene_app("scene-no-overlay", &[body]);
    app.mode = Mode::Insert;
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_scene_heading(true);
    assert!(
        matches!(app.mode, Mode::Insert),
        "stayed in the editor, no overlay"
    );
    let first = app.buffer.syntax().headings()[0].start;
    assert_eq!(
        app.buffer.cursor_byte(),
        first,
        "moved to the first heading"
    );
}

#[test]
fn bracket_bracket_keys_drive_the_scene_heading_motion() {
    // The `]]` / `[[` Normal-mode bindings route to the motion (#1009).
    // A lead-in line keeps the first heading off byte 0 so `]]` from the top
    // lands on it.
    let body = "lead\n\n# One\n\nalpha\n\n# Two\n\nbeta\n";
    let (mut app, _dir) = scene_app("scene-keys", &[body]);
    app.flags.vim = true;
    app.mode = Mode::Normal;
    let headings: Vec<usize> = app
        .buffer
        .syntax()
        .headings()
        .iter()
        .map(|h| h.start)
        .collect();
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    // `]]` → next heading.
    app.on_key(key(KeyCode::Char(']')));
    app.on_key(key(KeyCode::Char(']')));
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[0],
        "`]]` → first heading"
    );
    app.on_key(key(KeyCode::Char(']')));
    app.on_key(key(KeyCode::Char(']')));
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[1],
        "`]]` → second heading"
    );
    // `[[` → previous heading.
    app.on_key(key(KeyCode::Char('[')));
    app.on_key(key(KeyCode::Char('[')));
    assert_eq!(
        app.buffer.cursor_byte(),
        headings[0],
        "`[[` → first heading"
    );
}

// --- Continuous-view positions are chapter-local (#1010 jump-back bug) -----

#[test]
fn current_position_is_chapter_local_in_the_continuous_view() {
    // The window concatenates c0/c1/c2 ("alpha"/"beta"/"gamma"). With the cursor
    // in a *later* chapter the buffer byte is absolute — but the recorded position
    // must name that chapter and a byte *within* it, not `active_index()` + the
    // absolute offset.
    let (mut app, _dir) = three_chapter_app("cont-pos-local");
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());
    // active_index stays pinned to chapter 0 where the view opened.
    assert_eq!(app.project.active_index(), 0);

    // Move into chapter 2 ("gamma") and sit at the end of its first line.
    app.on_key(key(KeyCode::Down)); // into c1
    app.on_key(key(KeyCode::Down)); // into c2
    app.on_key(key(KeyCode::End));
    let m = app.manuscript.as_ref().unwrap();
    assert_eq!(m.current_chapter(), 2, "cursor is in chapter 2");
    let abs = app.edit_buffer().cursor_byte();
    let base = m
        .segment_spans()
        .into_iter()
        .find_map(|(idx, start, _)| (idx == 2).then_some(start))
        .unwrap();
    assert!(base > 0, "chapter 2's segment is not at buffer byte 0");

    let pos = app.current_position();
    assert_eq!(
        pos.chapter, 2,
        "records the cursor's true chapter, not index 0"
    );
    assert_eq!(
        pos.byte,
        abs - base,
        "records a chapter-local byte, not the absolute one"
    );
    // "gamma" is 5 bytes; End of its first line sits at the local offset 5.
    assert_eq!(pos.byte, 5, "chapter-local offset within \"gamma\"");
}

#[test]
fn a_continuous_view_edit_round_trips_to_the_right_chapter() {
    // The end-to-end guarantee: an edit made in a later segment of the continuous
    // view records a ring spot that `restore_jump` lands *in that chapter at the
    // edit's byte* — not at the end of the wrong (opening) chapter.
    let (mut app, _dir) = three_chapter_app("cont-ring-roundtrip");
    app.on_key(ctrl('l')); // enter continuous view

    // Move into chapter 2 ("gamma") and append there — this records an edit spot.
    app.on_key(key(KeyCode::Down)); // c1
    app.on_key(key(KeyCode::Down)); // c2
    app.on_key(key(KeyCode::End));
    type_str(&mut app, "!"); // "gamma!" — the edit spot is local byte 6 in chapter 2
    app.on_idle(); // seal the edit group

    // The ring holds the spot with the true chapter and a chapter-local byte.
    let spot = *app.vim_state.edits.spots.back().unwrap();
    assert_eq!(
        spot.chapter, 2,
        "the spot names chapter 2, not the opening 0"
    );
    assert_eq!(spot.byte, 6, "chapter-local byte within \"gamma!\"");

    // Wander to the top of the window, then cycle back to the edit. `restore_jump`
    // leaves the continuous view, opens chapter 2, and places the cursor.
    app.edit_buffer_mut().cursor = app.edit_buffer().cursor_at_byte(0);
    app.jump_to_older_edit();
    assert!(app.manuscript.is_none(), "restore left the continuous view");
    assert_eq!(
        app.project.active_index(),
        2,
        "landed in chapter 2, not the opening chapter"
    );
    assert_eq!(
        app.buffer.cursor_byte(),
        6,
        "cursor at the edit's byte in chapter 2, not the end of the wrong chapter"
    );
    assert_eq!(
        app.buffer.text(),
        "gamma!\n",
        "the edit persisted to chapter 2"
    );
}

#[test]
fn a_mark_set_in_the_continuous_view_round_trips_to_the_right_chapter() {
    // A Vim mark set with the cursor in a later segment jumps back to that chapter
    // and byte, the same chapter-local contract the ring gets.
    let (mut app, _dir) = three_chapter_app("cont-mark-roundtrip");
    app.flags.vim = true;
    app.on_key(ctrl('l')); // enter continuous view
    app.mode = Mode::Normal;

    // Sit in chapter 1 ("beta") and set mark 'a' there.
    app.on_key(key(KeyCode::Down)); // c1
    app.on_key(key(KeyCode::End)); // end of "beta"
    assert_eq!(app.manuscript.as_ref().unwrap().current_chapter(), 1);
    type_str(&mut app, "ma");
    let mark = *app.vim_state.marks.get(&'a').unwrap();
    assert_eq!(mark.chapter, 1, "mark names chapter 1");
    assert_eq!(mark.byte, 4, "chapter-local byte at the end of \"beta\"");

    // Jump to it: leaves the continuous view, opens chapter 1, restores the byte.
    type_str(&mut app, "`a");
    assert!(
        app.manuscript.is_none(),
        "the mark jump left continuous view"
    );
    assert_eq!(app.project.active_index(), 1, "re-opened chapter 1");
    assert_eq!(
        app.buffer.cursor_byte(),
        4,
        "cursor at the marked byte in chapter 1"
    );
}

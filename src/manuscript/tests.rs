//! Tests for the continuous-manuscript view (see [`super`]).

use super::*;
use crate::buffer::Cursor;
use std::fmt::Write;
use std::path::PathBuf;

/// Build a temp project whose chapters hold the given contents, and return
/// the loaded `Project` plus its directory.
fn project_with(name: &str, chapters: &[&str]) -> (Project, PathBuf) {
    let dir = crate::testutil::scratch_dir("ms", name);

    let mut manifest = String::from("title = \"Book\"\n\n");
    for (i, body) in chapters.iter().enumerate() {
        let file = format!("ch{i}.md");
        write!(
            manifest,
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
        )
        .unwrap();
        fs::write(dir.join(&file), body).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    (Project::open(&dir).unwrap(), dir)
}

fn lines_sum(m: &Manuscript) -> usize {
    m.segments.iter().map(|s| s.lines).sum()
}

#[test]
fn window_bounds_centers_and_clamps() {
    assert_eq!(window_bounds(0, 2), (0, 2)); // fewer than WINDOW: all of it
    assert_eq!(window_bounds(0, 5), (0, 3)); // at the start
    assert_eq!(window_bounds(2, 5), (1, 4)); // centered
    assert_eq!(window_bounds(4, 5), (2, 5)); // pulled back from the end
}

#[test]
fn open_loads_a_window_and_line_sums_match() {
    let (project, dir) = project_with("open", &["a1\na2\n", "b1\n", "c1\n", "d1\n"]);
    let m = Manuscript::open(&project).unwrap(); // active is chapter 0

    // Only WINDOW chapters loaded, never the whole novel.
    assert_eq!(m.segments.len(), WINDOW);
    assert_eq!(m.window_lo(), 0);
    assert_eq!(m.window_hi(), 3);
    // Invariant: per-segment line counts add up to the buffer's content lines.
    assert_eq!(lines_sum(&m), m.buffer.line_count() - 1); // +1 trailing line

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn front_matter_stays_out_of_the_flow_and_is_re_attached_on_save() {
    let (mut project, dir) = project_with(
        "frontmatter",
        &["---\nstatus: draft\n---\nAlpha prose.\n", "Beta prose.\n"],
    );
    let mut m = Manuscript::open(&project).unwrap();

    // The concatenated buffer holds only prose — no YAML in the flow.
    let flow = m.buffer.line_range_text(0, m.buffer.line_count());
    assert!(flow.contains("Alpha prose."), "flow:\n{flow}");
    assert!(
        !flow.contains("status:"),
        "front matter leaked into flow:\n{flow}"
    );

    // Edit chapter 0 and save; its front matter is re-attached untouched.
    m.insert_char('X');
    m.save(&mut project).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("ch0.md")).unwrap(),
        "---\nstatus: draft\n---\nXAlpha prose.\n"
    );
    // Chapter 1 (no front matter) is untouched.
    assert_eq!(
        fs::read_to_string(dir.join("ch1.md")).unwrap(),
        "Beta prose.\n"
    );

    fs::remove_dir_all(&dir).ok();
}

/// A four-body-chapter project split into two named parts (#412): body
/// positions 1-2 are "Part One", 3-4 are "Part Two".
fn project_with_parts(name: &str) -> (Project, PathBuf) {
    let dir = crate::testutil::scratch_dir("ms", name);
    let manifest = "title = \"Book\"\n\n\
        [[parts]]\nname = \"Part One\"\nchapters = [1, 2]\n\n\
        [[parts]]\nname = \"Part Two\"\nchapters = [3, 4]\n\n\
        [[chapters]]\ntitle = \"Ch0\"\nfile = \"ch0.md\"\n\n\
        [[chapters]]\ntitle = \"Ch1\"\nfile = \"ch1.md\"\n\n\
        [[chapters]]\ntitle = \"Ch2\"\nfile = \"ch2.md\"\n\n\
        [[chapters]]\ntitle = \"Ch3\"\nfile = \"ch3.md\"\n";
    for i in 0..4 {
        fs::write(dir.join(format!("ch{i}.md")), format!("body {i}\n")).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    (Project::open(&dir).unwrap(), dir)
}

#[test]
fn part_headers_show_at_each_transition_not_repeated() {
    let (mut project, dir) = project_with_parts("part-headers");
    project.set_active(2); // window loads chapters 1, 2, 3 (WINDOW = 3)
    let m = Manuscript::open(&project).unwrap();
    let structures: Vec<_> = m
        .segment_headers()
        .into_iter()
        .map(|h| h.structure)
        .collect();
    assert_eq!(
        structures,
        vec![
            Some("Part One".to_string()), // first loaded chapter — orientation
            Some("Part Two".to_string()), // the part boundary, drawn where it begins
            None,                         // same part as the chapter above — not repeated
        ],
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_first_loaded_chapter_names_its_part_for_orientation() {
    // A window fully inside Part Two still shows "Part Two" up top, so the
    // author flowing through the middle of a part knows where they are.
    let (mut project, dir) = project_with_parts("part-orientation");
    project.set_active(3); // window loads chapters 1, 2, 3 → but center 3 pulls back
    let m = Manuscript::open(&project).unwrap();
    // chapters 1 (Part One), 2 & 3 (Part Two): first header names the part it
    // opens in regardless of whether a boundary is in view.
    let first = &m.segment_headers()[0];
    assert!(
        first.structure.is_some(),
        "the first loaded chapter names its part"
    );
    fs::remove_dir_all(&dir).ok();
}

/// A project whose first chapter is a front-matter section (a prologue),
/// followed by plain body chapters in no part (#412).
fn project_with_section(name: &str) -> (Project, PathBuf) {
    let dir = crate::testutil::scratch_dir("ms", name);
    let manifest = "title = \"Book\"\n\n\
        [[chapters]]\ntitle = \"Prologue\"\nfile = \"ch0.md\"\n\n\
        [[chapters]]\ntitle = \"Ch1\"\nfile = \"ch1.md\"\n\n\
        [[chapters]]\ntitle = \"Ch2\"\nfile = \"ch2.md\"\n";
    fs::write(dir.join("ch0.md"), "---\nrole: prologue\n---\nopening\n").unwrap();
    fs::write(dir.join("ch1.md"), "body 1\n").unwrap();
    fs::write(dir.join("ch2.md"), "body 2\n").unwrap();
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    (Project::open(&dir).unwrap(), dir)
}

#[test]
fn section_headers_name_front_matter_and_body_chapters_show_nothing() {
    let (project, dir) = project_with_section("section-header");
    let m = Manuscript::open(&project).unwrap(); // active is chapter 0
    let structures: Vec<_> = m
        .segment_headers()
        .into_iter()
        .map(|h| h.structure)
        .collect();
    assert_eq!(
        structures,
        vec![
            Some("Prologue".to_string()), // the front-matter section names itself
            None,                         // a body chapter in no part has no header
            None,
        ],
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unstructured_project_draws_no_structure_headers() {
    // The common case: no parts, no sections — the feature is invisible.
    let (project, dir) = project_with("plain", &["a\n", "b\n", "c\n"]);
    let m = Manuscript::open(&project).unwrap();
    assert!(m.segment_headers().iter().all(|h| h.structure.is_none()));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn maps_global_line_to_the_right_chapter() {
    let (project, dir) = project_with("map", &["a1\na2\n", "b1\n", "c1\n"]);
    let m = Manuscript::open(&project).unwrap();
    // Segments: a = lines [0,2), b = [2,3), c = [3,4).
    assert_eq!(m.segments[m.owner_of_line(0)].index, 0);
    assert_eq!(m.segments[m.owner_of_line(1)].index, 0);
    assert_eq!(m.segments[m.owner_of_line(2)].index, 1);
    assert_eq!(m.segments[m.owner_of_line(3)].index, 2);
    // The trailing line maps to the last segment.
    assert_eq!(m.segments[m.owner_of_line(99)].index, 2);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn edit_marks_only_the_owning_chapter_and_saves_just_that_file() {
    let (mut project, dir) = project_with("edit", &["alpha\n", "beta\n", "gamma\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    // Put the cursor in chapter 1 ("beta", global line 1) and type.
    m.buffer.cursor = Cursor { line: 1, col: 4 };
    for c in "!!".chars() {
        m.insert_char(c);
    }
    // Only chapter 1 is dirty.
    assert!(!m.segments[0].dirty);
    assert!(m.segments[1].dirty);
    assert!(!m.segments[2].dirty);

    m.save(&mut project).unwrap();
    // The edit landed in chapter 1's file; the others are untouched.
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta!!\n");
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("ch2.md")).unwrap(), "gamma\n");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn backspace_at_a_chapter_start_is_a_fixed_seam() {
    let (mut project, dir) = project_with("seam", &["alpha\n", "beta\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    // Cursor at the first position of chapter 1 ("beta", line 1, col 0).
    m.buffer.cursor = Cursor { line: 1, col: 0 };
    m.backspace(); // would merge into chapter 0 — must do nothing
    m.save(&mut project).unwrap();

    // Both chapters intact and separate.
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
    assert!(!m.dirty()); // the refused edit didn't mark anything dirty...

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn forward_delete_at_a_chapter_end_is_a_fixed_seam() {
    let (mut project, dir) = project_with("fwd-seam", &["alpha\n", "beta\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    // Cursor at the last position of chapter 0 ("alpha", line 0, col 5).
    m.buffer.cursor = Cursor { line: 0, col: 5 };
    m.delete_forward(); // would pull chapter 1 up — must do nothing
    m.save(&mut project).unwrap();

    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn forward_delete_within_a_chapter_removes_the_char_ahead() {
    let (mut project, dir) = project_with("fwd", &["alpha\n", "beta\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    m.buffer.cursor = Cursor { line: 0, col: 0 };
    m.delete_forward(); // drop the 'a' of "alpha"
    m.save(&mut project).unwrap();

    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "lpha\n");
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn word_delete_back_stops_at_the_chapter_seam() {
    let (mut project, dir) = project_with("word-seam", &["alpha\n", "beta\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    // Cursor at the first position of chapter 1 ("beta", line 1, col 0) — a
    // word-delete here has only chapter 0 to reach back into. Must not cross.
    m.buffer.cursor = Cursor { line: 1, col: 0 };
    m.delete_word_back();
    m.save(&mut project).unwrap();

    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");

    // From inside chapter 1, it deletes the word within that chapter.
    m.buffer.cursor = Cursor { line: 1, col: 4 }; // after "beta"
    m.delete_word_back();
    m.save(&mut project).unwrap();
    // "beta" is gone; an emptied chapter body normalizes to an empty file.
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn unreadable_chapter_fails_rather_than_loading_empty() {
    let (project, dir) = project_with("unreadable", &["alpha\n", "beta\n"]);
    // The file vanishes out from under the view (removed / permission lost).
    fs::remove_file(dir.join("ch1.md")).unwrap();
    // Opening the window must surface the failure, not load ch1 as empty text
    // that a later save could write back over the (now-missing) file (#154).
    assert!(Manuscript::open(&project).is_err());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reslide_keeps_the_window_intact_when_a_paged_in_chapter_is_unreadable() {
    let (mut project, dir) = project_with("reslide-fail", &["a\n", "b\n", "c\n", "d\n", "e\n"]);
    let mut m = Manuscript::open(&project).unwrap(); // window [0,3)

    // Make chapter 3 (paged in when we slide forward) unreadable, then move
    // the cursor into chapter 2 and reslide.
    fs::remove_file(dir.join("ch3.md")).unwrap();
    m.buffer.cursor = Cursor { line: 2, col: 0 };
    assert!(
        m.reslide(&mut project).is_err(),
        "the failed read is surfaced"
    );
    // The window is unchanged — the old chapters are still loaded, not blanked.
    assert_eq!((m.window_lo(), m.window_hi()), (0, 3));
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "a\n");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn scrolling_pages_chapters_in_and_out_and_saves_dropped_edits() {
    let (mut project, dir) = project_with("slide", &["a\n", "b\n", "c\n", "d\n", "e\n"]);
    let mut m = Manuscript::open(&project).unwrap(); // window [0,3)
    assert_eq!((m.window_lo(), m.window_hi()), (0, 3));

    // Edit chapter 0, then step the cursor into chapter 2 (the last loaded)
    // and reslide — that's what pages the window forward.
    m.insert_char('X'); // chapter 0 dirty: "Xa"
    m.buffer.cursor = Cursor { line: 2, col: 0 }; // into chapter 2 ("c")
    assert_eq!(m.current_chapter(), 2);
    m.reslide(&mut project).unwrap();

    // Window paged forward to center on chapter 2; still only WINDOW loaded.
    assert_eq!(m.segments.len(), WINDOW);
    assert_eq!((m.window_lo(), m.window_hi()), (1, 4));
    // Chapter 0 dropped out — its pending edit was flushed to disk first.
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "Xa\n");
    // The cursor still sits in chapter 2.
    assert_eq!(m.current_chapter(), 2);

    fs::remove_dir_all(&dir).ok();
}

// --- Undo / redo (#649) ------------------------------------------------

/// The concatenated window text, for asserting the buffer as a whole.
fn flow(m: &Manuscript) -> String {
    m.buffer.line_range_text(0, m.buffer.line_count())
}

#[test]
fn undo_and_redo_a_continuous_edit_and_save_the_right_file() {
    let (mut project, dir) = project_with("undo", &["alpha\n", "beta\n", "gamma\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    // Type into chapter 1, then seal the group as the idle tick would.
    m.buffer.cursor = Cursor { line: 1, col: 4 }; // after "beta"
    for c in "!!".chars() {
        m.insert_char(c);
    }
    m.buffer.seal_undo_group();
    assert_eq!(flow(&m), "alpha\nbeta!!\ngamma\n");

    // Undo reverts the edit and clears the dirty flag, so nothing is written.
    assert!(m.undo());
    assert_eq!(flow(&m), "alpha\nbeta\ngamma\n");
    assert!(!m.dirty(), "undo restored the pre-edit (clean) state");
    assert_eq!(lines_sum(&m), m.buffer.line_count() - 1);

    // Redo re-applies it and re-dirties chapter 1; a save writes just that file.
    assert!(m.redo());
    assert_eq!(flow(&m), "alpha\nbeta!!\ngamma\n");
    m.save(&mut project).unwrap();
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta!!\n");
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("ch2.md")).unwrap(), "gamma\n");

    // Nothing left to redo.
    assert!(!m.redo());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_restores_line_counts_for_an_edit_that_added_a_line() {
    // A newline-adding edit near a chapter seam: undo must put the seam back
    // exactly, so the segment line counts stay consistent and the chapters
    // never merge.
    let (mut project, dir) = project_with("undo-seam", &["a1\na2\n", "b1\n", "c1\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    // Split chapter 0's last line by inserting a newline just before the seam
    // into chapter 1.
    m.buffer.cursor = Cursor { line: 1, col: 1 }; // inside "a2", chapter 0
    m.insert_char('\n');
    m.buffer.seal_undo_group();
    assert_eq!(m.segments[0].lines, 3); // chapter 0 grew a line
    assert_eq!(flow(&m), "a1\na\n2\nb1\nc1\n");

    // Undo drops the added line — segment 0 is back to two lines, and the seam
    // to chapter 1 lands where it did.
    assert!(m.undo());
    assert_eq!(m.segments[0].lines, 2);
    assert_eq!(m.segments[1].index, 1);
    assert_eq!(flow(&m), "a1\na2\nb1\nc1\n");
    assert_eq!(lines_sum(&m), m.buffer.line_count() - 1);

    // Saving after the undo leaves both chapters as they were.
    m.save(&mut project).unwrap();
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "a1\na2\n");
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "b1\n");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_walks_back_edits_across_two_chapters_each_to_its_own_file() {
    let (mut project, dir) = project_with("undo-two", &["alpha\n", "beta\n", "gamma\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    // First edit group: append to chapter 0.
    m.buffer.cursor = Cursor { line: 0, col: 5 };
    m.insert_char('0');
    m.buffer.seal_undo_group();
    // Second edit group: append to chapter 2.
    m.buffer.cursor = Cursor { line: 2, col: 5 };
    m.insert_char('2');
    m.buffer.seal_undo_group();
    assert_eq!(flow(&m), "alpha0\nbeta\ngamma2\n");
    assert!(m.segments[0].dirty && m.segments[2].dirty);

    // Undo peels off the chapter-2 edit only; chapter 0's edit stays.
    assert!(m.undo());
    assert_eq!(flow(&m), "alpha0\nbeta\ngamma\n");
    assert!(m.segments[0].dirty, "chapter 0's edit is still pending");
    assert!(!m.segments[2].dirty, "chapter 2 is back to clean");

    // A save now writes only chapter 0's surviving edit.
    m.save(&mut project).unwrap();
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha0\n");
    assert_eq!(fs::read_to_string(dir.join("ch2.md")).unwrap(), "gamma\n");

    // Undo the remaining edit: everything is back to the opened state.
    assert!(m.undo());
    assert_eq!(flow(&m), "alpha\nbeta\ngamma\n");
    assert!(!m.dirty());
    assert!(!m.undo(), "nothing left to undo");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_reslide_drops_undo_history_so_the_stacks_stay_in_step() {
    let (mut project, dir) = project_with("undo-reslide", &["a\n", "b\n", "c\n", "d\n", "e\n"]);
    let mut m = Manuscript::open(&project).unwrap(); // window [0,3)

    m.insert_char('X'); // chapter 0 dirty
    m.buffer.seal_undo_group();
    // Page the window forward — the buffer is rebuilt from disk.
    m.buffer.cursor = Cursor { line: 2, col: 0 };
    m.reslide(&mut project).unwrap();

    // The undo history is gone with the rebuilt buffer; undo is a clean no-op,
    // never a desync against the new window.
    assert!(!m.undo());
    assert_eq!((m.window_lo(), m.window_hi()), (1, 4));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_refused_seam_backspace_records_no_undo_entry() {
    // A backspace at a chapter's first position is a no-op; it must not leave a
    // phantom undo entry that would step the buffer with nothing to revert.
    let (project, dir) = project_with("undo-noop", &["alpha\n", "beta\n"]);
    let mut m = Manuscript::open(&project).unwrap();

    m.buffer.cursor = Cursor { line: 1, col: 0 }; // top of chapter 1 — refused
    m.backspace();
    assert!(!m.dirty());
    assert!(!m.undo(), "the refused edit left nothing to undo");
    fs::remove_dir_all(&dir).ok();
}

// --- Range edits (#700) ------------------------------------------------

#[test]
fn range_delete_attributes_to_the_owning_chapter_and_saves_it() {
    let (mut project, dir) = project_with("range-del", &["alpha\n", "beta\n", "gamma\n"]);
    let mut m = Manuscript::open(&project).unwrap();
    // Buffer: "alpha\nbeta\ngamma\n". Delete "et" from "beta" (bytes 7..9).
    assert!(m.replace_byte_range(7, 9, ""));
    assert_eq!(flow(&m), "alpha\nba\ngamma\n");
    assert!(!m.segments[0].dirty && m.segments[1].dirty && !m.segments[2].dirty);
    assert_eq!(lines_sum(&m), m.buffer.line_count() - 1);

    m.save(&mut project).unwrap();
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "ba\n");
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
    assert_eq!(fs::read_to_string(dir.join("ch2.md")).unwrap(), "gamma\n");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn range_replace_maintains_line_counts_when_lines_change() {
    let (project, dir) = project_with("range-lines", &["a1\na2\n", "b1\n", "c1\n"]);
    let mut m = Manuscript::open(&project).unwrap();
    assert_eq!(m.segments[0].lines, 2);
    // Replace "a1\na2" (bytes 0..5) with a single-line "X" — drops a line from
    // chapter 0, and the seam to chapter 1 must move up with it.
    assert!(m.replace_byte_range(0, 5, "X"));
    assert_eq!(flow(&m), "X\nb1\nc1\n");
    assert_eq!(m.segments[0].lines, 1);
    assert_eq!(m.segments[1].index, 1);
    assert_eq!(lines_sum(&m), m.buffer.line_count() - 1);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn range_delete_is_clamped_at_a_chapter_seam() {
    let (mut project, dir) = project_with("range-seam", &["alpha\n", "beta\n"]);
    let mut m = Manuscript::open(&project).unwrap();
    // Try to delete from inside chapter 0 across the seam into chapter 1
    // (bytes 2..9 spans "pha\nbe"). The clamp trims it to chapter 0's editable
    // span, leaving the terminating newline so the chapters never merge.
    m.buffer.cursor = Cursor { line: 0, col: 2 };
    assert!(m.replace_byte_range(2, 9, ""));
    assert_eq!(flow(&m), "al\nbeta\n"); // "pha" gone, seam and "beta" intact
    assert_eq!(m.segments[1].index, 1);
    assert_eq!(lines_sum(&m), m.buffer.line_count() - 1);

    m.save(&mut project).unwrap();
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "al\n");
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn range_delete_of_a_whole_seam_only_leaves_the_seam_intact() {
    // A delete aimed exactly at the newline that separates two chapters is
    // fully clamped away — it changes nothing and never merges the chapters.
    let (project, dir) = project_with("range-seamonly", &["alpha\n", "beta\n"]);
    let mut m = Manuscript::open(&project).unwrap();
    // Byte 5 is chapter 0's terminating '\n'; deleting [5,6) would merge.
    m.buffer.cursor = Cursor { line: 0, col: 5 };
    assert!(
        !m.replace_byte_range(5, 6, ""),
        "a pure-seam delete is a no-op"
    );
    assert_eq!(flow(&m), "alpha\nbeta\n");
    assert!(!m.dirty());
    assert!(!m.undo(), "the clamped no-op left nothing to undo");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_reverts_a_range_delete_and_restores_the_seams() {
    let (mut project, dir) = project_with("range-undo", &["a1\na2\n", "b1\n", "c1\n"]);
    let mut m = Manuscript::open(&project).unwrap();
    // Delete "a1\n" (bytes 0..3) — chapter 0 drops to one line.
    assert!(m.replace_byte_range(0, 3, ""));
    m.buffer.seal_undo_group();
    assert_eq!(flow(&m), "a2\nb1\nc1\n");
    assert_eq!(m.segments[0].lines, 1);

    // Undo puts the line — and the seam below it — back exactly.
    assert!(m.undo());
    assert_eq!(flow(&m), "a1\na2\nb1\nc1\n");
    assert_eq!(m.segments[0].lines, 2);
    assert_eq!(m.segments[1].index, 1);
    assert!(!m.dirty(), "undo restored the clean pre-edit state");
    assert_eq!(lines_sum(&m), m.buffer.line_count() - 1);

    // Redo re-applies it and re-dirties chapter 0.
    assert!(m.redo());
    assert_eq!(flow(&m), "a2\nb1\nc1\n");
    m.save(&mut project).unwrap();
    assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "a2\n");
    assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "b1\n");
    fs::remove_dir_all(&dir).ok();
}

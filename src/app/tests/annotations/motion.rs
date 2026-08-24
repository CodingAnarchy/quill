//! The next / previous *open* annotation motions (`]a` / `[a`, #1009): stepping
//! over resolved notes, crossing into the next chapter (skipping a chapter whose
//! notes are all resolved), reporting when none remain, working without the
//! review overlay open, and pushing the jumplist so a back-jump returns.

use super::*;
use crate::annotation::Live;
use crate::project::Project;

/// A `Live` note in `file` at `[start, end)` with the given resolved state — the
/// seed for the open-annotation motion tests.
fn note(file: &str, start: usize, end: usize, resolved: bool) -> Live {
    Live {
        chapter: file.into(),
        start,
        end,
        note: "n".into(),
        replies: Vec::new(),
        resolved,
        reviewer: None,
    }
}

#[test]
fn next_open_annotation_steps_only_over_open_notes() {
    // "first chapter\n" — three notes: open @0, resolved @6, open @9. `]a` skips
    // the resolved one.
    let (mut app, _dir) = two_chapter_app("anno-next-open");
    app.annotations = vec![
        note("one.md", 0, 5, false),
        note("one.md", 6, 8, true),
        note("one.md", 9, 12, false),
    ];
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_open_annotation(true);
    assert_eq!(
        app.buffer.cursor_byte(),
        9,
        "skipped the resolved note at 6"
    );
    // Nothing open beyond 9 in this chapter or a later one — reports the end.
    app.jump_to_open_annotation(true);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No next open annotation"),
        "{:?}",
        app.message
    );
}

#[test]
fn previous_open_annotation_walks_back_over_open_notes() {
    let (mut app, _dir) = two_chapter_app("anno-prev-open");
    app.annotations = vec![
        note("one.md", 0, 5, false),
        note("one.md", 6, 8, true),
        note("one.md", 9, 12, false),
    ];
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(false);
    assert_eq!(app.buffer.cursor_byte(), 9, "nearest open note going back");
    app.jump_to_open_annotation(false);
    assert_eq!(
        app.buffer.cursor_byte(),
        0,
        "skipped the resolved note at 6"
    );
}

#[test]
fn next_open_annotation_crosses_into_the_next_chapter() {
    // Chapter 0's only open note is behind the cursor; chapter 1 has an open note.
    let (mut app, _dir) = two_chapter_app("anno-cross-next");
    app.annotations = vec![note("one.md", 0, 5, false)];
    // Chapter 1's notes live in the project's persisted set (an unloaded chapter).
    app.project
        .set_chapter_annotations("two.md", &[note("two.md", 7, 13, false)])
        .unwrap();
    // Past chapter 0's single note.
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(true);
    assert_eq!(app.project.active_index(), 1, "crossed into chapter 1");
    assert_eq!(
        app.buffer.cursor_byte(),
        7,
        "landed on chapter 1's open note"
    );
}

#[test]
fn open_annotation_crossing_skips_a_chapter_with_only_resolved_notes() {
    // Chapter 1 has only a resolved note; the forward crossing skips it. (A three-
    // chapter project so there's a chapter 2 to land in.)
    let dir = app_scratch_dir("anno-skip-resolved");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n\
         [[chapters]]\ntitle = \"Three\"\nfile = \"three.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();
    fs::write(dir.join("three.md"), "third chapter\n").unwrap();
    let mut app = open_test_app(&dir);
    let _tmp = TempProject::new(dir);

    app.annotations = vec![note("one.md", 0, 5, false)];
    app.project
        .set_chapter_annotations("two.md", &[note("two.md", 0, 6, true)])
        .unwrap();
    app.project
        .set_chapter_annotations("three.md", &[note("three.md", 2, 7, false)])
        .unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(true);
    assert_eq!(
        app.project.active_index(),
        2,
        "skipped chapter 1's resolved-only notes"
    );
    assert_eq!(
        app.buffer.cursor_byte(),
        2,
        "landed on chapter 2's open note"
    );
}

#[test]
fn open_annotation_motion_reports_when_none_open_remain() {
    // A single resolved note: nothing open in either direction from anywhere.
    let (mut app, _dir) = two_chapter_app("anno-none-open");
    app.annotations = vec![note("one.md", 3, 8, true)];
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    let before = app.buffer.cursor_byte();
    app.jump_to_open_annotation(true);
    assert_eq!(app.buffer.cursor_byte(), before, "cursor unmoved");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No next open annotation"),
        "{:?}",
        app.message
    );
    app.jump_to_open_annotation(false);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No previous open annotation"),
        "{:?}",
        app.message
    );
}

#[test]
fn open_annotation_motion_works_without_opening_the_review_overlay() {
    let (mut app, _dir) = two_chapter_app("anno-no-overlay");
    app.mode = Mode::Insert;
    app.annotations = vec![note("one.md", 0, 5, false), note("one.md", 8, 12, false)];
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_open_annotation(true);
    assert!(
        matches!(app.mode, Mode::Insert),
        "stayed in the editor — no review overlay"
    );
    assert_eq!(app.buffer.cursor_byte(), 8, "moved to the next open note");
}

#[test]
fn crossing_open_annotation_pushes_the_jumplist_so_back_jump_returns() {
    let (mut app, _dir) = two_chapter_app("anno-jumpback");
    app.annotations = vec![note("one.md", 0, 5, false)];
    app.project
        .set_chapter_annotations("two.md", &[note("two.md", 0, 6, false)])
        .unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(true); // crosses to chapter 1
    assert_eq!(app.project.active_index(), 1, "crossed forward");
    app.jump_back();
    assert_eq!(
        app.project.active_index(),
        0,
        "back-jump returns to chapter 0"
    );
}

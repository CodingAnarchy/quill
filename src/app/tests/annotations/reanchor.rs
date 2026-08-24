//! Re-anchoring before a keystroke-path persist (continuous-view drift).
//!
//! Annotations are buffer-absolute offsets, reconciled with the current
//! buffer/window only by `sync_annotations` on the idle tick. Any persist on the
//! KEYSTROKE path (a reslide, entering/leaving the continuous view, a chapter
//! switch) fires after the buffer/window/text has already moved, so it must
//! re-anchor the live offsets to the current buffer first — otherwise it writes a
//! stale offset to disk. These three tests pin the three ways that used to drift.

use super::*;
use crate::buffer::Cursor;
use crate::project::Project;

/// A five-chapter project (`c0..c4.md`), each a single line, for the
/// window-paging tests: the continuous window holds three chapters, so walking
/// down the manuscript pages it more than once in a single input burst.
fn five_chapter_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    for (i, body) in ["alpha\n", "beta\n", "gamma\n", "delta\n", "epsilon\n"]
        .iter()
        .enumerate()
    {
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
fn a_second_reslide_in_one_burst_persists_the_correct_offset() {
    // #4: in the continuous view, a note on a chapter that pages out of the window
    // during a single input burst must keep its correct on-disk offset. The window
    // holds three chapters; three Downs walk the cursor down far enough to reslide
    // twice. Before the fix, the first reslide left `self.annotations` at the old
    // window's absolute offsets, so the second reslide's persist wrote those stale
    // offsets against the new window's bases — drifting chapter 1's note by len(c0).
    let (mut app, dir) = five_chapter_app("anno-double-reslide");
    app.toggle_continuous();
    assert_eq!(
        app.manuscript.as_ref().unwrap().loaded_chapters(),
        vec![0, 1, 2],
        "the window opens on the first three chapters"
    );

    // Annotate "beta" — the second loaded chapter (c1), buffer bytes [base, base+4).
    let beta_start = app.manuscript.as_ref().unwrap().segment_spans()[1].1;
    app.create_annotation(beta_start, beta_start + 4, "note on beta".into());
    assert_eq!(app.annotations[0].chapter, "c1.md");

    // Three Downs in one burst — no idle tick between them — crossing two reslides.
    for _ in 0..3 {
        app.on_key(key(KeyCode::Down));
    }
    // The window has paged past c1, so the idle re-anchor can't rescue its on-disk
    // offset (it's no longer loaded); the keystroke-path persists had to be right.

    let reopened = Project::open(&dir).unwrap();
    let c1 = reopened.annotations_for("c1.md");
    assert_eq!(c1.len(), 1);
    assert_eq!(
        (c1[0].start, c1[0].end),
        (0, 4),
        "chapter 1's note kept its chapter-local offset across two reslides"
    );
}

#[test]
fn entering_continuous_flushes_the_reanchored_offset() {
    // #11: single view — annotate, type before the note, let the idle tick
    // re-anchor it in memory (but not to disk), then Ctrl-L into the continuous
    // view WITHOUT a manual save. Entering flushes the prose; it must flush the
    // re-anchored annotation offset too, or the continuous view's reload overwrites
    // the correct in-memory offset with the stale on-disk one.
    let (mut app, _dir) = two_chapter_app("anno-enter-continuous-flush");
    // "first chapter\n" — annotate "chapter" (bytes 6..13).
    app.create_annotation(6, 13, "note on chapter".into());

    // Type three chars at the very start; on idle the note re-anchors to 9..16 in
    // memory — but the on-disk offset is still the pre-edit 6..13.
    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (9, 16),
        "re-anchored in memory on idle"
    );

    // Ctrl-L into the continuous view — no manual Ctrl-S first.
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_some(), "entered the continuous view");
    // The continuous view reloads annotations from disk on the next idle tick.
    app.on_idle();

    let a = &app.annotations[0];
    assert_eq!(
        app.edit_buffer().byte_slice(a.start, a.end),
        "chapter",
        "the note still covers its phrase after the continuous reload"
    );
}

#[test]
fn leaving_continuous_persists_the_burst_delta() {
    // #15: in the continuous view, annotate a word, then in the SAME burst (no idle
    // tick) type before it and Ctrl-L back to single view. Leaving is a
    // keystroke-path save; it must fold the burst's pending edit into the offset it
    // flushes, or it writes the pre-edit offset and the note drifts by the delta.
    let dir = app_scratch_dir("anno-leave-continuous-delta");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "alpha beta\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();
    let mut app = open_test_app(&dir);
    let _tmp = TempProject::new(dir.clone());

    app.toggle_continuous();
    // Annotate "beta" in the first loaded chapter (buffer bytes 6..10).
    app.create_annotation(6, 10, "note on beta".into());
    assert_eq!(app.annotations[0].chapter, "one.md");

    // Same burst, no idle: type three chars at the very start, before the note.
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    // Ctrl-L leaves the continuous view straight away.
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_none(), "back in the single-chapter view");

    let reopened = Project::open(&dir).unwrap();
    let c0 = reopened.annotations_for("one.md");
    assert_eq!(c0.len(), 1);
    assert_eq!(
        (c0[0].start, c0[0].end),
        (9, 13),
        "the flushed offset reflects the burst's insert before the note"
    );
}

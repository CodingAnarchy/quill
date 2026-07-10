//! Tests for join-lines (`J`) and reflow-paragraph (`gq`) (#1005): merging the
//! current line with the next with a normalized space, and re-wrapping a paragraph
//! by collapsing its hard breaks — both single undoable edits that never cross a
//! paragraph or scene boundary and never touch a word.

use super::*;
use std::fs;

/// A Vim app in Normal mode over `text`, cursor at byte 0 — a controlled buffer
/// for the join/reflow tests.
fn jr_app(name: &str, text: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    app.buffer = Buffer::from_text(text);
    (app, dir)
}

/// A two-chapter project whose chapter bodies are `first` and `second`, opened in
/// the continuous view with Vim mode on, resting in Normal — a real manuscript
/// (not the `jr_app` `manuscript = None` shim) so the seam-clamp tests exercise
/// the actual chapter concatenation and write-back (#1005 follow-up).
fn jr_continuous_app(name: &str, first: &str, second: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), first).unwrap();
    fs::write(dir.join("two.md"), second).unwrap();

    let mut app = open_test_app(&dir);
    app.vim = true;
    app.mode = Mode::Normal;
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some(), "continuous view is active");
    (app, TempProject::new(dir))
}

/// The words of a buffer, in order, ignoring all whitespace — used to assert that
/// join/reflow change only the whitespace, never a word.
fn words(app: &App) -> Vec<String> {
    app.buffer
        .text()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[test]
fn j_joins_the_next_line_with_a_single_space() {
    let (mut app, _dir) = jr_app("jr-join-basic", "the cat\nsat down\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "the cat sat down\n",
        "J merged the two lines with one space"
    );
}

#[test]
fn j_normalizes_ragged_whitespace_between_the_lines() {
    let (mut app, _dir) = jr_app("jr-join-ws", "the cat   \n   sat down\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "the cat sat down\n",
        "trailing and leading whitespace collapse to one space"
    );
}

#[test]
fn count_j_joins_several_lines() {
    let (mut app, _dir) = jr_app("jr-join-count", "a\nb\nc\nd\n");
    // `3J` folds three lines (a, b, c) into one.
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.buffer.text(), "a b c\nd\n", "3J joined three lines");
}

#[test]
fn j_never_merges_across_a_blank_line() {
    let (mut app, _dir) = jr_app("jr-join-para", "end of one\n\nstart of two\n");
    // Cursor on the last line of the first paragraph; J must not pull the blank
    // line (a paragraph boundary) or the next paragraph up.
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "end of one\n\nstart of two\n",
        "J respects the paragraph boundary (blank line)"
    );
}

#[test]
fn j_never_merges_across_a_scene_break() {
    let (mut app, _dir) = jr_app("jr-join-scene", "last line\n* * *\nnext scene\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "last line\n* * *\nnext scene\n",
        "J never folds a scene break (thematic break) up"
    );
}

#[test]
fn j_is_one_undo_step() {
    let (mut app, _dir) = jr_app("jr-join-undo", "the cat\nsat down\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.buffer.text(), "the cat sat down\n");
    app.on_key(key(KeyCode::Char('u'))); // one undo
    assert_eq!(
        app.buffer.text(),
        "the cat\nsat down\n",
        "a single undo restores the pre-join text"
    );
}

#[test]
fn dot_repeats_a_join() {
    // The dot-repeat cross-check (#1004): `J` flows through the change-dispatch
    // path and mutates via the standard edit primitive, so `.` repeats it with no
    // extra wiring.
    let (mut app, _dir) = jr_app("jr-join-dot", "one\ntwo\nthree\nfour\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.buffer.text(), "one two\nthree\nfour\n", "J joined once");
    // `.` repeats the join at the cursor, folding the next line up again.
    app.on_key(key(KeyCode::Char('.')));
    assert_eq!(
        app.buffer.text(),
        "one two three\nfour\n",
        ". repeated the join"
    );
}

#[test]
fn gq_reflows_collapsing_hard_breaks() {
    // A paragraph broken into short ragged lines re-wraps to the wrap column,
    // collapsing every internal hard break.
    let (mut app, _dir) = jr_app(
        "jr-reflow-collapse",
        "the quick\nbrown fox\njumps over\nthe lazy dog\n",
    );
    app.wrap_width.set(72); // a wide column, so the whole paragraph fits on one line
    let before = words(&app);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "the quick brown fox jumps over the lazy dog\n",
        "gq collapsed the hard breaks into one flowed line"
    );
    assert_eq!(before, words(&app), "reflow changed no word");
}

#[test]
fn gq_rewraps_to_the_configured_column_without_changing_words() {
    let (mut app, _dir) = jr_app(
        "jr-reflow-column",
        "alpha beta gamma delta epsilon zeta eta\n",
    );
    app.wrap_width.set(12); // a narrow column forces re-breaking
    let before = words(&app);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    // Every re-wrapped line stays within the column, and no line is blank.
    for line in app.buffer.text().lines() {
        assert!(!line.is_empty(), "reflow left no blank line");
        assert!(
            unicode_width::UnicodeWidthStr::width(line) <= 12,
            "line {line:?} exceeds the wrap column"
        );
    }
    assert_eq!(
        before,
        words(&app),
        "reflow preserved every word, byte-for-byte and in order"
    );
}

#[test]
fn gq_does_not_reach_across_a_blank_line() {
    let (mut app, _dir) = jr_app(
        "jr-reflow-para",
        "first para line one\nfirst para line two\n\nsecond para\n",
    );
    app.wrap_width.set(72);
    // Cursor in the first paragraph; reflow must only touch it.
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "first para line one first para line two\n\nsecond para\n",
        "reflow stayed inside the paragraph, leaving the blank line and next paragraph"
    );
}

#[test]
fn gq_does_not_reach_across_a_scene_break() {
    let (mut app, _dir) = jr_app(
        "jr-reflow-scene",
        "scene one\nline two\n* * *\nscene two\nline two\n",
    );
    app.wrap_width.set(72);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "scene one line two\n* * *\nscene two\nline two\n",
        "reflow never crosses a scene break"
    );
}

#[test]
fn gq_is_one_undo_step() {
    let (mut app, _dir) = jr_app("jr-reflow-undo", "the quick\nbrown fox\n");
    app.wrap_width.set(72);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(app.buffer.text(), "the quick brown fox\n");
    app.on_key(key(KeyCode::Char('u'))); // one undo
    assert_eq!(
        app.buffer.text(),
        "the quick\nbrown fox\n",
        "a single undo restores the pre-reflow paragraph"
    );
}

// --- Regression tests (#1005 follow-up) ---------------------------------------

#[test]
fn j_on_a_last_line_ending_in_an_em_dash_without_a_newline_does_not_panic() {
    // After the join, the merged line is the buffer's last line, ends in a
    // multibyte em-dash, and has no trailing newline — so `hi == len_bytes()` and
    // probing `hi-1..hi` slices a UTF-8 continuation byte, which ropey panics on
    // before the structural fix. `J` must fold the next line up and keep the
    // (still unterminated) line ending.
    let (mut app, _dir) = jr_app("jr-join-emdash-eof", "first line\nsecond line—");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "first line second line—",
        "J joined without panicking and preserved the missing trailing newline"
    );
}

#[test]
fn gq_on_a_last_paragraph_ending_in_an_em_dash_without_a_newline_does_not_panic() {
    // Same multibyte-final-grapheme/no-trailing-newline hazard on the reflow path.
    let (mut app, _dir) = jr_app("jr-reflow-emdash-eof", "the quick\nbrown fox—");
    app.wrap_width.set(72);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "the quick brown fox—",
        "gq reflowed without panicking and preserved the missing trailing newline"
    );
}

#[test]
fn j_in_continuous_view_stops_at_the_chapter_seam_and_does_not_duplicate_text() {
    // Chapters are concatenated `foo\nbar\n` with a single seam and no blank line.
    // Before the fix, `J` on the last paragraph of chapter one scanned across the
    // seam, folding chapter two's line into the replacement string; the range edit
    // then clamped to chapter one, duplicating that text (`foo\n\nbar\n` → wrong).
    // With the seam clamp, `J` has nothing below to join within the chapter, so the
    // buffer is untouched and neither chapter's file changes.
    let (mut app, dir) = jr_continuous_app("jr-cont-join-seam", "foo\n", "bar\n");
    assert_eq!(
        app.edit_buffer().text(),
        "foo\nbar\n",
        "the two chapters concatenate"
    );
    app.on_key(key(KeyCode::Char('J'))); // cursor on chapter one's only line
    assert_eq!(
        app.edit_buffer().text(),
        "foo\nbar\n",
        "J at the chapter seam did nothing — no fold across it, no duplication"
    );
    // Leaving persists the (unchanged) chapters to their own files, each intact.
    app.on_key(ctrl('l'));
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), "foo\n");
    assert_eq!(fs::read_to_string(dir.join("two.md")).unwrap(), "bar\n");
}

#[test]
fn gq_in_continuous_view_stops_at_the_chapter_seam() {
    // Reflow on chapter one's last paragraph must not gather chapter two's first
    // line across the seam. Chapter one is a two-line paragraph; reflow flows it to
    // one line and leaves chapter two untouched.
    let (mut app, dir) = jr_continuous_app(
        "jr-cont-reflow-seam",
        "the quick\nbrown fox\n",
        "the lazy dog\n",
    );
    app.wrap_width.set(72);
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.edit_buffer().text(),
        "the quick brown fox\nthe lazy dog\n",
        "reflow stayed inside chapter one, never pulling chapter two across the seam"
    );
    app.on_key(ctrl('l'));
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "the quick brown fox\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "the lazy dog\n"
    );
}

#[test]
fn j_does_not_fold_a_heading_into_the_prose_below_it() {
    // `## Scene` sits directly above prose with no blank line. `J` on the heading
    // must treat it as a boundary and not fold the prose up into `## Scene Text`.
    let (mut app, _dir) = jr_app("jr-join-heading", "## Scene\nText follows\n");
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(
        app.buffer.text(),
        "## Scene\nText follows\n",
        "J left the heading intact — a heading is a paragraph boundary"
    );
}

#[test]
fn gq_leaves_a_heading_above_the_prose_intact() {
    // The cursor is on the prose directly under a heading (no blank line). Reflow
    // must re-wrap only the prose and never fold the heading marker in.
    let (mut app, _dir) = jr_app(
        "jr-reflow-heading",
        "## Scene\nthe quick brown\nfox jumps\n",
    );
    app.wrap_width.set(72);
    // Move the cursor onto the prose (line 1), off the heading.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('g')));
    app.on_key(key(KeyCode::Char('q')));
    assert_eq!(
        app.buffer.text(),
        "## Scene\nthe quick brown fox jumps\n",
        "reflow re-wrapped the prose and left the heading untouched"
    );
}

//! The edit path: incremental syntax highlighting through edits (the InputEdit
//! bookkeeping in edit_insert/edit_remove), undo/redo grouping (#145), soft-wrap
//! navigation (#143), and the modeless editing keys (#641). Includes the
//! keystroke/reparse perf guards that keep typing decoupled from document size.

use super::*;
use std::fmt::Write;
use std::time::{Duration, Instant};

// The syntax highlights must stay correct through incremental edits — this
// exercises the InputEdit bookkeeping in edit_insert/edit_remove.
use crate::syntax::HighlightKind;

fn kinds(buf: &Buffer) -> Vec<HighlightKind> {
    buf.syntax()
        .highlights_in(0, usize::MAX)
        .map(|h| h.kind)
        .collect()
}

#[test]
fn typing_a_heading_updates_highlights_incrementally() {
    let mut buf = Buffer::empty();
    // A heading is recognized once the line is terminated, so press Enter
    // after it (as an author would before writing the body).
    for c in "# Title\n".chars() {
        buf.insert_char(c);
    }
    buf.refresh_syntax(); // edits are deferred; the event loop reparses on idle
    assert!(kinds(&buf).contains(&HighlightKind::Heading));
}

// Opt-in performance guards (run with `cargo test --release -- --ignored
// --nocapture`). A buffer only ever holds the chapter(s) being edited — one
// for the single-chapter view, at most ~3 for the continuous view — never
// the whole novel.

#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn keystroke_is_decoupled_from_document_size() {
    // A keystroke only *records* the edit; the reparse is deferred. So it
    // must be cheap regardless of buffer size. Stress it far past the real
    // ~3-chapter cap to prove the cost is in the edit, not the document.
    let mut text = String::new();
    for ch in 0..150 {
        write!(text, "# Chapter {ch}\n\n").unwrap();
        for _ in 0..50 {
            text.push_str("The night was *dark* and the road was **long** indeed.\n");
        }
        text.push('\n');
    }
    let words = text.split_whitespace().count();

    let path = temp_path("perf-big.md");
    write(&path, &text);
    let mut buf = Buffer::from_path(&path).unwrap();
    buf.cursor = Cursor {
        line: buf.line_count() / 2,
        col: 0,
    };

    let t = Instant::now();
    buf.insert_char('X'); // record-only, no parse
    let dt = t.elapsed();

    println!("keystroke(record-only) on {words} words = {dt:?}");
    assert!(dt.as_millis() < 4, "keystroke took {dt:?}");

    // The live word count is maintained incrementally (#868), so reading it
    // after a keystroke must also stay decoupled from document size — no
    // whole-document recount or `String` allocation per keystroke. Warm the
    // cache once (the one-time cold rebuild), then time a keystroke *plus* the
    // status-bar read that follows it.
    buf.word_count(); // warm the cache (one-time cold rebuild)
    let t = Instant::now();
    buf.insert_char('Y');
    let live = buf.word_count();
    let dt = t.elapsed();

    println!("keystroke + live word count on {words} words = {dt:?}");
    assert!(dt.as_millis() < 4, "keystroke + count took {dt:?}");
    // And the incrementally-maintained count is byte-for-byte the same the
    // old whole-document recount would have produced.
    assert_eq!(live, count_words(&buf.text()), "incremental count diverged");
    fs::remove_file(&path).ok();
}

#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn reparse_is_fast_at_window_size() {
    // Realistic worst case: ~3 long chapters live at once (continuous view).
    let mut text = String::new();
    for ch in 0..3 {
        write!(text, "# Chapter {ch}\n\n").unwrap();
        for _ in 0..150 {
            text.push_str("The night was *dark* and the road was **long** indeed.\n");
        }
        text.push('\n');
    }
    let words = text.split_whitespace().count();

    let path = temp_path("perf-window.md");
    write(&path, &text);
    let mut buf = Buffer::from_path(&path).unwrap();
    buf.cursor = Cursor {
        line: buf.line_count() / 2,
        col: 0,
    };

    // Best-of-N: each reparse is genuinely ~8ms, well under a frame, but a
    // single sample is dominated by one-off noise (scheduling, CPU ramp,
    // first-call allocation) that occasionally tips it over 16ms. Noise only
    // ever *adds* time, so the minimum across runs reflects the true compute
    // cost. Each run needs a fresh edit — `refresh_syntax` is a no-op when the
    // syntax isn't dirty.
    let mut best = Duration::MAX;
    for _ in 0..5 {
        buf.insert_char('X');
        let t = Instant::now();
        buf.refresh_syntax();
        best = best.min(t.elapsed());
    }

    println!("reparse on {words} words = {best:?} (best of 5)");
    assert!(best.as_millis() < 16, "reparse took {best:?}");
    fs::remove_file(&path).ok();
}

#[test]
fn editing_mid_document_keeps_highlights_correct() {
    // Open a multi-line doc, then add emphasis in the middle and check the
    // parse tracked the edit (heading stays, emphasis appears).
    let path = temp_path("mid-edit.md");
    write(&path, "# Chapter\n\nplain line\n");
    let mut buf = Buffer::from_path(&path).unwrap();

    // Move into the body line and wrap a word in asterisks.
    buf.cursor = Cursor { line: 2, col: 0 };
    buf.insert_char('*');
    buf.cursor = Cursor { line: 2, col: 6 }; // after "*plain"
    buf.insert_char('*');
    buf.refresh_syntax();

    let ks = kinds(&buf);
    assert!(ks.contains(&HighlightKind::Heading));
    assert!(ks.contains(&HighlightKind::Emphasis));
    fs::remove_file(&path).ok();
}

// --- Undo / redo (#145) ------------------------------------------------

#[test]
fn undo_reverts_a_typing_burst_and_redo_reapplies_it() {
    let mut buf = Buffer::empty();
    for c in "hello".chars() {
        buf.insert_char(c);
    }
    assert_eq!(buf.text(), "hello");
    // A burst with no idle in between is one group.
    assert!(buf.undo());
    assert_eq!(buf.text(), "");
    assert!(buf.redo());
    assert_eq!(buf.text(), "hello");
}

#[test]
fn sealing_starts_a_new_undo_group() {
    let mut buf = Buffer::empty();
    for c in "foo".chars() {
        buf.insert_char(c);
    }
    buf.seal_undo_group(); // idle boundary
    for c in "bar".chars() {
        buf.insert_char(c);
    }
    assert_eq!(buf.text(), "foobar");
    assert!(buf.undo());
    assert_eq!(buf.text(), "foo", "only the second burst is undone");
    assert!(buf.undo());
    assert_eq!(buf.text(), "");
    assert!(!buf.undo(), "nothing left to undo");
}

#[test]
fn a_new_edit_clears_the_redo_stack() {
    let mut buf = Buffer::empty();
    for c in "ab".chars() {
        buf.insert_char(c);
    }
    buf.undo();
    assert_eq!(buf.text(), "");
    buf.insert_char('z'); // a fresh edit invalidates the redo
    assert!(!buf.redo());
    assert_eq!(buf.text(), "z");
}

#[test]
fn undo_restores_the_cursor_and_reparses() {
    let mut buf = Buffer::empty();
    for c in "# Title\n".chars() {
        buf.insert_char(c);
    }
    buf.refresh_syntax();
    let cursor_before = buf.cursor; // line 1, col 0 — just under the heading
    buf.seal_undo_group();
    for c in "body".chars() {
        buf.insert_char(c);
    }
    assert!(buf.undo());
    assert_eq!(buf.text(), "# Title\n");
    assert_eq!(
        buf.cursor, cursor_before,
        "cursor returns to where the edit began"
    );
    // Highlights are rebuilt immediately on undo, not deferred to idle.
    assert!(kinds(&buf).contains(&HighlightKind::Heading));
}

#[test]
fn undo_and_redo_on_a_fresh_buffer_are_noops() {
    let mut buf = Buffer::empty();
    assert!(!buf.undo());
    assert!(!buf.redo());
}

// --- Soft-wrap navigation (#143) ---------------------------------------
// "aaaa bbbb cccc dddd" at width 10 wraps to "aaaa bbbb " | "cccc dddd".

#[test]
fn move_down_steps_through_wrapped_visual_rows() {
    let mut buf = Buffer::from_text("aaaa bbbb cccc dddd");
    buf.cursor = Cursor { line: 0, col: 0 };
    buf.move_down(10); // down one visual row — still the same logical line
    assert_eq!(buf.cursor.line, 0, "stayed on the wrapped logical line");
    assert_eq!(
        buf.cursor.col, 10,
        "landed at the start of the second visual row"
    );
    buf.move_up(10); // back up to the first visual row at the same column
    assert_eq!(buf.cursor, Cursor { line: 0, col: 0 });
}

#[test]
fn home_and_end_act_on_the_visual_row() {
    let mut buf = Buffer::from_text("aaaa bbbb cccc dddd");
    buf.cursor = Cursor { line: 0, col: 13 }; // inside the second visual row
    buf.move_home(10);
    assert_eq!(buf.cursor.col, 10, "Home goes to the visual row's start");
    buf.move_end(10);
    assert_eq!(buf.cursor.col, 19, "End goes to the visual row's end");
}

#[test]
fn vertical_move_without_wrapping_still_works() {
    let mut buf = Buffer::from_text("first\nsecond\n");
    buf.cursor = Cursor { line: 0, col: 3 };
    buf.move_down(80); // wide enough that nothing wraps
    assert_eq!(
        buf.cursor,
        Cursor { line: 1, col: 3 },
        "moves to the next logical line"
    );
}

#[test]
fn backspace_participates_in_undo() {
    let mut buf = Buffer::empty();
    for c in "draft".chars() {
        buf.insert_char(c);
    }
    buf.seal_undo_group();
    buf.backspace();
    buf.backspace();
    assert_eq!(buf.text(), "dra");
    assert!(buf.undo());
    assert_eq!(buf.text(), "draft", "the deletions undo as one group");
}

// --- Modeless editing keys (#641) --------------------------------------

#[test]
fn delete_forward_removes_the_char_ahead_of_the_cursor() {
    let mut buf = Buffer::from_text("abcd");
    buf.cursor = Cursor { line: 0, col: 1 };
    let joined = buf.delete_forward();
    assert!(!joined);
    assert_eq!(buf.text(), "acd", "the char ahead of the cursor is removed");
    assert_eq!(
        buf.cursor,
        Cursor { line: 0, col: 1 },
        "the cursor stays put"
    );
}

#[test]
fn delete_forward_at_line_end_joins_the_next_line_up() {
    let mut buf = Buffer::from_text("one\ntwo");
    buf.cursor = Cursor { line: 0, col: 3 }; // just past "one"
    let joined = buf.delete_forward();
    assert!(joined, "removing the newline reports a line join");
    assert_eq!(buf.text(), "onetwo");
    assert_eq!(buf.cursor, Cursor { line: 0, col: 3 });
}

#[test]
fn delete_forward_at_document_end_is_a_no_op() {
    let mut buf = Buffer::from_text("end");
    buf.cursor = Cursor { line: 0, col: 3 };
    assert!(!buf.delete_forward());
    assert_eq!(buf.text(), "end");
}

#[test]
fn delete_word_back_removes_the_previous_word() {
    let mut buf = Buffer::from_text("the quick brown");
    buf.move_line_end(); // cursor after "brown"
    let removed = buf.delete_word_back();
    assert_eq!(removed, 0);
    assert_eq!(buf.text(), "the quick ", "the last word is deleted");
    assert_eq!(buf.cursor.col, "the quick ".chars().count());
}

#[test]
fn delete_word_back_at_document_start_is_a_no_op() {
    let mut buf = Buffer::from_text("word");
    buf.cursor = Cursor { line: 0, col: 0 };
    assert_eq!(buf.delete_word_back(), 0);
    assert_eq!(buf.text(), "word");
}

#[test]
fn word_back_target_is_a_pure_query() {
    let mut buf = Buffer::from_text("the quick brown");
    buf.move_line_end();
    let before = buf.cursor;
    let target = buf.word_back_target();
    assert_eq!(target.col, 10, "start of \"brown\"");
    assert_eq!(buf.cursor, before, "the query leaves the cursor untouched");
}

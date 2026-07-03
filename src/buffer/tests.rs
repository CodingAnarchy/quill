//! Tests for the `src/buffer/mod.rs` module, split out to keep the module under the
//! ~1,000-line convention (#404). Pure move — no behavior change.

use super::*;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A unique-per-process path for a test file, inside a fresh scratch dir (#428).
/// `name` keeps its extension (some tests assert `.md`-vs-other handling), so it
/// names both the dir and the file within it.
fn temp_path(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("buffer", name).join(name)
}

fn write(path: &Path, bytes: &str) {
    fs::write(path, bytes).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

// --- Data-path resilience (#264) ---------------------------------------

#[test]
fn from_path_surfaces_a_read_error_rather_than_panicking() {
    // A path that exists but is a directory: `read_to_string` fails with a
    // non-NotFound error, which must surface as `Err`, not a panic and not a
    // silently-empty buffer (which a later save would write over the dir).
    // The name carries an extension so `with_default_extension` leaves it be
    // and the resolved path really is the directory.
    let dir = temp_path("from-path-is-a-dir.md");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let result = Buffer::from_path(&dir);
    assert!(
        result.is_err(),
        "opening a directory as a file should error"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_save_keeps_the_buffer_dirty_and_re_savable() {
    // Point the buffer at an existing directory so the write fails; the
    // unsaved content must remain (dirty stays true), so a later save to a
    // good path still works.
    let dir = temp_path("save-onto-a-dir");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let mut b = Buffer::from_text("precious unsaved words");
    b.path = Some(dir.clone());
    b.dirty = true;
    assert!(b.save().is_err(), "writing onto a directory should error");
    assert!(b.dirty, "a failed save must leave the buffer dirty");

    // Re-savable to a real file, with the content intact.
    let good = temp_path("save-onto-a-dir-good.md");
    b.path = Some(good.clone());
    b.save()
        .expect("the buffer should still save to a valid path");
    assert_eq!(read(&good), "precious unsaved words\n");

    fs::remove_dir_all(&dir).ok();
    fs::remove_file(&good).ok();
}

// --- Vim motions (#202) ------------------------------------------------

#[test]
fn word_motions_step_by_word_and_punctuation() {
    let mut b = Buffer::from_text("the quick, brown fox");
    assert_eq!(b.cursor.col, 0);
    b.move_word_forward(); // -> "quick"
    assert_eq!(b.cursor.col, 4);
    b.move_word_forward(); // -> ","  (punctuation is its own word)
    assert_eq!(b.cursor.col, 9);
    b.move_word_forward(); // -> "brown"
    assert_eq!(b.cursor.col, 11);
    b.move_word_end(); // end of "brown"
    assert_eq!(b.cursor.col, 15);
    b.move_word_backward(); // back to start of "brown"
    assert_eq!(b.cursor.col, 11);
}

#[test]
fn word_forward_crosses_line_boundaries() {
    let mut b = Buffer::from_text("end\nnext line");
    b.cursor = Cursor { line: 0, col: 1 };
    b.move_word_forward(); // from inside "end" to "next" on the following line
    assert_eq!((b.cursor.line, b.cursor.col), (1, 0));
}

#[test]
fn line_and_document_motions() {
    let mut b = Buffer::from_text("  indented\nmiddle\nlast line\n");
    b.cursor = Cursor { line: 0, col: 6 };
    b.move_line_start();
    assert_eq!(b.cursor.col, 0);
    b.move_first_non_blank();
    assert_eq!(b.cursor.col, 2, "^ skips leading blanks");
    b.move_line_end();
    assert_eq!(b.cursor.col, b.line_len());

    b.move_to_line(1);
    assert_eq!(b.cursor.line, 1);
    // G targets the last line with content, not the empty trailing line.
    b.move_to_line(b.last_line_index());
    assert_eq!(
        b.cursor.line, 2,
        "G skips the trailing newline's empty line"
    );
}

#[test]
fn find_char_lands_on_or_before_the_target() {
    // cols:                 0    5    10
    let mut b = Buffer::from_text("alpha-beta-gamma");
    b.find_char('-', FindKind::FindFwd, 1);
    assert_eq!(b.cursor.col, 5, "f lands on the first '-'");
    b.find_char('-', FindKind::FindFwd, 1);
    assert_eq!(b.cursor.col, 10, "f again lands on the next '-'");
    b.find_char('a', FindKind::FindBack, 1);
    assert_eq!(b.cursor.col, 9, "F lands on the previous 'a'");
    b.find_char('m', FindKind::TillFwd, 1);
    assert_eq!(b.cursor.col, 12, "t lands just before the next 'm'");

    // Count selects the nth occurrence; a miss leaves the cursor put.
    b.cursor = Cursor { line: 0, col: 0 };
    b.find_char('-', FindKind::FindFwd, 2);
    assert_eq!(b.cursor.col, 10, "2f- reaches the second '-'");
    b.find_char('z', FindKind::FindFwd, 1);
    assert_eq!(b.cursor.col, 10, "a missing target doesn't move");
}

#[test]
fn text_objects_cover_words_quotes_and_pairs() {
    // Word object: inner is the word, around takes the trailing space.
    let mut b = Buffer::from_text("the quick fox");
    b.cursor = Cursor { line: 0, col: 5 }; // inside "quick"
    assert_eq!(b.text_object(TextObject::Word, false), Some((4, 9)));
    assert_eq!(
        b.text_object(TextObject::Word, true),
        Some((4, 10)),
        "aw takes the trailing space"
    );

    // Quote object on the line.
    let mut b = Buffer::from_text("say \"hello\" now");
    b.cursor = Cursor { line: 0, col: 6 }; // inside the quotes
    assert_eq!(
        b.text_object(TextObject::Quote('"'), false),
        Some((5, 10)),
        "i\" is the inside"
    );
    assert_eq!(
        b.text_object(TextObject::Quote('"'), true),
        Some((4, 11)),
        "a\" includes the quotes"
    );

    // Bracket pair with nesting.
    let mut b = Buffer::from_text("f(a, g(b), c)");
    b.cursor = Cursor { line: 0, col: 3 }; // inside the outer parens
    assert_eq!(
        b.text_object(TextObject::Pair('(', ')'), false),
        Some((2, 12)),
        "i( spans to the matching close"
    );
    assert_eq!(
        b.text_object(TextObject::Pair('(', ')'), true),
        Some((1, 13)),
        "a( includes the parens"
    );
}

#[test]
fn motions_do_not_panic_at_boundaries() {
    let mut b = Buffer::from_text("word");
    b.move_word_backward(); // already at start
    assert_eq!(b.cursor.col, 0);
    b.cursor = Cursor { line: 0, col: 4 };
    b.move_word_forward(); // already at end
    b.move_word_end();
    assert_eq!(b.cursor.line, 0);
}

#[test]
fn word_motions_are_correct_past_the_scan_window() {
    // Runs longer than one class window (#660) must still resolve correctly: the
    // window doubles and rescans rather than stopping at its edge. Build a word
    // and a whitespace run each far larger than CLASS_WINDOW (4 KiB).
    let big = 10_000;
    let text = format!("{}{}{}", "a".repeat(big), " ".repeat(big), "b".repeat(3));
    let mut b = Buffer::from_text(&text);

    // From the head of the giant word, `w` crosses the whole word *and* the whole
    // whitespace run to reach "b" — a scan far past the initial window.
    b.cursor = Cursor { line: 0, col: 0 };
    b.move_word_forward();
    assert_eq!(b.cursor_byte(), big * 2, "w reaches the far word start");

    // `e` from the start lands on the last char of the giant word.
    b.cursor = Cursor { line: 0, col: 0 };
    b.move_word_end();
    assert_eq!(b.cursor_byte(), big - 1, "e reaches the far word end");

    // `b` from the far word walks back across the whitespace and the giant word.
    b.cursor = Cursor {
        line: 0,
        col: big * 2,
    };
    b.move_word_backward();
    assert_eq!(b.cursor_byte(), 0, "b reaches the far word start");

    // `iw` on the giant word spans its full extent, not just the window.
    b.cursor = Cursor {
        line: 0,
        col: big / 2,
    };
    assert_eq!(b.text_object(TextObject::Word, false), Some((0, big)));
}

#[test]
fn bracket_object_spans_a_large_gap() {
    // A pair whose delimiters straddle a large body must still match — the scan
    // streams rope chars outward from the cursor rather than copying the buffer.
    let inner = "x".repeat(10_000);
    let text = format!("({inner})");
    let b_len = text.len();
    let mut b = Buffer::from_text(&text);
    b.cursor = Cursor {
        line: 0,
        col: 5_000,
    };
    assert_eq!(
        b.text_object(TextObject::Pair('(', ')'), false),
        Some((1, b_len - 1)),
        "i( spans the whole body"
    );
    assert_eq!(
        b.text_object(TextObject::Pair('(', ')'), true),
        Some((0, b_len)),
        "a( includes the parens"
    );
}

#[test]
fn round_trip_is_byte_identical_with_trailing_newline() {
    let path = temp_path("roundtrip.md");
    let original = "# Chapter One\n\nThe night was *dark* and **stormy**.\n";
    write(&path, original);

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), original);
    fs::remove_file(&path).ok();
}

#[test]
fn front_matter_is_kept_off_the_prose_and_re_attached_on_save() {
    let path = temp_path("frontmatter.md");
    let original = "---\nstatus: draft\npov: Mara\n---\n# Chapter One\n\nThe body.\n";
    write(&path, original);

    let mut buf = Buffer::from_path(&path).unwrap();
    // The prose area (rope) holds only the body — no raw YAML.
    assert_eq!(buf.line(0), "# Chapter One");
    assert!(buf.front_matter().is_some());
    // Word count covers the body, not the front matter keys.
    assert_eq!(
        buf.word_count(),
        count_words("# Chapter One\n\nThe body.\n")
    );

    // Saving re-attaches the front matter untouched: byte-for-byte round-trip.
    buf.save().unwrap();
    assert_eq!(read(&path), original);
    fs::remove_file(&path).ok();
}

#[test]
fn save_adds_missing_trailing_newline() {
    let path = temp_path("no-newline.md");
    write(&path, "No newline at end");

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), "No newline at end\n");
    fs::remove_file(&path).ok();
}

#[test]
fn save_collapses_multiple_trailing_newlines_to_one() {
    let path = temp_path("many-newlines.md");
    write(&path, "Body\n\n\n\n");

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), "Body\n");
    fs::remove_file(&path).ok();
}

#[test]
fn empty_document_saves_empty_file() {
    let path = temp_path("empty.md");
    write(&path, "");

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), "");
    fs::remove_file(&path).ok();
}

#[test]
fn non_ascii_round_trips_exactly() {
    let path = temp_path("unicode.md");
    let original = "Café — résumé, naïve façade. 日本語のテキスト. 🚀✍️🌙\n";
    write(&path, original);

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), original);
    fs::remove_file(&path).ok();
}

#[test]
fn extensionless_path_defaults_to_md() {
    let path = temp_path("manuscript"); // no extension
    let buf = Buffer::from_path(&path).unwrap();

    let expected = path.with_extension("md");
    assert_eq!(buf.path.as_deref(), Some(expected.as_path()));
}

#[test]
fn explicit_extension_is_respected() {
    let path = temp_path("notes.txt");
    let buf = Buffer::from_path(&path).unwrap();

    assert_eq!(buf.path.as_deref(), Some(path.as_path()));
}

#[test]
fn count_words_uses_unicode_segmentation() {
    assert_eq!(count_words(""), 0);
    assert_eq!(count_words("   \n  "), 0);
    assert_eq!(count_words("one two three"), 3);
    assert_eq!(count_words("hello, world!"), 2);
    assert_eq!(count_words("it's a tidy plan"), 4); // contraction stays one word
                                                    // Unicode segmentation splits hyphenated compounds.
    assert_eq!(count_words("well-made"), 2);
}

#[test]
fn word_count_is_correct_across_rope_chunk_boundaries() {
    // A document large enough to span multiple rope chunks. A word split
    // across a chunk boundary must not be counted twice.
    let path = temp_path("wordcount.md");
    write(&path, &"word ".repeat(5000));

    let buf = Buffer::from_path(&path).unwrap();
    assert_eq!(buf.word_count(), 5000);

    fs::remove_file(&path).ok();
}

#[test]
fn word_count_cache_updates_on_edit_and_undo() {
    let mut buf = Buffer::from_text("one two");
    assert_eq!(buf.word_count(), 2);

    // Append a third word; the cached count reflects the edit (#151).
    buf.cursor = Cursor { line: 0, col: 7 };
    for c in " three".chars() {
        buf.insert_char(c);
    }
    assert_eq!(buf.word_count(), 3);

    // Delete it back; the count follows.
    buf.seal_undo_group();
    for _ in 0..6 {
        buf.backspace();
    }
    assert_eq!(buf.word_count(), 2);

    // Undo restores both the text and the count.
    buf.undo();
    assert_eq!(buf.word_count(), 3);
}

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
#[ignore = "release-only perf guard; run via cargo perf"]
fn word_motion_is_decoupled_from_document_size() {
    // A word motion scans a bounded window around the cursor (#660), not the
    // whole rope, so its cost can't grow with the document. Before the fix each
    // call copied the entire buffer to a `String`, so `w` at mid-document scaled
    // with the document; now the same motion on a buffer 100× larger costs
    // essentially the same. Compare a window-sized buffer against one far past
    // the ~3-chapter cap; best-of-N to shed one-off noise, as elsewhere.
    fn best_word_forward(text: &str) -> Duration {
        let mut best = Duration::MAX;
        for _ in 0..50 {
            let mut b = Buffer::from_text(text);
            b.cursor = Cursor {
                line: b.line_count() / 2,
                col: 0,
            };
            let t = Instant::now();
            b.move_word_forward();
            best = best.min(t.elapsed());
        }
        best
    }

    // Realistic prose (short lines, so line-length isn't the variable): a
    // window-sized buffer vs. one ~100× larger.
    let para = "The night was dark and the road was long indeed.\n".repeat(50);
    let small = para.clone(); // ~2.5 KB, ~one screen
    let big = para.repeat(100); // ~250 KB, far past the ~3-chapter cap

    let small_dt = best_word_forward(&small);
    let big_dt = best_word_forward(&big);
    let ratio = big_dt.as_secs_f64() / small_dt.as_secs_f64().max(f64::MIN_POSITIVE);
    println!(
        "w: small={small_dt:?} big={big_dt:?} ({} bytes, {ratio:.1}×)",
        big.len()
    );

    // The window is fixed-size, so the 100×-larger buffer's motion stays within a
    // small constant of the small one — nowhere near the 100× the byte counts
    // differ by. A copy-the-whole-rope regression would blow past this.
    assert!(
        ratio < 5.0,
        "word motion scaled with document size (small={small_dt:?}, big={big_dt:?}, {ratio:.1}×)"
    );
}

#[test]
fn grapheme_after_matches_a_full_document_scan() {
    // The windowed `grapheme_after` (#693) must be byte-identical to the old
    // whole-rope scan at every offset — including inside a multi-byte grapheme, a
    // grapheme cluster (flag emoji = two code points), the last grapheme, and past
    // the end (clamped). Compare against a reference computed over the whole text.
    fn reference(text: &str, byte: usize) -> usize {
        text[byte.min(text.len())..]
            .grapheme_indices(true)
            .nth(1)
            .map_or(text.len(), |(b, _)| byte + b)
    }

    let text = "aé🎉👍🏽🇺🇸z\ncafé\n"; // ASCII, accented char, emoji, skin-tone, flag
    let buf = Buffer::from_text(text);
    for byte in 0..=text.len() {
        // Only compare on grapheme boundaries — the callers only pass those, and a
        // mid-grapheme byte isn't a meaningful input for either implementation.
        if text.is_char_boundary(byte) {
            assert_eq!(
                buf.grapheme_after(byte),
                reference(text, byte),
                "grapheme_after mismatch at byte {byte}"
            );
        }
    }
}

#[test]
fn grapheme_after_deletes_a_multibyte_grapheme_at_the_cursor() {
    // The `x`/inclusive-delete path uses `grapheme_after` to cover the grapheme the
    // cursor sits on; a multi-byte cluster must be removed whole, not split.
    let mut buf = Buffer::from_text("é🎉z\n");
    // Delete the leading "é" (2 bytes): its boundary is byte 2.
    let hi = buf.grapheme_after(0);
    assert_eq!(hi, 2);
    buf.replace_byte_range(0, hi, "");
    assert_eq!(buf.text(), "🎉z\n");
    // Now delete the "🎉" (4 bytes) from the front.
    let hi = buf.grapheme_after(0);
    assert_eq!(hi, 4);
    buf.replace_byte_range(0, hi, "");
    assert_eq!(buf.text(), "z\n");
}

#[test]
fn grapheme_after_widens_past_a_window_straddling_grapheme() {
    // A pathological grapheme longer than the initial window must still resolve to
    // the correct next boundary — the window doubles and rescans rather than
    // stopping at its edge (#693). A base char plus a long run of combining marks
    // (U+0301, combining acute) is a single grapheme cluster; make it far longer
    // than `GRAPHEME_WINDOW`, then a trailing "z" so there is a real next boundary.
    let mut cluster = String::from("a");
    for _ in 0..2000 {
        cluster.push('\u{0301}');
    }
    assert!(
        cluster.len() > GRAPHEME_WINDOW,
        "cluster must exceed one window"
    );
    let text = format!("{cluster}z\n");
    let buf = Buffer::from_text(&text);
    // The whole base+combining run is one grapheme, so the boundary after byte 0 is
    // its end — the windowed scan must widen past it, not stop at the window edge.
    assert_eq!(buf.grapheme_after(0), cluster.len());
}

#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn grapheme_after_delete_is_decoupled_from_document_size() {
    // `x` (and every inclusive/visual delete) resolves the grapheme under the
    // cursor via `grapheme_after`, which now scans a bounded window (#693) instead
    // of copying the whole rope. So a delete at mid-document can't scale with the
    // document: the same `x` on a buffer 100× larger costs essentially the same.
    // Before the fix each call materialized the entire buffer to a `String`.
    fn best_delete(text: &str) -> Duration {
        let mut best = Duration::MAX;
        for _ in 0..50 {
            let mut b = Buffer::from_text(text);
            let byte = b.len_bytes() / 2;
            b.cursor = b.cursor_at_byte(byte);
            let t = Instant::now();
            // Mirror the `x` path: find the grapheme after the cursor, delete it.
            let hi = b.grapheme_after(b.cursor_byte());
            b.replace_byte_range(b.cursor_byte(), hi, "");
            best = best.min(t.elapsed());
        }
        best
    }

    let para = "The night was dark and the road was long indeed.\n".repeat(50);
    let small = para.clone(); // ~2.5 KB, ~one screen
    let big = para.repeat(100); // ~250 KB, far past the ~3-chapter cap

    let small_dt = best_delete(&small);
    let big_dt = best_delete(&big);
    let ratio = big_dt.as_secs_f64() / small_dt.as_secs_f64().max(f64::MIN_POSITIVE);
    println!(
        "x-delete: small={small_dt:?} big={big_dt:?} ({} bytes, {ratio:.1}×)",
        big.len()
    );

    // The window is fixed-size, so the 100×-larger buffer's delete stays within a
    // small constant of the small one — nowhere near the 100× the byte counts
    // differ by. A copy-the-whole-rope regression would blow past this.
    assert!(
        ratio < 5.0,
        "grapheme_after delete scaled with document size (small={small_dt:?}, big={big_dt:?}, {ratio:.1}×)"
    );
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

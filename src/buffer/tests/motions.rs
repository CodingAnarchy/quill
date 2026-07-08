//! Vim motions (#202), find-char, text objects, paragraph/sentence motions and
//! objects (#821), and the boundary/scan-window cases that prove motions widen
//! their scan rather than stopping at the window edge (#660). Includes the
//! word-motion perf guard.

use super::*;
use std::time::{Duration, Instant};

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

// --- Paragraph and sentence motions / objects (#821) -------------------

#[test]
fn paragraph_motions_jump_by_blank_line() {
    // Two paragraphs separated by a blank line, then a trailing paragraph.
    let text = "one two\nthree four\n\nfive six\n\nseven\n";
    // lines:     0        1          2   3          4   5       (6 = "")
    let mut b = Buffer::from_text(text);
    b.cursor = Cursor { line: 0, col: 2 };
    b.move_paragraph_forward();
    assert_eq!(
        b.cursor.line, 2,
        "brace-close lands on the blank line after"
    );
    b.move_paragraph_forward();
    assert_eq!(b.cursor.line, 4, "brace-close reaches the next blank line");
    b.move_paragraph_backward();
    assert_eq!(
        b.cursor.line, 2,
        "brace-open walks back to the previous blank"
    );
    b.move_paragraph_backward();
    assert_eq!(b.cursor.line, 0, "brace-open reaches the document start");
}

#[test]
fn paragraph_object_covers_the_paragraph_with_and_without_trailing_blank() {
    let text = "alpha line\nbeta line\n\nnext para\n";
    // bytes: "alpha line\n" = 0..11, "beta line\n" = 11..21, "\n" = 21..22, ...
    let mut b = Buffer::from_text(text);
    b.cursor = Cursor { line: 1, col: 0 }; // inside the first paragraph
    assert_eq!(
        b.text_object(TextObject::Paragraph, false),
        Some((0, 21)),
        "ip is the two non-blank lines"
    );
    assert_eq!(
        b.text_object(TextObject::Paragraph, true),
        Some((0, 22)),
        "ap also takes the trailing blank line"
    );
}

#[test]
fn paragraph_object_takes_leading_blank_when_none_trails() {
    // The paragraph runs to the document end with no trailing blank, so `ap` takes
    // the *leading* blank instead.
    let text = "intro\n\nlast para line";
    // "intro\n" = 0..6, "\n" = 6..7, "last para line" = 7..21 (no trailing newline)
    let mut b = Buffer::from_text(text);
    b.cursor = Cursor { line: 2, col: 0 };
    assert_eq!(
        b.text_object(TextObject::Paragraph, false),
        Some((7, 21)),
        "ip is the final paragraph"
    );
    assert_eq!(
        b.text_object(TextObject::Paragraph, true),
        Some((6, 21)),
        "ap takes the leading blank when nothing trails"
    );
}

#[test]
fn sentence_object_spans_one_sentence() {
    let text = "One fish. Two fish! Red fish?";
    //          0         10        20
    let mut b = Buffer::from_text(text);
    b.cursor = Cursor { line: 0, col: 12 }; // inside "Two fish!"
    assert_eq!(
        b.text_object(TextObject::Sentence, false),
        Some((10, 19)),
        "is is 'Two fish!' with no trailing space"
    );
    assert_eq!(
        b.text_object(TextObject::Sentence, true),
        Some((10, 20)),
        "as includes the trailing space"
    );
}

#[test]
fn sentence_object_at_paragraph_end_takes_leading_space() {
    let text = "First one. Second and last.";
    //          0         10
    let mut b = Buffer::from_text(text);
    b.cursor = Cursor { line: 0, col: 15 }; // inside the last sentence
    assert_eq!(
        b.text_object(TextObject::Sentence, false),
        Some((11, 27)),
        "is is the final sentence"
    );
    assert_eq!(
        b.text_object(TextObject::Sentence, true),
        Some((10, 27)),
        "as takes the leading space when there's no trailing one"
    );
}

#[test]
fn sentence_break_needs_whitespace_after_the_terminator() {
    // A dot with no whitespace after it (mid-token) is not a break — so the URL's
    // dots keep the sentence whole; Vim's heuristic is terminator + whitespace.
    let text = "Visit quill.md today. Then rest.";
    //          0         10        20        30
    let mut b = Buffer::from_text(text);
    b.cursor = Cursor { line: 0, col: 3 }; // inside the first sentence
    assert_eq!(
        b.text_object(TextObject::Sentence, false),
        Some((0, 21)),
        "the dot in 'quill.md' doesn't split the sentence"
    );
}

// Opt-in performance guard (run via `cargo perf`).

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

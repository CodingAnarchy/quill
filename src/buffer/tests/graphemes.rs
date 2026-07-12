//! `grapheme_after` (#693): the windowed next-boundary scan must be byte-identical
//! to a whole-rope scan at every offset (including inside multi-byte clusters,
//! flag emoji, and past the end), must delete multi-byte graphemes whole, must
//! widen past a window-straddling cluster, and — the perf guard — must stay
//! decoupled from document size.

use super::*;
use std::time::{Duration, Instant};

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
fn cursor_at_byte_snaps_a_mid_emoji_offset_without_panicking() {
    // A stale persisted position / mark / fold heading can name a byte offset that
    // now lands inside a multi-byte grapheme (the bytes shifted since it was
    // recorded). `cursor_at_byte` must floor to the previous char boundary instead
    // of slicing mid-codepoint, which panicked before the fix — bricking reopen.
    let buf = Buffer::from_text("a😀b\n"); // 'a'=1 byte, '😀'=4 bytes (bytes 1..5)
    for byte in 2..=4 {
        // Every interior byte of the emoji floors back to its start (byte 1),
        // which is grapheme column 1 on line 0.
        let cursor = buf.cursor_at_byte(byte);
        assert_eq!(
            cursor,
            Cursor { line: 0, col: 1 },
            "byte {byte} inside the emoji should floor to its boundary"
        );
    }
    // Boundaries themselves are unchanged: 'a' start, emoji start, emoji end, 'b'.
    assert_eq!(buf.cursor_at_byte(0), Cursor { line: 0, col: 0 });
    assert_eq!(buf.cursor_at_byte(1), Cursor { line: 0, col: 1 });
    assert_eq!(buf.cursor_at_byte(5), Cursor { line: 0, col: 2 });
    assert_eq!(buf.cursor_at_byte(6), Cursor { line: 0, col: 3 });
}

#[test]
fn cursor_at_byte_snaps_a_mid_em_dash_offset() {
    // The realistic case: smart typography turns "--" into an em-dash ("—", 3
    // bytes), so a position saved against the old ASCII text lands mid-codepoint.
    let buf = Buffer::from_text("He paused—then ran.\n");
    let dash_start = buf.text().find('—').unwrap(); // byte offset of the em-dash
    for offset in 1..3 {
        let cursor = buf.cursor_at_byte(dash_start + offset);
        // Floors to the em-dash's own boundary rather than panicking.
        assert_eq!(cursor, buf.cursor_at_byte(dash_start));
    }
}

#[test]
fn grapheme_after_snaps_a_non_boundary_byte_without_panicking() {
    // Today every caller passes a cursor-derived boundary, but a future caller (a
    // persisted position or mark whose byte offset now lands inside a multi-byte
    // grapheme) must not panic on the interior `byte_slice` (#1146). `grapheme_after`
    // floors to the previous char boundary, mirroring `cursor_at_byte`.
    let buf = Buffer::from_text("a😀b\n"); // 'a'=1 byte, '😀'=4 bytes (bytes 1..5)
                                           // Every interior byte of the emoji floors to its start (byte 1), so the byte
                                           // just past that grapheme is the emoji's end (byte 5).
    for byte in 1..=4 {
        assert_eq!(
            buf.grapheme_after(byte),
            5,
            "byte {byte} inside/at the emoji should floor to its start and return its end"
        );
    }
    // Real boundaries are unchanged: past 'a' (0→1), past the emoji (5→6), past 'b'.
    assert_eq!(buf.grapheme_after(0), 1);
    assert_eq!(buf.grapheme_after(5), 6);
}

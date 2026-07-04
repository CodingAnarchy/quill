//! Soft word-wrap layout (#143): pure functions that map one logical line to its
//! visual rows at a given column width. Shared by the renderer (to split a line
//! into rows) and by cursor navigation (to move by visual row), so the two always
//! agree on where a line breaks.
//!
//! Everything works in byte offsets within a single line's text (no trailing
//! newline) and display cells (`unicode-width`), so callers map to/from grapheme
//! columns at the edges.
//!
//! Wrapping is computed over the *source* line, before emphasis markers are
//! concealed for display. Concealment only ever removes a cell or two (and only
//! on the cursor's own span), so a row can render slightly narrower than the
//! column but never wider — and keeping the layout independent of the cursor's
//! position means navigation and rendering always agree on the break points.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Byte offsets within `text` where each visual row begins, given a wrap `width`
/// in cells. The first element is always `0`. Breaks at the last space that fits;
/// a word longer than `width` is hard-broken. A `width` of 0 disables wrapping.
pub fn row_starts(text: &str, width: usize) -> Vec<usize> {
    row_starts_first_indent(text, width, 0)
}

/// Like [`row_starts`], but the **first** visual row is `first_indent` cells
/// narrower — for a display-only first-line indent (#722). The renderer prepends
/// the indent to row 0, so its available text width must shrink by the same amount
/// or the opening row overflows the column and the terminal clips its last word.
/// Continuation rows use the full `width`. `first_indent` of 0 is exactly
/// [`row_starts`]. A `width` of 0 disables wrapping.
pub fn row_starts_first_indent(text: &str, width: usize, first_indent: usize) -> Vec<usize> {
    let mut starts = vec![0];
    if width == 0 || text.is_empty() {
        return starts;
    }
    let mut row_start = 0;
    let mut row_w = 0; // display width accumulated on the current row
    let mut last_space: Option<usize> = None; // byte just after the most recent space on this row
                                              // The first row is narrowed by the indent; every continuation row is full width.
    let mut limit = width.saturating_sub(first_indent).max(1);
    for (b, g) in text.grapheme_indices(true) {
        let gw = UnicodeWidthStr::width(g);
        if row_w + gw > limit && b > row_start {
            // Wrap before this grapheme: at the last space if there was one,
            // otherwise hard-break right here.
            let brk = match last_space {
                Some(sp) if sp > row_start => sp,
                _ => b,
            };
            starts.push(brk);
            row_start = brk;
            row_w = UnicodeWidthStr::width(&text[brk..b]) + gw;
            last_space = None;
            limit = width; // continuation rows regain the indent's width
        } else {
            row_w += gw;
        }
        if g == " " {
            last_space = Some(b + g.len());
        }
    }
    starts
}

/// The `(row index, x in cells)` of byte offset `at` within `text`, given its
/// `row_starts`.
pub fn visual_pos(text: &str, starts: &[usize], at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    let row = starts.partition_point(|&s| s <= at).saturating_sub(1);
    let x = UnicodeWidthStr::width(&text[starts[row]..at]);
    (row, x)
}

/// The byte offset on visual `row` nearest display column `x` (clamped to the
/// row's end), given `row_starts`.
pub fn byte_at_visual(text: &str, starts: &[usize], row: usize, x: usize) -> usize {
    let row = row.min(starts.len() - 1);
    let start = starts[row];
    let end = starts.get(row + 1).copied().unwrap_or(text.len());
    let mut w = 0;
    for (b, g) in text[start..end].grapheme_indices(true) {
        let gw = UnicodeWidthStr::width(g);
        if w + gw > x {
            return start + b;
        }
        w += gw;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_line_is_one_row() {
        assert_eq!(row_starts("hello world", 80), vec![0]);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        // width 10: "the quick " (10) | "brown fox" — breaks after the space.
        let starts = row_starts("the quick brown fox", 10);
        assert_eq!(starts, vec![0, 10]);
        // No mid-word break: each row starts at a word.
        assert_eq!(&"the quick brown fox"[10..], "brown fox");
    }

    #[test]
    fn hard_breaks_a_word_longer_than_width() {
        // "abcdefghij" is exactly width; "abcdefghijk" must hard-break.
        let starts = row_starts("abcdefghijk", 10);
        assert_eq!(starts, vec![0, 10]);
    }

    #[test]
    fn width_zero_disables_wrapping() {
        assert_eq!(row_starts("anything at all here", 0), vec![0]);
    }

    #[test]
    fn first_indent_narrows_only_the_first_row() {
        // width 10 with a 4-cell first-line indent: the opening row has only 6
        // cells of text ("the " fits; "the qu" would reach the limit but breaks at
        // the last space), and every continuation row regains the full width of 10
        // (#722). Without the narrowing the first row would render 4 + up-to-10 = 14
        // cells and the terminal would clip the overflow (the reported "you"→"yo").
        let text = "the quick brown fox";
        let starts = row_starts_first_indent(text, 10, 4);
        assert_eq!(starts, vec![0, 4, 10]);
        assert_eq!(&text[0..4], "the "); // indented row: 4 cells + 4 indent = 8 ≤ 10
        assert_eq!(&text[4..10], "quick "); // continuation row wraps at the full 10
        assert_eq!(&text[10..], "brown fox");
        // A zero indent is exactly the plain wrap.
        assert_eq!(row_starts_first_indent(text, 10, 0), row_starts(text, 10));
    }

    #[test]
    fn visual_pos_and_back_round_trip() {
        let text = "the quick brown fox";
        let starts = row_starts(text, 10); // [0, 10]
                                           // "brown" starts at byte 10 → row 1, x 0.
        assert_eq!(visual_pos(text, &starts, 10), (1, 0));
        // Byte 4 ("quick") → row 0, x 4.
        assert_eq!(visual_pos(text, &starts, 4), (0, 4));
        // Column 4 on row 1 maps back to byte 14 ("n" of brown).
        assert_eq!(byte_at_visual(text, &starts, 1, 4), 14);
    }

    #[test]
    fn x_past_row_end_clamps() {
        let text = "the quick brown fox";
        let starts = row_starts(text, 10);
        // A huge x on row 0 clamps to the row's end (byte 10).
        assert_eq!(byte_at_visual(text, &starts, 0, 999), 10);
    }
}

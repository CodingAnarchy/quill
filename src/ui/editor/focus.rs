//! Paragraph geometry the renderer keys off: the focus-mode paragraph span (#301)
//! and the display-only first-line indent decisions (#722). Both walk the buffer's
//! logical lines around the cursor to decide what dims and what indents.

use crate::buffer::Buffer;
use crate::syntax::HighlightKind;

use super::{ByteRange, PARAGRAPH_INDENT};

/// The absolute byte range of the paragraph under the cursor (#301 focus mode):
/// the run of consecutive non-blank lines around the cursor line. Everything
/// outside it dims. A blank cursor line (between paragraphs) yields an empty span,
/// so the whole page dims until the cursor re-enters prose. Prose paragraphs are
/// single soft-wrapped lines, so the line walk almost always stops immediately.
pub(super) fn focus_paragraph(buffer: &Buffer) -> ByteRange {
    let cursor_line = buffer.cursor.line;
    let blank = |l: usize| buffer.line(l).trim().is_empty();
    if blank(cursor_line) {
        let at = buffer.line_start_byte(cursor_line);
        return (at, at);
    }
    let mut first = cursor_line;
    while first > 0 && !blank(first - 1) {
        first -= 1;
    }
    let mut last = cursor_line;
    while last + 1 < buffer.line_count() && !blank(last + 1) {
        last += 1;
    }
    (
        buffer.line_start_byte(first),
        buffer.line_start_byte(last) + buffer.line(last).len(),
    )
}

/// Whether logical line `line` is the first line of a prose paragraph that should
/// receive the display-only first-line indent (#722). A paragraph's first line is
/// `sub == 0` of a non-blank logical line; this decides *which* logical lines are
/// indentable prose. Indented: a non-blank line that isn't a heading or fenced
/// code, and isn't the first paragraph immediately after a heading (matching the
/// manuscript convention the exports already follow — `h1+p`/`h2+p` get no indent).
/// Blank lines, headings, and fenced code are never indented; centered chapter/part
/// dividers and epigraphs are render-only inserts that never pass through here.
pub(super) fn starts_paragraph(buffer: &Buffer, line: usize) -> bool {
    let syntax = buffer.syntax();
    // A logical line is one of these kinds if a highlight of that kind covers its
    // start byte — the whole heading / fenced block is one span (`syntax.rs`).
    let is_kind = |l: usize, kind: HighlightKind| {
        let start = buffer.line_start_byte(l);
        syntax
            .highlights_in(start, start + 1)
            .any(|h| h.kind == kind && h.start <= start && h.end > start)
    };
    let blank = |l: usize| buffer.line(l).trim().is_empty();
    // Blank lines, headings, fenced code, and scene breaks are never indented — the
    // break line is a divider, not prose (#908).
    if blank(line)
        || is_kind(line, HighlightKind::Heading)
        || is_kind(line, HighlightKind::Code)
        || is_kind(line, HighlightKind::SceneBreak)
    {
        return false;
    }
    // The first paragraph that resumes after a heading, a scene break, or a code
    // block gets no indent (the manuscript convention the exports follow — a break
    // or preformatted section reads flush-left after it). Walk up past the blank
    // separator lines; if the nearest non-blank line above is one of those, skip it.
    let mut prev = line;
    while prev > 0 && blank(prev - 1) {
        prev -= 1;
    }
    !(prev > 0
        && (is_kind(prev - 1, HighlightKind::Heading)
            || is_kind(prev - 1, HighlightKind::SceneBreak)
            || is_kind(prev - 1, HighlightKind::Code)))
}

/// The first visual row's indent width in cells for `line`: the display-only
/// paragraph indent (#722) when it's enabled and `line` opens a paragraph, else 0.
/// The wrap math narrows the first row by this (rather than letting the prepended
/// indent overflow the column and clip the opening line's last word).
pub(super) fn first_row_indent(paragraph_indent: bool, buffer: &Buffer, line: usize) -> usize {
    if paragraph_indent && starts_paragraph(buffer, line) {
        PARAGRAPH_INDENT.len()
    } else {
        0
    }
}

//! The galley proofreading page (#1003): a read-only, paginated presentation of the
//! active chapter. Where the edit renderer ([`super::render_body`]) follows the cursor
//! and scrolls continuously, the galley draws one fixed screen-page of visual rows from a
//! stored top line — Space/Backspace step whole pages — so a proofing pass reads like a
//! printed galley rather than a moving wall of text. It never places a terminal cursor
//! (there is nothing to edit) and, optionally, prefixes each row with its logical line
//! number as a reference gutter.
//!
//! Pagination and render share one row-count function ([`page_rows`]) so the page the
//! renderer draws and the page the Space/Backspace math advances by are always the same
//! screenful. No folds and no scene-break inserts apply here — a proofing pass reads the
//! whole chapter text, still, exactly as it is.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::App;
use crate::buffer::Buffer;

use super::rows::{row_count, row_kind, styled_line, SpanHighlights};
use super::{centered_column, ByteRange};

/// The visual rows a logical line occupies in the galley at wrap `width`. The galley has
/// no first-line indent and no folds, so this is just the fenced-code-aware row count —
/// the single source of truth both the render loop and the paging walk read, so they can
/// never disagree on where a page ends.
fn page_rows(buffer: &Buffer, line: usize, width: usize) -> usize {
    let syntax = buffer.syntax();
    let start = buffer.line_start_byte(line);
    let text = buffer.line(line);
    // A read-only galley conceals every emphasis marker uniformly (no cursor to reveal a
    // span under), so classify against an empty conceal set — the same rows the render
    // loop below builds from `conceal_ranges(None)`.
    let conceal = syntax.conceal_ranges(None);
    let kind = row_kind(syntax, start, &text, &conceal);
    row_count(kind, &text, width, 0)
}

/// The top logical line of the page *after* the one starting at `top` (#1003): advance
/// past exactly `height` visual rows. Returns the line whose first visual row is the top
/// of the next screenful. Shared by [`App::galley_page_forward`]; kept in lockstep with
/// the render loop's fill so a page never skips or repeats a row across a turn.
#[must_use]
pub fn galley_next_top(buffer: &Buffer, top: usize, width: usize, height: usize) -> usize {
    if width == 0 || height == 0 {
        return top;
    }
    let total = buffer.line_count();
    let mut rows = 0usize;
    let mut line = top;
    while line < total {
        let r = page_rows(buffer, line, width);
        // Advancing past this line would fill the page — it starts the next one. A line
        // that itself exceeds a whole page (a very long unbroken paragraph) still advances
        // by one so paging always makes progress rather than sticking.
        if rows + r >= height && line > top {
            return line;
        }
        rows += r;
        line += 1;
    }
    total
}

/// The top logical line of the *last* page (#1003): the furthest a page can start and
/// still fill from the bottom of the screen, so the final page shows the tail of the
/// chapter rather than a mostly-blank screen past the end. Both the render (which clamps
/// its display to this) and the forward paging (which never advances past it) read it, so
/// they agree on where the chapter ends on screen.
#[must_use]
pub fn galley_last_top(buffer: &Buffer, width: usize, height: usize) -> usize {
    // The last page starts wherever a backward walk from the end lands: the previous
    // full screenful of rows, measured from just past the final line.
    galley_prev_top(buffer, buffer.line_count(), width, height)
}

/// The top logical line of the page *before* the one starting at `top` (#1003): retreat
/// so the previous screenful of `height` visual rows ends just above `top`, clamped at the
/// first line. Read-only paging's Backspace target.
#[must_use]
pub fn galley_prev_top(buffer: &Buffer, top: usize, width: usize, height: usize) -> usize {
    if width == 0 || height == 0 || top == 0 {
        return 0;
    }
    let mut rows = 0usize;
    let mut line = top;
    // Walk backward, accumulating rows, until one more line would overflow the page — that
    // is the first line still on the previous page.
    while line > 0 {
        let r = page_rows(buffer, line - 1, width);
        if rows + r > height {
            break;
        }
        rows += r;
        line -= 1;
    }
    line
}

/// Draw the galley proofreading page into `area` (#1003). Renders from the stored page
/// top down, one screenful of visual rows, read-only (no cursor placed). Records the page
/// geometry back on the app so Space/Backspace can page by exactly one screenful.
pub fn render_galley(frame: &mut Frame, area: Rect, app: &App) {
    let column = centered_column(area, app.focus_width);
    let buffer = app.edit_buffer();
    let Some(galley) = app.galley else { return };

    let height = column.height as usize;
    // Line numbers steal a fixed-width gutter from the prose column so the text still wraps
    // inside what's left — the number never overlays a word. Width fits the largest line
    // number that could show (the last line), plus a separating space.
    let gutter = if galley.line_numbers {
        gutter_width(buffer.line_count())
    } else {
        0
    };
    let wrap = column.width.saturating_sub(gutter as u16) as usize;
    // Record what the page was drawn at so the paging math advances by this exact
    // screenful (mirrors how the edit renderer feeds `wrap_width` back for navigation).
    app.galley_geometry.set((wrap as u16, height as u16));

    // A read-only galley: conceal every emphasis marker uniformly, and layer nothing else
    // — no search, spell, style-flag, proximity, annotation, or focus highlighting. The
    // proofing surface is the plain, still text. Build a clean `SpanHighlights` with every
    // field populated (the shared struct grew a `style_flags` and a `proximity` layer).
    let conceal = buffer.syntax().conceal_ranges(None);
    let empty: &[ByteRange] = &[];
    let hl = SpanHighlights {
        conceal,
        matches: empty,
        current: None,
        spell: empty,
        style_flags: empty,
        proximity: empty,
        annotations: empty,
        focus: None,
    };

    let syntax = buffer.syntax();
    let total = buffer.line_count();
    let dim = Style::default().fg(app.theme.dim);
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    // Clamp the displayed top so the last page shows the tail of the chapter rather than a
    // blank screen past the end — the same "never scroll past the bottom" clamp the edit
    // view applies. The stored top can sit past this (e.g. opened on a cursor near the very
    // end); the display retreats to the last full page, and forward paging is clamped to
    // match, so they never disagree.
    let last_top = galley_last_top(buffer, wrap, height);
    let mut line = galley.top_line.min(last_top).min(total.saturating_sub(1));
    'fill: while lines.len() < height && line < total {
        let start = buffer.line_start_byte(line);
        let text = buffer.line(line);
        let kind = row_kind(syntax, start, &text, &hl.conceal);
        let rows = styled_line(syntax, start, &text, &hl, wrap, false, kind);
        for (sub, row) in rows.into_iter().enumerate() {
            if lines.len() >= height {
                break 'fill;
            }
            if gutter > 0 {
                // The line number sits in the gutter on the first visual row of each
                // logical line; continuation rows show a blank gutter so a wrapped line
                // reads as one numbered unit.
                let label = if sub == 0 {
                    format!("{:>width$} ", line + 1, width = gutter - 1)
                } else {
                    " ".repeat(gutter)
                };
                let mut spans = vec![Span::styled(label, dim)];
                spans.extend(row.spans);
                lines.push(Line::from(spans));
            } else {
                lines.push(row);
            }
        }
        line += 1;
    }

    frame.render_widget(Paragraph::new(lines).block(Block::default()), column);
}

/// The gutter width (in cells) that numbers up to `line_count` need: the widest 1-based
/// number's digit count plus a trailing separating space.
fn gutter_width(line_count: usize) -> usize {
    let digits = line_count.max(1).to_string().len();
    digits + 1
}

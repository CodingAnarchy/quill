//! The annotation margin: each inline note drawn in the gutter to the right of the
//! prose column, level with the row its anchored text starts on, so the author
//! sees their notes while they write. Display-only — it reads the live
//! annotations and the frame's recorded [`EditorLayout`], never the files.
//!
//! Notes keep their anchor's row when they can; one that would collide with the
//! note above slides down below it, so the margin never overprints. A note longer
//! than a few rows is clipped with an ellipsis — the review list has the full
//! thread.

use std::fmt::Write;

use super::*;
use crate::annotation::Live;
use crate::app::mouse::VisualRow;
use crate::wrap::row_starts;

/// The narrowest margin worth drawing notes into; below this they'd wrap a word
/// or two per row and read as noise.
const MIN_WIDTH: u16 = 18;
/// The widest the note margin grows, however much room the terminal has.
const MAX_WIDTH: u16 = 40;
/// Blank cells between the prose column's right edge and the notes.
const GAP: u16 = 2;
/// The most rows one note takes before it's clipped with an ellipsis.
const MAX_NOTE_ROWS: usize = 4;

/// Whether the margin applies this frame: switched on, notes to show, and not in
/// composition mode (which hides all chrome).
pub(super) const fn wanted(app: &App) -> bool {
    app.annotation_margin && !app.annotations.is_empty() && !app.distraction_free
}

/// Place the prose `column` within `area` so the margin has room: keep it
/// centered when the right-hand gutter already fits a note margin, otherwise
/// shift it left just enough to open one. When even that won't fit, the column
/// stays centered and no margin is drawn.
pub(super) const fn make_room(area: Rect, column: Rect) -> Rect {
    let spare = area.width.saturating_sub(column.width);
    let need = GAP + MIN_WIDTH;
    let right_now = area.right().saturating_sub(column.right());
    if right_now >= need || spare < need + 1 {
        return column;
    }
    Rect {
        x: area.x + (spare - need),
        ..column
    }
}

/// Draw the notes for the rows `layout` recorded this frame into the gutter right
/// of its column, within `area` (the editor pane).
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App, layout: &EditorLayout) {
    let column = layout.column;
    let x = column.right() + GAP;
    let width = area.right().saturating_sub(x).min(MAX_WIDTH);
    if width < MIN_WIDTH || layout.rows.is_empty() {
        return;
    }
    let height = usize::from(column.height);
    let theme = app.theme;
    let cursor = app.edit_buffer().cursor_byte();

    let mut next_free = 0;
    for note in &app.annotations {
        let Some(anchor) = anchor_row(layout, note) else {
            continue;
        };
        let y = anchor.max(next_free);
        if y >= height {
            break;
        }
        let under_cursor = note.start <= cursor && cursor <= note.end;
        let rows = note_lines(note, usize::from(width), &theme, under_cursor);
        let shown = rows.len().min(height - y);
        let rect = Rect {
            x,
            y: column.y + y as u16,
            width,
            height: shown as u16,
        };
        frame.render_widget(Paragraph::new(rows), rect);
        // A blank row between stacked notes keeps adjacent ones distinct.
        next_free = y + shown + 1;
    }
}

/// The on-screen row (counted from the column's top) where `note`'s anchor
/// starts: the visible row holding its start byte, or the top row when the note
/// began above the viewport but still runs into it. `None` when it's off screen.
fn anchor_row(layout: &EditorLayout, note: &Live) -> Option<usize> {
    let span = |r: &VisualRow| (r.line_start + r.row_start, r.line_start + r.row_end);
    let hit = layout
        .rows
        .iter()
        .position(|r| r.editable && (span(r).0..=span(r).1).contains(&note.start));
    hit.or_else(|| {
        let top = layout.rows.iter().find(|r| r.editable).map(|r| span(r).0)?;
        (note.start < top && top < note.end).then_some(0)
    })
}

/// A note's margin rows: its text soft-wrapped to `width` (one cell of padding
/// each side) on the annotation tint, a ✓ on a resolved note, and the reply count
/// when the thread has any. Clipped to [`MAX_NOTE_ROWS`] with an ellipsis. The
/// note under the cursor is bold, tying it to the span being edited.
fn note_lines(note: &Live, width: usize, theme: &Theme, under_cursor: bool) -> Vec<Line<'static>> {
    // Flatten whitespace: an imported review comment can carry line breaks.
    let flat = note.note.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut text = if note.resolved {
        format!("✓ {flat}")
    } else {
        flat
    };
    if !note.replies.is_empty() {
        // Writing into a `String` can't fail.
        let _ = write!(text, " (+{})", note.replies.len());
    }
    let mut style = Style::default().bg(theme.annotation);
    if note.resolved {
        style = style.fg(theme.dim);
    }
    if under_cursor {
        style = style.add_modifier(Modifier::BOLD);
    }

    let inner = width.saturating_sub(2).max(1);
    let starts = row_starts(&text, inner);
    let clipped = starts.len() > MAX_NOTE_ROWS;
    let mut rows: Vec<String> = (0..starts.len().min(MAX_NOTE_ROWS))
        .map(|i| {
            let end = starts.get(i + 1).copied().unwrap_or(text.len());
            text[starts[i]..end].trim_end().to_string()
        })
        .collect();
    if clipped {
        if let Some(last) = rows.last_mut() {
            let mut cut: String = last.chars().take(inner.saturating_sub(1)).collect();
            cut.push('…');
            *last = cut;
        }
    }
    rows.into_iter()
        .map(|row| {
            let pad = inner.saturating_sub(row.width());
            Line::from(Span::styled(format!(" {row}{} ", " ".repeat(pad)), style))
        })
        .collect()
}

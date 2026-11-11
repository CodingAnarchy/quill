//! The scansion gutter (#1326): beside each verse line, level with its first
//! row, the line's verdict, stress reading, syllable count, and rhyme letter, in
//! the gutter to the left of the prose column. Display-only: it reads the cached
//! marks and the frame's recorded [`EditorLayout`], so drawing it is a binary
//! search per visible line, never a scan.

use ratatui::layout::Alignment;

use super::*;
use crate::verse::{GutterLine, Marks};

/// Blank cells between the gutter and the prose column.
const GAP: u16 = 2;
/// The narrowest gutter worth drawing.
const MIN_WIDTH: u16 = 8;
/// The widest the gutter grows, whatever the meter.
const MAX_WIDTH: u16 = 34;

/// Whether the gutter applies this frame: switched on, verse lines to mark, and
/// not in composition mode (which hides all chrome).
pub(super) const fn wanted(app: &App) -> bool {
    app.scansion_on && !app.scansion.lines.is_empty() && !app.distraction_free
}

/// The cells the widest gutter line needs, within the bounds.
fn needed(marks: &Marks) -> u16 {
    let widest = marks
        .lines
        .iter()
        .map(|l| cell_text(l).chars().count())
        .max()
        .unwrap_or(0);
    u16::try_from(widest)
        .unwrap_or(MAX_WIDTH)
        .clamp(MIN_WIDTH, MAX_WIDTH)
}

/// Place the prose `column` within `area` so the gutter has room on its left:
/// keep it where it is when the left-hand space already fits, otherwise shift
/// it right just enough. When even that won't fit, it stays put and the gutter
/// draws what fits.
pub(super) fn make_room(area: Rect, column: Rect, marks: &Marks) -> Rect {
    let need = GAP + needed(marks);
    let left_now = column.x.saturating_sub(area.x);
    let spare = area.width.saturating_sub(column.width);
    if left_now >= need || spare < need {
        return column;
    }
    Rect {
        x: area.x + need,
        ..column
    }
}

/// Draw the gutter for the rows `layout` recorded this frame, within `area`.
pub(super) fn draw(frame: &mut Frame, area: Rect, app: &App, layout: &EditorLayout) {
    let column = layout.column;
    let room = column.x.saturating_sub(area.x).saturating_sub(GAP);
    let width = room.min(needed(&app.scansion));
    if width < MIN_WIDTH || layout.rows.is_empty() {
        return;
    }
    let x = column.x - GAP - width;
    let lines = &app.scansion.lines;
    for (y, row) in layout.rows.iter().enumerate() {
        if !row.editable || row.row_start != 0 || y >= usize::from(column.height) {
            continue;
        }
        let Ok(i) = lines.binary_search_by_key(&row.line_start, |l| l.line_start) else {
            continue;
        };
        let rect = Rect {
            x,
            y: column.y + y as u16,
            width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(cell(&lines[i], &app.theme, usize::from(width)))
                .alignment(Alignment::Right),
            rect,
        );
    }
}

/// A gutter line as plain text, for measuring.
fn cell_text(line: &GutterLine) -> String {
    let mut text = String::new();
    if let Some(m) = &line.meter {
        text = format!("\u{2713} {} {:>2}", m.marks, m.syllables);
    }
    if let Some(r) = &line.rhyme {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&r.letter);
    }
    text
}

/// A gutter line's styled cells: the meter verdict (✓ fits, ~ by a licence, ✗
/// off), the reading, the syllable count, then the rhyme letter (red when the
/// ending breaks the scheme). Too wide for the gutter, the reading is cut from
/// the left, keeping the line's end, where most faults show.
fn cell(line: &GutterLine, theme: &Theme, width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let dim = Style::default().fg(theme.dim);
    if let Some(m) = &line.meter {
        let (glyph, color) = if !m.fits {
            ('\u{2717}', theme.removed)
        } else if m.licence {
            ('~', theme.revised)
        } else {
            ('\u{2713}', theme.added)
        };
        let rhyme_cells = line
            .rhyme
            .as_ref()
            .map_or(0, |r| r.letter.chars().count() + 1);
        let room = width.saturating_sub(5 + rhyme_cells);
        let marks: String = if m.marks.chars().count() > room {
            let skip = m.marks.chars().count() - room.saturating_sub(1);
            std::iter::once('\u{2026}')
                .chain(m.marks.chars().skip(skip))
                .collect()
        } else {
            m.marks.clone()
        };
        spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
        spans.push(Span::styled(
            format!(" {marks}"),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(format!(" {:>2}", m.syllables), dim));
    }
    if let Some(r) = &line.rhyme {
        let color = if r.fits { theme.added } else { theme.removed };
        if !spans.is_empty() {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(r.letter.clone(), Style::default().fg(color)));
    }
    Line::from(spans)
}

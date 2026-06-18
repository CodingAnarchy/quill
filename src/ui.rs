//! Rendering. Quill draws a centered, fixed-width column of prose with a thin
//! status line, keeping the eye in one place the way a printed page does.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::{App, Mode};
use crate::syntax::{Highlight, HighlightKind, Syntax};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    draw_editor(frame, app, chunks[0]);
    draw_status(frame, app, chunks[1]);
}

fn draw_editor(frame: &mut Frame, app: &App, area: Rect) {
    // Center a column of `focus_width` cells; fall back to full width if narrow.
    let width = app.focus_width.min(area.width.saturating_sub(2));
    let left_pad = area.width.saturating_sub(width) / 2;
    let column = Rect {
        x: area.x + left_pad,
        y: area.y + 1,
        width,
        height: area.height.saturating_sub(1),
    };

    let buffer = &app.buffer;
    let height = column.height as usize;

    // Vertically follow the cursor: keep it roughly centered in the viewport.
    let total = buffer.line_count();
    let cursor_line = buffer.cursor.line;
    let top = cursor_line.saturating_sub(height / 2).min(total.saturating_sub(height).max(0));

    let lines: Vec<Line> = (top..(top + height).min(total))
        .map(|i| styled_line(buffer.syntax(), buffer.line_start_byte(i), &buffer.line(i)))
        .collect();

    let paragraph = Paragraph::new(lines).block(Block::default());
    frame.render_widget(paragraph, column);

    // Place the terminal cursor (only in Insert mode).
    if app.mode == Mode::Insert {
        let screen_y = column.y + (cursor_line - top) as u16;
        let line = buffer.line(cursor_line);
        let col_width: u16 = line
            .chars()
            .take(buffer.cursor.col)
            .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as u16)
            .sum();
        let screen_x = column.x + col_width.min(column.width.saturating_sub(1));
        frame.set_cursor_position((screen_x, screen_y));
    }
}

/// Build a styled line by splitting `text` (one line, no newline) into runs of
/// equal style according to the syntax highlights overlapping it. `line_start`
/// is the line's absolute byte offset, since highlights use document offsets.
fn styled_line(syntax: &Syntax, line_start: usize, text: &str) -> Line<'static> {
    if text.is_empty() {
        return Line::raw("");
    }

    let hls: Vec<Highlight> = syntax
        .highlights_in(line_start, line_start + text.len())
        .copied()
        .collect();
    if hls.is_empty() {
        return Line::raw(text.to_string());
    }

    let mut spans: Vec<Span> = Vec::new();
    let mut run_start = 0usize;
    let mut run_style: Option<Style> = None;
    for (local_byte, _) in text.char_indices() {
        let style = style_at(&hls, line_start + local_byte);
        match run_style {
            Some(prev) if prev == style => {}
            Some(prev) => {
                spans.push(Span::styled(text[run_start..local_byte].to_string(), prev));
                run_start = local_byte;
                run_style = Some(style);
            }
            None => {
                run_start = local_byte;
                run_style = Some(style);
            }
        }
    }
    if let Some(style) = run_style {
        spans.push(Span::styled(text[run_start..].to_string(), style));
    }
    Line::from(spans)
}

/// Merge every highlight covering `byte` into one style. Headings get a color;
/// strong and emphasis layer on bold/italic, so nested `***x***` reads as both.
fn style_at(hls: &[Highlight], byte: usize) -> Style {
    let mut style = Style::default();
    for h in hls.iter().filter(|h| byte >= h.start && byte < h.end) {
        style = match h.kind {
            HighlightKind::Heading => style.fg(Color::Cyan).add_modifier(Modifier::BOLD),
            HighlightKind::Strong => style.add_modifier(Modifier::BOLD),
            HighlightKind::Emphasis => style.add_modifier(Modifier::ITALIC),
        };
    }
    style
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let buffer = &app.buffer;

    if app.mode == Mode::Prompt {
        let text = app.prompt.clone().unwrap_or_default();
        let prompt = Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )));
        frame.render_widget(prompt, area);
        return;
    }

    let name = buffer
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "[untitled]".into());

    let dirty = if buffer.dirty { " ●" } else { "" };
    let left = format!(" {name}{dirty}");
    let right = match &app.message {
        Some(m) => format!("{m}  "),
        None => format!(
            "{} words · ln {}/{}  ",
            buffer.word_count(),
            buffer.cursor.line + 1,
            buffer.line_count()
        ),
    };

    let pad = (area.width as usize)
        .saturating_sub(left.len() + right.len())
        .max(1);
    let line = Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
    ]);

    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 36))),
        area,
    );
}

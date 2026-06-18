//! Rendering. Quill draws a centered, fixed-width column of prose with a thin
//! status line, keeping the eye in one place the way a printed page does.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::{App, Mode};
use crate::syntax::{Heading, Highlight, HighlightKind, Syntax};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // The outline pane takes a left strip when toggled on (Ctrl-O); the editor
    // centers within whatever space remains, so the page stays balanced.
    if app.outline_visible {
        let outline_w = 28.min(chunks[0].width / 3);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(outline_w), Constraint::Min(1)])
            .split(chunks[0]);
        draw_nav_pane(frame, app, cols[0]);
        draw_editor(frame, app, cols[1]);
    } else {
        draw_editor(frame, app, chunks[0]);
    }

    draw_status(frame, app, chunks[1]);
}

/// The index of the heading whose section the cursor is currently in: the last
/// heading at or before the cursor. `None` if the cursor sits above the first
/// heading (or there are no headings).
fn current_section(headings: &[Heading], cursor_byte: usize) -> Option<usize> {
    headings.iter().rposition(|h| h.start <= cursor_byte)
}

/// The accent style for the active/current item in the nav pane.
fn active_style() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

/// One chapter row: a marker for the active chapter and its word count, reversed
/// while it's the Nav-mode selection.
fn chapter_line(title: &str, words: usize, is_active: bool, is_selected: bool) -> Line<'static> {
    let mut style = if is_active { active_style() } else { Style::default() };
    if is_selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let marker = if is_active { "▸ " } else { "  " };
    Line::from(Span::styled(format!("{marker}{title}  {words}"), style))
}

/// The active chapter's heading outline, indented by level, current section
/// accented. `skip` omits one heading by index (used to drop a leading H1 that
/// just repeats the chapter title) while leaving the remaining indices — and so
/// the `current` highlight — intact.
fn heading_outline_lines(
    headings: &[Heading],
    current: Option<usize>,
    skip: Option<usize>,
) -> Vec<Line<'static>> {
    headings
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != skip)
        .map(|(i, h)| {
            let indent = "  ".repeat(h.level as usize);
            let style = if Some(i) == current {
                active_style()
            } else {
                Style::default().fg(Color::Gray)
            };
            Line::from(Span::styled(format!("{indent}{}", h.title), style))
        })
        .collect()
}

/// If the chapter opens with a level-1 heading whose text matches the chapter
/// title, the outline would just repeat the title — return its index to skip it.
fn redundant_title_heading(headings: &[Heading], chapter_title: &str) -> Option<usize> {
    match headings.first() {
        Some(h) if h.level == 1 && h.title == chapter_title => Some(0),
        _ => None,
    }
}

/// The navigation pane: the project's chapters, with the active chapter's
/// in-chapter heading outline nested beneath it (the current section
/// highlighted). While browsing (Nav mode), the selected chapter is reversed.
fn draw_nav_pane(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::RIGHT);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = Span::styled(
        "CHAPTERS",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let mut lines: Vec<Line> = vec![Line::from(header), Line::raw("")];

    let headings = app.buffer.syntax().headings();
    let current_heading = current_section(headings, app.buffer.cursor_byte());

    // Count each chapter once for this frame, then reuse for the rows and the
    // total — so the active chapter's live count isn't recomputed twice.
    let counts: Vec<usize> = (0..app.project.manifest.chapters.len())
        .map(|i| app.chapter_word_count(i))
        .collect();

    for (i, chapter) in app.project.manifest.chapters.iter().enumerate() {
        let is_active = i == app.project.active_index();
        let is_selected = app.mode == Mode::Nav && i == app.nav_selected;
        lines.push(chapter_line(&chapter.title, counts[i], is_active, is_selected));
        if is_active {
            let skip = redundant_title_heading(headings, &chapter.title);
            lines.extend(heading_outline_lines(headings, current_heading, skip));
        }
    }

    // Rolled-up project total.
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{} words", counts.iter().sum::<usize>()),
        Style::default().fg(Color::DarkGray),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
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
    let top = cursor_line
        .saturating_sub(height / 2)
        .min(total.saturating_sub(height));

    // Emphasis markers are hidden, except on the span the cursor sits in (so it
    // stays editable). Concealment shifts screen columns, so the cursor X below
    // must be measured over the *visible* text, not the source.
    let cursor_byte = buffer.cursor_byte();
    let conceal = buffer.syntax().conceal_ranges(Some(cursor_byte));

    let lines: Vec<Line> = (top..(top + height).min(total))
        .map(|i| styled_line(buffer.syntax(), buffer.line_start_byte(i), &buffer.line(i), &conceal))
        .collect();

    let paragraph = Paragraph::new(lines).block(Block::default());
    frame.render_widget(paragraph, column);

    // Place the terminal cursor (only in Insert mode). The cursor's own span is
    // revealed, so the cursor never lands on a concealed character.
    if app.mode == Mode::Insert {
        let screen_y = column.y + (cursor_line - top) as u16;
        let col_width = visible_width_before(
            &buffer.line(cursor_line),
            buffer.line_start_byte(cursor_line),
            cursor_byte,
            &conceal,
        );
        let screen_x = column.x + col_width.min(column.width.saturating_sub(1));
        frame.set_cursor_position((screen_x, screen_y));
    }
}

/// Build a styled line: split `text` (one line, no newline) into runs of equal
/// style from the overlapping highlights, dropping any characters whose bytes
/// fall in a `conceal` range (hidden emphasis markers). `line_start` is the
/// line's absolute byte offset, since highlights and conceal ranges use document
/// offsets.
fn styled_line(
    syntax: &Syntax,
    line_start: usize,
    text: &str,
    conceal: &[(usize, usize)],
) -> Line<'static> {
    if text.is_empty() {
        return Line::raw("");
    }

    let line_end = line_start + text.len();
    let hls: Vec<Highlight> = syntax.highlights_in(line_start, line_end).copied().collect();
    let concealed_here = conceal.iter().any(|&(s, e)| s < line_end && e > line_start);
    if hls.is_empty() && !concealed_here {
        return Line::raw(text.to_string());
    }

    // Build runs character by character, since concealed chars are dropped and
    // visible runs may not be contiguous in the source.
    let mut spans: Vec<Span> = Vec::new();
    let mut run = String::new();
    let mut run_style = Style::default();
    for (local_byte, ch) in text.char_indices() {
        let abs = line_start + local_byte;
        if is_concealed(abs, conceal) {
            continue;
        }
        let style = style_at(&hls, abs);
        if !run.is_empty() && style != run_style {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
        }
        if run.is_empty() {
            run_style = style;
        }
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style));
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

fn is_concealed(byte: usize, conceal: &[(usize, usize)]) -> bool {
    conceal.iter().any(|&(s, e)| byte >= s && byte < e)
}

/// Visible cell width of the characters before `cursor_byte` on a line, skipping
/// concealed (hidden-marker) characters — so the on-screen cursor lands in the
/// right place even though markers have been removed from the rendered text.
fn visible_width_before(
    text: &str,
    line_start: usize,
    cursor_byte: usize,
    conceal: &[(usize, usize)],
) -> u16 {
    let mut width = 0u16;
    for (local_byte, ch) in text.char_indices() {
        let abs = line_start + local_byte;
        if abs >= cursor_byte {
            break;
        }
        if is_concealed(abs, conceal) {
            continue;
        }
        width += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
    }
    width
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let buffer = &app.buffer;

    if app.mode == Mode::Prompt {
        // Label plus any typed text (empty for y/N confirmations) — two borrowed
        // spans, no per-frame allocation.
        let style = Style::default().fg(Color::Black).bg(Color::Yellow);
        let prompt = Paragraph::new(Line::from(vec![
            Span::styled(app.prompt.as_deref().unwrap_or_default(), style),
            Span::styled(app.prompt_input.as_str(), style),
        ]));
        frame.render_widget(prompt, area);
        return;
    }

    // The manifest is the source of truth for the active chapter's name; every
    // real (and scratch) project always has an active chapter.
    let name = app.project.active_title().unwrap_or("[untitled]");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Cursor};
    use crate::project::Project;
    use ratatui::{backend::TestBackend, Terminal};

    fn buffer_with(text: &str) -> Buffer {
        let mut b = Buffer::empty();
        for c in text.chars() {
            b.insert_char(c);
        }
        b.refresh_syntax();
        b
    }

    /// An App over an in-memory scratch project, for render tests.
    fn app_with(text: &str) -> App {
        App::new(buffer_with(text), Project::scratch())
    }

    /// Render the app to a TestBackend and return the screen as plain text.
    fn render(app: &App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn markers_are_hidden_when_cursor_is_outside_the_span() {
        // After typing, the cursor sits past the span, so markers conceal.
        let app = app_with("*italic* word");
        let screen = render(&app, 80, 8);
        assert!(screen.contains("italic word"), "screen:\n{screen}");
        assert!(!screen.contains('*'), "markers should be hidden:\n{screen}");
    }

    #[test]
    fn markers_reappear_when_cursor_enters_the_span() {
        let mut app = app_with("*italic* word");
        app.buffer.cursor = Cursor { line: 0, col: 3 }; // inside "italic"
        let screen = render(&app, 80, 8);
        assert!(screen.contains("*italic*"), "screen:\n{screen}");
    }

    #[test]
    fn current_section_is_the_last_heading_at_or_before_the_cursor() {
        use crate::syntax::Heading;
        let hs = vec![
            Heading { level: 1, title: "A".into(), start: 0 },
            Heading { level: 2, title: "B".into(), start: 50 },
        ];
        assert_eq!(current_section(&hs, 0), Some(0));
        assert_eq!(current_section(&hs, 49), Some(0));
        assert_eq!(current_section(&hs, 50), Some(1));
        assert_eq!(current_section(&hs, 999), Some(1));

        // Cursor above the first heading, and the no-headings case.
        let later = vec![Heading { level: 1, title: "A".into(), start: 10 }];
        assert_eq!(current_section(&later, 5), None);
        assert_eq!(current_section(&[], 0), None);
    }

    /// An App over an in-memory project with several chapters (no files needed
    /// for rendering, which only reads chapter titles + the active buffer).
    fn app_with_chapters(titles: &[&str], active_text: &str) -> App {
        App::new(buffer_with(active_text), Project::scratch_with(titles))
    }

    #[test]
    fn pane_nests_active_chapter_headings() {
        let mut app = app_with("# Chapter\n\nbody\n\n## Scene\n\nmore\n");
        app.outline_visible = true;
        let screen = render(&app, 90, 16);
        assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
        // The scratch project's chapter, with the in-chapter headings nested.
        assert!(screen.contains("Chapter 1"), "screen:\n{screen}");
        assert!(screen.contains("Scene"), "screen:\n{screen}");
    }

    #[test]
    fn pane_lists_chapter_even_without_headings() {
        let mut app = app_with("just prose, no headings\n");
        app.outline_visible = true;
        let screen = render(&app, 90, 12);
        assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
        assert!(screen.contains("Chapter 1"), "screen:\n{screen}");
    }

    #[test]
    fn outline_skips_a_leading_h1_that_matches_the_chapter_title() {
        use crate::syntax::Heading;
        let headings = vec![
            Heading { level: 1, title: "The Crossing".into(), start: 0 },
            Heading { level: 2, title: "At the ford".into(), start: 30 },
        ];

        // The chapter is titled "The Crossing", so its opening H1 is redundant.
        let skip = redundant_title_heading(&headings, "The Crossing");
        assert_eq!(skip, Some(0));

        let texts: Vec<String> = heading_outline_lines(&headings, None, skip)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("At the ford"));
        assert!(!texts[0].contains("The Crossing"));

        // A non-matching title keeps every heading.
        assert_eq!(redundant_title_heading(&headings, "Different"), None);
        assert_eq!(
            heading_outline_lines(&headings, None, None).len(),
            2
        );
    }

    #[test]
    fn pane_lists_all_chapters_and_marks_the_active_one() {
        let mut app = app_with_chapters(&["One", "Two", "Three"], "body\n");
        app.outline_visible = true;
        let screen = render(&app, 90, 14);
        assert!(screen.contains("One"), "screen:\n{screen}");
        assert!(screen.contains("Two"), "screen:\n{screen}");
        assert!(screen.contains("Three"), "screen:\n{screen}");
        // The active chapter (index 0) carries the active marker.
        assert!(screen.contains("▸ One"), "screen:\n{screen}");
    }
}

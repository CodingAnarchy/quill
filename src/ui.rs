//! Rendering. Quill draws a centered, fixed-width column of prose with a thin
//! status line, keeping the eye in one place the way a printed page does.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::frontmatter::{Epigraph, FrontMatter, Status};
use crate::mode::{BibleReview, Findings, Mode};
use crate::manuscript::SegmentHeader;
use crate::syntax::{Heading, Highlight, HighlightKind, Syntax};

/// A rendered region of the screen. Each pane borrows the [`App`] and draws
/// itself into a given area. A custom trait rather than ratatui's `Widget`
/// because the editor needs `frame.set_cursor_position` for cursor placement,
/// which `Widget::render` can't do (it only receives a cell buffer).
trait Pane {
    fn render(&self, frame: &mut Frame, area: Rect);
}

/// The prose column: centered, fixed-width text following the cursor.
struct Editor<'a>(&'a App);
/// The left strip: chapter list with the active chapter's heading outline
/// nested beneath it (current section highlighted; the Nav-mode selection
/// reversed).
struct NavPane<'a>(&'a App);
/// The bottom line: chapter name / counts, or the active prompt.
struct StatusBar<'a>(&'a App);

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // The progress report and AI answer are full-screen read-only overlays.
    if let Some(lines) = &app.report {
        draw_report(frame, area, &app.project.manifest.title, lines);
        return;
    }
    if let Some(lines) = &app.ai_panel {
        draw_ai_panel(frame, area, lines);
        return;
    }
    if let Mode::BibleReview(review) = &app.mode {
        draw_bible_review(frame, area, review);
        return;
    }
    if let Mode::Findings(findings) = &app.mode {
        draw_findings(frame, area, findings);
        return;
    }

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
        NavPane(app).render(frame, cols[0]);
        Editor(app).render(frame, cols[1]);
    } else {
        Editor(app).render(frame, chunks[0]);
    }

    StatusBar(app).render(frame, chunks[1]);
}

/// Render the progress report full-screen in a bordered box.
fn draw_report(frame: &mut Frame, area: Rect, title: &str, lines: &[Line<'static>]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} — progress (any key to close) "));
    frame.render_widget(Paragraph::new(lines.to_vec()).block(block), area);
}

/// Render the story-bible conflict review: a diff of the front conflict (or the
/// scratch editor when editing the proposed version).
fn draw_bible_review(frame: &mut Frame, area: Rect, review: &BibleReview) {
    let Some(front) = review.conflicts.first() else {
        return;
    };
    let remaining = review.conflicts.len();

    // Editing the proposed version before saving.
    if let Some(buf) = &review.editing {
        let block = Block::default().borders(Borders::ALL).title(format!(
            " Edit proposed: {} — Ctrl-S save · Esc back ",
            front.name
        ));
        let text = buf.text();
        let lines: Vec<Line> = text.lines().map(|l| Line::raw(l.to_string())).collect();
        frame.render_widget(
            Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    // Diff view: current (on disk) → proposed.
    let mut lines = vec![
        Line::from(Span::styled(
            format!("'{}' was edited by hand — choose what to keep:", front.name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    for change in &front.diff {
        let color = match change.chars().next() {
            Some('-') => Color::Red,
            Some('+') => Color::Green,
            _ => Color::DarkGray,
        };
        lines.push(Line::from(Span::styled(change.clone(), Style::default().fg(color))));
    }

    let block = Block::default().borders(Borders::ALL).title(format!(
        " Bible conflict: {} ({remaining} left) — (o)verwrite (e)dit (k)eep · Esc cancel ",
        front.name
    ));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

/// Render the continuity findings as a navigable triage list.
fn draw_findings(frame: &mut Frame, area: Rect, findings: &Findings) {
    let mut lines = vec![
        Line::from(Span::styled(
            "Findings — ↑↓ move · enter jump · d dismiss · esc close",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    for (i, item) in findings.items.iter().enumerate() {
        let marker = if i == findings.selected { "▸ " } else { "  " };
        let mut style = Style::default();
        if i == findings.selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        let chapters = if item.chapters.is_empty() {
            String::new()
        } else {
            format!("  [{}]", item.chapters.join(", "))
        };
        lines.push(Line::from(Span::styled(format!("{marker}{}{chapters}", item.summary), style)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Findings ({}) ", findings.items.len()));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

/// Render the AI answer full-screen in a bordered box. Wraps, since the answer
/// is free-form prose rather than the report's pre-sized rows.
fn draw_ai_panel(frame: &mut Frame, area: Rect, lines: &[Line<'static>]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Ask the manuscript — Claude (any key to close) ");
    frame.render_widget(
        Paragraph::new(lines.to_vec())
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
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

/// Accent color for a chapter's revision status badge.
fn status_style(status: Status) -> Style {
    let color = match status {
        Status::Draft => Color::DarkGray,
        Status::Revised => Color::Yellow,
        Status::Final => Color::Green,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// One chapter row: a marker for the active chapter, its word count, and a
/// colored status badge — reversed while it's the Nav-mode selection.
fn chapter_line(
    title: &str,
    words: usize,
    status: Option<Status>,
    is_active: bool,
    is_selected: bool,
) -> Line<'static> {
    let mut style = if is_active { active_style() } else { Style::default() };
    if is_selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let marker = if is_active { "▸ " } else { "  " };
    let mut spans = vec![Span::styled(format!("{marker}{title}  {words}"), style)];
    if let Some(status) = status {
        let mut badge = status_style(status);
        if is_selected {
            badge = badge.add_modifier(Modifier::REVERSED);
        }
        spans.push(Span::styled(format!(" {}", status.badge()), badge));
    }
    Line::from(spans)
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

/// The top of the nav pane: book / series metadata above the chapter list.
/// Series (if any) sits above the book title; subtitle and author follow.
fn project_header(manifest: &crate::project::Manifest) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(series) = &manifest.series {
        lines.push(Line::from(Span::styled(
            series.clone(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(Span::styled(
        manifest.title.clone(),
        active_style(),
    )));
    if let Some(subtitle) = &manifest.subtitle {
        lines.push(Line::from(Span::styled(
            subtitle.clone(),
            Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
        )));
    }
    if let Some(author) = &manifest.author {
        lines.push(Line::from(Span::styled(
            format!("by {author}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "CHAPTERS",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(""));
    lines
}

impl Pane for NavPane<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.0;
        let block = Block::default().borders(Borders::RIGHT);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines: Vec<Line> = project_header(&app.project.manifest);

        let headings = app.buffer.syntax().headings();
        let current_heading = current_section(headings, app.buffer.cursor_byte());

        // Resolve the active index once: active_index() is an O(n) scan, so
        // calling it per row would be O(n²) per frame.
        let active = app.project.active_index();

        // The browser selection, if we're in Nav mode.
        let nav_selected = match app.mode {
            Mode::Nav { selected } => Some(selected),
            _ => None,
        };

        // Count each chapter once for this frame, then reuse for the rows and
        // the total — so the active chapter's live count isn't recomputed twice.
        let counts: Vec<usize> = (0..app.project.manifest.chapters.len())
            .map(|i| app.chapter_word_count(i))
            .collect();

        for (i, chapter) in app.project.manifest.chapters.iter().enumerate() {
            let is_active = i == active;
            let is_selected = nav_selected == Some(i);
            let status = app.project.chapter_status(i);
            lines.push(chapter_line(&chapter.title, counts[i], status, is_active, is_selected));
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
}

impl Pane for Editor<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.0;
        // Center a column of `focus_width` cells; fall back to full width if narrow.
        let width = app.focus_width.min(area.width.saturating_sub(2));
        let left_pad = area.width.saturating_sub(width) / 2;
        let column = Rect {
            x: area.x + left_pad,
            y: area.y + 1,
            width,
            height: area.height.saturating_sub(1),
        };

        let buffer = app.edit_buffer();
        let height = column.height as usize;

        // Vertically follow the cursor: keep it roughly centered in the viewport.
        let total = buffer.line_count();
        let cursor_line = buffer.cursor.line;
        let top = cursor_line
            .saturating_sub(height / 2)
            .min(total.saturating_sub(height));

        // Emphasis markers are hidden, except on the span the cursor sits in (so
        // it stays editable). Concealment shifts screen columns, so the cursor X
        // below must be measured over the *visible* text, not the source.
        let cursor_byte = buffer.cursor_byte();
        let conceal = buffer.syntax().conceal_ranges(Some(cursor_byte));

        // Display-only header blocks drawn above each chapter's body: a chapter
        // divider (continuous view), the book epigraph (above chapter 0), and the
        // chapter's own epigraph. Keyed by the buffer line they precede; injected
        // into the rendered rows, so the cursor's row is tracked as we go.
        let inserts = header_inserts(app, column.width);

        let mut lines: Vec<Line> = Vec::with_capacity(height);
        let mut cursor_row = None;
        let mut i = top;
        while lines.len() < height && i < total {
            if let Some((_, block)) = inserts.iter().find(|(at, _)| *at == i) {
                for line in block {
                    if lines.len() >= height {
                        break;
                    }
                    lines.push(line.clone());
                }
                if lines.len() >= height {
                    break;
                }
            }
            if i == cursor_line {
                cursor_row = Some(lines.len() as u16);
            }
            lines.push(styled_line(
                buffer.syntax(),
                buffer.line_start_byte(i),
                &buffer.line(i),
                &conceal,
            ));
            i += 1;
        }

        let paragraph = Paragraph::new(lines).block(Block::default());
        frame.render_widget(paragraph, column);

        // Editing a metadata field: a banner on the otherwise-blank top row.
        if let Mode::EditMeta(meta) = &app.mode {
            let banner = format!(" editing {} — Ctrl-S save · Esc cancel", meta.label);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    banner,
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ))),
                Rect { x: area.x, y: area.y, width: area.width, height: 1 },
            );
        }

        // Place the terminal cursor (only while editing text). The cursor's own
        // span is revealed, so it never lands on a concealed character.
        let editing = matches!(app.mode, Mode::Insert | Mode::EditMeta(_));
        if let (true, Some(row)) = (editing, cursor_row) {
            let screen_y = column.y + row;
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
}

/// The display-only header blocks to inject above the body, keyed by the buffer
/// line each precedes: per chapter, a divider (continuous view, not the first
/// chapter), the book epigraph (above chapter 0), then the chapter's epigraph.
fn header_inserts(app: &App, width: u16) -> Vec<(usize, Vec<Line<'static>>)> {
    // When editing an epigraph in the editor, the buffer *is* the epigraph —
    // don't inject chapter/book headers above it.
    if matches!(app.mode, Mode::EditMeta(_)) {
        return Vec::new();
    }
    let book_epigraph = app.project.manifest.epigraph();
    let headers = match &app.manuscript {
        Some(m) => m.segment_headers(),
        // Single-chapter view: one header for the active chapter at line 0.
        None => vec![SegmentHeader {
            start: 0,
            index: app.project.active_index(),
            title: None,
            epigraph: app.buffer.front_matter().and_then(FrontMatter::epigraph),
        }],
    };

    let mut inserts = Vec::new();
    for header in &headers {
        let mut block = Vec::new();
        if let Some(title) = &header.title {
            block.push(divider_line(title, width));
        }
        if header.index == 0 {
            if let Some(epigraph) = &book_epigraph {
                block.extend(epigraph_lines(epigraph));
            }
        }
        if let Some(epigraph) = &header.epigraph {
            block.extend(epigraph_lines(epigraph));
        }
        if !block.is_empty() {
            inserts.push((header.start, block));
        }
    }
    inserts
}

/// Display-only lines for an epigraph: the quotation (indented, italic) and its
/// attribution (`— Name`, dimmed), set off by blank lines. Never enters the
/// buffer or any file.
fn epigraph_lines(epigraph: &Epigraph) -> Vec<Line<'static>> {
    let indent = "    ";
    let mut lines = vec![Line::raw("")];
    for line in epigraph.text.lines() {
        lines.push(Line::from(Span::styled(
            format!("{indent}{line}"),
            Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
        )));
    }
    if let Some(attribution) = &epigraph.attribution {
        lines.push(Line::from(Span::styled(
            format!("{indent}— {attribution}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::raw(""));
    lines
}

/// A centered, dimmed chapter-break label, e.g. `—— Chapter Two ——`. Rendered
/// only; it never enters the buffer or any file.
fn divider_line(title: &str, width: u16) -> Line<'static> {
    let label = format!("—— {title} ——");
    let pad = (width as usize).saturating_sub(label.chars().count()) / 2;
    Line::from(Span::styled(
        format!("{}{label}", " ".repeat(pad)),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
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

impl Pane for StatusBar<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.0;
        let buffer = app.edit_buffer();

        if let Mode::Prompt(prompt_state) = &app.mode {
            // Label plus any typed text (empty for y/N confirmations) — two
            // borrowed spans, no per-frame allocation.
            let style = Style::default().fg(Color::Black).bg(Color::Yellow);
            let prompt = Paragraph::new(Line::from(vec![
                Span::styled(prompt_state.label.as_str(), style),
                Span::styled(prompt_state.input.as_str(), style),
            ]));
            frame.render_widget(prompt, area);
            return;
        }

        // While browsing, the bar describes the *selected* chapter: its title,
        // and (unless a transient message is up) its summary / POV / status.
        let (left, right) = if let Mode::Nav { selected } = app.mode {
            let title = app.project.chapter_title(selected).unwrap_or("[untitled]");
            let right = match &app.message {
                Some(m) => format!("{m}  "),
                None => {
                    let mut parts = Vec::new();
                    if let Some(s) = app.project.chapter_summary(selected) {
                        parts.push(s.to_string());
                    }
                    if let Some(p) = app.project.chapter_pov(selected) {
                        parts.push(format!("POV: {p}"));
                    }
                    if let Some(st) = app.project.chapter_status(selected) {
                        parts.push(st.label().to_string());
                    }
                    if parts.is_empty() {
                        String::new()
                    } else {
                        format!("{}  ", parts.join("  ·  "))
                    }
                }
            };
            (format!(" {title}"), right)
        } else {
            // The chapter the cursor is in: in continuous view that's the segment
            // under the cursor (with a marker); otherwise the single active chapter.
            let (name, dirtied) = match &app.manuscript {
                Some(m) => (format!("≡ {}", m.current_title()), m.dirty()),
                None => (
                    app.project.active_title().unwrap_or("[untitled]").to_string(),
                    buffer.dirty,
                ),
            };
            let dirty = if dirtied { " ●" } else { "" };
            // Persistent AI indicator (independent of the transient message, which
            // clears on the next keystroke): in-flight spinner, else an enabled tag.
            let ai_tag = if app.ai_busy() {
                " · ⟳ Claude"
            } else if app.ai.enabled {
                " · AI"
            } else {
                ""
            };
            let right = match &app.message {
                Some(m) => format!("{m}  "),
                None => format!(
                    "{} words · ln {}/{}  ",
                    buffer.word_count(),
                    buffer.cursor.line + 1,
                    buffer.line_count()
                ),
            };
            (format!(" {name}{dirty}{ai_tag}"), right)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Cursor};
    use crate::project::Project;
    use ratatui::{backend::TestBackend, Terminal};
    use std::fs;

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

    /// Performance guard (run with `cargo test --release -- --ignored
    /// --nocapture`). A full frame must render well within one 16ms frame, even
    /// for a large chapter shown alongside a many-chapter outline pane — the
    /// realistic worst case for per-frame work (the status bar's live word count
    /// over the whole buffer, plus a nav row per chapter). If a future change
    /// makes rendering re-read files or do per-chapter work proportional to the
    /// novel, this catches it.
    #[test]
    #[ignore]
    fn render_stays_within_a_frame() {
        use std::time::Instant;

        let titles: Vec<String> = (0..150).map(|i| format!("Chapter {i}")).collect();
        let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();
        let project = Project::scratch_with(&title_refs);

        let mut text = String::new();
        for _ in 0..7500 {
            text.push_str("The night was dark and the road was very long indeed.\n");
        }
        let words = text.split_whitespace().count();
        let mut buffer = Buffer::from_text(&text);
        buffer.refresh_syntax();
        let mut app = App::new(buffer, project);
        app.outline_visible = true;

        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap(); // warm
        let t = Instant::now();
        for _ in 0..30 {
            terminal.draw(|f| draw(f, &app)).unwrap();
        }
        let dt = t.elapsed() / 30;

        println!("render {words}w + 150-chapter outline = {dt:?}");
        assert!(dt.as_millis() < 16, "render took {dt:?}");
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

    #[test]
    fn nav_pane_shows_the_project_header() {
        let mut app = app_with_chapters(&["One", "Two"], "body\n");
        app.project.manifest.title = "Book of Dust".into();
        app.project.manifest.subtitle = Some("La Belle Sauvage".into());
        app.project.manifest.author = Some("Philip Pullman".into());
        app.project.manifest.series = Some("The Book of Dust".into());
        app.outline_visible = true;

        let screen = render(&app, 90, 16);
        assert!(screen.contains("Book of Dust"), "screen:\n{screen}");
        assert!(screen.contains("La Belle Sauvage"), "screen:\n{screen}");
        assert!(screen.contains("by Philip Pullman"), "screen:\n{screen}");
        assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
    }

    #[test]
    fn epigraphs_render_above_the_body() {
        // Book epigraph (manifest) + chapter epigraph (active buffer front matter)
        // both render above chapter 0's prose, with attributions.
        let mut app = app_with("# Chapter\n\nThe prose begins here.\n");
        app.project.manifest.epigraph = Some("In the beginning.".into());
        app.project.manifest.epigraph_by = Some("Genesis".into());
        let fm = app.buffer.front_matter_mut();
        fm.set_epigraph_text("A chapter quote.");
        fm.set_epigraph_by("Someone");

        let screen = render(&app, 80, 20);
        assert!(screen.contains("In the beginning."), "book epigraph:\n{screen}");
        assert!(screen.contains("— Genesis"), "book attribution:\n{screen}");
        assert!(screen.contains("A chapter quote."), "chapter epigraph:\n{screen}");
        assert!(screen.contains("— Someone"), "chapter attribution:\n{screen}");
        assert!(screen.contains("The prose begins here."), "body:\n{screen}");

        // The epigraph is display-only: it never entered the editable buffer.
        assert!(!app.buffer.line(0).contains("In the beginning"));
        assert!(!app.buffer.line(0).contains("A chapter quote"));
    }

    #[test]
    fn a_multiline_epigraph_renders_one_line_each() {
        let mut app = app_with("# Chapter\n\nBody.\n");
        let fm = app.buffer.front_matter_mut();
        fm.set_epigraph_text("Roses are red\nViolets are blue");
        fm.set_epigraph_by("Anon");

        let screen = render(&app, 80, 20);
        // Each verse line renders on its own row, attribution beneath.
        let red = screen.lines().position(|l| l.contains("Roses are red"));
        let blue = screen.lines().position(|l| l.contains("Violets are blue"));
        let by = screen.lines().position(|l| l.contains("— Anon"));
        assert!(red.is_some() && blue.is_some() && by.is_some(), "screen:\n{screen}");
        assert!(red < blue && blue < by, "lines out of order:\n{screen}");
    }

    #[test]
    fn editing_an_epigraph_shows_a_banner() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app_with("Body.\n");
        app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)); // → Nav
        app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)); // → epigraph editor
        assert!(matches!(app.mode, Mode::EditMeta(_)));

        let screen = render(&app, 80, 10);
        assert!(screen.contains("editing chapter epigraph"), "screen:\n{screen}");
        assert!(screen.contains("Ctrl-S save"), "screen:\n{screen}");
    }

    #[test]
    fn pane_shows_a_chapter_status_badge() {
        let mut app = app_with_chapters(&["One", "Two"], "body\n");
        app.project.manifest.chapters[0].status = Some(crate::frontmatter::Status::Final);
        app.outline_visible = true;
        let screen = render(&app, 90, 14);
        let row = screen.lines().find(|l| l.contains("One")).unwrap();
        assert!(row.contains('F'), "status badge missing in row: {row:?}");
    }

    #[test]
    fn nav_status_bar_shows_the_selected_chapters_metadata() {
        let mut app = app_with_chapters(&["One", "Two"], "body\n");
        app.project.manifest.chapters[1].summary = Some("A grand opening".into());
        app.project.manifest.chapters[1].pov = Some("Mara".into());
        app.project.manifest.chapters[1].status = Some(crate::frontmatter::Status::Revised);
        app.outline_visible = true;
        app.mode = Mode::Nav { selected: 1 };
        let screen = render(&app, 90, 14);
        assert!(screen.contains("A grand opening"), "screen:\n{screen}");
        assert!(screen.contains("POV: Mara"), "screen:\n{screen}");
        assert!(screen.contains("Revised"), "screen:\n{screen}");
    }

    #[test]
    fn continuous_view_shows_chapters_flowing_with_a_divider() {
        use crate::manuscript::Manuscript;

        // A two-chapter project on disk (the manuscript window reads files).
        let mut dir = std::env::temp_dir();
        dir.push(format!("quill-ui-cont-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
        )
        .unwrap();
        fs::write(dir.join("one.md"), "Alpha prose here.\n").unwrap();
        fs::write(dir.join("two.md"), "Beta prose here.\n").unwrap();

        let project = Project::open(&dir).unwrap();
        let buffer = Buffer::from_path(project.active_path()).unwrap();
        let mut app = App::new(buffer, project);
        app.manuscript = Some(Manuscript::open(&app.project));

        let screen = render(&app, 60, 12);
        // Both chapters' prose appears in one continuous flow...
        assert!(screen.contains("Alpha prose here."), "screen:\n{screen}");
        assert!(screen.contains("Beta prose here."), "screen:\n{screen}");
        // ...separated by a centered divider carrying the next chapter's title.
        assert!(screen.contains("—— Two ——"), "screen:\n{screen}");

        fs::remove_dir_all(&dir).ok();
    }
}

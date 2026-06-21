//! The prose editor pane (#150): soft-wrapped, syntax- and search-highlighted
//! rendering of the active buffer, with cursor following and concealed emphasis
//! markers. The most involved renderer, split out of `ui`. `use super::*` pulls
//! in the shared types and the `Pane` trait.

use super::*;
use crate::buffer::Buffer;

/// An absolute byte range `[start, end)` in the buffer — a search match (#144).
type ByteRange = (usize, usize);

impl Pane for Editor<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        render_focused_pane(frame, area, self.0);
    }
}

/// Render the focused editor pane (`App::buffer`) into `area`: full rendering with
/// headers, search highlighting, and the terminal cursor (#163: shared by the
/// single-pane view and the focused half of a split).
pub(super) fn render_focused_pane(frame: &mut Frame, area: Rect, app: &App) {
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
    // Feed navigation the same wrap column the screen uses (#143), so visual
    // up/down move by the rows the author actually sees.
    app.wrap_width.set(width);

    // Emphasis markers are hidden, except on the span the cursor sits in.
    let conceal = buffer.syntax().conceal_ranges(Some(buffer.cursor_byte()));
    // Find/replace highlighting (#144): every match gets a dim background, the
    // current one a bright background. Empty when not searching.
    let (search_matches, current_match): (&[ByteRange], Option<ByteRange>) = match &app.mode {
        Mode::Search(s) => (&s.matches, s.current_match()),
        _ => (&[], None),
    };
    let inserts = header_inserts(app, column.width);
    let editing = matches!(app.mode, Mode::Insert | Mode::EditMeta(_));
    render_body(frame, column, buffer, &inserts, &conceal, search_matches, current_match, editing);

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
}

/// Render a non-focused reference pane (#163): the other pane's prose under a
/// dimmed title bar naming its chapter — no cursor, no search highlight.
pub(super) fn render_reference_pane(frame: &mut Frame, area: Rect, app: &App, buffer: &Buffer) {
    let title = app.chapter_title_for(buffer).unwrap_or("reference");
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {title}"),
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        ))),
        Rect { x: area.x, y: area.y, width: area.width, height: 1 },
    );
    let body = Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height.saturating_sub(1) };
    let width = app.focus_width.min(body.width.saturating_sub(2));
    let left_pad = body.width.saturating_sub(width) / 2;
    let column = Rect { x: body.x + left_pad, y: body.y, width, height: body.height };
    let conceal = buffer.syntax().conceal_ranges(Some(buffer.cursor_byte()));
    render_body(frame, column, buffer, &[], &conceal, &[], None, false);
}

/// Render `buffer`'s soft-wrapped, styled prose into `column`, following the
/// cursor by visual row. Places the terminal cursor when `place_cursor`. The
/// shared core of both pane renderers (#163).
#[allow(clippy::too_many_arguments)]
fn render_body(
    frame: &mut Frame,
    column: Rect,
    buffer: &Buffer,
    inserts: &[(usize, Vec<Line<'static>>)],
    conceal: &[(usize, usize)],
    matches: &[ByteRange],
    current: Option<ByteRange>,
    place_cursor: bool,
) {
    let height = column.height as usize;
    let wrap = column.width as usize;
    let header_rows = |line: usize| inserts.iter().find(|(at, _)| *at == line).map_or(0, |(_, b)| b.len());
    let body_rows = |line: usize| crate::wrap::row_starts(&buffer.line(line), wrap).len();

    let total = buffer.line_count();
    let cursor_line = buffer.cursor.line;
    let cursor_byte = buffer.cursor_byte();

    // Where the cursor sits among its own line's visual rows.
    let cline = buffer.line(cursor_line);
    let cline_start = buffer.line_start_byte(cursor_line);
    let cstarts = crate::wrap::row_starts(&cline, wrap);
    let (cursor_subrow, _) = crate::wrap::visual_pos(&cline, &cstarts, cursor_byte - cline_start);

    // Follow the cursor by *visual* row, centered but clamped so we never scroll
    // past the top or leave blank space at the bottom. Bounded by the viewport.
    let target_above = height / 2;
    let mut above_avail = header_rows(cursor_line) + cursor_subrow;
    let mut l = cursor_line;
    while l > 0 && above_avail < height {
        l -= 1;
        above_avail += header_rows(l) + body_rows(l);
    }
    let mut below_avail = body_rows(cursor_line) - 1 - cursor_subrow;
    let mut l = cursor_line;
    while l + 1 < total && below_avail < height {
        l += 1;
        below_avail += header_rows(l) + body_rows(l);
    }
    let keep_above = target_above
        .max((height.saturating_sub(1)).saturating_sub(below_avail))
        .min(above_avail);

    let mut acc = header_rows(cursor_line) + cursor_subrow;
    let mut top = cursor_line;
    while acc < keep_above && top > 0 {
        top -= 1;
        acc += header_rows(top) + body_rows(top);
    }
    let mut to_skip = acc.saturating_sub(keep_above);

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let mut cursor_row = None;
    let mut ll = top;
    'fill: while lines.len() < height && ll < total {
        if let Some((_, block)) = inserts.iter().find(|(at, _)| *at == ll) {
            for line in block {
                if to_skip > 0 {
                    to_skip -= 1;
                } else if lines.len() < height {
                    lines.push(line.clone());
                } else {
                    break 'fill;
                }
            }
        }
        let body = styled_line(
            buffer.syntax(),
            buffer.line_start_byte(ll),
            &buffer.line(ll),
            conceal,
            matches,
            current,
            wrap,
        );
        for (sub, line) in body.into_iter().enumerate() {
            if to_skip > 0 {
                to_skip -= 1;
                continue;
            }
            if lines.len() >= height {
                break 'fill;
            }
            if ll == cursor_line && sub == cursor_subrow {
                cursor_row = Some(lines.len() as u16);
            }
            lines.push(line);
        }
        ll += 1;
    }

    frame.render_widget(Paragraph::new(lines).block(Block::default()), column);

    // Place the terminal cursor (focused, editing panes only). Its span is
    // revealed, so it never lands on a concealed character.
    if place_cursor {
        if let Some(row) = cursor_row {
            let screen_y = column.y + row;
            let row_start = cstarts[cursor_subrow];
            let col_width = visible_width_before(&cline[row_start..], cline_start + row_start, cursor_byte, conceal);
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

/// Soft-wrap a logical line into its visual rows (#143) and style each. `text` is
/// one line (no newline); `line_start` its absolute byte offset (highlights,
/// conceal, and match ranges use document offsets). Returns one [`Line`] per
/// wrapped row.
fn styled_line(
    syntax: &Syntax,
    line_start: usize,
    text: &str,
    conceal: &[(usize, usize)],
    matches: &[ByteRange],
    current: Option<ByteRange>,
    width: usize,
) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::raw("")];
    }
    let starts = crate::wrap::row_starts(text, width);
    starts
        .iter()
        .enumerate()
        .map(|(i, &rs)| {
            let re = starts.get(i + 1).copied().unwrap_or(text.len());
            styled_row(syntax, line_start, text, rs, re, conceal, matches, current)
        })
        .collect()
}

/// Style one visual row — the byte sub-range `[rs, re)` of `text` — into runs of
/// equal style, dropping concealed (hidden-marker) characters.
#[allow(clippy::too_many_arguments)]
fn styled_row(
    syntax: &Syntax,
    line_start: usize,
    text: &str,
    rs: usize,
    re: usize,
    conceal: &[(usize, usize)],
    matches: &[ByteRange],
    current: Option<ByteRange>,
) -> Line<'static> {
    let seg = &text[rs..re];
    if seg.is_empty() {
        return Line::raw("");
    }
    let seg_start = line_start + rs;
    let seg_end = line_start + re;
    let hls: Vec<Highlight> = syntax.highlights_in(seg_start, seg_end).copied().collect();
    let concealed_here = conceal.iter().any(|&(s, e)| s < seg_end && e > seg_start);
    let match_here = matches.iter().any(|&(s, e)| s < seg_end && e > seg_start);
    if hls.is_empty() && !concealed_here && !match_here {
        return Line::raw(seg.to_string());
    }

    // Build runs character by character, since concealed chars are dropped and
    // visible runs may not be contiguous in the source.
    let mut spans: Vec<Span> = Vec::new();
    let mut run = String::new();
    let mut run_style = Style::default();
    for (local_byte, ch) in seg.char_indices() {
        let abs = seg_start + local_byte;
        if is_concealed(abs, conceal) {
            continue;
        }
        let style = search_style(style_at(&hls, abs), abs, matches, current);
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

/// Layer search highlighting onto a character's base style: the current match
/// gets a bright background, any other match a dim one (#144).
fn search_style(
    base: Style,
    byte: usize,
    matches: &[ByteRange],
    current: Option<ByteRange>,
) -> Style {
    if let Some((s, e)) = current {
        if byte >= s && byte < e {
            return base.bg(Color::Yellow).fg(Color::Black);
        }
    }
    if matches.iter().any(|&(s, e)| byte >= s && byte < e) {
        return base.bg(Color::DarkGray);
    }
    base
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

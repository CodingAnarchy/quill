//! The authorial assistant pane (#307): the conversation transcript above a typed
//! request line, in a bordered box that brightens when it has key focus. Split out
//! of `ui`; `use super::*` pulls in the shared types and constants.

use super::*;

/// Render the authorial assistant pane (#307): the conversation transcript above a
/// typed request line, in a bordered box that brightens when it has key focus. The
/// transcript is pre-wrapped to the interior width so scrolling is line-exact.
pub(super) fn draw_assistant_pane(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    conv: &Assistant,
    focused: bool,
) {
    let theme = &app.theme;
    // Title carries the live state and the run's running cost (#309).
    let mut parts: Vec<String> = Vec::new();
    if conv.is_running() {
        parts.push("thinking… (Esc cancels)".into());
    }
    if conv.spent() > 0.0 {
        parts.push(format!("~${:.2}", conv.spent()));
    }
    let title = if parts.is_empty() {
        " Assistant ".to_string()
    } else {
        format!(" Assistant — {} ", parts.join(" · "))
    };
    let border = if focused { theme.heading } else { theme.dim };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // The bottom interior row is the request line; the rest is the transcript.
    let body_h = inner.height.saturating_sub(1) as usize;
    let lines = wrap_entries(conv.entries(), inner.width as usize, theme);
    let max_top = lines.len().saturating_sub(body_h);
    app.assistant_scroll_max.set(max_top as u16);
    let top = max_top.saturating_sub(conv.scroll_up() as usize);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(body_h).collect();
    frame.render_widget(
        Paragraph::new(shown),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_h as u16,
        },
    );

    // The request line, with the cursor placed at its end while focused and idle.
    let input_y = inner.y + body_h as u16;
    let prompt = if conv.is_running() { "… " } else { "> " };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prompt, Style::default().fg(theme.dim)),
            Span::styled(conv.input().to_string(), Style::default().fg(theme.heading)),
        ])),
        Rect {
            x: inner.x,
            y: input_y,
            width: inner.width,
            height: 1,
        },
    );
    if focused && !conv.is_running() {
        let col = (prompt.width() + conv.input().width()) as u16;
        let cx = inner.x + col.min(inner.width.saturating_sub(1));
        frame.set_cursor_position((cx, input_y));
    }
}

/// Lay the assistant transcript out as styled rows, each entry labelled by speaker
/// and word-wrapped to `width`, with a blank row between entries for readability.
fn wrap_entries(entries: &[crate::app::Entry], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for e in entries {
        let (label, style) = match e.who {
            Speaker::You => (
                "You: ",
                Style::default()
                    .fg(theme.heading)
                    .add_modifier(Modifier::BOLD),
            ),
            Speaker::Assistant => ("", Style::default()),
            Speaker::Tool => ("• ", Style::default().fg(theme.dim)),
            Speaker::System => (
                "",
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::ITALIC),
            ),
        };
        let body = format!("{label}{}", e.text);
        let starts = crate::wrap::row_starts(&body, width);
        for r in 0..starts.len() {
            let s = starts[r];
            let end = starts.get(r + 1).copied().unwrap_or(body.len());
            out.push(Line::from(Span::styled(
                body[s..end].trim_end().to_string(),
                style,
            )));
        }
        out.push(Line::raw(""));
    }
    out
}

/// Whether a split along `dir` would leave panes too small to use in `area`.
pub(super) fn too_cramped_to_split(dir: SplitDir, area: Rect) -> bool {
    match dir {
        SplitDir::SideBySide => area.width < MIN_PANE_WIDTH * 2,
        SplitDir::Stacked => area.height < MIN_PANE_HEIGHT * 2,
    }
}

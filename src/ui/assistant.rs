//! The authorial assistant pane (#307): the conversation transcript above a typed
//! request line, in a bordered box that brightens when it has key focus. Split out
//! of `ui`; `use super::*` pulls in the shared types and constants.

use std::rc::Rc;

use super::*;

/// The cache key for the wrapped transcript (#307): the wrap output changes only
/// when an entry is appended (entries are append-only, never edited in place), the
/// interior width changes (a resize), or the theme switches (it colors the rows).
/// Comparing this cheap signature lets an open pane reuse the pre-wrapped rows
/// instead of re-wrapping every entry (a `format!` + `row_starts` + a per-row
/// `to_string`) for the whole conversation each frame — mirroring `HeaderCache`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct WrapSignature {
    entries: usize,
    width: usize,
    theme: Theme,
}

/// Cached wrapped transcript rows and the signature they were built for, stored on
/// the [`Assistant`] and consulted by [`draw_assistant_pane`] each frame. Shared
/// via `Rc` so a cache hit is a refcount bump, never a re-wrap.
#[derive(Default)]
pub struct AssistantWrapCache {
    signature: Option<WrapSignature>,
    lines: Rc<Vec<Line<'static>>>,
}

impl AssistantWrapCache {
    /// The identity of the currently cached rows (#307), for tests to assert a
    /// frame reused them (same pointer) or rebuilt them (different).
    #[cfg(test)]
    pub(crate) fn identity(&self) -> *const Vec<Line<'static>> {
        Rc::as_ptr(&self.lines)
    }
}

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
    let lines = wrapped_lines(conv, inner.width as usize, theme);
    let max_top = lines.len().saturating_sub(body_h);
    app.assistant_scroll_max.set(max_top as u16);
    let top = max_top.saturating_sub(conv.scroll_up() as usize);
    let shown: Vec<Line> = lines.iter().skip(top).take(body_h).cloned().collect();
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

/// The wrapped transcript rows for this frame, reusing the cache when the entry
/// count, width, and theme are unchanged (#307). Only an appended entry, a resize,
/// or a theme switch re-wraps — an otherwise-untouched open pane is a refcount
/// bump. The output is byte-identical to calling [`wrap_entries`] every frame.
fn wrapped_lines(conv: &Assistant, width: usize, theme: &Theme) -> Rc<Vec<Line<'static>>> {
    let signature = WrapSignature {
        entries: conv.entries().len(),
        width,
        theme: *theme,
    };
    let mut cache = conv.wrap_cache().borrow_mut();
    if cache.signature != Some(signature) {
        cache.lines = Rc::new(wrap_entries(conv.entries(), width, theme));
        cache.signature = Some(signature);
    }
    Rc::clone(&cache.lines)
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
pub(super) const fn too_cramped_to_split(dir: SplitDir, area: Rect) -> bool {
    match dir {
        SplitDir::SideBySide => area.width < MIN_PANE_WIDTH * 2,
        SplitDir::Stacked => area.height < MIN_PANE_HEIGHT * 2,
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::buffer::Buffer;
    use crate::project::Project;
    use crate::ui::render_to_string as render;

    fn app_with_assistant() -> App {
        // `from_text` leaves the buffer clean (not dirty), so opening the pane —
        // which flushes the active chapter first — doesn't try to save a pathless
        // scratch buffer and bail.
        let mut buffer = Buffer::from_text("# Chapter\n\nBody text here.\n");
        buffer.refresh_syntax();
        let mut app = App::new(buffer, Project::scratch());
        app.test_open_assistant_with_entries();
        app
    }

    #[test]
    fn wrapped_transcript_is_cached_across_frames_and_invalidates_on_append() {
        // The assistant transcript is pre-wrapped once and reused per frame (#307):
        // an untouched pane reuses the same allocation and renders byte-identically;
        // appending an entry rebuilds it and shows the new line — mirroring the
        // header-block cache (#869).
        let mut app = app_with_assistant();

        let first = render(&app, 80, 20);
        let id1 = app.test_assistant_wrap_identity();
        let second = render(&app, 80, 20);
        let id2 = app.test_assistant_wrap_identity();
        assert_eq!(first, second, "an unchanged pane renders identically");
        assert_eq!(id1, id2, "an unchanged frame reuses the wrapped transcript");

        // Appending an entry invalidates the cache: the rows rebuild and the new
        // text shows on screen.
        app.test_assistant_append("a brand new reply line");
        let changed = render(&app, 80, 20);
        let id3 = app.test_assistant_wrap_identity();
        assert_ne!(
            id2, id3,
            "an appended entry rebuilds the wrapped transcript"
        );
        assert!(
            changed.contains("a brand new reply line"),
            "the new entry shows:\n{changed}"
        );
    }

    #[test]
    fn resizing_the_pane_rewraps_the_transcript() {
        // A resize changes the interior width, so the cached rows are invalidated
        // and re-wrapped to the new column (the wrap is width-keyed).
        let app = app_with_assistant();
        render(&app, 80, 20);
        let wide = app.test_assistant_wrap_identity();
        render(&app, 50, 20);
        let narrow = app.test_assistant_wrap_identity();
        assert_ne!(wide, narrow, "a resize re-wraps the transcript");
    }
}

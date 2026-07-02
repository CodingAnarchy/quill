//! The full-screen and box overlay renderers: the scrollable report/AI/guide
//! overlay, the story-bible conflict review, and the series tree navigator. Split
//! out of `ui`; `use super::*` pulls in the shared types and helpers.

use super::*;

/// Render the open full-screen overlay in a bordered box, scrolled and clamped to
/// its content. Report overlays show aligned tables (no wrap) titled with the
/// book + a label; the AI panel word-wraps its prose answer. One renderer for all
/// overlay kinds — a new overlay is a new [`OverlayKind`], not a new draw arm.
pub(super) fn draw_overlay(frame: &mut Frame, area: Rect, book: &str, overlay: &Overlay) {
    let lines = &overlay.lines;
    let scroll = overlay.scroll.min(max_scroll(lines.len(), area.height));
    let hint = scroll_hint(lines.len(), area.height);
    let (title, wrap) = match overlay.kind {
        // A report overlay can save its snapshot to a file (#601), so its hint
        // leads with the `s` key before the shared scroll/close hint.
        OverlayKind::Report(label) => (format!(" {book} — {label} (s save · {hint}) "), false),
        OverlayKind::AiPanel => (format!(" AI — Claude ({hint}) "), true),
        // The guide ships pre-wrapped to a readable width; keep it unwrapped so its
        // key-reference columns stay aligned and the scroll bound stays exact.
        OverlayKind::Guide => (format!(" Quill — user's guide ({hint}) "), false),
        // The pre-commit review (#704) confirms rather than dismisses, so its hint
        // names the commit/cancel keys (not "any key to close"); the scroll marker
        // is added only when the content is actually taller than the box.
        OverlayKind::CommitReview => {
            let scroll = if max_scroll(lines.len(), area.height) > 0 {
                " · ↑↓ scroll"
            } else {
                ""
            };
            (
                format!(" Review commit — enter commit · esc cancel{scroll} "),
                false,
            )
        }
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let mut paragraph = Paragraph::new(lines.clone())
        .block(block)
        .scroll((scroll, 0));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, area);
}

/// The largest valid scroll offset for `lines` of content in a bordered box of
/// `height` rows: enough to bring the last line into view (the interior is the
/// height minus the two borders), zero when it already fits. The single source
/// of the offset bound, shared by the renderer and the key handler.
pub(crate) fn max_scroll(lines: usize, height: u16) -> u16 {
    (lines as u16).saturating_sub(height.saturating_sub(2))
}

/// The title hint for a scrollable overlay — mentions scrolling only when the
/// content is actually taller than the box.
fn scroll_hint(lines: usize, height: u16) -> &'static str {
    if max_scroll(lines, height) > 0 {
        "↑↓ scroll · any other key to close"
    } else {
        "any key to close"
    }
}

/// Render the story-bible conflict review: a diff of the front conflict (or the
/// scratch editor when editing the proposed version).
pub(super) fn draw_bible_review(frame: &mut Frame, area: Rect, review: &BibleReview) {
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
            Paragraph::new(lines)
                .block(block)
                .wrap(Wrap { trim: false }),
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
    let theme = theme::current();
    for change in &front.diff {
        let color = match change.chars().next() {
            Some('-') => theme.removed,
            Some('+') => theme.added,
            _ => theme.dim,
        };
        lines.push(Line::from(Span::styled(
            change.clone(),
            Style::default().fg(color),
        )));
    }

    let block = Block::default().borders(Borders::ALL).title(format!(
        " Bible conflict: {} ({remaining} left) — (o)verwrite (e)dit (k)eep · Esc cancel ",
        front.name
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// The series tree navigator (#330): the project as an indented series → book →
/// part → chapter tree. Rows are derived from cached metadata (never files); the
/// selected row is reversed and the active book/chapter is accented.
pub(super) fn draw_tree(frame: &mut Frame, area: Rect, app: &App, tree: &Tree) {
    let rows = app.tree_rows(&tree.expanded);
    let sel = tree.selected.min(rows.len().saturating_sub(1));
    let title = format!(" {} ", app.project.series_title().unwrap_or("Project"));
    let lines: Vec<Line<'static>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let text = format!(
                "{}{}{}",
                marker(i == sel),
                "  ".repeat(r.indent as usize),
                r.label
            );
            let style = if i == sel {
                row_style(true)
            } else if r.active {
                active_style()
            } else {
                Style::default()
            };
            Line::from(Span::styled(text, style))
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        &title,
        "Project tree — ↑↓ move · → expand · ← collapse · enter open / expand · esc close",
        lines,
        Some(sel),
    );
}

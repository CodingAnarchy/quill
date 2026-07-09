//! The sync-conflict resolution overlay (#963): for each chapter a two-way sync
//! (#962) found changed on both machines, a local↔remote diff with keep-local /
//! take-remote / merge choices — or, while merging, the scratch editor seeded
//! with both versions. Mirrors the bible-review diff styling (additions green,
//! removals red, context dimmed); resolving every conflict completes the sync.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::mode::ConflictReview;
use crate::theme;

/// Render the conflict-resolution overlay for the front conflicting chapter (#963).
/// The diff view leads: a local→remote unified diff of the chapter both machines
/// changed, so the author sees exactly how the two versions differ before choosing
/// to keep local, take remote, or hand-merge. While merging, the scratch editor
/// (seeded with both versions) takes over. No-op once the queue has drained.
pub(in crate::ui) fn draw_conflict_review(frame: &mut Frame, area: Rect, review: &ConflictReview) {
    let Some(front) = review.conflicts.first() else {
        return;
    };
    let remaining = review.conflicts.len();

    // Merging: the scratch buffer seeded with both versions, edited into the final.
    if let Some(buf) = &review.editing {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" Merge {} — Ctrl-S accept · Esc back ", front.path));
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

    // Diff view: local → remote for the conflicting chapter.
    let theme = theme::current();
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "'{}' was changed on both machines — choose what to keep:",
                front.path
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  (-) your local version   (+) the remote version".to_string(),
            Style::default().fg(theme.dim),
        )),
        Line::raw(""),
    ];
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
        " Sync conflict: {} ({remaining} left) — keep (l)ocal · take (r)emote · (m)erge · Esc cancel ",
        front.path
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

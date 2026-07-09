//! The import-preview overlays: the manuscript-import preview (#702) and the
//! review-comment import preview (#886). Each pins a non-selectable summary
//! header at the top and scrolls the entry list beneath it by re-basing the list
//! overlay's `selected` index past the header block.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    Frame,
};

use super::{draw_list_overlay, list_row};
use crate::mode::{ImportPreview, ReviewImport};
use crate::theme;

/// Render the manuscript-import preview (#702): a summary header (chapter count,
/// the heading level the split used, book title/author, any dropped-element
/// warnings) then the list of chapters that will be created — shown so the author
/// can catch a mis-split before anything is written. Enter confirms, Esc cancels;
/// a Markdown source's split level can be toggled with `1`–`6`.
///
/// The header rows are non-selectable; the chapter list scrolls under them by
/// re-basing the list overlay's `selected` index past the header, so a long list
/// stays navigable while the summary stays pinned at the top.
pub(in crate::ui) fn draw_import_preview(frame: &mut Frame, area: Rect, preview: &ImportPreview) {
    let dim = Style::default().fg(theme::current().dim);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let n = preview.chapters.len();
    let title = " Import preview ".to_string();

    // The summary header, above the chapter list. Non-selectable rows.
    let mut header = vec![Line::from(Span::styled(
        format!(
            "{n} chapter{} will be created:",
            if n == 1 { "" } else { "s" }
        ),
        bold,
    ))];
    if let Some(book) = &preview.title {
        let by = preview
            .author
            .as_deref()
            .map(|a| format!("  ·  by {a}"))
            .unwrap_or_default();
        header.push(Line::from(Span::styled(format!("  {book}{by}"), dim)));
    }
    if let Some(level) = preview.level {
        let toggle = if preview.levels.len() > 1 {
            "  (press 1–6 to re-split on another heading level)"
        } else {
            ""
        };
        header.push(Line::from(Span::styled(
            format!("  split on heading level {level}{toggle}"),
            dim,
        )));
    }
    if !preview.warning.is_empty() {
        // The warning suffix is `" (…)"`; trim to the parenthetical for the header.
        header.push(Line::from(Span::styled(
            format!("  warnings:{}", preview.warning),
            dim,
        )));
    }
    header.push(Line::raw(""));

    let header_len = header.len();
    let chapter_rows = preview
        .chapters
        .iter()
        .enumerate()
        .map(|(i, (t, _))| list_row(i == preview.scroll, &format!("{}. {t}", i + 1), None));
    header.extend(chapter_rows);

    let levels = if preview.levels.len() > 1 {
        " · 1–6 change split level"
    } else {
        ""
    };
    let help = format!("Import preview — ↑↓ scroll{levels} · enter import · esc cancel");
    // Pin the scroll to the selected chapter row, past the header block.
    draw_list_overlay(
        frame,
        area,
        &title,
        &help,
        header,
        Some(header_len + preview.scroll),
    );
}

/// Render the review-comment import preview (#886): the reviewer's Word comments
/// recovered from a marked-up `.docx`, split into those that matched the active
/// chapter's prose (which will become annotations on merge) and the unmatched ones
/// (whose commented phrase no longer appears — the prose drifted), reported rather
/// than silently dropped. Enter merges the matches, Esc cancels.
pub(in crate::ui) fn draw_review_import(frame: &mut Frame, area: Rect, preview: &ReviewImport) {
    use crate::text::ellipsize;

    let dim = Style::default().fg(theme::current().dim);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let m = preview.matched.len();
    let d = preview.dropped.len();
    let title = " Import review comments ".to_string();

    let mut header = vec![Line::from(Span::styled(
        format!(
            "{m} comment{} will be added as annotations{}:",
            if m == 1 { "" } else { "s" },
            if d == 0 {
                String::new()
            } else {
                format!("  ·  {d} unmatched (the prose changed)")
            }
        ),
        bold,
    ))];
    header.push(Line::raw(""));
    let header_len = header.len();

    // The matched comments first (each becomes an annotation), then the dropped
    // ones, dimmed and flagged so the author sees what didn't carry.
    let mut i = 0;
    for (_, _, note, reviewer) in &preview.matched {
        // Attribute an imported comment to its named reviewer (#933) so a
        // multi-editor merge shows who left each note.
        let who = reviewer
            .as_deref()
            .map(|r| format!("  ({r})"))
            .unwrap_or_default();
        header.push(list_row(
            i == preview.scroll,
            &format!("+ {}{who}", ellipsize(note, 60)),
            None,
        ));
        i += 1;
    }
    for (phrase, note) in &preview.dropped {
        let text = format!(
            "✗ {}  (no match for \"{}\")",
            ellipsize(note, 40),
            ellipsize(phrase, 20)
        );
        header.push(Line::from(Span::styled(text, dim)));
        i += 1;
    }
    let _ = i;

    let help = "Review import — ↑↓ scroll · enter merge · esc cancel";
    draw_list_overlay(
        frame,
        area,
        &title,
        help,
        header,
        Some(header_len + preview.scroll),
    );
}

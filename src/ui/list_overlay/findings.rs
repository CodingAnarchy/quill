//! The analysis-surface overlays: the revision-triage findings list (#316), the
//! manuscript-health dashboard (#827), and the motif / character co-occurrence
//! report (#922). Each is a click-selectable list, so all draw through
//! [`draw_list_overlay_recording`] with a row-geometry recorder.

use std::cell::RefCell;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    Frame,
};

use super::{draw_list_overlay_recording, list_row};
use crate::app::ListLayout;
use crate::mode::{CoOccurrence, CoOccurrenceRow, Dashboard, DashboardRow, Findings};
use crate::project::Project;
use crate::theme;

/// Render the findings as a navigable triage list. `recorder` captures the row
/// geometry for click-to-select (#812).
pub(in crate::ui) fn draw_findings(
    frame: &mut Frame,
    area: Rect,
    findings: &Findings,
    project: &Project,
    recorder: &RefCell<ListLayout>,
) {
    // The type-to-filter query (#822) narrows the list to findings whose summary
    // or chapters match; the row index is preserved so the selection marker and
    // the click map land on the right finding.
    let query = findings.filter.query();
    let matches = |item: &crate::mode::Finding| {
        query.is_empty()
            || item.summary.to_lowercase().contains(&query)
            || item
                .chapters
                .iter()
                .any(|c| c.to_lowercase().contains(&query))
    };
    let visible: Vec<(usize, &crate::mode::Finding)> = findings
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches(item))
        .collect();
    let selected_pos = visible.iter().position(|(i, _)| *i == findings.selected);
    let title = format!(" Findings ({}) ", findings.items.len());
    let rows = visible
        .iter()
        .map(|(i, item)| {
            // In a series, a finding carries the book it was found in (#341); show
            // it so cross-book findings are attributed (chapter titles can repeat).
            let book = item
                .book
                .filter(|_| project.book_count() > 1)
                .and_then(|b| project.book_title(b));
            let chapters = item.chapters.join(", ");
            let loc = match (book, chapters.is_empty()) {
                (None, true) => String::new(),
                (None, false) => format!("  [{chapters}]"),
                (Some(t), true) => format!("  [{t}]"),
                (Some(t), false) => format!("  [{t} · {chapters}]"),
            };
            // The cross-pass agreement signal (#921) trails the location when more
            // than one pass flagged this span ("flagged by continuity + plot").
            let signal = item
                .agreement_signal()
                .map(|s| format!("  · {s}"))
                .unwrap_or_default();
            list_row(
                *i == findings.selected,
                &format!("{}{loc}{signal}", item.summary),
                None,
            )
        })
        .collect();
    let help = if findings.filter.typing {
        format!("Filter: {}_", findings.filter.text)
    } else if findings.filter.text.is_empty() {
        "Findings — ↑↓ move · enter jump · i mark xxx · r re-check · R re-check all · d dismiss · s save · / filter · esc close".to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears",
            findings.filter.text,
            visible.len(),
            if visible.len() == 1 { "" } else { "es" }
        )
    };
    draw_list_overlay_recording(
        frame,
        area,
        &title,
        &help,
        rows,
        selected_pos,
        Some(recorder),
    );
}

/// Render the manuscript-health dashboard (#827): open findings grouped by
/// kind/pass then chapter, each group headed by its count, with the individual
/// findings as selectable/jumpable rows. Kind headers are bold; chapter
/// sub-headers are dimmed; only finding rows carry the selection marker. `recorder`
/// captures the row geometry for click-to-select — a click on a header row is
/// ignored by the handler, keeping the selection on a jump target.
pub(in crate::ui) fn draw_dashboard(
    frame: &mut Frame,
    area: Rect,
    dashboard: &Dashboard,
    recorder: &RefCell<ListLayout>,
) {
    let title = format!(" Manuscript health ({} open) ", dashboard.total);
    let rows: Vec<Line<'static>> = dashboard
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| match row {
            DashboardRow::Kind { label, count } => Line::from(Span::styled(
                format!("{label}  ({count})"),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            DashboardRow::Chapter { title, count } => Line::from(Span::styled(
                format!("  {title}  ({count})"),
                Style::default().fg(theme::current().dim),
            )),
            DashboardRow::Finding(f) => {
                // The cross-pass agreement signal (#921) trails the summary when
                // more than one pass flagged the span ("flagged by continuity + plot").
                let signal = f
                    .agreement_signal()
                    .map(|s| format!("  · {s}"))
                    .unwrap_or_default();
                list_row(
                    i == dashboard.selected,
                    &format!("    {}{signal}", f.summary),
                    None,
                )
            }
        })
        .collect();
    // The scroll window keeps the selected finding row in view; the marker is
    // already baked into that row above, so this only drives scrolling.
    let selected_pos = Some(dashboard.selected);
    let help = "Health — ↑↓ move · enter jump · s save · esc close".to_string();
    draw_list_overlay_recording(
        frame,
        area,
        &title,
        &help,
        rows,
        selected_pos,
        Some(recorder),
    );
}

/// Render the motif / character co-occurrence report (#922) as a navigable list:
/// a bold section header, then each co-occurring pair / notable absence as a
/// jumpable row. The selected jumpable row is the one Enter jumps to.
pub(in crate::ui) fn draw_cooccurrence(
    frame: &mut Frame,
    area: Rect,
    report: &CoOccurrence,
    recorder: &RefCell<ListLayout>,
) {
    let rows: Vec<Line<'static>> = report
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| match row {
            CoOccurrenceRow::Header(label) => Line::from(Span::styled(
                (*label).to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            CoOccurrenceRow::Pair { line, .. } | CoOccurrenceRow::Absence { line, .. } => {
                list_row(i == report.selected, &format!("  {line}"), None)
            }
        })
        .collect();
    let help = "Co-occurrence — ↑↓ move · enter jump · s save · esc close".to_string();
    draw_list_overlay_recording(
        frame,
        area,
        " Motif & character co-occurrence ",
        &help,
        rows,
        Some(report.selected),
        Some(recorder),
    );
}

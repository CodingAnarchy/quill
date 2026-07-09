//! The revision-history overlay (#587): named milestones (#833) lead the list,
//! above the commits. A click-selectable list, drawn through
//! [`draw_list_overlay_recording`] with a row-geometry recorder.

use std::cell::RefCell;

use ratatui::{layout::Rect, Frame};

use super::{draw_list_overlay_recording, list_row};
use crate::app::ListLayout;
use crate::mode::{History, HistoryRow};

/// Render the history as a navigable list (#587): named milestones (#833) lead the
/// list, each labeled "◆ <name>" with its note (#966) when it has one, above the
/// commits, each a commit's date, short id, and summary. The selected row is the one
/// the chapter- and project-scoped diff / restore actions address.
pub(in crate::ui) fn draw_history(
    frame: &mut Frame,
    area: Rect,
    history: &History,
    chapter: &str,
    recorder: &RefCell<ListLayout>,
) {
    let title = format!(" History — {chapter} ");
    // The type-to-filter query (#822) narrows to rows whose text matches; the row
    // index is preserved so the marker and click map align. The combined milestone
    // + commit list (#833) is the single sequence the selection indexes into.
    let query = history.filter.to_lowercase();
    let all = history.rows();
    let label = |row: &HistoryRow| match row {
        // The tag falls back to its name when created without a note (#966), so
        // only a note distinct from the name is worth showing beside the milestone.
        HistoryRow::Milestone(t) if !t.message.is_empty() && t.message != t.name => {
            format!("◆ {}  {}  — {}", t.name, t.when, t.message)
        }
        HistoryRow::Milestone(t) => format!("◆ {}  {}", t.name, t.when),
        HistoryRow::Commit(c) => format!("{}  {}  {}", c.when, c.id, c.summary),
    };
    let visible: Vec<(usize, &HistoryRow)> = all
        .iter()
        .enumerate()
        .filter(|(_, r)| r.matches(&query))
        .collect();
    let selected_pos = visible.iter().position(|(i, _)| *i == history.selected);
    let rows = visible
        .iter()
        .map(|(i, r)| list_row(*i == history.selected, &label(r), None))
        .collect();
    let help = if history.typing {
        format!("Filter: {}_", history.filter)
    } else if history.filter.is_empty() {
        "History — ↑↓ move · d/r chapter · D/R project · n note · / filter · esc close".to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears",
            history.filter,
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

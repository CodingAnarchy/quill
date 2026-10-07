//! The revision-history overlay (#587): named milestones (#833) lead the list,
//! above the commits. A click-selectable list, drawn through
//! [`draw_list_overlay_recording`] with a row-geometry recorder.

use std::cell::RefCell;
use std::fmt::Write;

use ratatui::{layout::Rect, Frame};

use super::{draw_list_overlay_recording, list_row};
use crate::app::ListLayout;
use crate::mode::{History, HistoryRow};

/// Render the history as a navigable list with a branch/merge graph (#587/#964):
/// named milestones (#833) lead the list, each labeled "◆ <name>" with its note
/// (#966) when it has one, above the commits. Each commit row carries a Unicode
/// graph column — its lane's `●` and the active branch lines — before the date,
/// short id, and summary, so the revision topology (branches, merges) reads
/// visually; the chapters it changed (#964) trail as dimmed secondary text. The
/// selected row is the one the chapter- and project-scoped diff / restore actions
/// address.
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
    let query = history.filter.query();
    let all = history.rows();
    // A row's primary label and its dimmed secondary text. A milestone's secondary
    // is its note (#966); a commit's is the chapters it touched (#964), and its
    // primary leads with the graph column so the branch/merge topology draws.
    let render = |row: &HistoryRow| -> (String, Option<String>) {
        match row {
            // The tag falls back to its name when created without a note (#966), so
            // only a note distinct from the name is worth showing beside a milestone.
            HistoryRow::Milestone(t) if !t.message.is_empty() && t.message != t.name => {
                (format!("◆ {}  {}", t.name, t.when), Some(t.message.clone()))
            }
            HistoryRow::Milestone(t) => (format!("◆ {}  {}", t.name, t.when), None),
            HistoryRow::Commit(c) => (
                format!("{}{}  {}  {}", c.graph, c.when, c.id, c.summary),
                summarize_changed(&c.changed),
            ),
        }
    };
    let visible: Vec<(usize, &HistoryRow)> = all
        .iter()
        .enumerate()
        .filter(|(_, r)| r.matches(&query))
        .collect();
    let selected_pos = visible.iter().position(|(i, _)| *i == history.selected);
    let rows = visible
        .iter()
        .map(|(i, r)| {
            let (primary, secondary) = render(r);
            list_row(*i == history.selected, &primary, secondary)
        })
        .collect();
    let help = if history.filter.typing {
        format!("Filter: {}_", history.filter.text)
    } else if history.findings_anchor.is_some() {
        // Mid findings-diff (#1051): point A is picked; nudge toward point B.
        "Findings diff — pick point B and press f · esc close".to_string()
    } else if history.filter.text.is_empty() {
        "History — ↑↓ move · d/r chapter · D/R project · f findings · n note · / filter · esc close"
            .to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears",
            history.filter.text,
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

/// A compact "chapters changed" label for a commit's touched paths (#964), or
/// `None` when it changed nothing tracked. Shows each path's file name (the chapter
/// file, not its directory), caps the list at three so a wide-reaching commit stays
/// one line, and summarizes the overflow ("one.md, two.md +3 more"). The `.md`
/// suffix is kept so it reads as the chapter file the diff/restore act on.
fn summarize_changed(paths: &[String]) -> Option<String> {
    const CAP: usize = 3;
    if paths.is_empty() {
        return None;
    }
    let name = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let shown: Vec<String> = paths.iter().take(CAP).map(|p| name(p)).collect();
    let mut label = shown.join(", ");
    if paths.len() > CAP {
        let _ = write!(label, " +{} more", paths.len() - CAP);
    }
    Some(label)
}

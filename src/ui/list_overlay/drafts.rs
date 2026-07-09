//! The branch-as-draft overlay (#965): the project's drafts (local branches), one
//! per row, with the current one marked. A click-selectable list, drawn through
//! [`draw_list_overlay_recording`]. Enter switches to the selected draft, `n`
//! creates a new draft from the current state, `m` merges the selected draft into
//! the current one, Esc closes.

use std::cell::RefCell;

use ratatui::{layout::Rect, Frame};

use super::{draw_list_overlay_recording, list_row};
use crate::app::ListLayout;
use crate::mode::DraftsMenu;

/// Render the drafts overlay (#965): every draft (local branch), the current one
/// tagged "· current", selectable to switch / merge. `recorder` captures the row
/// geometry for click-to-select.
pub(in crate::ui) fn draw_drafts(
    frame: &mut Frame,
    area: Rect,
    menu: &DraftsMenu,
    recorder: &RefCell<ListLayout>,
) {
    let title = format!(" Drafts ({}) ", menu.drafts.len());
    let rows = menu
        .drafts
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let secondary = if *name == menu.current {
                Some("· current".to_string())
            } else {
                None
            };
            list_row(i == menu.selected, name, secondary)
        })
        .collect();
    let help = "Drafts — ↑↓ move · enter switch · n new · m merge into current · esc close";
    draw_list_overlay_recording(
        frame,
        area,
        &title,
        help,
        rows,
        Some(menu.selected),
        Some(recorder),
    );
}

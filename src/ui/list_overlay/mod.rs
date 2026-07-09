//! The navigable list overlays — the command palette, the project and directory
//! browsers, and the revision-triage findings list (#316). Each was near-identical
//! boilerplate (marker + selection style + dimmed secondary text, into a bordered
//! wrapping box); the row builders are consolidated here behind [`list_row`], and
//! the shared [`marker`] / [`row_style`] / [`draw_list_overlay`] primitives (also
//! used by the tree navigator and nav pane) live here too.
//!
//! The individual overlay renderers are split across cohesive submodules by
//! surface, re-exported through here so callers see one `list_overlay` module:
//! [`choosers`] (palette / project / dir / startup / model manager),
//! [`search`] (manuscript find & replace), [`findings`] (triage list, health
//! dashboard, co-occurrence report), [`history`], [`imports`] (manuscript- and
//! review-comment import previews), [`annotations`] (inline notes + corkboard),
//! and [`backups`]. This module keeps the shared row/scroll primitives and the two
//! `draw_list_overlay` entry points every overlay draws through.

use crate::theme;
use unicode_width::UnicodeWidthStr;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use std::cell::RefCell;

use crate::app::ListLayout;

mod annotations;
mod backups;
mod choosers;
mod findings;
mod history;
mod imports;
mod quick_switch;
mod search;

pub(super) use annotations::{draw_annotations, draw_corkboard};
pub(super) use backups::draw_backup_browser;
pub(super) use choosers::{
    draw_dir_browser, draw_model_manager, draw_palette, draw_project_browser,
};
pub use choosers::{draw_startup_browser, draw_startup_chooser};
pub(super) use findings::{draw_cooccurrence, draw_dashboard, draw_findings};
pub(super) use history::draw_history;
pub(super) use imports::{draw_import_preview, draw_review_import};
pub(super) use quick_switch::draw_quick_switch;
pub(super) use search::draw_project_search;

/// The selection marker for a list row.
pub(super) const fn marker(selected: bool) -> &'static str {
    if selected {
        "▸ "
    } else {
        "  "
    }
}

/// The style for a list row (reversed when selected).
pub(super) fn row_style(selected: bool) -> Style {
    if selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    }
}

/// A standard list row: the selection marker + `primary` text (reversed when
/// selected), optionally followed by dimmed `secondary` text (`  — …`). The
/// shared shape of every list-overlay row.
pub(super) fn list_row(selected: bool, primary: &str, secondary: Option<String>) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{}{primary}", marker(selected)),
        row_style(selected),
    )];
    if let Some(s) = secondary {
        spans.push(Span::styled(
            format!("  — {s}"),
            Style::default().fg(theme::current().dim),
        ));
    }
    Line::from(spans)
}

/// The display width of a row in terminal cells (its spans' widths summed).
fn row_cells(line: &Line) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// How many wrapped display rows `cells` of content occupy in a box `width` cells
/// wide — at least one. A lower bound for word-wrap (which can spill a little
/// further), so the scroll window keeps a one-row cushion below the selection.
fn wrapped_rows(cells: usize, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    cells.div_ceil(width).max(1)
}

/// The first row to render so `selected` stays visible in a `viewport`-line
/// window, given each row's wrapped `heights`. Once the list has scrolled the
/// selection pins to the bottom of the window (the most context above it shows).
/// Returns 0 when everything fits or there's no selection (#513).
fn scroll_offset(heights: &[usize], viewport: usize, selected: Option<usize>) -> usize {
    let total: usize = heights.iter().sum();
    match selected {
        Some(sel) if total > viewport && sel < heights.len() => {
            let mut offset = sel;
            let mut used = heights[sel];
            while offset > 0 && used + heights[offset - 1] <= viewport {
                offset -= 1;
                used += heights[offset];
            }
            offset
        }
        _ => 0,
    }
}

/// Render a navigable list overlay: a bold help line, a blank, then the rows, in
/// a bordered wrapping box (#316). Shared by every list overlay and the tree
/// navigator. When the rows don't all fit, the list scrolls to keep `selected` in
/// view and the title shows how many rows are hidden above/below (#513) — derived
/// from `selected` each frame, so no per-overlay scroll state is needed. Pass
/// `selected = None` for a list with no active row (it renders from the top).
pub(super) fn draw_list_overlay(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    help: &str,
    rows: Vec<Line<'static>>,
    selected: Option<usize>,
) {
    draw_list_overlay_recording(frame, area, title, help, rows, selected, None);
}

/// [`draw_list_overlay`] that also records the drawn row geometry into `recorder`
/// (#812), so a mouse click can be mapped back to a list index. Passed
/// `Some(&app.list_layout)` for the click-selectable list modes (findings,
/// corkboard, history, …); `None` for full-screen chooser lists with no click
/// target. Rows are treated as single screen rows for the click map (the common
/// case); a wrapped multi-row entry maps the click to that entry's first row.
pub(super) fn draw_list_overlay_recording(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    help: &str,
    rows: Vec<Line<'static>>,
    selected: Option<usize>,
    recorder: Option<&RefCell<ListLayout>>,
) {
    // Interior of the bordered box, then the rows region under the help + blank.
    let width = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let header_h = wrapped_rows(UnicodeWidthStr::width(help), width) + 1;
    let viewport = inner_h.saturating_sub(header_h);

    let heights: Vec<usize> = rows
        .iter()
        .map(|r| wrapped_rows(row_cells(r), width))
        .collect();
    let offset = scroll_offset(&heights, viewport, selected);

    // Record where the rows landed so a click can invert screen → list index
    // (#812). The rows start one row past the box's top border, the help line, and
    // the blank separator; the click map treats each entry as a single row.
    if let Some(rec) = recorder {
        *rec.borrow_mut() = ListLayout {
            first_row: area.y.saturating_add(1 + header_h as u16),
            height: viewport as u16,
            first_index: offset,
            len: heights.len(),
        };
    }

    // How many rows fit from `offset` — for the hidden-above/below affordance.
    let mut used = 0;
    let mut last = offset;
    while last < rows.len() && used + heights[last] <= viewport {
        used += heights[last];
        last += 1;
    }
    // A selected row taller than the whole viewport still counts as shown, so it
    // isn't double-counted into "below".
    let last = last.max(offset + 1);
    let (above, below) = (offset, rows.len().saturating_sub(last));
    let title = if above > 0 || below > 0 {
        format!("{title}↑{above} ↓{below} ")
    } else {
        title.to_string()
    };

    let mut lines = vec![
        Line::from(Span::styled(
            help.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    lines.extend(rows.into_iter().skip(offset));
    let block = Block::default().borders(Borders::ALL).title(title);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::scroll_offset;

    #[test]
    fn a_list_that_fits_does_not_scroll() {
        // 5 single-line rows in a 10-line viewport: always render from the top.
        let heights = [1; 5];
        assert_eq!(scroll_offset(&heights, 10, Some(0)), 0);
        assert_eq!(scroll_offset(&heights, 10, Some(4)), 0);
    }

    #[test]
    fn no_selection_renders_from_the_top() {
        let heights = [1; 50];
        assert_eq!(scroll_offset(&heights, 10, None), 0);
    }

    #[test]
    fn scrolling_keeps_the_selection_within_the_viewport() {
        // 50 single-line rows, a 10-line viewport. The selection stays visible,
        // pinned to the bottom of the window once scrolled.
        let heights = [1; 50];
        // Still near the top: no scroll until the selection passes the viewport.
        assert_eq!(scroll_offset(&heights, 10, Some(9)), 0);
        // Past it: the window scrolls so `selected` is the last visible row.
        assert_eq!(scroll_offset(&heights, 10, Some(10)), 1);
        assert_eq!(scroll_offset(&heights, 10, Some(30)), 21);
        assert_eq!(scroll_offset(&heights, 10, Some(49)), 40);
        // In every case the selection sits inside [offset, offset + viewport).
        for sel in 0..50 {
            let off = scroll_offset(&heights, 10, Some(sel));
            assert!(off <= sel && sel < off + 10, "sel {sel} visible from {off}");
        }
    }

    #[test]
    fn wrapped_rows_consume_their_extra_height_in_the_window() {
        // Row 4 wraps to 3 lines. Viewport 5, selecting the last row: the window
        // accounts for the tall row's height, so only rows 4,5,6 fit (3+1+1 = 5),
        // not the 5 rows a naive row count would show. Offset lands on row 4.
        let heights = [1, 1, 1, 1, 3, 1, 1];
        assert_eq!(scroll_offset(&heights, 5, Some(6)), 4);
    }
}

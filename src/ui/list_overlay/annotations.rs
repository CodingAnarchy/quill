//! The chapter-scoped card overlays: the inline annotations list (#302) and the
//! corkboard of scene index cards (#589 / #636 / #701). Both are click-selectable
//! lists drawn through [`draw_list_overlay_recording`] with a row-geometry
//! recorder.

use std::cell::RefCell;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    Frame,
};

use super::{draw_list_overlay_recording, list_row, marker};
use crate::app::ListLayout;
use crate::mode::Corkboard;
use crate::theme;

/// One annotation-list row (#302/#889): the marker + note over the dimmed snippet
/// it anchors, prefixed with a check for a resolved thread and suffixed with a
/// reply count. A resolved row is drawn dimmed throughout so it reads as done at a
/// glance, distinct from the still-open notes. When the note carries a named
/// reviewer (#886/#933), its name leads the note so a multi-editor list shows who
/// left each one.
fn annotation_row(selected: bool, a: &crate::annotation::Live, snippet: String) -> Line<'static> {
    let check = if a.resolved { "✓ " } else { "" };
    let replies = match a.replies.len() {
        0 => String::new(),
        1 => "  (1 reply)".to_string(),
        n => format!("  ({n} replies)"),
    };
    // A named reviewer is shown; an author-authored note carries no tag (it's the
    // implicit "you"), keeping the historic single-author row unchanged.
    let who = a
        .reviewer
        .as_deref()
        .map(|r| format!("{r}: "))
        .unwrap_or_default();
    let primary = format!("{check}{who}{}{replies}", a.note);
    if a.resolved {
        // A resolved thread reads as done: dim the whole row (still reversed when
        // selected so the cursor is legible).
        let dim = Style::default().fg(theme::current().dim);
        let style = if selected {
            dim.add_modifier(Modifier::REVERSED)
        } else {
            dim
        };
        return Line::from(vec![
            Span::styled(format!("{}{primary}", marker(selected)), style),
            Span::styled(format!("  — {snippet}"), dim),
        ]);
    }
    list_row(selected, &primary, Some(snippet))
}

/// Render the active chapter's inline annotations (#302) as a navigable list:
/// each row is the note over the (dimmed) snippet of text it's anchored to.
pub(in crate::ui) fn draw_annotations(
    frame: &mut Frame,
    area: Rect,
    app: &crate::app::App,
    selected: usize,
    recorder: &RefCell<ListLayout>,
) {
    let buffer = app.edit_buffer();
    let len = buffer.len_bytes();
    let title = format!(" Annotations ({}) ", app.annotations.len());
    // The type-to-filter query (#822) narrows to notes whose text or anchored
    // snippet match; the row index is preserved so the marker/click map align.
    let (filter, typing, show, sort, reviewer) = match &app.mode {
        crate::mode::Mode::Annotations {
            filter,
            typing,
            show,
            sort,
            reviewer,
            ..
        } => (filter.clone(), *typing, *show, *sort, reviewer.clone()),
        _ => (
            String::new(),
            false,
            crate::mode::AnnoShow::All,
            crate::mode::AnnoSort::Position,
            None,
        ),
    };
    // The visible notes in display order (#933): filtered, then ordered by the
    // active sort. The single list the app, the marker, and the click map agree on.
    let visible = app.annotation_visible_order();
    let selected_pos = visible.iter().position(|&i| i == selected);
    let rows = visible
        .iter()
        .map(|&i| {
            let a = &app.annotations[i];
            let (s, e) = (a.start.min(len), a.end.min(len));
            let snippet = if s < e {
                crate::text::ellipsize(&buffer.byte_slice(s, e), 40)
            } else {
                "(text gone)".to_string()
            };
            annotation_row(i == selected, a, snippet)
        })
        .collect();
    // The resolved/open filter (#889), sort order (#933), and per-reviewer filter
    // (#933) are part of the list identity — surface each slice/order and how to
    // cycle it alongside the text filter.
    let show_tag = match show {
        crate::mode::AnnoShow::All => String::new(),
        other => format!("[{}] ", other.label()),
    };
    let sort_tag = match sort {
        crate::mode::AnnoSort::Position => String::new(),
        other => format!("[by {}] ", other.label()),
    };
    let who_tag = reviewer
        .as_deref()
        .map(|r| format!("[@{r}] "))
        .unwrap_or_default();
    let tags = format!("{show_tag}{sort_tag}{who_tag}");
    let help = if typing {
        format!("Filter: {filter}_")
    } else if filter.is_empty() && reviewer.is_none() {
        format!(
            "{tags}Annotations — ↑↓ move · ]/[ next/prev open · enter jump · a reply · \
             r resolve · f {} · s sort · R reviewer · d delete · / filter · esc close",
            show.next().label()
        )
    } else {
        let n = visible.len();
        format!(
            "{tags}Filtered — {n} match{} · esc clears",
            if n == 1 { "" } else { "es" }
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

/// Render the corkboard (#589 / #636 / #701 / #974): scenes as index cards, each a status
/// badge + scene heading over its synopsis (the author's one-liner, or the first
/// line of prose as a fallback), indented by heading level so nested scenes read as
/// a hierarchy. Navigable; the author reorders scenes, edits each synopsis, and
/// cycles each status from here, or jumps the editor cursor to a scene with Enter.
///
/// The manuscript-wide board (#701) prefixes each chapter's first card with a
/// non-selectable chapter-title header, so the cards read grouped by chapter. Those
/// header rows shift the selected card's display position, so the highlight index is
/// re-derived over the interleaved rows rather than reusing the card index.
pub(in crate::ui) fn draw_corkboard(
    frame: &mut Frame,
    area: Rect,
    board: &Corkboard,
    recorder: &RefCell<ListLayout>,
) {
    // The selected scene's status reads in words in the title (the per-card badge
    // is the glance version); the count summarizes the board.
    let status = board
        .cards
        .get(board.selected)
        .map(|c| format!(" · {}", c.status.label()))
        .unwrap_or_default();
    let scope = if board.manuscript { "manuscript" } else { "" };
    let title = format!(" Corkboard {scope} ({} scenes{status}) ", board.cards.len());

    let mut rows: Vec<Line<'static>> = Vec::with_capacity(board.cards.len());
    // The display row the selected card lands on, once chapter headers are woven in.
    let mut selected_row = 0;
    // Track the chapter of the previous card so a header is emitted once per group.
    let mut last_chapter: Option<usize> = None;
    for (i, c) in board.cards.iter().enumerate() {
        // On the manuscript board, open a new group with a chapter-title header the
        // first time a chapter's cards appear.
        if board.manuscript && c.chapter != last_chapter {
            last_chapter = c.chapter;
            rows.push(Line::from(Span::styled(
                format!("  {}", c.chapter_title),
                crate::ui::active_style(),
            )));
        }
        if i == board.selected {
            selected_row = rows.len();
        }
        // Indent by heading depth (level 2 is the top scene level, so subtract it)
        // so `###` sub-scenes nest under their `##` parent.
        let indent = "  ".repeat((c.level.saturating_sub(2)) as usize);
        // The author's synopsis when set, else the first line of prose (#636).
        let synopsis = if !c.synopsis.is_empty() {
            Some(c.synopsis.clone())
        } else {
            (!c.prose_line.is_empty()).then(|| c.prose_line.clone())
        };
        // A status badge leads each card so the drafting state reads at a glance.
        rows.push(list_row(
            i == board.selected,
            &format!("{} {indent}{}", c.status.badge(), c.title),
            synopsis,
        ));
    }
    // Click-to-select maps a screen row to a card index; on the manuscript board
    // the interleaved chapter headers shift that mapping, so record the layout only
    // for the single-chapter board where row index == card index (#812).
    let rec = (!board.manuscript).then_some(recorder);
    draw_list_overlay_recording(
        frame,
        area,
        &title,
        "Corkboard — ↑↓ move · enter jump · K/J reorder · e synopsis · s status · u undo · esc close",
        rows,
        Some(selected_row),
        rec,
    );
}

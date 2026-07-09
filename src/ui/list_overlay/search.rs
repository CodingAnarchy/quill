//! The manuscript-wide find & replace overlay (#295): a stage-specific help line
//! over the matches, each row labeled with its chapter.

use ratatui::{layout::Rect, text::Line, Frame};

use super::{draw_list_overlay, list_row};
use crate::mode::{ProjectSearch, ProjectSearchStage};

/// Render the manuscript-wide find & replace overlay (#295): a stage-specific
/// help line over the matches, each row labeled with its chapter.
pub(in crate::ui) fn draw_project_search(frame: &mut Frame, area: Rect, ps: &ProjectSearch) {
    let n = ps.matches.len();
    let span = ps.chapter_span();
    let matches = |n: usize| if n == 1 { "match" } else { "matches" };
    let chapters = |n: usize| if n == 1 { "chapter" } else { "chapters" };
    // The active match modes (#570): a short tag after the query, e.g. "[Aa w]".
    let modes = match ps.opts.label() {
        ref m if m.is_empty() => String::new(),
        m => format!(" {m}"),
    };
    let help = match ps.stage {
        ProjectSearchStage::Query => {
            format!(
                "Find across manuscript: {}{modes}_ · enter search · ^C case · ^W word · ^R regex · esc cancel",
                ps.query
            )
        }
        ProjectSearchStage::Results => format!(
            "'{}'{modes} — {n} {} in {span} {} · ↑↓ move · enter jump · r replace · ^C case · ^W word · ^R regex · esc close",
            ps.query,
            matches(n),
            chapters(span),
        ),
        ProjectSearchStage::Replacement => {
            format!(
                "Replace '{}' with: {}_ · enter step · esc cancel",
                ps.query, ps.replacement
            )
        }
        ProjectSearchStage::Step => format!(
            "'{}' → '{}' · y replace · n skip · a replace all · enter done ({n} left)",
            ps.query, ps.replacement,
        ),
        ProjectSearchStage::Confirm => format!(
            "Replace all {n} {} across {span} {}? (y/N)",
            matches(n),
            chapters(span),
        ),
    };
    // Rows: the match list, with the selection highlighted while navigating.
    let show_selection = matches!(
        ps.stage,
        ProjectSearchStage::Results | ProjectSearchStage::Step
    );
    let rows: Vec<Line<'static>> = ps
        .matches
        .iter()
        .enumerate()
        .map(|(i, m)| {
            list_row(
                show_selection && i == ps.selected,
                &m.context,
                Some(m.title.clone()),
            )
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        " Find across manuscript ",
        &help,
        rows,
        show_selection.then_some(ps.selected),
    );
}

//! The navigable list overlays — the command palette, the project and directory
//! browsers, and the revision-triage findings list (#316). Each was near-identical
//! boilerplate (marker + selection style + dimmed secondary text, into a bordered
//! wrapping box); the row builders are consolidated here behind [`list_row`], and
//! the shared [`marker`] / [`row_style`] / [`draw_list_overlay`] primitives (also
//! used by the tree navigator and nav pane) live here too.

use crate::theme;
use std::path::Path;
use unicode_width::UnicodeWidthStr;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::mode::{
    Corkboard, DirBrowser, Findings, History, Palette, PaletteRow, ProjectBrowser, ProjectSearch,
    ProjectSearchStage,
};
use crate::project::Project;

/// The selection marker for a list row.
pub(super) fn marker(selected: bool) -> &'static str {
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
pub(super) fn list_row(
    selected: bool,
    primary: String,
    secondary: Option<String>,
) -> Line<'static> {
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

pub(super) fn draw_palette(frame: &mut Frame, area: Rect, palette: &Palette) {
    let rows = palette
        .rows()
        .into_iter()
        .enumerate()
        .map(|(ri, row)| {
            let selected = ri == palette.selected;
            match row {
                // A collapsible group heading: a ▾/▸ disclosure + the group name,
                // in the heading color (reversed when selected) (#514).
                PaletteRow::Header { group, collapsed } => {
                    let arrow = if collapsed { "▸" } else { "▾" };
                    // The heading accent shared with the tree/nav, reversed when selected.
                    let mut style = super::active_style();
                    if selected {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    Line::from(Span::styled(format!("{arrow} {group}"), style))
                }
                // An item, indented under its group heading when it has one.
                PaletteRow::Item(i) => {
                    let item = &palette.items[i];
                    let indent = if item.group.is_some() { "  " } else { "" };
                    list_row(
                        selected,
                        format!("{indent}{}", item.label),
                        Some(item.desc.clone()),
                    )
                }
            }
        })
        .collect();
    let title = format!(" {} ", palette.title);
    // The `R` force-rebuild gesture only applies to the AI bible passes (#176).
    let rebuild = if palette.title == "AI" {
        " · R rebuild bible (full)"
    } else {
        ""
    };
    // Mention expand/collapse only when the palette actually has group headings.
    let groups = if palette.items.iter().any(|it| it.group.is_some()) {
        " · ←/→ collapse/expand"
    } else {
        ""
    };
    let help = format!(
        "{} — ↑↓ move · enter run{groups}{rebuild} · esc close",
        palette.title
    );
    draw_list_overlay(frame, area, &title, &help, rows, Some(palette.selected));
}

/// Render the no-path startup chooser (#526): a two-row list — create a new
/// project, or open an existing one — over the whole screen. Reuses the list
/// overlay so it matches the in-editor browsers. `open_existing` is false when
/// there are no openable projects (the row is then absent and the caller skips
/// straight to new-project), but the chooser is only shown when it's true.
pub fn draw_startup_chooser(frame: &mut Frame, area: Rect, selected: usize) {
    let rows = vec![
        list_row(
            selected == 0,
            "Create a new project".to_string(),
            Some("name it and start a fresh book".to_string()),
        ),
        list_row(
            selected == 1,
            "Open an existing project".to_string(),
            Some("pick from your recent projects and library".to_string()),
        ),
    ];
    draw_list_overlay(
        frame,
        area,
        " Quill ",
        "Quill — ↑↓ move · enter choose · esc quit",
        rows,
        Some(selected),
    );
}

/// Render the startup project picker (#526): the openable projects as a navigable
/// list, full-screen (there's no editor behind it yet). Same rows as the
/// in-editor browser, minus the "(open)" marker since nothing is open.
pub fn draw_startup_browser(
    frame: &mut Frame,
    area: Rect,
    projects: &[crate::recent::RecentProject],
    selected: usize,
) {
    let rows = projects
        .iter()
        .enumerate()
        .map(|(i, p)| {
            list_row(
                i == selected,
                p.title.clone(),
                Some(p.path.display().to_string()),
            )
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        " Open a project ",
        "Open a project — ↑↓ move · enter open · esc back",
        rows,
        Some(selected),
    );
}

/// Render the recently-opened projects as a navigable list (#270). The
/// currently-open project is marked, and each row shows the title over its path.
/// A `/`-filter (#612) narrows the list to titles/paths matching the query; the
/// help line shows the query while typing and the match count when filtered.
pub(super) fn draw_project_browser(
    frame: &mut Frame,
    area: Rect,
    browser: &ProjectBrowser,
    current: &Path,
) {
    let current = current.canonicalize();
    let query = browser.filter.to_lowercase();
    let matches = |p: &crate::recent::RecentProject| {
        query.is_empty()
            || p.title.to_lowercase().contains(&query)
            || p.path.to_string_lossy().to_lowercase().contains(&query)
    };
    // Filter to matching projects, keeping the original index so the selection
    // marker lands on the right row; the row that renders selected is the one
    // whose original index is `browser.selected`.
    let visible: Vec<(usize, &crate::recent::RecentProject)> = browser
        .projects
        .iter()
        .enumerate()
        .filter(|(_, p)| matches(p))
        .collect();
    let selected_pos = visible.iter().position(|(i, _)| *i == browser.selected);
    let rows = visible
        .iter()
        .map(|(i, p)| {
            let here = current.as_ref().map(|c| c == &p.path).unwrap_or(false);
            let tag = if here { "  (open)" } else { "" };
            list_row(
                *i == browser.selected,
                format!("{}{tag}", p.title),
                Some(p.path.display().to_string()),
            )
        })
        .collect();
    let help = if browser.typing {
        format!("Filter: {}_", browser.filter)
    } else if browser.filter.is_empty() {
        "Projects — ↑↓ move · enter open · / filter · r rename · d delete · b browse files · n new · esc cancel".to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears · r rename · d delete",
            browser.filter,
            visible.len(),
            if visible.len() == 1 { "" } else { "es" }
        )
    };
    draw_list_overlay(frame, area, " Projects ", &help, rows, selected_pos);
}

/// Render the filesystem navigator (#271): the current directory's subfolders,
/// with `..` to go up and Quill projects marked as openable.
pub(super) fn draw_dir_browser(frame: &mut Frame, area: Rect, browser: &DirBrowser) {
    let rows = browser
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let tag = if e.is_project {
                "  (project — enter opens)"
            } else if e.is_parent {
                "  (up)"
            } else {
                "/"
            };
            list_row(i == browser.selected, format!("{}{tag}", e.label), None)
        })
        .collect();
    let title = format!(" {} ", browser.cwd.display());
    draw_list_overlay(
        frame,
        area,
        &title,
        "Browse — ↑↓ move · enter open/into · n new project · ← up · esc cancel",
        rows,
        Some(browser.selected),
    );
}

/// Render the manuscript-wide find & replace overlay (#295): a stage-specific
/// help line over the matches, each row labeled with its chapter.
pub(super) fn draw_project_search(frame: &mut Frame, area: Rect, ps: &ProjectSearch) {
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
                "Find across manuscript: {}{modes}_ · enter search · ^C case · ^W word · esc cancel",
                ps.query
            )
        }
        ProjectSearchStage::Results => format!(
            "'{}'{modes} — {n} {} in {span} {} · ↑↓ move · enter jump · r replace · ^C case · ^W word · esc close",
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
                m.context.clone(),
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

/// Render the findings as a navigable triage list.
pub(super) fn draw_findings(frame: &mut Frame, area: Rect, findings: &Findings, project: &Project) {
    let title = format!(" Findings ({}) ", findings.items.len());
    let rows = findings
        .items
        .iter()
        .enumerate()
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
            list_row(
                i == findings.selected,
                format!("{}{loc}", item.summary),
                None,
            )
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        &title,
        "Findings — ↑↓ move · enter jump · i mark xxx · r re-check · d dismiss · esc close",
        rows,
        Some(findings.selected),
    );
}

/// Render the commit history as a navigable list (#587): each row is a commit's
/// date, short id, and summary. The selected commit is the one the diff / restore
/// actions address.
pub(super) fn draw_history(frame: &mut Frame, area: Rect, history: &History, chapter: &str) {
    let title = format!(" History — {chapter} ");
    let rows = history
        .commits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            list_row(
                i == history.selected,
                format!("{}  {}  {}", c.when, c.id, c.summary),
                None,
            )
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        &title,
        "History — ↑↓ move · d diff this chapter · r restore this chapter · esc close",
        rows,
        Some(history.selected),
    );
}

/// Render the active chapter's inline annotations (#302) as a navigable list:
/// each row is the note over the (dimmed) snippet of text it's anchored to.
pub(super) fn draw_annotations(
    frame: &mut Frame,
    area: Rect,
    app: &crate::app::App,
    selected: usize,
) {
    let buffer = app.edit_buffer();
    let len = buffer.len_bytes();
    let title = format!(" Annotations ({}) ", app.annotations.len());
    let rows = app
        .annotations
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let (s, e) = (a.start.min(len), a.end.min(len));
            let snippet = if s < e {
                crate::text::ellipsize(&buffer.byte_slice(s, e), 40)
            } else {
                "(text gone)".to_string()
            };
            list_row(i == selected, a.note.clone(), Some(snippet))
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        &title,
        "Annotations — ↑↓ move · enter jump · d delete · esc close",
        rows,
        Some(selected),
    );
}

/// Render the read-only corkboard (#589, Phase 1): the active chapter's scenes as
/// index cards — each a scene heading over its first line of prose (the
/// synopsis), indented by heading level so nested scenes read as a hierarchy.
/// Navigable; Enter jumps the editor cursor to the selected scene.
pub(super) fn draw_corkboard(frame: &mut Frame, area: Rect, board: &Corkboard) {
    let title = format!(" Corkboard ({} scenes) ", board.cards.len());
    let rows = board
        .cards
        .iter()
        .enumerate()
        .map(|(i, c)| {
            // Indent by heading depth (level 2 is the top scene level, so subtract
            // it) so `###` sub-scenes nest under their `##` parent.
            let indent = "  ".repeat((c.level.saturating_sub(2)) as usize);
            let synopsis = (!c.synopsis.is_empty()).then(|| c.synopsis.clone());
            list_row(
                i == board.selected,
                format!("{indent}{}", c.title),
                synopsis,
            )
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        &title,
        "Corkboard — ↑↓ move · enter jump to scene · esc close",
        rows,
        Some(board.selected),
    );
}

/// Render the local AI model catalog (#379): each model with its characteristics
/// and a one-line description, the ones in the author's set marked. Enter/Space
/// toggles membership.
pub(super) fn draw_model_manager(
    frame: &mut Frame,
    area: Rect,
    app: &crate::app::App,
    selected: usize,
) {
    use crate::ai::model_select::MODELS;
    let set = &app.ai.local_models;
    let title = format!(" Local AI models ({} chosen) ", set.len());
    let rows = MODELS
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mark = if set.iter().any(|id| id == m.id()) {
                "● "
            } else {
                "○ "
            };
            // On-disk status (#382), from the cached set — no per-frame cache probe.
            let disk = if app.model_is_downloaded(m.id()) {
                " · ✓ downloaded"
            } else {
                ""
            };
            let stats = format!(
                "{:.1} GB · needs {} GB RAM · {} ctx{disk}",
                m.size_gb,
                m.min_ram_gb,
                m.context_label()
            );
            list_row(
                i == selected,
                format!("{mark}Qwen2.5-{}", m.id()),
                Some(format!("{stats} — {}", m.description)),
            )
        })
        .collect();
    // Local inference is a build-time opt-in; say so rather than imply it runs.
    let help = if cfg!(feature = "local") {
        "Local models — ↑↓ move · enter/space use · d download · x remove · esc close"
    } else {
        "Local models — ↑↓ move · enter/space choose · esc close · (rebuild with --features local to download/run)"
    };
    draw_list_overlay(frame, area, &title, help, rows, Some(selected));
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

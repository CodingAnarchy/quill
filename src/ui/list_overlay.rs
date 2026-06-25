//! The navigable list overlays — the command palette, the project and directory
//! browsers, and the revision-triage findings list (#316). Each was near-identical
//! boilerplate (marker + selection style + dimmed secondary text, into a bordered
//! wrapping box); the row builders are consolidated here behind [`list_row`], and
//! the shared [`marker`] / [`row_style`] / [`draw_list_overlay`] primitives (also
//! used by the tree navigator and nav pane) live here too.

use std::path::Path;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::mode::{DirBrowser, Findings, Palette, ProjectBrowser, ProjectSearch, ProjectSearchStage};
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
pub(super) fn list_row(selected: bool, primary: String, secondary: Option<String>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{}{primary}", marker(selected)), row_style(selected))];
    if let Some(s) = secondary {
        spans.push(Span::styled(format!("  — {s}"), Style::default().fg(Color::DarkGray)));
    }
    Line::from(spans)
}

/// Render a navigable list overlay: a bold help line, a blank, then the rows, in
/// a bordered wrapping box. Shared by every list overlay and the tree navigator.
pub(super) fn draw_list_overlay(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    help: &str,
    rows: impl Iterator<Item = Line<'static>>,
) {
    let mut lines = vec![
        Line::from(Span::styled(help.to_string(), Style::default().add_modifier(Modifier::BOLD))),
        Line::raw(""),
    ];
    lines.extend(rows);
    let block = Block::default().borders(Borders::ALL).title(title.to_string());
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn draw_palette(frame: &mut Frame, area: Rect, palette: &Palette) {
    let rows = palette
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| list_row(i == palette.selected, item.label.clone(), Some(item.desc.clone())));
    let title = format!(" {} ", palette.title);
    // The `R` force-rebuild gesture only applies to the AI bible passes (#176).
    let rebuild = if palette.title == "AI" { " · R rebuild bible (full)" } else { "" };
    let help = format!("{} — ↑↓ move · enter run{rebuild} · esc close", palette.title);
    draw_list_overlay(frame, area, &title, &help, rows);
}

/// Render the recently-opened projects as a navigable list (#270). The
/// currently-open project is marked, and each row shows the title over its path.
pub(super) fn draw_project_browser(frame: &mut Frame, area: Rect, browser: &ProjectBrowser, current: &Path) {
    let current = current.canonicalize();
    let rows = browser.projects.iter().enumerate().map(|(i, p)| {
        let here = current.as_ref().map(|c| c == &p.path).unwrap_or(false);
        let tag = if here { "  (open)" } else { "" };
        list_row(i == browser.selected, format!("{}{tag}", p.title), Some(p.path.display().to_string()))
    });
    draw_list_overlay(frame, area, " Projects ", "Projects — ↑↓ move · enter open · b browse files · n new project · esc cancel", rows);
}

/// Render the filesystem navigator (#271): the current directory's subfolders,
/// with `..` to go up and Quill projects marked as openable.
pub(super) fn draw_dir_browser(frame: &mut Frame, area: Rect, browser: &DirBrowser) {
    let rows = browser.entries.iter().enumerate().map(|(i, e)| {
        let tag = if e.is_project {
            "  (project — enter opens)"
        } else if e.is_parent {
            "  (up)"
        } else {
            "/"
        };
        list_row(i == browser.selected, format!("{}{tag}", e.label), None)
    });
    let title = format!(" {} ", browser.cwd.display());
    draw_list_overlay(frame, area, &title, "Browse — ↑↓ move · enter open/into · n new project · ← up · esc cancel", rows);
}

/// Render the manuscript-wide find & replace overlay (#295): a stage-specific
/// help line over the matches, each row labeled with its chapter.
pub(super) fn draw_project_search(frame: &mut Frame, area: Rect, ps: &ProjectSearch) {
    let n = ps.matches.len();
    let span = ps.chapter_span();
    let matches = |n: usize| if n == 1 { "match" } else { "matches" };
    let chapters = |n: usize| if n == 1 { "chapter" } else { "chapters" };
    let help = match ps.stage {
        ProjectSearchStage::Query => {
            format!("Find across manuscript: {}_ · enter search · esc cancel", ps.query)
        }
        ProjectSearchStage::Results => format!(
            "'{}' — {n} {} in {span} {} · ↑↓ move · enter jump · r replace · esc close",
            ps.query,
            matches(n),
            chapters(span),
        ),
        ProjectSearchStage::Replacement => {
            format!("Replace '{}' with: {}_ · enter step · esc cancel", ps.query, ps.replacement)
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
    let show_selection = matches!(ps.stage, ProjectSearchStage::Results | ProjectSearchStage::Step);
    let rows: Vec<Line<'static>> = ps
        .matches
        .iter()
        .enumerate()
        .map(|(i, m)| list_row(show_selection && i == ps.selected, m.context.clone(), Some(m.title.clone())))
        .collect();
    draw_list_overlay(frame, area, " Find across manuscript ", &help, rows.into_iter());
}

/// Render the findings as a navigable triage list.
pub(super) fn draw_findings(frame: &mut Frame, area: Rect, findings: &Findings, project: &Project) {
    let title = format!(" Findings ({}) ", findings.items.len());
    let rows = findings.items.iter().enumerate().map(|(i, item)| {
        // In a series, a finding carries the book it was found in (#341); show it
        // so cross-book findings are attributed (chapter titles can repeat).
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
        list_row(i == findings.selected, format!("{}{loc}", item.summary), None)
    });
    draw_list_overlay(frame, area, &title, "Findings — ↑↓ move · enter jump · i mark xxx · d dismiss · esc close", rows);
}

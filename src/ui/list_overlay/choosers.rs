//! The chooser list overlays: the command palette, the project and directory
//! browsers, the startup chooser/picker, and the local model manager — the
//! full-screen navigable lists with no click target (all draw through the plain
//! [`draw_list_overlay`]).

use std::path::Path;

use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    Frame,
};

use super::{draw_list_overlay, list_row};
use crate::mode::{DirBrowser, Palette, PaletteRow, ProjectBrowser};

pub(in crate::ui) fn draw_palette(frame: &mut Frame, area: Rect, palette: &Palette) {
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
                    let mut style = crate::ui::active_style();
                    if selected {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    Line::from(Span::styled(format!("{arrow} {group}"), style))
                }
                // An item, indented under its group heading when it has one. When the
                // action has a dedicated key binding (#1103), its shortcut is shown
                // after the label so the palette doubles as a live cheat sheet.
                PaletteRow::Item(i) => {
                    let item = &palette.items[i];
                    let indent = if item.group.is_some() { "  " } else { "" };
                    let mut line = list_row(
                        selected,
                        &format!("{indent}{}", item.label),
                        Some(item.desc.clone()),
                    );
                    if let Some(sc) = item.shortcut {
                        line.spans
                            .push(Span::styled(format!("  [{sc}]"), crate::ui::active_style()));
                    }
                    line
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

/// Render the no-path startup chooser (#526): a three-row list — create a new
/// standalone project, start a new multi-book series (#1135), or open an existing
/// one — over the whole screen. Reuses the list overlay so it matches the
/// in-editor browsers. The chooser is only shown when there's something to open.
pub fn draw_startup_chooser(frame: &mut Frame, area: Rect, selected: usize) {
    let rows = vec![
        list_row(
            selected == 0,
            "Create a new project",
            Some("name it and start a fresh book".to_string()),
        ),
        list_row(
            selected == 1,
            "Start a new series",
            Some("name the series and its first book (#1135)".to_string()),
        ),
        list_row(
            selected == 2,
            "Open an existing project",
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
        .map(|(i, p)| list_row(i == selected, &p.title, Some(p.path.display().to_string())))
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
pub(in crate::ui) fn draw_project_browser(
    frame: &mut Frame,
    area: Rect,
    browser: &ProjectBrowser,
    current: &Path,
) {
    let current = current.canonicalize();
    let query = browser.filter.query();
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
            let here = current.as_ref().is_ok_and(|c| c == &p.path);
            let tag = if here { "  (open)" } else { "" };
            list_row(
                *i == browser.selected,
                &format!("{}{tag}", p.title),
                Some(p.path.display().to_string()),
            )
        })
        .collect();
    let help = if browser.filter.typing {
        format!("Filter: {}_", browser.filter.text)
    } else if browser.filter.text.is_empty() {
        "Projects — ↑↓ move · enter open · / filter · r rename · d delete · b browse files · n new · esc cancel".to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears · r rename · d delete",
            browser.filter.text,
            visible.len(),
            if visible.len() == 1 { "" } else { "es" }
        )
    };
    draw_list_overlay(frame, area, " Projects ", &help, rows, selected_pos);
}

/// Render the filesystem navigator (#271): the current directory's subfolders,
/// with `..` to go up and Quill projects marked as openable.
pub(in crate::ui) fn draw_dir_browser(frame: &mut Frame, area: Rect, browser: &DirBrowser) {
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
            list_row(i == browser.selected, &format!("{}{tag}", e.label), None)
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

/// Render the local AI model catalog (#379): each model with its characteristics
/// and a one-line description, the ones in the author's set marked. Enter/Space
/// toggles membership.
pub(in crate::ui) fn draw_model_manager(
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
            // "needs" is the computed worst-case footprint at the model's FULL
            // context window — weights + KV cache + overhead (#728), not a fixed
            // floor. For Qwen3 the 8-KV-head cache at the native 40k window is the
            // dominant term (#790), so this is the honest peak a whole-window pass
            // would cost; ordinary passes use far less. The quant is shown because
            // the Qwen3 catalog is mixed (Q8_0 for the smallest tiers, #789).
            let stats = format!(
                "{} · {:.1} GB · needs ≤{:.1} GB · {} ctx{disk}",
                m.quant,
                m.size_gb,
                m.required_mem_gb(m.context_tokens),
                m.context_label()
            );
            list_row(
                i == selected,
                &format!("{mark}Qwen3-{}", m.id()),
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

//! Rendering. Quill draws a centered, fixed-width column of prose with a thin
//! status line, keeping the eye in one place the way a printed page does.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Shadow, Wrap},
    Frame,
};

use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::app::{Assistant, Speaker, SplitContent, SplitDir};
use crate::mode::{BibleReview, GuideNav, Mode, Overlay, OverlayKind, Tree};
use crate::theme::{self, Theme};

mod assistant;
mod editor;
mod list_overlay;
mod nav_pane;
mod overlay;
mod status;
pub use assistant::AssistantWrapCache;
use assistant::{draw_assistant_pane, too_cramped_to_split};
pub use editor::HeaderCache;
use nav_pane::NavPane;
use status::{
    draw_prompt, editor_status_bar, mode_chip, nav_status_bar, prompt_height, search_status,
};
// Galley proofreading pagination (#1003): the read-only paged view's page-advance math,
// shared with the app-layer Space/Backspace paging so render and paging step in lockstep.
pub use editor::{galley_last_top, galley_next_top, galley_prev_top};
use list_overlay::{
    draw_annotations, draw_backup_browser, draw_conflict_review, draw_cooccurrence, draw_corkboard,
    draw_dashboard, draw_dir_browser, draw_drafts, draw_findings, draw_history,
    draw_import_preview, draw_list_overlay, draw_model_manager, draw_palette, draw_project_browser,
    draw_project_search, draw_quick_switch, draw_review_import, list_row, marker, row_style,
};
pub use list_overlay::{draw_startup_browser, draw_startup_chooser};
use overlay::{draw_bible_review, draw_overlay, draw_tree};
// `max_scroll` is shared with the report key handler (`crate::ui::max_scroll`).
pub use overlay::max_scroll;

/// A rendered region of the screen. Each pane borrows the [`App`] and draws
/// itself into a given area. A custom trait rather than ratatui's `Widget`
/// because the editor needs `frame.set_cursor_position` for cursor placement,
/// which `Widget::render` can't do (it only receives a cell buffer).
trait Pane {
    fn render(&self, frame: &mut Frame, area: Rect);
}

/// The prose column: centered, fixed-width text following the cursor. The second
/// field is the active chapter's index, resolved once per draw.
struct Editor<'a>(&'a App, usize);
/// The bottom line: chapter name / counts, or the active prompt. The second field
/// is the active chapter's index, resolved once per draw.
struct StatusBar<'a>(&'a App, usize);

pub fn draw(frame: &mut Frame, app: &App) {
    // The render path styles from the active theme (#300); affirm it each frame.
    theme::set_current(app.theme);
    let area = frame.area();

    // A full-screen read-only overlay (progress / analysis report or AI answer),
    // scrollable when it runs past the screen, replaces the editor entirely.
    if let Some(overlay) = &app.overlay {
        draw_overlay(frame, area, &app.project.manifest.title, overlay);
        return;
    }

    // The editor is always the base layer; modal surfaces (#349) float over it on
    // a dimmed backdrop, so the active surface is unmistakable and the editor
    // recedes without disappearing.
    // Resolve the active chapter once per frame — `active_index()` is an
    // O(chapters) scan of the manifest — and thread it to every pane that needs it
    // (the status bar's tags, the chapter header, the outline pane, pane titles).
    let active = app.project.active_index();
    draw_editor_base(frame, area, app, active);

    // Box overlays each render through the shared `modal` frame (dim + shadow);
    // a prompt lives on the status line, so it only dims the editor behind it.
    // A fresh frame: clear the click→list-index map (#812); the list modes below
    // re-record it when they draw, so a click only ever selects a row that's on
    // screen this frame.
    app.list_layout.borrow_mut().clear();
    match &app.mode {
        Mode::BibleReview(review) => modal(frame, area, |f, r| draw_bible_review(f, r, review)),
        Mode::ConflictReview(review) => {
            modal(frame, area, |f, r| draw_conflict_review(f, r, review));
        }
        Mode::Findings(findings) => modal(frame, area, |f, r| {
            draw_findings(f, r, findings, &app.project, &app.list_layout);
        }),
        Mode::Dashboard(dashboard) => modal(frame, area, |f, r| {
            draw_dashboard(f, r, dashboard, &app.list_layout);
        }),
        Mode::CoOccurrence(report) => modal(frame, area, |f, r| {
            draw_cooccurrence(f, r, report, &app.list_layout);
        }),
        Mode::History(history) => {
            let chapter = app.project.chapter_title(active).unwrap_or("this chapter");
            modal(frame, area, |f, r| {
                draw_history(f, r, history, chapter, &app.list_layout);
            });
        }
        Mode::Annotations { selected, .. } => {
            modal(frame, area, |f, r| {
                draw_annotations(f, r, app, *selected, &app.list_layout);
            });
        }
        Mode::Corkboard(board) => modal(frame, area, |f, r| {
            draw_corkboard(f, r, board, &app.list_layout);
        }),
        Mode::ModelManager { selected } => {
            modal(frame, area, |f, r| draw_model_manager(f, r, app, *selected));
        }
        Mode::ProjectSearch(ps) => modal(frame, area, |f, r| draw_project_search(f, r, ps)),
        Mode::Palette(palette) => modal(frame, area, |f, r| draw_palette(f, r, palette)),
        Mode::Tree(tree) => modal(frame, area, |f, r| draw_tree(f, r, app, tree)),
        Mode::ProjectBrowser(browser) => modal(frame, area, |f, r| {
            draw_project_browser(f, r, browser, &app.project.root);
        }),
        Mode::DirBrowser(browser) => modal(frame, area, |f, r| draw_dir_browser(f, r, browser)),
        Mode::ImportPreview(preview) => {
            modal(frame, area, |f, r| draw_import_preview(f, r, preview));
        }
        Mode::ReviewImport(preview) => {
            modal(frame, area, |f, r| draw_review_import(f, r, preview));
        }
        Mode::BackupBrowser(browser) => modal(frame, area, |f, r| {
            draw_backup_browser(f, r, browser, &app.list_layout);
        }),
        Mode::Drafts(menu) => modal(frame, area, |f, r| {
            draw_drafts(f, r, menu, &app.list_layout);
        }),
        Mode::QuickSwitch(switch) => modal(frame, area, |f, r| draw_quick_switch(f, r, switch)),
        // A long prompt wraps upward over the editor; keep all its rows undimmed.
        Mode::Prompt(prompt) => dim_backdrop(frame, area, prompt_height(prompt, area)),
        _ => {}
    }
}

/// Draw the editor base layer: the prose pane (with the outline strip when shown)
/// and the status line beneath it. Always drawn; modal overlays float on top.
fn draw_editor_base(frame: &mut Frame, area: Rect, app: &App, active: usize) {
    // Galley proofreading mode (#1003): a distinct read-only, paginated presentation —
    // one screen-page of the chapter at a time, chrome-free like composition mode, but
    // page-driven and never editable. Drawn straight into the whole screen and returns,
    // so no status bar, outline, or split shows while proofing.
    if app.in_galley() {
        app.pane_rects.set(None); // no split on screen — no click target
        editor::render_galley(frame, area, app);
        return;
    }

    // Distraction-free composition mode (#975): hide every piece of chrome — the
    // outline pane, the status bar, and any split — so the whole screen is the
    // centered prose column. Reuses the single-pane editor path (the same one focus
    // mode already dims into). Toggling off restores the full layout below.
    if app.distraction_free {
        app.pane_rects.set(None); // no split on screen — no click target
        Editor(app, active).render(frame, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // The outline pane takes a left strip when toggled on (Ctrl-O); the editor
    // region centers within whatever space remains, so the page stays balanced.
    let editor_area = if app.outline_visible {
        let outline_w = 28.min(chunks[0].width / 3);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(outline_w), Constraint::Min(1)])
            .split(chunks[0]);
        NavPane { app, active }.render(frame, cols[0]);
        cols[1]
    } else {
        chunks[0]
    };

    draw_editor_region(frame, editor_area, app, active);
    StatusBar(app, active).render(frame, chunks[1]);
}

/// Layer a floating modal over the already-drawn editor (#349): dim the editor as
/// a backdrop, drop a shadow, clear the box area, then hand the inner rect to the
/// overlay's own renderer. The single place every box overlay gets visual depth,
/// so they're all treated alike rather than each opting in.
fn modal(frame: &mut Frame, screen: Rect, draw_box: impl FnOnce(&mut Frame, Rect)) {
    dim_backdrop(frame, screen, 1);
    let inner = modal_area(screen);
    // The shadow sits down-and-right of the box, over the dimmed editor; shade
    // glyphs render on any terminal, so the depth survives a limited palette.
    let shadow = Shadow::dark_shade().style(Style::default().fg(theme::current().dim));
    frame.render_widget(&shadow, inner);
    frame.render_widget(Clear, inner); // wipe the dimmed editor under the box so it stays crisp
    draw_box(frame, inner);
}

/// The centered rect a floating modal occupies: inset from the screen so a dimmed
/// margin (and the shadow) shows around it, and clear of the bottom status line.
fn modal_area(screen: Rect) -> Rect {
    let mx = (screen.width / 12).clamp(2, 8);
    let my = (screen.height / 10).clamp(1, 4);
    Rect {
        x: screen.x + mx,
        y: screen.y + my,
        width: screen.width.saturating_sub(mx * 2),
        // Leave the top/bottom margins plus the status line (and a row for the shadow).
        height: screen.height.saturating_sub(my * 2 + 1),
    }
}

/// De-emphasize the editor behind a modal by dimming every cell above the bottom
/// `keep` rows — the status line, or every row of a wrapped prompt (#349). O(area), about what the live word count already costs per frame.
/// `Modifier::DIM` degrades gracefully — a terminal that ignores it just shows the
/// editor at normal brightness, with the framed, shadowed overlay still on top.
fn dim_backdrop(frame: &mut Frame, screen: Rect, keep: u16) {
    let buf = frame.buffer_mut();
    let bottom = screen.bottom().saturating_sub(keep); // keep the status line legible
    for y in screen.top()..bottom {
        for x in screen.left()..screen.right() {
            let cell = &mut buf[(x, y)];
            cell.modifier.insert(Modifier::DIM);
            // Drop bold too: bold and faint fight on most terminals, so a bold
            // heading wouldn't otherwise recede with the rest of the page.
            cell.modifier.remove(Modifier::BOLD);
        }
    }
}

/// Render the whole app to a plain-text screen of the given size — a shared
/// test helper for both the `ui` and `app` test suites.
#[cfg(test)]
pub fn render_to_string(app: &App, w: u16, h: u16) -> String {
    // Scoped to the fn: this helper is `#[cfg(test)]`, so a module-level import
    // would be unused in a non-test build.
    use ratatui::{backend::TestBackend, Terminal};
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// A split pane narrower (or, stacked, shorter) than this is too cramped to use,
/// so the layout collapses to the focused pane (#166).
const MIN_PANE_WIDTH: u16 = 30;
const MIN_PANE_HEIGHT: u16 = 6;

/// Draw the editor region: one pane, or two when the window is split — a second
/// editor (#163) or a docked report/analysis/AI surface (#164), at the split
/// ratio set by `Ctrl-w </>` (#166).
fn draw_editor_region(frame: &mut Frame, area: Rect, app: &App, active: usize) {
    let Some(view) = app.split_view() else {
        app.pane_rects.set(None);
        Editor(app, active).render(frame, area);
        return;
    };

    // Below the threshold a split would render both panes too thin to use — draw
    // only the focused pane (#166). The split state survives, so widening the
    // terminal restores it; this mirrors how the outline pane yields when narrow.
    if too_cramped_to_split(view.dir, area) {
        app.pane_rects.set(None); // only one pane is on screen — no click target

        match &view.content {
            // A docked report with scroll focus takes the full area.
            SplitContent::Surface(overlay) if app.surface_pane_focused() => {
                app.surface_height.set(area.height);
                draw_overlay(frame, area, &app.project.manifest.title, overlay);
            }
            // A focused assistant pane takes the full area (#307).
            SplitContent::Assistant {
                conv,
                focused: true,
            } => {
                draw_assistant_pane(frame, area, app, conv, true);
            }
            // Otherwise the focused pane is always the editor (`App::buffer`).
            _ => Editor(app, active).render(frame, area),
        }
        return;
    }

    let (direction, divider_borders) = match view.dir {
        SplitDir::SideBySide => (Direction::Horizontal, Borders::LEFT),
        SplitDir::Stacked => (Direction::Vertical, Borders::TOP),
    };
    // Reserve a one-cell divider between the panes (#163 follow-up) so the editor and
    // the reference/second pane read as distinct regions: a dim vertical rule side by
    // side, a horizontal one when stacked.
    let halves = Layout::default()
        .direction(direction)
        .constraints([
            Constraint::Percentage(view.first_ratio),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);
    let (focused_area, other_area) = if view.focused_first {
        (halves[0], halves[2])
    } else {
        (halves[2], halves[0])
    };
    frame.render_widget(
        Block::default()
            .borders(divider_borders)
            .border_style(Style::default().fg(app.theme.dim)),
        halves[1],
    );
    // Record the pane rects so a mouse click in the other pane can switch focus to it
    // (#163 follow-up); the divider column is excluded, so a click on it is ignored.
    app.pane_rects.set(Some((focused_area, other_area)));

    // The focused editor (`App::buffer`) always draws in `focused_area`; the other
    // pane shows a second editor or a docked surface.
    editor::render_focused_pane(frame, focused_area, app, active);
    match view.content {
        SplitContent::Editor(other) => {
            editor::render_reference_pane(frame, other_area, app, other, active);
        }
        SplitContent::Surface(overlay) => {
            // Feed the surface's scroll bound the height it's actually drawn in.
            app.surface_height.set(other_area.height);
            draw_overlay(frame, other_area, &app.project.manifest.title, overlay);
        }
        SplitContent::Assistant { conv, focused } => {
            draw_assistant_pane(frame, other_area, app, conv, focused);
        }
    }
}

/// The accent style for the active/current item in the nav pane.
pub fn active_style() -> Style {
    Style::default()
        .fg(theme::current().heading)
        .add_modifier(Modifier::BOLD)
}

impl Pane for StatusBar<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.0;
        let theme = app.theme;

        if let Mode::Prompt(prompt_state) = &app.mode {
            draw_prompt(frame, area, frame.area(), prompt_state, &theme);
            return;
        }

        // Find / replace (#144): the query line plus a match count or the
        // available actions for the current step.
        if let Mode::Search(s) = &app.mode {
            let style = Style::default().fg(theme.on_accent).bg(theme.match_current);
            let prompt = Paragraph::new(Line::from(Span::styled(search_status(s), style)));
            frame.render_widget(prompt, area);
            return;
        }

        let (left, right) = if let Mode::Nav {
            selected,
            filter,
            typing,
        } = &app.mode
        {
            nav_status_bar(app, *selected, filter, *typing)
        } else {
            editor_status_bar(app, self.1)
        };

        let chip = mode_chip(app);
        let chip_w = chip.as_ref().map_or(0, |s| s.content.width());

        let pad = (area.width as usize)
            .saturating_sub(chip_w + left.width() + right.width())
            .max(1);
        let mut spans = Vec::new();
        spans.extend(chip);
        spans.push(Span::styled(
            left,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(right, Style::default().fg(theme.dim)));
        let line = Line::from(spans);

        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(theme.status_bg)),
            area,
        );
    }
}

#[cfg(test)]
mod tests;

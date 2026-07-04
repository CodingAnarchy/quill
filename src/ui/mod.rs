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
use crate::frontmatter::{Band, Epigraph, FrontMatter, Role, Status};
use crate::manuscript::SegmentHeader;
use crate::mode::{BibleReview, Mode, Overlay, OverlayKind, RefFiles, Search, SearchStage, Tree};
use crate::syntax::{Heading, Highlight, HighlightKind, Syntax};
use crate::theme::{self, Theme};

mod assistant;
mod editor;
mod list_overlay;
mod overlay;
use assistant::{draw_assistant_pane, too_cramped_to_split};
use list_overlay::{
    draw_annotations, draw_corkboard, draw_dir_browser, draw_findings, draw_history,
    draw_import_preview, draw_list_overlay, draw_model_manager, draw_palette, draw_project_browser,
    draw_project_search, list_row, marker, row_style,
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

/// The prose column: centered, fixed-width text following the cursor.
struct Editor<'a>(&'a App);
/// The left strip: chapter list with the active chapter's heading outline
/// nested beneath it (current section highlighted; the Nav-mode selection
/// reversed).
struct NavPane<'a>(&'a App);
/// The bottom line: chapter name / counts, or the active prompt.
struct StatusBar<'a>(&'a App);

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
    draw_editor_base(frame, area, app);

    // Box overlays each render through the shared `modal` frame (dim + shadow);
    // a prompt lives on the status line, so it only dims the editor behind it.
    // A fresh frame: clear the click→list-index map (#812); the list modes below
    // re-record it when they draw, so a click only ever selects a row that's on
    // screen this frame.
    app.list_layout.borrow_mut().clear();
    match &app.mode {
        Mode::BibleReview(review) => modal(frame, area, |f, r| draw_bible_review(f, r, review)),
        Mode::Findings(findings) => modal(frame, area, |f, r| {
            draw_findings(f, r, findings, &app.project, &app.list_layout);
        }),
        Mode::History(history) => {
            let chapter = app.project.active_title().unwrap_or("this chapter");
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
        Mode::Prompt(_) => dim_backdrop(frame, area),
        _ => {}
    }
}

/// Draw the editor base layer: the prose pane (with the outline strip when shown)
/// and the status line beneath it. Always drawn; modal overlays float on top.
fn draw_editor_base(frame: &mut Frame, area: Rect, app: &App) {
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
        NavPane(app).render(frame, cols[0]);
        cols[1]
    } else {
        chunks[0]
    };

    draw_editor_region(frame, editor_area, app);
    StatusBar(app).render(frame, chunks[1]);
}

/// Layer a floating modal over the already-drawn editor (#349): dim the editor as
/// a backdrop, drop a shadow, clear the box area, then hand the inner rect to the
/// overlay's own renderer. The single place every box overlay gets visual depth,
/// so they're all treated alike rather than each opting in.
fn modal(frame: &mut Frame, screen: Rect, draw_box: impl FnOnce(&mut Frame, Rect)) {
    dim_backdrop(frame, screen);
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

/// De-emphasize the editor behind a modal by dimming every cell above the status
/// line (#349). O(area), about what the live word count already costs per frame.
/// `Modifier::DIM` degrades gracefully — a terminal that ignores it just shows the
/// editor at normal brightness, with the framed, shadowed overlay still on top.
fn dim_backdrop(frame: &mut Frame, screen: Rect) {
    let buf = frame.buffer_mut();
    let bottom = screen.bottom().saturating_sub(1); // keep the status line legible
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
fn draw_editor_region(frame: &mut Frame, area: Rect, app: &App) {
    let Some(view) = app.split_view() else {
        Editor(app).render(frame, area);
        return;
    };

    // Below the threshold a split would render both panes too thin to use — draw
    // only the focused pane (#166). The split state survives, so widening the
    // terminal restores it; this mirrors how the outline pane yields when narrow.
    if too_cramped_to_split(view.dir, area) {
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
            _ => Editor(app).render(frame, area),
        }
        return;
    }

    let direction = match view.dir {
        SplitDir::SideBySide => Direction::Horizontal,
        SplitDir::Stacked => Direction::Vertical,
    };
    let halves = Layout::default()
        .direction(direction)
        .constraints([
            Constraint::Percentage(view.first_ratio),
            Constraint::Percentage(100 - view.first_ratio),
        ])
        .split(area);
    let (focused_area, other_area) = if view.focused_first {
        (halves[0], halves[1])
    } else {
        (halves[1], halves[0])
    };

    // The focused editor (`App::buffer`) always draws in `focused_area`; the other
    // pane shows a second editor or a docked surface.
    editor::render_focused_pane(frame, focused_area, app);
    match view.content {
        SplitContent::Editor(other) => editor::render_reference_pane(frame, other_area, app, other),
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

/// The index of the heading whose section the cursor is currently in: the last
/// heading at or before the cursor. `None` if the cursor sits above the first
/// heading (or there are no headings).
fn current_section(headings: &[Heading], cursor_byte: usize) -> Option<usize> {
    headings.iter().rposition(|h| h.start <= cursor_byte)
}

/// The accent style for the active/current item in the nav pane.
pub fn active_style() -> Style {
    Style::default()
        .fg(theme::current().heading)
        .add_modifier(Modifier::BOLD)
}

/// Accent color for a chapter's revision status badge.
fn status_style(status: Status) -> Style {
    let theme = theme::current();
    let color = match status {
        Status::Draft => theme.dim,
        Status::Revised => theme.revised,
        Status::Final => theme.added,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// One chapter row: a marker for the active chapter, its word count, and a
/// colored status badge — reversed while it's the Nav-mode selection.
#[allow(clippy::too_many_arguments)] // a chapter row's fields read clearer flat than boxed
fn chapter_line(
    title: &str,
    number: Option<usize>,
    words: usize,
    goal: Option<usize>,
    status: Option<Status>,
    role: Option<Role>,
    is_active: bool,
    is_selected: bool,
) -> Line<'static> {
    let mut style = if is_active {
        active_style()
    } else {
        Style::default()
    };
    if is_selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let marker = if is_active { "▸ " } else { "  " };
    // A grouped body chapter (#181) shows its 1-based number; sections don't.
    let label = number.map_or_else(
        || format!("{marker}{title}"),
        |n| format!("{marker}{n}. {title}"),
    );
    // With a goal (#146), show progress (a ✓ once reached); otherwise just the count.
    let count = goal.map_or_else(
        || words.to_string(),
        |g| format!("{words} / {g}{}", if words >= g { " ✓" } else { "" }),
    );
    // The selection highlight reverses every span on the row.
    let sel = |s: Style| {
        if is_selected {
            s.add_modifier(Modifier::REVERSED)
        } else {
            s
        }
    };
    let mut spans = vec![Span::styled(format!("{label}  {count}"), style)];
    // A front/back-matter section shows its role (#179) as a dim italic tag.
    if let Some(role) = role {
        let tag = Style::default()
            .fg(theme::current().dim)
            .add_modifier(Modifier::ITALIC);
        spans.push(Span::styled(format!(" · {}", role.label()), sel(tag)));
    }
    if let Some(status) = status {
        spans.push(Span::styled(
            format!(" {}", status.badge()),
            sel(status_style(status)),
        ));
    }
    Line::from(spans)
}

/// The reference-file browser's rows (#165/#590): a header, then each file as its
/// path relative to the project root (the bible structure stays legible), the
/// selection reversed. The header and hint reflect whether it's the bible-only
/// view (#590) or the full reference list (#165).
fn ref_file_lines(app: &App, state: &RefFiles) -> Vec<Line<'static>> {
    let (title, hint) = if state.bible_only {
        ("Story bible", "enter edit · esc back")
    } else {
        ("Reference files", "enter open · f/esc back")
    };
    let mut lines = vec![
        Line::from(Span::styled(
            title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            hint.to_string(),
            Style::default().fg(theme::current().dim),
        )),
        Line::raw(""),
    ];
    for (i, path) in state.files.iter().enumerate() {
        let label = path
            .strip_prefix(&app.project.root)
            .unwrap_or(path)
            .to_string_lossy();
        lines.push(list_row(i == state.selected, &label, None));
    }
    lines
}

/// The active chapter's heading outline, indented by level, current section
/// accented. `skip` omits one heading by index (used to drop a leading H1 that
/// just repeats the chapter title) while leaving the remaining indices — and so
/// the `current` highlight — intact. `selected` is the focused-outline cursor
/// (#657): when set, that heading is drawn reversed so the author sees which one
/// Enter will jump to; otherwise the outline is the plain display list.
fn heading_outline_lines(
    headings: &[Heading],
    current: Option<usize>,
    selected: Option<usize>,
    skip: Option<usize>,
    query: &str,
) -> Vec<Line<'static>> {
    headings
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != skip)
        // When the focused outline is filtered (#822), show only headings whose
        // title matches; the index numbering is untouched so `selected` still
        // lands on the right heading.
        .filter(|(_, h)| query.is_empty() || h.title.to_lowercase().contains(query))
        .map(|(i, h)| {
            let indent = "  ".repeat(h.level as usize);
            let mut style = if Some(i) == current {
                active_style()
            } else {
                Style::default().fg(theme::current().muted)
            };
            if Some(i) == selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            Line::from(Span::styled(format!("{indent}{}", h.title), style))
        })
        .collect()
}

/// If the chapter opens with a level-1 heading whose text matches the chapter
/// title, the outline would just repeat the title — return its index to skip it.
fn redundant_title_heading(headings: &[Heading], chapter_title: &str) -> Option<usize> {
    match headings.first() {
        Some(h) if h.level == 1 && h.title == chapter_title => Some(0),
        _ => None,
    }
}

/// The top of the nav pane: book / series metadata above the chapter list.
/// Series (if any) sits above the book title; subtitle and author follow.
fn project_header(manifest: &crate::project::Manifest) -> Vec<Line<'static>> {
    let theme = theme::current();
    let mut lines = Vec::new();
    if let Some(series) = &manifest.series {
        lines.push(Line::from(Span::styled(
            series.clone(),
            Style::default().fg(theme.dim),
        )));
    }
    lines.push(Line::from(Span::styled(
        manifest.title.clone(),
        active_style(),
    )));
    if let Some(subtitle) = &manifest.subtitle {
        lines.push(Line::from(Span::styled(
            subtitle.clone(),
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    if let Some(author) = &manifest.author {
        lines.push(Line::from(Span::styled(
            format!("by {author}"),
            Style::default().fg(theme.dim),
        )));
    }
    lines.push(Line::raw(""));
    lines
}

/// A band header for the chapter pane (#181): "CHAPTERS" for the body — so an
/// all-body project reads exactly as before — and "FRONT MATTER" / "BACK MATTER"
/// for the section bands.
fn band_header(band: Band) -> Line<'static> {
    let label = match band {
        Band::Front => "FRONT MATTER",
        Band::Body => "CHAPTERS",
        Band::Back => "BACK MATTER",
    };
    Line::from(Span::styled(
        label.to_string(),
        Style::default()
            .fg(theme::current().dim)
            .add_modifier(Modifier::BOLD),
    ))
}

impl Pane for NavPane<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.0;
        let block = Block::default().borders(Borders::RIGHT);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // The reference-file browser (#165) replaces the chapter list with the
        // bible entries and notes the author can open in a side pane.
        if let Mode::RefFiles(state) = &app.mode {
            frame.render_widget(Paragraph::new(ref_file_lines(app, state)), inner);
            return;
        }

        let mut lines: Vec<Line> = project_header(&app.project.manifest);

        let headings = app.buffer.syntax().headings();
        let current_heading = current_section(headings, app.buffer.cursor_byte());
        // When the outline is focused (#657), draw its selection cursor reversed.
        let outline_selected = match &app.mode {
            Mode::Outline { selected, .. } => Some(*selected),
            _ => None,
        };
        // When the outline is focused with an active filter (#822), the pane shows
        // only headings whose title matches; the query is lowercased once here.
        let outline_query = app.outline_query();
        let outline_typing = matches!(&app.mode, Mode::Outline { typing: true, .. });

        // Resolve the active index once: active_index() is an O(n) scan, so
        // calling it per row would be O(n²) per frame.
        let active = app.project.active_index();

        // The browser selection, if we're in Nav mode.
        let nav_selected = match &app.mode {
            Mode::Nav { selected, .. } => Some(*selected),
            _ => None,
        };
        // When a quick-jump filter is active (#410), the list shows only the
        // chapters that match; body numbering still counts every chapter so the
        // numbers stay stable as you filter. The query is lowercased once here,
        // not once per chapter.
        let filtering = matches!(&app.mode, Mode::Nav { filter, .. } if !filter.is_empty());
        let nav_query = app.nav_query();

        // Count each chapter once for this frame, then reuse for the rows and
        // the total — so the active chapter's live count isn't recomputed twice.
        // Reuse the `active` index resolved above: `chapter_word_count` would
        // otherwise re-run the O(n) `active_index()` scan per chapter, making
        // the counts loop O(chapters²) per frame.
        let counts: Vec<usize> = (0..app.project.manifest.chapters.len())
            .map(|i| app.chapter_word_count_with_active(i, active))
            .collect();
        // Resolve each chapter's band once, like the counts above.
        let bands: Vec<Band> = (0..counts.len())
            .map(|i| app.project.chapter_band(i))
            .collect();

        // Group by band (#181) only when the project actually has sections, so an
        // all-body project keeps the flat, un-numbered "CHAPTERS" list it had.
        let grouped = bands.iter().any(|&b| b != Band::Body);
        let mut shown_band: Option<Band> = None;
        let mut body_n = 0usize;
        for (i, chapter) in app.project.manifest.chapters.iter().enumerate() {
            let band = bands[i];
            // Body chapters carry a 1-based number once the list is grouped;
            // sections show their role instead. Counted before the filter skip so
            // a filtered list still shows each chapter's real number.
            let number = (grouped && band == Band::Body).then(|| {
                body_n += 1;
                body_n
            });
            if filtering && !app.nav_chapter_matches(i, &nav_query) {
                continue;
            }
            // The band header is emitted lazily, just before the first matching
            // chapter of a band, so a filter that hides a whole band hides its
            // header too.
            if shown_band != Some(band) {
                if shown_band.is_some() {
                    lines.push(Line::raw("")); // a gap between bands
                }
                lines.push(band_header(band));
                lines.push(Line::raw(""));
                shown_band = Some(band);
            }

            let is_active = i == active;
            let is_selected = nav_selected == Some(i);
            let status = app.project.chapter_status(i);
            let goal = app.project.chapter_goal(i);
            let role = app.project.chapter_role(i);
            lines.push(chapter_line(
                &chapter.title,
                number,
                counts[i],
                goal,
                status,
                role,
                is_active,
                is_selected,
            ));
            if is_active {
                let skip = redundant_title_heading(headings, &chapter.title);
                lines.extend(heading_outline_lines(
                    headings,
                    current_heading,
                    outline_selected,
                    skip,
                    &outline_query,
                ));
                // While filtering the focused outline (#822), show the query (a
                // trailing `_` caret while typing) under the narrowed headings.
                if outline_typing || !outline_query.is_empty() {
                    let caret = if outline_typing { "_" } else { "" };
                    lines.push(Line::from(Span::styled(
                        format!("  filter: {outline_query}{caret}"),
                        Style::default().fg(theme::current().dim),
                    )));
                }
            }
        }

        // Rolled-up totals: the body word count, kept clear of any front/back
        // matter so the manuscript total isn't muddied (#181).
        let total: usize = counts.iter().sum();
        let body_total: usize = bands
            .iter()
            .zip(&counts)
            .filter(|(&b, _)| b == Band::Body)
            .map(|(_, &c)| c)
            .sum();
        let total_line = if total != body_total {
            format!("{body_total} words · {} in sections", total - body_total)
        } else {
            format!("{total} words")
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            total_line,
            Style::default().fg(theme::current().dim),
        )));

        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// The status-bar line for the find/replace mode (#144): the query, a match
/// count, and the keys available at the current step.
fn search_status(s: &Search) -> String {
    let count = if s.matches.is_empty() {
        if s.query.is_empty() {
            String::new()
        } else {
            "  no matches".to_string()
        }
    } else {
        format!("  {}/{}", s.current + 1, s.matches.len())
    };
    // The active match modes (#570): a short tag after the query, e.g. "[Aa w]".
    let modes = match s.opts.label() {
        ref m if m.is_empty() => String::new(),
        m => format!(" {m}"),
    };
    match s.stage {
        // Vim search (#206) shows its `/` / `?` prompt; the Ctrl-F find labels itself.
        SearchStage::Query if s.from_vim => {
            format!(" {}{}{count}", if s.backward { '?' } else { '/' }, s.query)
        }
        SearchStage::Query => format!(" Find: {}{modes}{count}  · ^C case · ^W word", s.query),
        SearchStage::Navigate => {
            format!(
                " Find: {}{modes}{count}  · n next · N prev · r replace · ^C case · ^W word · Esc done",
                s.query
            )
        }
        SearchStage::Replacement => {
            format!(" Replace '{}'{modes} with: {}", s.query, s.replacement)
        }
        SearchStage::Step => format!(
            " Replace '{}' → '{}'?{count}  · y · n skip · a all · Esc done ({} done)",
            s.query, s.replacement, s.replaced
        ),
    }
}

/// Save-state indicator (#409): ● when there are unsaved changes, ✓ when the
/// buffer is on disk — so persistence is legible at a glance.
const fn dirty_marker(dirtied: bool) -> &'static str {
    if dirtied {
        " ●"
    } else {
        " ✓"
    }
}

/// Persistent AI indicator (independent of the transient message, which clears
/// on the next keystroke): in-flight spinner, else an enabled tag. While a
/// request is in flight, name the backend it's actually running on (#372) —
/// `⟳ Claude` or `⟳ local` — so a routed pass is visible.
fn ai_tag(app: &App) -> String {
    if app.ai_busy() {
        format!(" · ⟳ {}", app.ai_in_flight_label())
    } else if app.ai.enabled {
        " · AI".to_string()
    } else {
        String::new()
    }
}

/// Version-control indicator (#54/#132): a clean repo shows "git", an
/// uncommitted one shows the changed-file count.
fn vc_tag(app: &App) -> String {
    match (app.under_vc, app.vc_changes) {
        (false, _) => String::new(),
        (true, 0) => " · git".into(),
        (true, n) => format!(" · git ●{n}"),
    }
}

/// A running writing sprint (#299): live countdown + net words written, always
/// visible (independent of the transient message on the right).
fn sprint_tag(app: &App) -> String {
    app.sprint.as_ref().map_or_else(String::new, |s| {
        format!(
            " · {}",
            crate::app::sprint::status_segment(s.remaining(), s.words(app.project_word_count()))
        )
    })
}

/// While browsing, the bar describes the *selected* chapter: its title, and
/// (unless a transient message is up) its summary / POV / status.
fn nav_status_bar(app: &App, selected: usize, filter: &str, typing: bool) -> (String, String) {
    let title = app.project.chapter_title(selected).unwrap_or("[untitled]");
    // A quick-jump filter (#410) takes over the bar: the live query and a
    // match count on the left, the highlighted chapter and the keys on the
    // right. `/` enters this; Esc keeps the filter (commands now act on
    // the narrowed list), a second Esc clears it.
    if typing || !filter.is_empty() {
        let n = app.nav_match_count();
        let count = match n {
            0 => "  no matches".to_string(),
            1 => "  1 match".to_string(),
            n => format!("  {n} matches"),
        };
        let hint = if typing {
            "  · Enter open · Esc keep filter"
        } else {
            "  · Esc clears · / refine"
        };
        (format!(" /{filter}{count}"), format!("{title}{hint}  "))
    } else {
        let right = app.message.as_ref().map_or_else(
            || {
                let mut parts = Vec::new();
                if let Some(s) = app.project.chapter_summary(selected) {
                    parts.push(s.to_string());
                }
                if let Some(p) = app.project.chapter_pov(selected) {
                    parts.push(format!("POV: {p}"));
                }
                if let Some(st) = app.project.chapter_status(selected) {
                    parts.push(st.label().to_string());
                }
                if parts.is_empty() {
                    String::new()
                } else {
                    format!("{}  ", parts.join("  ·  "))
                }
            },
            |m| format!("{m}  "),
        );
        (format!(" {title}"), right)
    }
}

/// The editor bar: the active chapter's name and save state, the persistent
/// AI / version-control / sprint tags, and a word/line count (or a transient
/// message) on the right.
fn editor_status_bar(app: &App) -> (String, String) {
    let buffer = app.edit_buffer();
    // The chapter the cursor is in: in continuous view that's the segment
    // under the cursor (with a marker); otherwise the single active chapter.
    let (name, dirtied) = app.manuscript.as_ref().map_or_else(
        // The focused buffer's own title — usually the active chapter, but
        // a reference pane swapped into focus (#165) shows its file name.
        || (app.pane_title_for(buffer), buffer.dirty),
        |m| (format!("≡ {}", m.current_title()), m.dirty()),
    );
    let dirty = dirty_marker(dirtied);
    let right = app.message.as_ref().map_or_else(
        || {
            format!(
                "{} words · ln {}/{}  ",
                buffer.word_count(),
                buffer.cursor.line + 1,
                buffer.line_count()
            )
        },
        |m| format!("{m}  "),
    );
    let left = format!(
        " {name}{dirty}{}{}{}",
        vc_tag(app),
        ai_tag(app),
        sprint_tag(app)
    );
    (left, right)
}

/// Vim mode (#27): a chip at the far left naming the editing mode. Only while
/// actually in the editor (not browsing or editing a metadata field).
fn mode_chip(app: &App) -> Option<Span<'static>> {
    let theme = app.theme;
    match app.mode {
        _ if !app.vim => None, // Vim mode off: no chip
        Mode::Insert => Some(("INSERT", theme.mode_insert)),
        Mode::Visual {
            linewise: false, ..
        } => Some(("VISUAL", theme.mode_visual)),
        Mode::Visual { linewise: true, .. } => Some(("VISUAL LINE", theme.mode_visual)),
        Mode::Normal => Some(("NORMAL", theme.mode_normal)),
        _ => None, // not in the editor (browsing, prompt, …)
    }
    .map(|(label, bg)| {
        Span::styled(
            format!(" {label} "),
            Style::default()
                .fg(theme.on_accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )
    })
}

impl Pane for StatusBar<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.0;
        let theme = app.theme;

        if let Mode::Prompt(prompt_state) = &app.mode {
            // Label plus any typed text (empty for y/N confirmations) — two
            // borrowed spans, no per-frame allocation.
            let style = Style::default().fg(theme.on_accent).bg(theme.match_current);
            let prompt = Paragraph::new(Line::from(vec![
                Span::styled(prompt_state.label.as_str(), style),
                Span::styled(prompt_state.input.as_str(), style),
            ]));
            frame.render_widget(prompt, area);
            // Draw the caret at its position within the typed text (#706); the
            // text prompts accept caret movement, so the block cursor has to
            // follow it rather than sit at the end. y/N confirmations carry no
            // input, so the caret rests right after the label.
            let before =
                prompt_state.label.width() + prompt_state.input[..prompt_state.caret].width();
            let col = area
                .x
                .saturating_add(before as u16)
                .min(area.right().saturating_sub(1));
            frame.set_cursor_position((col, area.y));
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
            editor_status_bar(app)
        };

        let chip = mode_chip(app);
        let chip_w = chip.as_ref().map_or(0, |s| s.content.chars().count());

        let pad = (area.width as usize)
            .saturating_sub(chip_w + left.len() + right.len())
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

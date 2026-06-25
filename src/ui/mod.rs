//! Rendering. Quill draws a centered, fixed-width column of prose with a thin
//! status line, keeping the eye in one place the way a printed page does.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Shadow, Wrap},
    Frame,
};

use crate::app::App;
use crate::frontmatter::{Band, Epigraph, FrontMatter, Role, Status};
use crate::mode::{BibleReview, Mode, Overlay, OverlayKind, RefFiles, Search, SearchStage, Tree};
use crate::app::{SplitContent, SplitDir};
use crate::manuscript::SegmentHeader;
use crate::syntax::{Heading, Highlight, HighlightKind, Syntax};

mod editor;
mod list_overlay;
use list_overlay::{
    draw_annotations, draw_dir_browser, draw_findings, draw_list_overlay, draw_model_manager, draw_palette,
    draw_project_browser, draw_project_search, list_row, marker, row_style,
};

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
    crate::theme::set_current(app.theme);
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
    match &app.mode {
        Mode::BibleReview(review) => modal(frame, area, |f, r| draw_bible_review(f, r, review)),
        Mode::Findings(findings) => modal(frame, area, |f, r| draw_findings(f, r, findings, &app.project)),
        Mode::Annotations { selected } => modal(frame, area, |f, r| draw_annotations(f, r, app, *selected)),
        Mode::ModelManager { selected } => modal(frame, area, |f, r| draw_model_manager(f, r, app, *selected)),
        Mode::ProjectSearch(ps) => modal(frame, area, |f, r| draw_project_search(f, r, ps)),
        Mode::Palette(palette) => modal(frame, area, |f, r| draw_palette(f, r, palette)),
        Mode::Tree(tree) => modal(frame, area, |f, r| draw_tree(f, r, app, tree)),
        Mode::ProjectBrowser(browser) => {
            modal(frame, area, |f, r| draw_project_browser(f, r, browser, &app.project.root))
        }
        Mode::DirBrowser(browser) => modal(frame, area, |f, r| draw_dir_browser(f, r, browser)),
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
    let shadow = Shadow::dark_shade().style(Style::default().fg(crate::theme::current().dim));
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
pub(crate) fn render_to_string(app: &App, w: u16, h: u16) -> String {
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
        match (&view.content, app.surface_pane_focused()) {
            // The only full-area surface case: a docked report with scroll focus.
            (SplitContent::Surface(overlay), true) => {
                app.surface_height.set(area.height);
                draw_overlay(frame, area, &app.project.manifest.title, overlay);
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
        .constraints([Constraint::Percentage(view.first_ratio), Constraint::Percentage(100 - view.first_ratio)])
        .split(area);
    let (focused_area, other_area) =
        if view.focused_first { (halves[0], halves[1]) } else { (halves[1], halves[0]) };

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
    }
}

/// Whether a split along `dir` would leave panes too small to use in `area`.
fn too_cramped_to_split(dir: SplitDir, area: Rect) -> bool {
    match dir {
        SplitDir::SideBySide => area.width < MIN_PANE_WIDTH * 2,
        SplitDir::Stacked => area.height < MIN_PANE_HEIGHT * 2,
    }
}

/// Render the open full-screen overlay in a bordered box, scrolled and clamped to
/// its content. Report overlays show aligned tables (no wrap) titled with the
/// book + a label; the AI panel word-wraps its prose answer. One renderer for all
/// overlay kinds — a new overlay is a new [`OverlayKind`], not a new draw arm.
fn draw_overlay(frame: &mut Frame, area: Rect, book: &str, overlay: &Overlay) {
    let lines = &overlay.lines;
    let scroll = overlay.scroll.min(max_scroll(lines.len(), area.height));
    let hint = scroll_hint(lines.len(), area.height);
    let (title, wrap) = match overlay.kind {
        OverlayKind::Report(label) => (format!(" {book} — {label} ({hint}) "), false),
        OverlayKind::AiPanel => (format!(" AI — Claude ({hint}) "), true),
        // The guide ships pre-wrapped to a readable width; keep it unwrapped so its
        // key-reference columns stay aligned and the scroll bound stays exact.
        OverlayKind::Guide => (format!(" Quill — user's guide ({hint}) "), false),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let mut paragraph = Paragraph::new(lines.clone()).block(block).scroll((scroll, 0));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, area);
}

/// The largest valid scroll offset for `lines` of content in a bordered box of
/// `height` rows: enough to bring the last line into view (the interior is the
/// height minus the two borders), zero when it already fits. The single source
/// of the offset bound, shared by the renderer and the key handler.
pub(crate) fn max_scroll(lines: usize, height: u16) -> u16 {
    (lines as u16).saturating_sub(height.saturating_sub(2))
}

/// The title hint for a scrollable overlay — mentions scrolling only when the
/// content is actually taller than the box.
fn scroll_hint(lines: usize, height: u16) -> &'static str {
    if max_scroll(lines, height) > 0 {
        "↑↓ scroll · any other key to close"
    } else {
        "any key to close"
    }
}

/// Render the story-bible conflict review: a diff of the front conflict (or the
/// scratch editor when editing the proposed version).
fn draw_bible_review(frame: &mut Frame, area: Rect, review: &BibleReview) {
    let Some(front) = review.conflicts.first() else {
        return;
    };
    let remaining = review.conflicts.len();

    // Editing the proposed version before saving.
    if let Some(buf) = &review.editing {
        let block = Block::default().borders(Borders::ALL).title(format!(
            " Edit proposed: {} — Ctrl-S save · Esc back ",
            front.name
        ));
        let text = buf.text();
        let lines: Vec<Line> = text.lines().map(|l| Line::raw(l.to_string())).collect();
        frame.render_widget(
            Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    // Diff view: current (on disk) → proposed.
    let mut lines = vec![
        Line::from(Span::styled(
            format!("'{}' was edited by hand — choose what to keep:", front.name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    let theme = crate::theme::current();
    for change in &front.diff {
        let color = match change.chars().next() {
            Some('-') => theme.removed,
            Some('+') => theme.added,
            _ => theme.dim,
        };
        lines.push(Line::from(Span::styled(change.clone(), Style::default().fg(color))));
    }

    let block = Block::default().borders(Borders::ALL).title(format!(
        " Bible conflict: {} ({remaining} left) — (o)verwrite (e)dit (k)eep · Esc cancel ",
        front.name
    ));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

/// Render the AI command palette as a navigable action list.
/// The series tree navigator (#330): the project as an indented series → book →
/// part → chapter tree. Rows are derived from cached metadata (never files); the
/// selected row is reversed and the active book/chapter is accented.
fn draw_tree(frame: &mut Frame, area: Rect, app: &App, tree: &Tree) {
    let rows = app.tree_rows(&tree.expanded);
    let sel = tree.selected.min(rows.len().saturating_sub(1));
    let title = format!(" {} ", app.project.series_title().unwrap_or("Project"));
    let lines: Vec<Line<'static>> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let text = format!("{}{}{}", marker(i == sel), "  ".repeat(r.indent as usize), r.label);
            let style = if i == sel {
                row_style(true)
            } else if r.active {
                active_style()
            } else {
                Style::default()
            };
            Line::from(Span::styled(text, style))
        })
        .collect();
    draw_list_overlay(
        frame,
        area,
        &title,
        "Project tree — ↑↓ move · → expand · ← collapse · enter open / expand · esc close",
        lines.into_iter(),
    );
}

/// The index of the heading whose section the cursor is currently in: the last
/// heading at or before the cursor. `None` if the cursor sits above the first
/// heading (or there are no headings).
fn current_section(headings: &[Heading], cursor_byte: usize) -> Option<usize> {
    headings.iter().rposition(|h| h.start <= cursor_byte)
}

/// The accent style for the active/current item in the nav pane.
fn active_style() -> Style {
    Style::default().fg(crate::theme::current().heading).add_modifier(Modifier::BOLD)
}

/// Accent color for a chapter's revision status badge.
fn status_style(status: Status) -> Style {
    let theme = crate::theme::current();
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
    let mut style = if is_active { active_style() } else { Style::default() };
    if is_selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let marker = if is_active { "▸ " } else { "  " };
    // A grouped body chapter (#181) shows its 1-based number; sections don't.
    let label = match number {
        Some(n) => format!("{marker}{n}. {title}"),
        None => format!("{marker}{title}"),
    };
    // With a goal (#146), show progress (a ✓ once reached); otherwise just the count.
    let count = match goal {
        Some(g) => format!("{words} / {g}{}", if words >= g { " ✓" } else { "" }),
        None => words.to_string(),
    };
    // The selection highlight reverses every span on the row.
    let sel = |s: Style| if is_selected { s.add_modifier(Modifier::REVERSED) } else { s };
    let mut spans = vec![Span::styled(format!("{label}  {count}"), style)];
    // A front/back-matter section shows its role (#179) as a dim italic tag.
    if let Some(role) = role {
        let tag = Style::default().fg(crate::theme::current().dim).add_modifier(Modifier::ITALIC);
        spans.push(Span::styled(format!(" · {}", role.label()), sel(tag)));
    }
    if let Some(status) = status {
        spans.push(Span::styled(format!(" {}", status.badge()), sel(status_style(status))));
    }
    Line::from(spans)
}

/// The reference-file browser's rows (#165): a header, then each file as its
/// path relative to the project root (the bible structure stays legible), the
/// selection reversed.
fn ref_file_lines(app: &App, state: &RefFiles) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Reference files".to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "enter open · f/esc back".to_string(),
            Style::default().fg(crate::theme::current().dim),
        )),
        Line::raw(""),
    ];
    for (i, path) in state.files.iter().enumerate() {
        let label = path.strip_prefix(&app.project.root).unwrap_or(path).to_string_lossy();
        lines.push(list_row(i == state.selected, label.to_string(), None));
    }
    lines
}

/// The active chapter's heading outline, indented by level, current section
/// accented. `skip` omits one heading by index (used to drop a leading H1 that
/// just repeats the chapter title) while leaving the remaining indices — and so
/// the `current` highlight — intact.
fn heading_outline_lines(
    headings: &[Heading],
    current: Option<usize>,
    skip: Option<usize>,
) -> Vec<Line<'static>> {
    headings
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != skip)
        .map(|(i, h)| {
            let indent = "  ".repeat(h.level as usize);
            let style = if Some(i) == current {
                active_style()
            } else {
                Style::default().fg(crate::theme::current().muted)
            };
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
    let theme = crate::theme::current();
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
            Style::default().fg(theme.muted).add_modifier(Modifier::ITALIC),
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
        Style::default().fg(crate::theme::current().dim).add_modifier(Modifier::BOLD),
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

        // Resolve the active index once: active_index() is an O(n) scan, so
        // calling it per row would be O(n²) per frame.
        let active = app.project.active_index();

        // The browser selection, if we're in Nav mode.
        let nav_selected = match app.mode {
            Mode::Nav { selected } => Some(selected),
            _ => None,
        };

        // Count each chapter once for this frame, then reuse for the rows and
        // the total — so the active chapter's live count isn't recomputed twice.
        let counts: Vec<usize> = (0..app.project.manifest.chapters.len())
            .map(|i| app.chapter_word_count(i))
            .collect();
        // Resolve each chapter's band once, like the counts above.
        let bands: Vec<Band> = (0..counts.len()).map(|i| app.project.chapter_band(i)).collect();

        // Group by band (#181) only when the project actually has sections, so an
        // all-body project keeps the flat, un-numbered "CHAPTERS" list it had.
        let grouped = bands.iter().any(|&b| b != Band::Body);
        let mut shown_band: Option<Band> = None;
        let mut body_n = 0usize;
        for (i, chapter) in app.project.manifest.chapters.iter().enumerate() {
            let band = bands[i];
            if shown_band != Some(band) {
                if shown_band.is_some() {
                    lines.push(Line::raw("")); // a gap between bands
                }
                lines.push(band_header(band));
                lines.push(Line::raw(""));
                shown_band = Some(band);
            }
            // Body chapters carry a 1-based number once the list is grouped;
            // sections show their role instead.
            let number = (grouped && band == Band::Body).then(|| {
                body_n += 1;
                body_n
            });

            let is_active = i == active;
            let is_selected = nav_selected == Some(i);
            let status = app.project.chapter_status(i);
            let goal = app.project.chapter_goal(i);
            let role = app.project.chapter_role(i);
            lines.push(chapter_line(&chapter.title, number, counts[i], goal, status, role, is_active, is_selected));
            if is_active {
                let skip = redundant_title_heading(headings, &chapter.title);
                lines.extend(heading_outline_lines(headings, current_heading, skip));
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
        lines.push(Line::from(Span::styled(total_line, Style::default().fg(crate::theme::current().dim))));

        frame.render_widget(Paragraph::new(lines), inner);
    }
}


/// The status-bar line for the find/replace mode (#144): the query, a match
/// count, and the keys available at the current step.
fn search_status(s: &Search) -> String {
    let count = if s.matches.is_empty() {
        if s.query.is_empty() { String::new() } else { "  no matches".to_string() }
    } else {
        format!("  {}/{}", s.current + 1, s.matches.len())
    };
    match s.stage {
        // Vim search (#206) shows its `/` / `?` prompt; the Ctrl-F find labels itself.
        SearchStage::Query if s.from_vim => format!(" {}{}{count}", if s.backward { '?' } else { '/' }, s.query),
        SearchStage::Query => format!(" Find: {}{count}", s.query),
        SearchStage::Navigate => {
            format!(" Find: {}{count}  · n next · N prev · r replace · Esc done", s.query)
        }
        SearchStage::Replacement => format!(" Replace '{}' with: {}", s.query, s.replacement),
        SearchStage::Step => format!(
            " Replace '{}' → '{}'?{count}  · y · n skip · a all · Esc done ({} done)",
            s.query, s.replacement, s.replaced
        ),
    }
}


impl Pane for StatusBar<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.0;
        let theme = app.theme;
        let buffer = app.edit_buffer();

        if let Mode::Prompt(prompt_state) = &app.mode {
            // Label plus any typed text (empty for y/N confirmations) — two
            // borrowed spans, no per-frame allocation.
            let style = Style::default().fg(theme.on_accent).bg(theme.match_current);
            let prompt = Paragraph::new(Line::from(vec![
                Span::styled(prompt_state.label.as_str(), style),
                Span::styled(prompt_state.input.as_str(), style),
            ]));
            frame.render_widget(prompt, area);
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

        // While browsing, the bar describes the *selected* chapter: its title,
        // and (unless a transient message is up) its summary / POV / status.
        let (left, right) = if let Mode::Nav { selected } = app.mode {
            let title = app.project.chapter_title(selected).unwrap_or("[untitled]");
            let right = match &app.message {
                Some(m) => format!("{m}  "),
                None => {
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
                }
            };
            (format!(" {title}"), right)
        } else {
            // The chapter the cursor is in: in continuous view that's the segment
            // under the cursor (with a marker); otherwise the single active chapter.
            let (name, dirtied) = match &app.manuscript {
                Some(m) => (format!("≡ {}", m.current_title()), m.dirty()),
                // The focused buffer's own title — usually the active chapter, but
                // a reference pane swapped into focus (#165) shows its file name.
                None => (app.pane_title_for(buffer), buffer.dirty),
            };
            let dirty = if dirtied { " ●" } else { "" };
            // Persistent AI indicator (independent of the transient message, which
            // clears on the next keystroke): in-flight spinner, else an enabled tag.
            // While a request is in flight, name the backend it's actually running
            // on (#372) — `⟳ Claude` or `⟳ local` — so a routed pass is visible.
            let ai_tag = if app.ai_busy() {
                format!(" · ⟳ {}", app.ai_in_flight_label())
            } else if app.ai.enabled {
                " · AI".to_string()
            } else {
                String::new()
            };
            // Version-control indicator (#54/#132): a clean repo shows "git", an
            // uncommitted one shows the changed-file count.
            let vc_tag = match (app.under_vc, app.vc_changes) {
                (false, _) => String::new(),
                (true, 0) => " · git".into(),
                (true, n) => format!(" · git ●{n}"),
            };
            let right = match &app.message {
                Some(m) => format!("{m}  "),
                None => format!(
                    "{} words · ln {}/{}  ",
                    buffer.word_count(),
                    buffer.cursor.line + 1,
                    buffer.line_count()
                ),
            };
            (format!(" {name}{dirty}{vc_tag}{ai_tag}"), right)
        };

        // Vim mode (#27): a chip at the far left naming the editing mode. Only
        // while actually in the editor (not browsing or editing a metadata field).
        let mode_chip = match app.mode {
            _ if !app.vim => None, // Vim mode off: no chip
            Mode::Insert => Some(("INSERT", theme.mode_insert)),
            Mode::Visual { linewise: false, .. } => Some(("VISUAL", theme.mode_visual)),
            Mode::Visual { linewise: true, .. } => Some(("VISUAL LINE", theme.mode_visual)),
            Mode::Normal => Some(("NORMAL", theme.mode_normal)),
            _ => None, // not in the editor (browsing, prompt, …)
        }
        .map(|(label, bg)| {
            Span::styled(
                format!(" {label} "),
                Style::default().fg(theme.on_accent).bg(bg).add_modifier(Modifier::BOLD),
            )
        });
        let chip_w = mode_chip.as_ref().map_or(0, |s| s.content.chars().count());

        let pad = (area.width as usize)
            .saturating_sub(chip_w + left.len() + right.len())
            .max(1);
        let mut spans = Vec::new();
        spans.extend(mode_chip);
        spans.push(Span::styled(left, Style::default().add_modifier(Modifier::BOLD)));
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
mod tests {
    use super::*;
    use super::render_to_string as render;
    use crate::buffer::{Buffer, Cursor};
    use crate::project::Project;
    use ratatui::style::Color;
    use ratatui::{backend::TestBackend, Terminal};
    use std::fs;

    fn buffer_with(text: &str) -> Buffer {
        let mut b = Buffer::empty();
        for c in text.chars() {
            b.insert_char(c);
        }
        b.refresh_syntax();
        b
    }

    /// An App over an in-memory scratch project, for render tests.
    fn app_with(text: &str) -> App {
        App::new(buffer_with(text), Project::scratch())
    }

    /// Performance guard (run with `cargo test --release -- --ignored
    /// --nocapture`). A full frame must render well within one 16ms frame, even
    /// for a large chapter shown alongside a many-chapter outline pane — the
    /// realistic worst case for per-frame work (the status bar's live word count
    /// over the whole buffer, plus a nav row per chapter). If a future change
    /// makes rendering re-read files or do per-chapter work proportional to the
    /// novel, this catches it.
    ///
    /// Focus mode and typewriter scrolling (#301) are both on here, over a corpus
    /// with no blank lines — the pathological case where the focused "paragraph"
    /// spans the whole buffer, so `focus_paragraph`'s line walk does its most
    /// work — plus a spread of inline annotations (#302) so their per-row/char
    /// scan is timed. Confirms none of these adds cost that breaks the frame budget.
    #[test]
    #[ignore]
    fn render_stays_within_a_frame() {
        use std::time::Instant;

        let titles: Vec<String> = (0..150).map(|i| format!("Chapter {i}")).collect();
        let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();
        let project = Project::scratch_with(&title_refs);

        let mut text = String::new();
        for _ in 0..7500 {
            text.push_str("The night was dark and the road was very long indeed.\n");
        }
        let words = text.split_whitespace().count();
        let mut buffer = Buffer::from_text(&text);
        buffer.refresh_syntax();
        let mut app = App::new(buffer, project);
        app.outline_visible = true;
        app.focus_mode = true;
        app.typewriter = true;
        // A spread of inline annotations (#302) so the per-row/char annotation
        // scan is exercised under the frame budget too.
        app.annotations = (0..50)
            .map(|i| crate::annotation::Live { start: i * 53, end: i * 53 + 5, note: "note".into() })
            .collect();

        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap(); // warm
        let t = Instant::now();
        for _ in 0..30 {
            terminal.draw(|f| draw(f, &app)).unwrap();
        }
        let dt = t.elapsed() / 30;

        println!("render {words}w + 150-chapter outline = {dt:?}");
        assert!(dt.as_millis() < 16, "render took {dt:?}");

        // And again with a modal overlay open (#349): the same editor plus the
        // O(screen) backdrop dim and the shadowed box. Confirms the depth pass
        // doesn't push a full frame past budget.
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)); // open the run-action palette
        assert!(matches!(app.mode, Mode::Palette(_)));
        terminal.draw(|f| draw(f, &app)).unwrap(); // warm
        let t = Instant::now();
        for _ in 0..30 {
            terminal.draw(|f| draw(f, &app)).unwrap();
        }
        let modal_dt = t.elapsed() / 30;
        println!("render with a modal overlay = {modal_dt:?}");
        assert!(modal_dt.as_millis() < 16, "modal render took {modal_dt:?}");
    }

    #[test]
    fn a_split_collapses_only_once_a_pane_would_be_too_small() {
        let wide = Rect { x: 0, y: 0, width: 100, height: 30 };
        let thin = Rect { x: 0, y: 0, width: 50, height: 30 };
        let short = Rect { x: 0, y: 0, width: 100, height: 8 };
        assert!(!too_cramped_to_split(SplitDir::SideBySide, wide));
        assert!(too_cramped_to_split(SplitDir::SideBySide, thin), "two columns need room");
        assert!(!too_cramped_to_split(SplitDir::Stacked, wide));
        assert!(too_cramped_to_split(SplitDir::Stacked, short), "two rows need room");
    }

    #[test]
    fn long_paragraph_soft_wraps_within_the_focus_column() {
        // One long logical line (a single paragraph, no internal newlines).
        let long = "word ".repeat(60);
        let app = app_with(&format!("{long}\n"));
        let screen = render(&app, 100, 20);

        // It appears on several rows (wrapped), not truncated to one... ("word word"
        // matches body rows, not the status bar's "60 words" count).
        let content_rows: Vec<&str> = screen.lines().filter(|l| l.contains("word word")).collect();
        assert!(content_rows.len() > 1, "the long line wrapped:\n{screen}");
        // ...and no rendered row is wider than the centered focus column (+ pad).
        for row in content_rows {
            assert!(row.trim_end().chars().count() <= 90, "row stays within the column: {row:?}");
        }
    }

    #[test]
    fn vim_mode_is_shown_in_the_status_bar() {
        let mut app = app_with("first chapter\n");
        // Off: no mode chip.
        assert!(!render(&app, 80, 8).contains("NORMAL"), "no chip when Vim is off");
        // On, resting in Normal.
        app.vim = true;
        app.mode = Mode::Normal;
        assert!(render(&app, 80, 8).contains("NORMAL"), "Normal shown");
        // Entering Insert updates the indicator.
        app.mode = Mode::Insert;
        let screen = render(&app, 80, 8);
        assert!(screen.contains("INSERT") && !screen.contains("NORMAL"), "Insert shown:\n{screen}");
        // Visual modes show their own labels.
        app.mode = Mode::Visual { anchor: 0, linewise: false };
        assert!(render(&app, 80, 8).contains("VISUAL"), "Visual shown");
        app.mode = Mode::Visual { anchor: 0, linewise: true };
        assert!(render(&app, 80, 8).contains("VISUAL LINE"), "Visual Line shown");
    }

    #[test]
    fn visual_selection_draws_a_highlight() {
        let mut app = app_with("hello world\n");
        app.vim = true;
        // Select the first few graphemes (anchor at 0, cursor at col 2).
        app.mode = Mode::Visual { anchor: 0, linewise: false };
        app.buffer.cursor = Cursor { line: 0, col: 2 };
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        let highlighted = (0..40).any(|x| (0..6).any(|y| buf[(x, y)].style().bg == Some(Color::Yellow)));
        assert!(highlighted, "the selection carries a highlight background");
    }

    #[test]
    fn markers_are_hidden_when_cursor_is_outside_the_span() {
        // After typing, the cursor sits past the span, so markers conceal.
        let app = app_with("*italic* word");
        let screen = render(&app, 80, 8);
        assert!(screen.contains("italic word"), "screen:\n{screen}");
        assert!(!screen.contains('*'), "markers should be hidden:\n{screen}");
    }

    #[test]
    fn typewriter_pins_the_cursor_line_to_the_centre() {
        // A document taller than the page, cursor resting on the last line.
        let mut text = String::new();
        for i in 0..40 {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&format!("Line{i:02}"));
        }
        let mut app = app_with(&text); // typed in, so the cursor rests at the end
        let h = 21u16;
        let row_of_last = |app: &App| {
            render(app, 40, h)
                .lines()
                .position(|l| l.contains("Line39"))
                .expect("the last line is on screen")
        };

        // Off: the cursor's line has drifted to the bottom of the page.
        app.typewriter = false;
        let off = row_of_last(&app);
        // On: it's lifted back to the vertical centre, text scrolling under it.
        app.typewriter = true;
        let on = row_of_last(&app);

        assert!(on < off, "typewriter lifts the last line off the bottom: on={on} off={off}");
        assert!((8..=12).contains(&on), "pinned near the centre of a 20-row page: {on}");
    }

    #[test]
    fn focus_mode_dims_the_other_paragraphs() {
        // Two paragraphs; the cursor (typed to the end) rests in the second.
        let mut app = app_with("alpha alpha\n\nbravo bravo");
        let dim = app.theme.dim;

        let first_letter_fg = |app: &App, needle: &str| -> Option<Color> {
            let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
            terminal.draw(|f| draw(f, app)).unwrap();
            let buf = terminal.backend().buffer().clone();
            let row = (0..12u16)
                .find(|&y| (0..40).any(|x| buf[(x, y)].symbol() == "a" || buf[(x, y)].symbol() == "b") && {
                    let line: String = (0..40).map(|x| buf[(x, y)].symbol()).collect();
                    line.contains(needle)
                })
                .expect("paragraph on screen");
            (0..40)
                .map(|x| &buf[(x, row)])
                .find(|c| c.symbol().chars().next().is_some_and(|ch| ch.is_alphabetic()))
                .unwrap()
                .style()
                .fg
        };

        // Off (default): both paragraphs render in the normal prose colour.
        assert_ne!(first_letter_fg(&app, "alpha"), Some(dim), "no dimming when focus mode is off");

        // On: the other paragraph dims; the one under the cursor stays normal.
        app.focus_mode = true;
        assert_eq!(first_letter_fg(&app, "alpha"), Some(dim), "the other paragraph dims");
        assert_ne!(first_letter_fg(&app, "bravo"), Some(dim), "the focused paragraph stays normal");
    }

    #[test]
    fn the_model_manager_lists_the_catalog_with_characteristics() {
        let mut app = app_with("text\n");
        app.mode = Mode::ModelManager { selected: 0 };
        let screen = render(&app, 100, 24);
        assert!(screen.contains("Local AI models"), "{screen}");
        // Models appear with their context window and description.
        assert!(screen.contains("Qwen2.5-7B"), "the catalog is listed:\n{screen}");
        assert!(screen.contains("1M ctx"), "the extended-context model's window shows:\n{screen}");
        assert!(screen.contains("Long context"), "descriptions are shown:\n{screen}");
    }

    #[test]
    fn an_open_modal_dims_the_editor_and_casts_a_shadow() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app_with("# Heading one\n\nThe night was dark and long.\n");

        let render = |app: &App| {
            let mut t = Terminal::new(TestBackend::new(80, 20)).unwrap();
            t.draw(|f| draw(f, app)).unwrap();
            t.backend().buffer().clone()
        };

        // No modal open: nothing behind is dimmed.
        let plain = render(&app);
        let plain_dim = (0..80u16)
            .flat_map(|x| (0..20u16).map(move |y| (x, y)))
            .filter(|&(x, y)| plain[(x, y)].modifier.contains(Modifier::DIM))
            .count();
        assert_eq!(plain_dim, 0, "nothing is dimmed without a modal");

        // Palette open: the editor behind dims, and the box casts a shadow.
        app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Palette(_)), "Ctrl-K opened the palette");
        let buf = render(&app);
        let mut dimmed = 0usize;
        let mut shadow = false;
        for y in 0..20u16 {
            for x in 0..80u16 {
                if buf[(x, y)].modifier.contains(Modifier::DIM) {
                    dimmed += 1;
                }
                if buf[(x, y)].symbol() == "▓" {
                    shadow = true;
                }
            }
        }
        assert!(dimmed > 0, "the editor behind the modal is dimmed");
        assert!(shadow, "the modal casts a drop shadow");
    }

    #[test]
    fn an_annotated_span_is_marked_in_the_editor() {
        let mut app = app_with("first chapter\n");
        let anno_bg = app.theme.annotation;
        // Anchor a note to "first" (bytes 0..5).
        app.annotations = vec![crate::annotation::Live { start: 0, end: 5, note: "check".into() }];

        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buf = terminal.backend().buffer().clone();

        let marked = (0..40).any(|x| (0..8).any(|y| buf[(x, y)].style().bg == Some(anno_bg)));
        assert!(marked, "the annotated span carries the annotation background");
    }

    #[test]
    fn markers_reappear_when_cursor_enters_the_span() {
        let mut app = app_with("*italic* word");
        app.buffer.cursor = Cursor { line: 0, col: 3 }; // inside "italic"
        let screen = render(&app, 80, 8);
        assert!(screen.contains("*italic*"), "screen:\n{screen}");
    }

    #[test]
    fn current_section_is_the_last_heading_at_or_before_the_cursor() {
        use crate::syntax::Heading;
        let hs = vec![
            Heading { level: 1, title: "A".into(), start: 0 },
            Heading { level: 2, title: "B".into(), start: 50 },
        ];
        assert_eq!(current_section(&hs, 0), Some(0));
        assert_eq!(current_section(&hs, 49), Some(0));
        assert_eq!(current_section(&hs, 50), Some(1));
        assert_eq!(current_section(&hs, 999), Some(1));

        // Cursor above the first heading, and the no-headings case.
        let later = vec![Heading { level: 1, title: "A".into(), start: 10 }];
        assert_eq!(current_section(&later, 5), None);
        assert_eq!(current_section(&[], 0), None);
    }

    /// An App over an in-memory project with several chapters (no files needed
    /// for rendering, which only reads chapter titles + the active buffer).
    fn app_with_chapters(titles: &[&str], active_text: &str) -> App {
        App::new(buffer_with(active_text), Project::scratch_with(titles))
    }

    #[test]
    fn pane_nests_active_chapter_headings() {
        let mut app = app_with("# Chapter\n\nbody\n\n## Scene\n\nmore\n");
        app.outline_visible = true;
        let screen = render(&app, 90, 16);
        assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
        // The scratch project's chapter, with the in-chapter headings nested.
        assert!(screen.contains("Chapter 1"), "screen:\n{screen}");
        assert!(screen.contains("Scene"), "screen:\n{screen}");
    }

    #[test]
    fn pane_lists_chapter_even_without_headings() {
        let mut app = app_with("just prose, no headings\n");
        app.outline_visible = true;
        let screen = render(&app, 90, 12);
        assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
        assert!(screen.contains("Chapter 1"), "screen:\n{screen}");
    }

    #[test]
    fn outline_skips_a_leading_h1_that_matches_the_chapter_title() {
        use crate::syntax::Heading;
        let headings = vec![
            Heading { level: 1, title: "The Crossing".into(), start: 0 },
            Heading { level: 2, title: "At the ford".into(), start: 30 },
        ];

        // The chapter is titled "The Crossing", so its opening H1 is redundant.
        let skip = redundant_title_heading(&headings, "The Crossing");
        assert_eq!(skip, Some(0));

        let texts: Vec<String> = heading_outline_lines(&headings, None, skip)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts.len(), 1);
        assert!(texts[0].contains("At the ford"));
        assert!(!texts[0].contains("The Crossing"));

        // A non-matching title keeps every heading.
        assert_eq!(redundant_title_heading(&headings, "Different"), None);
        assert_eq!(
            heading_outline_lines(&headings, None, None).len(),
            2
        );
    }

    #[test]
    fn pane_lists_all_chapters_and_marks_the_active_one() {
        let mut app = app_with_chapters(&["One", "Two", "Three"], "body\n");
        app.outline_visible = true;
        let screen = render(&app, 90, 14);
        assert!(screen.contains("One"), "screen:\n{screen}");
        assert!(screen.contains("Two"), "screen:\n{screen}");
        assert!(screen.contains("Three"), "screen:\n{screen}");
        // The active chapter (index 0) carries the active marker.
        assert!(screen.contains("▸ One"), "screen:\n{screen}");
    }

    #[test]
    fn outline_shows_chapter_goal_progress() {
        let mut app = app_with_chapters(&["One", "Two"], "alpha beta gamma\n"); // 3 words, active
        app.outline_visible = true;

        app.project.manifest.chapters[0].goal = Some(10);
        let screen = render(&app, 90, 12);
        assert!(screen.contains("3 / 10"), "progress toward the goal:\n{screen}");

        // A reached goal is marked.
        app.project.manifest.chapters[0].goal = Some(2);
        let screen = render(&app, 90, 12);
        assert!(screen.contains("3 / 2 ✓"), "reached goal marked:\n{screen}");
    }

    #[test]
    fn nav_pane_shows_the_project_header() {
        let mut app = app_with_chapters(&["One", "Two"], "body\n");
        app.project.manifest.title = "Book of Dust".into();
        app.project.manifest.subtitle = Some("La Belle Sauvage".into());
        app.project.manifest.author = Some("Philip Pullman".into());
        app.project.manifest.series = Some("The Book of Dust".into());
        app.outline_visible = true;

        let screen = render(&app, 90, 16);
        assert!(screen.contains("Book of Dust"), "screen:\n{screen}");
        assert!(screen.contains("La Belle Sauvage"), "screen:\n{screen}");
        assert!(screen.contains("by Philip Pullman"), "screen:\n{screen}");
        assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
    }

    #[test]
    fn epigraphs_render_above_the_body() {
        // Book epigraph (manifest) + chapter epigraph (active buffer front matter)
        // both render above chapter 0's prose, with attributions.
        let mut app = app_with("# Chapter\n\nThe prose begins here.\n");
        app.project.manifest.epigraph = Some("In the beginning.".into());
        app.project.manifest.epigraph_by = Some("Genesis".into());
        let fm = app.buffer.front_matter_mut();
        fm.set_epigraph_text("A chapter quote.");
        fm.set_epigraph_by("Someone");

        let screen = render(&app, 80, 20);
        assert!(screen.contains("In the beginning."), "book epigraph:\n{screen}");
        assert!(screen.contains("— Genesis"), "book attribution:\n{screen}");
        assert!(screen.contains("A chapter quote."), "chapter epigraph:\n{screen}");
        assert!(screen.contains("— Someone"), "chapter attribution:\n{screen}");
        assert!(screen.contains("The prose begins here."), "body:\n{screen}");

        // The epigraph is display-only: it never entered the editable buffer.
        assert!(!app.buffer.line(0).contains("In the beginning"));
        assert!(!app.buffer.line(0).contains("A chapter quote"));
    }

    #[test]
    fn a_multiline_epigraph_renders_one_line_each() {
        let mut app = app_with("# Chapter\n\nBody.\n");
        let fm = app.buffer.front_matter_mut();
        fm.set_epigraph_text("Roses are red\nViolets are blue");
        fm.set_epigraph_by("Anon");

        let screen = render(&app, 80, 20);
        // Each verse line renders on its own row, attribution beneath.
        let red = screen.lines().position(|l| l.contains("Roses are red"));
        let blue = screen.lines().position(|l| l.contains("Violets are blue"));
        let by = screen.lines().position(|l| l.contains("— Anon"));
        assert!(red.is_some() && blue.is_some() && by.is_some(), "screen:\n{screen}");
        assert!(red < blue && blue < by, "lines out of order:\n{screen}");
    }

    #[test]
    fn editing_an_epigraph_shows_a_banner() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = app_with("Body.\n");
        app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)); // → Nav
        app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)); // → epigraph editor
        assert!(matches!(app.mode, Mode::EditMeta(_)));

        let screen = render(&app, 80, 10);
        assert!(screen.contains("editing chapter epigraph"), "screen:\n{screen}");
        assert!(screen.contains("Ctrl-S save"), "screen:\n{screen}");
    }

    #[test]
    fn pane_shows_a_chapter_status_badge() {
        let mut app = app_with_chapters(&["One", "Two"], "body\n");
        app.project.manifest.chapters[0].status = Some(crate::frontmatter::Status::Final);
        app.outline_visible = true;
        let screen = render(&app, 90, 14);
        let row = screen.lines().find(|l| l.contains("One")).unwrap();
        assert!(row.contains('F'), "status badge missing in row: {row:?}");
    }

    #[test]
    fn nav_status_bar_shows_the_selected_chapters_metadata() {
        let mut app = app_with_chapters(&["One", "Two"], "body\n");
        app.project.manifest.chapters[1].summary = Some("A grand opening".into());
        app.project.manifest.chapters[1].pov = Some("Mara".into());
        app.project.manifest.chapters[1].status = Some(crate::frontmatter::Status::Revised);
        app.outline_visible = true;
        app.mode = Mode::Nav { selected: 1 };
        let screen = render(&app, 90, 14);
        assert!(screen.contains("A grand opening"), "screen:\n{screen}");
        assert!(screen.contains("POV: Mara"), "screen:\n{screen}");
        assert!(screen.contains("Revised"), "screen:\n{screen}");
    }

    #[test]
    fn continuous_view_shows_chapters_flowing_with_a_divider() {
        use crate::manuscript::Manuscript;

        // A two-chapter project on disk (the manuscript window reads files).
        let mut dir = std::env::temp_dir();
        dir.push(format!("quill-ui-cont-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
        )
        .unwrap();
        fs::write(dir.join("one.md"), "Alpha prose here.\n").unwrap();
        fs::write(dir.join("two.md"), "Beta prose here.\n").unwrap();

        let project = Project::open(&dir).unwrap();
        let buffer = Buffer::from_path(project.active_path()).unwrap();
        let mut app = App::new(buffer, project);
        app.manuscript = Some(Manuscript::open(&app.project).unwrap());

        let screen = render(&app, 60, 12);
        // Both chapters' prose appears in one continuous flow...
        assert!(screen.contains("Alpha prose here."), "screen:\n{screen}");
        assert!(screen.contains("Beta prose here."), "screen:\n{screen}");
        // ...separated by a centered divider carrying the next chapter's title.
        assert!(screen.contains("—— Two ——"), "screen:\n{screen}");

        fs::remove_dir_all(&dir).ok();
    }
}

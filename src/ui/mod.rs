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
use crate::frontmatter::{Band, Epigraph, FrontMatter, Role, Status};
use crate::mode::{BibleReview, Mode, Overlay, OverlayKind, RefFiles, Search, SearchStage, Tree};
use crate::app::{Assistant, Speaker, SplitContent, SplitDir};
use crate::theme::{self, Theme};
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
        match &view.content {
            // A docked report with scroll focus takes the full area.
            SplitContent::Surface(overlay) if app.surface_pane_focused() => {
                app.surface_height.set(area.height);
                draw_overlay(frame, area, &app.project.manifest.title, overlay);
            }
            // A focused assistant pane takes the full area (#307).
            SplitContent::Assistant { conv, focused: true } => {
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
        SplitContent::Assistant { conv, focused } => draw_assistant_pane(frame, other_area, app, conv, focused),
    }
}

/// Render the authorial assistant pane (#307): the conversation transcript above a
/// typed request line, in a bordered box that brightens when it has key focus. The
/// transcript is pre-wrapped to the interior width so scrolling is line-exact.
fn draw_assistant_pane(frame: &mut Frame, area: Rect, app: &App, conv: &Assistant, focused: bool) {
    let theme = &app.theme;
    // Title carries the live state and the run's running cost (#309).
    let mut parts: Vec<String> = Vec::new();
    if conv.is_running() {
        parts.push("thinking… (Esc cancels)".into());
    }
    if conv.spent() > 0.0 {
        parts.push(format!("~${:.2}", conv.spent()));
    }
    let title = if parts.is_empty() {
        " Assistant ".to_string()
    } else {
        format!(" Assistant — {} ", parts.join(" · "))
    };
    let border = if focused { theme.heading } else { theme.dim };
    let block = Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // The bottom interior row is the request line; the rest is the transcript.
    let body_h = inner.height.saturating_sub(1) as usize;
    let lines = wrap_entries(conv.entries(), inner.width as usize, theme);
    let max_top = lines.len().saturating_sub(body_h);
    app.assistant_scroll_max.set(max_top as u16);
    let top = max_top.saturating_sub(conv.scroll_up() as usize);
    let shown: Vec<Line> = lines.into_iter().skip(top).take(body_h).collect();
    frame.render_widget(
        Paragraph::new(shown),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: body_h as u16 },
    );

    // The request line, with the cursor placed at its end while focused and idle.
    let input_y = inner.y + body_h as u16;
    let prompt = if conv.is_running() { "… " } else { "> " };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prompt, Style::default().fg(theme.dim)),
            Span::styled(conv.input().to_string(), Style::default().fg(theme.heading)),
        ])),
        Rect { x: inner.x, y: input_y, width: inner.width, height: 1 },
    );
    if focused && !conv.is_running() {
        let col = (prompt.width() + conv.input().width()) as u16;
        let cx = inner.x + col.min(inner.width.saturating_sub(1));
        frame.set_cursor_position((cx, input_y));
    }
}

/// Lay the assistant transcript out as styled rows, each entry labelled by speaker
/// and word-wrapped to `width`, with a blank row between entries for readability.
fn wrap_entries(entries: &[crate::app::Entry], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for e in entries {
        let (label, style) = match e.who {
            Speaker::You => ("You: ", Style::default().fg(theme.heading).add_modifier(Modifier::BOLD)),
            Speaker::Assistant => ("", Style::default()),
            Speaker::Tool => ("• ", Style::default().fg(theme.dim)),
            Speaker::System => ("", Style::default().fg(theme.muted).add_modifier(Modifier::ITALIC)),
        };
        let body = format!("{label}{}", e.text);
        let starts = crate::wrap::row_starts(&body, width);
        for r in 0..starts.len() {
            let s = starts[r];
            let end = starts.get(r + 1).copied().unwrap_or(body.len());
            out.push(Line::from(Span::styled(body[s..end].trim_end().to_string(), style)));
        }
        out.push(Line::raw(""));
    }
    out
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
    let theme = theme::current();
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
    Style::default().fg(theme::current().heading).add_modifier(Modifier::BOLD)
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
        let tag = Style::default().fg(theme::current().dim).add_modifier(Modifier::ITALIC);
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
            Style::default().fg(theme::current().dim),
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
                Style::default().fg(theme::current().muted)
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
        Style::default().fg(theme::current().dim).add_modifier(Modifier::BOLD),
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
        lines.push(Line::from(Span::styled(total_line, Style::default().fg(theme::current().dim))));

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
        let (left, right) = if let Mode::Nav { selected, filter, typing } = &app.mode {
            let selected = *selected;
            let title = app.project.chapter_title(selected).unwrap_or("[untitled]");
            // A quick-jump filter (#410) takes over the bar: the live query and a
            // match count on the left, the highlighted chapter and the keys on the
            // right. `/` enters this; Esc keeps the filter (commands now act on
            // the narrowed list), a second Esc clears it.
            if *typing || !filter.is_empty() {
                let n = app.nav_match_count();
                let count = match n {
                    0 => "  no matches".to_string(),
                    1 => "  1 match".to_string(),
                    n => format!("  {n} matches"),
                };
                let hint = if *typing {
                    "  · Enter open · Esc keep filter"
                } else {
                    "  · Esc clears · / refine"
                };
                (format!(" /{filter}{count}"), format!("{title}{hint}  "))
            } else {
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
            }
        } else {
            // The chapter the cursor is in: in continuous view that's the segment
            // under the cursor (with a marker); otherwise the single active chapter.
            let (name, dirtied) = match &app.manuscript {
                Some(m) => (format!("≡ {}", m.current_title()), m.dirty()),
                // The focused buffer's own title — usually the active chapter, but
                // a reference pane swapped into focus (#165) shows its file name.
                None => (app.pane_title_for(buffer), buffer.dirty),
            };
            // Save-state indicator (#409): ● when there are unsaved changes, ✓ when
            // the buffer is on disk — so persistence is legible at a glance.
            let dirty = if dirtied { " ●" } else { " ✓" };
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
            // A running writing sprint (#299): live countdown + net words written,
            // always visible (independent of the transient message on the right).
            let sprint_tag = match &app.sprint {
                Some(s) => format!(" · {}", crate::app::sprint::status_segment(s.remaining(), s.words(app.project_word_count()))),
                None => String::new(),
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
            (format!(" {name}{dirty}{vc_tag}{ai_tag}{sprint_tag}"), right)
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
mod tests;

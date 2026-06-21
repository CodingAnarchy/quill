//! Rendering. Quill draws a centered, fixed-width column of prose with a thin
//! status line, keeping the eye in one place the way a printed page does.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::app::App;
use crate::frontmatter::{Band, Epigraph, FrontMatter, Role, Status};
use crate::mode::{BibleReview, Findings, Mode, Overlay, OverlayKind, Palette, RefFiles, Search, SearchStage};
use crate::app::{SplitContent, SplitDir};
use crate::manuscript::SegmentHeader;
use crate::syntax::{Heading, Highlight, HighlightKind, Syntax};

mod editor;

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
    let area = frame.area();

    // A full-screen read-only overlay (progress / analysis report or AI answer),
    // scrollable when it runs past the screen.
    if let Some(overlay) = &app.overlay {
        draw_overlay(frame, area, &app.project.manifest.title, overlay);
        return;
    }
    if let Mode::BibleReview(review) = &app.mode {
        draw_bible_review(frame, area, review);
        return;
    }
    if let Mode::Findings(findings) = &app.mode {
        draw_findings(frame, area, findings);
        return;
    }
    if let Mode::Palette(palette) = &app.mode {
        draw_palette(frame, area, palette);
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
        NavPane(app).render(frame, cols[0]);
        cols[1]
    } else {
        chunks[0]
    };

    draw_editor_region(frame, editor_area, app);
    StatusBar(app).render(frame, chunks[1]);
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
    for change in &front.diff {
        let color = match change.chars().next() {
            Some('-') => Color::Red,
            Some('+') => Color::Green,
            _ => Color::DarkGray,
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
fn draw_palette(frame: &mut Frame, area: Rect, palette: &Palette) {
    let rows = palette.items.iter().enumerate().map(|(i, item)| {
        Line::from(vec![
            Span::styled(format!("{}{}", marker(i == palette.selected), item.label), row_style(i == palette.selected)),
            Span::styled(format!("  — {}", item.desc), Style::default().fg(Color::DarkGray)),
        ])
    });
    let title = format!(" {} ", palette.title);
    // The `R` force-rebuild gesture only applies to the AI bible passes (#176).
    let rebuild = if palette.title == "AI" { " · R rebuild bible (full)" } else { "" };
    let help = format!("{} — ↑↓ move · enter run{rebuild} · esc close", palette.title);
    draw_list_overlay(frame, area, &title, &help, rows);
}

/// Render the findings as a navigable triage list.
fn draw_findings(frame: &mut Frame, area: Rect, findings: &Findings) {
    let title = format!(" Findings ({}) ", findings.items.len());
    let rows = findings.items.iter().enumerate().map(|(i, item)| {
        let chapters = if item.chapters.is_empty() {
            String::new()
        } else {
            format!("  [{}]", item.chapters.join(", "))
        };
        Line::from(Span::styled(
            format!("{}{}{chapters}", marker(i == findings.selected), item.summary),
            row_style(i == findings.selected),
        ))
    });
    draw_list_overlay(frame, area, &title, "Findings — ↑↓ move · enter jump · i mark xxx · d dismiss · esc close", rows);
}

/// The selection marker for a list row.
fn marker(selected: bool) -> &'static str {
    if selected {
        "▸ "
    } else {
        "  "
    }
}

/// The style for a list row (reversed when selected).
fn row_style(selected: bool) -> Style {
    if selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    }
}

/// Render a navigable list overlay: a bold help line, a blank, then the rows, in
/// a bordered wrapping box. Shared by the findings and palette overlays.
fn draw_list_overlay(
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

/// The index of the heading whose section the cursor is currently in: the last
/// heading at or before the cursor. `None` if the cursor sits above the first
/// heading (or there are no headings).
fn current_section(headings: &[Heading], cursor_byte: usize) -> Option<usize> {
    headings.iter().rposition(|h| h.start <= cursor_byte)
}

/// The accent style for the active/current item in the nav pane.
fn active_style() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

/// Accent color for a chapter's revision status badge.
fn status_style(status: Status) -> Style {
    let color = match status {
        Status::Draft => Color::DarkGray,
        Status::Revised => Color::Yellow,
        Status::Final => Color::Green,
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
        let tag = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
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
            Style::default().fg(Color::DarkGray),
        )),
        Line::raw(""),
    ];
    for (i, path) in state.files.iter().enumerate() {
        let label = path.strip_prefix(&app.project.root).unwrap_or(path).to_string_lossy();
        lines.push(Line::from(Span::styled(
            format!("{}{label}", marker(i == state.selected)),
            row_style(i == state.selected),
        )));
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
                Style::default().fg(Color::Gray)
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
    let mut lines = Vec::new();
    if let Some(series) = &manifest.series {
        lines.push(Line::from(Span::styled(
            series.clone(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(Span::styled(
        manifest.title.clone(),
        active_style(),
    )));
    if let Some(subtitle) = &manifest.subtitle {
        lines.push(Line::from(Span::styled(
            subtitle.clone(),
            Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC),
        )));
    }
    if let Some(author) = &manifest.author {
        lines.push(Line::from(Span::styled(
            format!("by {author}"),
            Style::default().fg(Color::DarkGray),
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
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
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

        // Group by band (#181) only when the project actually has sections, so an
        // all-body project keeps the flat, un-numbered "CHAPTERS" list it had.
        let grouped = (0..counts.len()).any(|i| app.project.chapter_band(i) != Band::Body);
        let mut shown_band: Option<Band> = None;
        let mut body_n = 0usize;
        for (i, chapter) in app.project.manifest.chapters.iter().enumerate() {
            let band = app.project.chapter_band(i);
            if shown_band != Some(band) {
                if shown_band.is_some() {
                    lines.push(Line::raw("")); // a gap between bands
                }
                lines.push(band_header(band));
                lines.push(Line::raw(""));
                shown_band = Some(band);
            }
            // Body chapters carry a number once the list is grouped; sections show
            // their role instead.
            let number = (band == Band::Body).then(|| {
                body_n += 1;
                body_n
            });
            let number = grouped.then_some(number).flatten();

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
        let body_total: usize = (0..counts.len())
            .filter(|&i| app.project.chapter_band(i) == Band::Body)
            .map(|i| counts[i])
            .sum();
        let total_line = if total != body_total {
            format!("{body_total} words · {} in sections", total - body_total)
        } else {
            format!("{total} words")
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(total_line, Style::default().fg(Color::DarkGray))));

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
        let buffer = app.edit_buffer();

        if let Mode::Prompt(prompt_state) = &app.mode {
            // Label plus any typed text (empty for y/N confirmations) — two
            // borrowed spans, no per-frame allocation.
            let style = Style::default().fg(Color::Black).bg(Color::Yellow);
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
            let style = Style::default().fg(Color::Black).bg(Color::Yellow);
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
            let ai_tag = if app.ai_busy() {
                " · ⟳ Claude"
            } else if app.ai.enabled {
                " · AI"
            } else {
                ""
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

        let pad = (area.width as usize)
            .saturating_sub(left.len() + right.len())
            .max(1);
        let line = Line::from(vec![
            Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" ".repeat(pad)),
            Span::styled(right, Style::default().fg(Color::DarkGray)),
        ]);

        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::Rgb(30, 30, 36))),
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

        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap(); // warm
        let t = Instant::now();
        for _ in 0..30 {
            terminal.draw(|f| draw(f, &app)).unwrap();
        }
        let dt = t.elapsed() / 30;

        println!("render {words}w + 150-chapter outline = {dt:?}");
        assert!(dt.as_millis() < 16, "render took {dt:?}");
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
    fn markers_are_hidden_when_cursor_is_outside_the_span() {
        // After typing, the cursor sits past the span, so markers conceal.
        let app = app_with("*italic* word");
        let screen = render(&app, 80, 8);
        assert!(screen.contains("italic word"), "screen:\n{screen}");
        assert!(!screen.contains('*'), "markers should be hidden:\n{screen}");
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

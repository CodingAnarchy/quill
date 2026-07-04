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

use std::cell::RefCell;

use crate::app::ListLayout;
use crate::mode::{
    Corkboard, DirBrowser, Findings, History, HistoryRow, ImportPreview, Palette, PaletteRow,
    ProjectBrowser, ProjectSearch, ProjectSearchStage,
};
use crate::project::Project;

/// The selection marker for a list row.
pub(super) const fn marker(selected: bool) -> &'static str {
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
pub(super) fn list_row(selected: bool, primary: &str, secondary: Option<String>) -> Line<'static> {
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
    draw_list_overlay_recording(frame, area, title, help, rows, selected, None);
}

/// [`draw_list_overlay`] that also records the drawn row geometry into `recorder`
/// (#812), so a mouse click can be mapped back to a list index. Passed
/// `Some(&app.list_layout)` for the click-selectable list modes (findings,
/// corkboard, history, …); `None` for full-screen chooser lists with no click
/// target. Rows are treated as single screen rows for the click map (the common
/// case); a wrapped multi-row entry maps the click to that entry's first row.
pub(super) fn draw_list_overlay_recording(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    help: &str,
    rows: Vec<Line<'static>>,
    selected: Option<usize>,
    recorder: Option<&RefCell<ListLayout>>,
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

    // Record where the rows landed so a click can invert screen → list index
    // (#812). The rows start one row past the box's top border, the help line, and
    // the blank separator; the click map treats each entry as a single row.
    if let Some(rec) = recorder {
        *rec.borrow_mut() = ListLayout {
            first_row: area.y.saturating_add(1 + header_h as u16),
            height: viewport as u16,
            first_index: offset,
            len: heights.len(),
        };
    }

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
                        &format!("{indent}{}", item.label),
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
            "Create a new project",
            Some("name it and start a fresh book".to_string()),
        ),
        list_row(
            selected == 1,
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
                &format!("{}{tag}", p.title),
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

/// Render the findings as a navigable triage list. `recorder` captures the row
/// geometry for click-to-select (#812).
pub(super) fn draw_findings(
    frame: &mut Frame,
    area: Rect,
    findings: &Findings,
    project: &Project,
    recorder: &RefCell<ListLayout>,
) {
    // The type-to-filter query (#822) narrows the list to findings whose summary
    // or chapters match; the row index is preserved so the selection marker and
    // the click map land on the right finding.
    let query = findings.filter.to_lowercase();
    let matches = |item: &crate::mode::Finding| {
        query.is_empty()
            || item.summary.to_lowercase().contains(&query)
            || item
                .chapters
                .iter()
                .any(|c| c.to_lowercase().contains(&query))
    };
    let visible: Vec<(usize, &crate::mode::Finding)> = findings
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches(item))
        .collect();
    let selected_pos = visible.iter().position(|(i, _)| *i == findings.selected);
    let title = format!(" Findings ({}) ", findings.items.len());
    let rows = visible
        .iter()
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
                *i == findings.selected,
                &format!("{}{loc}", item.summary),
                None,
            )
        })
        .collect();
    let help = if findings.typing {
        format!("Filter: {}_", findings.filter)
    } else if findings.filter.is_empty() {
        "Findings — ↑↓ move · enter jump · i mark xxx · r re-check · d dismiss · / filter · esc close".to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears",
            findings.filter,
            visible.len(),
            if visible.len() == 1 { "" } else { "es" }
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

/// Render the history as a navigable list (#587): named milestones (#833) lead the
/// list, each labeled "◆ <name>", above the commits, each a commit's date, short id,
/// and summary. The selected row is the one the chapter- and project-scoped diff /
/// restore actions address.
pub(super) fn draw_history(
    frame: &mut Frame,
    area: Rect,
    history: &History,
    chapter: &str,
    recorder: &RefCell<ListLayout>,
) {
    let title = format!(" History — {chapter} ");
    // The type-to-filter query (#822) narrows to rows whose text matches; the row
    // index is preserved so the marker and click map align. The combined milestone
    // + commit list (#833) is the single sequence the selection indexes into.
    let query = history.filter.to_lowercase();
    let all = history.rows();
    let label = |row: &HistoryRow| match row {
        HistoryRow::Milestone(t) => format!("◆ {}  {}", t.name, t.when),
        HistoryRow::Commit(c) => format!("{}  {}  {}", c.when, c.id, c.summary),
    };
    let visible: Vec<(usize, &HistoryRow)> = all
        .iter()
        .enumerate()
        .filter(|(_, r)| r.matches(&query))
        .collect();
    let selected_pos = visible.iter().position(|(i, _)| *i == history.selected);
    let rows = visible
        .iter()
        .map(|(i, r)| list_row(*i == history.selected, &label(r), None))
        .collect();
    let help = if history.typing {
        format!("Filter: {}_", history.filter)
    } else if history.filter.is_empty() {
        "History — ↑↓ move · d/r chapter · D/R project · / filter · esc close".to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears",
            history.filter,
            visible.len(),
            if visible.len() == 1 { "" } else { "es" }
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

/// Render the manuscript-import preview (#702): a summary header (chapter count,
/// the heading level the split used, book title/author, any dropped-element
/// warnings) then the list of chapters that will be created — shown so the author
/// can catch a mis-split before anything is written. Enter confirms, Esc cancels;
/// a Markdown source's split level can be toggled with `1`–`6`.
///
/// The header rows are non-selectable; the chapter list scrolls under them by
/// re-basing the list overlay's `selected` index past the header, so a long list
/// stays navigable while the summary stays pinned at the top.
pub(super) fn draw_import_preview(frame: &mut Frame, area: Rect, preview: &ImportPreview) {
    let dim = Style::default().fg(theme::current().dim);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let n = preview.chapters.len();
    let title = " Import preview ".to_string();

    // The summary header, above the chapter list. Non-selectable rows.
    let mut header = vec![Line::from(Span::styled(
        format!(
            "{n} chapter{} will be created:",
            if n == 1 { "" } else { "s" }
        ),
        bold,
    ))];
    if let Some(book) = &preview.title {
        let by = preview
            .author
            .as_deref()
            .map(|a| format!("  ·  by {a}"))
            .unwrap_or_default();
        header.push(Line::from(Span::styled(format!("  {book}{by}"), dim)));
    }
    if let Some(level) = preview.level {
        let toggle = if preview.levels.len() > 1 {
            "  (press 1–6 to re-split on another heading level)"
        } else {
            ""
        };
        header.push(Line::from(Span::styled(
            format!("  split on heading level {level}{toggle}"),
            dim,
        )));
    }
    if !preview.warning.is_empty() {
        // The warning suffix is `" (…)"`; trim to the parenthetical for the header.
        header.push(Line::from(Span::styled(
            format!("  warnings:{}", preview.warning),
            dim,
        )));
    }
    header.push(Line::raw(""));

    let header_len = header.len();
    let chapter_rows = preview
        .chapters
        .iter()
        .enumerate()
        .map(|(i, (t, _))| list_row(i == preview.scroll, &format!("{}. {t}", i + 1), None));
    header.extend(chapter_rows);

    let levels = if preview.levels.len() > 1 {
        " · 1–6 change split level"
    } else {
        ""
    };
    let help = format!("Import preview — ↑↓ scroll{levels} · enter import · esc cancel");
    // Pin the scroll to the selected chapter row, past the header block.
    draw_list_overlay(
        frame,
        area,
        &title,
        &help,
        header,
        Some(header_len + preview.scroll),
    );
}

/// Render the active chapter's inline annotations (#302) as a navigable list:
/// each row is the note over the (dimmed) snippet of text it's anchored to.
pub(super) fn draw_annotations(
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
    let (filter, typing) = match &app.mode {
        crate::mode::Mode::Annotations { filter, typing, .. } => (filter.clone(), *typing),
        _ => (String::new(), false),
    };
    let query = filter.to_lowercase();
    let visible: Vec<usize> = (0..app.annotations.len())
        .filter(|&i| app.annotation_matches(i, &query))
        .collect();
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
            list_row(i == selected, &a.note, Some(snippet))
        })
        .collect();
    let help = if typing {
        format!("Filter: {filter}_")
    } else if filter.is_empty() {
        "Annotations — ↑↓ move · enter jump · d delete · / filter · esc close".to_string()
    } else {
        format!(
            "Filter “{}” — {} match{} · esc clears",
            filter,
            visible.len(),
            if visible.len() == 1 { "" } else { "es" }
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

/// Render the corkboard (#589 / #636 / #701): scenes as index cards, each a status
/// badge + scene heading over its synopsis (the author's one-liner, or the first
/// line of prose as a fallback), indented by heading level so nested scenes read as
/// a hierarchy. Navigable; the author reorders scenes, edits each synopsis, and
/// cycles each status from here, or jumps the editor cursor to a scene with Enter.
///
/// The manuscript-wide board (#701) prefixes each chapter's first card with a
/// non-selectable chapter-title header, so the cards read grouped by chapter. Those
/// header rows shift the selected card's display position, so the highlight index is
/// re-derived over the interleaved rows rather than reusing the card index.
pub(super) fn draw_corkboard(
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
                super::active_style(),
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
        "Corkboard — ↑↓ move · enter jump · K/J reorder · e synopsis · s status · esc close",
        rows,
        Some(selected_row),
        rec,
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

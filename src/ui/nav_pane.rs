//! The outline / nav pane: the left strip listing the book's chapters, grouped
//! into front matter / body / back matter bands (#181), with the active chapter's
//! heading outline nested beneath it and a rolled-up word total at the foot.
//!
//! The list is a virtual one. The pane walks a cheap, unstyled row model
//! ([`Row`]) to find where the rows that matter sit, scrolls just enough to keep
//! them on screen, and styles only the rows that are actually visible — so a
//! frame beside a 1500-chapter series costs about what one beside a short novel
//! does, and a list longer than the pane still shows the Nav selection, the
//! active chapter, and the totals line.

use std::ops::ControlFlow;
use std::path::Path;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::{active_style, list_row, Pane};
use crate::app::App;
use crate::frontmatter::{Band, Role, Status};
use crate::mode::{Mode, RefFiles};
use crate::project::Manifest;
use crate::syntax::Heading;
use crate::theme;

/// The left strip: chapter list with the active chapter's heading outline
/// nested beneath it (current section highlighted; the Nav-mode selection
/// reversed). `active` is the active chapter's index, resolved once per draw.
pub(super) struct NavPane<'a> {
    pub app: &'a App,
    pub active: usize,
}

/// The index of the heading whose section the cursor is currently in: the last
/// heading at or before the cursor. `None` if the cursor sits above the first
/// heading (or there are no headings).
pub(super) fn current_section(headings: &[Heading], cursor_byte: usize) -> Option<usize> {
    headings.iter().rposition(|h| h.start <= cursor_byte)
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
    // Group by folder rather than list bare paths (#165/#590): the files arrive
    // sorted by path, so each folder's files are contiguous — emit a dim header
    // when the folder changes, then each file by its clean stem, indented under it.
    // Files sitting directly in the root show flush with no header. `selected` stays
    // a file index (headers are render-only), so the arrows step file-to-file and
    // skip the headers.
    let mut current: Option<String> = None;
    for (i, path) in state.files.iter().enumerate() {
        let group = ref_group(app, path, state.bible_only);
        if group != current {
            if let Some(g) = &group {
                lines.push(Line::from(Span::styled(
                    g.clone(),
                    Style::default().fg(theme::current().dim),
                )));
            }
            current.clone_from(&group);
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled");
        let label = if group.is_some() {
            format!("  {name}")
        } else {
            name.to_string()
        };
        lines.push(list_row(i == state.selected, &label, None));
    }
    lines
}

/// The folder a reference file is grouped under in the browser (#165/#590): its
/// parent directory relative to the project root, `None` for a file sitting
/// directly in the root (shown flush, no header). In the bible-only view the
/// leading `bible/` is stripped so headers read "characters", not
/// "bible/characters", and a file directly under `bible/` groups flush like a root
/// file.
fn ref_group(app: &App, path: &Path, bible_only: bool) -> Option<String> {
    let rel = path.strip_prefix(&app.project.root).unwrap_or(path);
    let group = rel.parent()?.to_string_lossy().replace('\\', "/");
    if group.is_empty() {
        return None;
    }
    if !bible_only {
        return Some(group);
    }
    // Bible-only view: strip the leading `bible/` so headers read "characters", not
    // "bible/characters"; a file sitting directly in `bible/` groups flush (None).
    if let Some(rest) = group.strip_prefix("bible/") {
        return Some(rest.to_string());
    }
    (group != "bible").then_some(group)
}

/// One row of the active chapter's heading outline, indented by level, the
/// current section accented. `selected` marks the focused-outline cursor (#657),
/// drawn reversed so the author sees which heading Enter will jump to.
fn heading_line(heading: &Heading, is_current: bool, is_selected: bool) -> Line<'static> {
    let indent = "  ".repeat(heading.level as usize);
    let mut style = if is_current {
        active_style()
    } else {
        Style::default().fg(theme::current().muted)
    };
    if is_selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(format!("{indent}{}", heading.title), style))
}

/// If the chapter opens with a level-1 heading whose text matches the chapter
/// title, the outline would just repeat the title — return its index to skip it.
pub(super) fn redundant_title_heading(headings: &[Heading], chapter_title: &str) -> Option<usize> {
    match headings.first() {
        Some(h) if h.level == 1 && h.title == chapter_title => Some(0),
        _ => None,
    }
}

/// The top of the nav pane: book / series metadata above the chapter list.
/// Series (if any) sits above the book title; subtitle and author follow.
fn project_header(manifest: &Manifest) -> Vec<Line<'static>> {
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

/// A short story's label row (#1239): "SCENES" and the story's word count,
/// standing where a novel's chapter list would.
fn story_line(words: usize, selected: bool) -> Line<'static> {
    let mut style = Style::default()
        .fg(theme::current().dim)
        .add_modifier(Modifier::BOLD);
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(format!("SCENES  {words}w"), style))
}

/// One row of the pane's scrolling list, before any styling — cheap to produce,
/// so the pane can walk every row to place the scroll and only style the ones on
/// screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    /// Line `n` of the book / series header block.
    Header(usize),
    /// The blank gap between two bands.
    Gap,
    /// A band's header label (#181).
    Band(Band),
    /// The blank row under a band header.
    BandBlank,
    /// Chapter `index`, with its 1-based body number when the list is grouped.
    Chapter { index: usize, number: Option<usize> },
    /// A short story's one body chapter (#1239), shown as a "SCENES" label over
    /// its outline rather than as a chapter row — a story has no chapter list.
    Story { index: usize },
    /// Heading `index` of the active chapter's nested outline.
    Heading(usize),
    /// The focused outline's filter line (#822).
    Filter,
}

/// Everything the row walk needs, resolved once per frame.
struct RowModel<'a> {
    app: &'a App,
    active: usize,
    header_len: usize,
    /// The active chapter's headings (empty when a reference pane is focused).
    headings: &'a [Heading],
    /// A leading H1 that just repeats the chapter title, left out of the outline.
    skip: Option<usize>,
    /// The focused outline's lowercased filter (#822); empty = unfiltered.
    outline_query: String,
    outline_typing: bool,
    /// The Nav quick-jump filter (#410), lowercased; `None` when not filtering.
    nav_query: Option<String>,
    /// Whether the list is grouped into bands (the project has sections, #181).
    grouped: bool,
    /// A short story's chapter index (#1239), drawn as [`Row::Story`].
    story: Option<usize>,
}

impl RowModel<'_> {
    /// Visit every row in display order, stopping early when `visit` breaks.
    fn walk(&self, mut visit: impl FnMut(Row) -> ControlFlow<()>) -> ControlFlow<()> {
        for n in 0..self.header_len {
            visit(Row::Header(n))?;
        }
        let project = &self.app.project;
        let mut shown_band: Option<Band> = None;
        let mut body_n = 0usize;
        for index in 0..project.manifest.chapters.len() {
            let band = project.chapter_band(index);
            // Body chapters carry a 1-based number once the list is grouped;
            // sections show their role instead. Counted before the filter skip so
            // a filtered list still shows each chapter's real number.
            let number = (self.grouped && band == Band::Body).then(|| {
                body_n += 1;
                body_n
            });
            if let Some(q) = &self.nav_query {
                if !self.app.nav_chapter_matches(index, q) {
                    continue;
                }
            }
            // The band header is emitted lazily, just before the first matching
            // chapter of a band, so a filter that hides a whole band hides its
            // header too.
            let is_story = Some(index) == self.story;
            if shown_band != Some(band) {
                if shown_band.is_some() {
                    visit(Row::Gap)?;
                }
                // A story's own row is its label, so it takes no band header.
                if !is_story {
                    visit(Row::Band(band))?;
                    visit(Row::BandBlank)?;
                }
                shown_band = Some(band);
            }
            visit(if is_story {
                Row::Story { index }
            } else {
                Row::Chapter { index, number }
            })?;
            if index == self.active {
                for (i, h) in self.headings.iter().enumerate() {
                    // When the focused outline is filtered (#822), show only
                    // headings whose title matches; the index numbering is
                    // untouched so the selection still lands on the right heading.
                    if Some(i) == self.skip
                        || !(self.outline_query.is_empty()
                            || h.title.to_lowercase().contains(&self.outline_query))
                    {
                        continue;
                    }
                    visit(Row::Heading(i))?;
                }
                // While filtering the focused outline (#822), show the query under
                // the narrowed headings.
                if self.outline_typing || !self.outline_query.is_empty() {
                    visit(Row::Filter)?;
                }
            }
        }
        ControlFlow::Continue(())
    }
}

/// The first row offset that keeps rows `lo..=hi` on a `height`-row screen,
/// moving `prev` as little as possible. When the span is taller than the screen,
/// `hi` wins (it's the more specific row: a heading under its chapter). Clamped so
/// a shrinking list never leaves blank rows below its end.
fn scroll_to_show(prev: usize, lo: usize, hi: usize, height: usize, total: usize) -> usize {
    let mut offset = prev.min(lo);
    if hi >= offset + height {
        offset = hi + 1 - height;
    }
    offset.min(total.saturating_sub(height))
}

impl Pane for NavPane<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let app = self.app;
        let active = self.active;
        let block = Block::default().borders(Borders::RIGHT);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // The reference-file browser (#165) replaces the chapter list with the
        // bible entries and notes the author can open in a side pane.
        if let Mode::RefFiles(state) = &app.mode {
            frame.render_widget(Paragraph::new(ref_file_lines(app, state)), inner);
            return;
        }

        let header = project_header(&app.project.manifest);

        // The outline nested under the active chapter is *that chapter's* structure.
        // When a non-chapter reference pane is focused (a bible entry / note, #165),
        // `app.buffer` holds that file, not the active chapter — so its headings must
        // not be nested under a chapter row. Treat the outline as empty then, leaving
        // the chapter list flat rather than mis-nesting the bible's sections.
        let headings = if app.focus_shows_active_chapter() {
            app.buffer.syntax().headings()
        } else {
            &[]
        };
        let current_heading = current_section(headings, app.buffer.cursor_byte());
        // When the outline is focused (#657), draw its selection cursor reversed.
        let outline_selected = match &app.mode {
            Mode::Outline { selected, .. } => Some(*selected),
            _ => None,
        };
        // The browser selection, if we're in Nav mode.
        let nav_selected = match &app.mode {
            Mode::Nav { selected, .. } => Some(*selected),
            _ => None,
        };
        // When a quick-jump filter is active (#410), the list shows only the
        // chapters that match; body numbering still counts every chapter so the
        // numbers stay stable as you filter. The query is lowercased once here,
        // not once per chapter.
        let filtering = matches!(&app.mode, Mode::Nav { filter, .. } if filter.is_active());

        // One cheap pass over the cached per-chapter metadata (no Strings): whether
        // the project has sections at all — the list groups by band (#181) only
        // then, so an all-body project keeps its flat, un-numbered "CHAPTERS" list —
        // and the words in those sections, kept out of the body total.
        let mut grouped = false;
        let mut section_words = 0usize;
        for i in 0..app.project.manifest.chapters.len() {
            if app.project.chapter_band(i) != Band::Body {
                grouped = true;
                section_words += app.chapter_word_count_with_active(i, active);
            }
        }

        let model = RowModel {
            app,
            active,
            header_len: header.len(),
            headings,
            skip: app
                .project
                .chapter_title(active)
                .and_then(|title| redundant_title_heading(headings, title)),
            outline_query: app.outline_query(),
            outline_typing: matches!(&app.mode, Mode::Outline { filter, .. } if filter.typing),
            nav_query: filtering.then(|| app.nav_query()),
            grouped,
            story: app.project.manifest.story_chapter(),
        };

        // First pass: count the rows and find where the ones that must stay on
        // screen sit — the Nav selection, or the active chapter with its current
        // (or, focused, its selected) heading, and where its nested outline ends.
        // Each chapter row is found with its *lead*: the first row that should
        // come into view with it — its band header when it opens a band (the book
        // header too, for the first band), so scrolling up to the top of a band
        // reveals the label above it rather than stopping one row short.
        let mut total = 0usize;
        let (mut chapter_row, mut heading_row, mut selected_row) = (None, None, None);
        let (mut band_lead, mut prev, mut block_end) = (None, None, 0);
        let _ = model.walk(|row| {
            match row {
                Row::Band(_) => band_lead = Some(band_lead.map_or(0, |_| total)),
                Row::Chapter { index, .. } => {
                    let lead = match (prev, band_lead) {
                        (Some(Row::BandBlank), Some(lead)) => lead,
                        _ => total,
                    };
                    if index == active {
                        chapter_row = Some((lead, total));
                        block_end = total;
                    }
                    if Some(index) == nav_selected {
                        selected_row = Some((lead, total));
                    }
                }
                Row::Story { index } => {
                    if index == active {
                        chapter_row = Some((total, total));
                        block_end = total;
                    }
                    if Some(index) == nav_selected {
                        selected_row = Some((total, total));
                    }
                }
                // Only the active chapter nests rows under it.
                Row::Heading(_) | Row::Filter => block_end = total,
                _ => {}
            }
            prev = Some(row);
            if matches!(row, Row::Heading(i) if Some(i) == outline_selected.or(current_heading)) {
                heading_row = Some(total);
            }
            total += 1;
            ControlFlow::Continue(())
        });

        // The rolled-up totals line pins to the foot of the pane (below a blank
        // separator) whenever the list overflows, so it's always on screen: the
        // scroll is clamped so an overflowing list always fills `list_height`,
        // which puts the totals on the pane's last row.
        let height = inner.height as usize;
        let footer = if total + 2 <= height {
            2 // everything fits: the totals follow the list as before
        } else {
            height.saturating_sub(1).min(2)
        };
        let list_height = height - footer;
        let prev = app.nav_scroll.get();
        let offset = match (nav_selected, selected_row, chapter_row) {
            (Some(_), Some((lead, row)), _) => scroll_to_show(prev, lead, row, list_height, total),
            (None, _, Some((lead, row))) => {
                // Bring the active chapter's whole block (its nested outline) into
                // view where it fits, then make sure the chapter row and its current
                // heading are on screen — the second step wins when the outline
                // is taller than the pane.
                let block = scroll_to_show(prev, lead, block_end, list_height, total);
                let hi = heading_row.unwrap_or(row);
                scroll_to_show(block, row, hi, list_height, total)
            }
            _ => prev.min(total.saturating_sub(list_height)),
        };
        app.nav_scroll.set(offset);

        // Second pass: style only the rows on screen.
        let mut lines: Vec<Line> = Vec::with_capacity(height);
        let mut row_n = 0usize;
        let _ = model.walk(|row| {
            if row_n >= offset + list_height {
                return ControlFlow::Break(());
            }
            if row_n >= offset {
                lines.push(match row {
                    Row::Header(n) => header[n].clone(),
                    Row::Gap | Row::BandBlank => Line::raw(""),
                    Row::Band(band) => band_header(band),
                    Row::Chapter { index, number } => {
                        let project = &app.project;
                        chapter_line(
                            project.chapter_title(index).unwrap_or_default(),
                            number,
                            app.chapter_word_count_with_active(index, active),
                            project.chapter_goal(index),
                            project.chapter_status(index),
                            project.chapter_role(index),
                            index == active,
                            nav_selected == Some(index),
                        )
                    }
                    Row::Story { index } => story_line(
                        app.chapter_word_count_with_active(index, active),
                        nav_selected == Some(index),
                    ),
                    Row::Heading(i) => heading_line(
                        &headings[i],
                        Some(i) == current_heading,
                        Some(i) == outline_selected,
                    ),
                    Row::Filter => {
                        let caret = if model.outline_typing { "_" } else { "" };
                        Line::from(Span::styled(
                            format!("  filter: {}{caret}", model.outline_query),
                            Style::default().fg(theme::current().dim),
                        ))
                    }
                });
            }
            row_n += 1;
            ControlFlow::Continue(())
        });

        // Rolled-up totals: the body word count, kept clear of any front/back
        // matter so the manuscript total isn't muddied (#181). The whole-project
        // total is the maintained running sum with the live buffer swapped in for
        // the active chapter — not a per-frame re-sum of every chapter.
        if footer > 0 {
            let total_words = app.project_word_count_with_active(active);
            let body_words = total_words.saturating_sub(section_words);
            let total_line = if section_words > 0 {
                format!("{body_words} words · {section_words} in sections")
            } else {
                format!("{total_words} words")
            };
            if footer == 2 {
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(Span::styled(
                total_line,
                Style::default().fg(theme::current().dim),
            )));
        }

        frame.render_widget(Paragraph::new(lines), inner);
    }
}

//! Display-only header blocks injected above the prose body (#412/#518): the
//! chapter dividers, part/section labels, and epigraphs. Built (and cached, #869)
//! from the resolved segment headers, then keyed by the buffer line each precedes.
//! Split out of the editor renderer as its own concern.

use std::rc::Rc;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::frontmatter::{Epigraph, FrontMatter};
use crate::manuscript::{Manuscript, SegmentHeader};
use crate::mode::Mode;
use crate::theme::{self, Theme};

/// Everything the display-only header blocks (#412/#518) are built from — the
/// cache key (#869). Rebuilding the blocks wraps and formats every epigraph line
/// into owned [`Line`]s each frame; comparing this cheap signature against the
/// last one lets an untouched editor reuse the built blocks instead. It captures
/// exactly the inputs [`build_header_inserts`] reads, so any change (an edited
/// epigraph, a paged manuscript window, a resize, a theme switch, entering/leaving
/// epigraph editing) misses the cache and rebuilds.
#[derive(Clone, PartialEq)]
struct HeaderSignature {
    width: u16,
    /// Epigraph editing hides all headers, so it's part of the key.
    edit_meta: bool,
    theme: Theme,
    book_epigraph: Option<Epigraph>,
    /// One entry per segment header: `(start, index, title, structure, epigraph)`.
    headers: Vec<HeaderKey>,
}

type HeaderKey = (
    usize,
    usize,
    Option<String>,
    Option<String>,
    Option<Epigraph>,
);

/// Cached header blocks and the signature they were built for (#869), stored on
/// the `App` and consulted by [`header_inserts`] each frame. Shared via `Rc` so a
/// cache hit is a refcount bump, never a re-wrap of the epigraph lines.
#[derive(Default)]
pub struct HeaderCache {
    signature: Option<HeaderSignature>,
    inserts: Rc<Vec<(usize, Vec<Line<'static>>)>>,
}

impl HeaderCache {
    /// The identity of the currently cached blocks (#869), for tests to assert a
    /// frame reused them (same pointer) or rebuilt them (different) — without
    /// exposing the blocks themselves.
    #[cfg(test)]
    pub(crate) fn identity(&self) -> *const Vec<(usize, Vec<Line<'static>>)> {
        Rc::as_ptr(&self.inserts)
    }
}

/// The display-only header blocks to inject above the body, keyed by the buffer
/// line each precedes: per chapter, a divider (continuous view, not the first
/// chapter), the book epigraph (above chapter 0), then the chapter's epigraph.
///
/// Cached on the `App` (#869): the blocks change only when an input does (an edited
/// epigraph, a paged window, a resize, a theme switch, entering/leaving epigraph
/// editing), so a signature check lets an untouched frame reuse them rather than
/// re-wrapping every epigraph line.
pub(super) fn header_inserts(app: &App, width: u16) -> Rc<Vec<(usize, Vec<Line<'static>>)>> {
    let edit_meta = matches!(app.mode, Mode::EditMeta(_));
    // When editing an epigraph in the editor, the buffer *is* the epigraph —
    // don't inject chapter/book headers above it.
    let book_epigraph = if edit_meta {
        None
    } else {
        app.project.manifest.epigraph()
    };
    let headers = if edit_meta {
        Vec::new()
    } else {
        app.manuscript.as_ref().map_or_else(
            // Single-chapter view: one header for the active chapter at line 0.
            || {
                vec![SegmentHeader {
                    start: 0,
                    index: app.project.active_index(),
                    title: None,
                    structure: None,
                    epigraph: app.buffer.front_matter().and_then(FrontMatter::epigraph),
                }]
            },
            Manuscript::segment_headers,
        )
    };

    let signature = HeaderSignature {
        width,
        edit_meta,
        theme: theme::current(),
        book_epigraph: book_epigraph.clone(),
        headers: headers
            .iter()
            .map(|h| {
                (
                    h.start,
                    h.index,
                    h.title.clone(),
                    h.structure.clone(),
                    h.epigraph.clone(),
                )
            })
            .collect(),
    };

    let mut cache = app.header_cache.borrow_mut();
    if cache.signature.as_ref() == Some(&signature) {
        return Rc::clone(&cache.inserts);
    }
    let inserts = Rc::new(build_header_inserts(
        &headers,
        book_epigraph.as_ref(),
        width,
    ));
    cache.signature = Some(signature);
    cache.inserts = Rc::clone(&inserts);
    inserts
}

/// Wrap and format the header blocks from the resolved segment headers and book
/// epigraph — the allocating half of [`header_inserts`], split out so the cache
/// runs it only when the signature changes (#869).
fn build_header_inserts(
    headers: &[SegmentHeader],
    book_epigraph: Option<&Epigraph>,
    width: u16,
) -> Vec<(usize, Vec<Line<'static>>)> {
    let mut inserts = Vec::new();
    for header in headers {
        let mut block = Vec::new();
        // The part/section name sits above the chapter divider, where the
        // structure changes (#412) — render-only, never in the buffer.
        if let Some(structure) = &header.structure {
            block.push(structure_line(structure, width));
        }
        if let Some(title) = &header.title {
            block.push(divider_line(title, width));
        }
        if header.index == 0 {
            if let Some(epigraph) = book_epigraph {
                block.extend(epigraph_lines(epigraph, width));
            }
        }
        if let Some(epigraph) = &header.epigraph {
            block.extend(epigraph_lines(epigraph, width));
        }
        if !block.is_empty() {
            inserts.push((header.start, block));
        }
    }
    inserts
}

/// Display-only lines for an epigraph: the quotation (inset, italic) and its
/// attribution (`— Name`, dimmed, right-aligned to match the export), set off by
/// blank lines. Long lines soft-wrap to the column so nothing is cut off (#518).
/// Never enters the buffer or any file.
fn epigraph_lines(epigraph: &Epigraph, width: u16) -> Vec<Line<'static>> {
    const INDENT: &str = "    ";
    let theme = theme::current();
    // Wrap within the column, leaving room for the inset on every row.
    let wrap_width = (width as usize).saturating_sub(INDENT.len()).max(1);
    let quote = Style::default()
        .fg(theme.muted)
        .add_modifier(Modifier::ITALIC);
    let mut lines = vec![Line::raw("")];
    for src in epigraph.text.lines() {
        for row in wrap_rows(src, wrap_width) {
            lines.push(Line::from(Span::styled(format!("{INDENT}{row}"), quote)));
        }
    }
    if let Some(attribution) = &epigraph.attribution {
        // Right-aligned to the column edge, matching the export (`text-align: right`).
        let dim = Style::default().fg(theme.dim);
        for row in wrap_rows(&format!("— {attribution}"), width as usize) {
            let pad = (width as usize).saturating_sub(row.chars().count());
            lines.push(Line::from(Span::styled(
                format!("{}{row}", " ".repeat(pad)),
                dim,
            )));
        }
    }
    lines.push(Line::raw(""));
    lines
}

/// Soft-wrap one logical line into display rows of at most `width` cells, breaking
/// on spaces where possible (the same wrapping the prose body uses).
fn wrap_rows(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let starts = crate::wrap::row_starts(text, width);
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(text.len());
            text[start..end].trim_end().to_string()
        })
        .collect()
}

/// A centered, faint part/section header, e.g. `PART TWO`, drawn above the
/// chapter divider where the manuscript's structure changes (#412). Set apart
/// from the dim-bold, title-case chapter divider by uppercasing and an accent
/// color; like the divider it's render-only — never in the buffer or any file.
fn structure_line(name: &str, width: u16) -> Line<'static> {
    let style = Style::default()
        .fg(theme::current().heading)
        .add_modifier(Modifier::ITALIC);
    centered_line(&name.to_uppercase(), width, style)
}

/// A centered, dimmed chapter-break label, e.g. `—— Chapter Two ——`. Rendered
/// only; it never enters the buffer or any file.
fn divider_line(title: &str, width: u16) -> Line<'static> {
    let style = Style::default()
        .fg(theme::current().dim)
        .add_modifier(Modifier::BOLD);
    centered_line(&format!("—— {title} ——"), width, style)
}

/// A `label` padded to sit centered in `width` cells and styled — the shared
/// shape of the chapter divider and the part/section header (#412), both of
/// which are render-only.
fn centered_line(label: &str, width: u16, style: Style) -> Line<'static> {
    let pad = (width as usize).saturating_sub(label.chars().count()) / 2;
    Line::from(Span::styled(format!("{}{label}", " ".repeat(pad)), style))
}

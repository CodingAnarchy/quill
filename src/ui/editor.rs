//! The prose editor pane (#150): soft-wrapped, syntax- and search-highlighted
//! rendering of the active buffer, with cursor following and concealed emphasis
//! markers. The most involved renderer, split out of `ui`. `use super::*` pulls
//! in the shared types and the `Pane` trait.

use super::*;
use crate::app::mouse::{EditorLayout, VisualRow};
use crate::buffer::Buffer;
use crate::theme::{self, Theme};

/// An absolute byte range `[start, end)` in the buffer — a search match (#144).
type ByteRange = (usize, usize);

/// The display-only first-line paragraph indent (#722): a fixed four spaces on the
/// first visual row of each prose paragraph when the setting is on, matching the
/// exports' first-line indent. Never enters the buffer or any file.
const PARAGRAPH_INDENT: &str = "    ";

/// The "what to highlight in this span" overlays threaded through the body
/// renderer (#431): one cohesive bundle, built once per pane and passed by
/// reference through `render_body → styled_line → styled_row`, so a new overlay
/// kind is one field rather than a parameter added to three signatures.
struct SpanHighlights<'a> {
    /// Emphasis-marker ranges to hide, except on the cursor's span (#296).
    conceal: &'a [(usize, usize)],
    /// Search/replace matches or the Visual selection, dim-highlighted (#144/#205).
    matches: &'a [ByteRange],
    /// The current search match / live selection, highlighted brighter (#144/#205).
    current: Option<ByteRange>,
    /// Misspelled-word ranges, underlined (#357).
    spell: &'a [ByteRange],
    /// Annotated spans, tinted background (#302).
    annotations: &'a [ByteRange],
    /// The focus-mode paragraph; everything outside it dims (#301).
    focus: Option<ByteRange>,
}

impl Pane for Editor<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) {
        render_focused_pane(frame, area, self.0);
    }
}

/// Render the focused editor pane (`App::buffer`) into `area`: full rendering with
/// headers, search highlighting, and the terminal cursor (#163: shared by the
/// single-pane view and the focused half of a split).
pub(super) fn render_focused_pane(frame: &mut Frame, area: Rect, app: &App) {
    // The prose column, with a blank top row (the EditMeta banner draws there).
    let column = centered_column(
        Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(1),
        },
        app.focus_width,
    );

    let buffer = app.edit_buffer();
    // Feed navigation the same wrap column the screen uses (#143), so visual
    // up/down move by the rows the author actually sees.
    app.wrap_width.set(column.width);

    // Emphasis markers are hidden, except on the span the cursor sits in.
    let conceal = buffer.syntax().conceal_ranges(Some(buffer.cursor_byte()));
    // Find/replace highlighting (#144): every match gets a dim background, the
    // current one a bright background. Empty when not searching.
    let (search_matches, current_match): (&[ByteRange], Option<ByteRange>) = match &app.mode {
        Mode::Search(s) => (&s.matches, s.current_match()),
        // Vim Visual mode (#205): the live selection draws as the highlight.
        Mode::Visual { anchor, linewise } => (&[], Some(app.selection_bytes(*anchor, *linewise))),
        _ => (&[], None),
    };
    let inserts = header_inserts(app, column.width);
    // Misspelled-word flags (#357), computed on idle; underlined under the prose.
    let spell = app.spell_flags.as_slice();
    // Inline author annotations (#302/#370): the anchored spans get a tinted
    // background, in both the single-chapter and continuous views (the live ranges
    // are buffer-absolute against whichever buffer is active).
    let annotations = app.annotation_ranges();
    // No cursor while a docked surface pane has scroll focus (#164). Vim Normal /
    // Visual mode still shows the cursor — that's the selection's active end.
    let editing = matches!(
        app.mode,
        Mode::Insert | Mode::Normal | Mode::Visual { .. } | Mode::EditMeta(_)
    ) && !app.surface_pane_focused()
        && !app.assistant_pane_focused();
    // Focus mode (#301): the un-dimmed span is the paragraph under the cursor.
    let focus = app.focus_mode.then(|| focus_paragraph(buffer));
    let highlights = SpanHighlights {
        conceal: &conceal,
        matches: search_matches,
        current: current_match,
        spell,
        annotations: &annotations,
        focus,
    };
    // Record the focused editor's layout so a click can invert screen → byte
    // (#757), and honor the read-ahead wheel-scroll offset (#764). Both apply only
    // to the focused editing pane, never a reference pane.
    let mut editor_layout = app.editor_layout.borrow_mut();
    let clamped = render_body(
        frame,
        column,
        buffer,
        &inserts,
        &highlights,
        app.typewriter,
        editing,
        app.paragraph_indent,
        app.scroll_offset.get(),
        Some(&mut editor_layout),
    );
    drop(editor_layout);
    // Store the offset the renderer actually applied (clamped to the content), so a
    // wheel-up right after over-scrolling responds immediately (#764).
    app.scroll_offset.set(clamped);

    // Editing a metadata field: a banner on the otherwise-blank top row.
    if let Mode::EditMeta(meta) = &app.mode {
        let banner = format!(" editing {} — Ctrl-S save · Esc cancel", meta.label);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                banner,
                Style::default()
                    .fg(app.theme.on_accent)
                    .bg(app.theme.heading),
            ))),
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: 1,
            },
        );
    }
}

/// Render a non-focused reference pane (#163): the other pane's prose under a
/// dimmed title bar naming its chapter — no cursor, no search highlight.
pub(super) fn render_reference_pane(frame: &mut Frame, area: Rect, app: &App, buffer: &Buffer) {
    let title = app.pane_title_for(buffer);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {title}"),
            Style::default()
                .fg(app.theme.dim)
                .add_modifier(Modifier::BOLD),
        ))),
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
    );
    let body = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };
    let column = centered_column(body, app.focus_width);
    // No cursor in a reference pane, so conceal every emphasis marker uniformly.
    let conceal = buffer.syntax().conceal_ranges(None);
    // A reference pane isn't where you write — no annotations, typewriter, or focus.
    let highlights = SpanHighlights {
        conceal: &conceal,
        matches: &[],
        current: None,
        spell: &[],
        annotations: &[],
        focus: None,
    };
    render_body(
        frame,
        column,
        buffer,
        &[],
        &highlights,
        false,
        false,
        app.paragraph_indent,
        0,
        None,
    );
}

/// Render `buffer`'s soft-wrapped, styled prose into `column`, following the
/// cursor by visual row. Places the terminal cursor when `place_cursor`. The
/// shared core of both pane renderers (#163).
#[allow(clippy::too_many_arguments)]
fn render_body(
    frame: &mut Frame,
    column: Rect,
    buffer: &Buffer,
    inserts: &[(usize, Vec<Line<'static>>)],
    hl: &SpanHighlights,
    typewriter: bool,
    place_cursor: bool,
    paragraph_indent: bool,
    // Read-ahead viewport offset in visual rows (#764): shift the cursor-driven top
    // down by this many rows, clamped so no blank space shows past the end. Zero
    // (and `None` for a reference pane) leaves the cursor-driven behavior unchanged.
    scroll_offset: usize,
    // When `Some`, record the focused editor's per-visual-row layout here (#757) so
    // a click can be inverted to a buffer byte. `None` for the reference pane.
    mut layout: Option<&mut EditorLayout>,
) -> usize {
    let height = column.height as usize;
    let wrap = column.width as usize;
    let header_rows = |line: usize| {
        inserts
            .iter()
            .find(|(at, _)| *at == line)
            .map_or(0, |(_, b)| b.len())
    };
    // Row count honoring fenced-code treatment (#723): a hidden fence line is zero
    // rows, verbatim preview code is one, everything else soft-wraps — kept in
    // lockstep with the fill loop's `styled_line` so scroll math matches the render.
    let syntax = buffer.syntax();
    let line_rows = |line: usize| {
        let start = buffer.line_start_byte(line);
        let text = buffer.line(line);
        let kind = row_kind(syntax, start, &text, hl.conceal);
        row_count(kind, &text, wrap)
    };
    let body_rows = line_rows;

    let total = buffer.line_count();
    let cursor_line = buffer.cursor.line;
    let cursor_byte = buffer.cursor_byte();

    // Where the cursor sits among its own line's visual rows.
    let cline = buffer.line(cursor_line);
    let cline_start = buffer.line_start_byte(cursor_line);
    let cstarts = crate::wrap::row_starts(&cline, wrap);
    let (cursor_subrow, _) = crate::wrap::visual_pos(&cline, &cstarts, cursor_byte - cline_start);

    // Follow the cursor by *visual* row, keeping it near the page centre. Normally
    // the centre is clamped so we never scroll past the top or leave blank space at
    // the bottom; with typewriter scrolling on (#301) we keep the cursor pinned at
    // the centre even past the end, so the line you're writing never drifts down.
    let target_above = height / 2;
    let mut above_avail = header_rows(cursor_line) + cursor_subrow;
    let mut l = cursor_line;
    while l > 0 && above_avail < height {
        l -= 1;
        above_avail += header_rows(l) + body_rows(l);
    }
    let keep_above = if typewriter {
        target_above.min(above_avail)
    } else {
        let mut below_avail = body_rows(cursor_line) - 1 - cursor_subrow;
        let mut l = cursor_line;
        while l + 1 < total && below_avail < height {
            l += 1;
            below_avail += header_rows(l) + body_rows(l);
        }
        target_above
            .max((height.saturating_sub(1)).saturating_sub(below_avail))
            .min(above_avail)
    };

    let mut acc = header_rows(cursor_line) + cursor_subrow;
    let mut top = cursor_line;
    while acc < keep_above && top > 0 {
        top -= 1;
        acc += header_rows(top) + body_rows(top);
    }
    let mut to_skip = acc.saturating_sub(keep_above);

    // Read-ahead wheel scroll (#764): advance the cursor-driven top by the offset,
    // clamped so the bottom never shows blank space past the end. Applied by walking
    // the same rows the fill loop does — starting from the cursor-driven top, skip
    // `scroll_offset` more visual rows, but stop early if fewer than a screenful
    // remain below (which would leave a blank tail).
    // The offset actually applied this frame, clamped to the content — handed back
    // to the caller so the stored offset can't run away past the last valid row.
    let mut applied_offset = 0usize;
    if scroll_offset > 0 {
        // Rows from the cursor-driven top to the end of the document.
        let mut rows_below = header_rows(top).saturating_sub(to_skip);
        for l in top..total {
            if l > top {
                rows_below += header_rows(l);
            }
            rows_below += body_rows(l);
        }
        // The most we can advance and still fill the screen from the bottom up.
        let room = rows_below.saturating_sub(height);
        applied_offset = scroll_offset.min(room);
        let mut advance = applied_offset;
        // Consume `advance` more rows off the current top, moving `top`/`to_skip`.
        while advance > 0 {
            let line_rows = header_rows(top) + body_rows(top);
            let remaining = line_rows - to_skip;
            if advance < remaining {
                to_skip += advance;
                advance = 0;
            } else {
                advance -= remaining;
                to_skip = 0;
                top += 1;
            }
        }
    }

    // Record the on-screen layout for click inversion (#757): the column, and one
    // entry per drawn visual row.
    if let Some(map) = layout.as_deref_mut() {
        map.column = column;
        map.conceal = hl.conceal.to_vec();
        map.last_line = total.saturating_sub(1);
        map.rows.clear();
    }

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let mut cursor_row = None;
    let mut ll = top;
    let mut skip = to_skip;
    'fill: while lines.len() < height && ll < total {
        if let Some((_, block)) = inserts.iter().find(|(at, _)| *at == ll) {
            for line in block {
                if skip > 0 {
                    skip -= 1;
                } else if lines.len() < height {
                    // A header block precedes line `ll`; a click here maps to that
                    // line's start (it carries no editable text of its own).
                    record_row(&mut layout, ll, buffer, 0, 0, false, false, false);
                    lines.push(line.clone());
                } else {
                    break 'fill;
                }
            }
        }
        let indent = paragraph_indent && starts_paragraph(buffer, ll);
        let line_start = buffer.line_start_byte(ll);
        let text = buffer.line(ll);
        let kind = row_kind(syntax, line_start, &text, hl.conceal);
        let verbatim = matches!(kind, RowKind::Code { verbatim: true });
        let starts = row_starts_for(kind, &text, wrap);
        let body = styled_line(syntax, line_start, &text, hl, wrap, indent, kind);
        for (sub, line) in body.into_iter().enumerate() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if lines.len() >= height {
                break 'fill;
            }
            if ll == cursor_line && sub == cursor_subrow {
                cursor_row = Some(lines.len() as u16);
            }
            // The byte span of this visual row within its logical line.
            let rs = starts.get(sub).copied().unwrap_or(0);
            let re = starts.get(sub + 1).copied().unwrap_or(text.len());
            record_row(
                &mut layout,
                ll,
                buffer,
                rs,
                re,
                true,
                verbatim,
                indent && sub == 0,
            );
            lines.push(line);
        }
        ll += 1;
    }

    frame.render_widget(Paragraph::new(lines).block(Block::default()), column);

    // Place the terminal cursor (focused, editing panes only). Its span is
    // revealed, so it never lands on a concealed character.
    if place_cursor {
        if let Some(row) = cursor_row {
            let screen_y = column.y + row;
            let row_start = cstarts[cursor_subrow];
            let mut col_width = visible_width_before(
                &cline[row_start..],
                cline_start + row_start,
                cursor_byte,
                hl.conceal,
            );
            // The first-line indent shifts the cursor right too, but only on the
            // paragraph's first visual row (#722) — continuation rows aren't indented.
            if paragraph_indent && cursor_subrow == 0 && starts_paragraph(buffer, cursor_line) {
                col_width += PARAGRAPH_INDENT.len() as u16;
            }
            let screen_x = column.x + col_width.min(column.width.saturating_sub(1));
            frame.set_cursor_position((screen_x, screen_y));
        }
    }

    applied_offset
}

/// The display-only header blocks to inject above the body, keyed by the buffer
/// line each precedes: per chapter, a divider (continuous view, not the first
/// chapter), the book epigraph (above chapter 0), then the chapter's epigraph.
fn header_inserts(app: &App, width: u16) -> Vec<(usize, Vec<Line<'static>>)> {
    // When editing an epigraph in the editor, the buffer *is* the epigraph —
    // don't inject chapter/book headers above it.
    if matches!(app.mode, Mode::EditMeta(_)) {
        return Vec::new();
    }
    let book_epigraph = app.project.manifest.epigraph();
    let headers = match &app.manuscript {
        Some(m) => m.segment_headers(),
        // Single-chapter view: one header for the active chapter at line 0.
        None => vec![SegmentHeader {
            start: 0,
            index: app.project.active_index(),
            title: None,
            structure: None,
            epigraph: app.buffer.front_matter().and_then(FrontMatter::epigraph),
        }],
    };

    let mut inserts = Vec::new();
    for header in &headers {
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
            if let Some(epigraph) = &book_epigraph {
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
    let theme = theme::current();
    const INDENT: &str = "    ";
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
    centered_line(name.to_uppercase(), width, style)
}

/// A centered, dimmed chapter-break label, e.g. `—— Chapter Two ——`. Rendered
/// only; it never enters the buffer or any file.
fn divider_line(title: &str, width: u16) -> Line<'static> {
    let style = Style::default()
        .fg(theme::current().dim)
        .add_modifier(Modifier::BOLD);
    centered_line(format!("—— {title} ——"), width, style)
}

/// A `label` padded to sit centered in `width` cells and styled — the shared
/// shape of the chapter divider and the part/section header (#412), both of
/// which are render-only.
fn centered_line(label: String, width: u16, style: Style) -> Line<'static> {
    let pad = (width as usize).saturating_sub(label.chars().count()) / 2;
    Line::from(Span::styled(format!("{}{label}", " ".repeat(pad)), style))
}

/// How a logical line renders with respect to fenced code blocks (#723). Prose is
/// the default; the fence lines and code content of a block get special treatment
/// that mirrors emphasis's reveal-on-cursor concealment.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// Ordinary prose: soft-wrapped, styled per the highlight spans.
    Prose,
    /// A fence-delimiter row (the ``` ``` ``` line) that is currently concealed
    /// because the cursor is outside its block — it renders as zero rows so the
    /// block reads as a set-apart preview with the fences hidden.
    HiddenFence,
    /// A code-content line inside a fenced block. `verbatim` when the block is in
    /// preview (cursor outside): whitespace is preserved and the line is *not*
    /// soft-wrapped/reflowed. When the cursor is inside, the fences are revealed
    /// and the content edits like prose, so it soft-wraps (`verbatim == false`).
    Code { verbatim: bool },
}

/// Classify a logical line for fenced-code rendering (#723). A line is a
/// [`RowKind::HiddenFence`] when its whole content sits inside a currently-active
/// conceal range (a hidden fence row); a [`RowKind::Code`] when a `Code` highlight
/// covers its start byte. `verbatim` is set when that block's fences are concealed
/// (preview state) — the same all-or-nothing signal the conceal list already
/// carries, so no separate cursor test is needed here.
fn row_kind(syntax: &Syntax, line_start: usize, text: &str, conceal: &[(usize, usize)]) -> RowKind {
    let line_end = line_start + text.len();
    // A fence row is fully hidden: its content range lies within one conceal range.
    // (Fence lines are never blank, so an empty line is only ordinary blank prose.)
    if !text.is_empty()
        && conceal
            .iter()
            .any(|&(s, e)| s <= line_start && line_end <= e)
    {
        return RowKind::HiddenFence;
    }
    // Code content: a `Code` span covers the line's start. Its block is in preview
    // (verbatim) when any fence line of that span is still concealed.
    let probe = line_start.min(line_end.saturating_sub(1));
    if let Some(code) = syntax
        .highlights_in(line_start, line_end.max(line_start + 1))
        .find(|h| h.kind == HighlightKind::Code && h.start <= probe && probe < h.end)
    {
        let verbatim = conceal.iter().any(|&(s, e)| s < code.end && e > code.start);
        return RowKind::Code { verbatim };
    }
    RowKind::Prose
}

/// The display rows a logical line occupies at wrap `width`, honoring fenced-code
/// treatment (#723): a hidden fence row is zero rows; verbatim code is one row (no
/// reflow); everything else soft-wraps. Kept in lockstep with [`styled_line`] so
/// the scroll math and the rendered output always agree on row counts.
fn row_count(kind: RowKind, text: &str, width: usize) -> usize {
    match kind {
        RowKind::HiddenFence => 0,
        RowKind::Code { verbatim: true } => 1,
        _ => crate::wrap::row_starts(text, width).len(),
    }
}

/// The byte offsets (within the line) of each visual row's start, honoring
/// fenced-code treatment (#723) so the click layout map (#757) agrees with the
/// render: verbatim preview code is a single row starting at 0; a hidden fence has
/// none; everything else soft-wraps. A trailing sentinel isn't added — the caller
/// reads the end of the last row as the line length.
fn row_starts_for(kind: RowKind, text: &str, width: usize) -> Vec<usize> {
    match kind {
        RowKind::HiddenFence => Vec::new(),
        RowKind::Code { verbatim: true } => vec![0],
        _ => crate::wrap::row_starts(text, width),
    }
}

/// Append one visual row to the click layout map (#757), if one is being recorded.
/// `rs`/`re` are the row's byte span within its logical line.
#[allow(clippy::too_many_arguments)]
fn record_row(
    layout: &mut Option<&mut EditorLayout>,
    line: usize,
    buffer: &Buffer,
    rs: usize,
    re: usize,
    editable: bool,
    verbatim: bool,
    indent: bool,
) {
    if let Some(map) = layout.as_deref_mut() {
        map.rows.push(VisualRow {
            line,
            line_start: buffer.line_start_byte(line),
            row_start: rs,
            row_end: re,
            editable,
            verbatim,
            indent,
        });
    }
}

/// Soft-wrap a logical line into its visual rows (#143) and style each. `text` is
/// one line (no newline); `line_start` its absolute byte offset (highlights,
/// conceal, and match ranges use document offsets). Returns one [`Line`] per
/// wrapped row — or no rows for a hidden fence line, and a single verbatim row for
/// preview code (#723).
fn styled_line(
    syntax: &Syntax,
    line_start: usize,
    text: &str,
    hl: &SpanHighlights,
    width: usize,
    indent: bool,
    kind: RowKind,
) -> Vec<Line<'static>> {
    match kind {
        // Concealed fence line: contributes no visual row, so the block's fences
        // vanish and it reads as a set-apart preview.
        RowKind::HiddenFence => return Vec::new(),
        // Preview code: one verbatim row, whitespace preserved (no soft-wrap), set
        // apart with a full-width tint. An empty content line still gets a tinted
        // row so the block reads as one continuous box.
        RowKind::Code { verbatim: true } => {
            let row = styled_row(syntax, line_start, text, 0, text.len(), hl);
            return vec![code_row(row, width)];
        }
        _ => {}
    }
    if text.is_empty() {
        return vec![Line::raw("")];
    }
    let code = matches!(kind, RowKind::Code { .. });
    let starts = crate::wrap::row_starts(text, width);
    starts
        .iter()
        .enumerate()
        .map(|(i, &rs)| {
            let re = starts.get(i + 1).copied().unwrap_or(text.len());
            let row = styled_row(syntax, line_start, text, rs, re, hl);
            // Revealed code (cursor inside the block): keep the set-apart tint so the
            // box stays visible while editing, but let it soft-wrap like prose.
            if code {
                code_row(row, width)
            } else if indent && i == 0 {
                // First-line paragraph indent (#722): a display-only leading space on
                // the paragraph's first visual row only, mirroring `epigraph_lines`'
                // leading-space precedent. Never enters the buffer or any file.
                indented(row)
            } else {
                row
            }
        })
        .collect()
}

/// Set a fenced-code row apart (#723): tint every cell — the row's characters and
/// a trailing pad out to the column edge — with `theme.code` as a background, so
/// the block reads as one continuous box distinct from the surrounding prose. The
/// pad is appended after the content, so it never shifts the on-screen cursor,
/// which is only ever placed before it (the cursor's block is revealed, not preview).
fn code_row(row: Line<'static>, width: usize) -> Line<'static> {
    let bg = theme::current().code_bg;
    let used: usize = row
        .spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    let mut spans: Vec<Span> = row
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content, s.style.bg(bg)))
        .collect();
    let pad = width.saturating_sub(used);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
    }
    Line::from(spans)
}

/// Prepend the display-only first-line indent to a rendered row (#722), reusing the
/// first span's style so the indent doesn't introduce a style change mid-run.
fn indented(row: Line<'static>) -> Line<'static> {
    let style = row.spans.first().map_or(Style::default(), |s| s.style);
    let mut spans = Vec::with_capacity(row.spans.len() + 1);
    spans.push(Span::styled(PARAGRAPH_INDENT, style));
    spans.extend(row.spans);
    Line::from(spans)
}

/// Style one visual row — the byte sub-range `[rs, re)` of `text` — into runs of
/// equal style, dropping concealed (hidden-marker) characters.
fn styled_row(
    syntax: &Syntax,
    line_start: usize,
    text: &str,
    rs: usize,
    re: usize,
    hl: &SpanHighlights,
) -> Line<'static> {
    let seg = &text[rs..re];
    if seg.is_empty() {
        return Line::raw("");
    }
    let seg_start = line_start + rs;
    let seg_end = line_start + re;
    let theme = theme::current(); // once per row, not per character
                                  // Focus mode (#301): the row is entirely outside the focused paragraph, so it
                                  // dims as a whole — caught here before any per-character styling work.
    if hl
        .focus
        .is_some_and(|(s, e)| seg_start >= e || seg_end <= s)
    {
        return Line::from(Span::styled(
            seg.to_string(),
            Style::default().fg(theme.dim),
        ));
    }
    let hls: Vec<Highlight> = syntax.highlights_in(seg_start, seg_end).copied().collect();
    let concealed_here = hl
        .conceal
        .iter()
        .any(|&(s, e)| s < seg_end && e > seg_start);
    let match_here = hl
        .matches
        .iter()
        .any(|&(s, e)| s < seg_end && e > seg_start);
    // `current` (the search current match, or the Visual selection #205) can be
    // set without any entry in `matches`, so check it too before skipping styling.
    let current_here = hl
        .current
        .is_some_and(|(s, e)| s < seg_end && e > seg_start);
    let spell_here = hl.spell.iter().any(|&(s, e)| s < seg_end && e > seg_start);
    let annotated_here = hl
        .annotations
        .iter()
        .any(|&(s, e)| s < seg_end && e > seg_start);
    // A row fully inside the focused span (or focus off) needs no per-char dimming.
    let focus_covers_row = hl.focus.is_none_or(|(s, e)| s <= seg_start && e >= seg_end);
    if hls.is_empty()
        && !concealed_here
        && !match_here
        && !current_here
        && !spell_here
        && !annotated_here
        && focus_covers_row
    {
        return Line::raw(seg.to_string());
    }

    // Build runs character by character, since concealed chars are dropped and
    // visible runs may not be contiguous in the source.
    let mut spans: Vec<Span> = Vec::new();
    let mut run = String::new();
    let mut run_style = Style::default();
    for (local_byte, ch) in seg.char_indices() {
        let abs = seg_start + local_byte;
        if is_concealed(abs, hl.conceal) {
            continue;
        }
        let mut style = search_style(
            style_at(&hls, abs, theme),
            abs,
            hl.matches,
            hl.current,
            theme,
        );
        // Misspelled words (#357) get an underline, layered on whatever else.
        if hl.spell.iter().any(|&(s, e)| abs >= s && abs < e) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        // Annotated spans (#302) get a tinted background, layered on the rest.
        if hl.annotations.iter().any(|&(s, e)| abs >= s && abs < e) {
            style = style.bg(theme.annotation);
        }
        // Focus mode (#301): a char outside the focused paragraph renders flat-dim,
        // overriding any syntax/emphasis/match styling so the paragraph stands out.
        if let Some((s, e)) = hl.focus {
            if abs < s || abs >= e {
                style = Style::default().fg(theme.dim);
            }
        }
        if !run.is_empty() && style != run_style {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
        }
        if run.is_empty() {
            run_style = style;
        }
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style));
    }
    Line::from(spans)
}

/// Merge every highlight covering `byte` into one style. Headings get a color;
/// strong and emphasis layer on bold/italic, so nested `***x***` reads as both.
fn style_at(hls: &[Highlight], byte: usize, theme: Theme) -> Style {
    let mut style = Style::default();
    for h in hls.iter().filter(|h| byte >= h.start && byte < h.end) {
        style = match h.kind {
            HighlightKind::Heading => style.fg(theme.heading).add_modifier(Modifier::BOLD),
            HighlightKind::Strong => style.add_modifier(Modifier::BOLD),
            HighlightKind::Emphasis => style.add_modifier(Modifier::ITALIC),
            // Verbatim, set apart by color; the fence markers show literally (#522).
            HighlightKind::Code => style.fg(theme.code),
        };
    }
    style
}

fn is_concealed(byte: usize, conceal: &[(usize, usize)]) -> bool {
    conceal.iter().any(|&(s, e)| byte >= s && byte < e)
}

/// The absolute byte range of the paragraph under the cursor (#301 focus mode):
/// the run of consecutive non-blank lines around the cursor line. Everything
/// outside it dims. A blank cursor line (between paragraphs) yields an empty span,
/// so the whole page dims until the cursor re-enters prose. Prose paragraphs are
/// single soft-wrapped lines, so the line walk almost always stops immediately.
fn focus_paragraph(buffer: &Buffer) -> ByteRange {
    let cursor_line = buffer.cursor.line;
    let blank = |l: usize| buffer.line(l).trim().is_empty();
    if blank(cursor_line) {
        let at = buffer.line_start_byte(cursor_line);
        return (at, at);
    }
    let mut first = cursor_line;
    while first > 0 && !blank(first - 1) {
        first -= 1;
    }
    let mut last = cursor_line;
    while last + 1 < buffer.line_count() && !blank(last + 1) {
        last += 1;
    }
    (
        buffer.line_start_byte(first),
        buffer.line_start_byte(last) + buffer.line(last).len(),
    )
}

/// Whether logical line `line` is the first line of a prose paragraph that should
/// receive the display-only first-line indent (#722). A paragraph's first line is
/// `sub == 0` of a non-blank logical line; this decides *which* logical lines are
/// indentable prose. Indented: a non-blank line that isn't a heading or fenced
/// code, and isn't the first paragraph immediately after a heading (matching the
/// manuscript convention the exports already follow — `h1+p`/`h2+p` get no indent).
/// Blank lines, headings, and fenced code are never indented; centered chapter/part
/// dividers and epigraphs are render-only inserts that never pass through here.
fn starts_paragraph(buffer: &Buffer, line: usize) -> bool {
    let syntax = buffer.syntax();
    // A logical line is one of these kinds if a highlight of that kind covers its
    // start byte — the whole heading / fenced block is one span (`syntax.rs`).
    let is_kind = |l: usize, kind: HighlightKind| {
        let start = buffer.line_start_byte(l);
        syntax
            .highlights_in(start, start + 1)
            .any(|h| h.kind == kind && h.start <= start && h.end > start)
    };
    let blank = |l: usize| buffer.line(l).trim().is_empty();
    if blank(line) || is_kind(line, HighlightKind::Heading) || is_kind(line, HighlightKind::Code) {
        return false;
    }
    // The first paragraph after a heading gets no indent: walk up past the blank
    // separator lines; if the nearest non-blank line above is a heading, skip it.
    let mut prev = line;
    while prev > 0 && blank(prev - 1) {
        prev -= 1;
    }
    !(prev > 0 && is_kind(prev - 1, HighlightKind::Heading))
}

/// A `focus_width`-wide prose column centered horizontally in `area` (full width
/// when the area is narrow). Shared by both pane renderers (#163).
fn centered_column(area: Rect, focus_width: u16) -> Rect {
    let width = focus_width.min(area.width.saturating_sub(2));
    let left_pad = area.width.saturating_sub(width) / 2;
    Rect {
        x: area.x + left_pad,
        y: area.y,
        width,
        height: area.height,
    }
}

/// Layer search highlighting onto a character's base style: the current match
/// gets a bright background, any other match a dim one (#144).
fn search_style(
    base: Style,
    byte: usize,
    matches: &[ByteRange],
    current: Option<ByteRange>,
    theme: Theme,
) -> Style {
    if let Some((s, e)) = current {
        if byte >= s && byte < e {
            return base.bg(theme.match_current).fg(theme.on_accent);
        }
    }
    if matches.iter().any(|&(s, e)| byte >= s && byte < e) {
        return base.bg(theme.match_other);
    }
    base
}

/// Visible cell width of the characters before `cursor_byte` on a line, skipping
/// concealed (hidden-marker) characters — so the on-screen cursor lands in the
/// right place even though markers have been removed from the rendered text.
fn visible_width_before(
    text: &str,
    line_start: usize,
    cursor_byte: usize,
    conceal: &[(usize, usize)],
) -> u16 {
    let mut width = 0u16;
    for (local_byte, ch) in text.char_indices() {
        let abs = line_start + local_byte;
        if abs >= cursor_byte {
            break;
        }
        if is_concealed(abs, conceal) {
            continue;
        }
        width += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
    }
    width
}

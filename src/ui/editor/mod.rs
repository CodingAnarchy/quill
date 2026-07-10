//! The prose editor pane (#150): soft-wrapped, syntax- and search-highlighted
//! rendering of the active buffer, with cursor following and concealed emphasis
//! markers. The most involved renderer, split out of `ui`. `use super::*` pulls
//! in the shared types and the `Pane` trait.
//!
//! The renderer is split across cohesive submodules, re-exported through here so
//! callers see one `editor` module: [`header`] builds the display-only chapter
//! dividers and epigraph blocks (with their frame cache); [`rows`] soft-wraps and
//! styles each logical line into its visual rows; [`focus`] computes the
//! focus-mode paragraph and first-line-indent geometry. This module keeps the two
//! pane entry points and the shared `render_body` core that drives them.

use std::rc::Rc;

use super::*;
use crate::app::mouse::EditorLayout;
use crate::app::FoldView;
use crate::buffer::Buffer;
use crate::syntax::HighlightKind;

mod focus;
mod galley;
mod header;
mod rows;

pub use galley::{galley_last_top, galley_next_top, galley_prev_top};
// The galley page renderer is dispatched from `ui::draw_editor_base` (#1003).
pub use galley::render_galley;
pub use header::HeaderCache;

use focus::{first_row_indent, focus_paragraph, starts_paragraph};
use header::header_inserts;
use rows::{
    fold_glyph_cells, folded_heading_line, record_row, row_count, row_kind, row_starts_for,
    styled_line, visible_width_before, RowKind, SpanHighlights,
};

/// An absolute byte range `[start, end)` in the buffer — a search match (#144).
type ByteRange = (usize, usize);

/// The display-only first-line paragraph indent (#722): a fixed four spaces on the
/// first visual row of each prose paragraph when the setting is on, matching the
/// exports' first-line indent. Never enters the buffer or any file.
const PARAGRAPH_INDENT: &str = "    ";

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
    let inserts = merge_inserts(
        header_inserts(app, column.width).as_ref(),
        &scene_break_inserts(buffer),
    );
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
    // Active style-flag hits (#1001), computed on idle; underlined like a
    // misspelling under the prose while a sweep is on (empty otherwise).
    let style_flags = app.style_flags.as_slice();
    // Near-proximity word repeats (#1002), computed on idle; dim-marked in place
    // while the toggle is on (empty otherwise).
    let proximity = app.proximity_repeats.as_slice();
    let highlights = SpanHighlights {
        conceal,
        matches: search_matches,
        current: current_match,
        spell,
        style_flags,
        proximity,
        annotations: &annotations,
        focus,
    };
    // Heading/scene folds (#975): the lines a collapsed section hides (rendered as
    // zero rows) and the headings that carry a collapse marker. Display-only —
    // computed off the syntax cache, never touching the buffer.
    let folds = app.fold_view();
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
        &folds,
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
        conceal,
        matches: &[],
        current: None,
        spell: &[],
        style_flags: &[],
        proximity: &[],
        annotations: &[],
        focus: None,
    };
    let inserts = scene_break_inserts(buffer);
    // A reference pane shows a different chapter — folds apply only to the buffer
    // the author is editing, so it never folds (#975).
    let folds = FoldView::none();
    render_body(
        frame,
        column,
        buffer,
        &inserts,
        &highlights,
        &folds,
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
    // Heading/scene folds (#975): a collapsed section's body lines render as zero
    // rows (like a concealed fence), and its heading shows a fold marker. Empty for
    // the reference pane and whenever nothing is folded.
    folds: &FoldView,
    typewriter: bool,
    place_cursor: bool,
    paragraph_indent: bool,
    // Signed viewport offset in visual rows (#764/#936): shift the cursor-driven top
    // by this many rows — positive down (read-ahead), negative up toward the start of
    // the chapter — clamped so no blank space shows past the end and it never runs
    // above the first row. Zero (and `None` for a reference pane) leaves the
    // cursor-driven behavior unchanged. Returns the offset actually applied this
    // frame (clamped to the content) so the caller can store the clamped value.
    scroll_offset: isize,
    // When `Some`, record the focused editor's per-visual-row layout here (#757) so
    // a click can be inverted to a buffer byte. `None` for the reference pane.
    mut layout: Option<&mut EditorLayout>,
) -> isize {
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
        // A line hidden inside a collapsed fold (#975) contributes no visual rows,
        // exactly like a concealed fence — so it drops out of the scroll math and
        // the fill loop alike, and the section reads as just its header.
        if folds.is_hidden(line) {
            return 0;
        }
        // A collapsed heading is always a single row (its marker line), regardless
        // of how the raw heading text would soft-wrap — kept in lockstep with the
        // fill loop's `folded_heading_line`, which emits exactly one row.
        if folds.is_collapsed_heading(line) {
            return 1;
        }
        let start = buffer.line_start_byte(line);
        let text = buffer.line(line);
        let kind = row_kind(syntax, start, &text, &hl.conceal);
        row_count(
            kind,
            &text,
            wrap,
            first_row_indent(paragraph_indent, buffer, line),
        )
    };
    let body_rows = line_rows;

    let total = buffer.line_count();
    let cursor_line = buffer.cursor.line;
    let cursor_byte = buffer.cursor_byte();

    // Where the cursor sits among its own line's visual rows.
    let cline = buffer.line(cursor_line);
    let cline_start = buffer.line_start_byte(cursor_line);
    let cstarts = crate::wrap::row_starts_first_indent(
        &cline,
        wrap,
        first_row_indent(paragraph_indent, buffer, cursor_line),
    );
    let (cursor_subrow, _) = crate::wrap::visual_pos(&cline, &cstarts, cursor_byte - cline_start);
    // The cursor's line always contributes at least one row to the scroll math, even
    // if a fold hid it (#975) — the fold logic snaps the cursor onto a visible line,
    // but this keeps the centering arithmetic (below) from underflowing on any frame
    // where the cursor momentarily sits on a zero-row folded line.
    let cursor_line_rows = body_rows(cursor_line).max(1);
    let cursor_subrow = cursor_subrow.min(cursor_line_rows - 1);

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
        let mut below_avail = cursor_line_rows - 1 - cursor_subrow;
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

    // Wheel scroll (#764/#936): shift the cursor-driven top by the offset — positive
    // advances toward the end (read-ahead), negative pages back toward the start —
    // clamped so the bottom never shows blank space past the end and the top never
    // runs above the first row. Applied by walking the same rows the fill loop does.
    // The offset actually applied this frame, clamped to the content — handed back to
    // the caller so the stored offset can't run away past a valid edge.
    let mut applied_offset = 0isize;
    if scroll_offset > 0 {
        let want = scroll_offset as usize;
        // Rows from the cursor-driven top toward the end of the document. We only
        // need to know whether at least a screenful plus the requested offset
        // remains below — `applied_offset` is capped at `want` — so stop counting
        // once we've seen `height + want` rows rather than walking the whole buffer
        // (#778). The clamp result is identical: `room` is consumed only via
        // `want.min(room)`, so any excess past `height + want` never changes it.
        let enough = height + want;
        let mut rows_below = header_rows(top).saturating_sub(to_skip);
        for l in top..total {
            if rows_below >= enough {
                break;
            }
            if l > top {
                rows_below += header_rows(l);
            }
            rows_below += body_rows(l);
        }
        // The most we can advance and still fill the screen from the bottom up.
        let room = rows_below.saturating_sub(height);
        let mut advance = want.min(room);
        applied_offset = advance as isize;
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
    } else if scroll_offset < 0 {
        // Page back up: retreat the cursor-driven top toward the start of the
        // chapter, stopping at the very first row. `moved` records how far we
        // actually climbed so the stored offset clamps to the top rather than
        // accumulating past it.
        let mut remaining = (-scroll_offset) as usize;
        let mut moved = 0usize;
        while remaining > 0 {
            if to_skip > 0 {
                // Rows still skipped ahead of line `top` retreat one-for-one.
                let step = remaining.min(to_skip);
                to_skip -= step;
                remaining -= step;
                moved += step;
            } else if top > 0 {
                // Step onto the previous line and reveal its last visual row.
                top -= 1;
                let line_rows = header_rows(top) + body_rows(top);
                if line_rows == 0 {
                    // A zero-row line (a hidden fence) shows nothing — skip past it
                    // without spending a row of movement.
                    continue;
                }
                to_skip = line_rows - 1;
                remaining -= 1;
                moved += 1;
            } else {
                // At the first row of the chapter — clamped, no blank space above.
                break;
            }
        }
        applied_offset = -(moved as isize);
    }

    // Record the on-screen layout for click inversion (#757): the column, and one
    // entry per drawn visual row.
    if let Some(map) = layout.as_deref_mut() {
        map.column = column;
        map.conceal = Rc::clone(&hl.conceal);
        map.last_line = total.saturating_sub(1);
        map.rows.clear();
    }

    let mut lines: Vec<Line> = Vec::with_capacity(height);
    let mut cursor_row = None;
    let mut ll = top;
    let mut skip = to_skip;
    'fill: while lines.len() < height && ll < total {
        // A line hidden inside a collapsed fold (#975) contributes nothing — not its
        // body and not any header insert keyed on it (e.g. a scene break that now
        // lives inside the folded section). Skip it whole, in lockstep with
        // `line_rows` returning 0, so the scroll math and the render agree.
        if folds.is_hidden(ll) {
            ll += 1;
            continue;
        }
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
        let kind = row_kind(syntax, line_start, &text, &hl.conceal);
        let verbatim = matches!(kind, RowKind::Code { verbatim: true });
        // A collapsed heading (#975) renders its header text with a leading fold
        // glyph so the author sees the section is folded; the byte span still maps
        // per raw character so the cursor and clicks land correctly on the heading.
        let collapsed = folds.is_collapsed_heading(ll);
        // A collapsed heading is one row spanning the whole line (like a concealed
        // scene break), so a click on it maps to the heading's start; otherwise the
        // usual per-wrap-row starts drive the click map.
        let starts = if collapsed {
            vec![0]
        } else {
            row_starts_for(
                kind,
                &text,
                wrap,
                if indent { PARAGRAPH_INDENT.len() } else { 0 },
            )
        };
        let body = if collapsed {
            folded_heading_line(syntax, line_start, &text, hl)
        } else {
            styled_line(syntax, line_start, &text, hl, wrap, indent, kind)
        };
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
            // A collapsed heading (#975) is rendered with a leading display-only fold
            // glyph that isn't in the buffer, so a per-character click→byte walk would
            // land the cursor ~glyph-width cells too far right (bug I). Record it as a
            // non-editable row (like a divider) so a click maps to the heading's start
            // byte — the one sensible edit point on a collapsed header — and the glyph
            // never desyncs the click map. The cursor-x below adds the glyph width so
            // the caret still renders over the heading text.
            record_row(
                &mut layout,
                ll,
                buffer,
                rs,
                re,
                !collapsed,
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
                &hl.conceal,
            );
            // The first-line indent shifts the cursor right too, but only on the
            // paragraph's first visual row (#722) — continuation rows aren't indented.
            if paragraph_indent && cursor_subrow == 0 && starts_paragraph(buffer, cursor_line) {
                col_width += PARAGRAPH_INDENT.len() as u16;
            }
            // A collapsed heading (#975) renders with a leading display-only fold
            // glyph that isn't in the buffer, so the heading text — and any caret on it
            // — sits that many cells further right than the raw character widths imply.
            // Add the glyph width so the caret tracks the on-screen text (bug I),
            // mirroring the first-line-indent shift above.
            if folds.is_collapsed_heading(cursor_line) {
                col_width += fold_glyph_cells();
            }
            let screen_x = column.x + col_width.min(column.width.saturating_sub(1));
            frame.set_cursor_position((screen_x, screen_y));
        }
    }

    applied_offset
}

/// Render-only blank rows for extra breathing room around each scene break (#908),
/// keyed by the buffer line each precedes — one before the break line and one
/// before the line right after it. Modeled on the chapter-title `header_inserts`
/// mechanism: the fill loop threads them in as non-editable rows and accounts their
/// counts in the scroll math, so a scene division stands apart while scrolling
/// without ever touching the buffer. The break's raw `---` stays the real, editable
/// line — these are only the surrounding whitespace.
fn scene_break_inserts(buffer: &Buffer) -> Vec<(usize, Vec<Line<'static>>)> {
    let syntax = buffer.syntax();
    let mut inserts: Vec<(usize, Vec<Line<'static>>)> = Vec::new();
    let gap = || vec![Line::raw("")];
    for h in syntax.highlights_in(0, usize::MAX) {
        if h.kind != HighlightKind::SceneBreak {
            continue;
        }
        let line = buffer.cursor_at_byte(h.start).line;
        // One blank above the divider, and one before the line that resumes after
        // it — symmetric breathing room larger than a normal paragraph gap.
        inserts.push((line, gap()));
        if line + 1 < buffer.line_count() {
            inserts.push((line + 1, gap()));
        }
    }
    inserts
}

/// Merge two insert lists (each keyed by the buffer line it precedes) into one
/// keyed list, concatenating the blocks of any shared line so the `render_body`
/// fill loop — which finds a single entry per line — sees both (#908). Kept sorted
/// by line for a stable, predictable order.
fn merge_inserts(
    a: &[(usize, Vec<Line<'static>>)],
    b: &[(usize, Vec<Line<'static>>)],
) -> Vec<(usize, Vec<Line<'static>>)> {
    let mut merged: Vec<(usize, Vec<Line<'static>>)> = a.to_vec();
    for (line, block) in b {
        if let Some(entry) = merged.iter_mut().find(|(at, _)| at == line) {
            entry.1.extend(block.iter().cloned());
        } else {
            merged.push((*line, block.clone()));
        }
    }
    merged.sort_by_key(|(at, _)| *at);
    merged
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

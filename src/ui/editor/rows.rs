//! Soft-wrapping, fenced-code classification, and per-character styling of the
//! prose body (#143/#144/#296/#723). The renderer in the parent module walks the
//! buffer's logical lines; this module turns each into its styled visual rows —
//! wrapping the text, dropping concealed emphasis markers, and layering syntax,
//! search, spell, annotation, and focus styling onto every run.

use std::rc::Rc;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::mouse::{EditorLayout, VisualRow};
use crate::buffer::Buffer;
use crate::syntax::{Highlight, HighlightKind, Syntax};
use crate::theme::{self, Theme};

use super::header::centered_line;
use super::{ByteRange, PARAGRAPH_INDENT};

/// The typeset scene-break divider (#908) a concealed thematic break renders as —
/// the printed-book convention of three spaced asterisks, centered in the column.
const SCENE_BREAK_GLYPH: &str = "* * *";

/// The glyph a collapsed heading (#975) carries so the author can see the section
/// is folded: a right-pointing triangle before the header text, the near-universal
/// "expandable" affordance. An expanded heading shows no glyph.
const FOLD_GLYPH: &str = "▸ ";

/// The display width (in terminal cells) of [`FOLD_GLYPH`], computed once so the
/// cursor-x math in the parent renderer can shift the caret past the display-only
/// glyph on a collapsed heading row — mirroring how the first-line paragraph indent
/// (#722) is added back after being peeled. Kept beside the glyph so the two can't
/// drift.
pub(super) fn fold_glyph_cells() -> u16 {
    unicode_width::UnicodeWidthStr::width(FOLD_GLYPH) as u16
}

/// The "what to highlight in this span" overlays threaded through the body
/// renderer (#431): one cohesive bundle, built once per pane and passed by
/// reference through `render_body → styled_line → styled_row`, so a new overlay
/// kind is one field rather than a parameter added to three signatures.
pub(super) struct SpanHighlights<'a> {
    /// Emphasis-marker ranges to hide, except on the cursor's span (#296). Held as
    /// the `Rc` the parse caches (#869) so the layout recorder shares it for click
    /// inversion rather than cloning the ranges each frame.
    pub(super) conceal: Rc<[(usize, usize)]>,
    /// Search/replace matches or the Visual selection, dim-highlighted (#144/#205).
    pub(super) matches: &'a [ByteRange],
    /// The current search match / live selection, highlighted brighter (#144/#205).
    pub(super) current: Option<ByteRange>,
    /// Misspelled-word ranges, underlined (#357).
    pub(super) spell: &'a [ByteRange],
    /// Active style-flag ranges, underlined (#1001) — the current sweep category's
    /// hits, marked in place like a misspelling so the author can see each as they
    /// step through it. Empty (no underline) when no sweep is active.
    pub(super) style_flags: &'a [ByteRange],
    /// Near-proximity repeated content words, dim-marked (#1002) — a word echoed
    /// within a short window of the visible text, surfaced in place as a subdued
    /// marker while the toggle is on so the local repetition is visible where it
    /// happens. Empty (no marking) when the toggle is off.
    pub(super) proximity: &'a [ByteRange],
    /// Annotated spans, tinted background (#302).
    pub(super) annotations: &'a [ByteRange],
    /// The focus-mode paragraph; everything outside it dims (#301).
    pub(super) focus: Option<ByteRange>,
}

/// How a logical line renders with respect to fenced code blocks (#723). Prose is
/// the default; the fence lines and code content of a block get special treatment
/// that mirrors emphasis's reveal-on-cursor concealment.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RowKind {
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
    /// A thematic-break line — the author's scene divider (#908). In preview
    /// (cursor outside) it renders as a single centered `* * *` row instead of the
    /// raw markers; `revealed` (cursor on the line) shows the raw text as prose so
    /// it can be edited or deleted. The `---` stays a real, editable buffer line —
    /// this is the heading/emphasis "transform an editable line" model.
    SceneBreak { revealed: bool },
}

/// Classify a logical line for fenced-code rendering (#723). A line is a
/// [`RowKind::HiddenFence`] when its whole content sits inside a currently-active
/// conceal range (a hidden fence row); a [`RowKind::Code`] when a `Code` highlight
/// covers its start byte. `verbatim` is set when that block's fences are concealed
/// (preview state) — the same all-or-nothing signal the conceal list already
/// carries, so no separate cursor test is needed here.
pub(super) fn row_kind(
    syntax: &Syntax,
    line_start: usize,
    text: &str,
    conceal: &[(usize, usize)],
) -> RowKind {
    let line_end = line_start + text.len();
    // A scene break (#908): a `SceneBreak` span covers the line's start. It renders
    // as a centered `* * *` divider while concealed (cursor outside), and as its raw
    // markers while revealed (cursor on the line) so it can be edited. Checked before
    // the hidden-fence rule because the break's own conceal marker would otherwise
    // read as a hidden fence and drop the line to zero rows. A blank line is never a
    // break (the grammar needs marker characters), so an empty line falls through.
    let probe = line_start.min(line_end.saturating_sub(1));
    if !text.is_empty() {
        if let Some(brk) = syntax
            .highlights_in(line_start, line_end.max(line_start + 1))
            .find(|h| h.kind == HighlightKind::SceneBreak && h.start <= probe && probe < h.end)
        {
            // Revealed exactly when this break's own line is not concealed — the same
            // all-or-nothing reveal the conceal list carries for a fenced block.
            let revealed = !conceal.iter().any(|&(s, e)| s < brk.end && e > brk.start);
            return RowKind::SceneBreak { revealed };
        }
    }
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
pub(super) fn row_count(kind: RowKind, text: &str, width: usize, first_indent: usize) -> usize {
    match kind {
        RowKind::HiddenFence => 0,
        // Verbatim preview code is one row (no reflow); a concealed scene break is
        // one centered `* * *` divider row (#908). A revealed break shows the raw
        // markers, which soft-wrap like any short line.
        RowKind::Code { verbatim: true } | RowKind::SceneBreak { revealed: false } => 1,
        _ => crate::wrap::row_starts_first_indent(text, width, first_indent).len(),
    }
}

/// The byte offsets (within the line) of each visual row's start, honoring
/// fenced-code treatment (#723) so the click layout map (#757) agrees with the
/// render: verbatim preview code is a single row starting at 0; a hidden fence has
/// none; everything else soft-wraps. A trailing sentinel isn't added — the caller
/// reads the end of the last row as the line length.
pub(super) fn row_starts_for(
    kind: RowKind,
    text: &str,
    width: usize,
    first_indent: usize,
) -> Vec<usize> {
    match kind {
        RowKind::HiddenFence => Vec::new(),
        // Verbatim preview code and the concealed `* * *` divider (#908) are each a
        // single row spanning the whole line, so a click maps to the line's start;
        // a revealed break maps per raw character like prose.
        RowKind::Code { verbatim: true } | RowKind::SceneBreak { revealed: false } => vec![0],
        _ => crate::wrap::row_starts_first_indent(text, width, first_indent),
    }
}

/// The three independent click-map traits of a recorded visual row (#1083) —
/// named so a call site reads `RowFlags { editable, verbatim, indent }` instead of
/// three bare, positional booleans. Each is orthogonal (a row can be any
/// combination), so a struct of named fields fits better than an enum.
#[derive(Clone, Copy)]
pub(super) struct RowFlags {
    /// Whether the row carries editable buffer text (a click maps per character),
    /// as opposed to a display-only row like a header block or collapsed heading
    /// (a click maps to the line's start).
    pub(super) editable: bool,
    /// Whether the row is verbatim preview code — whitespace preserved, not
    /// reflowed — so the click map treats it as a single unwrapped row.
    pub(super) verbatim: bool,
    /// Whether the row carries the display-only first-line paragraph indent (#722),
    /// so the click→byte math accounts for the leading indent that isn't in the
    /// buffer.
    pub(super) indent: bool,
}

/// Append one visual row to the click layout map (#757), if one is being recorded.
/// `rs`/`re` are the row's byte span within its logical line.
pub(super) fn record_row(
    layout: &mut Option<&mut EditorLayout>,
    line: usize,
    buffer: &Buffer,
    rs: usize,
    re: usize,
    flags: RowFlags,
) {
    if let Some(map) = layout.as_deref_mut() {
        map.rows.push(VisualRow {
            line,
            line_start: buffer.line_start_byte(line),
            row_start: rs,
            row_end: re,
            editable: flags.editable,
            verbatim: flags.verbatim,
            indent: flags.indent,
        });
    }
}

/// Soft-wrap a logical line into its visual rows (#143) and style each. `text` is
/// one line (no newline); `line_start` its absolute byte offset (highlights,
/// conceal, and match ranges use document offsets). Returns one [`Line`] per
/// wrapped row — or no rows for a hidden fence line, and a single verbatim row for
/// preview code (#723).
pub(super) fn styled_line(
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
        // Concealed scene break (#908): one centered `* * *` divider row in place of
        // the raw markers, dimmed so it reads as a quiet typeset division. Revealed
        // (cursor on the line) it falls through to render the raw text as prose.
        RowKind::SceneBreak { revealed: false } => {
            let style = Style::default().fg(theme::current().dim);
            let col = u16::try_from(width).unwrap_or(u16::MAX);
            return vec![centered_line(SCENE_BREAK_GLYPH, col, style)];
        }
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
    let starts = crate::wrap::row_starts_first_indent(
        text,
        width,
        if indent { PARAGRAPH_INDENT.len() } else { 0 },
    );
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

/// Render a collapsed heading (#975) as a single row: its styled header text with a
/// leading fold glyph, so the author sees the section is folded and can expand it.
/// Display-only, like the scene-break divider — the raw heading line is untouched in
/// the buffer. Returned as a one-element `Vec` so it drops straight into the fill
/// loop beside `styled_line`.
pub(super) fn folded_heading_line(
    syntax: &Syntax,
    line_start: usize,
    text: &str,
    hl: &SpanHighlights,
) -> Vec<Line<'static>> {
    // Style the raw heading text as it renders normally (concealment applied), then
    // prepend the fold glyph in the heading color so the marker reads as part of it.
    let row = styled_row(syntax, line_start, text, 0, text.len(), hl);
    let glyph_style = row.spans.first().map_or_else(
        || Style::default().fg(theme::current().heading),
        |s| s.style,
    );
    let mut spans = Vec::with_capacity(row.spans.len() + 1);
    spans.push(Span::styled(FOLD_GLYPH, glyph_style));
    spans.extend(row.spans);
    vec![Line::from(spans)]
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
    let style = row.spans.first().map_or_else(Style::default, |s| s.style);
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
    let has_highlight = syntax.highlights_in(seg_start, seg_end).next().is_some();
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
    let style_flag_here = hl
        .style_flags
        .iter()
        .any(|&(s, e)| s < seg_end && e > seg_start);
    let proximity_here = hl
        .proximity
        .iter()
        .any(|&(s, e)| s < seg_end && e > seg_start);
    let annotated_here = hl
        .annotations
        .iter()
        .any(|&(s, e)| s < seg_end && e > seg_start);
    // A row fully inside the focused span (or focus off) needs no per-char dimming.
    let focus_covers_row = hl.focus.is_none_or(|(s, e)| s <= seg_start && e >= seg_end);
    if !has_highlight
        && !concealed_here
        && !match_here
        && !current_here
        && !spell_here
        && !style_flag_here
        && !proximity_here
        && !annotated_here
        && focus_covers_row
    {
        return Line::raw(seg.to_string());
    }

    // Past the fast path: some styling applies here, so materialize the highlight
    // spans now (skipped for the common no-highlight row handled above).
    let hls: Vec<Highlight> = syntax.highlights_in(seg_start, seg_end).copied().collect();

    // Build runs character by character, since concealed chars are dropped and
    // visible runs may not be contiguous in the source.
    let mut spans: Vec<Span> = Vec::new();
    let mut run = String::new();
    let mut run_style = Style::default();
    for (local_byte, ch) in seg.char_indices() {
        let abs = seg_start + local_byte;
        if is_concealed(abs, &hl.conceal) {
            continue;
        }
        let mut style = search_style(
            style_at(&hls, abs, theme),
            abs,
            hl.matches,
            hl.current,
            theme,
        );
        // Misspelled words (#357) and active style-flag hits (#1001) get an
        // underline, layered on whatever else — the shared "flagged in place" mark.
        if hl.spell.iter().any(|&(s, e)| abs >= s && abs < e)
            || hl.style_flags.iter().any(|&(s, e)| abs >= s && abs < e)
        {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        // Near-proximity repeats (#1002) get a dim marker — a subdued cue that this
        // content word was echoed within a short window, layered on the rest so the
        // author sees the local repetition without it shouting like a misspelling.
        if hl.proximity.iter().any(|&(s, e)| abs >= s && abs < e) {
            style = style.add_modifier(Modifier::DIM);
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
            // Only reached when the break is revealed (cursor on the line): the raw
            // `---` markers show dimmed so they read as the divider they'll become
            // once the cursor leaves (#908). Concealed breaks never enter this path.
            HighlightKind::SceneBreak => style.fg(theme.dim),
        };
    }
    style
}

fn is_concealed(byte: usize, conceal: &[(usize, usize)]) -> bool {
    conceal.iter().any(|&(s, e)| byte >= s && byte < e)
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
pub(super) fn visible_width_before(
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

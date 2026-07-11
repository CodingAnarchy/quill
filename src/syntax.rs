//! Incremental Markdown parsing.
//!
//! This is the shared parse the rest of Quill builds on. It wraps
//! [`tree_sitter_md`], which parses Markdown in two layers — a block grammar
//! (headings, lists, paragraphs) and an inline grammar (emphasis, links).
//!
//! Parsing is split from editing on purpose. [`Syntax::record_edit`] is cheap
//! (it only fixes up tree-sitter's offset bookkeeping) and runs on the
//! keystroke path; [`Syntax::reparse`] does the real work and is driven off the
//! hot path once typing goes idle (see the event loop in `main.rs`). The cost of
//! a reparse scales with document size — fine for a chapter, which is the unit a
//! buffer holds — so keeping it off every keystroke is what keeps typing instant.
//!
//! For now it exposes highlight spans (used by the renderer). The block tree is
//! kept available for the upcoming in-chapter outline (issue #11) to consume
//! rather than re-parsing.

use std::rc::Rc;

use ropey::Rope;
use tree_sitter::{InputEdit, Node, Point};
use tree_sitter_md::{MarkdownParseOptions, MarkdownParser, MarkdownTree};

/// What a highlighted span represents. The renderer maps each to a `Style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Heading,
    Strong,
    Emphasis,
    /// A fenced code block (#522), rendered verbatim and set apart — no emphasis
    /// concealment or smart typography applies inside it.
    Code,
    /// A CommonMark thematic break (#908) — the author's scene divider (`---`,
    /// `***`, `* * *`, …). Rendered as a centered `* * *` divider with breathing
    /// room, and drives the flush-left rule for the paragraph that resumes after
    /// it. The raw markers reveal on cursor, like a fenced-code fence line.
    SceneBreak,
}

/// A styled range of the document, in absolute byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Highlight {
    pub start: usize,
    pub end: usize,
    pub kind: HighlightKind,
}

/// An emphasis delimiter run (`*`, `_`, `**`, `__`) and the span it belongs to.
/// The renderer hides the delimiter, except when the cursor is inside the span
/// so the author can edit the markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Marker {
    /// The delimiter's own byte range (what gets concealed).
    delim_start: usize,
    delim_end: usize,
    /// The enclosing emphasis/strong span; concealment lifts when the cursor is
    /// anywhere within it.
    span_start: usize,
    span_end: usize,
}

/// A heading in the document — the in-chapter outline is a list of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    /// Heading level (1–6), used to indent the outline.
    pub level: u8,
    /// The heading text, without markers.
    pub title: String,
    /// Byte offset where the heading begins, for "which section is the cursor in".
    pub start: usize,
}

/// The live parse of a single document (one chapter file).
pub struct Syntax {
    parser: MarkdownParser,
    tree: MarkdownTree,
    /// Highlight spans for the whole document, sorted by start byte.
    highlights: Vec<Highlight>,
    /// Emphasis delimiters, for soft-rendering (concealing) the markers.
    markers: Vec<Marker>,
    /// Every marker's conceal range (all delimiters hidden), start-sorted — the
    /// full list [`conceal_ranges`](Self::conceal_ranges) returns when nothing is
    /// revealed. Built once per parse/reparse (markers change only then) and shared
    /// via `Rc` so per-frame rendering neither rebuilds nor clones it (#869).
    conceal_all: Rc<[(usize, usize)]>,
    /// Headings in document order — the in-chapter outline (#11).
    headings: Vec<Heading>,
    /// The buffer line each scene break (`---`, `* * *`, …) sits on, in document
    /// order. Computed once per parse/reparse (breaks change only then) so the
    /// scene-break divider renderer reads a ready slice instead of scanning every
    /// highlight and allocating a line-lookup `String` per break each frame — the
    /// same shape as the `conceal_all`/`highlights` caches.
    scene_breaks: Vec<usize>,
}

impl Syntax {
    /// Parse a document from scratch.
    pub fn new(text: &str) -> Self {
        let mut parser = MarkdownParser::default();
        #[allow(
            clippy::expect_used,
            reason = "tree-sitter `parse` returns `None` only on a timeout or \
                      cancellation, neither of which this call configures"
        )]
        let tree = parser
            .parse(text.as_bytes(), None)
            .expect("markdown parse should not fail without a timeout/cancellation");
        let (highlights, markers, headings) = collect(&tree, |s, e| text[s..e].to_string());
        let conceal_all = conceal_all_from(&markers);
        let scene_breaks = scene_break_lines_from(&highlights, |b| byte_to_line_in_str(text, b));
        Self {
            parser,
            tree,
            highlights,
            markers,
            conceal_all,
            headings,
            scene_breaks,
        }
    }

    /// Record one edit so the next [`reparse`](Self::reparse) is incremental.
    /// `edit` describes the change in the pre-edit coordinate space (the
    /// tree-sitter contract). Cheap — safe to call on the keystroke path; call
    /// it once per edit, then `reparse` once when convenient. Multiple recorded
    /// edits are coalesced into a single incremental reparse.
    pub fn record_edit(&mut self, edit: &InputEdit) {
        self.tree.edit(edit);
    }

    /// Re-parse incrementally against the recorded edits and refresh highlights.
    /// Run off the keystroke path (on idle). Reads the current document straight
    /// from the `rope` — both the tree-sitter parse and the heading-text lookups
    /// pull chunks/slices from it, so no full-document copy is allocated per
    /// reparse (#218). The re-collect is over the bounded editing window.
    pub fn reparse(&mut self, rope: &Rope) {
        let len = rope.len_bytes();
        // Feed tree-sitter rope chunks: it asks for the bytes starting at an
        // offset and we hand back the rest of that chunk (empty past the end).
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= len {
                return &[];
            }
            let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - chunk_start..]
        };
        if let Some(tree) = self.parser.parse_with_options(
            &mut read,
            Some(&self.tree),
            MarkdownParseOptions::default(),
        ) {
            self.tree = tree;
            let (highlights, markers, headings) =
                collect(&self.tree, |s, e| rope.byte_slice(s..e).to_string());
            self.scene_breaks =
                scene_break_lines_from(&highlights, |b| rope.byte_to_line(b.min(len)));
            self.highlights = highlights;
            self.conceal_all = conceal_all_from(&markers);
            self.markers = markers;
            self.headings = headings;
        }
    }

    /// Highlights overlapping `[start, end)` in document order. The list is
    /// start-sorted, so a binary search skips every highlight starting at or
    /// after `end` rather than scanning the whole list per rendered line (#218).
    pub fn highlights_in(&self, start: usize, end: usize) -> impl Iterator<Item = &Highlight> {
        let upper = self.highlights.partition_point(|h| h.start < end);
        self.highlights[..upper]
            .iter()
            .filter(move |h| h.end > start)
    }

    /// Emphasis-delimiter byte ranges to hide when rendering. A delimiter is
    /// concealed unless `reveal_at` (the cursor's byte offset) lies within the
    /// delimiter's enclosing span — so the markers reappear for editing only on
    /// the span the cursor is in. Pass `None` to conceal every delimiter.
    ///
    /// Returns a shared `Rc` so the renderer neither rebuilds the list per frame
    /// nor clones it into the click-inversion layout (#869). The common cases —
    /// `None`, or a cursor not inside any emphasis span — hand back the cached
    /// full list (a refcount bump, no allocation); only a cursor actually sitting
    /// on a span (which reveals that span's markers) builds a filtered list.
    pub fn conceal_ranges(&self, reveal_at: Option<usize>) -> Rc<[(usize, usize)]> {
        let Some(b) = reveal_at else {
            return Rc::clone(&self.conceal_all);
        };
        // Reveal only when the cursor is actually within some span; otherwise the
        // filtered result is byte-for-byte the full list, so reuse the cache.
        let reveals_any = self
            .markers
            .iter()
            .any(|m| m.span_start <= b && b <= m.span_end);
        if !reveals_any {
            return Rc::clone(&self.conceal_all);
        }
        self.markers
            .iter()
            .filter(|m| !(m.span_start <= b && b <= m.span_end))
            .map(|m| (m.delim_start, m.delim_end))
            .collect()
    }

    /// Headings in document order — the in-chapter outline.
    pub fn headings(&self) -> &[Heading] {
        &self.headings
    }

    /// The buffer line each scene break sits on, in document order (#908). Cached
    /// per parse/reparse so the scene-break divider renderer reads it directly
    /// rather than scanning every highlight and doing a per-break line lookup each
    /// frame.
    pub fn scene_break_lines(&self) -> &[usize] {
        &self.scene_breaks
    }
}

/// The full conceal list (every marker's delimiter range) as a shared slice, in
/// the markers' start-sorted order. Cached on [`Syntax`] per parse/reparse (#869).
fn conceal_all_from(markers: &[Marker]) -> Rc<[(usize, usize)]> {
    markers
        .iter()
        .map(|m| (m.delim_start, m.delim_end))
        .collect()
}

/// The buffer line each scene break sits on, in document order — cached on
/// [`Syntax`] per parse/reparse (#908). Highlights are start-sorted, so the breaks
/// come out in document order; `line_of` maps a break's start byte to its line.
fn scene_break_lines_from(
    highlights: &[Highlight],
    mut line_of: impl FnMut(usize) -> usize,
) -> Vec<usize> {
    highlights
        .iter()
        .filter(|h| h.kind == HighlightKind::SceneBreak)
        .map(|h| line_of(h.start))
        .collect()
}

/// The 0-based line a byte offset falls on within `text` — the newline count
/// before it. Matches `Rope::byte_to_line`, so the scene-break line cache is the
/// same whether built from a `&str` (initial parse) or a `Rope` (reparse).
fn byte_to_line_in_str(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text.as_bytes()[..byte]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

/// Walk the block tree (headings) and every inline tree (emphasis/strong) into a
/// flat, start-sorted list of highlight spans plus the emphasis delimiters to
/// conceal. Inline trees carry absolute document offsets, so their node ranges
/// line up with the block tree's.
fn collect(
    tree: &MarkdownTree,
    read: impl Fn(usize, usize) -> String,
) -> (Vec<Highlight>, Vec<Marker>, Vec<Heading>) {
    let mut out = Vec::new();
    let mut markers = Vec::new();
    let mut headings = Vec::new();

    walk(tree.block_tree().root_node(), &mut |node| {
        if matches!(node.kind(), "atx_heading" | "setext_heading") {
            out.push(Highlight {
                start: node.start_byte(),
                end: node.end_byte(),
                kind: HighlightKind::Heading,
            });
            headings.push(heading_info(&node, &read));
        } else if node.kind() == "fenced_code_block" {
            // The whole fence (delimiters + content) is set apart; its content
            // has no inline tree, so no emphasis is parsed or concealed inside.
            out.push(Highlight {
                start: node.start_byte(),
                end: node.end_byte(),
                kind: HighlightKind::Code,
            });
            // Register the opening/closing fence *lines* as concealable markers,
            // reusing the emphasis reveal-on-cursor path (#723): each fence line is
            // hidden unless the cursor is anywhere in the block, so the block reads
            // as a set-apart preview until you step in to edit the raw markup. The
            // reveal span is the whole `fenced_code_block` node. The line range runs
            // from a delimiter's line start to the byte after it (including the
            // newline), so the whole row disappears — not just the backticks.
            for line in fence_delimiter_lines(&node, &read) {
                markers.push(Marker {
                    delim_start: line.0,
                    delim_end: line.1,
                    span_start: node.start_byte(),
                    span_end: node.end_byte(),
                });
            }
        } else if node.kind() == "thematic_break" {
            // A scene break (#908). Driving this off the tree-sitter node (not a
            // regex over line text) is what keeps a setext-heading underline —
            // `---` on the line below a title — from being misread as a break: the
            // grammar parses that underline as part of the `setext_heading`, so no
            // `thematic_break` node is emitted there.
            out.push(Highlight {
                start: node.start_byte(),
                end: node.end_byte(),
                kind: HighlightKind::SceneBreak,
            });
            // Conceal the raw markers so the line renders as a centered divider,
            // reusing the fenced-code reveal-on-cursor path (#723): the marker range
            // is the break line itself and the reveal span is the same node, so the
            // divider becomes the raw text only while the cursor sits on it. The
            // range runs to the byte after the line's newline (clamped to the node)
            // so it lines up with the whole logical line the renderer conceals.
            markers.push(Marker {
                delim_start: node.start_byte(),
                delim_end: node.end_byte(),
                span_start: node.start_byte(),
                span_end: node.end_byte(),
            });
        }
    });

    for inline in tree.inline_trees() {
        walk(inline.root_node(), &mut |node| {
            let kind = match node.kind() {
                "strong_emphasis" => HighlightKind::Strong,
                "emphasis" => HighlightKind::Emphasis,
                _ => return,
            };

            // Record this span's own delimiter children (e.g. the two `*` in
            // `*x*`, or the `**` in `**y**`) so the renderer can hide them.
            let mut cursor = node.walk();
            if cursor.goto_first_child() {
                loop {
                    let child = cursor.node();
                    if child.kind() == "emphasis_delimiter" {
                        markers.push(Marker {
                            delim_start: child.start_byte(),
                            delim_end: child.end_byte(),
                            span_start: node.start_byte(),
                            span_end: node.end_byte(),
                        });
                    }
                    if !cursor.goto_next_sibling() {
                        break;
                    }
                }
            }

            out.push(Highlight {
                start: node.start_byte(),
                end: node.end_byte(),
                kind,
            });
        });
    }

    out.sort_by_key(|h| h.start);
    markers.sort_by_key(|m| m.delim_start);
    headings.sort_by_key(|h| h.start);
    (out, markers, headings)
}

/// The byte range of each fence-delimiter *line* in a `fenced_code_block` — the
/// opening and (when present) closing ``` ``` ``` rows, each spanning from the
/// delimiter's line start through its trailing newline so the whole row can be
/// hidden. The delimiter node itself is just the backticks (plus, for the opener,
/// an info string on the same line), so we extend the range to the next newline
/// (inclusive) — the actual end of the fence row — clamped to the block's end.
fn fence_delimiter_lines(
    node: &Node,
    read: &impl Fn(usize, usize) -> String,
) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let node_end = node.end_byte();
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.kind() == "fenced_code_block_delimiter" {
                let start = child.start_byte();
                // Read from the delimiter to the block's end and find its line's
                // terminating newline; the concealed range covers up to and
                // including it so the whole row (backticks + any info string +
                // newline) disappears. An unterminated final fence ends at the block.
                let rest = read(start, node_end);
                let line_end = rest
                    .find('\n')
                    .map_or(node_end, |nl| start + nl + 1)
                    .min(node_end);
                lines.push((start, line_end));
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    lines
}

/// Pull a heading's level and title out of its node. Reads the heading's source
/// text (via `read`) rather than walking marker children, which keeps atx and
/// setext uniform.
fn heading_info(node: &Node, read: &impl Fn(usize, usize) -> String) -> Heading {
    let raw = read(node.start_byte(), node.end_byte());
    if node.kind() == "setext_heading" {
        // "Title\n===" (level 1) or "Title\n---" (level 2).
        let mut lines = raw.lines();
        let title = lines.next().unwrap_or("").trim().to_string();
        let underline = lines.next().unwrap_or("");
        let level = if underline.trim_start().starts_with('=') {
            1
        } else {
            2
        };
        Heading {
            level,
            title,
            start: node.start_byte(),
        }
    } else {
        // "### Title ###" — count leading '#', strip optional closing '#'.
        let line = raw.lines().next().unwrap_or("").trim();
        let hashes = line.chars().take_while(|&c| c == '#').count();
        let level = hashes.clamp(1, 6) as u8;
        let title = line[hashes..]
            .trim()
            .trim_end_matches('#')
            .trim()
            .to_string();
        Heading {
            level,
            title,
            start: node.start_byte(),
        }
    }
}

/// Pre-order traversal. Markdown nesting is shallow, so plain recursion is fine.
fn walk<'a>(node: Node<'a>, f: &mut impl FnMut(Node<'a>)) {
    f(node);
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            walk(cursor.node(), f);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(s: &Syntax) -> Vec<Highlight> {
        s.highlights_in(0, usize::MAX).copied().collect()
    }

    #[test]
    fn detects_atx_heading() {
        let text = "# Chapter One\n\nBody text.\n";
        let s = Syntax::new(text);
        let h = all(&s)
            .into_iter()
            .find(|h| h.kind == HighlightKind::Heading)
            .expect("heading highlight");
        // The heading node range includes the line's trailing newline.
        assert_eq!(text[h.start..h.end].trim_end(), "# Chapter One");
    }

    #[test]
    fn detects_emphasis_and_strong() {
        let text = "plain *ital* and **bold** end\n";
        let s = Syntax::new(text);
        let hs = all(&s);

        let em = hs
            .iter()
            .find(|h| h.kind == HighlightKind::Emphasis)
            .expect("emphasis");
        assert_eq!(&text[em.start..em.end], "*ital*");

        let st = hs
            .iter()
            .find(|h| h.kind == HighlightKind::Strong)
            .expect("strong");
        assert_eq!(&text[st.start..st.end], "**bold**");
    }

    #[test]
    fn highlights_in_filters_by_range() {
        // Two headings on different lines; range query returns only the overlap.
        let text = "# One\n\n## Two\n";
        let s = Syntax::new(text);
        let second_line_start = text.find("## Two").unwrap();
        let hits: Vec<_> = s
            .highlights_in(second_line_start, text.len())
            .copied()
            .collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(text[hits[0].start..hits[0].end].trim_end(), "## Two");
    }

    // Collect the concealed substrings for readability in the assertions below.
    fn concealed(text: &str, s: &Syntax, reveal_at: Option<usize>) -> Vec<String> {
        s.conceal_ranges(reveal_at)
            .iter()
            .map(|&(a, b)| text[a..b].to_string())
            .collect()
    }

    #[test]
    fn conceals_emphasis_and_strong_delimiters() {
        let text = "a *ital* and **bold** z\n";
        let s = Syntax::new(text);
        // Every delimiter char is concealed: `*ital*` has two, `**bold**` has
        // four (the grammar emits each `*` as its own delimiter node).
        let hidden = concealed(text, &s, None);
        assert_eq!(hidden.len(), 6);
        assert!(hidden.iter().all(|d| d == "*"));
    }

    #[test]
    fn cursor_in_span_reveals_only_that_spans_markers() {
        let text = "a *ital* and **bold** z\n";
        let s = Syntax::new(text);
        let bold_start = text.find("**bold").unwrap();

        // Cursor inside *ital*: its two `*` reveal; the four `*` of **bold** stay
        // hidden (and they're the ones near the bold span).
        let inside_ital = text.find("ital").unwrap() + 1;
        let still_hidden = s.conceal_ranges(Some(inside_ital));
        assert_eq!(still_hidden.len(), 4);
        assert!(still_hidden.iter().all(|&(a, _)| a >= bold_start));

        // Cursor inside **bold**: its four `*` reveal; only the two `*` of *ital*
        // stay hidden.
        let inside_bold = text.find("bold").unwrap() + 1;
        let still_hidden = s.conceal_ranges(Some(inside_bold));
        assert_eq!(still_hidden.len(), 2);
        assert!(still_hidden.iter().all(|&(a, _)| a < bold_start));
    }

    #[test]
    fn fenced_code_block_is_highlighted_and_holds_no_concealed_emphasis() {
        let text = "Intro.\n\n```\nlog *not italic* line\n```\n\nAfter.\n";
        let s = Syntax::new(text);
        let code = all(&s)
            .into_iter()
            .find(|h| h.kind == HighlightKind::Code)
            .expect("code highlight");
        // The whole fence (delimiters + content) is the highlighted span.
        let span = &text[code.start..code.end];
        assert!(
            span.contains("```") && span.contains("log *not italic* line"),
            "{span}"
        );
        // The `*` inside the fence is plain code, so no emphasis delimiter is
        // concealed — the only concealed ranges are the fence lines themselves.
        let hidden = concealed(text, &s, None);
        assert!(hidden.iter().all(|d| d.trim() == "```"), "{hidden:?}");
    }

    #[test]
    fn conceals_whole_fence_lines_and_reveals_them_inside_the_block() {
        // Opening fence carries an info string; both fence rows (with newline) hide.
        let text = "Intro.\n\n```rust\nlet x = 1;\n```\n\nAfter.\n";
        let s = Syntax::new(text);

        // Cursor outside: the two concealed ranges are exactly the fence *lines*,
        // each including its trailing newline (so the whole row disappears, not
        // just the backticks), and the info string is part of the hidden opener.
        let outside = s.conceal_ranges(None);
        assert_eq!(outside.len(), 2, "opening and closing fence lines");
        assert_eq!(&text[outside[0].0..outside[0].1], "```rust\n");
        assert_eq!(&text[outside[1].0..outside[1].1], "```\n");

        // The content line between the fences is never concealed.
        let content = text.find("let x").unwrap();
        assert!(!outside.iter().any(|&(a, b)| a <= content && content < b));

        // Cursor inside the block reveals the raw fences for editing.
        assert!(s.conceal_ranges(Some(content)).is_empty());
        // Cursor on the fence line itself counts as inside (the reveal span is the
        // whole node), so the fences show while editing there too.
        let on_open = outside[0].0;
        assert!(s.conceal_ranges(Some(on_open)).is_empty());
    }

    #[test]
    fn detects_thematic_break_and_conceals_it_reveal_on_cursor() {
        // Every CommonMark thematic-break form gets a `SceneBreak` highlight and a
        // conceal marker over its line, revealed only when the cursor sits on it.
        for marker in ["---", "***", "___", "* * *", "- - -"] {
            let text = format!("Para one.\n\n{marker}\n\nPara two.\n");
            let s = Syntax::new(&text);
            let brk = all(&s)
                .into_iter()
                .find(|h| h.kind == HighlightKind::SceneBreak)
                .unwrap_or_else(|| panic!("scene break for {marker:?}"));
            assert_eq!(text[brk.start..brk.end].trim_end(), marker);

            // Cursor away from the break: the raw markers are concealed (so it can
            // render as a divider). Cursor on the break line: they reveal for editing.
            let away = text.find("Para two").unwrap();
            assert!(
                concealed(&text, &s, Some(away))
                    .iter()
                    .any(|d| d.trim() == marker),
                "the break conceals for {marker:?}"
            );
            let on_break = brk.start;
            assert!(
                !concealed(&text, &s, Some(on_break))
                    .iter()
                    .any(|d| d.trim() == marker),
                "the break reveals under the cursor for {marker:?}"
            );
        }
    }

    #[test]
    fn caches_scene_break_lines_on_parse_and_reparse() {
        // The scene-break line cache (#908) records the buffer line each break sits
        // on, computed once per parse — the renderer reads it instead of scanning
        // every highlight and doing a per-break line lookup each frame.
        let text = "Para one.\n\n---\n\nPara two.\n\n* * *\n\nPara three.\n";
        let mut s = Syntax::new(text);
        // `---` is on line 2, `* * *` on line 6 (both 0-based, newline-counted).
        assert_eq!(s.scene_break_lines(), &[2, 6]);

        // A reparse recomputes the cache off the rope (here the same document, since
        // an incremental reparse needs its edits recorded to reconcile a change), so
        // the cache is repopulated rather than left stale.
        let rope = Rope::from_str(text);
        s.reparse(&rope);
        assert_eq!(s.scene_break_lines(), &[2, 6]);

        // And the cached line matches the old per-break `byte_to_line` derivation
        // exactly, for every break, on the initial parse.
        let s2 = Syntax::new(text);
        let derived: Vec<usize> = s2
            .highlights_in(0, usize::MAX)
            .filter(|h| h.kind == HighlightKind::SceneBreak)
            .map(|h| Rope::from_str(text).byte_to_line(h.start))
            .collect();
        assert_eq!(s2.scene_break_lines(), derived.as_slice());
    }

    #[test]
    fn a_setext_heading_underline_is_not_a_scene_break() {
        // `---` under a title is a setext heading, not a thematic break — driving off
        // the tree-sitter node (not a line regex) keeps them apart.
        let text = "A Title\n---\n\nBody.\n";
        let s = Syntax::new(text);
        assert!(
            all(&s).iter().all(|h| h.kind != HighlightKind::SceneBreak),
            "the setext underline is not read as a scene break"
        );
        assert!(
            all(&s).iter().any(|h| h.kind == HighlightKind::Heading),
            "it is a heading"
        );
    }

    #[test]
    fn fence_concealment_is_independent_per_block() {
        let text = "```\na\n```\n\nmid\n\n```\nb\n```\n";
        let s = Syntax::new(text);
        // Four fence lines total across the two blocks when the cursor is elsewhere.
        assert_eq!(s.conceal_ranges(Some(text.find("mid").unwrap())).len(), 4);
        // Cursor in the first block reveals only its two fences; the second's stay.
        let in_first = text.find("a\n").unwrap();
        assert_eq!(s.conceal_ranges(Some(in_first)).len(), 2);
    }

    #[test]
    fn conceal_list_is_computed_once_per_parse_and_shared() {
        // The full conceal list (nothing revealed) is cached per parse (#869): two
        // calls hand back the *same* allocation, so a frame reuses it rather than
        // rebuilding an O(markers) vec. The cursor-outside-any-span case reuses it
        // too, and the value is byte-identical to the old per-call filter.
        let text = "a *ital* and **bold** z\n";
        let s = Syntax::new(text);
        let full = s.conceal_ranges(None);
        let full_len = full.len();
        assert!(full_len > 0, "the emphasis/strong delimiters are concealed");

        let again = s.conceal_ranges(None);
        assert!(
            Rc::ptr_eq(&full, &again),
            "the full list is cached, not rebuilt"
        );

        // The cursor at document end sits in no span, so nothing is revealed — the
        // same cached list is returned rather than a fresh filtered copy.
        let outside = s.conceal_ranges(Some(text.len()));
        assert!(
            Rc::ptr_eq(&full, &outside),
            "no reveal reuses the cached list"
        );

        // The cursor inside the *ital* span reveals that span's markers, so the
        // result differs — a freshly built (shorter) list, matching the old filter.
        let inside = s.conceal_ranges(Some(text.find("ital").unwrap()));
        assert!(inside.len() < full_len, "the ital markers are revealed");
        assert!(!inside.is_empty(), "the bold markers stay concealed");
        assert!(
            !Rc::ptr_eq(&full, &inside),
            "a reveal builds a filtered list"
        );
    }

    #[test]
    fn reparse_refreshes_the_cached_conceal_list() {
        // A reparse rebuilds the cached full list so it tracks the current markers
        // (#869) — here the emphasis is gone, so the list empties and is shared.
        let mut s = Syntax::new("plain *text* here\n");
        assert!(
            !s.conceal_ranges(None).is_empty(),
            "the emphasis is concealed"
        );
        let rope = Rope::from_str("plain text here\n");
        s.reparse(&rope);
        assert!(
            s.conceal_ranges(None).is_empty(),
            "the removed emphasis leaves nothing to conceal after reparse"
        );
    }

    #[test]
    fn malformed_emphasis_has_no_markers() {
        // A lone `*` and an unclosed `**` are plain text — nothing to conceal,
        // nothing highlighted.
        let text = "a lone * star and **unclosed run\n";
        let s = Syntax::new(text);
        assert!(s.conceal_ranges(None).is_empty());
        assert!(all(&s).iter().all(|h| h.kind == HighlightKind::Heading));
    }

    #[test]
    fn extracts_atx_headings_with_level_and_title() {
        let text = "# Book One\n\nIntro.\n\n## Scene A\n\ntext\n\n### Beat ###\n\nmore\n";
        let s = Syntax::new(text);
        let got: Vec<(u8, &str)> = s
            .headings()
            .iter()
            .map(|h| (h.level, h.title.as_str()))
            .collect();
        assert_eq!(got, vec![(1, "Book One"), (2, "Scene A"), (3, "Beat")]);
    }

    #[test]
    fn extracts_setext_headings() {
        let text = "Title Line\n=====\n\nbody\n\nA Subsection\n---\n";
        let s = Syntax::new(text);
        let got: Vec<(u8, &str)> = s
            .headings()
            .iter()
            .map(|h| (h.level, h.title.as_str()))
            .collect();
        assert_eq!(got, vec![(1, "Title Line"), (2, "A Subsection")]);
    }

    #[test]
    fn headings_are_in_document_order_and_empty_when_none() {
        assert!(Syntax::new("Just prose, no headings.\n")
            .headings()
            .is_empty());

        let text = "## Second-level first\n\n# Top level after\n";
        let s = Syntax::new(text);
        let starts: Vec<usize> = s.headings().iter().map(|h| h.start).collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    }
}

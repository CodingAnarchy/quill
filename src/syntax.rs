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

use tree_sitter::{InputEdit, Node, Tree};
use tree_sitter_md::{MarkdownParser, MarkdownTree};

/// What a highlighted span represents. The renderer maps each to a `Style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Heading,
    Strong,
    Emphasis,
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
    /// Headings in document order — the in-chapter outline (#11).
    headings: Vec<Heading>,
}

impl Syntax {
    /// Parse a document from scratch.
    pub fn new(text: &str) -> Self {
        let mut parser = MarkdownParser::default();
        let tree = parser
            .parse(text.as_bytes(), None)
            .expect("markdown parse should not fail without a timeout/cancellation");
        let (highlights, markers, headings) = collect(&tree, text);
        Self {
            parser,
            tree,
            highlights,
            markers,
            headings,
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
    /// `text` is the current document. Run off the keystroke path (on idle).
    ///
    /// Handing tree-sitter the full text and re-collecting all highlights is
    /// simple and correct; the incremental re-parse and chapter-sized documents
    /// keep it cheap. If whole-manuscript buffers ever make this matter, the
    /// performance epic (#14) can parse only the visible region.
    pub fn reparse(&mut self, text: &str) {
        if let Some(tree) = self.parser.parse(text.as_bytes(), Some(&self.tree)) {
            self.tree = tree;
            let (highlights, markers, headings) = collect(&self.tree, text);
            self.highlights = highlights;
            self.markers = markers;
            self.headings = headings;
        }
    }

    /// Highlights overlapping `[start, end)` in document order.
    pub fn highlights_in(&self, start: usize, end: usize) -> impl Iterator<Item = &Highlight> {
        self.highlights
            .iter()
            .filter(move |h| h.start < end && h.end > start)
    }

    /// Emphasis-delimiter byte ranges to hide when rendering. A delimiter is
    /// concealed unless `reveal_at` (the cursor's byte offset) lies within the
    /// delimiter's enclosing span — so the markers reappear for editing only on
    /// the span the cursor is in. Pass `None` to conceal every delimiter.
    pub fn conceal_ranges(&self, reveal_at: Option<usize>) -> Vec<(usize, usize)> {
        self.markers
            .iter()
            .filter(|m| match reveal_at {
                Some(b) => !(m.span_start <= b && b <= m.span_end),
                None => true,
            })
            .map(|m| (m.delim_start, m.delim_end))
            .collect()
    }

    /// Headings in document order — the in-chapter outline.
    pub fn headings(&self) -> &[Heading] {
        &self.headings
    }

    /// The block-level syntax tree, for future structural consumers.
    #[allow(dead_code)]
    pub fn block_tree(&self) -> &Tree {
        self.tree.block_tree()
    }
}

/// Walk the block tree (headings) and every inline tree (emphasis/strong) into a
/// flat, start-sorted list of highlight spans plus the emphasis delimiters to
/// conceal. Inline trees carry absolute document offsets, so their node ranges
/// line up with the block tree's.
fn collect(tree: &MarkdownTree, text: &str) -> (Vec<Highlight>, Vec<Marker>, Vec<Heading>) {
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
            headings.push(heading_info(&node, text));
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

/// Pull a heading's level and title out of its node. Works on the source text
/// rather than walking marker children, which keeps atx and setext uniform.
fn heading_info(node: &Node, text: &str) -> Heading {
    let raw = &text[node.start_byte()..node.end_byte()];
    if node.kind() == "setext_heading" {
        // "Title\n===" (level 1) or "Title\n---" (level 2).
        let mut lines = raw.lines();
        let title = lines.next().unwrap_or("").trim().to_string();
        let underline = lines.next().unwrap_or("");
        let level = if underline.trim_start().starts_with('=') { 1 } else { 2 };
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
        let title = line[hashes..].trim().trim_end_matches('#').trim().to_string();
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
            .into_iter()
            .map(|(a, b)| text[a..b].to_string())
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
        assert_eq!(
            got,
            vec![(1, "Book One"), (2, "Scene A"), (3, "Beat")]
        );
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
        assert!(Syntax::new("Just prose, no headings.\n").headings().is_empty());

        let text = "## Second-level first\n\n# Top level after\n";
        let s = Syntax::new(text);
        let starts: Vec<usize> = s.headings().iter().map(|h| h.start).collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    }
}

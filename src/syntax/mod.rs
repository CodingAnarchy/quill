//! Markdown parsing, re-run per edit batch by re-parsing only what changed.
//!
//! This is the shared parse the rest of Quill builds on. It uses
//! [`tree_sitter_md`]'s two grammars — a block grammar (headings, lists,
//! paragraphs) and an inline grammar (emphasis, links) — and distills the trees
//! into the flat highlight / conceal / heading / scene-break lists the renderer
//! and outline read.
//!
//! Editing never touches the parse: a keystroke only marks the buffer's syntax
//! dirty, and [`Syntax::reparse`] runs once per input batch (the settled pass in
//! the `main.rs` event loop). A reparse does *not* re-run tree-sitter over the
//! whole document. Tree-sitter's own incremental reparse is no help here: the
//! markdown block grammar's internal nodes are all "fragile" (only leaf tokens
//! are reused, so an incremental block parse costs ~85% of a full one), and the
//! inline grammar re-parses every paragraph's inline tree from scratch. Instead
//! the document is cut into independent blocks at blank lines (see [`segment`]),
//! each block's parse is cached by its exact text, and a reparse runs
//! tree-sitter only on the block(s) whose text changed — one paragraph per
//! keystroke, so the cost no longer scales with chapter size. The result is
//! identical to a fresh whole-document parse (the property tests assert it).

use std::cell::RefCell;
use std::rc::Rc;

use ropey::str_utils::byte_to_line_idx;
use ropey::Rope;
use tree_sitter::{Node, Tree};

use segment::Segments;

mod segment;

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

/// A memoized cursor-revealed conceal list: the enclosing spans it reveals (the
/// key) and the conceal ranges left once their markers are dropped.
type Revealed = (Vec<(usize, usize)>, Rc<[(usize, usize)]>);

/// The live parse of a single document (one chapter file).
pub struct Syntax {
    /// The per-block parse cache that makes a reparse proportional to the edit,
    /// not the document.
    segments: Segments,
    /// Reused copy of the document text for [`reparse`](Self::reparse): the
    /// block cutter and the per-block cache key both read contiguous bytes, so
    /// the rope is flattened into this one buffer (its capacity kept across
    /// reparses, so no per-reparse allocation once warm).
    scratch: String,
    /// Highlight spans for the whole document, sorted by start byte.
    highlights: Vec<Highlight>,
    /// `highlight_reach[i]` is the furthest `end` among `highlights[..=i]` —
    /// non-decreasing even though the spans nest, so [`highlights_in`]
    /// (Self::highlights_in) can binary-search past every span that ends before
    /// its window instead of filtering the whole prefix of the list.
    highlight_reach: Vec<usize>,
    /// Emphasis delimiters, for soft-rendering (concealing) the markers. Sorted by
    /// `delim_start`; the delimiter ranges are pairwise disjoint.
    markers: Vec<Marker>,
    /// `marker_reach[i]` is the furthest `span_end` among `markers[..=i]`, so
    /// [`conceal_ranges`](Self::conceal_ranges) finds the spans enclosing the
    /// cursor by binary search rather than a scan of every marker each frame.
    marker_reach: Vec<usize>,
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
    /// The last cursor-revealed conceal list, keyed by the spans it reveals. A
    /// cursor resting on an emphasis span asks for the same filtered list every
    /// frame; this hands back the shared `Rc` instead of rebuilding it per frame.
    /// Cleared on every parse/reparse (the markers it was built from change).
    revealed: RefCell<Option<Revealed>>,
}

impl Syntax {
    /// Parse a document from scratch.
    pub fn new(text: &str) -> Self {
        let mut syntax = Self {
            segments: Segments::default(),
            scratch: String::new(),
            highlights: Vec::new(),
            highlight_reach: Vec::new(),
            markers: Vec::new(),
            marker_reach: Vec::new(),
            conceal_all: Rc::from([]),
            headings: Vec::new(),
            scene_breaks: Vec::new(),
            revealed: RefCell::new(None),
        };
        syntax.rebuild(text);
        syntax
    }

    /// Re-parse against the current document and refresh every derived list.
    /// Runs once per input batch, off the keystroke itself. Only blocks whose
    /// text changed since the last parse go through tree-sitter; every other
    /// block's cached result is reused (shifted to its new offset), so the cost
    /// tracks the size of the edit rather than the chapter. Needs no record of
    /// the edits: any `rope` is reconciled by content.
    pub fn reparse(&mut self, rope: &Rope) {
        let mut text = std::mem::take(&mut self.scratch);
        text.clear();
        text.reserve(rope.len_bytes());
        for chunk in rope.chunks() {
            text.push_str(chunk);
        }
        self.rebuild(&text);
        self.scratch = text;
    }

    /// Recompute the derived lists for `text`, reusing every cached block parse.
    fn rebuild(&mut self, text: &str) {
        self.highlights.clear();
        self.markers.clear();
        self.headings.clear();
        for (base, parsed) in self.segments.parse(text) {
            let parsed = &parsed.collected;
            self.highlights
                .extend(parsed.highlights.iter().map(|h| Highlight {
                    start: h.start + base,
                    end: h.end + base,
                    kind: h.kind,
                }));
            self.markers.extend(parsed.markers.iter().map(|m| Marker {
                delim_start: m.delim_start + base,
                delim_end: m.delim_end + base,
                span_start: m.span_start + base,
                span_end: m.span_end + base,
            }));
            self.headings
                .extend(parsed.headings.iter().map(|h| Heading {
                    level: h.level,
                    title: h.title.clone(),
                    start: h.start + base,
                }));
        }
        self.highlight_reach = running_max(self.highlights.iter().map(|h| h.end));
        self.marker_reach = running_max(self.markers.iter().map(|m| m.span_end));
        self.conceal_all = conceal_all_from(&self.markers);
        // The renderer narrows the conceal list to each row by binary search on
        // both ends, which needs the delimiter ranges start-sorted and disjoint.
        debug_assert!(
            self.conceal_all.windows(2).all(|w| w[0].1 <= w[1].0),
            "conceal ranges must be start-sorted and disjoint"
        );
        self.revealed.replace(None);
        self.scene_breaks = scene_break_lines_from(&self.highlights, text);
    }

    /// Highlights overlapping `[start, end)` in document order. The list is
    /// start-sorted, so a binary search skips every highlight starting at or
    /// after `end` rather than scanning the whole list per rendered line (#218);
    /// a second one on the running end (`highlight_reach`) skips every earlier
    /// highlight that ends at or before `start`, so the scan covers only the
    /// spans near the window rather than the whole prefix of the document.
    pub fn highlights_in(&self, start: usize, end: usize) -> impl Iterator<Item = &Highlight> {
        let lower = self.highlight_reach.partition_point(|&r| r <= start);
        let upper = self.highlights.partition_point(|h| h.start < end);
        self.highlights
            .get(lower..upper)
            .unwrap_or(&[])
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
    /// on a span (which reveals that span's markers) builds a filtered list, and
    /// that list is memoized so a cursor resting on the span reuses it per frame.
    ///
    /// Finding the enclosing spans is a binary search, not a scan of every marker:
    /// each span's first marker sits at the span's own start (an emphasis node's
    /// opening delimiter, a block's first fence line, a break's own line), so a
    /// span holding `b` has a marker starting at or before `b`; and the running
    /// `marker_reach` skips every earlier marker whose span ends before `b`.
    pub fn conceal_ranges(&self, reveal_at: Option<usize>) -> Rc<[(usize, usize)]> {
        let Some(b) = reveal_at else {
            return Rc::clone(&self.conceal_all);
        };
        let upper = self.markers.partition_point(|m| m.delim_start <= b);
        let lower = self.marker_reach[..upper].partition_point(|&r| r < b);
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for m in &self.markers[lower..upper] {
            let span = (m.span_start, m.span_end);
            if m.span_start <= b && b <= m.span_end && !spans.contains(&span) {
                spans.push(span);
            }
        }
        // Not within any span: the filtered result is byte-for-byte the full
        // list, so reuse the cache.
        if spans.is_empty() {
            return Rc::clone(&self.conceal_all);
        }
        if let Some((key, list)) = self.revealed.borrow().as_ref() {
            if *key == spans {
                return Rc::clone(list);
            }
        }
        let list: Rc<[(usize, usize)]> = self
            .markers
            .iter()
            .filter(|m| !spans.contains(&(m.span_start, m.span_end)))
            .map(|m| (m.delim_start, m.delim_end))
            .collect();
        self.revealed.replace(Some((spans, Rc::clone(&list))));
        list
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

/// The running maximum of `values`: element `i` is the largest of the first
/// `i + 1`. Over a start-sorted list of (possibly nested) spans' ends it is
/// non-decreasing, so a window query can binary-search past every span that ends
/// before the window.
fn running_max(values: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut furthest = 0;
    values
        .map(|v| {
            furthest = furthest.max(v);
            furthest
        })
        .collect()
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
/// come out in document order and the line count is carried forward from one
/// break to the next: one pass over `text`, however many breaks there are. Lines
/// are counted the way the rope counts them (`str_utils::byte_to_line_idx`), so
/// the cache agrees with `Rope::byte_to_line` for any line-ending style.
fn scene_break_lines_from(highlights: &[Highlight], text: &str) -> Vec<usize> {
    let (mut line, mut from) = (0, 0);
    highlights
        .iter()
        .filter(|h| h.kind == HighlightKind::SceneBreak)
        .map(|h| {
            // A break starts a line, so the previous break's start (a line start
            // too) never splits a CRLF pair between two counted slices.
            let to = h.start.min(text.len());
            line += byte_to_line_idx(&text[from..to], to - from);
            from = to;
            line
        })
        .collect()
}

/// Fold every non-ASCII byte to an ASCII letter for the tree-sitter parse view.
///
/// The tree-sitter-markdown C scanner's ordered-list-marker rule calls
/// `isdigit(lexer->lookahead)` on the raw Unicode codepoint (#1164,
/// `scanner.c:740`). `isdigit` is only defined for `unsigned char`/EOF, so a
/// codepoint ≥ 256 (CJK, emoji) indexes out of bounds in glibc's ctype table and
/// segfaults — a real crash on Linux, reachable from opening a file with such
/// text (macOS's libc doesn't fault, which is why it hid).
///
/// Folding is length-preserving (one byte in, one byte out), so every byte offset
/// the parse produces still lands on the same char boundary in the real text, and
/// every ASCII markdown marker (`#`, `*`, `-`, digits, `.`, …) is untouched.
/// Non-ASCII prose carries no markdown syntax, so folding it to a letter can't
/// change the parse structure — only the parse *view* is folded; the rope and all
/// rendered/collected text keep the real bytes.
fn ascii_fold(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .map(|&b| if b < 0x80 { b } else { b'a' })
        .collect()
}

/// The highlights, conceal markers, and headings of one parse, in the parsed
/// text's own byte offsets.
#[derive(Debug, Default, PartialEq, Eq)]
struct Collected {
    highlights: Vec<Highlight>,
    markers: Vec<Marker>,
    headings: Vec<Heading>,
}

/// Walk the block tree (headings, fences, scene breaks) and every inline tree
/// (emphasis/strong) into a flat, start-sorted list of highlight spans plus the
/// emphasis delimiters to conceal. Inline trees carry the same offsets as the
/// block tree (they parse ranges of the same text), so their node ranges line up.
fn collect(block: &Tree, inlines: &[Tree], read: impl Fn(usize, usize) -> String) -> Collected {
    let mut out = Vec::new();
    let mut markers = Vec::new();
    let mut headings = Vec::new();

    walk(block.root_node(), &mut |node| {
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

    for inline in inlines {
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
    Collected {
        highlights: out,
        markers,
        headings,
    }
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
mod tests;

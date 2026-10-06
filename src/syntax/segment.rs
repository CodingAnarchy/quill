//! Cutting a document into independently parseable blocks, and caching each
//! block's parse by its text.
//!
//! Tree-sitter's incremental reparse doesn't pay off for Markdown: the block
//! grammar's internal nodes are all "fragile" (only leaf tokens get reused, so an
//! incremental block parse still shifts and reduces every token), and the inline
//! grammar re-parses each paragraph's inline tree in full. Profiled on a ~6k-word
//! chapter, one typed character cost ~11ms of reparse — about a full parse.
//!
//! Markdown block structure is local, though: after a blank line, a line that
//! starts at column 0 begins a fresh block with no container (paragraph, list,
//! quote, indented code) still open. So the document is cut at such lines, each
//! piece is parsed on its own, and the results are cached by the piece's exact
//! text. Typing changes one piece; every other piece is a cache hit. A piece is
//! only accepted as a unit when its parse closed every block before its end —
//! a leaf block that can span blank lines (an unclosed fenced code block, an HTML
//! block, a list or indented code that swallows the trailing blank line) reaches
//! the piece's end, and the piece is merged with the next one instead. That check
//! is what makes the result identical to parsing the whole text at once.

use std::collections::HashMap;
use std::mem;
use std::rc::Rc;

use tree_sitter::{Node, Parser, Range, Tree};
use tree_sitter_md::{INLINE_LANGUAGE, LANGUAGE};

use super::{ascii_fold, collect, Collected};

/// One piece's parse, in the piece's own byte offsets (0 = the piece's start).
#[derive(Debug)]
pub(super) struct Parsed {
    pub(super) collected: Collected,
    /// Some block other than the document/section wrapper runs to the very end
    /// of the piece — it may continue into whatever follows, so the piece can't
    /// stand alone and is merged with the next one.
    open_end: bool,
    /// When the block left open is a top-level fenced code block: its fence
    /// character and run length, so the cutter can jump straight to the line
    /// that could close it instead of growing the piece one cut at a time.
    open_fence: Option<(u8, usize)>,
}

/// The block and inline parsers plus the per-piece parse cache.
pub(super) struct Segments {
    block: Parser,
    inline: Parser,
    /// Every piece of the last parsed text, keyed by its exact bytes.
    cache: HashMap<Box<[u8]>, Rc<Parsed>>,
    /// How many pieces went through tree-sitter — lets tests assert a keystroke
    /// reparses one block, not the document.
    #[cfg(test)]
    pub(super) fresh_parses: usize,
}

impl Default for Segments {
    fn default() -> Self {
        let mut block = Parser::new();
        let mut inline = Parser::new();
        #[allow(
            clippy::expect_used,
            reason = "the bundled grammars are built against this tree-sitter's ABI"
        )]
        {
            block
                .set_language(&LANGUAGE.into())
                .expect("markdown block grammar loads");
            inline
                .set_language(&INLINE_LANGUAGE.into())
                .expect("markdown inline grammar loads");
        }
        Self {
            block,
            inline,
            cache: HashMap::new(),
            #[cfg(test)]
            fresh_parses: 0,
        }
    }
}

impl Segments {
    /// Parse `text` as the sequence of its pieces, each paired with its start
    /// offset, in document order. Pieces whose text was in the previous parse
    /// come from the cache; only new text goes through tree-sitter. The cache is
    /// replaced by exactly this parse's pieces, so it never outgrows the document.
    pub(super) fn parse(&mut self, text: &str) -> Vec<(usize, Rc<Parsed>)> {
        let cuts = cut_points(text);
        let mut old = mem::take(&mut self.cache);
        let mut out = Vec::new();
        let (mut start, mut next) = (0, 0); // `next`: the first cut after `start`
        loop {
            // Try ending the piece at the next cut; if a block is still open at
            // its end, absorb more cuts — 1, 2, 4, … — so a block spanning many
            // cuts costs O(its length) in total, never one parse per cut. An open
            // fenced code block skips ahead to the first cut past a line that
            // could close it (to the end of the text when none can), so an
            // unclosed fence costs one parse of the rest, not a doubling series.
            let mut step = 1;
            let mut j = next;
            let (end, parsed) = loop {
                let end = cuts.get(j).copied().unwrap_or(text.len());
                let parsed = self.lookup_or_parse(&text[start..end], &mut old);
                if !parsed.open_end || end == text.len() {
                    break (end, parsed);
                }
                let grown = (j + step).min(cuts.len());
                step *= 2;
                j = match parsed.open_fence {
                    Some((ch, len)) => {
                        let close = closing_fence_end(text, end, ch, len);
                        grown.max(cuts.partition_point(|&c| c < close))
                    }
                    None => grown,
                };
            };
            out.push((start, parsed));
            if end == text.len() {
                break;
            }
            start = end;
            next = j + 1;
        }
        out
    }

    /// The parse of one piece: from this parse's cache (a repeated piece), the
    /// previous parse's cache (moved over, key and all — no allocation), or a
    /// fresh tree-sitter parse.
    fn lookup_or_parse(
        &mut self,
        piece: &str,
        old: &mut HashMap<Box<[u8]>, Rc<Parsed>>,
    ) -> Rc<Parsed> {
        let key = piece.as_bytes();
        if let Some(parsed) = self.cache.get(key) {
            return Rc::clone(parsed);
        }
        if let Some((key, parsed)) = old.remove_entry(key) {
            self.cache.insert(key, Rc::clone(&parsed));
            return parsed;
        }
        // Cached even when open-ended: the document's last piece is (its final
        // block runs to the end of the text) and is reused like any other. A
        // trial piece that ends up merged is simply dropped on the next parse.
        let parsed = Rc::new(self.parse_piece(piece));
        self.cache.insert(key.into(), Rc::clone(&parsed));
        parsed
    }

    /// Run tree-sitter over one piece: the block grammar, then the inline grammar
    /// over each inline node's ranges, exactly as `tree_sitter_md::MarkdownParser`
    /// does for a fresh parse.
    fn parse_piece(&mut self, piece: &str) -> Parsed {
        #[cfg(test)]
        {
            self.fresh_parses += 1;
        }
        let folded = ascii_fold(piece.as_bytes());
        #[allow(
            clippy::expect_used,
            reason = "tree-sitter `parse` returns `None` only on a timeout or \
                      cancellation, neither of which this parser configures"
        )]
        let block = self
            .block
            .parse(&folded, None)
            .expect("markdown parse should not fail without a timeout/cancellation");
        let mut inlines = Vec::new();
        for ranges in inline_ranges(&block) {
            if self.inline.set_included_ranges(&ranges).is_ok() {
                inlines.extend(self.inline.parse(&folded, None));
            }
        }
        let open_end = reaches_end(block.root_node(), piece.len());
        let open_fence = if open_end {
            open_fence(block.root_node(), piece)
        } else {
            None
        };
        Parsed {
            collected: collect(&block, &inlines, |s, e| piece[s..e].to_string()),
            open_end,
            open_fence,
        }
    }
}

/// The byte offsets where `text` may be cut into independently parsed pieces:
/// the start of a line that follows an empty line and begins with a character
/// that can't continue an indented container (no leading space/tab) and can't
/// open front matter (`---` / `+++` are recognized as metadata only at the start
/// of the text being parsed, so a piece never starts with `-` or `+`). Whether a
/// cut actually holds — no block still open across it — is checked after the
/// piece is parsed ([`Parsed::open_end`]).
fn cut_points(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut cuts = Vec::new();
    for p in 2..bytes.len() {
        if bytes[p - 1] == b'\n'
            && bytes[p - 2] == b'\n'
            && !matches!(bytes[p], b' ' | b'\t' | b'\n' | b'\r' | b'-' | b'+')
        {
            cuts.push(p);
        }
    }
    cuts
}

/// Whether some node other than the `document`/`section` wrappers runs to `len`
/// (the piece's end) — i.e. might continue past a cut there. Checks every node,
/// not just the last block, so a trailing zero-width node can't mask an open one.
fn reaches_end(node: Node, len: usize) -> bool {
    let mut cursor = node.walk();
    let reaches = node.children(&mut cursor).any(|child| {
        child.end_byte() >= len
            && (!matches!(child.kind(), "document" | "section") || reaches_end(child, len))
    });
    reaches
}

/// The fence of a top-level fenced code block left open at the piece's end —
/// its character (`` ` `` or `~`) and run length — or `None` when the open block
/// is anything else.
fn open_fence(node: Node, piece: &str) -> Option<(u8, usize)> {
    let mut cursor = node.walk();
    let last = node.children(&mut cursor).last()?;
    match last.kind() {
        "section" => open_fence(last, piece),
        "fenced_code_block" => {
            let delim = piece.as_bytes().get(last.start_byte()..)?;
            let delim = &delim[delim.iter().take_while(|&&b| b == b' ').count()..];
            let ch = *delim.first()?;
            let len = delim.iter().take_while(|&&b| b == ch).count();
            matches!(ch, b'`' | b'~').then_some((ch, len))
        }
        _ => None,
    }
}

/// The end of the first line at or after `from` that could close a fence of
/// `len` × `ch` (up to three spaces, at least `len` fence characters, then only
/// whitespace), or `text.len()` when no line can. Only a hint for where to cut:
/// the piece it yields is still checked like any other.
fn closing_fence_end(text: &str, from: usize, ch: u8, len: usize) -> usize {
    let mut pos = from;
    for line in text[from..].split_inclusive('\n') {
        pos += line.len();
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|&&b| b == b' ').count();
        let run = bytes[indent..].iter().take_while(|&&b| b == ch).count();
        if indent <= 3 && run >= len && bytes[indent + run..].iter().all(u8::is_ascii_whitespace) {
            return pos;
        }
    }
    text.len()
}

/// The included ranges of every inline node's inline parse, in document order —
/// the same walk and range construction as `MarkdownParser::parse_with_options`:
/// an `inline` (or `pipe_table_cell`) node's range minus its named children after
/// the first (block continuation markers inside a lazy paragraph, etc.).
fn inline_ranges(block: &Tree) -> Vec<Vec<Range>> {
    let mut out = Vec::new();
    let mut cursor = block.walk();
    'outer: loop {
        let node = loop {
            let kind = cursor.node().kind();
            if kind == "inline" || kind == "pipe_table_cell" || !cursor.goto_first_child() {
                while !cursor.goto_next_sibling() {
                    if !cursor.goto_parent() {
                        break 'outer;
                    }
                }
            }
            let kind = cursor.node().kind();
            if kind == "inline" || kind == "pipe_table_cell" {
                break cursor.node();
            }
        };
        let mut range = node.range();
        let mut ranges = Vec::new();
        if cursor.goto_first_child() {
            while cursor.goto_next_sibling() {
                if !cursor.node().is_named() {
                    continue;
                }
                let child = cursor.node().range();
                ranges.push(Range {
                    start_byte: range.start_byte,
                    start_point: range.start_point,
                    end_byte: child.start_byte,
                    end_point: child.start_point,
                });
                range.start_byte = child.end_byte;
                range.start_point = child.end_point;
            }
            cursor.goto_parent();
        }
        ranges.push(range);
        out.push(ranges);
    }
    out
}

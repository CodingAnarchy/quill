//! The text buffer: a rope-backed document with a single cursor.
//!
//! A novel is mostly long runs of prose with occasional edits in the middle, so
//! a rope (`ropey`) gives us cheap inserts/deletes at any offset without copying
//! the whole document around.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use ropey::Rope;
use tree_sitter::{InputEdit, Point};
use unicode_segmentation::UnicodeSegmentation;

use crate::atomic;
use crate::frontmatter::FrontMatter;
use crate::syntax::Syntax;

mod history;
mod motions;
use history::Snapshot;
pub use motions::{FindKind, TextObject};

/// Initial window size (bytes) [`Buffer::grapheme_after`](Buffer::grapheme_after)
/// scans forward from the offset before falling back to a wider window (#693). A
/// single grapheme is a handful of bytes, so this covers it in one pass without
/// copying the whole rope; the doubling retry keeps a pathological grapheme that
/// straddles the window edge correct.
const GRAPHEME_WINDOW: usize = 64;

/// A cursor position expressed in (line, grapheme-column).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
}

/// An in-memory document plus the path it was loaded from / will save to.
pub struct Buffer {
    rope: Rope,
    pub path: Option<PathBuf>,
    /// The chapter file's YAML front matter, kept off the rope so the editor
    /// shows only the prose and re-attached on save. `None` when the file has no
    /// front matter block (one is created lazily when a field is first set).
    front_matter: Option<FrontMatter>,
    pub cursor: Cursor,
    /// Whether there are unsaved changes.
    pub dirty: bool,
    /// Remembered display column (cells) for vertical movement, so moving up/down
    /// through short/wrapped rows doesn't permanently collapse the cursor toward
    /// column 0. In display cells because movement is by *visual* row (#143).
    goal_x: Option<usize>,
    /// Live incremental Markdown parse. Edits are *recorded* into it on the
    /// keystroke path; the actual reparse runs via [`Buffer::refresh_syntax`]
    /// once typing goes idle, so a keystroke never blocks on a parse.
    syntax: Syntax,
    /// Set when an edit has been recorded but not yet reparsed.
    syntax_dirty: bool,
    /// Undo / redo history (#145), as whole-document snapshots. A group spans a
    /// burst of edits between idle ticks, so one undo reverses a meaningful chunk
    /// (a word or phrase), not a single keystroke.
    undo_stack: VecDeque<Snapshot>,
    redo_stack: VecDeque<Snapshot>,
    /// True while an edit group is open: the pre-edit state has been snapshotted
    /// and subsequent edits fold into the same undo entry until the group is
    /// sealed on idle ([`Buffer::seal_undo_group`]).
    group_open: bool,
    /// Cached word count (#151), so the status bar / outline don't re-segment the
    /// whole document every frame. Invalidated (`None`) on edit and recomputed
    /// lazily on the next read; `Cell` so the cheap [`word_count`] reader stays
    /// `&self`.
    word_count_cache: Cell<Option<usize>>,
    /// Byte-range edits since the last drain (#302): each is `(start, old_end,
    /// new_end)` — bytes `[start, old_end)` were replaced by `new_end - start`
    /// new bytes. Recorded on the keystroke path, drained on idle so author
    /// annotations can re-anchor without re-scanning the whole buffer.
    anchor_edits: Vec<(usize, usize, usize)>,
    /// Set when a new undo group opens (#370), so the app can snapshot author
    /// annotations in lockstep with the undo stack — undo/redo swaps the whole
    /// rope without an edit delta, so annotations are restored, not re-mapped.
    /// Read-and-cleared via [`take_checkpointed`](Buffer::take_checkpointed).
    checkpointed: bool,
}

impl Buffer {
    /// An empty, untitled buffer. Test-only today; #24 (a freshly created chapter
    /// before its first save) will reuse it — un-gate it then.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self::from_text("")
    }

    /// An in-memory buffer over `text`, with no backing path. Used for the
    /// continuous-manuscript view, whose buffer spans several chapter files and
    /// is saved by splitting it back out (see [`crate::manuscript`]), so it has
    /// no single path or front matter of its own. The one real constructor: the
    /// others ([`empty`](Self::empty), [`from_path`](Self::from_path)) build their
    /// body and delegate here, so a new field is initialized in one place.
    pub fn from_text(text: &str) -> Self {
        Self {
            syntax: Syntax::new(text),
            rope: Rope::from_str(text),
            path: None,
            front_matter: None,
            cursor: Cursor::default(),
            dirty: false,
            goal_x: None,
            syntax_dirty: false,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            group_open: false,
            word_count_cache: Cell::new(None),
            anchor_edits: Vec::new(),
            checkpointed: false,
        }
    }

    /// Load a file from disk, or start an empty buffer pointed at `path` if the
    /// file does not exist yet. The path is normalized to Quill's manuscript
    /// format: a path with no extension gets `.md` (see [`with_default_extension`]).
    ///
    /// Any YAML front matter is split off and kept aside ([`front_matter`]); the
    /// rope and the parse hold only the prose body, so the editor never shows the
    /// raw block.
    pub fn from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = with_default_extension(path.as_ref());
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        let (front, body) = crate::frontmatter::split(&text);
        let mut buffer = Self::from_text(body);
        buffer.path = Some(path);
        buffer.front_matter = front.map(FrontMatter::from_raw);
        Ok(buffer)
    }

    /// A buffer over file contents that came from somewhere other than the file at
    /// `path` — a past commit's version of a chapter (#587). Splits off front matter
    /// and points the buffer at `path` like [`from_path`](Self::from_path), but the
    /// text is supplied rather than read, and the buffer is marked dirty: the
    /// recovered version lands in the editor for review, and saving writes it out.
    pub fn from_file_text(path: impl AsRef<Path>, text: &str) -> Self {
        let (front, body) = crate::frontmatter::split(text);
        let mut buffer = Self::from_text(body);
        buffer.path = Some(with_default_extension(path.as_ref()));
        buffer.front_matter = front.map(FrontMatter::from_raw);
        buffer.dirty = true;
        buffer
    }

    /// The chapter's front matter, if any.
    pub fn front_matter(&self) -> Option<&FrontMatter> {
        self.front_matter.as_ref()
    }

    /// The chapter's front matter for editing, creating an empty block the
    /// first time a field is set. The caller marks the buffer dirty so the
    /// change persists on the next save.
    pub fn front_matter_mut(&mut self) -> &mut FrontMatter {
        self.front_matter.get_or_insert_with(FrontMatter::default)
    }

    /// Persist the buffer to its path. Returns an error if there is no path.
    ///
    /// The manuscript is written as UTF-8 Markdown, unchanged except for one
    /// documented normalization: a non-empty body ends with exactly one
    /// trailing newline (the POSIX/git convention), and an empty body writes no
    /// body. Interior bytes — including any line endings — are left exactly as
    /// the author wrote them, so a conforming file round-trips byte-for-byte.
    /// Any front matter the file had is re-attached above the body untouched.
    pub fn save(&mut self) -> std::io::Result<()> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| std::io::Error::other("no file path set"))?;

        let body = normalize_trailing_newline(&self.rope.to_string());
        let front = self
            .front_matter
            .as_ref()
            .filter(|f| !f.is_empty())
            .map(FrontMatter::as_str);
        let content = crate::frontmatter::join(front, &body);
        atomic::write(path, content.as_bytes())?;
        self.dirty = false;
        Ok(())
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines().max(1)
    }

    /// The whole buffer as a string (used to commit a scratch metadata edit).
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Replace the entire prose body with `text`, marking the buffer dirty and
    /// resetting the cursor to the start. The path and front matter are left as
    /// they are. Used by crash recovery (#263) to load a previous session's
    /// unsaved work back into the active chapter; the undo history starts fresh
    /// from the recovered text.
    pub fn replace_text(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.syntax = Syntax::new(text);
        self.syntax_dirty = false;
        self.cursor = Cursor::default();
        self.goal_x = None;
        self.word_count_cache.set(None);
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.group_open = false;
        self.checkpointed = false; // fresh baseline — no annotation snapshot to take (#370)
        self.dirty = true;
    }

    /// The text of a single line, without its trailing newline.
    pub fn line(&self, idx: usize) -> String {
        if idx >= self.rope.len_lines() {
            return String::new();
        }
        let mut s = self.rope.line(idx).to_string();
        s.truncate(s.trim_end_matches('\n').len());
        s
    }

    /// The exact text of the line range `[start, end)`, including the lines'
    /// trailing newlines — the rope slice verbatim. Used to split the continuous
    /// view's buffer back into per-chapter files.
    pub fn line_range_text(&self, start: usize, end: usize) -> String {
        let last = self.rope.len_lines();
        let start = start.min(last);
        let end = end.min(last);
        if start >= end {
            return String::new();
        }
        let from = self.rope.line_to_char(start);
        let to = self.rope.line_to_char(end);
        self.rope.slice(from..to).to_string()
    }

    /// Number of grapheme clusters on a line (how far the cursor can travel).
    fn line_graphemes(&self, idx: usize) -> usize {
        byte_to_col(&self.line(idx), usize::MAX)
    }

    /// The [`Cursor`] (line, grapheme-column) for an absolute byte offset — the
    /// inverse of [`cursor_byte`], used to jump to a search match (#144).
    pub fn cursor_at_byte(&self, byte: usize) -> Cursor {
        let byte = byte.min(self.rope.len_bytes());
        let line = self.rope.byte_to_line(byte);
        let line_start_byte = self.rope.line_to_byte(line);
        let line_text = self.line(line);
        let within = (byte - line_start_byte).min(line_text.len());
        Cursor {
            line,
            col: byte_to_col(&line_text, within),
        }
    }

    /// Replace the byte range `[start, end)` with `text`, then put the cursor just
    /// after the inserted text (#144 find-and-replace). Goes through the same edit
    /// path as typing, so it is captured by undo and the incremental parse.
    pub fn replace_byte_range(&mut self, start: usize, end: usize, text: &str) {
        let start_char = self.rope.byte_to_char(start);
        let end_char = self.rope.byte_to_char(end);
        self.edit_remove(start_char, end_char);
        self.edit_insert(start_char, text);
        self.cursor = self.cursor_at_byte(start + text.len());
        self.goal_x = None;
        self.dirty = true;
    }

    /// Convert the cursor's (line, col) into an absolute char offset in the rope.
    fn cursor_char_idx(&self) -> usize {
        let line_start = self.rope.line_to_char(
            self.cursor
                .line
                .min(self.rope.len_lines().saturating_sub(1)),
        );
        let line = self.line(self.cursor.line);
        let byte = col_to_byte(&line, self.cursor.col);
        // Translate the byte offset within the line to a char offset.
        line_start + line[..byte].chars().count()
    }

    // --- Editing -----------------------------------------------------------

    /// Insert text at a char index and tell the parser exactly what changed, so
    /// the reparse can be incremental. Does not touch the cursor.
    fn edit_insert(&mut self, char_idx: usize, text: &str) {
        self.checkpoint();
        self.word_count_cache.set(None); // the text changed; recount on next read
        let start_byte = self.rope.char_to_byte(char_idx);
        let start_position = point_at_byte(&self.rope, start_byte);

        self.rope.insert(char_idx, text);

        let new_end_byte = start_byte + text.len();
        let edit = InputEdit {
            start_byte,
            old_end_byte: start_byte,
            new_end_byte,
            start_position,
            old_end_position: start_position,
            new_end_position: point_at_byte(&self.rope, new_end_byte),
        };
        self.syntax.record_edit(&edit);
        self.anchor_edits
            .push((start_byte, start_byte, new_end_byte));
        self.syntax_dirty = true;
    }

    /// Remove the char range `[start, end)` and tell the parser what changed.
    /// Does not touch the cursor.
    fn edit_remove(&mut self, start: usize, end: usize) {
        self.checkpoint();
        self.word_count_cache.set(None); // the text changed; recount on next read
        let start_byte = self.rope.char_to_byte(start);
        let old_end_byte = self.rope.char_to_byte(end);
        let start_position = point_at_byte(&self.rope, start_byte);
        let old_end_position = point_at_byte(&self.rope, old_end_byte);

        self.rope.remove(start..end);

        let edit = InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte,
            start_position,
            old_end_position,
            new_end_position: point_at_byte(&self.rope, start_byte),
        };
        self.syntax.record_edit(&edit);
        self.anchor_edits
            .push((start_byte, old_end_byte, start_byte));
        self.syntax_dirty = true;
    }

    /// Take the byte-range edits recorded since the last call (#302), to re-anchor
    /// author annotations off the keystroke path. Each is `(start, old_end,
    /// new_end)`: bytes `[start, old_end)` became `new_end - start` new bytes.
    pub fn drain_anchor_edits(&mut self) -> Vec<(usize, usize, usize)> {
        std::mem::take(&mut self.anchor_edits)
    }

    /// Reparse the document if it has changed since the last parse. Cheap to
    /// call when nothing is dirty. Run this off the keystroke path — once input
    /// goes idle — so typing never blocks on a parse.
    pub fn refresh_syntax(&mut self) {
        if self.syntax_dirty {
            // Reparse straight from the rope — no full-document copy (#218).
            self.syntax.reparse(&self.rope);
            self.syntax_dirty = false;
        }
    }

    /// Up to `max` characters immediately before the cursor, for context-sensitive
    /// input transforms like smart quotes (#296). O(max) — independent of line and
    /// document size, so it stays off the per-keystroke cost ceiling.
    pub fn text_before_cursor(&self, max: usize) -> String {
        let end = self.cursor_char_idx();
        let start = end.saturating_sub(max);
        self.rope.slice(start..end).to_string()
    }

    pub fn insert_char(&mut self, c: char) {
        let idx = self.cursor_char_idx();
        let mut buf = [0u8; 4];
        self.edit_insert(idx, c.encode_utf8(&mut buf));
        self.dirty = true;
        if c == '\n' {
            self.cursor.line += 1;
            self.cursor.col = 0;
        } else {
            self.cursor.col += 1;
        }
        self.goal_x = None;
    }

    /// Delete the grapheme before the cursor (Backspace).
    pub fn backspace(&mut self) {
        if self.cursor.col > 0 {
            let line = self.line(self.cursor.line);
            let target = self.cursor.col - 1;
            let (start_b, g) = line
                .grapheme_indices(true)
                .nth(target)
                .map(|(b, g)| (b, g.to_string()))
                .unwrap_or((line.len(), String::new()));
            let line_start = self.rope.line_to_char(self.cursor.line);
            let start_c = line_start + line[..start_b].chars().count();
            let end_c = start_c + g.chars().count();
            self.edit_remove(start_c, end_c);
            self.cursor.col = target;
            self.dirty = true;
        } else if self.cursor.line > 0 {
            // Join with the previous line.
            let prev_len = self.line_graphemes(self.cursor.line - 1);
            let idx = self.rope.line_to_char(self.cursor.line);
            self.edit_remove(idx - 1, idx); // remove the newline
            self.cursor.line -= 1;
            self.cursor.col = prev_len;
            self.dirty = true;
        }
        self.goal_x = None;
    }

    /// Delete the grapheme *at* the cursor (forward Delete). At the end of a line
    /// it removes the newline, pulling the next line up; the cursor stays put.
    /// Returns whether a line join happened (a newline was removed) — the
    /// continuous view uses it to keep its per-chapter line counts in step.
    pub fn delete_forward(&mut self) -> bool {
        let len = self.line_graphemes(self.cursor.line);
        if self.cursor.col < len {
            let line = self.line(self.cursor.line);
            let (start_b, g) = line
                .grapheme_indices(true)
                .nth(self.cursor.col)
                .map(|(b, g)| (b, g.to_string()))
                .unwrap_or((line.len(), String::new()));
            let line_start = self.rope.line_to_char(self.cursor.line);
            let start_c = line_start + line[..start_b].chars().count();
            let end_c = start_c + g.chars().count();
            self.edit_remove(start_c, end_c);
            self.dirty = true;
            false
        } else if self.cursor.line + 1 < self.line_count() {
            // At the line end: remove the newline, joining the next line up. The
            // cursor already sits at the join point, so it needs no adjustment.
            let idx = self.rope.line_to_char(self.cursor.line + 1);
            self.edit_remove(idx - 1, idx);
            self.dirty = true;
            true
        } else {
            false
        }
    }

    /// The cursor position the previous-word motion/delete lands on — the start
    /// of the word before the cursor. Moves the cursor there and back so it's a
    /// pure query; the continuous view uses it to clamp a word-delete to a chapter
    /// seam before removing anything.
    pub fn word_back_target(&mut self) -> Cursor {
        let saved = self.cursor;
        let saved_goal = self.goal_x;
        self.move_word_backward();
        let target = self.cursor;
        self.cursor = saved;
        self.goal_x = saved_goal;
        target
    }

    /// Delete from the start of the previous word to the cursor (Ctrl/Alt-
    /// Backspace). Uses the same word classification as the Vim word motions, so
    /// it stops at the same boundaries `b` would. A no-op at the very start of the
    /// document. Returns the number of newlines removed, so the continuous view
    /// can adjust its per-chapter line counts.
    pub fn delete_word_back(&mut self) -> usize {
        let end = self.cursor_char_idx();
        if end == 0 {
            return 0;
        }
        self.move_word_backward();
        let start = self.cursor_char_idx();
        if start >= end {
            // `b` didn't move off the cursor; fall back to a single Backspace so
            // the key always makes progress.
            self.cursor = self.cursor_at_byte(self.rope.char_to_byte(end));
            self.backspace();
            return 0;
        }
        let newlines = self
            .rope
            .slice(start..end)
            .chars()
            .filter(|&c| c == '\n')
            .count();
        self.edit_remove(start, end); // cursor is already at `start` from `move_word_backward`
        self.dirty = true;
        self.goal_x = None;
        newlines
    }

    // --- Movement ----------------------------------------------------------

    pub fn move_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line_graphemes(self.cursor.line);
        }
        self.goal_x = None;
    }

    pub fn move_right(&mut self) {
        let len = self.line_graphemes(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.line_count() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
        self.goal_x = None;
    }

    /// Move up one *visual* row at `width` (#143): to the previous wrapped row of
    /// the same logical line, or the last row of the line above, keeping the
    /// remembered display column.
    pub fn move_up(&mut self, width: usize) {
        self.move_vertical(-1, width);
    }

    /// Move down one *visual* row at `width` (#143).
    pub fn move_down(&mut self, width: usize) {
        self.move_vertical(1, width);
    }

    /// The cursor's current visual layout on its own line: the line text, its
    /// wrap row starts, the visual row the cursor is on, and its display column.
    fn cursor_visual_row(&self, width: usize) -> (String, Vec<usize>, usize, usize) {
        let line_text = self.line(self.cursor.line);
        let starts = crate::wrap::row_starts(&line_text, width);
        let cur_byte = col_to_byte(&line_text, self.cursor.col);
        let (row, x) = crate::wrap::visual_pos(&line_text, &starts, cur_byte);
        (line_text, starts, row, x)
    }

    fn move_vertical(&mut self, dir: i32, width: usize) {
        let (line_text, starts, row, x) = self.cursor_visual_row(width);
        let goal = *self.goal_x.get_or_insert(x);

        if dir < 0 {
            if row > 0 {
                let b = crate::wrap::byte_at_visual(&line_text, &starts, row - 1, goal);
                self.cursor.col = byte_to_col(&line_text, b);
            } else if self.cursor.line > 0 {
                self.cursor.line -= 1;
                let prev = self.line(self.cursor.line);
                let pstarts = crate::wrap::row_starts(&prev, width);
                let b = crate::wrap::byte_at_visual(&prev, &pstarts, pstarts.len() - 1, goal);
                self.cursor.col = byte_to_col(&prev, b);
            } else {
                self.cursor.col = 0; // already on the first row of the document
            }
        } else if row + 1 < starts.len() {
            let b = crate::wrap::byte_at_visual(&line_text, &starts, row + 1, goal);
            self.cursor.col = byte_to_col(&line_text, b);
        } else if self.cursor.line + 1 < self.line_count() {
            self.cursor.line += 1;
            let next = self.line(self.cursor.line);
            let nstarts = crate::wrap::row_starts(&next, width);
            let b = crate::wrap::byte_at_visual(&next, &nstarts, 0, goal);
            self.cursor.col = byte_to_col(&next, b);
        } else {
            self.cursor.col = self.line_graphemes(self.cursor.line); // last row of the document
        }
    }

    /// Grapheme length of the current logical line — how far the cursor can
    /// travel on it.
    pub fn line_len(&self) -> usize {
        self.line_graphemes(self.cursor.line)
    }

    /// Move to the start of the current *logical* line (column 0) — the anchor
    /// for opening a line above (#27).
    pub fn move_line_start(&mut self) {
        self.cursor.col = 0;
        self.goal_x = None;
    }

    /// Move to the end of the current *logical* line, past its last grapheme —
    /// the anchor for opening a line below (#27). Unlike [`move_end`](Self::move_end)
    /// this spans the whole paragraph, not just the cursor's wrapped row.
    pub fn move_line_end(&mut self) {
        self.cursor.col = self.line_graphemes(self.cursor.line);
        self.goal_x = None;
    }

    /// Move to the start of the current visual row (#143).
    pub fn move_home(&mut self, width: usize) {
        let (line_text, starts, row, _) = self.cursor_visual_row(width);
        self.cursor.col = byte_to_col(&line_text, starts[row]);
        self.goal_x = None;
    }

    /// Move to the end of the current visual row (#143). At a soft-wrap boundary
    /// the cursor sits just before the trailing space, so it reads as the row's
    /// end rather than the next row's start.
    pub fn move_end(&mut self, width: usize) {
        let (line_text, starts, row, _) = self.cursor_visual_row(width);
        let mut end = starts.get(row + 1).copied().unwrap_or(line_text.len());
        if row + 1 < starts.len() {
            if let Some((b, " ")) = line_text[..end].grapheme_indices(true).next_back() {
                end = b; // drop the trailing soft-wrap space
            }
        }
        self.cursor.col = byte_to_col(&line_text, end);
        self.goal_x = None;
    }

    // --- Vim motions (#202) ------------------------------------------------

    /// Move to the first non-blank grapheme of the current logical line — Vim's
    /// `^`. Falls back to the line start for an all-blank line.
    pub fn move_first_non_blank(&mut self) {
        let line = self.line(self.cursor.line);
        let blank = line
            .graphemes(true)
            .take_while(|g| g.chars().all(char::is_whitespace))
            .count();
        self.cursor.col = blank.min(self.line_graphemes(self.cursor.line));
        self.goal_x = None;
    }

    /// Jump to logical line `line` (0-based, clamped), at its first non-blank —
    /// Vim's `gg` / `{count}G`.
    pub fn move_to_line(&mut self, line: usize) {
        self.cursor.line = line.min(self.line_count().saturating_sub(1));
        self.move_first_non_blank();
    }

    /// The line index Vim's `G` targets — the last line with content (skipping a
    /// trailing newline's empty line).
    pub fn last_line_index(&self) -> usize {
        let n = self.line_count();
        if n > 1 && self.line(n - 1).is_empty() {
            n - 2
        } else {
            n - 1
        }
    }

    // --- Operator support (#203) -------------------------------------------

    /// Total byte length of the document.
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// The byte just past the grapheme starting at `byte` — so an inclusive
    /// motion or `x` can cover the grapheme the cursor is on. Clamped to the end.
    ///
    /// Scans a bounded window starting at `byte` rather than copying the whole
    /// rope (#693): a single grapheme is short, so [`GRAPHEME_WINDOW`] almost
    /// always covers it in one pass. If the window ends mid-grapheme with the
    /// document still beyond it, widen and retry so the boundary is byte-identical
    /// to a full-document scan even for a grapheme straddling the window edge.
    pub fn grapheme_after(&self, byte: usize) -> usize {
        let len = self.rope.len_bytes();
        let byte = byte.min(len);
        let mut radius = GRAPHEME_WINDOW;
        loop {
            let hi = (byte + radius).min(len);
            let hi = self.rope.char_to_byte(self.rope.byte_to_char(hi));
            let text = self.rope.byte_slice(byte..hi).to_string();
            match text.grapheme_indices(true).nth(1) {
                Some((b, _)) => return byte + b,
                // No boundary within the window. If it reaches the document end,
                // that end *is* the boundary; otherwise widen and rescan.
                None if hi >= len => return len,
                None => radius = radius.saturating_mul(2),
            }
        }
    }

    /// The byte where the line after `line` begins — i.e. just past `line`'s
    /// newline — or the buffer end if `line` is the last. The boundary linewise
    /// deletes and puts work against.
    pub fn next_line_byte(&self, line: usize) -> usize {
        if line + 1 < self.line_count() {
            self.line_start_byte(line + 1)
        } else {
            self.len_bytes()
        }
    }

    /// The text in byte range `[start, end)` — what an operator records to the
    /// register before deleting it.
    pub fn byte_slice(&self, start: usize, end: usize) -> String {
        let len = self.rope.len_bytes();
        let (start, end) = (start.min(len), end.min(len));
        if start >= end {
            return String::new();
        }
        self.rope.byte_slice(start..end).to_string()
    }

    // --- Stats -------------------------------------------------------------

    /// Total word count across the document.
    pub fn word_count(&self) -> usize {
        if let Some(n) = self.word_count_cache.get() {
            return n;
        }
        // Count over the whole text, not per rope chunk: a word straddling a
        // chunk boundary would otherwise be counted twice. Cached until the next
        // edit (#151) so the status bar / outline don't re-segment every frame.
        let n = count_words(&self.rope.to_string());
        self.word_count_cache.set(Some(n));
        n
    }

    // --- Syntax ------------------------------------------------------------

    /// The live Markdown parse, for highlighting and (later) the outline.
    pub fn syntax(&self) -> &Syntax {
        &self.syntax
    }

    /// Absolute byte offset where line `idx` begins — lets the renderer map a
    /// line's characters to the document offsets the syntax highlights use.
    pub fn line_start_byte(&self, idx: usize) -> usize {
        let idx = idx.min(self.rope.len_lines().saturating_sub(1));
        self.rope.line_to_byte(idx)
    }

    /// Absolute byte offset of the cursor — used to decide which emphasis span,
    /// if any, should reveal its markers for editing.
    pub fn cursor_byte(&self) -> usize {
        self.rope.char_to_byte(self.cursor_char_idx())
    }
}

/// A tree-sitter [`Point`] (row, byte-column-within-line) for a byte offset.
/// Byte offset of grapheme column `col` within a single line's `text` (the line
/// end if `col` is past the last grapheme).
fn col_to_byte(text: &str, col: usize) -> usize {
    text.grapheme_indices(true)
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// Grapheme column of byte offset `byte` within a single line's `text`.
fn byte_to_col(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].graphemes(true).count()
}

fn point_at_byte(rope: &Rope, byte: usize) -> Point {
    let row = rope.byte_to_line(byte);
    let column = byte - rope.line_to_byte(row);
    Point { row, column }
}

/// Count the words in a string (Unicode word segmentation). Shared by the live
/// buffer count and the per-chapter file counts in the navigation pane.
pub fn count_words(text: &str) -> usize {
    text.unicode_words().count()
}

/// Quill's on-disk newline convention: a non-empty file ends with exactly one
/// trailing newline (POSIX/git), and empty content writes an empty file.
/// Interior bytes are left untouched. Shared by [`Buffer::save`] and the
/// continuous view's per-chapter split so both write files the same way.
pub fn normalize_trailing_newline(text: &str) -> String {
    let mut content = text.to_string();
    content.truncate(content.trim_end_matches('\n').len());
    if !content.is_empty() {
        content.push('\n');
    }
    content
}

/// Quill's manuscript format is Markdown. A path with no extension gets `.md`;
/// an explicit extension (`.md`, `.txt`, anything) is respected so an author
/// who names a file deliberately keeps that name.
fn with_default_extension(path: &Path) -> PathBuf {
    if path.extension().is_none() {
        path.with_extension("md")
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests;

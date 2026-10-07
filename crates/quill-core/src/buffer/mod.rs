//! The text buffer: a rope-backed document with a single cursor.
//!
//! A novel is mostly long runs of prose with occasional edits in the middle, so
//! a rope (`ropey`) gives us cheap inserts/deletes at any offset without copying
//! the whole document around.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use crate::atomic;
use crate::frontmatter::FrontMatter;
use crate::syntax::Syntax;
use crate::text;

mod history;
mod motions;
use history::Snapshot;
pub use motions::CharClass;
pub use motions::{CaseChange, CaseEdit, FindKind, TextObject};

/// Initial window size (bytes) [`Buffer::grapheme_after`](Buffer::grapheme_after)
/// scans forward from the offset before falling back to a wider window (#693). A
/// single grapheme is a handful of bytes, so this covers it in one pass without
/// copying the whole rope; the doubling retry keeps a pathological grapheme that
/// straddles the window edge correct.
const GRAPHEME_WINDOW: usize = 64;

/// The source of [`Buffer::revision`] values: process-wide, so a revision names
/// one content state of one buffer and never repeats — not even across a buffer
/// that is dropped and rebuilt (a chapter switch, a continuous-view reslide).
/// Starts at 1 so [`NO_REVISION`] (0) is never handed out.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

/// Sentinel for "no revision": the anchor base after a wholesale rope swap, which
/// no recorded edit delta maps from.
const NO_REVISION: u64 = 0;

/// A fresh, never-before-issued revision.
fn fresh_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

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
    /// Live Markdown parse. An edit only marks it dirty; the reparse runs via
    /// [`Buffer::refresh_syntax`] once per input batch, so a keystroke never
    /// blocks on a parse (and the reparse touches only the changed block).
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
    /// Monotonic count of text-mutating edits (#1004): bumped once per rope
    /// insert / remove. Not persisted or reset — callers compare a saved value
    /// against [`edit_seq`](Buffer::edit_seq) to tell whether a command actually
    /// changed the document (a pure motion leaves it unchanged), which is how
    /// Vim's `.` (repeat) decides what to record as the last change.
    edits: u64,
    /// Content revision: a process-unique id for the document text as it stands,
    /// re-issued on every text change — an edit, an undo/redo restore, or a
    /// wholesale [`replace_text`](Buffer::replace_text). Unlike [`edits`](Self::edits)
    /// (which Vim's `.` reads, and which undo must not move), this changes whenever
    /// the text does, so it is a cheap "has the content changed since I last
    /// looked?" key: the recovery sidecar skips rewriting an unchanged buffer, and
    /// the spell flags know when to recompute.
    revision: u64,
    /// The revision the pending [`anchor_edits`](Self::anchor_edits) start from —
    /// the text as of the last [`drain_anchor_edits`](Buffer::drain_anchor_edits).
    /// A cache computed against that revision can be remapped through the drained
    /// deltas instead of recomputed. [`NO_REVISION`] after a rope swap (undo/redo,
    /// `replace_text`), which the deltas don't describe.
    anchor_base: u64,
}

impl Buffer {
    /// An empty, untitled buffer. Test-only today; #24 (a freshly created chapter
    /// before its first save) will reuse it — un-gate it then.
    #[cfg(any(test, feature = "test-support"))]
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
        let revision = fresh_revision();
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
            edits: 0,
            revision,
            anchor_base: revision,
        }
    }

    /// A monotonic count of text-mutating edits (#1004): each rope insert /
    /// remove bumps it by one, so a difference between two reads means the
    /// document changed in between. Never reset — compare deltas, not the
    /// absolute value. Vim's `.` uses this to record only commands that edited.
    pub const fn edit_seq(&self) -> u64 {
        self.edits
    }

    /// The content revision: process-unique, re-issued whenever the text changes
    /// (edit, undo/redo, wholesale replace) and never reused, so two equal reads
    /// mean the same buffer with the same text. See the field docs.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// The revision the pending anchor edits apply to (#302): a cache computed at
    /// this revision is brought current by remapping it through the deltas
    /// [`drain_anchor_edits`](Self::drain_anchor_edits) returns. `0` after a rope
    /// swap, which no delta describes — such a cache must be recomputed.
    pub const fn anchor_base(&self) -> u64 {
        self.anchor_base
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
    pub const fn front_matter(&self) -> Option<&FrontMatter> {
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

        // One copy of the rope (not a `to_string` and then a normalized second
        // copy): materialize it, then trim the trailing newlines in place.
        let mut body = String::new();
        self.text_into(&mut body);
        normalize_trailing_newline_in_place(&mut body);
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

    /// The text as a rope — an O(1) clone hands a snapshot to a worker thread.
    pub const fn rope(&self) -> &Rope {
        &self.rope
    }

    /// The whole buffer as a string (used to commit a scratch metadata edit).
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Materialize the whole buffer into `out`, reusing its capacity. The idle
    /// spell pass (#780) keeps a persistent scratch `String` and calls this each
    /// tick rather than allocating a fresh whole-document copy via [`text`](Self::text);
    /// appending the rope's chunks skips the intermediate allocation too.
    pub fn text_into(&self, out: &mut String) {
        out.clear();
        out.reserve(self.rope.len_bytes());
        for chunk in self.rope.chunks() {
            out.push_str(chunk);
        }
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
        self.revision = fresh_revision();
        self.anchor_base = NO_REVISION; // a wholesale swap no edit delta describes
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
        // Snap to a valid char boundary before slicing. A byte offset that lands
        // inside a multibyte grapheme — a stale persisted position, mark, or fold
        // heading recorded before smart-typography rewrote the bytes — would
        // otherwise panic on the `&str` slice in `byte_to_col`, bricking reopen at
        // startup. Flooring to the previous boundary leaves valid offsets untouched.
        let byte = self
            .rope
            .char_to_byte(self.rope.byte_to_char(byte.min(self.rope.len_bytes())));
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

    /// Insert text at a char index and mark the parse stale (the next
    /// [`refresh_syntax`](Self::refresh_syntax) reparses only the changed
    /// block). Does not touch the cursor.
    fn edit_insert(&mut self, char_idx: usize, text: &str) {
        self.checkpoint();
        self.edits = self.edits.wrapping_add(1);
        self.revision = fresh_revision();
        let start_byte = self.rope.char_to_byte(char_idx);

        // Maintain the word count incrementally (#868): re-segment only the
        // line(s) the insert touches, not the whole chapter. A word never
        // straddles a `\n` (UAX #29 makes a newline a word boundary), so the
        // document total is the sum of the per-line counts — subtract the old
        // span's words and add the new span's. The edit lands on the line at
        // `char_idx`; any newlines in `text` extend the new span by that many.
        let touched_line = self.rope.char_to_line(char_idx);
        let old_span_words = self.words_in_line_range(touched_line, touched_line);
        let added_lines = text.bytes().filter(|&b| b == b'\n').count();

        self.rope.insert(char_idx, text);

        self.adjust_word_count(touched_line, touched_line + added_lines, old_span_words);

        let new_end_byte = start_byte + text.len();
        self.anchor_edits
            .push((start_byte, start_byte, new_end_byte));
        self.syntax_dirty = true;
    }

    /// Remove the char range `[start, end)` and mark the parse stale. Does not
    /// touch the cursor.
    fn edit_remove(&mut self, start: usize, end: usize) {
        self.checkpoint();
        self.edits = self.edits.wrapping_add(1);
        self.revision = fresh_revision();
        let start_byte = self.rope.char_to_byte(start);
        let old_end_byte = self.rope.char_to_byte(end);

        // Incremental word count (#868): the removal spans lines
        // `[first, last]`; after it, they collapse onto `first`. Re-segment only
        // that range, never the whole chapter (see `edit_insert`).
        let first_line = self.rope.char_to_line(start);
        let last_line = self.rope.char_to_line(end);
        let old_span_words = self.words_in_line_range(first_line, last_line);

        self.rope.remove(start..end);

        self.adjust_word_count(first_line, first_line, old_span_words);

        self.anchor_edits
            .push((start_byte, old_end_byte, start_byte));
        self.syntax_dirty = true;
    }

    /// Words in the inclusive line range `[first, last]`, via Unicode word
    /// segmentation over just those lines. Used to maintain the running word
    /// count incrementally (#868): the edit paths re-segment only the handful of
    /// lines an edit touches, not the whole chapter. The materialized slice is
    /// bounded by the affected lines, so this is independent of document size.
    fn words_in_line_range(&self, first: usize, last: usize) -> usize {
        let lines = self.rope.len_lines();
        if first >= lines {
            return 0;
        }
        let last = last.min(lines.saturating_sub(1));
        let from = self.rope.line_to_char(first);
        // Include the last line's trailing newline (up to the next line's start,
        // or the document end). A newline is never inside a word, so counting
        // over it can't merge or split a word — the tally is unaffected.
        let to = if last + 1 < lines {
            self.rope.line_to_char(last + 1)
        } else {
            self.rope.len_chars()
        };
        count_words(&self.rope.slice(from..to).to_string())
    }

    /// Fold an edit's word-count delta into the cache (#868). The edit replaced
    /// the old lines (whose words were `old_span_words`) with the new lines
    /// `[first, last]`; recount only those and adjust the running total. When the
    /// cache is empty (e.g. after undo swapped the whole rope) there is nothing
    /// to adjust — the next reader recomputes from scratch.
    fn adjust_word_count(&self, first: usize, last: usize, old_span_words: usize) {
        if let Some(total) = self.word_count_cache.get() {
            let new_span_words = self.words_in_line_range(first, last);
            let updated = total + new_span_words - old_span_words;
            self.word_count_cache.set(Some(updated));
        }
    }

    /// Take the byte-range edits recorded since the last call (#302), to re-anchor
    /// author annotations off the keystroke path. Each is `(start, old_end,
    /// new_end)`: bytes `[start, old_end)` became `new_end - start` new bytes.
    pub fn drain_anchor_edits(&mut self) -> Vec<(usize, usize, usize)> {
        self.anchor_base = self.revision;
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

    /// The single character immediately *at* the cursor (the one a Forward-Delete
    /// would remove), or `None` at the very end of the buffer. Used by auto-pair
    /// (#1007) to decide whether typing a closer should skip over an existing one.
    /// O(1) — it reads just the one char at the cursor's char index, never scanning
    /// the line or the document, so it stays off the per-keystroke cost ceiling.
    pub fn char_after_cursor(&self) -> Option<char> {
        let idx = self.cursor_char_idx();
        (idx < self.rope.len_chars()).then(|| self.rope.char(idx))
    }

    pub fn insert_char(&mut self, c: char) {
        let idx = self.cursor_char_idx();
        let start_byte = self.rope.char_to_byte(idx);
        let mut buf = [0u8; 4];
        self.edit_insert(idx, c.encode_utf8(&mut buf));
        self.dirty = true;
        if c == '\n' {
            self.cursor.line += 1;
            self.cursor.col = 0;
        } else {
            // `col` is a grapheme column, so it can't blindly `+= 1`: the inserted
            // scalar may *merge* into the preceding grapheme rather than start a
            // new one — a combining mark (U+0301), a ZWJ, a variation selector
            // (U+FE0F), or a regional indicator that completes a flag — leaving the
            // line's grapheme count unchanged. Derive the column from the inserted
            // char's end byte via the grapheme-aware byte→col mapping (as
            // `replace_byte_range` does): a normal char advances one column, a
            // merge stays on the (unchanged) boundary, so `col` never runs off the
            // end of the line (#16). Reads only the current line, so it stays
            // O(line) — decoupled from document size like the `+= 1` it replaces.
            self.cursor = self.cursor_at_byte(start_byte + c.len_utf8());
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
    pub const fn move_line_start(&mut self) {
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

    // --- UTF-16 positions (#1232) --------------------------------------------
    //
    // UIKit addresses text in UTF-16 code units, the buffer in UTF-8 bytes. An
    // emoji is two code units and four bytes; an "é" written as "e" plus a
    // combining accent is two code units and three bytes. The phone converts
    // every position through these, in O(log n) on the rope, never by counting
    // through the text.

    /// Total length in UTF-16 code units.
    pub fn len_utf16(&self) -> usize {
        self.rope.len_utf16_cu()
    }

    /// The byte offset of UTF-16 code unit `cu`, clamped to the end. An offset
    /// inside a surrogate pair (half an emoji) rounds down to the start of
    /// its character, so an edit can never split one.
    pub fn byte_of_utf16(&self, cu: usize) -> usize {
        let cu = cu.min(self.rope.len_utf16_cu());
        self.rope.char_to_byte(self.rope.utf16_cu_to_char(cu))
    }

    /// The UTF-16 code unit offset of byte `byte`, clamped to the end.
    pub fn utf16_of_byte(&self, byte: usize) -> usize {
        let byte = byte.min(self.rope.len_bytes());
        self.rope.char_to_utf16_cu(self.rope.byte_to_char(byte))
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
        // Snap to a valid char boundary before slicing, mirroring `cursor_at_byte`:
        // a future caller (a stale persisted position, mark, or fold heading whose
        // byte offset now lands inside a multi-byte grapheme) must not panic on the
        // `byte_slice(byte..hi)` below. Flooring to the previous boundary leaves the
        // cursor-derived offsets today's callers pass untouched.
        let byte = self
            .rope
            .char_to_byte(self.rope.byte_to_char(byte.min(len)));
        let mut radius = GRAPHEME_WINDOW;
        loop {
            let hi = (byte + radius).min(len);
            let hi = self.rope.char_to_byte(self.rope.byte_to_char(hi));
            let slice = self.rope.byte_slice(byte..hi);
            // The window is short (#693), so it almost always lands inside a single
            // rope chunk — borrow it with no allocation; only the rare chunk-straddling
            // window falls back to a copy (#869).
            let second_grapheme = |text: &str| text.grapheme_indices(true).nth(1).map(|(b, _)| b);
            let boundary = slice
                .as_str()
                .map_or_else(|| second_grapheme(&slice.to_string()), second_grapheme);
            match boundary {
                Some(b) => return byte + b,
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
    ///
    /// The cache is maintained incrementally on the edit path (#868), so a
    /// keystroke never triggers an O(document) recount. This cold path runs only
    /// when the cache is empty — a fresh buffer, or after undo/redo/replace
    /// swapped the whole rope — and rebuilds it once from scratch. Even that
    /// avoids a whole-document `String`: it segments line by line over the rope's
    /// chunks, buffering only the current line. A word never straddles a `\n`
    /// (UAX #29), so summing the per-line counts equals segmenting the whole
    /// text, with no chunk-boundary double-count hazard.
    pub fn word_count(&self) -> usize {
        if let Some(n) = self.word_count_cache.get() {
            return n;
        }
        let mut total = 0;
        let mut line = String::new();
        for chunk in self.rope.chunks() {
            let mut rest = chunk;
            while let Some(nl) = rest.find('\n') {
                line.push_str(&rest[..nl]);
                total += count_words(&line);
                line.clear();
                rest = &rest[nl + 1..];
            }
            line.push_str(rest);
        }
        total += count_words(&line); // the final line, which has no trailing `\n`
        self.word_count_cache.set(Some(total));
        total
    }

    /// How many times `needle` occurs in the document under the default match
    /// rule — exactly [`text::count_matches`] over [`text`](Self::text), but
    /// without materializing the whole document: it scans line by line over the
    /// rope's chunks, like [`word_count`](Self::word_count). A needle with no
    /// `\n` can never match across a line break (matching is per character), so
    /// the per-line counts sum to the whole-text count. A needle that does span
    /// lines falls back to the whole-text scan.
    pub fn count_matches(&self, needle: &str) -> usize {
        if needle.contains('\n') {
            return text::count_matches(&self.text(), needle);
        }
        let mut total = 0;
        let mut line = String::new();
        for chunk in self.rope.chunks() {
            let mut rest = chunk;
            while let Some(nl) = rest.find('\n') {
                line.push_str(&rest[..nl]);
                total += text::count_matches(&line, needle);
                line.clear();
                rest = &rest[nl + 1..];
            }
            line.push_str(rest);
        }
        total + text::count_matches(&line, needle)
    }

    // --- Syntax ------------------------------------------------------------

    /// The live Markdown parse, for highlighting and (later) the outline.
    pub const fn syntax(&self) -> &Syntax {
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

    /// The line a byte offset falls on (clamped to the document end). O(log n)
    /// on the rope — no line copy or grapheme scan, unlike
    /// [`cursor_at_byte`](Self::cursor_at_byte), for callers that only need the
    /// line.
    pub fn line_of_byte(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.rope.len_bytes()))
    }
}

/// Byte offset of grapheme column `col` within a single line's `text` (the line
/// end if `col` is past the last grapheme).
fn col_to_byte(text: &str, col: usize) -> usize {
    text.grapheme_indices(true)
        .nth(col)
        .map_or(text.len(), |(b, _)| b)
}

/// Grapheme column of byte offset `byte` within a single line's `text`.
fn byte_to_col(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].graphemes(true).count()
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
    normalize_trailing_newline_in_place(&mut content);
    content
}

/// [`normalize_trailing_newline`] on an owned string, without copying it.
fn normalize_trailing_newline_in_place(content: &mut String) {
    content.truncate(content.trim_end_matches('\n').len());
    if !content.is_empty() {
        content.push('\n');
    }
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

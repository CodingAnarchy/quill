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

use crate::frontmatter::FrontMatter;
use crate::syntax::Syntax;

mod history;
use history::Snapshot;

/// Direction and stop rule for the `f`/`t`/`F`/`T` find-char motions (#202):
/// land *on* the target (`Find`) or just *before* it (`Till`), forward or back.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FindKind {
    FindFwd,
    FindBack,
    TillFwd,
    TillBack,
}

impl FindKind {
    /// The opposite direction — for `,`, which repeats a find the other way.
    pub fn reversed(self) -> Self {
        match self {
            Self::FindFwd => Self::FindBack,
            Self::FindBack => Self::FindFwd,
            Self::TillFwd => Self::TillBack,
            Self::TillBack => Self::TillFwd,
        }
    }
}

/// A Vim text object (#203) the operators `d`/`c` act on — `iw`/`aw`,
/// `i"`/`a"`, `i(`/`a(`, etc. The `around` flag (inner vs. a-) is passed
/// separately to [`Buffer::text_object`].
#[derive(Clone, Copy)]
pub enum TextObject {
    /// `w`: the word (or whitespace run) under the cursor.
    Word,
    /// `"` / `'` / `` ` ``: text between a pair of that quote on the line.
    Quote(char),
    /// `(` `)` `[` `]` `{` `}` `<` `>`: the enclosing matched pair.
    Pair(char, char),
}

/// Vim's character classes for word motions: a word is a run of word-characters
/// or a run of punctuation, separated by whitespace.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Punct,
    Space,
}

impl CharClass {
    fn of(grapheme: &str) -> Self {
        let c = grapheme.chars().next().unwrap_or(' ');
        if c.is_whitespace() {
            Self::Space
        } else if c.is_alphanumeric() || c == '_' {
            Self::Word
        } else {
            Self::Punct
        }
    }
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
}

impl Buffer {
    /// An empty, untitled buffer. Used by tests today; the path for a freshly
    /// created chapter before its first save (#24) will reuse it.
    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self {
            rope: Rope::new(),
            path: None,
            front_matter: None,
            cursor: Cursor::default(),
            dirty: false,
            goal_x: None,
            syntax: Syntax::new(""),
            syntax_dirty: false,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            group_open: false,
            word_count_cache: Cell::new(None),
        }
    }

    /// An in-memory buffer over `text`, with no backing path. Used for the
    /// continuous-manuscript view, whose buffer spans several chapter files and
    /// is saved by splitting it back out (see [`crate::manuscript`]), so it has
    /// no single path or front matter of its own.
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
        Ok(Self {
            syntax: Syntax::new(body),
            rope: Rope::from_str(body),
            path: Some(path),
            front_matter: front.map(FrontMatter::from_raw),
            cursor: Cursor::default(),
            dirty: false,
            goal_x: None,
            syntax_dirty: false,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            group_open: false,
            word_count_cache: Cell::new(None),
        })
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
        fs::write(path, content.as_bytes())?;
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
        Cursor { line, col: byte_to_col(&line_text, within) }
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
        let line_start = self.rope.line_to_char(self.cursor.line.min(self.rope.len_lines().saturating_sub(1)));
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
        self.syntax_dirty = true;
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
        let blank = line.graphemes(true).take_while(|g| g.chars().all(char::is_whitespace)).count();
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

    /// Move to the start of the next word — Vim's `w`. A word is a run of
    /// word-characters or a run of punctuation; whitespace separates them. Spans
    /// line boundaries like Vim.
    pub fn move_word_forward(&mut self) {
        let items = self.classes();
        let mut i = self.grapheme_index(&items);
        let Some(&(_, start)) = items.get(i) else { return };
        if start != CharClass::Space {
            while items.get(i).is_some_and(|&(_, c)| c == start) {
                i += 1;
            }
        }
        while items.get(i).is_some_and(|&(_, c)| c == CharClass::Space) {
            i += 1;
        }
        let target = items.get(i).map_or_else(|| self.rope.len_bytes(), |&(b, _)| b);
        self.cursor = self.cursor_at_byte(target);
        self.goal_x = None;
    }

    /// Move to the start of the current/previous word — Vim's `b`.
    pub fn move_word_backward(&mut self) {
        let items = self.classes();
        let cur = self.grapheme_index(&items);
        if cur == 0 {
            return;
        }
        let mut i = cur - 1;
        while i > 0 && items[i].1 == CharClass::Space {
            i -= 1;
        }
        if items[i].1 == CharClass::Space {
            self.cursor = self.cursor_at_byte(items[0].0);
            self.goal_x = None;
            return;
        }
        let class = items[i].1;
        while i > 0 && items[i - 1].1 == class {
            i -= 1;
        }
        self.cursor = self.cursor_at_byte(items[i].0);
        self.goal_x = None;
    }

    /// Move to the end of the next word — Vim's `e`.
    pub fn move_word_end(&mut self) {
        let items = self.classes();
        let mut i = self.grapheme_index(&items) + 1;
        while items.get(i).is_some_and(|&(_, c)| c == CharClass::Space) {
            i += 1;
        }
        let Some(&(_, class)) = items.get(i) else { return };
        while items.get(i + 1).is_some_and(|&(_, c)| c == class) {
            i += 1;
        }
        self.cursor = self.cursor_at_byte(items[i].0);
        self.goal_x = None;
    }

    /// Find the `count`-th occurrence of `target` on the current line and move to
    /// it (or just shy of it, for `t`/`T`) — Vim's `f`/`t`/`F`/`T`. No move when
    /// there is no such occurrence.
    pub fn find_char(&mut self, target: char, kind: FindKind, count: usize) {
        let line = self.line(self.cursor.line);
        let hits: Vec<usize> = line
            .graphemes(true)
            .enumerate()
            .filter(|(_, g)| g.starts_with(target))
            .map(|(i, _)| i)
            .collect();
        let cur = self.cursor.col;
        let nth = count.saturating_sub(1);
        let found = match kind {
            FindKind::FindFwd | FindKind::TillFwd => hits.iter().filter(|&&c| c > cur).nth(nth).copied(),
            FindKind::FindBack | FindKind::TillBack => hits.iter().rev().filter(|&&c| c < cur).nth(nth).copied(),
        };
        if let Some(col) = found {
            self.cursor.col = match kind {
                FindKind::TillFwd => col - 1,
                FindKind::TillBack => col + 1,
                _ => col,
            };
            self.goal_x = None;
        }
    }

    /// The whole buffer as `(byte offset, character class)` per grapheme — the
    /// scan the word motions walk.
    fn classes(&self) -> Vec<(usize, CharClass)> {
        let text = self.rope.to_string();
        text.grapheme_indices(true).map(|(b, g)| (b, CharClass::of(g))).collect()
    }

    /// Index into [`classes`](Self::classes) of the grapheme at the cursor (the
    /// length when the cursor is at the very end).
    fn grapheme_index(&self, items: &[(usize, CharClass)]) -> usize {
        let byte = self.cursor_byte();
        items.iter().position(|&(b, _)| b >= byte).unwrap_or(items.len())
    }

    // --- Operator support (#203) -------------------------------------------

    /// Total byte length of the document.
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// The byte just past the grapheme starting at `byte` — so an inclusive
    /// motion or `x` can cover the grapheme the cursor is on. Clamped to the end.
    pub fn grapheme_after(&self, byte: usize) -> usize {
        let text = self.rope.to_string();
        text[byte.min(text.len())..]
            .grapheme_indices(true)
            .nth(1)
            .map_or(text.len(), |(b, _)| byte + b)
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

    /// The byte range of a text object around the cursor (#203) — `inner` is the
    /// content, the `around` form includes the delimiters / trailing whitespace.
    /// `None` when there's no such object here.
    pub fn text_object(&self, object: TextObject, around: bool) -> Option<(usize, usize)> {
        match object {
            TextObject::Word => self.word_object(around),
            TextObject::Quote(q) => self.quote_object(q, around),
            TextObject::Pair(open, close) => self.pair_object(open, close, around),
        }
    }

    /// `iw` / `aw`: the run of same-class graphemes under the cursor; `aw` also
    /// takes the trailing whitespace (or leading, when there's none trailing).
    fn word_object(&self, around: bool) -> Option<(usize, usize)> {
        let items = self.classes();
        if items.is_empty() {
            return None;
        }
        let cur = self.grapheme_index(&items).min(items.len() - 1);
        let class = items[cur].1;
        let mut start = cur;
        while start > 0 && items[start - 1].1 == class {
            start -= 1;
        }
        let mut end = cur; // last grapheme index of the run
        while end + 1 < items.len() && items[end + 1].1 == class {
            end += 1;
        }
        let mut start_b = items[start].0;
        let mut end_b = items.get(end + 1).map_or_else(|| self.len_bytes(), |&(b, _)| b);
        if around {
            let mut e = end + 1;
            if items.get(e).is_some_and(|&(_, c)| c == CharClass::Space) {
                while items.get(e).is_some_and(|&(_, c)| c == CharClass::Space) {
                    e += 1;
                }
                end_b = items.get(e).map_or_else(|| self.len_bytes(), |&(b, _)| b);
            } else {
                while start > 0 && items[start - 1].1 == CharClass::Space {
                    start -= 1;
                }
                start_b = items[start].0;
            }
        }
        Some((start_b, end_b))
    }

    /// `i"` / `a"` (and `'`, `` ` ``): the text between the pair of quotes on the
    /// current line that the cursor is at or before.
    fn quote_object(&self, quote: char, around: bool) -> Option<(usize, usize)> {
        let line = self.line(self.cursor.line);
        let base = self.line_start_byte(self.cursor.line);
        let marks: Vec<usize> = line
            .grapheme_indices(true)
            .filter(|(_, g)| g.starts_with(quote))
            .map(|(b, _)| b)
            .collect();
        let cur = col_to_byte(&line, self.cursor.col);
        for pair in marks.chunks(2) {
            let [open, close] = *pair else { break };
            if cur <= close {
                return Some(if around {
                    (base + open, base + close + quote.len_utf8())
                } else {
                    (base + open + quote.len_utf8(), base + close)
                });
            }
        }
        None
    }

    /// `i(` / `a(` (and the other bracket pairs): the enclosing matched pair,
    /// honoring nesting, searched across the whole buffer.
    fn pair_object(&self, open: char, close: char, around: bool) -> Option<(usize, usize)> {
        let text = self.rope.to_string();
        let cur = self.cursor_byte().min(text.len());
        // Walk back to the unmatched open bracket enclosing the cursor.
        let mut depth = 0usize;
        let mut open_b = None;
        for (b, ch) in text[..=cur.min(text.len().saturating_sub(1))].char_indices().rev() {
            if ch == close && b != cur {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    open_b = Some(b);
                    break;
                }
                depth -= 1;
            }
        }
        let open_b = open_b?;
        // Walk forward to its matching close.
        depth = 0;
        let mut close_b = None;
        for (b, ch) in text[open_b + open.len_utf8()..].char_indices() {
            let b = open_b + open.len_utf8() + b;
            if ch == open {
                depth += 1;
            } else if ch == close {
                if depth == 0 {
                    close_b = Some(b);
                    break;
                }
                depth -= 1;
            }
        }
        let close_b = close_b?;
        Some(if around {
            (open_b, close_b + close.len_utf8())
        } else {
            (open_b + open.len_utf8(), close_b)
        })
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

    #[allow(dead_code)] // part of the buffer's public API; used by upcoming stats UI
    pub fn char_count(&self) -> usize {
        self.rope.len_chars()
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
    text.grapheme_indices(true).nth(col).map(|(b, _)| b).unwrap_or(text.len())
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
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A unique-per-process temp path so parallel tests don't collide.
    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("quill-test-{}-{}", std::process::id(), name));
        p
    }

    fn write(path: &Path, bytes: &str) {
        fs::write(path, bytes).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    // --- Vim motions (#202) ------------------------------------------------

    #[test]
    fn word_motions_step_by_word_and_punctuation() {
        let mut b = Buffer::from_text("the quick, brown fox");
        assert_eq!(b.cursor.col, 0);
        b.move_word_forward(); // -> "quick"
        assert_eq!(b.cursor.col, 4);
        b.move_word_forward(); // -> ","  (punctuation is its own word)
        assert_eq!(b.cursor.col, 9);
        b.move_word_forward(); // -> "brown"
        assert_eq!(b.cursor.col, 11);
        b.move_word_end(); // end of "brown"
        assert_eq!(b.cursor.col, 15);
        b.move_word_backward(); // back to start of "brown"
        assert_eq!(b.cursor.col, 11);
    }

    #[test]
    fn word_forward_crosses_line_boundaries() {
        let mut b = Buffer::from_text("end\nnext line");
        b.cursor = Cursor { line: 0, col: 1 };
        b.move_word_forward(); // from inside "end" to "next" on the following line
        assert_eq!((b.cursor.line, b.cursor.col), (1, 0));
    }

    #[test]
    fn line_and_document_motions() {
        let mut b = Buffer::from_text("  indented\nmiddle\nlast line\n");
        b.cursor = Cursor { line: 0, col: 6 };
        b.move_line_start();
        assert_eq!(b.cursor.col, 0);
        b.move_first_non_blank();
        assert_eq!(b.cursor.col, 2, "^ skips leading blanks");
        b.move_line_end();
        assert_eq!(b.cursor.col, b.line_len());

        b.move_to_line(1);
        assert_eq!(b.cursor.line, 1);
        // G targets the last line with content, not the empty trailing line.
        b.move_to_line(b.last_line_index());
        assert_eq!(b.cursor.line, 2, "G skips the trailing newline's empty line");
    }

    #[test]
    fn find_char_lands_on_or_before_the_target() {
        // cols:                 0    5    10
        let mut b = Buffer::from_text("alpha-beta-gamma");
        b.find_char('-', FindKind::FindFwd, 1);
        assert_eq!(b.cursor.col, 5, "f lands on the first '-'");
        b.find_char('-', FindKind::FindFwd, 1);
        assert_eq!(b.cursor.col, 10, "f again lands on the next '-'");
        b.find_char('a', FindKind::FindBack, 1);
        assert_eq!(b.cursor.col, 9, "F lands on the previous 'a'");
        b.find_char('m', FindKind::TillFwd, 1);
        assert_eq!(b.cursor.col, 12, "t lands just before the next 'm'");

        // Count selects the nth occurrence; a miss leaves the cursor put.
        b.cursor = Cursor { line: 0, col: 0 };
        b.find_char('-', FindKind::FindFwd, 2);
        assert_eq!(b.cursor.col, 10, "2f- reaches the second '-'");
        b.find_char('z', FindKind::FindFwd, 1);
        assert_eq!(b.cursor.col, 10, "a missing target doesn't move");
    }

    #[test]
    fn text_objects_cover_words_quotes_and_pairs() {
        // Word object: inner is the word, around takes the trailing space.
        let mut b = Buffer::from_text("the quick fox");
        b.cursor = Cursor { line: 0, col: 5 }; // inside "quick"
        assert_eq!(b.text_object(TextObject::Word, false), Some((4, 9)));
        assert_eq!(b.text_object(TextObject::Word, true), Some((4, 10)), "aw takes the trailing space");

        // Quote object on the line.
        let mut b = Buffer::from_text("say \"hello\" now");
        b.cursor = Cursor { line: 0, col: 6 }; // inside the quotes
        assert_eq!(b.text_object(TextObject::Quote('"'), false), Some((5, 10)), "i\" is the inside");
        assert_eq!(b.text_object(TextObject::Quote('"'), true), Some((4, 11)), "a\" includes the quotes");

        // Bracket pair with nesting.
        let mut b = Buffer::from_text("f(a, g(b), c)");
        b.cursor = Cursor { line: 0, col: 3 }; // inside the outer parens
        assert_eq!(b.text_object(TextObject::Pair('(', ')'), false), Some((2, 12)), "i( spans to the matching close");
        assert_eq!(b.text_object(TextObject::Pair('(', ')'), true), Some((1, 13)), "a( includes the parens");
    }

    #[test]
    fn motions_do_not_panic_at_boundaries() {
        let mut b = Buffer::from_text("word");
        b.move_word_backward(); // already at start
        assert_eq!(b.cursor.col, 0);
        b.cursor = Cursor { line: 0, col: 4 };
        b.move_word_forward(); // already at end
        b.move_word_end();
        assert_eq!(b.cursor.line, 0);
    }

    #[test]
    fn round_trip_is_byte_identical_with_trailing_newline() {
        let path = temp_path("roundtrip.md");
        let original = "# Chapter One\n\nThe night was *dark* and **stormy**.\n";
        write(&path, original);

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), original);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn front_matter_is_kept_off_the_prose_and_re_attached_on_save() {
        let path = temp_path("frontmatter.md");
        let original = "---\nstatus: draft\npov: Mara\n---\n# Chapter One\n\nThe body.\n";
        write(&path, original);

        let mut buf = Buffer::from_path(&path).unwrap();
        // The prose area (rope) holds only the body — no raw YAML.
        assert_eq!(buf.line(0), "# Chapter One");
        assert!(buf.front_matter().is_some());
        // Word count covers the body, not the front matter keys.
        assert_eq!(buf.word_count(), count_words("# Chapter One\n\nThe body.\n"));

        // Saving re-attaches the front matter untouched: byte-for-byte round-trip.
        buf.save().unwrap();
        assert_eq!(read(&path), original);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_adds_missing_trailing_newline() {
        let path = temp_path("no-newline.md");
        write(&path, "No newline at end");

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), "No newline at end\n");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_collapses_multiple_trailing_newlines_to_one() {
        let path = temp_path("many-newlines.md");
        write(&path, "Body\n\n\n\n");

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), "Body\n");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn empty_document_saves_empty_file() {
        let path = temp_path("empty.md");
        write(&path, "");

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), "");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn non_ascii_round_trips_exactly() {
        let path = temp_path("unicode.md");
        let original = "Café — résumé, naïve façade. 日本語のテキスト. 🚀✍️🌙\n";
        write(&path, original);

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), original);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn extensionless_path_defaults_to_md() {
        let path = temp_path("manuscript"); // no extension
        let buf = Buffer::from_path(&path).unwrap();

        let expected = path.with_extension("md");
        assert_eq!(buf.path.as_deref(), Some(expected.as_path()));
    }

    #[test]
    fn explicit_extension_is_respected() {
        let path = temp_path("notes.txt");
        let buf = Buffer::from_path(&path).unwrap();

        assert_eq!(buf.path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn count_words_uses_unicode_segmentation() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("   \n  "), 0);
        assert_eq!(count_words("one two three"), 3);
        assert_eq!(count_words("hello, world!"), 2);
        assert_eq!(count_words("it's a tidy plan"), 4); // contraction stays one word
        // Unicode segmentation splits hyphenated compounds.
        assert_eq!(count_words("well-made"), 2);
    }

    #[test]
    fn word_count_is_correct_across_rope_chunk_boundaries() {
        // A document large enough to span multiple rope chunks. A word split
        // across a chunk boundary must not be counted twice.
        let path = temp_path("wordcount.md");
        write(&path, &"word ".repeat(5000));

        let buf = Buffer::from_path(&path).unwrap();
        assert_eq!(buf.word_count(), 5000);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn word_count_cache_updates_on_edit_and_undo() {
        let mut buf = Buffer::from_text("one two");
        assert_eq!(buf.word_count(), 2);

        // Append a third word; the cached count reflects the edit (#151).
        buf.cursor = Cursor { line: 0, col: 7 };
        for c in " three".chars() {
            buf.insert_char(c);
        }
        assert_eq!(buf.word_count(), 3);

        // Delete it back; the count follows.
        buf.seal_undo_group();
        for _ in 0..6 {
            buf.backspace();
        }
        assert_eq!(buf.word_count(), 2);

        // Undo restores both the text and the count.
        buf.undo();
        assert_eq!(buf.word_count(), 3);
    }

    // The syntax highlights must stay correct through incremental edits — this
    // exercises the InputEdit bookkeeping in edit_insert/edit_remove.
    use crate::syntax::HighlightKind;

    fn kinds(buf: &Buffer) -> Vec<HighlightKind> {
        buf.syntax()
            .highlights_in(0, usize::MAX)
            .map(|h| h.kind)
            .collect()
    }

    #[test]
    fn typing_a_heading_updates_highlights_incrementally() {
        let mut buf = Buffer::empty();
        // A heading is recognized once the line is terminated, so press Enter
        // after it (as an author would before writing the body).
        for c in "# Title\n".chars() {
            buf.insert_char(c);
        }
        buf.refresh_syntax(); // edits are deferred; the event loop reparses on idle
        assert!(kinds(&buf).contains(&HighlightKind::Heading));
    }

    // Opt-in performance guards (run with `cargo test --release -- --ignored
    // --nocapture`). A buffer only ever holds the chapter(s) being edited — one
    // for the single-chapter view, at most ~3 for the continuous view — never
    // the whole novel.

    #[test]
    #[ignore]
    fn keystroke_is_decoupled_from_document_size() {
        use std::time::Instant;

        // A keystroke only *records* the edit; the reparse is deferred. So it
        // must be cheap regardless of buffer size. Stress it far past the real
        // ~3-chapter cap to prove the cost is in the edit, not the document.
        let mut text = String::new();
        for ch in 0..150 {
            text.push_str(&format!("# Chapter {ch}\n\n"));
            for _ in 0..50 {
                text.push_str("The night was *dark* and the road was **long** indeed.\n");
            }
            text.push('\n');
        }
        let words = text.split_whitespace().count();

        let path = temp_path("perf-big.md");
        write(&path, &text);
        let mut buf = Buffer::from_path(&path).unwrap();
        buf.cursor = Cursor {
            line: buf.line_count() / 2,
            col: 0,
        };

        let t = Instant::now();
        buf.insert_char('X'); // record-only, no parse
        let dt = t.elapsed();

        println!("keystroke(record-only) on {words} words = {dt:?}");
        assert!(dt.as_millis() < 4, "keystroke took {dt:?}");
        fs::remove_file(&path).ok();
    }

    #[test]
    #[ignore]
    fn reparse_is_fast_at_window_size() {
        use std::time::{Duration, Instant};

        // Realistic worst case: ~3 long chapters live at once (continuous view).
        let mut text = String::new();
        for ch in 0..3 {
            text.push_str(&format!("# Chapter {ch}\n\n"));
            for _ in 0..150 {
                text.push_str("The night was *dark* and the road was **long** indeed.\n");
            }
            text.push('\n');
        }
        let words = text.split_whitespace().count();

        let path = temp_path("perf-window.md");
        write(&path, &text);
        let mut buf = Buffer::from_path(&path).unwrap();
        buf.cursor = Cursor {
            line: buf.line_count() / 2,
            col: 0,
        };

        // Best-of-N: each reparse is genuinely ~8ms, well under a frame, but a
        // single sample is dominated by one-off noise (scheduling, CPU ramp,
        // first-call allocation) that occasionally tips it over 16ms. Noise only
        // ever *adds* time, so the minimum across runs reflects the true compute
        // cost. Each run needs a fresh edit — `refresh_syntax` is a no-op when the
        // syntax isn't dirty.
        let mut best = Duration::MAX;
        for _ in 0..5 {
            buf.insert_char('X');
            let t = Instant::now();
            buf.refresh_syntax();
            best = best.min(t.elapsed());
        }

        println!("reparse on {words} words = {best:?} (best of 5)");
        assert!(best.as_millis() < 16, "reparse took {best:?}");
        fs::remove_file(&path).ok();
    }

    #[test]
    fn editing_mid_document_keeps_highlights_correct() {
        // Open a multi-line doc, then add emphasis in the middle and check the
        // parse tracked the edit (heading stays, emphasis appears).
        let path = temp_path("mid-edit.md");
        write(&path, "# Chapter\n\nplain line\n");
        let mut buf = Buffer::from_path(&path).unwrap();

        // Move into the body line and wrap a word in asterisks.
        buf.cursor = Cursor { line: 2, col: 0 };
        buf.insert_char('*');
        buf.cursor = Cursor { line: 2, col: 6 }; // after "*plain"
        buf.insert_char('*');
        buf.refresh_syntax();

        let ks = kinds(&buf);
        assert!(ks.contains(&HighlightKind::Heading));
        assert!(ks.contains(&HighlightKind::Emphasis));
        fs::remove_file(&path).ok();
    }

    // --- Undo / redo (#145) ------------------------------------------------

    #[test]
    fn undo_reverts_a_typing_burst_and_redo_reapplies_it() {
        let mut buf = Buffer::empty();
        for c in "hello".chars() {
            buf.insert_char(c);
        }
        assert_eq!(buf.text(), "hello");
        // A burst with no idle in between is one group.
        assert!(buf.undo());
        assert_eq!(buf.text(), "");
        assert!(buf.redo());
        assert_eq!(buf.text(), "hello");
    }

    #[test]
    fn sealing_starts_a_new_undo_group() {
        let mut buf = Buffer::empty();
        for c in "foo".chars() {
            buf.insert_char(c);
        }
        buf.seal_undo_group(); // idle boundary
        for c in "bar".chars() {
            buf.insert_char(c);
        }
        assert_eq!(buf.text(), "foobar");
        assert!(buf.undo());
        assert_eq!(buf.text(), "foo", "only the second burst is undone");
        assert!(buf.undo());
        assert_eq!(buf.text(), "");
        assert!(!buf.undo(), "nothing left to undo");
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut buf = Buffer::empty();
        for c in "ab".chars() {
            buf.insert_char(c);
        }
        buf.undo();
        assert_eq!(buf.text(), "");
        buf.insert_char('z'); // a fresh edit invalidates the redo
        assert!(!buf.redo());
        assert_eq!(buf.text(), "z");
    }

    #[test]
    fn undo_restores_the_cursor_and_reparses() {
        let mut buf = Buffer::empty();
        for c in "# Title\n".chars() {
            buf.insert_char(c);
        }
        buf.refresh_syntax();
        let cursor_before = buf.cursor; // line 1, col 0 — just under the heading
        buf.seal_undo_group();
        for c in "body".chars() {
            buf.insert_char(c);
        }
        assert!(buf.undo());
        assert_eq!(buf.text(), "# Title\n");
        assert_eq!(buf.cursor, cursor_before, "cursor returns to where the edit began");
        // Highlights are rebuilt immediately on undo, not deferred to idle.
        assert!(kinds(&buf).contains(&HighlightKind::Heading));
    }

    #[test]
    fn undo_and_redo_on_a_fresh_buffer_are_noops() {
        let mut buf = Buffer::empty();
        assert!(!buf.undo());
        assert!(!buf.redo());
    }

    // --- Soft-wrap navigation (#143) ---------------------------------------
    // "aaaa bbbb cccc dddd" at width 10 wraps to "aaaa bbbb " | "cccc dddd".

    #[test]
    fn move_down_steps_through_wrapped_visual_rows() {
        let mut buf = Buffer::from_text("aaaa bbbb cccc dddd");
        buf.cursor = Cursor { line: 0, col: 0 };
        buf.move_down(10); // down one visual row — still the same logical line
        assert_eq!(buf.cursor.line, 0, "stayed on the wrapped logical line");
        assert_eq!(buf.cursor.col, 10, "landed at the start of the second visual row");
        buf.move_up(10); // back up to the first visual row at the same column
        assert_eq!(buf.cursor, Cursor { line: 0, col: 0 });
    }

    #[test]
    fn home_and_end_act_on_the_visual_row() {
        let mut buf = Buffer::from_text("aaaa bbbb cccc dddd");
        buf.cursor = Cursor { line: 0, col: 13 }; // inside the second visual row
        buf.move_home(10);
        assert_eq!(buf.cursor.col, 10, "Home goes to the visual row's start");
        buf.move_end(10);
        assert_eq!(buf.cursor.col, 19, "End goes to the visual row's end");
    }

    #[test]
    fn vertical_move_without_wrapping_still_works() {
        let mut buf = Buffer::from_text("first\nsecond\n");
        buf.cursor = Cursor { line: 0, col: 3 };
        buf.move_down(80); // wide enough that nothing wraps
        assert_eq!(buf.cursor, Cursor { line: 1, col: 3 }, "moves to the next logical line");
    }

    #[test]
    fn backspace_participates_in_undo() {
        let mut buf = Buffer::empty();
        for c in "draft".chars() {
            buf.insert_char(c);
        }
        buf.seal_undo_group();
        buf.backspace();
        buf.backspace();
        assert_eq!(buf.text(), "dra");
        assert!(buf.undo());
        assert_eq!(buf.text(), "draft", "the deletions undo as one group");
    }
}

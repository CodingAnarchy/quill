//! The text buffer: a rope-backed document with a single cursor.
//!
//! A novel is mostly long runs of prose with occasional edits in the middle, so
//! a rope (`ropey`) gives us cheap inserts/deletes at any offset without copying
//! the whole document around.

use std::path::{Path, PathBuf};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

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
    pub cursor: Cursor,
    /// Whether there are unsaved changes.
    pub dirty: bool,
    /// Remembered column for vertical movement, so moving up/down through
    /// short lines doesn't permanently collapse the cursor toward column 0.
    goal_col: Option<usize>,
}

impl Buffer {
    /// An empty, untitled buffer.
    pub fn empty() -> Self {
        Self {
            rope: Rope::new(),
            path: None,
            cursor: Cursor::default(),
            dirty: false,
            goal_col: None,
        }
    }

    /// Load a file from disk, or start an empty buffer pointed at `path` if the
    /// file does not exist yet.
    pub fn from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let rope = match std::fs::read_to_string(path) {
            Ok(text) => Rope::from_str(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Rope::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            rope,
            path: Some(path.to_path_buf()),
            cursor: Cursor::default(),
            dirty: false,
            goal_col: None,
        })
    }

    /// Persist the buffer to its path. Returns an error if there is no path.
    pub fn save(&mut self) -> std::io::Result<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "no file path set")
        })?;
        let mut file = std::fs::File::create(path)?;
        self.rope.write_to(&mut file)?;
        self.dirty = false;
        Ok(())
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines().max(1)
    }

    /// The text of a single line, without its trailing newline.
    pub fn line(&self, idx: usize) -> String {
        if idx >= self.rope.len_lines() {
            return String::new();
        }
        let slice = self.rope.line(idx);
        let s = slice.to_string();
        s.trim_end_matches('\n').to_string()
    }

    /// Number of grapheme clusters on a line (how far the cursor can travel).
    fn line_graphemes(&self, idx: usize) -> usize {
        self.line(idx).graphemes(true).count()
    }

    /// Convert the cursor's (line, col) into an absolute char offset in the rope.
    fn cursor_char_idx(&self) -> usize {
        let line_start = self.rope.line_to_char(self.cursor.line.min(self.rope.len_lines().saturating_sub(1)));
        let line = self.line(self.cursor.line);
        let byte: usize = line
            .grapheme_indices(true)
            .nth(self.cursor.col)
            .map(|(b, _)| b)
            .unwrap_or(line.len());
        // Translate the byte offset within the line to a char offset.
        let char_in_line = line[..byte].chars().count();
        line_start + char_in_line
    }

    // --- Editing -----------------------------------------------------------

    pub fn insert_char(&mut self, c: char) {
        let idx = self.cursor_char_idx();
        self.rope.insert_char(idx, c);
        self.dirty = true;
        if c == '\n' {
            self.cursor.line += 1;
            self.cursor.col = 0;
        } else {
            self.cursor.col += 1;
        }
        self.goal_col = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
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
            self.rope.remove(start_c..end_c);
            self.cursor.col = target;
            self.dirty = true;
        } else if self.cursor.line > 0 {
            // Join with the previous line.
            let prev_len = self.line_graphemes(self.cursor.line - 1);
            let idx = self.rope.line_to_char(self.cursor.line);
            self.rope.remove(idx - 1..idx); // remove the newline
            self.cursor.line -= 1;
            self.cursor.col = prev_len;
            self.dirty = true;
        }
        self.goal_col = None;
    }

    // --- Movement ----------------------------------------------------------

    pub fn move_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.line_graphemes(self.cursor.line);
        }
        self.goal_col = None;
    }

    pub fn move_right(&mut self) {
        let len = self.line_graphemes(self.cursor.line);
        if self.cursor.col < len {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.line_count() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
        self.goal_col = None;
    }

    pub fn move_up(&mut self) {
        if self.cursor.line == 0 {
            self.cursor.col = 0;
            return;
        }
        let goal = *self.goal_col.get_or_insert(self.cursor.col);
        self.cursor.line -= 1;
        self.cursor.col = goal.min(self.line_graphemes(self.cursor.line));
    }

    pub fn move_down(&mut self) {
        if self.cursor.line + 1 >= self.line_count() {
            self.cursor.col = self.line_graphemes(self.cursor.line);
            return;
        }
        let goal = *self.goal_col.get_or_insert(self.cursor.col);
        self.cursor.line += 1;
        self.cursor.col = goal.min(self.line_graphemes(self.cursor.line));
    }

    pub fn move_home(&mut self) {
        self.cursor.col = 0;
        self.goal_col = None;
    }

    pub fn move_end(&mut self) {
        self.cursor.col = self.line_graphemes(self.cursor.line);
        self.goal_col = None;
    }

    // --- Stats -------------------------------------------------------------

    /// Total word count across the document.
    pub fn word_count(&self) -> usize {
        self.rope
            .chunks()
            .flat_map(|chunk| chunk.unicode_words())
            .count()
    }

    #[allow(dead_code)] // part of the buffer's public API; used by upcoming stats UI
    pub fn char_count(&self) -> usize {
        self.rope.len_chars()
    }
}

//! The text buffer: a rope-backed document with a single cursor.
//!
//! A novel is mostly long runs of prose with occasional edits in the middle, so
//! a rope (`ropey`) gives us cheap inserts/deletes at any offset without copying
//! the whole document around.

use std::path::{Path, PathBuf};

use ropey::Rope;
use tree_sitter::{InputEdit, Point};
use unicode_segmentation::UnicodeSegmentation;

use crate::syntax::Syntax;

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
    /// Live incremental Markdown parse. Edits are *recorded* into it on the
    /// keystroke path; the actual reparse runs via [`Buffer::refresh_syntax`]
    /// once typing goes idle, so a keystroke never blocks on a parse.
    syntax: Syntax,
    /// Set when an edit has been recorded but not yet reparsed.
    syntax_dirty: bool,
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
            syntax: Syntax::new(""),
            syntax_dirty: false,
        }
    }

    /// Load a file from disk, or start an empty buffer pointed at `path` if the
    /// file does not exist yet. The path is normalized to Quill's manuscript
    /// format: a path with no extension gets `.md` (see [`with_default_extension`]).
    pub fn from_path(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = with_default_extension(path.as_ref());
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            syntax: Syntax::new(&text),
            rope: Rope::from_str(&text),
            path: Some(path),
            cursor: Cursor::default(),
            dirty: false,
            goal_col: None,
            syntax_dirty: false,
        })
    }

    /// Persist the buffer to its path. Returns an error if there is no path.
    ///
    /// The manuscript is written as UTF-8 Markdown, unchanged except for one
    /// documented normalization: a non-empty file ends with exactly one
    /// trailing newline (the POSIX/git convention), and an empty document
    /// writes an empty file. Interior bytes — including any line endings — are
    /// left exactly as the author wrote them, so a conforming file round-trips
    /// byte-for-byte.
    pub fn save(&mut self) -> std::io::Result<()> {
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| std::io::Error::other("no file path set"))?;

        let mut content = self.rope.to_string();
        let trimmed_len = content.trim_end_matches('\n').len();
        content.truncate(trimmed_len);
        if !content.is_empty() {
            content.push('\n');
        }

        std::fs::write(path, content)?;
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
        let mut s = self.rope.line(idx).to_string();
        s.truncate(s.trim_end_matches('\n').len());
        s
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

    /// Insert text at a char index and tell the parser exactly what changed, so
    /// the reparse can be incremental. Does not touch the cursor.
    fn edit_insert(&mut self, char_idx: usize, text: &str) {
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
            let text = self.rope.to_string();
            self.syntax.reparse(&text);
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
fn point_at_byte(rope: &Rope, byte: usize) -> Point {
    let row = rope.byte_to_line(byte);
    let column = byte - rope.line_to_byte(row);
    Point { row, column }
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
        std::fs::write(path, bytes).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn round_trip_is_byte_identical_with_trailing_newline() {
        let path = temp_path("roundtrip.md");
        let original = "# Chapter One\n\nThe night was *dark* and **stormy**.\n";
        write(&path, original);

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), original);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_adds_missing_trailing_newline() {
        let path = temp_path("no-newline.md");
        write(&path, "No newline at end");

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), "No newline at end\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn save_collapses_multiple_trailing_newlines_to_one() {
        let path = temp_path("many-newlines.md");
        write(&path, "Body\n\n\n\n");

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), "Body\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn empty_document_saves_empty_file() {
        let path = temp_path("empty.md");
        write(&path, "");

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), "");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn non_ascii_round_trips_exactly() {
        let path = temp_path("unicode.md");
        let original = "Café — résumé, naïve façade. 日本語のテキスト. 🚀✍️🌙\n";
        write(&path, original);

        let mut buf = Buffer::from_path(&path).unwrap();
        buf.save().unwrap();

        assert_eq!(read(&path), original);
        std::fs::remove_file(&path).ok();
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
        std::fs::remove_file(&path).ok();
    }

    #[test]
    #[ignore]
    fn reparse_is_fast_at_window_size() {
        use std::time::Instant;

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
        buf.insert_char('X');

        let t = Instant::now();
        buf.refresh_syntax();
        let dt = t.elapsed();

        println!("reparse on {words} words = {dt:?}");
        assert!(dt.as_millis() < 16, "reparse took {dt:?}");
        std::fs::remove_file(&path).ok();
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
        std::fs::remove_file(&path).ok();
    }
}

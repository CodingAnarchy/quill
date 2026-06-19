//! The text buffer: a rope-backed document with a single cursor.
//!
//! A novel is mostly long runs of prose with occasional edits in the middle, so
//! a rope (`ropey`) gives us cheap inserts/deletes at any offset without copying
//! the whole document around.

use std::fs;
use std::path::{Path, PathBuf};

use ropey::Rope;
use tree_sitter::{InputEdit, Point};
use unicode_segmentation::UnicodeSegmentation;

use crate::frontmatter::FrontMatter;
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
    /// The chapter file's YAML front matter, kept off the rope so the editor
    /// shows only the prose and re-attached on save. `None` when the file has no
    /// front matter block (one is created lazily when a field is first set).
    front_matter: Option<FrontMatter>,
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
            goal_col: None,
            syntax: Syntax::new(""),
            syntax_dirty: false,
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
            goal_col: None,
            syntax_dirty: false,
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
            goal_col: None,
            syntax_dirty: false,
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
        // Count over the whole text, not per rope chunk: a word straddling a
        // chunk boundary would otherwise be counted twice.
        count_words(&self.rope.to_string())
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
}

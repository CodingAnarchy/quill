//! The continuous-manuscript view: a window of a few adjacent chapters
//! concatenated into one editable [`Buffer`], so the author can read and edit
//! across chapter boundaries as if it were a single document — without loading
//! the whole novel.
//!
//! At most [`WINDOW`] chapters are held in memory at once. As the cursor crosses
//! into a new chapter, [`Manuscript::reslide`] pages chapters in and out around
//! it. Each chapter occupies a contiguous run of lines in the buffer (a
//! [`Segment`]); edits are attributed to the segment under the cursor and
//! written back to that chapter's file on [`Manuscript::save`].
//!
//! Chapter boundaries are **fixed seams**: the cursor moves across them freely,
//! but an edit never merges or splits chapters (a Backspace at a chapter's first
//! position is a no-op). So every edit belongs to exactly one chapter file, and
//! restructuring stays in the nav pane. The centered title dividers the renderer
//! draws between chapters are display-only and never enter the buffer.

use crate::buffer::{count_words, normalize_trailing_newline, Buffer};
use crate::project::Project;

/// Maximum chapters held in memory at once (a window around the edit point).
const WINDOW: usize = 3;

/// One loaded chapter's span within the concatenated [`Manuscript`] buffer.
struct Segment {
    /// Manifest index of this chapter.
    index: usize,
    /// Display title, for the divider the renderer draws above it.
    title: String,
    /// Number of lines this chapter occupies in the buffer (always ≥ 1, so even
    /// an empty chapter is editable).
    lines: usize,
    /// Whether this chapter has edits not yet written to its file.
    dirty: bool,
}

pub struct Manuscript {
    /// The loaded window: contiguous chapters, `≤ WINDOW` of them.
    segments: Vec<Segment>,
    /// Concatenation of the window's chapters — the text actually edited.
    buffer: Buffer,
}

impl Manuscript {
    /// Open the continuous view, windowed around the project's active chapter,
    /// cursor at that chapter's start.
    pub fn open(project: &Project) -> Self {
        let active = project.active_index();
        let (lo, hi) = window_bounds(active, chapter_count(project));
        let mut m = Manuscript {
            segments: Vec::new(),
            buffer: Buffer::from_text(""),
        };
        m.rebuild(project, lo, hi, active, 0, 0);
        m
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// Any unsaved edits in the window?
    pub fn dirty(&self) -> bool {
        self.segments.iter().any(|s| s.dirty)
    }

    /// The segment the cursor is in.
    fn current_segment(&self) -> &Segment {
        &self.segments[self.owner_of_line(self.buffer.cursor.line)]
    }

    /// Manifest index of the chapter the cursor is in.
    pub fn current_chapter(&self) -> usize {
        self.current_segment().index
    }

    /// Title of the chapter the cursor is in (for the status bar).
    pub fn current_title(&self) -> &str {
        &self.current_segment().title
    }

    /// Where the renderer should draw a chapter divider: the global start line
    /// and (borrowed) title of each segment after the first.
    pub fn dividers(&self) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, s) in self.segments.iter().enumerate() {
            if i > 0 {
                out.push((start, s.title.as_str()));
            }
            start += s.lines;
        }
        out
    }

    // --- Editing -----------------------------------------------------------

    /// Insert a character at the cursor, attributing it to the chapter the
    /// cursor is in.
    pub fn insert_char(&mut self, c: char) {
        let seg = self.owner_of_line(self.buffer.cursor.line);
        self.buffer.insert_char(c);
        if c == '\n' {
            self.segments[seg].lines += 1;
        }
        self.segments[seg].dirty = true;
    }

    /// Delete the grapheme before the cursor. A Backspace at a chapter's first
    /// position is a no-op — chapters are fixed seams and never merge.
    pub fn backspace(&mut self) {
        let line = self.buffer.cursor.line;
        let col = self.buffer.cursor.col;
        let seg = self.owner_of_line(line);
        let seg_start = self.segment_start_line(seg);

        // At the very top of a non-first chapter: this would join into the
        // previous chapter. Fixed seam — refuse.
        if col == 0 && line == seg_start && seg > 0 {
            return;
        }

        // A col-0 backspace above the segment's first line joins two lines
        // *within* this chapter (the seam guard ruled out crossing one).
        let joins_line = col == 0 && line > seg_start;
        self.buffer.backspace();
        if joins_line {
            self.segments[seg].lines -= 1;
        }
        self.segments[seg].dirty = true;
    }

    // --- Saving ------------------------------------------------------------

    /// Write every chapter with unsaved edits back to its file (splitting the
    /// buffer at segment boundaries) and refresh its cached word count. Clean
    /// chapters are not rewritten.
    pub fn save(&mut self, project: &mut Project) -> std::io::Result<()> {
        let mut start = 0;
        for s in &mut self.segments {
            let end = start + s.lines;
            if s.dirty {
                let raw = self.buffer.line_range_text(start, end);
                let content = normalize_trailing_newline(&raw);
                std::fs::write(project.chapter_path(s.index), &content)?;
                project.set_chapter_words(s.index, count_words(&content));
                s.dirty = false;
            }
            start = end;
        }
        Ok(())
    }

    // --- Windowing ---------------------------------------------------------

    /// Re-center the window on the chapter the cursor is now in, paging chapters
    /// in and out so at most [`WINDOW`] are ever held. A no-op while the cursor
    /// stays within the loaded window's center.
    pub fn reslide(&mut self, project: &mut Project) -> std::io::Result<()> {
        let line = self.buffer.cursor.line;
        let seg = self.owner_of_line(line);
        let current = self.segments[seg].index;
        let (lo, hi) = window_bounds(current, chapter_count(project));
        if lo == self.window_lo() && hi == self.window_hi() {
            return Ok(());
        }
        // The window is rebuilt from disk, so flush every pending edit first —
        // otherwise reloading a chapter that stays in view would lose its edits.
        let local = line - self.segment_start_line(seg);
        let col = self.buffer.cursor.col;
        self.save(project)?;
        self.rebuild(project, lo, hi, current, local, col);
        Ok(())
    }

    /// Load chapters `[lo, hi)` into a fresh buffer, rebuild the segment table,
    /// and restore the cursor to `keep_chapter` at `keep_local`/`keep_col`.
    fn rebuild(
        &mut self,
        project: &Project,
        lo: usize,
        hi: usize,
        keep_chapter: usize,
        keep_local: usize,
        keep_col: usize,
    ) {
        let mut text = String::new();
        let mut segments = Vec::new();
        for index in lo..hi {
            let raw = std::fs::read_to_string(project.chapter_path(index)).unwrap_or_default();
            // Give every chapter at least one line so it's always editable; an
            // empty chapter becomes a single empty line (save normalizes it back).
            let mut content = normalize_trailing_newline(&raw);
            if content.is_empty() {
                content.push('\n');
            }
            let lines = content.matches('\n').count();
            let title = project.chapter_title(index).unwrap_or_default().to_string();
            text.push_str(&content);
            segments.push(Segment {
                index,
                title,
                lines,
                dirty: false,
            });
        }

        self.buffer = Buffer::from_text(&text);
        self.segments = segments;

        // Restore the cursor: the kept chapter's start line plus the local line.
        let seg = self
            .segments
            .iter()
            .position(|s| s.index == keep_chapter)
            .unwrap_or(0);
        let line = self.segment_start_line(seg);
        self.buffer.cursor.line = (line + keep_local).min(self.buffer.line_count() - 1);
        self.buffer.cursor.col = keep_col;
    }

    // --- Line ↔ segment mapping --------------------------------------------

    /// The segment owning a global line. The final (trailing) line maps to the
    /// last segment.
    fn owner_of_line(&self, line: usize) -> usize {
        let mut acc = 0;
        for (i, s) in self.segments.iter().enumerate() {
            if line < acc + s.lines {
                return i;
            }
            acc += s.lines;
        }
        self.segments.len() - 1
    }

    /// The global line where segment `seg` begins.
    fn segment_start_line(&self, seg: usize) -> usize {
        self.segments[..seg].iter().map(|s| s.lines).sum()
    }

    fn window_lo(&self) -> usize {
        self.segments[0].index
    }

    fn window_hi(&self) -> usize {
        self.segments[self.segments.len() - 1].index + 1
    }
}

fn chapter_count(project: &Project) -> usize {
    project.manifest.chapters.len()
}

/// The window `[lo, hi)` of up to [`WINDOW`] chapters centered on `center`,
/// clamped to `[0, count)` (and pulled back from the end so it stays full).
fn window_bounds(center: usize, count: usize) -> (usize, usize) {
    if count <= WINDOW {
        return (0, count);
    }
    let lo = center.saturating_sub(WINDOW / 2);
    let hi = (lo + WINDOW).min(count);
    (hi - WINDOW, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Cursor;
    use std::path::PathBuf;

    /// Build a temp project whose chapters hold the given contents, and return
    /// the loaded `Project` plus its directory.
    fn project_with(name: &str, chapters: &[&str]) -> (Project, PathBuf) {
        let mut dir = std::env::temp_dir();
        dir.push(format!("quill-ms-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut manifest = String::from("title = \"Book\"\n\n");
        for (i, body) in chapters.iter().enumerate() {
            let file = format!("ch{i}.md");
            manifest.push_str(&format!("[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"));
            std::fs::write(dir.join(&file), body).unwrap();
        }
        std::fs::write(Project::manifest_path(&dir), manifest).unwrap();
        (Project::open(&dir).unwrap(), dir)
    }

    fn lines_sum(m: &Manuscript) -> usize {
        m.segments.iter().map(|s| s.lines).sum()
    }

    #[test]
    fn window_bounds_centers_and_clamps() {
        assert_eq!(window_bounds(0, 2), (0, 2)); // fewer than WINDOW: all of it
        assert_eq!(window_bounds(0, 5), (0, 3)); // at the start
        assert_eq!(window_bounds(2, 5), (1, 4)); // centered
        assert_eq!(window_bounds(4, 5), (2, 5)); // pulled back from the end
    }

    #[test]
    fn open_loads_a_window_and_line_sums_match() {
        let (project, dir) = project_with("open", &["a1\na2\n", "b1\n", "c1\n", "d1\n"]);
        let m = Manuscript::open(&project); // active is chapter 0

        // Only WINDOW chapters loaded, never the whole novel.
        assert_eq!(m.segments.len(), WINDOW);
        assert_eq!(m.window_lo(), 0);
        assert_eq!(m.window_hi(), 3);
        // Invariant: per-segment line counts add up to the buffer's content lines.
        assert_eq!(lines_sum(&m), m.buffer.line_count() - 1); // +1 trailing line

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn maps_global_line_to_the_right_chapter() {
        let (project, dir) = project_with("map", &["a1\na2\n", "b1\n", "c1\n"]);
        let m = Manuscript::open(&project);
        // Segments: a = lines [0,2), b = [2,3), c = [3,4).
        assert_eq!(m.segments[m.owner_of_line(0)].index, 0);
        assert_eq!(m.segments[m.owner_of_line(1)].index, 0);
        assert_eq!(m.segments[m.owner_of_line(2)].index, 1);
        assert_eq!(m.segments[m.owner_of_line(3)].index, 2);
        // The trailing line maps to the last segment.
        assert_eq!(m.segments[m.owner_of_line(99)].index, 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_marks_only_the_owning_chapter_and_saves_just_that_file() {
        let (mut project, dir) = project_with("edit", &["alpha\n", "beta\n", "gamma\n"]);
        let mut m = Manuscript::open(&project);

        // Put the cursor in chapter 1 ("beta", global line 1) and type.
        m.buffer.cursor = Cursor { line: 1, col: 4 };
        for c in "!!".chars() {
            m.insert_char(c);
        }
        // Only chapter 1 is dirty.
        assert!(!m.segments[0].dirty);
        assert!(m.segments[1].dirty);
        assert!(!m.segments[2].dirty);

        m.save(&mut project).unwrap();
        // The edit landed in chapter 1's file; the others are untouched.
        assert_eq!(std::fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta!!\n");
        assert_eq!(std::fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
        assert_eq!(std::fs::read_to_string(dir.join("ch2.md")).unwrap(), "gamma\n");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backspace_at_a_chapter_start_is_a_fixed_seam() {
        let (mut project, dir) = project_with("seam", &["alpha\n", "beta\n"]);
        let mut m = Manuscript::open(&project);

        // Cursor at the first position of chapter 1 ("beta", line 1, col 0).
        m.buffer.cursor = Cursor { line: 1, col: 0 };
        m.backspace(); // would merge into chapter 0 — must do nothing
        m.save(&mut project).unwrap();

        // Both chapters intact and separate.
        assert_eq!(std::fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
        assert_eq!(std::fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
        assert!(!m.dirty()); // the refused edit didn't mark anything dirty...

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scrolling_pages_chapters_in_and_out_and_saves_dropped_edits() {
        let (mut project, dir) =
            project_with("slide", &["a\n", "b\n", "c\n", "d\n", "e\n"]);
        let mut m = Manuscript::open(&project); // window [0,3)
        assert_eq!((m.window_lo(), m.window_hi()), (0, 3));

        // Edit chapter 0, then step the cursor into chapter 2 (the last loaded)
        // and reslide — that's what pages the window forward.
        m.insert_char('X'); // chapter 0 dirty: "Xa"
        m.buffer.cursor = Cursor { line: 2, col: 0 }; // into chapter 2 ("c")
        assert_eq!(m.current_chapter(), 2);
        m.reslide(&mut project).unwrap();

        // Window paged forward to center on chapter 2; still only WINDOW loaded.
        assert_eq!(m.segments.len(), WINDOW);
        assert_eq!((m.window_lo(), m.window_hi()), (1, 4));
        // Chapter 0 dropped out — its pending edit was flushed to disk first.
        assert_eq!(std::fs::read_to_string(dir.join("ch0.md")).unwrap(), "Xa\n");
        // The cursor still sits in chapter 2.
        assert_eq!(m.current_chapter(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}

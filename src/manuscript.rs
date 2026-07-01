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

use std::fs;

use crate::atomic;
use crate::buffer::{count_words, normalize_trailing_newline, Buffer};
use crate::frontmatter::{self, Epigraph, FrontMatter, Role};
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
    /// The chapter file's front matter, kept out of the flowing text and
    /// re-attached when this chapter is written back. `None` if it has none.
    front_matter: Option<FrontMatter>,
    /// Whether this chapter has edits not yet written to its file.
    dirty: bool,
    /// The part or front/back-matter section this chapter belongs to (#412), for
    /// the render-only structure header the continuous view draws at each
    /// transition. `None` for a body chapter that's in no part.
    structure: Option<Structure>,
}

/// A chapter's structural grouping for the continuous view's render-only header
/// (#412): a manifest part, or a front/back-matter section. `label` is what the
/// renderer shows; `key` identifies the grouping so the header is drawn only
/// where the structure changes, never repeated within the same part/section.
#[derive(Clone, PartialEq)]
struct Structure {
    label: String,
    key: StructKey,
}

#[derive(Clone, PartialEq)]
enum StructKey {
    Part(usize),
    Section(Role),
}

/// The structural grouping of chapter `index` for the continuous header (#412):
/// its front/back-matter section if it has a role, else the manifest part it
/// falls in, else `None` (a plain body chapter in no part). Derived from cached
/// manifest metadata — no file reads, so it stays off the draw path's budget.
fn structure_of(project: &Project, index: usize) -> Option<Structure> {
    if let Some(role) = project.chapter_role(index) {
        return Some(Structure {
            label: role.label().to_string(),
            key: StructKey::Section(role),
        });
    }
    let pi = project.manifest.part_of(index)?;
    let label = project.book_part_display_name(project.active_book(), pi);
    Some(Structure {
        label,
        key: StructKey::Part(pi),
    })
}

pub struct Manuscript {
    /// The loaded window: contiguous chapters, `≤ WINDOW` of them.
    segments: Vec<Segment>,
    /// Concatenation of the window's chapters — the text actually edited.
    buffer: Buffer,
}

/// What the editor needs to render a chapter's header in the continuous flow.
pub struct SegmentHeader {
    /// Global line where the chapter begins.
    pub start: usize,
    /// Manifest index of the chapter.
    pub index: usize,
    /// Divider title to draw above it, or `None` for the first loaded chapter.
    pub title: Option<String>,
    /// The part/section name to draw above the divider (#412), set only where the
    /// structure changes (the first loaded chapter, or a part/section boundary);
    /// `None` to repeat nothing within the same part/section.
    pub structure: Option<String>,
    /// The chapter's epigraph, if any.
    pub epigraph: Option<Epigraph>,
}

impl Manuscript {
    /// Open the continuous view, windowed around the project's active chapter,
    /// cursor at that chapter's start. Fails if a chapter in the window can't be
    /// read (#154), rather than presenting it as empty.
    pub fn open(project: &Project) -> std::io::Result<Self> {
        let active = project.active_index();
        let (lo, hi) = window_bounds(active, chapter_count(project));
        let mut m = Manuscript {
            segments: Vec::new(),
            buffer: Buffer::from_text(""),
        };
        m.rebuild(project, lo, hi, active, 0, 0)?;
        Ok(m)
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

    /// What the editor draws above each loaded chapter: the global start line,
    /// the chapter index, a divider title (`None` for the first segment, so no
    /// rule is drawn before it), and the chapter's epigraph.
    pub fn segment_headers(&self) -> Vec<SegmentHeader> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut prev: Option<&Structure> = None;
        for (i, s) in self.segments.iter().enumerate() {
            // Show the structure name at each transition: above the first loaded
            // chapter (orientation — "you're in Part Two"), and wherever the
            // part/section changes from the chapter above. Never repeated within
            // the same grouping (#412).
            let structure = s
                .structure
                .as_ref()
                .filter(|cur| prev.is_none_or(|p| p.key != cur.key))
                .map(|cur| cur.label.clone());
            out.push(SegmentHeader {
                start,
                index: s.index,
                title: (i > 0).then(|| s.title.clone()),
                structure,
                epigraph: s.front_matter.as_ref().and_then(FrontMatter::epigraph),
            });
            prev = s.structure.as_ref();
            start += s.lines;
        }
        out
    }

    /// The manifest indices of the loaded chapters, in window order (#370). A
    /// stable signature for the window — it changes when the window pages, not
    /// when prose is edited — so the app knows when to reload annotations.
    pub fn loaded_chapters(&self) -> Vec<usize> {
        self.segments.iter().map(|s| s.index).collect()
    }

    /// Each loaded chapter's byte span in the concatenated buffer (#370):
    /// `(manifest index, start_byte, end_byte)`, in window order. Lets the app
    /// translate annotations between buffer-absolute and chapter-local offsets.
    pub fn segment_spans(&self) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::with_capacity(self.segments.len());
        let mut start_line = 0;
        for s in &self.segments {
            let start_byte = self.buffer.line_start_byte(start_line);
            let end_line = start_line + s.lines;
            let end_byte = if end_line >= self.buffer.line_count() {
                self.buffer.len_bytes()
            } else {
                self.buffer.line_start_byte(end_line)
            };
            out.push((s.index, start_byte, end_byte));
            start_line = end_line;
        }
        out
    }

    /// Whether the next [`reslide`](Self::reslide) would page the window (and so
    /// rebuild the buffer), as opposed to a no-op (#370). The app persists
    /// annotations before a rebuild, since it resets every buffer offset.
    pub fn would_reslide(&self, project: &Project) -> bool {
        let seg = self.owner_of_line(self.buffer.cursor.line);
        let current = self.segments[seg].index;
        let (lo, hi) = window_bounds(current, chapter_count(project));
        lo != self.window_lo() || hi != self.window_hi()
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

    /// Delete the grapheme at the cursor (forward Delete). At a chapter's last
    /// position it would join into the next chapter; that's a fixed seam, so it's
    /// a no-op. A join within the chapter drops one line from its count.
    pub fn delete_forward(&mut self) {
        let line = self.buffer.cursor.line;
        let seg = self.owner_of_line(line);
        let seg_end = self.segment_start_line(seg) + self.segments[seg].lines; // one past the last line
        let at_line_end = self.buffer.cursor.col >= self.buffer.line_len();

        // On the last line of this chapter, at its end: this would pull the next
        // chapter up. Fixed seam — refuse.
        if at_line_end && line + 1 >= seg_end {
            return;
        }
        if self.buffer.delete_forward() {
            self.segments[seg].lines -= 1;
        }
        self.segments[seg].dirty = true;
    }

    /// Delete the previous word (Ctrl/Alt-Backspace). The deletion is clamped to
    /// this chapter's start so it never reaches across the fixed seam into the
    /// chapter above; each newline it removes drops a line from the count.
    pub fn delete_word_back(&mut self) {
        let line = self.buffer.cursor.line;
        let col = self.buffer.cursor.col;
        let seg = self.owner_of_line(line);
        let seg_start = self.segment_start_line(seg);

        // Already at this chapter's first position — a word-delete would only have
        // the previous chapter to reach into. Fixed seam — refuse.
        if col == 0 && line == seg_start && seg > 0 {
            return;
        }

        let before = self.buffer.line_count();
        if self.buffer.word_back_target().line < seg_start {
            // The word runs back across the seam into the chapter above. Delete
            // only down to this chapter's first position instead of crossing it.
            while (self.buffer.cursor.line, self.buffer.cursor.col) != (seg_start, 0) {
                self.buffer.backspace();
            }
        } else {
            self.buffer.delete_word_back();
        }
        let removed = before.saturating_sub(self.buffer.line_count());
        self.segments[seg].lines = self.segments[seg].lines.saturating_sub(removed);
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
                let body = normalize_trailing_newline(&raw);
                let front = s
                    .front_matter
                    .as_ref()
                    .filter(|f| !f.is_empty())
                    .map(FrontMatter::as_str);
                let content = frontmatter::join(front, &body);
                atomic::write(&project.chapter_path(s.index), content.as_bytes())?;
                // Count the prose only, not the front matter keys.
                project.set_chapter_words(s.index, count_words(&body));
                s.dirty = false;
            }
            start = end;
        }
        Ok(())
    }

    // --- Recovery ----------------------------------------------------------

    /// Each dirty segment's `(manifest index, prose body)`, snapshotting the
    /// in-memory buffer — not the on-disk files (#566). The body is normalized
    /// the same way [`save`](Self::save) computes it, with front matter left out
    /// (recovery captures prose only, re-attaching front matter on save), so a
    /// recovered body compares cleanly against the chapter's on-disk prose.
    ///
    /// Runs on the idle cadence, so it does only buffer slicing over the loaded
    /// window — no file reads, no work proportional to the novel.
    pub fn dirty_segments(&self) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        let mut start = 0;
        for s in &self.segments {
            let end = start + s.lines;
            if s.dirty {
                let raw = self.buffer.line_range_text(start, end);
                out.push((s.index, normalize_trailing_newline(&raw)));
            }
            start = end;
        }
        out
    }

    /// Overlay recovered prose onto the loaded window (#566): for each
    /// `(manifest index, body)` whose chapter is currently loaded, replace that
    /// segment's text with the recovered body and mark it dirty, so the recovered
    /// work sits in the editable buffer for review and a save persists it — never
    /// writing files behind the author's back, mirroring single-chapter restore.
    /// Returns the indices actually restored (those still in the window). The
    /// cursor lands at the start of the first restored chapter.
    pub fn restore_bodies(&mut self, chapters: &[(usize, String)]) -> Vec<usize> {
        let mut restored = Vec::new();
        let mut text = String::new();
        let mut first_start_line: Option<usize> = None;
        let mut start_line = 0;
        for s in &mut self.segments {
            let recovered = chapters.iter().find(|(idx, _)| *idx == s.index);
            let mut content = match recovered {
                Some((_, body)) => normalize_trailing_newline(body),
                None => {
                    let end = start_line + s.lines;
                    self.buffer.line_range_text(start_line, end)
                }
            };
            start_line += s.lines;
            if content.is_empty() {
                content.push('\n');
            }
            if recovered.is_some() {
                s.dirty = true;
                restored.push(s.index);
                first_start_line.get_or_insert(text.matches('\n').count());
            }
            s.lines = content.matches('\n').count();
            text.push_str(&content);
        }
        self.buffer = Buffer::from_text(&text);
        let line = first_start_line.unwrap_or(0);
        self.buffer.cursor.line = line.min(self.buffer.line_count().saturating_sub(1));
        self.buffer.cursor.col = 0;
        restored
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
        self.rebuild(project, lo, hi, current, local, col)
    }

    /// Load chapters `[lo, hi)` into a fresh buffer, rebuild the segment table,
    /// and restore the cursor to `keep_chapter` at `keep_local`/`keep_col`.
    ///
    /// All chapters are read up front; if any read fails (#154) the error is
    /// returned and `self` is left untouched, so an unreadable chapter is never
    /// loaded as empty text the author could then save over its real content.
    fn rebuild(
        &mut self,
        project: &Project,
        lo: usize,
        hi: usize,
        keep_chapter: usize,
        keep_local: usize,
        keep_col: usize,
    ) -> std::io::Result<()> {
        let mut text = String::new();
        let mut segments = Vec::new();
        for index in lo..hi {
            // Propagate any read failure before mutating self — every manifest
            // chapter has a file (created on add), so a failure here means the
            // file is gone or unreadable, not a legitimately empty chapter.
            let raw = fs::read_to_string(project.chapter_path(index))?;
            // Front matter stays out of the flowing text; keep it to re-attach on save.
            let (front, body) = frontmatter::split(&raw);
            // Give every chapter at least one line so it's always editable; an
            // empty chapter becomes a single empty line (save normalizes it back).
            let mut content = normalize_trailing_newline(body);
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
                front_matter: front.map(FrontMatter::from_raw),
                dirty: false,
                structure: structure_of(project, index),
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
        Ok(())
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
        let dir = crate::testutil::scratch_dir("ms", name);

        let mut manifest = String::from("title = \"Book\"\n\n");
        for (i, body) in chapters.iter().enumerate() {
            let file = format!("ch{i}.md");
            manifest.push_str(&format!(
                "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
            ));
            fs::write(dir.join(&file), body).unwrap();
        }
        fs::write(Project::manifest_path(&dir), manifest).unwrap();
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
        let m = Manuscript::open(&project).unwrap(); // active is chapter 0

        // Only WINDOW chapters loaded, never the whole novel.
        assert_eq!(m.segments.len(), WINDOW);
        assert_eq!(m.window_lo(), 0);
        assert_eq!(m.window_hi(), 3);
        // Invariant: per-segment line counts add up to the buffer's content lines.
        assert_eq!(lines_sum(&m), m.buffer.line_count() - 1); // +1 trailing line

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn front_matter_stays_out_of_the_flow_and_is_re_attached_on_save() {
        let (mut project, dir) = project_with(
            "frontmatter",
            &["---\nstatus: draft\n---\nAlpha prose.\n", "Beta prose.\n"],
        );
        let mut m = Manuscript::open(&project).unwrap();

        // The concatenated buffer holds only prose — no YAML in the flow.
        let flow = m.buffer.line_range_text(0, m.buffer.line_count());
        assert!(flow.contains("Alpha prose."), "flow:\n{flow}");
        assert!(
            !flow.contains("status:"),
            "front matter leaked into flow:\n{flow}"
        );

        // Edit chapter 0 and save; its front matter is re-attached untouched.
        m.insert_char('X');
        m.save(&mut project).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("ch0.md")).unwrap(),
            "---\nstatus: draft\n---\nXAlpha prose.\n"
        );
        // Chapter 1 (no front matter) is untouched.
        assert_eq!(
            fs::read_to_string(dir.join("ch1.md")).unwrap(),
            "Beta prose.\n"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// A four-body-chapter project split into two named parts (#412): body
    /// positions 1-2 are "Part One", 3-4 are "Part Two".
    fn project_with_parts(name: &str) -> (Project, PathBuf) {
        let dir = crate::testutil::scratch_dir("ms", name);
        let manifest = "title = \"Book\"\n\n\
            [[parts]]\nname = \"Part One\"\nchapters = [1, 2]\n\n\
            [[parts]]\nname = \"Part Two\"\nchapters = [3, 4]\n\n\
            [[chapters]]\ntitle = \"Ch0\"\nfile = \"ch0.md\"\n\n\
            [[chapters]]\ntitle = \"Ch1\"\nfile = \"ch1.md\"\n\n\
            [[chapters]]\ntitle = \"Ch2\"\nfile = \"ch2.md\"\n\n\
            [[chapters]]\ntitle = \"Ch3\"\nfile = \"ch3.md\"\n";
        for i in 0..4 {
            fs::write(dir.join(format!("ch{i}.md")), format!("body {i}\n")).unwrap();
        }
        fs::write(Project::manifest_path(&dir), manifest).unwrap();
        (Project::open(&dir).unwrap(), dir)
    }

    #[test]
    fn part_headers_show_at_each_transition_not_repeated() {
        let (mut project, dir) = project_with_parts("part-headers");
        project.set_active(2); // window loads chapters 1, 2, 3 (WINDOW = 3)
        let m = Manuscript::open(&project).unwrap();
        let structures: Vec<_> = m
            .segment_headers()
            .into_iter()
            .map(|h| h.structure)
            .collect();
        assert_eq!(
            structures,
            vec![
                Some("Part One".to_string()), // first loaded chapter — orientation
                Some("Part Two".to_string()), // the part boundary, drawn where it begins
                None,                         // same part as the chapter above — not repeated
            ],
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_first_loaded_chapter_names_its_part_for_orientation() {
        // A window fully inside Part Two still shows "Part Two" up top, so the
        // author flowing through the middle of a part knows where they are.
        let (mut project, dir) = project_with_parts("part-orientation");
        project.set_active(3); // window loads chapters 1, 2, 3 → but center 3 pulls back
        let m = Manuscript::open(&project).unwrap();
        // chapters 1 (Part One), 2 & 3 (Part Two): first header names the part it
        // opens in regardless of whether a boundary is in view.
        let first = &m.segment_headers()[0];
        assert!(
            first.structure.is_some(),
            "the first loaded chapter names its part"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// A project whose first chapter is a front-matter section (a prologue),
    /// followed by plain body chapters in no part (#412).
    fn project_with_section(name: &str) -> (Project, PathBuf) {
        let dir = crate::testutil::scratch_dir("ms", name);
        let manifest = "title = \"Book\"\n\n\
            [[chapters]]\ntitle = \"Prologue\"\nfile = \"ch0.md\"\n\n\
            [[chapters]]\ntitle = \"Ch1\"\nfile = \"ch1.md\"\n\n\
            [[chapters]]\ntitle = \"Ch2\"\nfile = \"ch2.md\"\n";
        fs::write(dir.join("ch0.md"), "---\nrole: prologue\n---\nopening\n").unwrap();
        fs::write(dir.join("ch1.md"), "body 1\n").unwrap();
        fs::write(dir.join("ch2.md"), "body 2\n").unwrap();
        fs::write(Project::manifest_path(&dir), manifest).unwrap();
        (Project::open(&dir).unwrap(), dir)
    }

    #[test]
    fn section_headers_name_front_matter_and_body_chapters_show_nothing() {
        let (project, dir) = project_with_section("section-header");
        let m = Manuscript::open(&project).unwrap(); // active is chapter 0
        let structures: Vec<_> = m
            .segment_headers()
            .into_iter()
            .map(|h| h.structure)
            .collect();
        assert_eq!(
            structures,
            vec![
                Some("Prologue".to_string()), // the front-matter section names itself
                None,                         // a body chapter in no part has no header
                None,
            ],
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unstructured_project_draws_no_structure_headers() {
        // The common case: no parts, no sections — the feature is invisible.
        let (project, dir) = project_with("plain", &["a\n", "b\n", "c\n"]);
        let m = Manuscript::open(&project).unwrap();
        assert!(m.segment_headers().iter().all(|h| h.structure.is_none()));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn maps_global_line_to_the_right_chapter() {
        let (project, dir) = project_with("map", &["a1\na2\n", "b1\n", "c1\n"]);
        let m = Manuscript::open(&project).unwrap();
        // Segments: a = lines [0,2), b = [2,3), c = [3,4).
        assert_eq!(m.segments[m.owner_of_line(0)].index, 0);
        assert_eq!(m.segments[m.owner_of_line(1)].index, 0);
        assert_eq!(m.segments[m.owner_of_line(2)].index, 1);
        assert_eq!(m.segments[m.owner_of_line(3)].index, 2);
        // The trailing line maps to the last segment.
        assert_eq!(m.segments[m.owner_of_line(99)].index, 2);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_marks_only_the_owning_chapter_and_saves_just_that_file() {
        let (mut project, dir) = project_with("edit", &["alpha\n", "beta\n", "gamma\n"]);
        let mut m = Manuscript::open(&project).unwrap();

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
        assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta!!\n");
        assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
        assert_eq!(fs::read_to_string(dir.join("ch2.md")).unwrap(), "gamma\n");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backspace_at_a_chapter_start_is_a_fixed_seam() {
        let (mut project, dir) = project_with("seam", &["alpha\n", "beta\n"]);
        let mut m = Manuscript::open(&project).unwrap();

        // Cursor at the first position of chapter 1 ("beta", line 1, col 0).
        m.buffer.cursor = Cursor { line: 1, col: 0 };
        m.backspace(); // would merge into chapter 0 — must do nothing
        m.save(&mut project).unwrap();

        // Both chapters intact and separate.
        assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
        assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
        assert!(!m.dirty()); // the refused edit didn't mark anything dirty...

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn forward_delete_at_a_chapter_end_is_a_fixed_seam() {
        let (mut project, dir) = project_with("fwd-seam", &["alpha\n", "beta\n"]);
        let mut m = Manuscript::open(&project).unwrap();

        // Cursor at the last position of chapter 0 ("alpha", line 0, col 5).
        m.buffer.cursor = Cursor { line: 0, col: 5 };
        m.delete_forward(); // would pull chapter 1 up — must do nothing
        m.save(&mut project).unwrap();

        assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
        assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn forward_delete_within_a_chapter_removes_the_char_ahead() {
        let (mut project, dir) = project_with("fwd", &["alpha\n", "beta\n"]);
        let mut m = Manuscript::open(&project).unwrap();

        m.buffer.cursor = Cursor { line: 0, col: 0 };
        m.delete_forward(); // drop the 'a' of "alpha"
        m.save(&mut project).unwrap();

        assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "lpha\n");
        assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn word_delete_back_stops_at_the_chapter_seam() {
        let (mut project, dir) = project_with("word-seam", &["alpha\n", "beta\n"]);
        let mut m = Manuscript::open(&project).unwrap();

        // Cursor at the first position of chapter 1 ("beta", line 1, col 0) — a
        // word-delete here has only chapter 0 to reach back into. Must not cross.
        m.buffer.cursor = Cursor { line: 1, col: 0 };
        m.delete_word_back();
        m.save(&mut project).unwrap();

        assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "alpha\n");
        assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "beta\n");

        // From inside chapter 1, it deletes the word within that chapter.
        m.buffer.cursor = Cursor { line: 1, col: 4 }; // after "beta"
        m.delete_word_back();
        m.save(&mut project).unwrap();
        // "beta" is gone; an emptied chapter body normalizes to an empty file.
        assert_eq!(fs::read_to_string(dir.join("ch1.md")).unwrap(), "");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unreadable_chapter_fails_rather_than_loading_empty() {
        let (project, dir) = project_with("unreadable", &["alpha\n", "beta\n"]);
        // The file vanishes out from under the view (removed / permission lost).
        fs::remove_file(dir.join("ch1.md")).unwrap();
        // Opening the window must surface the failure, not load ch1 as empty text
        // that a later save could write back over the (now-missing) file (#154).
        assert!(Manuscript::open(&project).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reslide_keeps_the_window_intact_when_a_paged_in_chapter_is_unreadable() {
        let (mut project, dir) = project_with("reslide-fail", &["a\n", "b\n", "c\n", "d\n", "e\n"]);
        let mut m = Manuscript::open(&project).unwrap(); // window [0,3)

        // Make chapter 3 (paged in when we slide forward) unreadable, then move
        // the cursor into chapter 2 and reslide.
        fs::remove_file(dir.join("ch3.md")).unwrap();
        m.buffer.cursor = Cursor { line: 2, col: 0 };
        assert!(
            m.reslide(&mut project).is_err(),
            "the failed read is surfaced"
        );
        // The window is unchanged — the old chapters are still loaded, not blanked.
        assert_eq!((m.window_lo(), m.window_hi()), (0, 3));
        assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "a\n");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scrolling_pages_chapters_in_and_out_and_saves_dropped_edits() {
        let (mut project, dir) = project_with("slide", &["a\n", "b\n", "c\n", "d\n", "e\n"]);
        let mut m = Manuscript::open(&project).unwrap(); // window [0,3)
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
        assert_eq!(fs::read_to_string(dir.join("ch0.md")).unwrap(), "Xa\n");
        // The cursor still sits in chapter 2.
        assert_eq!(m.current_chapter(), 2);

        fs::remove_dir_all(&dir).ok();
    }
}

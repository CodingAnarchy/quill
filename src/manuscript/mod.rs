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

use std::collections::VecDeque;
use std::fs;

use crate::atomic;
use crate::buffer::{count_words, normalize_trailing_newline, Buffer};
use crate::frontmatter::{self, Epigraph, FrontMatter, Role};
use crate::project::Project;
use crate::text::count_matches;

/// Maximum chapters held in memory at once (a window around the edit point).
const WINDOW: usize = 3;

/// A segment's undo/redo bookkeeping, snapshotted alongside the buffer's rope so
/// undo/redo in the continuous view keeps per-chapter line counts and dirty flags
/// consistent (#649). The buffer already snapshots the whole rope per edit group;
/// this captures what the rope alone can't recover — where the chapter seams fall,
/// since the buffer has no seam markers — for each loaded segment, in window order.
#[derive(Clone)]
struct SegState {
    lines: usize,
    dirty: bool,
}

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
    /// Per-segment bookkeeping snapshots that mirror the buffer's undo/redo
    /// stacks one-for-one (#649). The buffer snapshots the rope; these snapshot
    /// the segment `lines`/`dirty` so a restore puts the seams back where they
    /// were. Pushed when a new edit group opens (in lockstep with the buffer),
    /// and popped alongside the buffer's rope on undo/redo.
    undo_states: VecDeque<Vec<SegState>>,
    redo_states: VecDeque<Vec<SegState>>,
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
        let mut m = Self {
            segments: Vec::new(),
            buffer: Buffer::from_text(""),
            undo_states: VecDeque::new(),
            redo_states: VecDeque::new(),
        };
        m.rebuild(project, lo, hi, active, 0, 0)?;
        Ok(m)
    }

    pub const fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub const fn buffer_mut(&mut self) -> &mut Buffer {
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

    /// The current per-segment bookkeeping, in window order — the pre-edit state
    /// an undo returns to (#649).
    fn seg_states(&self) -> Vec<SegState> {
        self.segments
            .iter()
            .map(|s| SegState {
                lines: s.lines,
                dirty: s.dirty,
            })
            .collect()
    }

    /// Push a pre-edit segment snapshot when the buffer just opened a new undo
    /// group (#649). `was_open` is the buffer's group state captured *before* the
    /// edit: a `false → true` transition means the edit opened a fresh group, so
    /// `pre` (the state before it) belongs on the undo stack and any redo history
    /// is now invalid. If the buffer didn't open a group — a no-op edit, or an
    /// edit folded into an already-open group — nothing is pushed, keeping the two
    /// stacks in exact step.
    fn commit_checkpoint(&mut self, was_open: bool, pre: Vec<SegState>) {
        if was_open || !self.buffer.group_open() {
            return;
        }
        self.undo_states.push_back(pre);
        while self.undo_states.len() > Buffer::MAX_UNDO_GROUPS {
            self.undo_states.pop_front();
        }
        self.redo_states.clear();
    }

    /// Drop the segment undo/redo history — the buffer was rebuilt (a reslide or a
    /// recovery restore), resetting its own stack, so the segment stacks reset too
    /// to stay in step (#649).
    fn clear_history(&mut self) {
        self.undo_states.clear();
        self.redo_states.clear();
    }

    /// Insert a character at the cursor, attributing it to the chapter the
    /// cursor is in.
    pub fn insert_char(&mut self, c: char) {
        let seg = self.owner_of_line(self.buffer.cursor.line);
        let pre = self.seg_states();
        let was_open = self.buffer.group_open();
        self.buffer.insert_char(c);
        if c == '\n' {
            self.segments[seg].lines += 1;
        }
        self.segments[seg].dirty = true;
        self.commit_checkpoint(was_open, pre);
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
        let pre = self.seg_states();
        let was_open = self.buffer.group_open();
        self.buffer.backspace();
        if joins_line {
            self.segments[seg].lines -= 1;
        }
        self.segments[seg].dirty = true;
        self.commit_checkpoint(was_open, pre);
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
        let pre = self.seg_states();
        let was_open = self.buffer.group_open();
        if self.buffer.delete_forward() {
            self.segments[seg].lines -= 1;
        }
        self.segments[seg].dirty = true;
        self.commit_checkpoint(was_open, pre);
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
        let pre = self.seg_states();
        let was_open = self.buffer.group_open();
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
        self.commit_checkpoint(was_open, pre);
    }

    // --- Range edits (#700) ------------------------------------------------

    /// The buffer-absolute byte span `[start, end)` of segment `seg`. The end is
    /// the start of the next segment's first line, i.e. one past this chapter's
    /// terminating newline — the fixed seam a range edit must never cross.
    fn segment_byte_span(&self, seg: usize) -> (usize, usize) {
        let start_line = self.segment_start_line(seg);
        let end_line = start_line + self.segments[seg].lines;
        let start = self.buffer.line_start_byte(start_line);
        let end = if end_line >= self.buffer.line_count() {
            self.buffer.len_bytes()
        } else {
            self.buffer.line_start_byte(end_line)
        };
        (start, end)
    }

    /// The byte range a range edit starting at `start` is allowed to touch: the
    /// owning chapter's span, clamped so it never reaches across a fixed chapter
    /// seam (#700). A middle chapter keeps its terminating newline (the seam that
    /// separates it from the chapter below); the last loaded chapter has no
    /// trailing newline of its own, so its whole span is editable. Returns
    /// `(seg, lo, hi)` — the owning segment and the clamped `[lo, hi)`.
    fn clamp_to_chapter(&self, start: usize) -> (usize, usize, usize) {
        let seg = self.owner_of_line(self.buffer.cursor_at_byte(start).line);
        let (seg_start, seg_end) = self.segment_byte_span(seg);
        let is_last = seg + 1 == self.segments.len();
        let max_end = if is_last {
            seg_end
        } else {
            seg_end.saturating_sub(1) // leave the terminating newline in place
        };
        (seg, seg_start.max(start), max_end.max(start))
    }

    /// Replace the buffer-absolute byte range `[start, end)` with `text`, keeping
    /// per-chapter seam/line-count/dirty bookkeeping correct (#700). This is the
    /// range-edit primitive the continuous view's Vim operators and find/replace
    /// route through, generalizing the char-level seam-maintaining edits above.
    ///
    /// The range is attributed to the chapter its `start` falls in and **clamped
    /// to that chapter** — a range that would reach across a fixed chapter seam is
    /// trimmed back so it never touches another chapter's text or the newline that
    /// keeps the chapters apart (seams never merge).
    ///
    /// Returns whether any text actually changed, so a caller can tell a real edit
    /// from a fully-clamped no-op (e.g. a delete aimed only at a seam).
    pub fn replace_byte_range(&mut self, start: usize, end: usize, text: &str) -> bool {
        let start = start.min(self.buffer.len_bytes());
        let end = end.min(self.buffer.len_bytes()).max(start);
        let (seg, lo, hi) = self.clamp_to_chapter(start);
        let start = start.max(lo);
        let end = end.min(hi).max(start);
        if start >= end && text.is_empty() {
            return false;
        }

        // Line-count delta: newlines removed from the chapter vs. newlines added by
        // the replacement. Computed on the exact clamped span so the segment's line
        // count — and thus every seam below it — stays exact.
        let removed = self.buffer.byte_slice(start, end).matches('\n').count();
        let added = text.matches('\n').count();

        let pre = self.seg_states();
        let was_open = self.buffer.group_open();
        self.buffer.replace_byte_range(start, end, text);
        self.segments[seg].lines = self.segments[seg].lines + added - removed;
        self.segments[seg].dirty = true;
        self.commit_checkpoint(was_open, pre);
        true
    }

    // --- Undo / redo (#649) ------------------------------------------------

    /// Undo the most recent edit group. Reverts the buffer's rope and restores
    /// the per-chapter line counts / dirty flags to match, so the seams land
    /// exactly where they were before the edit (#649). Returns `false` when
    /// there's nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.history_step(true)
    }

    /// Re-apply the most recently undone group, restoring the buffer and its
    /// segment bookkeeping together (#649). Returns `false` when there's nothing
    /// to redo.
    pub fn redo(&mut self) -> bool {
        self.history_step(false)
    }

    /// Step the buffer and the segment history one entry in lockstep. The two
    /// stacks are pushed together on every edit, so they pop together here: undo
    /// pops the undo stacks (saving the current state for redo), redo the reverse.
    /// The buffer's step and this one always agree on whether an entry exists, so
    /// they never fall out of sync.
    fn history_step(&mut self, undo: bool) -> bool {
        let current = self.seg_states();
        let restored = if undo {
            self.undo_states.pop_back()
        } else {
            self.redo_states.pop_back()
        };
        let Some(states) = restored else {
            return false;
        };
        // Step the rope; the two stacks are kept one-for-one, so this must succeed.
        let stepped = if undo {
            self.buffer.undo()
        } else {
            self.buffer.redo()
        };
        debug_assert!(stepped, "segment history out of step with the buffer");
        if undo {
            self.redo_states.push_back(current);
        } else {
            self.undo_states.push_back(current);
        }
        // Restore the seams: the window's segments are the same chapters in the
        // same order (a reslide would have rebuilt and cleared history), so a
        // positional restore of each segment's line count / dirty flag is exact.
        for (seg, state) in self.segments.iter_mut().zip(states) {
            seg.lines = state.lines;
            seg.dirty = state.dirty;
        }
        true
    }

    // --- Saving ------------------------------------------------------------

    /// Write every chapter with unsaved edits back to its file (splitting the
    /// buffer at segment boundaries) and refresh its cached word count. Clean
    /// chapters are not rewritten.
    pub fn save(&mut self, project: &mut Project) -> std::io::Result<()> {
        let mut start = 0;
        let last = self.segments.len().saturating_sub(1);
        for (i, s) in self.segments.iter_mut().enumerate() {
            // The concatenated buffer always ends in a single `\n`, so the rope
            // reports one trailing empty line past the last segment's `lines`.
            // That line belongs to no `start + lines` span, yet the cursor can
            // reach it (`G`/repeated Down at the end) and text typed there lands
            // in the rope without growing the last segment — so the last
            // segment's span must run through EOF to capture it, or that prose
            // is silently dropped on save. Mirrors `clamp_to_chapter`'s
            // "last segment owns through EOF" for byte ranges.
            let end = if i == last {
                self.buffer.line_count()
            } else {
                start + s.lines
            };
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
                // Count the prose only, not the front matter keys. The TODO count is
                // refreshed alongside the word count (#867) so the save-path roll-up
                // reads the cache instead of re-reading this chapter from disk.
                project.set_chapter_words(s.index, count_words(&body));
                project.set_chapter_todo(
                    s.index,
                    count_matches(&body, project.manifest.todo_marker()),
                );
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
        let last = self.segments.len().saturating_sub(1);
        for (i, s) in self.segments.iter().enumerate() {
            // The last segment runs through EOF (the buffer's trailing empty
            // line), so prose typed on that orphan line is captured by the
            // recovery snapshot too — matching `save` so recovery never misses
            // what a save would keep (see `save`).
            let end = if i == last {
                self.buffer.line_count()
            } else {
                start + s.lines
            };
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
            let mut content = if let Some((_, body)) = recovered {
                normalize_trailing_newline(body)
            } else {
                let end = start_line + s.lines;
                self.buffer.line_range_text(start_line, end)
            };
            start_line += s.lines;
            if content.is_empty() {
                content.push('\n');
            }
            if recovered.is_some() {
                s.dirty = true;
                restored.push(s.index);
                first_start_line.get_or_insert_with(|| text.matches('\n').count());
            }
            s.lines = content.matches('\n').count();
            text.push_str(&content);
        }
        self.buffer = Buffer::from_text(&text);
        self.clear_history(); // the fresh buffer has no undo history (#649)
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
        // Paging the window rebuilds the buffer from disk (its undo stack resets),
        // and the segment set changes — so undo history can't span the reslide and
        // is dropped, keeping the two stacks in step (#649).
        self.clear_history();

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

    /// The last global line index of the segment (chapter) containing `line`
    /// (#1005 follow-up). The chapters in the window are concatenated with a
    /// single `\n` seam and no blank line, so a plain "blank line or thematic
    /// break" boundary check never sees the seam — a forward-scanning join or
    /// reflow in a chapter's last paragraph would otherwise fold in or gather
    /// the next chapter's first line across it. Callers clamp their forward scan
    /// to this line so they stop at the seam.
    pub fn segment_end_line(&self, line: usize) -> usize {
        let seg = self.owner_of_line(line);
        // `lines` is always ≥ 1, so this never underflows.
        self.segment_start_line(seg) + self.segments[seg].lines - 1
    }

    fn window_lo(&self) -> usize {
        self.segments[0].index
    }

    fn window_hi(&self) -> usize {
        self.segments[self.segments.len() - 1].index + 1
    }
}

const fn chapter_count(project: &Project) -> usize {
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
mod tests;

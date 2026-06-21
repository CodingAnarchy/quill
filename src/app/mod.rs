//! Top-level application state and the key handling that drives editing. The
//! editor-mode types live in [`crate::mode`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use similar::{ChangeTag, TextDiff};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::ai::{self, bible, Backend, ClaudeBackend, Outcome};
use crate::ai::character::{self, Character};
use crate::ai::continuity::{self, Continuity};
use crate::ai::setting::{self, Setting};
use crate::analysis;
use crate::buffer::{self, Buffer};
use crate::date;
use crate::frontmatter::{Band, Epigraph, FrontMatter, Role, Status};
use crate::history;
use crate::manuscript::Manuscript;
use crate::mode::{
    BibleConflict, BibleReview, Command, Finding, Findings, GitAction, MetaEdit, MetaField, Mode,
    Overlay, Palette, PaletteItem, Pending, PromptState, RefFiles,
};
use crate::project::{Manifest, Project};
use crate::report::{self, Momentum, PartDelta};

/// Shown when the author tries to delete a project's last remaining chapter.
mod assist;
mod exporting;
mod search;
mod meta;
mod panes;
mod reports;
mod task;
mod vcs_actions;

use panes::Split;
pub(crate) use panes::{SplitContent, SplitDir};
#[cfg(test)]
pub(crate) use panes::SplitView; // the type name is only spelled in tests
use task::{Poll, Task};
use vcs_actions::VcMsg;

const ONLY_CHAPTER_MSG: &str = "Can't delete the only chapter";


pub struct App {
    pub buffer: Buffer,
    /// The open project (manifest, root, active chapter index). The `buffer`
    /// above holds the active chapter's text.
    pub project: Project,
    /// When `Some`, the continuous-manuscript view is active: editing and
    /// rendering go through this windowed multi-chapter buffer instead of the
    /// single-chapter `buffer` above (which is then the saved, idle chapter).
    pub manuscript: Option<Manuscript>,
    /// The second editor pane (#163), when the window is split. The focused pane
    /// is always `buffer`; this holds the other one.
    split: Option<Split>,
    /// True between a `Ctrl-w` window prefix and the command key that follows it.
    window_pending: bool,
    pub mode: Mode,
    pub should_quit: bool,
    /// Distraction-free centering: target column width for the text, in cells.
    pub focus_width: u16,
    /// Whether the chapter / outline pane is shown.
    pub outline_visible: bool,
    /// Whether the project is under version control (#54). Cached so the status
    /// bar needn't stat the filesystem every frame; set at open and after the
    /// author initializes a repository.
    pub under_vc: bool,
    /// Count of uncommitted changes (#132), cached for the status bar. Refreshed
    /// on idle and after save / commit / init — never per frame. Zero when clean
    /// or not under version control.
    pub vc_changes: usize,
    /// The open full-screen overlay (progress report, analysis report, or AI
    /// answer), if any — read-only, scrollable, dismissed by any other key.
    /// Pre-rendered once on open so it isn't rebuilt per frame. A single slot:
    /// the overlays are mutually exclusive, and a new one is a new
    /// [`crate::mode::Overlay`] value, not a new field.
    pub overlay: Option<Overlay>,
    /// Height of the terminal in rows, refreshed each frame by the renderer.
    /// Lets the key handler clamp overlay scrolling to the content that exists.
    pub viewport_height: u16,
    /// The editor's wrap column (focus-column width in cells), set by the renderer
    /// each frame so visual-row cursor navigation (#143) wraps lines the same way
    /// the screen does. A `Cell` because the renderer borrows `App` immutably.
    pub wrap_width: std::cell::Cell<u16>,
    /// Height of a docked surface pane (#164), set by the renderer so its scroll
    /// bound matches what's drawn.
    pub(crate) surface_height: std::cell::Cell<u16>,
    /// Words inserted / deleted today and the count of words moved out of the
    /// manuscript today (deleted chapters). Seeded from the day's snapshot on
    /// load; fed to the progress report and persisted to the history log.
    added: usize,
    removed: usize,
    moved_to_unused: usize,
    /// Today's added / deleted words per part, in manifest part order (empty
    /// when the manifest defines no parts). The global counters above equal the
    /// sums of these plus any edits in chapters outside every part.
    part_added: Vec<usize>,
    part_removed: Vec<usize>,
    /// Per-chapter word baselines (chapter file → last counted words) the tally
    /// measures against. Keyed by file identity so it survives reordering; an
    /// edit's delta is attributed to the part of the chapter that changed.
    baselines: HashMap<String, usize>,
    /// AI configuration (opt-in, off by default), loaded from the user's home
    /// config. Drives whether the "Ask the manuscript" command is reachable.
    pub ai: ai::Settings,
    /// In-flight AI request (#152), run on a worker thread so it never blocks the
    /// editor. Polled each loop tick by [`App::poll_ai`].
    ai_task: Task<Outcome>,
    /// In-flight remote push/pull (#134/#152), run on a worker thread. Polled each
    /// loop tick by [`App::poll_vc`].
    vc_task: Task<VcMsg>,
    /// The loaded local backend, cached after first use (loading a model is slow)
    /// and shared with the worker thread. Empty until the first local request;
    /// stays empty when the Claude backend is used. Type-erased so it needs no
    /// `local`-feature cfg here.
    local_backend: Arc<Mutex<Option<Arc<dyn Backend>>>>,
    /// Continuity findings waiting to be shown — stashed while a timeline
    /// conflict is resolved first, then opened by [`App::open_findings`].
    pending_findings: Vec<Finding>,
    /// All findings accumulated this session (continuity / setting / style /
    /// naming / gaps), de-duplicated — the "revision triage" list.
    triage: Vec<Finding>,
    /// The bible pass in flight and the chapter digests it was built from (#90),
    /// recorded to the incremental-scoping state file once the pass applies
    /// cleanly. `(tracker, digests)`; `None` when no bible pass is pending.
    pending_pass: Option<(String, ai::incremental::Digests)>,
    /// One-line transient message shown in the status bar.
    pub message: Option<String>,
}

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        let under_vc = crate::vcs::is_repo(&project.root);
        let mut app = Self {
            buffer,
            project,
            manuscript: None,
            split: None,
            window_pending: false,
            mode: Mode::Insert,
            should_quit: false,
            focus_width: 72,
            outline_visible: false,
            under_vc,
            vc_changes: 0,
            overlay: None,
            viewport_height: 0,
            wrap_width: std::cell::Cell::new(72),
            surface_height: std::cell::Cell::new(0),
            added: 0,
            removed: 0,
            moved_to_unused: 0,
            part_added: Vec::new(),
            part_removed: Vec::new(),
            baselines: HashMap::new(),
            ai: ai::Settings::load(),
            ai_task: Task::idle(),
            vc_task: Task::idle(),
            local_backend: Arc::new(Mutex::new(None)),
            pending_findings: Vec::new(),
            triage: Vec::new(),
            pending_pass: None,
            message: None,
        };
        // Baselines for the daily word tally: edits move each chapter from here.
        let parts = app.project.manifest.parts.as_ref().map_or(0, Vec::len);
        app.part_added = vec![0; parts];
        app.part_removed = vec![0; parts];
        app.seed_baselines();
        app.refresh_vc_status();
        app
    }


    /// Reset the per-chapter word baselines to the current counts, so the next
    /// tally measures edits from here. Used at startup and after a structural
    /// edit that shouldn't be booked as prose.
    fn seed_baselines(&mut self) {
        self.baselines = (0..self.chapter_count())
            .map(|i| (self.project.chapter_file(i).to_string(), self.chapter_word_count(i)))
            .collect();
    }

    /// The buffer the author is currently editing: the continuous-view buffer
    /// when that view is active, otherwise the single active chapter. The
    /// renderer and movement go through this so both views share one path.
    pub fn edit_buffer(&self) -> &Buffer {
        match &self.mode {
            Mode::EditMeta(m) => &m.buffer,
            Mode::BibleReview(r) if r.editing.is_some() => r.editing.as_ref().unwrap(),
            _ => match &self.manuscript {
                Some(m) => m.buffer(),
                None => &self.buffer,
            },
        }
    }

    pub fn edit_buffer_mut(&mut self) -> &mut Buffer {
        match &mut self.mode {
            Mode::EditMeta(m) => &mut m.buffer,
            Mode::BibleReview(r) if r.editing.is_some() => r.editing.as_mut().unwrap(),
            _ => match &mut self.manuscript {
                Some(m) => m.buffer_mut(),
                None => &mut self.buffer,
            },
        }
    }

    /// Called from the event loop once input goes idle: reparse the active
    /// buffer for highlighting, and book the day's word delta. Both are off the
    /// keystroke path, so typing stays instant.
    pub fn on_idle(&mut self) {
        let buffer = self.edit_buffer_mut();
        buffer.refresh_syntax();
        buffer.seal_undo_group(); // a fresh burst of typing becomes a new undo group
        self.tally_words();
        self.refresh_vc_status();
    }

    /// Attribute each chapter's word change since the last idle tick to today's
    /// global and per-part added / removed counters. Walking per chapter (rather
    /// than the project total) lets a delta be booked to the part of the chapter
    /// that changed; the global counters stay equal to the sum of the parts.
    /// Structural edits (chapter delete) drop their baseline so they aren't
    /// booked as prose.
    fn tally_words(&mut self) {
        for i in 0..self.chapter_count() {
            let words = self.chapter_word_count(i);
            let file = self.project.chapter_file(i);
            let last = self.baselines.get(file).copied().unwrap_or(0);
            let delta = words as i64 - last as i64;
            if delta == 0 {
                continue;
            }
            let (add, rem) = if delta > 0 {
                (delta as usize, 0)
            } else {
                (0, (-delta) as usize)
            };
            self.added += add;
            self.removed += rem;
            // part_of() only yields indices that exist, and the per-part vecs are
            // sized to the part count, so direct indexing is safe — and the global
            // counters stay equal to the sum of the parts by construction.
            if let Some(p) = self.project.manifest.part_of(i) {
                self.part_added[p] += add;
                self.part_removed[p] += rem;
            }
            self.baselines.insert(file.to_string(), words);
        }
    }

    /// Seed today's counters from the history log (resume the day across a
    /// reopen). Called once at startup after the project is open.
    pub fn load_history(&mut self) {
        if let Some(snap) = history::for_date(&self.project.root, &date::iso(date::today())) {
            self.added = snap.added;
            self.removed = snap.removed;
            self.moved_to_unused = snap.moved_to_unused;
            // Resume per-part counters only when they match the current parts
            // config; a changed manifest starts the day's per-part tally fresh.
            if snap.part_added.len() == self.part_added.len() {
                self.part_added = snap.part_added;
                self.part_removed = snap.part_removed;
            }
        }
    }

    /// Write today's snapshot to the history log (best-effort).
    pub fn persist_snapshot(&self) {
        let snapshot = history::Snapshot {
            date: date::iso(date::today()),
            words: self.project_word_count(),
            added: self.added,
            removed: self.removed,
            todo: self.count_todo(),
            moved_to_unused: self.moved_to_unused,
            parts: self.part_word_counts(),
            part_added: self.part_added.clone(),
            part_removed: self.part_removed.clone(),
        };
        let _ = history::persist(&self.project.root, &snapshot);
    }

    /// Current word total for each part, in manifest part order (empty when no
    /// parts). Each part sums the live counts of the chapters in its range.
    /// Each part's display name (its `name`, or `"{label} {n}"` when unnamed) and
    /// the 0-based chapter range it spans (`start..end`, clamped to the chapters
    /// that exist). Empty when the book defines no parts. The single home for the
    /// 1-based→0-based range conversion and the unnamed-part naming.
    fn part_ranges(&self) -> Vec<(String, std::ops::Range<usize>)> {
        let Some(parts) = self.project.manifest.parts.as_ref() else {
            return Vec::new();
        };
        let label = self.project.manifest.part_label();
        // Part ranges are 1-based positions among the body chapters (#185); the
        // body block is contiguous after the band reorder, starting past the
        // front matter — so a body position maps to `front + position`.
        let front = self.project.manifest.front_chapter_count();
        let body = self.project.body_chapter_count();
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let name = p.name.clone().unwrap_or_else(|| format!("{label} {}", i + 1));
                let start = p.chapters[0].saturating_sub(1).min(body);
                let end = p.chapters[1].min(body).max(start);
                (name, (front + start)..(front + end))
            })
            .collect()
    }

    fn part_word_counts(&self) -> Vec<usize> {
        self.part_ranges()
            .into_iter()
            .map(|(_, range)| range.map(|i| self.chapter_word_count(i)).sum())
            .collect()
    }

    /// Whether the active edit target has unsaved changes (either view).
    fn edit_dirty(&self) -> bool {
        match &self.manuscript {
            Some(m) => m.dirty(),
            None => self.buffer.dirty,
        }
    }

    /// Word count for chapter `index`: live from the buffer when it's active
    /// (updates as you type), otherwise the count cached on the chapter entry.
    pub fn chapter_word_count(&self, index: usize) -> usize {
        if index == self.project.active_index() {
            self.buffer.word_count()
        } else {
            self.project.chapter_words(index)
        }
    }

    /// Total words across the whole project (the active chapter live, the rest
    /// cached). Used by the progress report.
    pub fn project_word_count(&self) -> usize {
        (0..self.chapter_count())
            .map(|i| self.chapter_word_count(i))
            .sum()
    }

    /// Words in body chapters only — what the progress report measures against the
    /// goal and projects from (#182), excluding front/back matter.
    pub fn body_word_count(&self) -> usize {
        (0..self.chapter_count())
            .filter(|&i| self.project.chapter_band(i) == Band::Body)
            .map(|i| self.chapter_word_count(i))
            .sum()
    }

    /// Words in front/back-matter sections — shown in the report but not counted
    /// toward the goal (#182).
    pub fn section_word_count(&self) -> usize {
        self.project_word_count() - self.body_word_count()
    }


    /// Handle a terminal event, dispatching key presses to the active mode.
    /// Non-key events and key releases/repeats are ignored.
    pub fn handle_event(&mut self, ev: Event) {
        if let Event::Key(key) = ev {
            if key.kind == KeyEventKind::Press {
                self.on_key(key);
            }
        }
    }

    /// Handle one key event, mutating state. Returns nothing; the caller
    /// re-renders afterward.
    pub fn on_key(&mut self, key: KeyEvent) {
        // A fresh keystroke clears any stale status message.
        self.message = None;

        // The key after a `Ctrl-w` window prefix (#163) is a window command, not
        // editing input.
        if self.window_pending {
            self.window_pending = false;
            self.window_command(key.code);
            return;
        }
        // A read-only surface takes scroll keys: a docked surface pane with scroll
        // focus (#164), or a full-screen overlay (progress/analysis report, AI
        // answer). Either way Ctrl-w still starts a window command — switching
        // focus, closing, or (for an overlay) docking it into a split (#164).
        let surface_key: Option<fn(&mut App, KeyEvent)> = if self.surface_pane_focused() {
            Some(App::on_key_surface)
        } else if self.overlay.is_some() {
            Some(App::on_key_overlay)
        } else {
            None
        };
        if let Some(handle) = surface_key {
            let ctrl_w = matches!(key.code, KeyCode::Char('w')) && key.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_w {
                self.window_pending = true;
            } else {
                handle(self, key);
            }
            return;
        }

        // Non-binding patterns: dispatch on the variant without moving its data.
        match self.mode {
            Mode::Prompt(_) => self.on_key_prompt(key),
            Mode::Nav { .. } => self.on_key_nav(key),
            Mode::EditMeta(_) => self.on_key_edit_meta(key),
            Mode::BibleReview(_) => self.on_key_bible_review(key),
            Mode::Findings(_) => self.on_key_findings(key),
            Mode::Palette(_) => self.on_key_palette(key),
            Mode::Search(_) => self.on_key_search(key),
            Mode::RefFiles(_) => self.on_key_reffiles(key),
            Mode::Insert => self.on_key_insert(key),
        }
    }

    /// Save the current chapter (if dirty), then load `index` as the active
    /// chapter. The active index and the live buffer always move together here,
    /// so the two never drift. A no-op if `index` is already active or invalid.
    /// Save the active chapter if it has unsaved edits and refresh its cached
    /// word count. Returns `Err(())` (and sets a status message) if the save
    /// failed, so callers can abort before switching away from it.
    fn flush_active(&mut self) -> Result<(), ()> {
        if self.buffer.dirty {
            if let Err(e) = self.buffer.save() {
                self.message = Some(format!("Save failed: {e}"));
                return Err(());
            }
        }
        // The chapter is now on disk; refresh its cached count.
        self.cache_focus_words();
        Ok(())
    }

    fn activate_chapter(&mut self, index: usize) {
        if index >= self.project.manifest.chapters.len() || index == self.project.active_index() {
            return;
        }
        // Flush the chapter we're leaving before loading the next.
        if self.flush_active().is_err() {
            return;
        }
        match Buffer::from_path(self.project.chapter_path(index)) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.project.set_active(index);
                // No need to cache the new active chapter's count: while active,
                // chapter_word_count reads it live from the buffer.
                if let Some(title) = self.project.active_title() {
                    self.message = Some(format!("Chapter: {title}"));
                }
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    fn chapter_count(&self) -> usize {
        self.project.manifest.chapters.len()
    }

    // --- Continuous-manuscript view ---------------------------------------

    /// Toggle between the single-chapter view and the continuous view.
    fn toggle_continuous(&mut self) {
        if self.manuscript.is_some() {
            self.leave_continuous();
        } else {
            self.enter_continuous();
        }
    }

    /// Enter the continuous view: flush the single chapter, then open a window
    /// of chapters around it.
    fn enter_continuous(&mut self) {
        if self.flush_active().is_err() {
            return;
        }
        self.close_split_if_any(); // the continuous view and a split can't share the editor (#163)
        match Manuscript::open(&self.project) {
            Ok(m) => {
                self.manuscript = Some(m);
                self.message = Some("Continuous view".into());
            }
            // A chapter in the window couldn't be read (#154) — stay in the
            // single-chapter view rather than show it blank.
            Err(e) => self.message = Some(format!("Couldn't open continuous view: {e}")),
        }
    }

    /// Leave the continuous view: save its chapters, then reopen the single
    /// chapter the cursor was in. Stays in continuous view if the save fails, so
    /// no edits are lost.
    fn leave_continuous(&mut self) {
        let Some(mut m) = self.manuscript.take() else {
            return;
        };
        if let Err(e) = m.save(&mut self.project) {
            self.message = Some(format!("Save failed: {e}"));
            self.manuscript = Some(m);
            return;
        }
        self.project.set_active(m.current_chapter());
        match Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.message = Some("Chapter view".into());
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    // --- Edit dispatch (single chapter or continuous) ----------------------

    fn edit_insert_char(&mut self, c: char) {
        match &mut self.manuscript {
            Some(m) => m.insert_char(c),
            None => self.buffer.insert_char(c),
        }
    }

    fn edit_backspace(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.backspace(),
            None => self.buffer.backspace(),
        }
    }

    /// Undo / redo one edit group (#145). Scoped to the single-chapter view: the
    /// continuous view's segment bookkeeping isn't snapshotted, so history there
    /// is guarded with a message rather than a silent desync (a follow-up).
    fn history_step(&mut self, redo: bool) {
        let verb = if redo { "Redo" } else { "Undo" };
        if self.manuscript.is_some() {
            self.message = Some(format!("{verb} isn't available in the continuous view yet — Ctrl-l to return."));
            return;
        }
        let changed = if redo { self.buffer.redo() } else { self.buffer.undo() };
        if changed {
            self.cache_focus_words();
            self.message = None;
        } else {
            self.message = Some(format!("Nothing to {}.", verb.to_lowercase()));
        }
    }

    /// The editor's current wrap column (#143), in cells.
    fn wrap_w(&self) -> usize {
        self.wrap_width.get() as usize
    }

    /// Apply a wrap-aware cursor movement (up/down/home/end) at the current wrap
    /// width, paging the continuous view if the cursor crossed a chapter.
    fn edit_move_wrapped(&mut self, mv: impl FnOnce(&mut Buffer, usize)) {
        let w = self.wrap_w();
        self.edit_move(move |b| mv(b, w));
    }

    /// Apply a cursor movement to the active edit buffer, then page the window
    /// if the continuous view's cursor crossed into another chapter.
    fn edit_move(&mut self, mv: impl FnOnce(&mut Buffer)) {
        mv(self.edit_buffer_mut());
        if let Some(mut m) = self.manuscript.take() {
            // reslide flushes pending edits then pages the window; either the save
            // or a chapter read (#154) can fail. The window is left intact on a
            // failed read, so the view simply doesn't page.
            if let Err(e) = m.reslide(&mut self.project) {
                self.message = Some(format!("Continuous view: {e}"));
            }
            self.manuscript = Some(m);
        }
    }

    /// Save the active edit target (the continuous window, or the single chapter).
    fn save_edit(&mut self) {
        if let Some(mut m) = self.manuscript.take() {
            let result = m.save(&mut self.project);
            self.manuscript = Some(m);
            self.message = Some(match result {
                Ok(()) => "Saved".into(),
                Err(e) => format!("Save failed: {e}"),
            });
        } else {
            match self.buffer.save() {
                Ok(()) => {
                    self.cache_focus_words();
                    self.refresh_split_sibling(); // a clean sibling on this chapter reflects the save (#163)
                    self.message = Some("Saved".into());
                }
                Err(e) => self.message = Some(format!("Save failed: {e}")),
            }
        }
        // Checkpoint today's momentum to the history log.
        self.persist_snapshot();
    }

    fn next_chapter(&mut self) {
        self.activate_chapter(self.project.active_index() + 1);
    }

    fn prev_chapter(&mut self) {
        // At chapter 0 this targets 0 — a no-op via activate_chapter's guard.
        self.activate_chapter(self.project.active_index().saturating_sub(1));
    }

    // --- Chapter structure editing ----------------------------------------
    //
    // Each wraps a `Project` op and returns the chapter the browser should now
    // select. The active chapter is tracked by file identity, so insert/rename/
    // move need no active fixup; only delete (of the active chapter) reloads.

    fn create_chapter(&mut self, at: usize, title: &str) -> usize {
        match self.project.add_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Added '{title}'")),
            Err(e) => self.message = Some(format!("Add failed: {e}")),
        }
        at.min(self.chapter_count() - 1)
    }

    fn rename_chapter(&mut self, at: usize, title: &str) {
        match self.project.rename_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Renamed to '{title}'")),
            Err(e) => self.message = Some(format!("Rename failed: {e}")),
        }
    }

    /// Move chapter `sel` up or down one slot; returns its new position (the
    /// active chapter follows its content automatically via identity tracking).
    fn move_chapter(&mut self, sel: usize, up: bool) -> usize {
        let target = if up {
            sel.checked_sub(1)
        } else {
            (sel + 1 < self.chapter_count()).then_some(sel + 1)
        };
        let Some(j) = target else { return sel };
        // Reordering stays within a band — a section can't be shuffled out of
        // front/back matter into the body or vice versa (#180).
        if self.project.chapter_band(sel) != self.project.chapter_band(j) {
            self.message = Some("Reordering stays within front matter / body / back matter.".into());
            return sel;
        }
        match self.project.swap_chapters(sel, j) {
            Ok(()) => j,
            Err(e) => {
                self.message = Some(format!("Move failed: {e}"));
                sel
            }
        }
    }

    /// Delete chapter `at`; returns the chapter the browser should select next.
    fn delete_chapter(&mut self, at: usize) -> usize {
        if self.chapter_count() <= 1 {
            self.message = Some(ONLY_CHAPTER_MSG.into());
            return at;
        }
        // Deleting the active chapter is the one structural edit that changes the
        // buffer's content — every other op keeps the active file present, so its
        // index just re-derives.
        let deleting_active = at == self.project.active_index();
        if deleting_active && self.buffer.dirty {
            // Persist the latest text first so the copy moved to unused/ is current.
            let _ = self.buffer.save();
        }
        // The chapter's words leave the manuscript — booked as "moved to unused"
        // today, not as words deleted (that's prose editing).
        let removed_words = self.chapter_word_count(at);
        if let Err(e) = self.project.remove_chapter(at) {
            self.message = Some(format!("Delete failed: {e}"));
            return at.min(self.chapter_count() - 1);
        }

        if deleting_active {
            // The active file is gone — point at a neighbor and reload.
            let neighbor = at.min(self.chapter_count() - 1);
            self.project.set_active(neighbor);
            match Buffer::from_path(self.project.active_path()) {
                Ok(buffer) => self.buffer = buffer,
                Err(e) => self.message = Some(format!("Open failed: {e}")),
            }
        }
        self.moved_to_unused += removed_words;
        // Absorb the structural drop so the next tally doesn't book it as removed:
        // re-baseline every chapter to its current count (this also drops the
        // deleted file's stale entry, since the map is rebuilt by file identity).
        self.seed_baselines();
        self.message = Some("Moved chapter to unused/".into());
        at.min(self.chapter_count() - 1)
    }

    // --- Chapter metadata (front matter) ----------------------------------

    /// Open a prompt. `nav_return` is the chapter to reselect when it closes
    /// (`None` returns to insert mode, e.g. the quit prompt).
    fn open_prompt(
        &mut self,
        pending: Pending,
        label: impl Into<String>,
        prefill: &str,
        nav_return: Option<usize>,
    ) {
        self.mode = Mode::Prompt(PromptState {
            pending,
            label: label.into(),
            input: prefill.to_string(),
            nav_return,
        });
    }

    /// Enter (or stay in) the chapter browser with `selected` highlighted.
    fn select(&mut self, selected: usize) {
        self.mode = Mode::Nav { selected };
    }

    /// The prompt state, while in [`Mode::Prompt`]. Only call from a prompt key
    /// handler, which has already matched the variant.
    fn prompt_mut(&mut self) -> &mut PromptState {
        match &mut self.mode {
            Mode::Prompt(state) => state,
            _ => unreachable!("prompt editing runs only in Prompt mode"),
        }
    }

    fn on_key_nav(&mut self, key: KeyEvent) {
        let Mode::Nav { selected: sel } = self.mode else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select(sel.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                let last = self.chapter_count() - 1;
                self.select((sel + 1).min(last));
            }
            KeyCode::Enter => {
                self.activate_chapter(sel);
                self.mode = Mode::Insert;
            }
            // Open the selected chapter in a side pane (#165) — compare two
            // chapters, or keep one as reference while editing another.
            KeyCode::Char('o') => {
                self.open_file_in_pane(self.project.chapter_path(sel));
                self.mode = Mode::Insert;
            }
            // Switch to the reference-file browser (#165): bible entries and notes.
            KeyCode::Char('f') => self.open_ref_files(),
            // Reorder the selected chapter.
            KeyCode::Char('K') => {
                let s = self.move_chapter(sel, true);
                self.select(s);
            }
            KeyCode::Char('J') => {
                let s = self.move_chapter(sel, false);
                self.select(s);
            }
            // Add / rename / delete (each returns to the browser when done).
            KeyCode::Char('a') => {
                self.open_prompt(Pending::NewChapter { at: sel + 1 }, "New chapter title: ", "", Some(sel))
            }
            KeyCode::Char('r') => {
                let title = self.project.manifest.chapters[sel].title.clone();
                self.open_prompt(Pending::Rename { at: sel }, "Rename chapter: ", &title, Some(sel));
            }
            // Chapter metadata (front matter): status, summary, POV, epigraph.
            KeyCode::Char('s') => self.cycle_status(sel),
            KeyCode::Char('m') => {
                let prefill = self.project.chapter_summary(sel).unwrap_or("").to_string();
                self.open_prompt(Pending::SetMeta(MetaField::Summary { at: sel }), "Summary: ", &prefill, Some(sel));
            }
            KeyCode::Char('v') => {
                let prefill = self.project.chapter_pov(sel).unwrap_or("").to_string();
                self.open_prompt(Pending::SetMeta(MetaField::Pov { at: sel }), "POV: ", &prefill, Some(sel));
            }
            KeyCode::Char('g') => {
                let text = self.chapter_epigraph(sel).map(|e| e.text).unwrap_or_default();
                self.edit_meta(MetaField::ChapterEpigraph { at: sel }, "chapter epigraph", text, Some(sel));
            }
            // Set the chapter's word-count goal (#146); blank clears it.
            KeyCode::Char('l') => {
                let prefill = self.project.chapter_goal(sel).map(|g| g.to_string()).unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Goal { at: sel }), "Word goal: ", &prefill, Some(sel));
            }
            // Set the file's section role (#179); blank clears it to a body chapter.
            KeyCode::Char('S') => {
                let prefill = self.project.chapter_role(sel).map(|r| r.key().to_string()).unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Role { at: sel }),
                    "Section role (prologue/epilogue/appendix/…; blank = body): ",
                    &prefill,
                    Some(sel),
                );
            }
            // Project-level book / series metadata.
            KeyCode::Char('t') => {
                let cur = self.project.manifest.title.clone();
                self.open_prompt(Pending::SetMeta(MetaField::BookTitle), "Book title: ", &cur, Some(sel));
            }
            KeyCode::Char('b') => {
                let cur = self.project.manifest.subtitle.clone().unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Subtitle), "Subtitle: ", &cur, Some(sel));
            }
            KeyCode::Char('w') => {
                let cur = self.project.manifest.author.clone().unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Author), "Author: ", &cur, Some(sel));
            }
            KeyCode::Char('e') => {
                let cur = self.project.manifest.series.clone().unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Series), "Series: ", &cur, Some(sel));
            }
            KeyCode::Char('G') => {
                let text = self.project.manifest.epigraph.clone().unwrap_or_default();
                self.edit_meta(MetaField::BookEpigraph, "book epigraph", text, Some(sel));
            }
            KeyCode::Char('d') => {
                if self.chapter_count() <= 1 {
                    self.message = Some(ONLY_CHAPTER_MSG.into());
                } else {
                    let title = self.project.manifest.chapters[sel].title.clone();
                    self.open_prompt(
                        Pending::Delete { at: sel },
                        format!("Delete '{title}'? (y/N)"),
                        "",
                        Some(sel),
                    );
                }
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    /// Enter the reference-file browser (#165): the project's bible entries and
    /// notes. Stays in the chapter browser with a note when there are none.
    fn open_ref_files(&mut self) {
        let files = self.project.reference_files();
        if files.is_empty() {
            self.message = Some("No reference files yet — add notes or a bible under the project.".into());
            return;
        }
        self.mode = Mode::RefFiles(Box::new(RefFiles { files, selected: 0 }));
    }

    /// Navigate the reference-file browser (#165): arrows move, Enter opens the
    /// selected file in a side pane, `f`/Esc return to the chapter browser.
    fn on_key_reffiles(&mut self, key: KeyEvent) {
        let Mode::RefFiles(state) = &mut self.mode else {
            return;
        };
        if let Some(s) = assist::list_move(state.selected, state.files.len(), key.code) {
            state.selected = s;
            return;
        }
        match key.code {
            KeyCode::Enter => {
                let path = state.files[state.selected].clone();
                self.open_file_in_pane(path);
                self.mode = Mode::Insert;
            }
            // Back to the chapter browser.
            KeyCode::Char('f') | KeyCode::Esc => self.select(self.project.active_index()),
            _ => {}
        }
    }

    fn on_key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt(state) = &self.mode else {
            return;
        };
        let pending = state.pending;
        let nav_return = state.nav_return;

        // Where to go when the prompt closes without acting.
        let closed = closed_mode(nav_return);

        match pending {
            Pending::Quit => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.should_quit = true,
                _ => self.mode = closed,
            },
            Pending::Delete { at } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let selected = self.delete_chapter(at);
                    self.select(selected);
                }
                _ => self.mode = closed,
            },
            Pending::ConfirmAi(action, force) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.run_ai_action(action, force);
                }
                _ => self.mode = closed,
            },
            Pending::ConfirmRestore => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.restore_chapter();
                }
                _ => self.mode = closed,
            },
            // The text prompts share the editing keys; only Enter differs.
            Pending::NewChapter { .. }
            | Pending::Rename { .. }
            | Pending::SetMeta(_)
            | Pending::AskAi
            | Pending::Commit
            | Pending::SetRemote => {
                match key.code {
                    KeyCode::Char(c) => self.prompt_mut().input.push(c),
                    KeyCode::Backspace => {
                        self.prompt_mut().input.pop();
                    }
                    KeyCode::Enter => {
                        let text = std::mem::take(&mut self.prompt_mut().input);
                        let text = text.trim();
                        let fallback = nav_return.unwrap_or(0);
                        match pending {
                            Pending::NewChapter { at } => {
                                let sel =
                                    if text.is_empty() { fallback } else { self.create_chapter(at, text) };
                                self.select(sel);
                            }
                            Pending::Rename { at } => {
                                if !text.is_empty() {
                                    self.rename_chapter(at, text);
                                }
                                self.select(fallback);
                            }
                            Pending::SetMeta(field) => {
                                // Single-line metadata prompts (summary, POV,
                                // attribution, book fields). The multi-line
                                // epigraph quotations are edited in the editor
                                // (Mode::EditMeta), which chains here for the
                                // attribution. A role change re-sorts the list, so
                                // follow the chapter to its new index (#180).
                                let moved = self.apply_meta(field, text);
                                self.select(moved.unwrap_or(fallback));
                            }
                            Pending::AskAi => {
                                // Back to editing immediately; the answer arrives
                                // asynchronously in the overlay.
                                self.mode = closed;
                                self.ask_ai(text);
                            }
                            Pending::Commit => {
                                self.mode = closed;
                                if text.is_empty() {
                                    self.message = Some("Commit cancelled — empty message.".into());
                                } else {
                                    self.commit(text);
                                }
                            }
                            Pending::SetRemote => {
                                self.mode = closed;
                                self.set_remote(text);
                            }
                            // Confirmations never reach the text arm.
                            _ => self.select(fallback),
                        }
                    }
                    KeyCode::Esc => self.mode = closed,
                    _ => {}
                }
            }
        }
    }

    fn on_key_insert(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match (ctrl, key.code) {
            (true, KeyCode::Char('s')) => self.save_edit(),
            (true, KeyCode::Char('q')) => {
                if self.edit_dirty() {
                    self.open_prompt(Pending::Quit, "Unsaved changes — quit anyway? (y/N)", "", None);
                } else {
                    self.should_quit = true;
                }
            }
            (true, KeyCode::Char('=') | KeyCode::Char('+')) => {
                self.focus_width = (self.focus_width + 4).min(160);
            }
            (true, KeyCode::Char('-')) => {
                self.focus_width = self.focus_width.saturating_sub(4).max(32);
            }
            (true, KeyCode::Char('o')) => {
                self.outline_visible = !self.outline_visible;
            }
            // Open the version-control palette (#54) — init / commit / history /
            // changes / restore, in one menu. Ctrl-v: "version control".
            (true, KeyCode::Char('v')) => self.open_git_palette(),
            // Open the progress report (scroll to read; any other key closes).
            (true, KeyCode::Char('r')) => {
                self.overlay = Some(Overlay::report("progress", self.report_lines()));
            }
            // Open the local analysis report — editing metrics. Ctrl-t: "text"
            // analysis. Scrollable (it can run past one screen); any other key closes.
            (true, KeyCode::Char('t')) => {
                self.overlay = Some(Overlay::report("analysis", self.analysis_lines()));
            }
            // Open the AI command palette (opt-in). When AI isn't ready, explain
            // how to enable it rather than listing dead actions.
            (true, KeyCode::Char('a')) => match self.ai.check_ready() {
                Ok(_) => self.open_ai_palette(),
                Err(e) => self.message = Some(e.to_string()),
            },
            // Compile the manuscript to a single export file (#190). Ctrl-e.
            (true, KeyCode::Char('e')) => self.export_markdown(),
            // Toggle the continuous-manuscript view. Ctrl-l ("long" view), not
            // Ctrl-m — terminals deliver Ctrl-m as Enter.
            (true, KeyCode::Char('l')) => self.toggle_continuous(),
            // Step to the next / previous chapter (single-chapter view only;
            // in continuous view you just scroll across boundaries).
            (true, KeyCode::Char('n')) if self.manuscript.is_none() => self.next_chapter(),
            (true, KeyCode::Char('p')) if self.manuscript.is_none() => self.prev_chapter(),
            // Focus the chapter pane to browse. Chapter structure is edited in
            // the single-chapter view, so leave continuous first.
            (true, KeyCode::Char('g')) => {
                self.leave_continuous();
                self.outline_visible = true;
                self.mode = Mode::Nav {
                    selected: self.project.active_index(),
                };
            }

            // Undo / redo (#145). Ctrl-z undoes a burst of typing; Ctrl-y redoes.
            (true, KeyCode::Char('z')) => self.history_step(false),
            (true, KeyCode::Char('y')) => self.history_step(true),
            // Find (#144). Ctrl-f opens the query; from there `r` starts a replace.
            (true, KeyCode::Char('f')) => self.open_find(),
            // Window/split prefix (#163): the next key is a window command
            // (s/v split, w/h/j/k/l focus, c/q close). Vim's Ctrl-w.
            (true, KeyCode::Char('w')) => self.window_pending = true,

            (false, KeyCode::Char(c)) => self.edit_insert_char(c),
            (false, KeyCode::Enter) => self.edit_insert_char('\n'),
            (false, KeyCode::Backspace) => self.edit_backspace(),
            (false, KeyCode::Tab) => {
                for _ in 0..4 {
                    self.edit_insert_char(' ');
                }
            }

            (false, KeyCode::Left) => self.edit_move(Buffer::move_left),
            (false, KeyCode::Right) => self.edit_move(Buffer::move_right),
            (false, KeyCode::Up) => self.edit_move_wrapped(Buffer::move_up),
            (false, KeyCode::Down) => self.edit_move_wrapped(Buffer::move_down),
            (false, KeyCode::Home) => self.edit_move_wrapped(Buffer::move_home),
            (false, KeyCode::End) => self.edit_move_wrapped(Buffer::move_end),

            _ => {}
        }
    }


}

/// The new scroll offset for a scroll key over a `cur` position within
/// `[0, max]`, paging by `page` lines — or `None` if `code` isn't a scroll key.
/// Shared by the full-screen overlay and the docked surface pane (#164).
fn scroll_target(code: KeyCode, ctrl: bool, cur: u16, max: u16, page: u16) -> Option<u16> {
    let (cur, max, page) = (cur as i32, max as i32, page as i32);
    let target = match (ctrl, code) {
        (false, KeyCode::Down | KeyCode::Char('j')) => cur + 1,
        (false, KeyCode::Up | KeyCode::Char('k')) => cur - 1,
        (false, KeyCode::PageDown | KeyCode::Char(' ')) | (true, KeyCode::Char('d')) => cur + page,
        (false, KeyCode::PageUp | KeyCode::Char('b')) | (true, KeyCode::Char('u')) => cur - page,
        (false, KeyCode::Home | KeyCode::Char('g')) => 0,
        (false, KeyCode::End | KeyCode::Char('G')) => max,
        _ => return None,
    };
    Some(target.clamp(0, max) as u16)
}

/// Where an overlay (prompt or metadata editor) returns when it closes without
/// acting: back to the chapter browser at `selected`, or to insert mode.
fn closed_mode(nav_return: Option<usize>) -> Mode {
    match nav_return {
        Some(selected) => Mode::Nav { selected },
        None => Mode::Insert,
    }
}


#[cfg(test)]
mod tests;

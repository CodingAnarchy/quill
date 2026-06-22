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
use crate::buffer::{self, Buffer, FindKind, TextObject};
use crate::date;
use crate::frontmatter::{Band, Epigraph, FrontMatter, Role, Status};
use crate::history;
use crate::manuscript::Manuscript;
use crate::mode::{
    BibleConflict, BibleReview, Command, Find, Finding, Findings, GitAction, MetaEdit, MetaField,
    Mode, Op, Overlay, Palette, PaletteItem, Pending, PromptState, RefFiles, VimPending, VimState,
    Yank,
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
mod workflows;

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
    /// Opt-in Vim-style modal editing (#27), from the `[editor]` config. When on,
    /// the editor's resting state is [`Mode::Normal`] and the status bar shows the
    /// mode; when off, editing is exactly as it has always been.
    pub vim: bool,
    /// Transient Vim Normal-mode state (#202): pending count, multi-key command,
    /// and last find. Only meaningful while `vim` is on.
    vim_state: VimState,
    /// Vim key remaps (#208): a pressed key → the default key whose action it
    /// should run, built from the `[editor.keys]` config. Empty (no remaps) by
    /// default, so the built-in bindings apply unchanged.
    vim_keys: HashMap<char, char>,
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
    /// Author-defined workflows (#226) from the `[workflows]` config — named,
    /// ordered sequences of registry actions run as one step. Loaded once at
    /// startup, sorted by name for a stable run-workflow menu.
    workflows: Vec<crate::workflow::Workflow>,
    /// Workflow triggers (#228) from the `[triggers]` config — on-save and bound
    /// Vim Normal-mode keys, validated against `workflows` at startup.
    triggers: crate::workflow::Triggers,
    /// The workflow currently executing (#227), if any: its resolved steps and
    /// where it is. `Some` while a run is in progress — including while it's
    /// suspended waiting on an async AI step. Drives the step state machine.
    workflow_run: Option<workflows::WorkflowRun>,
    /// One-line transient message shown in the status bar.
    pub message: Option<String>,
}

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        let under_vc = crate::vcs::is_repo(&project.root);
        let editor = crate::config::EditorConfig::load();
        let vim = editor.vim;
        let (vim_keys, key_errors) = build_keymap(&editor.keys);
        let loaded = crate::workflow::load();
        let trigger_errors = loaded.errors;
        let mut app = Self {
            buffer,
            project,
            manuscript: None,
            split: None,
            window_pending: false,
            // With Vim mode on, the editor rests in Normal; otherwise it's always
            // the modeless Insert.
            mode: if vim { Mode::Normal } else { Mode::Insert },
            vim,
            vim_state: VimState::default(),
            vim_keys,
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
            workflows: loaded.workflows,
            triggers: loaded.triggers,
            workflow_run: None,
            message: None,
        };
        // Baselines for the daily word tally: edits move each chapter from here.
        let parts = app.project.manifest.parts.as_ref().map_or(0, Vec::len);
        app.part_added = vec![0; parts];
        app.part_removed = vec![0; parts];
        app.seed_baselines();
        app.refresh_vc_status();
        // Surface dropped config — bad key remaps (#208) and bad workflow triggers
        // (#228) — rather than ignoring them silently.
        let mut warnings = Vec::new();
        if vim && !key_errors.is_empty() {
            warnings.push(format!("Vim keymap ignored: {}", key_errors.join("; ")));
        }
        if !trigger_errors.is_empty() {
            warnings.push(format!("Workflow triggers ignored: {}", trigger_errors.join("; ")));
        }
        if !warnings.is_empty() {
            app.message = Some(warnings.join(" · "));
        }
        app
    }


    /// Reset the per-chapter word baselines to the current counts, so the next
    /// tally measures edits from here. Used at startup and after a structural
    /// edit that shouldn't be booked as prose.
    fn seed_baselines(&mut self) {
        self.baselines = (0..self.chapter_count())
            .map(|i| (self.project.chapter_file(i).to_string(), self.chapter_word_count(i)))
            .collect();
        // A full re-baseline supersedes any pending per-chapter changes (#220).
        self.project.clear_words_changed();
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
        // Only a few chapters can have changed since the last idle tick: the live
        // active chapter, plus any whose cached count moved (continuous-view saves,
        // reslide). Re-checking just those keeps the tally O(edit), not O(project)
        // (#220). Keyed by file so a reorder between ticks can't misattribute.
        let mut files = self.project.take_words_changed();
        files.insert(self.project.active_file().to_string()); // always re-check the live active chapter
        for file in files {
            let Some(i) = self.project.index_of_file(&file) else {
                continue; // chapter gone (e.g. deleted) since it was recorded
            };
            let words = self.chapter_word_count(i);
            let last = self.baselines.get(&file).copied().unwrap_or(0);
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
            self.baselines.insert(file, words);
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

        // While a workflow is running (suspended on an async step), Esc cancels it
        // — cleanly, after the current step finishes (#227). Only in an editing
        // mode: a prompt/palette owns Esc itself (a cost-confirm cancels there).
        if self.workflow_run.is_some()
            && key.code == KeyCode::Esc
            && matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
        {
            self.cancel_workflow();
            return;
        }

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
            Mode::Normal => self.on_key_normal(key),
            Mode::Visual { .. } => self.on_key_visual(key),
        }
    }

    /// The editor's resting mode: Vim Normal when Vim mode is on, else the
    /// modeless Insert. Every "return to the editor" path routes through this so
    /// it lands in the right mode for the user's configuration.
    fn editing_mode(&self) -> Mode {
        if self.vim {
            Mode::Normal
        } else {
            Mode::Insert
        }
    }

    /// Where an overlay (prompt or metadata editor) returns when it closes
    /// without acting: back to the chapter browser at `selected`, or to the
    /// editor's resting mode.
    fn closed_mode(&self, nav_return: Option<usize>) -> Mode {
        match nav_return {
            Some(selected) => Mode::Nav { selected },
            None => self.editing_mode(),
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
                // A Visual selection's anchor is a byte in the single-chapter
                // buffer; it's meaningless against the manuscript window, so drop
                // back to the resting mode (#205).
                if matches!(self.mode, Mode::Visual { .. }) {
                    self.mode = self.editing_mode();
                }
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
        // Run the on-save workflow trigger (#228), unless this save is itself a
        // workflow's own step (guarded inside) — no re-entrancy.
        self.trigger_on_save();
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

    /// Open the Vim Ex command line (#207): `:` then a command (`w`/`q`/`wq`/…).
    fn open_ex_command(&mut self) {
        self.vim_state.op = None;
        self.open_prompt(Pending::ExCommand, ":", "", None);
    }

    /// Quit, or — when there are unsaved changes — open the confirm prompt first.
    /// Shared by Ctrl-Q and `:q`.
    fn request_quit(&mut self) {
        if self.edit_dirty() {
            self.open_prompt(Pending::Quit, "Unsaved changes — quit anyway? (y/N)", "", None);
        } else {
            self.should_quit = true;
        }
    }

    /// Run a typed Ex command (#207). The supported set is the file commands a
    /// Vim user reaches for first; anything else is reported, like Vim's E492.
    fn run_ex_command(&mut self, cmd: &str) {
        match cmd {
            "" => {} // bare `:` — nothing to do
            "w" => self.save_edit(),
            // `:q` prompts on unsaved changes, exactly as Ctrl-Q does.
            "q" => self.request_quit(),
            // Write then quit; don't quit if the write failed (buffer still dirty).
            "wq" | "x" => {
                self.save_edit();
                if !self.edit_dirty() {
                    self.should_quit = true;
                }
            }
            "q!" => self.should_quit = true,
            other => self.message = Some(format!("Not an editor command: {other}")),
        }
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
                self.mode = self.editing_mode();
            }
            // Open the selected chapter in a side pane (#165) — compare two
            // chapters, or keep one as reference while editing another.
            KeyCode::Char('o') => {
                self.open_file_in_pane(self.project.chapter_path(sel));
                self.mode = self.editing_mode();
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
                self.mode = self.editing_mode();
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
        let closed = self.closed_mode(nav_return);

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
                    // Mid-workflow, the confirm gates that step: run it as the
                    // workflow's async step so the run advances on completion.
                    if self.workflow_run.is_some() {
                        self.start_async_step(action);
                    } else {
                        self.run_ai_action(action, force);
                    }
                }
                _ => {
                    self.mode = closed;
                    // Declining a workflow step's cost confirm cancels the run.
                    self.decline_workflow();
                }
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
            | Pending::SetRemote
            | Pending::ExCommand => {
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
                            Pending::ExCommand => {
                                self.mode = closed; // back to the editor, then run it
                                self.run_ex_command(text);
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

    /// The editor's `Ctrl-` shortcuts, shared by Insert and Vim Normal mode so
    /// they keep working in both. A `Ctrl-` combo never produces text in either
    /// mode, so handling is fire-and-forget — an unrecognized one is simply
    /// ignored.
    fn on_key_ctrl(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('s') => self.save_edit(),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('=') | KeyCode::Char('+') => {
                self.focus_width = (self.focus_width + 4).min(160);
            }
            KeyCode::Char('-') => {
                self.focus_width = self.focus_width.saturating_sub(4).max(32);
            }
            KeyCode::Char('o') => {
                self.outline_visible = !self.outline_visible;
            }
            // Open the version-control palette (#54) — init / commit / history /
            // changes / restore, in one menu. Ctrl-v: "version control".
            KeyCode::Char('v') => self.open_git_palette(),
            // Open the progress report (scroll to read; any other key closes).
            KeyCode::Char('r') => self.open_progress_report(),
            // Open the local analysis report — editing metrics. Ctrl-t: "text"
            // analysis. Scrollable (it can run past one screen); any other key closes.
            KeyCode::Char('t') => self.open_analysis_report(),
            // Open the AI command palette (opt-in). When AI isn't ready, explain
            // how to enable it rather than listing dead actions.
            KeyCode::Char('a') => match self.ai.check_ready() {
                Ok(_) => self.open_ai_palette(),
                Err(e) => self.message = Some(e.to_string()),
            },
            // Open the export menu (#7) — Markdown / Word. Ctrl-e.
            KeyCode::Char('e') => self.open_export_palette(),
            // Open the unified run-action menu (#225) — every invokable action
            // (save, export, reports, AI passes) in one searchable list. Ctrl-k
            // for "command", the one menu that reaches the others.
            KeyCode::Char('k') => self.open_action_palette(),
            // Open the run-workflow menu (#226) — the author's named action
            // sequences. Ctrl-x: "execute" a workflow.
            KeyCode::Char('x') => self.open_workflow_palette(),
            // Toggle the continuous-manuscript view. Ctrl-l ("long" view), not
            // Ctrl-m — terminals deliver Ctrl-m as Enter.
            KeyCode::Char('l') => self.toggle_continuous(),
            // Step to the next / previous chapter (single-chapter view only;
            // in continuous view you just scroll across boundaries).
            KeyCode::Char('n') if self.manuscript.is_none() => self.next_chapter(),
            KeyCode::Char('p') if self.manuscript.is_none() => self.prev_chapter(),
            // Focus the chapter pane to browse. Chapter structure is edited in
            // the single-chapter view, so leave continuous first.
            KeyCode::Char('g') => {
                self.leave_continuous();
                self.outline_visible = true;
                self.mode = Mode::Nav {
                    selected: self.project.active_index(),
                };
            }

            // Undo / redo (#145). Ctrl-z undoes a burst of typing; Ctrl-y redoes.
            // (Vim's redo, Ctrl-r, is taken by the progress report; in Vim mode
            // `u` undoes and Ctrl-y redoes.)
            KeyCode::Char('z') => self.history_step(false),
            KeyCode::Char('y') => self.history_step(true),
            // Find (#144). Ctrl-f opens the query; from there `r` starts a replace.
            KeyCode::Char('f') => self.open_find(),
            // Window/split prefix (#163): the next key is a window command
            // (s/v split, w/h/j/k/l focus, c/q close). Vim's Ctrl-w.
            KeyCode::Char('w') => self.window_pending = true,

            _ => {}
        }
    }

    fn on_key_insert(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.on_key_ctrl(key.code);
            return;
        }
        match key.code {
            // In Vim mode, Esc leaves Insert for Normal; without it, Esc is inert.
            KeyCode::Esc if self.vim => self.mode = Mode::Normal,

            KeyCode::Char(c) => self.edit_insert_char(c),
            KeyCode::Enter => self.edit_insert_char('\n'),
            KeyCode::Backspace => self.edit_backspace(),
            KeyCode::Tab => {
                for _ in 0..4 {
                    self.edit_insert_char(' ');
                }
            }

            KeyCode::Left => self.edit_move(Buffer::move_left),
            KeyCode::Right => self.edit_move(Buffer::move_right),
            KeyCode::Up => self.edit_move_wrapped(Buffer::move_up),
            KeyCode::Down => self.edit_move_wrapped(Buffer::move_down),
            KeyCode::Home => self.edit_move_wrapped(Buffer::move_home),
            KeyCode::End => self.edit_move_wrapped(Buffer::move_end),

            _ => {}
        }
    }

    /// Vim Normal mode (#27): keys are commands. This foundation covers entering
    /// Insert (`i`/`a`/`I`/`A`/`o`/`O`) and basic cursor motion (`hjkl` + arrows);
    /// richer motions, operators, and Visual mode are later children of #27.
    fn on_key_normal(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.on_key_ctrl(key.code);
            return;
        }
        let KeyCode::Char(c) = key.code else {
            return self.on_key_normal_special(key.code);
        };

        // Resolve a multi-key command first: this char is its second key.
        match self.vim_state.pending {
            VimPending::G => {
                self.vim_state.pending = VimPending::None;
                if c == 'g' {
                    let target = self.vim_state.count.take().map_or(0, |n| n.saturating_sub(1));
                    self.goto_line(target); // gg → first line (or {count}gg); operator-aware
                } else {
                    self.vim_state.count = None; // an unknown `g_` aborts
                    self.vim_state.op = None;
                }
                return;
            }
            VimPending::Find(kind) => {
                self.vim_state.pending = VimPending::None;
                let count = self.take_count();
                self.vim_state.last_find = Some(Find { kind, target: c });
                // f/t are inclusive; charwise() moves or operates (dfx) as needed.
                self.charwise(1, true, move |b| b.find_char(c, kind, count));
                return;
            }
            VimPending::Object { around } => {
                self.vim_state.pending = VimPending::None;
                self.apply_text_object(c, around);
                return;
            }
            VimPending::Register => {
                self.vim_state.pending = VimPending::None;
                if c.is_ascii_alphabetic() {
                    self.vim_state.selected_register = Some(c.to_ascii_lowercase());
                }
                return;
            }
            VimPending::None => {}
        }

        // A key bound to a workflow (#228) runs it — a standalone Normal command,
        // so only when nothing (operator, count) is pending. Checked before the
        // remap and built-ins, so a bound key overrides its default meaning, the
        // author's choice (like the #208 remaps).
        if self.vim_state.op.is_none() && self.vim_state.count.is_none() {
            if let Some(name) = self.triggers.keys.get(&c).cloned() {
                self.run_workflow_named(&name);
                return;
            }
        }

        // Past the pending second-keys (find targets, register names, object
        // ids), a command key goes through the user's remaps (#208).
        let c = self.remap_key(c);

        // A digit (other than a leading `0`, which is the line-start motion)
        // accumulates the count prefix.
        if c.is_ascii_digit() && !(c == '0' && self.vim_state.count.is_none()) {
            let d = c.to_digit(10).unwrap_or(0) as usize;
            let count = self.vim_state.count.unwrap_or(0).saturating_mul(10).saturating_add(d);
            self.vim_state.count = Some(count);
            return;
        }

        // Commands that await another key keep the count (and any operator)
        // pending.
        let prefix = match c {
            'g' => Some(VimPending::G),
            'f' => Some(VimPending::Find(FindKind::FindFwd)),
            'F' => Some(VimPending::Find(FindKind::FindBack)),
            't' => Some(VimPending::Find(FindKind::TillFwd)),
            'T' => Some(VimPending::Find(FindKind::TillBack)),
            // `"x` selects the register for the next yank / delete / put.
            '"' => Some(VimPending::Register),
            _ => None,
        };
        if let Some(p) = prefix {
            self.vim_state.pending = p;
            return;
        }

        // Terminal commands consume the count.
        let raw = self.vim_state.count.take();
        let count = raw.unwrap_or(1);
        match c {
            // Operators: `d`/`c`/`y` await a motion or text object; a repeated
            // operator (`dd`/`cc`/`yy`) acts linewise on the current line(s).
            'd' => self.operator(Op::Delete, count),
            'c' => self.operator(Op::Change, count),
            'y' => self.operator(Op::Yank, count),
            // `Y` yanks the line (Vim's default), like `yy`.
            'Y' => {
                let cur = self.edit_buffer().cursor.line;
                self.operate_linewise(Op::Yank, cur, cur + count.saturating_sub(1));
            }
            // Put the register after (`p`) / before (`P`) the cursor or line.
            'p' => self.put(true, count),
            'P' => self.put(false, count),
            // Enter Visual mode (#205): `v` characterwise, `V` linewise.
            'v' | 'V' => self.mode = Mode::Visual { anchor: self.edit_buffer().cursor_byte(), linewise: c == 'V' },
            // `i`/`a` are a text-object prefix while an operator is pending,
            // otherwise they enter Insert.
            'i' if self.vim_state.op.is_some() => self.vim_state.pending = VimPending::Object { around: false },
            'a' if self.vim_state.op.is_some() => self.vim_state.pending = VimPending::Object { around: true },
            'i' => self.mode = Mode::Insert,
            // Append: after the cursor's grapheme, but never past the line end.
            'a' => {
                self.edit_move(|b| {
                    if b.cursor.col < b.line_len() {
                        b.move_right();
                    }
                });
                self.mode = Mode::Insert;
            }
            'I' => {
                self.edit_move(Buffer::move_first_non_blank);
                self.mode = Mode::Insert;
            }
            'A' => {
                self.edit_move(Buffer::move_line_end);
                self.mode = Mode::Insert;
            }
            'o' => self.open_line(false),
            'O' => self.open_line(true),

            // Single-key edits.
            'x' => self.delete_chars(count),
            'D' => self.delete_to_line_end(Op::Delete),
            'C' => self.delete_to_line_end(Op::Change),
            'u' => self.history_step(false),

            // Character motions (charwise: move, or an operator range).
            'h' => self.charwise(count, false, Buffer::move_left),
            'l' | ' ' => self.charwise(count, false, Buffer::move_right),

            // Word motions; `cw` acts like `ce` (to the word end), as in Vim.
            'w' if matches!(self.vim_state.op, Some((Op::Change, _))) => {
                self.charwise(count, true, Buffer::move_word_end);
            }
            'w' => self.charwise(count, false, Buffer::move_word_forward),
            'b' => self.charwise(count, false, Buffer::move_word_backward),
            'e' => self.charwise(count, true, Buffer::move_word_end),

            // Line motions.
            '0' => self.charwise(1, false, Buffer::move_line_start),
            '^' => self.charwise(1, false, Buffer::move_first_non_blank),
            '$' => self.charwise(1, true, Buffer::move_line_end),

            // Linewise vertical motions (`j`/`k`) and document jump (`G`).
            'j' => self.vertical(count, true),
            'k' => self.vertical(count, false),
            'G' => {
                let target = raw.map_or_else(|| self.edit_buffer().last_line_index(), |n| n.saturating_sub(1));
                self.goto_line(target);
            }

            // Repeat the last find: `;` same direction, `,` reversed.
            ';' => self.repeat_find(count, false),
            ',' => self.repeat_find(count, true),

            // Search (#206): `/` forward, `?` backward, `n`/`N` repeat / reverse,
            // `*` the word under the cursor.
            '/' => self.open_vim_search(false),
            '?' => self.open_vim_search(true),
            'n' => self.vim_repeat(count, |s| s.vim_search_repeat(false)),
            'N' => self.vim_repeat(count, |s| s.vim_search_repeat(true)),
            '*' => self.vim_search_word(),

            // Ex command line (#207): `:w`, `:q`, `:wq`, …
            ':' => self.open_ex_command(),

            // Anything else cancels a dangling operator.
            _ => self.vim_state.op = None,
        }

        // A `"x` register selection only lives until the operator/put that uses
        // it. If this key completed a command (nothing left pending), drop it.
        if self.vim_state.op.is_none() && self.vim_state.pending == VimPending::None {
            self.vim_state.selected_register = None;
        }
    }

    /// `d` / `c`: begin an operator awaiting a motion, or — when the same
    /// operator is already pending (`dd`/`cc`) — act linewise on the current
    /// line(s). `count` is this keypress's count.
    fn operator(&mut self, op: Op, count: usize) {
        match self.vim_state.op.take() {
            Some((pending, opc)) => {
                if pending == op {
                    let lines = opc.saturating_mul(count).max(1);
                    let cur = self.edit_buffer().cursor.line;
                    self.operate_linewise(op, cur, cur + lines - 1);
                }
                // A mismatched operator (`dc`) just cancels.
            }
            None => self.vim_state.op = Some((op, count)),
        }
    }

    /// Resolve an `i`/`a` text object (`diw`, `ci"`, …) and apply the pending
    /// operator to its range.
    fn apply_text_object(&mut self, id: char, around: bool) {
        let object = match id {
            'w' => Some(TextObject::Word),
            '"' | '\'' | '`' => Some(TextObject::Quote(id)),
            '(' | ')' | 'b' => Some(TextObject::Pair('(', ')')),
            '[' | ']' => Some(TextObject::Pair('[', ']')),
            '{' | '}' | 'B' => Some(TextObject::Pair('{', '}')),
            '<' | '>' => Some(TextObject::Pair('<', '>')),
            _ => None,
        };
        match (object, self.vim_state.op.take()) {
            (Some(object), Some((op, _))) => {
                if let Some((lo, hi)) = self.edit_buffer().text_object(object, around) {
                    self.operate_charwise(op, lo, hi);
                }
            }
            _ => self.vim_state.op = None, // an unknown object cancels
        }
    }

    /// Take a pending operator together with the effective count — the operator's
    /// own count times the motion's count (`2d3w` → 6). `None` when no operator is
    /// pending, so the motion just moves the cursor.
    fn take_op(&mut self, count: usize) -> Option<(Op, usize)> {
        self.vim_state.op.take().map(|(op, opc)| (op, count.saturating_mul(opc)))
    }

    /// A charwise motion: move the cursor, or — when an operator is pending —
    /// delete/change the span from the cursor to where the motion lands.
    /// `inclusive` covers the landing grapheme (Vim's `e`, `$`, `f`, `t`).
    fn charwise(&mut self, count: usize, inclusive: bool, step: impl Fn(&mut Buffer)) {
        if let Some((op, n)) = self.take_op(count) {
            let (lo, hi) = {
                let buf = self.edit_buffer_mut();
                let start = buf.cursor_byte();
                let saved = buf.cursor;
                for _ in 0..n {
                    step(buf);
                }
                let target = buf.cursor_byte();
                buf.cursor = saved;
                let hi = start.max(target);
                (start.min(target), if inclusive { buf.grapheme_after(hi) } else { hi })
            };
            self.operate_charwise(op, lo, hi);
        } else {
            self.edit_move(move |b| {
                for _ in 0..count {
                    step(b);
                }
            });
        }
    }

    /// A linewise vertical motion (`j`/`k`): move by visual rows, or — with an
    /// operator pending — delete/change whole lines.
    fn vertical(&mut self, count: usize, down: bool) {
        if let Some((op, n)) = self.take_op(count) {
            let cur = self.edit_buffer().cursor.line;
            let other = if down { cur + n } else { cur.saturating_sub(n) };
            self.operate_linewise(op, cur.min(other), cur.max(other));
        } else {
            let mv = if down { Buffer::move_down } else { Buffer::move_up };
            self.vim_repeat(count, move |s| s.edit_move_wrapped(mv));
        }
    }

    /// Jump to `target` (0-based line): move the cursor, or — with an operator
    /// pending (`dG`, `dgg`) — act linewise over the lines between.
    fn goto_line(&mut self, target: usize) {
        if let Some((op, _)) = self.take_op(1) {
            let cur = self.edit_buffer().cursor.line;
            self.operate_linewise(op, cur.min(target), cur.max(target));
        } else {
            self.edit_move(move |b| b.move_to_line(target));
        }
    }

    /// `D` / `C`: delete (or change) from the cursor to the end of the line.
    fn delete_to_line_end(&mut self, op: Op) {
        let buf = self.edit_buffer();
        let lo = buf.cursor_byte();
        let hi = buf.line_start_byte(buf.cursor.line) + buf.line(buf.cursor.line).len();
        self.operate_charwise(op, lo, hi);
    }

    /// `x`: delete `count` graphemes from under the cursor, never past the line.
    fn delete_chars(&mut self, count: usize) {
        let (lo, hi) = {
            let buf = self.edit_buffer();
            let lo = buf.cursor_byte();
            let line_end = buf.line_start_byte(buf.cursor.line) + buf.line(buf.cursor.line).len();
            let mut hi = lo;
            for _ in 0..count {
                hi = buf.grapheme_after(hi).min(line_end);
            }
            (lo, hi)
        };
        self.operate_charwise(Op::Delete, lo, hi);
    }

    /// Apply a charwise operator to byte range `[lo, hi)`: record the text in the
    /// register, then — for delete/change — remove it; yank leaves the text and
    /// just moves the cursor to the start. Change ends in Insert mode.
    fn operate_charwise(&mut self, op: Op, lo: usize, hi: usize) {
        if self.guard_vim_edit() {
            return;
        }
        if lo < hi {
            let text = {
                let buf = self.edit_buffer_mut();
                let text = buf.byte_slice(lo, hi);
                if op == Op::Yank {
                    buf.cursor = buf.cursor_at_byte(lo);
                } else {
                    buf.replace_byte_range(lo, hi, "");
                }
                text
            };
            self.store_register(Yank { text, linewise: false });
            if op != Op::Yank {
                self.cache_focus_words();
            }
        }
        if op == Op::Change {
            self.mode = Mode::Insert;
        }
    }

    /// Apply a linewise operator to the inclusive line range `[first, last]`:
    /// record the whole lines, then delete them (`dd`), clear them to one blank
    /// line and enter Insert (`cc`), or — for yank — leave them and move to the
    /// first.
    fn operate_linewise(&mut self, op: Op, first: usize, last: usize) {
        if self.guard_vim_edit() {
            return;
        }
        let text = {
            let buf = self.edit_buffer_mut();
            let last = last.min(buf.line_count().saturating_sub(1));
            let start = buf.line_start_byte(first);
            // The register always holds the full lines (with their newlines).
            let end = buf.next_line_byte(last);
            let text = buf.byte_slice(start, end);
            match op {
                Op::Delete => buf.replace_byte_range(start, end, ""),
                Op::Change => {
                    // Clear the lines' content but keep one line to type on.
                    let content_end = buf.line_start_byte(last) + buf.line(last).len();
                    buf.replace_byte_range(start, content_end, "");
                }
                Op::Yank => {}
            }
            text
        };
        self.store_register(Yank { text, linewise: true });
        self.edit_move(move |b| b.move_to_line(first));
        if op != Op::Yank {
            self.cache_focus_words();
        }
        if op == Op::Change {
            self.edit_move(Buffer::move_line_start);
            self.mode = Mode::Insert;
        }
    }

    /// Record a yank into the unnamed register, and into the named register if
    /// `"x` selected one.
    fn store_register(&mut self, yank: Yank) {
        if let Some(name) = self.vim_state.selected_register.take() {
            self.vim_state.registers.named.insert(name, yank.clone());
        }
        self.vim_state.registers.unnamed = Some(yank);
    }

    /// The register `p` / `P` reads from: the `"x`-selected named register, else
    /// the unnamed one.
    fn active_register(&mut self) -> Option<Yank> {
        match self.vim_state.selected_register.take() {
            Some(name) => self.vim_state.registers.named.get(&name).cloned(),
            None => self.vim_state.registers.unnamed.clone(),
        }
    }

    /// `p` / `P`: put the active register `count` times — inline for a charwise
    /// yank, on new lines for a linewise one.
    fn put(&mut self, after: bool, count: usize) {
        if self.guard_vim_edit() {
            return;
        }
        let Some(yank) = self.active_register() else { return };
        let body = yank.text.repeat(count);
        let buf = self.edit_buffer_mut();
        if yank.linewise {
            let line = buf.cursor.line;
            let at = if after { buf.next_line_byte(line) } else { buf.line_start_byte(line) };
            buf.replace_byte_range(at, at, &body);
            buf.move_to_line(if after { line + 1 } else { line });
        } else {
            let line_end = buf.line_start_byte(buf.cursor.line) + buf.line(buf.cursor.line).len();
            let at = if after {
                buf.grapheme_after(buf.cursor_byte()).min(line_end)
            } else {
                buf.cursor_byte()
            };
            buf.replace_byte_range(at, at, &body);
            buf.move_left(); // land on the last grapheme put, as Vim does
        }
        self.cache_focus_words();
    }

    /// Vim editing commands need the per-chapter buffer; like undo (#145), they're
    /// guarded out of the continuous view, whose segment bookkeeping a raw range
    /// edit would desync. Returns whether the command was blocked.
    fn guard_vim_edit(&mut self) -> bool {
        if self.manuscript.is_some() {
            self.message = Some("Editing commands aren't available in the continuous view — Ctrl-l to return.".into());
            true
        } else {
            false
        }
    }

    /// Vim Visual mode (#205): motions extend the selection; `d`/`c`/`y` act on
    /// it and leave Visual. Motion handling is reused from Normal mode (which just
    /// moves the cursor here, since no operator is pending).
    fn on_key_visual(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.on_key_ctrl(key.code);
            return;
        }
        let Mode::Visual { anchor, linewise } = self.mode else {
            return;
        };
        // Mid multi-key motion (an `f`/`g` second key, or a `"` register name):
        // let the Normal handler resolve it, extending the selection.
        if self.vim_state.pending != VimPending::None {
            self.on_key_normal(key);
            return;
        }
        // Non-character keys (Esc, arrows) handled directly; command chars go
        // through the user's remaps (#208) first.
        let c = match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Char(c) => self.remap_key(c),
            _ => return self.on_key_normal(key), // arrows / Home / End extend the selection
        };
        match c {
            // `v` / `V` switch the selection kind, or leave Visual if already it.
            'v' | 'V' => {
                let want_line = c == 'V';
                self.mode = if linewise == want_line {
                    Mode::Normal
                } else {
                    Mode::Visual { anchor, linewise: want_line }
                };
            }
            // `o` swaps the fixed and active ends of the selection.
            'o' => {
                let cur = self.edit_buffer().cursor_byte();
                let buf = self.edit_buffer_mut();
                buf.cursor = buf.cursor_at_byte(anchor);
                self.mode = Mode::Visual { anchor: cur, linewise };
            }
            // Operators act on the selection, then leave Visual (Insert for `c`).
            'd' | 'x' => self.visual_operate(Op::Delete, anchor, linewise),
            'c' | 's' => self.visual_operate(Op::Change, anchor, linewise),
            'y' => self.visual_operate(Op::Yank, anchor, linewise),
            // `p` replaces the selection with the register.
            'p' => self.visual_put(anchor, linewise),
            // Motions (and counts / multi-key prefixes) extend the selection by
            // reusing the Normal-mode dispatch (which remaps again to the same key).
            _ if is_motion_key(c) => self.on_key_normal(key),
            _ => {} // ignore anything else, so nothing slips into Insert / edits
        }
    }

    /// The inclusive line range of the current Visual selection.
    fn selection_lines(&self, anchor: usize) -> (usize, usize) {
        let buf = self.edit_buffer();
        let a = buf.cursor_at_byte(anchor).line;
        let b = buf.cursor.line;
        (a.min(b), a.max(b))
    }

    /// The byte range the current Visual selection covers — whole lines when
    /// linewise, else the chars between the ends (inclusive of the later one).
    /// Shared by the operators, put, and the highlight.
    pub(crate) fn selection_bytes(&self, anchor: usize, linewise: bool) -> (usize, usize) {
        let buf = self.edit_buffer();
        if linewise {
            let (first, last) = self.selection_lines(anchor);
            (buf.line_start_byte(first), buf.next_line_byte(last))
        } else {
            let cur = buf.cursor_byte();
            (anchor.min(cur), buf.grapheme_after(anchor.max(cur)))
        }
    }

    /// Apply an operator to the Visual selection, then leave Visual.
    fn visual_operate(&mut self, op: Op, anchor: usize, linewise: bool) {
        self.vim_state.count = None;
        if linewise {
            let (first, last) = self.selection_lines(anchor);
            self.operate_linewise(op, first, last);
        } else {
            let (lo, hi) = self.selection_bytes(anchor, false);
            self.operate_charwise(op, lo, hi);
        }
        // `c` left us in Insert; delete/yank return to Normal.
        if op != Op::Change {
            self.mode = Mode::Normal;
        }
    }

    /// `p` in Visual: replace the selection with the register, then leave Visual.
    fn visual_put(&mut self, anchor: usize, linewise: bool) {
        if self.guard_vim_edit() {
            return;
        }
        let Some(yank) = self.active_register() else {
            self.mode = Mode::Normal;
            return;
        };
        let (lo, hi) = self.selection_bytes(anchor, linewise);
        let buf = self.edit_buffer_mut();
        buf.replace_byte_range(lo, hi, &yank.text);
        buf.cursor = buf.cursor_at_byte(lo);
        self.cache_focus_words();
        self.mode = Mode::Normal;
    }

    /// Normal-mode handling for non-character keys: the arrow keys move (count-
    /// aware), Home/End go to the row edges, and Esc clears any pending command.
    fn on_key_normal_special(&mut self, code: KeyCode) {
        let count = self.take_count();
        // The arrow keys mirror hjkl, so they compose with a pending operator too
        // (`d<Down>` works like `dj`).
        match code {
            KeyCode::Left => self.charwise(count, false, Buffer::move_left),
            KeyCode::Right => self.charwise(count, false, Buffer::move_right),
            KeyCode::Up => self.vertical(count, false),
            KeyCode::Down => self.vertical(count, true),
            KeyCode::Home => self.edit_move_wrapped(Buffer::move_home),
            KeyCode::End => self.edit_move_wrapped(Buffer::move_end),
            _ => {} // Esc and the rest: the count is already cleared above.
        }
        // Esc / Home / End (and the rest) also cancel a pending operator/command;
        // the motion arrows already consumed any operator via charwise/vertical.
        self.vim_state.pending = VimPending::None;
        self.vim_state.op = None;
    }

    /// Take the pending count, defaulting to 1, and clear it.
    fn take_count(&mut self) -> usize {
        self.vim_state.count.take().unwrap_or(1)
    }

    /// Translate a pressed key through the user's remaps (#208): a remapped key
    /// runs the default key's action; an unmapped key is itself. Applied to
    /// command keys only — not to find targets, register names, or object ids.
    fn remap_key(&self, c: char) -> char {
        self.vim_keys.get(&c).copied().unwrap_or(c)
    }

    /// Run `motion` `n` times — how a count repeats a simple motion.
    fn vim_repeat(&mut self, n: usize, mut motion: impl FnMut(&mut App)) {
        for _ in 0..n {
            motion(self);
        }
    }

    /// Replay the last `f`/`t`/`F`/`T`, in its own direction (`reverse == false`)
    /// or the opposite — Vim's `;` and `,`. Operator-aware, so `d;` works.
    fn repeat_find(&mut self, count: usize, reverse: bool) {
        let Some(find) = self.vim_state.last_find else {
            self.vim_state.op = None;
            return;
        };
        let kind = if reverse { find.kind.reversed() } else { find.kind };
        let target = find.target;
        self.charwise(1, true, move |b| b.find_char(target, kind, count));
    }

    /// Vim `o` / `O`: open a blank line below (`above == false`) or above the
    /// current logical line and enter Insert mode there. Anchors on logical line
    /// boundaries so a soft-wrapped paragraph isn't split mid-way.
    fn open_line(&mut self, above: bool) {
        if above {
            self.edit_move(Buffer::move_line_start);
            self.edit_insert_char('\n');
            self.edit_move_wrapped(Buffer::move_up); // onto the new blank line
        } else {
            self.edit_move(Buffer::move_line_end);
            self.edit_insert_char('\n');
        }
        self.mode = Mode::Insert;
    }


}

/// The remappable Vim commands (#208): each named action and its default key.
/// `[editor.keys]` binds an action name to a different key; the action set is the
/// single-key Normal/Visual commands (`gg` and the `"` register prefix, being
/// multi-key, aren't remappable). Documented in the README.
const VIM_ACTIONS: &[(&str, char)] = &[
    ("left", 'h'),
    ("down", 'j'),
    ("up", 'k'),
    ("right", 'l'),
    ("word_forward", 'w'),
    ("word_backward", 'b'),
    ("word_end", 'e'),
    ("line_start", '0'),
    ("first_non_blank", '^'),
    ("line_end", '$'),
    ("last_line", 'G'),
    ("find_forward", 'f'),
    ("find_backward", 'F'),
    ("till_forward", 't'),
    ("till_backward", 'T'),
    ("repeat_find", ';'),
    ("repeat_find_reverse", ','),
    ("delete", 'd'),
    ("change", 'c'),
    ("yank", 'y'),
    ("delete_char", 'x'),
    ("delete_to_line_end", 'D'),
    ("change_to_line_end", 'C'),
    ("put_after", 'p'),
    ("put_before", 'P'),
    ("undo", 'u'),
    ("insert", 'i'),
    ("append", 'a'),
    ("insert_line_start", 'I'),
    ("append_line_end", 'A'),
    ("open_below", 'o'),
    ("open_above", 'O'),
    ("visual", 'v'),
    ("visual_line", 'V'),
    ("search_forward", '/'),
    ("search_backward", '?'),
    ("search_next", 'n'),
    ("search_prev", 'N'),
    ("search_word", '*'),
    ("ex_command", ':'),
];

/// Build the pressed-key → default-key remap from the `[editor.keys]` config
/// (#208). Each entry is `action = key`; the result maps that key to the
/// action's default key, so the existing dispatch runs unchanged. Returns any
/// bad entries (unknown action, or a value that isn't a single key) for
/// reporting — they're skipped, leaving the default binding in place.
fn build_keymap(keys: &HashMap<String, String>) -> (HashMap<char, char>, Vec<String>) {
    let mut map = HashMap::new();
    let mut errors = Vec::new();
    for (action, key) in keys {
        let Some(&(_, default)) = VIM_ACTIONS.iter().find(|(name, _)| name == action) else {
            errors.push(format!("unknown action '{action}'"));
            continue;
        };
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(k), None) => {
                if map.insert(k, default).is_some() {
                    errors.push(format!("key '{k}' is bound by more than one action"));
                }
            }
            _ => errors.push(format!("'{action}' must be a single key, got '{key}'")),
        }
    }
    (map, errors)
}

/// Whether `c` is a Normal-mode motion key — a count digit, a motion, or a
/// multi-key motion prefix (`g`/`f`/`"`). These are the keys Visual mode forwards
/// to the Normal handler to extend the selection. Keep in sync with the motion
/// arms of [`App::on_key_normal`]; a new motion added there must be listed here
/// to work in Visual mode too (forwarding fails safe — an omission just means the
/// motion doesn't extend a selection, never a stray edit).
fn is_motion_key(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            'h' | 'j' | 'k' | 'l' | ' ' | 'w' | 'b' | 'e' | '0' | '^' | '$' | 'g' | 'G' | 'f' | 'F'
                | 't' | 'T' | ';' | ',' | '"'
        )
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



#[cfg(test)]
mod tests;

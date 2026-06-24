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
use crate::buffer::{self, Buffer, TextObject};
use crate::date;
use crate::frontmatter::{Epigraph, FrontMatter, Role, Status};
use crate::manuscript::Manuscript;
use crate::mode::{
    BibleConflict, BibleReview, Command, Finding, Findings, GitAction,
    MetaEdit, MetaField, Mode, Overlay, Palette, PaletteItem, Pending, RefFiles, VimState,
};
use crate::project::{Manifest, Project};
use crate::recovery;
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

mod chapters;
mod counts;
mod persistence;
mod project_ops;
mod prompts;
mod vim;
mod wizard;

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
    /// Image-generation configuration (#238): the `[image]` section, opt-in and
    /// key-gated separately from the text `[ai]` settings.
    pub image: ai::image::ImageSettings,
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
    /// Cached local image model (#240), loaded once on first use and reused;
    /// empty for the remote image backend. Type-erased like `local_backend`.
    image_backend: Arc<Mutex<Option<Arc<dyn ai::image::ImageBackend>>>>,
    /// The portrait targets `(slug, name, prompt)` the author picked (#244),
    /// awaiting the cost confirm; taken when they confirm, cleared on cancel.
    pending_portraits: Option<Vec<(String, String, String)>>,
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
    /// The steps of an AI-proposed workflow (#232) awaiting the author's
    /// name-confirm prompt; taken when they save it, cleared when they cancel.
    pending_workflow: Option<Vec<String>>,
    /// The workflow currently executing (#227), if any: its resolved steps and
    /// where it is. `Some` while a run is in progress — including while it's
    /// suspended waiting on an async AI step. Drives the step state machine.
    workflow_run: Option<workflows::WorkflowRun>,
    /// Crash recovery (#263): unsaved work found at launch, awaiting the author's
    /// restore/discard choice. `Some` only while the recovery prompt is open;
    /// taken when they restore, cleared when they discard or defer.
    pending_recovery: Option<recovery::Recovery>,
    /// Whether a recovery sidecar is currently on disk for this session's edits,
    /// so the idle cadence clears it exactly once when the buffer goes clean
    /// rather than issuing a remove on every idle tick.
    recovery_written: bool,
    /// The project the author chose in the browser, held while the unsaved-
    /// changes confirm is open (#270); taken when they confirm the switch.
    pending_switch: Option<PathBuf>,
    /// A setup-wizard setting concept (#282) deferred until the characters draft
    /// finishes — only one AI pass runs at a time, so the second is queued here
    /// and fired by `poll_ai` when the first completes.
    wizard_pending_setting: Option<String>,
    /// A label for the notes import in flight (#283) — a file name or a count —
    /// shown in the summary when the pass applies; taken by `apply_import`.
    import_source: Option<String>,
    /// The notes folder chosen for "new project from notes" (#286), held between
    /// the folder prompt and the new-project-path prompt; taken when the project
    /// is created and the import runs.
    pending_notes_folder: Option<PathBuf>,
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
            image: ai::image::ImageSettings::load(),
            ai_task: Task::idle(),
            vc_task: Task::idle(),
            local_backend: Arc::new(Mutex::new(None)),
            image_backend: Arc::new(Mutex::new(None)),
            pending_portraits: None,
            pending_findings: Vec::new(),
            triage: Vec::new(),
            pending_pass: None,
            workflows: loaded.workflows,
            triggers: loaded.triggers,
            pending_workflow: None,
            workflow_run: None,
            pending_recovery: None,
            recovery_written: false,
            pending_switch: None,
            wizard_pending_setting: None,
            import_source: None,
            pending_notes_folder: None,
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
        self.capture_recovery();
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

        // F1 opens the user's guide from an editing mode (#281). Guarded so it
        // doesn't pre-empt a key's meaning elsewhere: with an overlay already up,
        // F1 falls through and closes it like any other key; in a prompt/menu the
        // mode owns its keys.
        if key.code == KeyCode::F(1)
            && self.overlay.is_none()
            && matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
        {
            self.open_guide();
            return;
        }

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
            Mode::ProjectBrowser(_) => self.on_key_project_browser(key),
            Mode::DirBrowser(_) => self.on_key_dir_browser(key),
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
        // A clean save means there's nothing left to recover (#263).
        if !self.edit_dirty() {
            self.clear_recovery();
        }
        // Checkpoint today's momentum to the history log.
        self.persist_snapshot();
        // Run the on-save workflow trigger (#228). This is the explicit-save
        // command (Ctrl-S / `:w`); implicit flushes (chapter switch, leaving the
        // continuous view) save the buffer directly and intentionally don't fire
        // it. A no-op when this save is itself a workflow's own step — no recursion.
        self.trigger_on_save();
    }

    // --- Crash recovery (#263) --------------------------------------------


    // --- Project browser & switching (#270) -------------------------------


    // --- New project (#273) -----------------------------------------------


    // --- New-project setup wizard (#282) ----------------------------------


    // --- Filesystem navigator (#271) --------------------------------------


    // --- Chapter structure editing ----------------------------------------
    //
    // Each wraps a `Project` op and returns the chapter the browser should now
    // select. The active chapter is tracked by file identity, so insert/rename/
    // move need no active fixup; only delete (of the active chapter) reloads.


    // --- Chapter metadata (front matter) ----------------------------------


    /// Quit, or — when there are unsaved changes — open the confirm prompt first.
    /// Shared by Ctrl-Q and `:q`.
    fn request_quit(&mut self) {
        if self.edit_dirty() {
            self.open_prompt(Pending::Quit, "Unsaved changes — quit anyway? (y/N)", "", None);
        } else {
            self.should_quit = true;
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
            // Open the project browser (#270) — switch to a recently-opened
            // project without leaving the editor. Ctrl-b: "browse" projects.
            KeyCode::Char('b') => self.open_project_browser(),
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

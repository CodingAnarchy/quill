//! Top-level application state and the key handling that drives editing. The
//! editor-mode types live in [`crate::mode`].

use crate::theme;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::ai::character::{self, Character};
use crate::ai::continuity::{self, Continuity};
use crate::ai::plot::{self, Plot};
use crate::ai::setting::{self, Setting};
// The AI theme/motif tracker (#828) — aliased `ai_theme` so it doesn't collide
// with `crate::theme`, the UI color theme used bare as `theme::` throughout `app`.
use crate::ai::theme as ai_theme;
use crate::ai::theme::Theme as ThemeBible;
use crate::ai::{self, bible, Backend, ClaudeBackend, Outcome};
use crate::analysis;
use crate::buffer::{self, Buffer, TextObject};
use crate::date;
use crate::frontmatter::{Epigraph, FrontMatter, Role, Status};
use crate::manuscript::Manuscript;
use crate::mode::{
    BookField, Command, Finding, FindingKind, Findings, GitAction, History, HistoryRow,
    ImportPreview, MetaEdit, MetaField, Mode, Overlay, Palette, PaletteItem, Pending, RefFiles,
    VimState,
};
use crate::project::{unique_dest, GoalField, Manifest, Project, DEFAULT_WORDS_PER_PAGE};
use crate::report::{self, Momentum, PartDelta};
use crate::typography;

/// Shown when the author tries to delete a project's last remaining chapter.
mod annotations;
mod api_keys;
mod archive;
mod assist;
mod assistant;
mod backup;
mod backup_browser;
mod batch_update;
mod bible_review;
mod corkboard;
mod dispatch;
mod editing;
mod export_style;
mod exporting;
mod findings_store;
mod folds;
mod fonts;
mod health;
mod images;
mod imports;
mod marks;
mod meta;
mod models;
pub mod mouse; // the click→byte layout map + mouse handlers (#757/#764)
mod nav;
mod outline;
mod palette;
mod panes;
mod pass_queue;
mod qa_store;
mod reports;
mod search;
mod settings;
pub mod task; // the generic background-task runner, reused by the assistant loop (#306)
mod vcs_actions;
mod worker;
mod workflows;

mod chapters;
mod configure;
mod cooccurrence;
mod counts;
mod persistence;
mod project_ops;
mod project_search;
mod prompts;
mod proximity_repeats; // the near-proximity word-repeat marking: toggle, dim-mark in place (#1002)
mod quick_switch;
mod sequenced;
mod series_bible;
mod spell;
pub mod sprint; // the writing-sprint state + status segment, read by the UI (#299)
mod style_flags; // the style-flag sweep: pick a category, step through it, underline it (#1001)
mod tree;
mod vim;
mod vim_case;
mod wizard;

pub use assistant::{Assistant, Entry, Speaker};
pub use backup::BackupEntry;
pub use folds::{FoldState, FoldView, GalleyState};
// The key-dispatch helpers moved to `dispatch` (#759); re-imported here so the
// `app`-module call sites keep spelling them unqualified / via `super::`.
use dispatch::{build_keymap, scroll_target};
pub use mouse::{EditorLayout, ListLayout};
#[cfg(test)]
pub use panes::SplitView; // the type name is only spelled in tests
use panes::{OtherPane, Split};
pub use panes::{SplitContent, SplitDir};
use task::{Poll, Task};

const ONLY_CHAPTER_MSG: &str = "Can't delete the only chapter";

mod state;
pub use state::App;

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        let under_vc = crate::vcs::is_repo(&project.root);
        // Register the project so every config loader below layers this project's
        // `book.toml` `[quill]` overrides over the global config (#710). Set before
        // the first load, and re-set here on a switch (open_in_place rebuilds the
        // App), so a project's overrides take effect the moment it opens and replace
        // the prior project's when another is opened in place.
        crate::config::set_project_dir(Some(project.root.clone()));
        let editor = crate::config::EditorConfig::load();
        let vim = editor.vim;
        let smart_typography = editor.smart_typography;
        let auto_pair = editor.auto_pair;
        let typewriter = editor.typewriter;
        let focus_mode = editor.focus_mode;
        let paragraph_indent = editor.paragraph_indent;
        let autosave_interval_secs = editor.autosave_interval_secs;
        let daily_nudge = editor.daily_nudge;
        let (vim_keys, key_errors) = build_keymap(&editor.keys);
        let (theme, theme_warnings) = crate::config::load_theme();
        // Make the loaded palette the active one for rendering (#300). `ui::draw`
        // re-affirms it each frame; setting it here covers content built before
        // the first draw (e.g. a report overlay).
        theme::set_current(theme);
        let loaded = crate::workflow::load();
        let trigger_errors = loaded.errors;
        let (snippets, snippet_errors) = crate::config::load_snippets();
        // Load the active chapter's annotations (#302) so they mark immediately,
        // before the first idle tick.
        let active_file = project.active_file().to_string();
        let annotations = project.annotations_for(&active_file);
        let annotations_context = format!("S:{active_file}");
        // Restore the durable revision-triage list (#591): the open (not
        // dismissed) findings from the project's store, so triage survives a
        // restart and picks up where the author left off.
        let findings_store = findings_store::FindingsStore::load(&project.root);
        let triage = findings_store.open();
        // Restore the durable "Ask the manuscript" Q&A history (#654): the persisted
        // asks, so a past lookup and its jumpable citations survive a restart
        // instead of the log rebuilding empty each launch.
        let qa_store = qa_store::QaStore::load(&project.root);
        let qa_log = qa_store.entries();
        let mut app = Self {
            buffer,
            project,
            manuscript: None,
            split: None,
            assistant_scroll_max: std::cell::Cell::new(0),
            window_pending: false,
            // With Vim mode on, the editor rests in Normal; otherwise it's always
            // the modeless Insert.
            mode: if vim { Mode::Normal } else { Mode::Insert },
            vim,
            smart_typography,
            auto_pair,
            theme,
            spell_flags: Vec::new(),
            spell_scratch: String::new(),
            style_flag_category: None,
            style_flags: Vec::new(),
            proximity_repeats_on: false,
            proximity_repeats: Vec::new(),
            routed_backend: std::cell::Cell::new(None),
            ai_in_flight: std::cell::Cell::new(None),
            routed_model: std::cell::Cell::new(None),
            routed_cloud_model: std::cell::Cell::new(None),
            ai_routing_notice: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            test_local_present: None,
            annotations,
            annotations_context,
            anno_undo: std::collections::VecDeque::new(),
            anno_redo: std::collections::VecDeque::new(),
            cork_undo: None,
            vim_state: VimState::default(),
            vim_keys,
            case_op: None,
            should_quit: false,
            focus_width: editor.column_width,
            typewriter,
            focus_mode,
            paragraph_indent,
            folds: FoldState::default(),
            distraction_free: false,
            galley: None,
            galley_geometry: std::cell::Cell::new((0, 0)),
            autosave_interval_secs,
            last_activity: std::time::Instant::now(),
            outline_visible: false,
            browser_opened_outline: false,
            under_vc,
            vc_changes: 0,
            vc_status_dirty: false,
            last_vc_refresh: std::time::Instant::now(),
            vc_status_idle_interval: vcs_actions::VC_STATUS_INTERVAL,
            vc_status_task: Task::idle(),
            overlay: None,
            viewport_height: 0,
            wrap_width: std::cell::Cell::new(72),
            surface_height: std::cell::Cell::new(0),
            pane_rects: std::cell::Cell::new(None),
            editor_layout: std::cell::RefCell::new(EditorLayout::default()),
            header_cache: std::cell::RefCell::new(crate::ui::HeaderCache::default()),
            list_layout: std::cell::RefCell::new(ListLayout::default()),
            scroll_offset: std::cell::Cell::new(0),
            added: 0,
            removed: 0,
            moved_to_unused: 0,
            part_added: Vec::new(),
            part_removed: Vec::new(),
            session_base_added: 0,
            session_base_removed: 0,
            last_history_snapshot: std::cell::RefCell::new(None),
            baselines: HashMap::new(),
            ai: ai::Settings::load(),
            image: ai::image::ImageSettings::load(),
            ai_task: Task::idle(),
            vc_task: Task::idle(),
            pull_origin: None,
            sync_conflicts: Vec::new(),
            pending_sync_conflicts: false,
            pending_draft_merge: None,
            backup: crate::config::BackupConfig::load(),
            backup_task: Task::idle(),
            backup_pending: false,
            last_backup_attempt: std::time::Instant::now(),
            last_backup: None,
            local_backup_task: Task::idle(),
            last_local_backup_attempt: std::time::Instant::now(),
            last_local_backup: None,
            download_task: Task::idle(),
            download_progress: None,
            downloaded_models: Vec::new(),
            local_backend: Arc::new(Mutex::new(HashMap::new())),
            image_backend: Arc::new(Mutex::new(None)),
            pending_portraits: None,
            pending_findings: Vec::new(),
            triage,
            findings_store,
            pending_recheck: None,
            pending_recheck_all: None,
            verify_baselines: std::collections::BTreeMap::new(),
            pending_pass: None,
            workflows: loaded.workflows,
            triggers: loaded.triggers,
            snippets,
            pending_workflow: None,
            pending_config: None,
            workflow_run: None,
            series_bible_run: None,
            pass_queue_run: None,
            pass_progress: Arc::new(Mutex::new(None)),
            batch_task: Task::idle(),
            batch_apply_book: std::cell::Cell::new(None),
            pending_recovery: None,
            recovery_written: false,
            pending_switch: None,
            pending_manage: None,
            pending_restore_rev: None,
            pending_milestone_name: None,
            wizard_pending_setting: None,
            import_source: None,
            pending_import_source: None,
            pending_archive_source: None,
            pending_restore_backup: None,
            sprint: None,
            session_target: None,
            daily_nudge,
            nudge_dismissed: false,
            qa_log,
            qa_store,
            pending_qa_question: None,
            accuracy_concern: None,
            relationship_pair: None,
            pass_scope: None,
            message: None,
            clipboard: Box::new(crate::clipboard::Osc52Clipboard::new()),
            mouse_anchor: None,
            palette_collapsed: HashMap::new(),
        };
        // Baselines + per-part counters for the daily word tally (sized to the
        // active book's parts; edits move each chapter from here).
        app.reset_tally_for_active_book();
        app.refresh_vc_status();
        // Surface dropped config — bad key remaps (#208) and bad workflow triggers
        // (#228) — rather than ignoring them silently.
        let mut warnings = Vec::new();
        // The project is open in another live Quill instance (#871): warn up front
        // that it's read-only, so the author knows saves are refused rather than
        // discovering it at the first Ctrl-S. Leads the warnings — it's the most
        // consequential thing to know.
        if let Some(holder) = app.project.busy_holder() {
            warnings.push(format!(
                "Read-only: this project is already open in {holder}. Saves are disabled."
            ));
        }
        if vim && !key_errors.is_empty() {
            warnings.push(format!("Vim keymap ignored: {}", key_errors.join("; ")));
        }
        if !trigger_errors.is_empty() {
            warnings.push(format!(
                "Workflow triggers ignored: {}",
                trigger_errors.join("; ")
            ));
        }
        if !theme_warnings.is_empty() {
            warnings.push(theme_warnings.join("; "));
        }
        if !snippet_errors.is_empty() {
            warnings.push(format!("Snippets ignored: {}", snippet_errors.join("; ")));
        }
        // A stale scheduled-local-backup warning (#888): if local backups are on but
        // the newest archive is older than the configured window, say so on open so
        // the author knows their exposure. Best-effort — off when disabled or recent.
        if let Some(stale) = app.stale_local_backup_warning() {
            warnings.push(stale);
        }
        if !warnings.is_empty() {
            app.message = Some(warnings.join(" · "));
        }
        app
    }

    /// Swap in the in-memory [`FakeClipboard`](crate::clipboard::FakeClipboard) so
    /// tests exercise the copy / paste seam without touching the real OS clipboard
    /// or emitting a terminal escape (#531, #811). Test-only.
    #[cfg(test)]
    pub(crate) fn use_fake_clipboard(&mut self) {
        self.clipboard = Box::new(crate::clipboard::FakeClipboard::new());
    }

    /// The current system-clipboard contents (#811) — what a copy last placed
    /// there, or a `set` from a test's fake. Reads through the seam, so tests can
    /// assert what a yank / delete / Visual copy wrote without a real clipboard.
    #[cfg(test)]
    pub(crate) fn clipboard_contents(&self) -> Option<String> {
        self.clipboard.get()
    }

    /// Place `text` on the clipboard through the seam (#811) — a test's stand-in
    /// for "the OS clipboard already holds this", so the paste path can be driven
    /// hermetically.
    #[cfg(test)]
    pub(crate) fn set_clipboard(&self, text: &str) {
        self.clipboard.set(text);
    }

    /// The buffer the author is currently editing: the continuous-view buffer
    /// when that view is active, otherwise the single active chapter. The
    /// renderer and movement go through this so both views share one path.
    pub fn edit_buffer(&self) -> &Buffer {
        match &self.mode {
            Mode::EditMeta(m) => &m.buffer,
            Mode::BibleReview(r) => {
                if let Some(b) = &r.editing {
                    return b;
                }
                self.manuscript
                    .as_ref()
                    .map_or(&self.buffer, Manuscript::buffer)
            }
            Mode::ConflictReview(r) => {
                if let Some(b) = &r.editing {
                    return b;
                }
                self.manuscript
                    .as_ref()
                    .map_or(&self.buffer, Manuscript::buffer)
            }
            _ => self
                .manuscript
                .as_ref()
                .map_or(&self.buffer, Manuscript::buffer),
        }
    }

    pub fn edit_buffer_mut(&mut self) -> &mut Buffer {
        match &mut self.mode {
            Mode::EditMeta(m) => &mut m.buffer,
            Mode::BibleReview(r) => {
                if let Some(b) = &mut r.editing {
                    return b;
                }
                match &mut self.manuscript {
                    Some(m) => m.buffer_mut(),
                    None => &mut self.buffer,
                }
            }
            Mode::ConflictReview(r) => {
                if let Some(b) = &mut r.editing {
                    return b;
                }
                match &mut self.manuscript {
                    Some(m) => m.buffer_mut(),
                    None => &mut self.buffer,
                }
            }
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
                                  // Drop fold keys whose heading the reparse just deleted or rewrote (#975), so
                                  // a stale fold can never hide live prose.
        self.prune_folds();
        self.refresh_spell_flags();
        self.refresh_style_flags();
        self.refresh_proximity_repeats();
        self.sync_annotations();
        self.tally_words();
        self.refresh_vc_status_on_idle();
        self.recheck_instance_lock();
        self.capture_recovery();
        // Poll any in-flight background bible batches (#950) — cheap: a no-op unless
        // the opt-in is on and the durable record has batches to reconcile.
        self.reconcile_batch_update();
        // Re-check open findings whose chapters changed (#1053) — a no-op unless the
        // opt-in is on; bounded to the open findings' changed chapters (content-digest
        // keyed), never a whole-manuscript sweep, and off the keystroke path.
        self.verify_findings_on_idle();
        // Mark this input burst's end, so autosave fires only after the
        // configured seconds of genuine idle (#409).
        self.last_activity = std::time::Instant::now();
    }

    /// Drop the revision-pass highlight caches on an edit (#1035), so the
    /// style-flag (#1001) and proximity-repeat (#1002) marks never render on
    /// pre-edit byte spans while the author is actively typing. Both caches are
    /// computed on the idle tick and address byte offsets into the buffer as it
    /// was at that tick; an insert or delete shifts everything after it, so
    /// between the edit and the next idle recompute a cached span points at the
    /// wrong characters. Clearing them here suppresses the marks for the length
    /// of an active typing burst — they reappear at the correct offsets on the
    /// next idle pass ([`on_idle`](Self::on_idle) re-runs both refreshes), which
    /// is the behavior we want (a mark shouldn't drift onto the wrong word
    /// mid-keystroke). This mirrors how the spell underline already blinks off
    /// while a misspelled word is being retyped.
    ///
    /// O(1) on the keystroke path: it truncates two small window-scoped vectors
    /// (no destructors to run, capacity retained for the next refresh) and does
    /// no rope, document-, or span-proportional work. The early return keeps it
    /// free once the caches are already empty (the common case — no pass on, or
    /// still mid-burst). Called from the shared edit chokepoint
    /// ([`record_edit_location`](Self::record_edit_location)), never per draw.
    pub(crate) fn invalidate_revision_highlights(&mut self) {
        if !self.style_flags.is_empty() {
            self.style_flags.clear();
        }
        if !self.proximity_repeats.is_empty() {
            self.proximity_repeats.clear();
        }
    }

    /// On idle, re-check a read-only project's instance lock and reclaim it if the
    /// holder we deferred to at open has since crashed (#890). A no-op unless we're
    /// running read-only; when the reclaim succeeds the project becomes writable
    /// and we tell the author a stale lock was cleared, so a confusing dead-end
    /// (saves silently refused) recovers transparently without a restart.
    ///
    /// Kept as its own additive path off the shared idle loop (#888 also edits the
    /// tick): it only touches the lock, never the parse/tally/backup work.
    fn recheck_instance_lock(&mut self) {
        if self.project.recheck_lock() {
            self.message = Some(
                "Stale lock cleared: another instance had exited; the project is writable again."
                    .into(),
            );
        }
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
}

#[cfg(test)]
mod tests;

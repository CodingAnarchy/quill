//! Top-level application state and the key handling that drives editing. The
//! editor-mode types live in [`crate::mode`].

use crate::theme;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::ai::character::{self, Character};
use crate::ai::continuity::{self, Continuity};
use crate::ai::plot::{self, Plot};
use crate::ai::setting::{self, Setting};
use crate::ai::{self, bible, Backend, ClaudeBackend, Outcome};
use crate::analysis;
use crate::buffer::{self, Buffer, TextObject};
use crate::date;
use crate::frontmatter::{Epigraph, FrontMatter, Role, Status};
use crate::manuscript::Manuscript;
use crate::mode::{
    BookField, Command, Finding, Findings, GitAction, History, MetaEdit, MetaField, Mode, Overlay,
    Palette, PaletteItem, Pending, RefFiles, VimState,
};
use crate::project::{GoalField, Manifest, Project, DEFAULT_WORDS_PER_PAGE};
use crate::recovery;
use crate::report::{self, Momentum, PartDelta};
use crate::typography;

/// Shown when the author tries to delete a project's last remaining chapter.
mod annotations;
mod api_keys;
mod archive;
mod assist;
mod assistant;
mod bible_review;
mod corkboard;
mod editing;
mod export_style;
mod exporting;
mod findings_store;
mod fonts;
mod images;
mod imports;
mod meta;
mod models;
mod nav;
mod palette;
mod panes;
mod reports;
mod search;
mod settings;
pub(crate) mod task; // the generic background-task runner, reused by the assistant loop (#306)
mod vcs_actions;
mod worker;
mod workflows;

mod chapters;
mod configure;
mod counts;
mod persistence;
mod project_ops;
mod project_search;
mod prompts;
mod sequenced;
mod series_bible;
pub(crate) mod sprint; // the writing-sprint state + status segment, read by the UI (#299)
mod tree;
mod vim;
mod wizard;

pub(crate) use assistant::{Assistant, Entry, Speaker};
use imports::PendingImportSource;
#[cfg(test)]
pub(crate) use panes::SplitView; // the type name is only spelled in tests
use panes::{OtherPane, Split};
pub(crate) use panes::{SplitContent, SplitDir};
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
    /// The assistant transcript's maximum scroll-up (#307), set by its renderer and
    /// read by its key handler to clamp Up/Down (mirrors `surface_height`). The
    /// conversation itself lives in the split's `OtherPane::Assistant`.
    pub(crate) assistant_scroll_max: std::cell::Cell<u16>,
    /// True between a `Ctrl-w` window prefix and the command key that follows it.
    window_pending: bool,
    pub mode: Mode,
    /// Opt-in Vim-style modal editing (#27), from the `[editor]` config. When on,
    /// the editor's resting state is [`Mode::Normal`] and the status bar shows the
    /// mode; when off, editing is exactly as it has always been.
    pub vim: bool,
    /// Smart typography (#296), from the `[editor]` config: substitute curly
    /// quotes / em-dashes / ellipses as the author types prose. On by default;
    /// when off, typed punctuation is inserted verbatim.
    pub smart_typography: bool,
    /// The active color theme (#300), from the `[theme]` config. Threaded through
    /// the render path so the palette is data, not hard-coded literals.
    pub theme: crate::theme::Theme,
    /// Byte ranges of misspelled words in the active (edit) buffer (#357),
    /// recomputed on the idle tick (never the keystroke path) and underlined by
    /// the editor. Empty until the first idle pass.
    pub(crate) spell_flags: Vec<(usize, usize)>,
    /// One-shot per-action backend override from the hybrid router (#372): set just
    /// before an AI request, consumed by `backend_spec`. `None` ⇒ the session
    /// backend. A `Cell` so the read-only `backend_spec` can take it.
    routed_backend: std::cell::Cell<Option<ai::BackendKind>>,
    /// The backend of the in-flight AI request (#372), for the status indicator:
    /// set when a request is built, cleared when it resolves.
    ai_in_flight: std::cell::Cell<Option<ai::BackendKind>>,
    /// One-shot local model the router picked for the next request (#377), set
    /// beside `routed_backend` when routing on-device and consumed by
    /// `backend_spec`. `None` ⇒ the single hardware-recommended model.
    routed_model: std::cell::Cell<Option<&'static ai::model_select::Model>>,
    /// Inline author annotations (#302) for the active chapter, as live buffer
    /// ranges (sorted by start). Loaded when the chapter changes and re-anchored
    /// on the idle tick; empty in the continuous view. The chapter `.md` never
    /// holds them, so exports stay clean.
    pub(crate) annotations: Vec<crate::annotation::Live>,
    /// A signature of the view the live [`annotations`](Self::annotations) were
    /// loaded for (#370): `"S:<file>"` in the single-chapter view, `"C:<idx,…>"`
    /// for the continuous window. The idle tick reloads (and re-translates)
    /// annotations when this changes — a chapter switch, a window page, or a view
    /// toggle — rather than re-anchoring across an unrelated layout.
    annotations_context: String,
    /// Annotation snapshots paralleling the buffer's undo / redo stacks (#370):
    /// one per edit group, pushed when a group opens and restored alongside the
    /// rope, so undo/redo moves notes back to where the text moved (a rope swap
    /// carries no edit delta to re-anchor through).
    anno_undo: std::collections::VecDeque<Vec<crate::annotation::Live>>,
    anno_redo: std::collections::VecDeque<Vec<crate::annotation::Live>>,
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
    /// Typewriter scrolling (#301), from the `[editor]` config: pin the cursor's
    /// line to the vertical centre of the page instead of letting it drift to the
    /// bottom. A toggle (palette action); off by default.
    pub typewriter: bool,
    /// Focus mode (#301), from the `[editor]` config: dim every paragraph but the
    /// one under the cursor. A toggle (palette action); off by default.
    pub focus_mode: bool,
    /// Autosave interval in seconds (#409), from `[editor]`; `0` is off.
    pub(crate) autosave_interval_secs: u64,
    /// Time of the last input burst (reset each idle tick), so autosave can fire
    /// only after the configured seconds of genuine idle (#409).
    pub(crate) last_activity: std::time::Instant,
    /// Whether the chapter / outline pane is shown.
    pub outline_visible: bool,
    /// Whether the chapter browser (Ctrl-G) is what turned the outline pane on,
    /// so Esc can dismiss the pane it opened (#529) while leaving an outline the
    /// author had already toggled on with Ctrl-O alone. `true` only while the
    /// browser is the reason the pane is visible.
    pub(crate) browser_opened_outline: bool,
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
    /// A background local-model download (#382): `(model id, result)`, polled in
    /// the event loop so a multi-gigabyte fetch never blocks the editor.
    download_task: Task<(String, Result<(), String>)>,
    /// Model ids known to be on disk (#382), refreshed when the manager opens and
    /// after a download/remove — so the manager shows status without probing the
    /// `hf-hub` cache every frame.
    downloaded_models: Vec<String>,
    /// The loaded local backend, cached after first use (loading a model is slow)
    /// and shared with the worker thread. Keyed by model id (#377) so several
    /// downloaded models can be cached and reused as the router picks between them;
    /// empty until the first local request. Type-erased so it needs no
    /// `local`-feature cfg here.
    local_backend: Arc<Mutex<HashMap<String, Arc<dyn Backend>>>>,
    /// Cached local image model (#240), loaded once on first use and reused;
    /// empty for the remote image backend. Type-erased like `local_backend`.
    image_backend: Arc<Mutex<Option<Arc<dyn ai::image::ImageBackend>>>>,
    /// The portrait targets `(slug, name, prompt)` the author picked (#244),
    /// awaiting the cost confirm; taken when they confirm, cleared on cancel.
    pending_portraits: Option<Vec<(String, String, String)>>,
    /// Continuity findings waiting to be shown — stashed while a timeline
    /// conflict is resolved first, then opened by [`App::open_findings`].
    pending_findings: Vec<Finding>,
    /// All open findings (continuity / setting / style / naming / gaps),
    /// de-duplicated — the "revision triage" list. Loaded from the project's
    /// durable store at startup and kept in sync with it, so dismissals stick and
    /// the list survives a restart (#591).
    triage: Vec<Finding>,
    /// The durable triage store (#591): every finding the project has seen plus
    /// the author's dismiss decisions, persisted as `bible/findings.toml`. Mirrors
    /// dismiss / mark actions so a re-run doesn't re-surface a dismissed finding.
    findings_store: findings_store::FindingsStore,
    /// The finding being re-checked (#608): its originating pass is re-running
    /// scoped to its chapters, and when the result lands [`App::apply_recheck`]
    /// clears it if it's gone or keeps it if still flagged. `Some` only while a
    /// recheck request is in flight; taken when its result applies.
    pending_recheck: Option<Finding>,
    /// The bible pass in flight and the chapter digests it was built from (#90),
    /// recorded to the incremental-scoping state file once the pass applies
    /// cleanly. `(tracker, digests, merge)` — `merge` keeps the other books'
    /// state when folding one book of a series (#339); `None` when none pending.
    pending_pass: Option<(String, ai::incremental::Digests, bool)>,
    /// Author-defined workflows (#226) from the `[workflows]` config — named,
    /// ordered sequences of registry actions run as one step. Loaded once at
    /// startup, sorted by name for a stable run-workflow menu.
    workflows: Vec<crate::workflow::Workflow>,
    /// Workflow triggers (#228) from the `[triggers]` config — on-save and bound
    /// Vim Normal-mode keys, validated against `workflows` at startup.
    triggers: crate::workflow::Triggers,
    /// Author-defined text snippets (#411) from the `[snippets]` config, sorted by
    /// name. Inserted verbatim from the snippet picker; empty when none are set.
    snippets: Vec<crate::config::Snippet>,
    /// The steps of an AI-proposed workflow (#232) awaiting the author's
    /// name-confirm prompt; taken when they save it, cleared when they cancel.
    pending_workflow: Option<Vec<String>>,
    /// AI-proposed config changes (#303) — validated `(dotted_key, value)` pairs
    /// awaiting the author's y/N confirm; taken on apply, cleared on cancel.
    pending_config: Option<Vec<(String, String)>>,
    /// The workflow currently executing (#227), if any: its resolved steps and
    /// where it is. `Some` while a run is in progress — including while it's
    /// suspended waiting on an async AI step. Drives the step state machine.
    workflow_run: Option<workflows::WorkflowRun>,
    /// A "Refresh series bible" run in progress (#340), if any: the queue of
    /// per-book bible passes and where it is. `Some` while the book-by-book
    /// refresh is running (including suspended on an in-flight pass).
    series_bible_run: Option<series_bible::SeriesBibleRun>,
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
    /// The project a browser rename / delete prompt targets (#612), held while
    /// the folder-name input or the delete confirm is open; taken when the action
    /// runs. Distinct from `pending_switch` (the switch target) so a manage
    /// action never disturbs a pending switch.
    pending_manage: Option<PathBuf>,
    /// A setup-wizard setting concept (#282) deferred until the characters draft
    /// finishes — only one AI pass runs at a time, so the second is queued here
    /// and fired by `poll_ai` when the first completes.
    wizard_pending_setting: Option<String>,
    /// A label for the notes import in flight (#283) — a file name or a count —
    /// shown in the summary when the pass applies; taken by `apply_import`.
    import_source: Option<String>,
    /// The source chosen for a two-step import wizard (#508): a folder of notes
    /// to seed a new project from (#286) or a manuscript to split into chapters
    /// (#298), held between the source prompt and the new-project-path prompt;
    /// taken when the project is created and the import runs.
    pending_import_source: Option<PendingImportSource>,
    /// The `.zip` archive chosen to restore a project from (#594), held between
    /// the source prompt and the new-folder-path prompt; taken when the folder is
    /// entered and the archive is unpacked.
    pending_archive_source: Option<PathBuf>,
    /// An active writing sprint (#299): a timed session with a live word count.
    /// Session state, never persisted; `None` when no sprint is running.
    pub(crate) sprint: Option<sprint::Sprint>,
    /// The in-session "Ask the manuscript" history (#609), newest last: each ask,
    /// its grounded answer, and the chapters it cited. Lets the author re-open a
    /// past lookup (and re-jump its citations) without re-asking. Never persisted
    /// — it's a session recall, not a stored artifact.
    pub(crate) qa_log: Vec<assist::QaEntry>,
    /// The question of the "Ask the manuscript" request in flight (#609), captured
    /// when it's sent so the answer can be filed under it in [`qa_log`](Self::qa_log);
    /// taken when the answer applies, `None` when no ask is pending.
    pub(crate) pending_qa_question: Option<String>,
    /// One-line transient message shown in the status bar.
    pub message: Option<String>,
}

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        let under_vc = crate::vcs::is_repo(&project.root);
        let editor = crate::config::EditorConfig::load();
        let vim = editor.vim;
        let smart_typography = editor.smart_typography;
        let typewriter = editor.typewriter;
        let focus_mode = editor.focus_mode;
        let autosave_interval_secs = editor.autosave_interval_secs;
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
            theme,
            spell_flags: Vec::new(),
            routed_backend: std::cell::Cell::new(None),
            ai_in_flight: std::cell::Cell::new(None),
            routed_model: std::cell::Cell::new(None),
            annotations,
            annotations_context,
            anno_undo: std::collections::VecDeque::new(),
            anno_redo: std::collections::VecDeque::new(),
            vim_state: VimState::default(),
            vim_keys,
            should_quit: false,
            focus_width: 72,
            typewriter,
            focus_mode,
            autosave_interval_secs,
            last_activity: std::time::Instant::now(),
            outline_visible: false,
            browser_opened_outline: false,
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
            download_task: Task::idle(),
            downloaded_models: Vec::new(),
            local_backend: Arc::new(Mutex::new(HashMap::new())),
            image_backend: Arc::new(Mutex::new(None)),
            pending_portraits: None,
            pending_findings: Vec::new(),
            triage,
            findings_store,
            pending_recheck: None,
            pending_pass: None,
            workflows: loaded.workflows,
            triggers: loaded.triggers,
            snippets,
            pending_workflow: None,
            pending_config: None,
            workflow_run: None,
            series_bible_run: None,
            pending_recovery: None,
            recovery_written: false,
            pending_switch: None,
            pending_manage: None,
            wizard_pending_setting: None,
            import_source: None,
            pending_import_source: None,
            pending_archive_source: None,
            sprint: None,
            qa_log: Vec::new(),
            pending_qa_question: None,
            message: None,
        };
        // Baselines + per-part counters for the daily word tally (sized to the
        // active book's parts; edits move each chapter from here).
        app.reset_tally_for_active_book();
        app.refresh_vc_status();
        // Surface dropped config — bad key remaps (#208) and bad workflow triggers
        // (#228) — rather than ignoring them silently.
        let mut warnings = Vec::new();
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
        self.refresh_spell_flags();
        self.sync_annotations();
        self.tally_words();
        self.refresh_vc_status();
        self.capture_recovery();
        // Mark this input burst's end, so autosave fires only after the
        // configured seconds of genuine idle (#409).
        self.last_activity = std::time::Instant::now();
    }

    /// Recompute the misspelled-word flags for the active buffer (#357), off the
    /// keystroke path. The configured TODO marker is never flagged.
    fn refresh_spell_flags(&mut self) {
        let marker = self.project.manifest.todo_marker().to_ascii_lowercase();
        let text = self.edit_buffer().text();
        // Accept the TODO marker and any word in the project's custom dictionary
        // (#358), so invented names aren't flagged.
        self.spell_flags = crate::spell::misspellings(&text, |w| {
            w.eq_ignore_ascii_case(&marker) || self.project.is_accepted(w)
        });
    }

    /// Move the cursor to the next (`dir >= 0`) or previous misspelled word in the
    /// active buffer (#359), wrapping around the ends. Refreshes the flag set
    /// first, so a word that's since been fixed or accepted isn't a target; when
    /// none remain it says so rather than moving silently.
    pub(crate) fn navigate_spell_flag(&mut self, dir: i32) {
        self.refresh_spell_flags();
        if self.spell_flags.is_empty() {
            self.message = Some("No spelling flags.".into());
            return;
        }
        let cur = self.edit_buffer().cursor_byte();
        let target = if dir >= 0 {
            self.spell_flags
                .iter()
                .find(|&&(s, _)| s > cur)
                .map_or(self.spell_flags[0].0, |&(s, _)| s)
        } else {
            self.spell_flags
                .iter()
                .rev()
                .find(|&&(s, _)| s < cur)
                .map_or(self.spell_flags[self.spell_flags.len() - 1].0, |&(s, _)| s)
        };
        let buffer = self.edit_buffer_mut();
        buffer.cursor = buffer.cursor_at_byte(target);
    }

    /// Accept the flagged word at the cursor into the project's custom dictionary
    /// (#358): add it (persisted to `dictionary.txt`), then clear the flag. The
    /// cursor must be on (or just past) a misspelled word; otherwise it says so.
    pub(crate) fn accept_word_at_cursor(&mut self) {
        let cursor = self.edit_buffer().cursor_byte();
        let flag = self
            .spell_flags
            .iter()
            .copied()
            .find(|&(s, e)| cursor >= s && cursor < e)
            // Also accept when the cursor rests just after the word (e.g. right
            // after typing it).
            .or_else(|| self.spell_flags.iter().copied().find(|&(_, e)| cursor == e));
        let Some((s, e)) = flag else {
            self.message = Some("No flagged word at the cursor.".into());
            return;
        };
        let word = self.edit_buffer().byte_slice(s, e);
        match self.project.accept_word(&word) {
            Ok(true) => {
                self.refresh_spell_flags();
                self.message = Some(format!("Added '{word}' to the dictionary."));
            }
            Ok(false) => self.message = Some(format!("'{word}' is already in the dictionary.")),
            Err(err) => self.message = Some(format!("Couldn't write the dictionary: {err}")),
        }
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

        // While a sequenced run is in progress (a workflow #227, or a series-bible
        // refresh #340) suspended on an async step, Esc cancels it — cleanly, after
        // the current step finishes (#344). Only in an editing mode: a prompt or
        // palette owns Esc itself (a cost-confirm cancels there).
        if self.sequenced_run_active()
            && key.code == KeyCode::Esc
            && matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
        {
            self.cancel_run();
            return;
        }

        // The key after a `Ctrl-w` window prefix (#163) is a window command, not
        // editing input.
        if self.window_pending {
            self.window_pending = false;
            self.window_command(key.code);
            return;
        }
        // The assistant pane takes all keys while focused (#307): typing the
        // request, submitting, scrolling, cancel/unfocus on Esc. Ctrl-w still
        // starts a window command (switch focus, close the pane).
        if self.assistant_pane_focused() {
            let ctrl_w = matches!(key.code, KeyCode::Char('w'))
                && key.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_w {
                self.window_pending = true;
            } else {
                self.on_key_assistant(key);
            }
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
            let ctrl_w = matches!(key.code, KeyCode::Char('w'))
                && key.modifiers.contains(KeyModifiers::CONTROL);
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
            Mode::History(_) => self.on_key_history(key),
            Mode::Annotations { .. } => self.on_key_annotations(key),
            Mode::Corkboard(_) => self.on_key_corkboard(key),
            Mode::ModelManager { .. } => self.on_key_model_manager(key),
            Mode::Palette(_) => self.on_key_palette(key),
            Mode::Search(_) => self.on_key_search(key),
            Mode::ProjectSearch(_) => self.on_key_project_search(key),
            Mode::RefFiles(_) => self.on_key_reffiles(key),
            Mode::Tree(_) => self.on_key_tree(key),
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
            self.open_prompt(
                Pending::Quit,
                "Unsaved changes — quit anyway? (y/N)",
                "",
                None,
            );
        } else {
            self.should_quit = true;
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
            // Open the AI command palette (opt-in). When AI is off, offer the in-app
            // on-ramp to enable it (#602) rather than a dead-end config instruction;
            // when it's on but unready (no key), explain rather than list dead actions.
            KeyCode::Char('a') => match self.ai.check_ready() {
                Ok(_) => self.open_ai_palette(),
                Err(_) if !self.ai.enabled => self.offer_enable_ai(None),
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
                // Remember whether the browser is what opened the pane, so Esc
                // can put the view back exactly as it was (#529) — hiding a pane
                // Ctrl-G revealed, but leaving an outline already up via Ctrl-O.
                self.browser_opened_outline = !self.outline_visible;
                self.outline_visible = true;
                self.select(self.project.active_index());
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl || alt {
            // The standard editing/motion combos (#641): word-wise cursor motion
            // (Ctrl-←/→) and word-delete (Ctrl/Alt-Backspace). Everything else
            // with Ctrl held is a Quill command (save, find, …).
            match key.code {
                KeyCode::Left => return self.edit_move(Buffer::move_word_backward),
                KeyCode::Right => return self.edit_move(Buffer::move_word_forward),
                KeyCode::Backspace => return self.edit_delete_word_back(),
                _ if ctrl => return self.on_key_ctrl(key.code),
                _ => return, // an Alt combo we don't bind
            }
        }
        match key.code {
            // In Vim mode, Esc leaves Insert for Normal; without it, Esc is inert.
            KeyCode::Esc if self.vim => self.mode = Mode::Normal,

            KeyCode::Char(c) => self.edit_insert_char(c),
            KeyCode::Enter => self.edit_insert_char('\n'),
            KeyCode::Backspace => self.edit_backspace(),
            KeyCode::Delete => self.edit_delete_forward(),
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
            KeyCode::PageUp => self.edit_page(false),
            KeyCode::PageDown => self.edit_page(true),

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

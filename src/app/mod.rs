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
use crate::ai::{self, bible, Backend, ClaudeBackend, Outcome};
use crate::analysis;
use crate::buffer::{self, Buffer, TextObject};
use crate::date;
use crate::frontmatter::{Epigraph, FrontMatter, Role, Status};
use crate::manuscript::Manuscript;
use crate::mode::{
    BookField, Command, Finding, Findings, GitAction, History, ImportPreview, MetaEdit, MetaField,
    Mode, Overlay, Palette, PaletteItem, Pending, RefFiles, VimState,
};
use crate::project::{unique_dest, GoalField, Manifest, Project, DEFAULT_WORDS_PER_PAGE};
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
mod dispatch;
mod editing;
mod export_style;
mod exporting;
mod findings_store;
mod fonts;
mod images;
mod imports;
mod meta;
mod models;
pub mod mouse; // the click→byte layout map + mouse handlers (#757/#764)
mod nav;
mod outline;
mod palette;
mod panes;
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
mod counts;
mod persistence;
mod project_ops;
mod project_search;
mod prompts;
mod sequenced;
mod series_bible;
pub mod sprint; // the writing-sprint state + status segment, read by the UI (#299)
mod tree;
mod vim;
mod wizard;

pub use assistant::{Assistant, Entry, Speaker};
// The key-dispatch helpers moved to `dispatch` (#759); re-imported here so the
// `app`-module call sites keep spelling them unqualified / via `super::`.
use dispatch::{build_keymap, scroll_target};
use imports::PendingImportSource;
pub use mouse::EditorLayout;
#[cfg(test)]
pub use panes::SplitView; // the type name is only spelled in tests
use panes::{OtherPane, Split};
pub use panes::{SplitContent, SplitDir};
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
    /// Reusable scratch holding the active buffer's text for the idle spell pass
    /// (#780), so `refresh_spell_flags` doesn't allocate a fresh whole-document
    /// `String` every idle tick — it refills this in place instead.
    spell_scratch: String,
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
    /// A one-line notice the worker sets when a routed-local request couldn't
    /// obtain its model (download/init failed) and fell back to Claude (#725), so
    /// the fallback is never silent. Shared with the worker thread; read and
    /// cleared by `poll_ai` when the outcome lands.
    ai_routing_notice: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Test-only override for "is a usable local model on disk?" (#725), so the
    /// hermetic routing tests can exercise the not-downloaded / downloaded paths
    /// without touching the real `hf-hub` cache or the network. `None` in
    /// production (the real cache probe is used).
    #[cfg(test)]
    pub(crate) test_local_present: Option<bool>,
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
    /// First-line paragraph indent (#722), from the `[editor]` config: visually
    /// indent the first line of each prose paragraph, display-only. A toggle
    /// (palette action); off by default.
    pub paragraph_indent: bool,
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
    /// after save / commit / init (when it can actually have changed) and on a
    /// bounded interval to catch external edits — never per idle tick from pure
    /// typing (#779), which can't change git status until a save. Zero when clean
    /// or not under version control.
    pub vc_changes: usize,
    /// Whether the working tree may have changed since the last status walk (#779):
    /// set by a save / commit / init, so the next idle tick recomputes the count
    /// instead of walking the tree after every input burst. The interval fallback
    /// ([`Self::last_vc_refresh`]) still catches edits made outside Quill.
    vc_status_dirty: bool,
    /// When the working-tree status was last recomputed (#779), for the interval
    /// fallback that picks up external edits even when nothing in-app marked the
    /// status dirty.
    last_vc_refresh: std::time::Instant,
    /// The off-thread working-tree status walk (#779): a `.gitignore`-respecting
    /// stat walk is the heaviest thing on the idle tick, so it runs on a worker and
    /// its resulting change count lands via [`Self::refresh_vc_status_on_idle`],
    /// never blocking the UI thread.
    vc_status_task: Task<usize>,
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
    /// The focused editor's per-visual-row screen layout (#757), recorded by the
    /// renderer each frame and read by the mouse-click handler to invert screen
    /// coordinates back to a buffer position without redoing the layout math. A
    /// `RefCell` because the renderer borrows `App` immutably; empty until the
    /// first draw or when an overlay replaces the editor.
    pub(crate) editor_layout: std::cell::RefCell<EditorLayout>,
    /// The editor viewport's read-ahead scroll offset (#764): visual rows the view
    /// is shifted down past the cursor-driven top, decoupled from the cursor. The
    /// renderer honors it and writes back the value clamped to the content, so a
    /// wheel-up right after over-scrolling responds immediately; any cursor move /
    /// edit snaps it back to zero, bringing the view back to the cursor as before.
    /// A `Cell` so the immutable render path can store the clamped offset.
    pub(crate) scroll_offset: std::cell::Cell<usize>,
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
    /// cleanly. `(tracker, digests, merge)` — `merge` is `Some(preserve)` for one
    /// book of a series: it folds this book's digests in while keeping the other
    /// books' entries (the `preserve` set) and pruning this book's stale ones,
    /// including deleted chapters (#339, #645); `None` replaces (standalone novel).
    /// The outer `Option` is `None` when no bible pass is pending.
    pending_pass: Option<(
        String,
        ai::incremental::Digests,
        Option<std::collections::BTreeSet<String>>,
    )>,
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
    /// The "Ask the manuscript" history (#609), newest last: each ask, its
    /// grounded answer, and the chapters it cited. Lets the author re-open a past
    /// lookup (and re-jump its citations) without re-asking. Loaded from the
    /// durable store at startup and appended to as asks return, so it survives a
    /// restart (#654).
    pub(crate) qa_log: Vec<assist::QaEntry>,
    /// The durable Q&A store (#654): the grounded-ask history persisted as
    /// `bible/qa-log.toml`, mirroring [`qa_log`](Self::qa_log) so a past lookup and
    /// its jumpable citations reload on open. Within the AI boundary — analysis
    /// recall (questions, answers, citations), never manuscript prose.
    qa_store: qa_store::QaStore,
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
        // Register the project so every config loader below layers this project's
        // `book.toml` `[quill]` overrides over the global config (#710). Set before
        // the first load, and re-set here on a switch (open_in_place rebuilds the
        // App), so a project's overrides take effect the moment it opens and replace
        // the prior project's when another is opened in place.
        crate::config::set_project_dir(Some(project.root.clone()));
        let editor = crate::config::EditorConfig::load();
        let vim = editor.vim;
        let smart_typography = editor.smart_typography;
        let typewriter = editor.typewriter;
        let focus_mode = editor.focus_mode;
        let paragraph_indent = editor.paragraph_indent;
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
            theme,
            spell_flags: Vec::new(),
            spell_scratch: String::new(),
            routed_backend: std::cell::Cell::new(None),
            ai_in_flight: std::cell::Cell::new(None),
            routed_model: std::cell::Cell::new(None),
            ai_routing_notice: std::sync::Arc::new(std::sync::Mutex::new(None)),
            #[cfg(test)]
            test_local_present: None,
            annotations,
            annotations_context,
            anno_undo: std::collections::VecDeque::new(),
            anno_redo: std::collections::VecDeque::new(),
            vim_state: VimState::default(),
            vim_keys,
            should_quit: false,
            focus_width: editor.column_width,
            typewriter,
            focus_mode,
            paragraph_indent,
            autosave_interval_secs,
            last_activity: std::time::Instant::now(),
            outline_visible: false,
            browser_opened_outline: false,
            under_vc,
            vc_changes: 0,
            vc_status_dirty: false,
            last_vc_refresh: std::time::Instant::now(),
            vc_status_task: Task::idle(),
            overlay: None,
            viewport_height: 0,
            wrap_width: std::cell::Cell::new(72),
            surface_height: std::cell::Cell::new(0),
            editor_layout: std::cell::RefCell::new(EditorLayout::default()),
            scroll_offset: std::cell::Cell::new(0),
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
            qa_log,
            qa_store,
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
            Mode::BibleReview(r) => {
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
        self.refresh_vc_status_on_idle();
        self.capture_recovery();
        // Mark this input burst's end, so autosave fires only after the
        // configured seconds of genuine idle (#409).
        self.last_activity = std::time::Instant::now();
    }

    /// Recompute the misspelled-word flags for the active buffer (#357), off the
    /// keystroke path. The configured TODO marker is never flagged.
    fn refresh_spell_flags(&mut self) {
        let marker = self.project.manifest.todo_marker().to_ascii_lowercase();
        // Refill the persistent scratch in place rather than allocating a fresh
        // whole-document `String` every idle tick (#780). Take it out so the
        // buffer read and the write target don't alias `self`, then put it back.
        let mut scratch = std::mem::take(&mut self.spell_scratch);
        self.edit_buffer().text_into(&mut scratch);
        // Accept the TODO marker and any word in the project's custom dictionary
        // (#358), so invented names aren't flagged.
        self.spell_flags = crate::spell::misspellings(&scratch, |w| {
            w.eq_ignore_ascii_case(&marker) || self.project.is_accepted(w)
        });
        self.spell_scratch = scratch;
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
        let Some((s, e)) = self.flagged_word_at_cursor() else {
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

    /// The flagged misspelling at (or nearest before) the cursor: the flag the
    /// cursor sits inside, or — right after typing the word — the one it rests just
    /// after. `None` when the cursor isn't on a flag. Shared by the accept (#358)
    /// and suggest (#647) paths.
    fn flagged_word_at_cursor(&self) -> Option<(usize, usize)> {
        let cursor = self.edit_buffer().cursor_byte();
        self.spell_flags
            .iter()
            .copied()
            .find(|&(s, e)| cursor >= s && cursor < e)
            .or_else(|| self.spell_flags.iter().copied().find(|&(_, e)| cursor == e))
    }

    /// Open a pick-list of offline spelling suggestions for the flagged word at the
    /// cursor (#647), and replace the word with the one the author picks. Within the
    /// assist boundary: the author singled out a flagged word and asked for
    /// corrections; Quill looks up candidates from the bundled dictionary and the
    /// author picks and places one — no prose is generated. Refreshes the flags
    /// first so a since-fixed word isn't offered; says so when the cursor isn't on a
    /// flag or the engine has no candidate.
    pub(crate) fn suggest_word_at_cursor(&mut self) {
        self.refresh_spell_flags();
        let Some((s, e)) = self.flagged_word_at_cursor() else {
            self.message = Some("No flagged word at the cursor.".into());
            return;
        };
        let word = self.edit_buffer().byte_slice(s, e);
        let suggestions = crate::spell::suggestions(&word);
        if suggestions.is_empty() {
            self.message = Some(format!("No suggestions for '{word}'."));
            return;
        }
        let items = suggestions
            .into_iter()
            .map(|sugg| {
                PaletteItem::new(
                    sugg.clone(),
                    format!("Replace '{word}' with '{sugg}'"),
                    Command::ReplaceMisspelling {
                        start: s,
                        end: e,
                        word: word.clone(),
                        replacement: sugg,
                    },
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Spelling suggestions \u{2014} pick a correction",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Replace the flagged word in byte range `[start, end)` with `replacement`
    /// (#647), the correction picked from the suggestions list. Goes through the
    /// normal edit path (so it works in both the single-chapter and continuous
    /// views) as one undo group: position the cursor at the word, delete it, then
    /// type the replacement verbatim (no typography substitution — the dictionary
    /// word lands exactly as chosen). Refreshes the flags so the corrected word
    /// clears at once. Guarded against a stale range (the buffer changed since the
    /// list opened): if the slice no longer matches `word`, it declines rather than
    /// corrupting other text.
    pub(crate) fn replace_misspelling(
        &mut self,
        start: usize,
        end: usize,
        word: &str,
        replacement: &str,
    ) {
        let buffer = self.edit_buffer();
        if end > buffer.len_bytes() || buffer.byte_slice(start, end) != word {
            self.message = Some("The text changed \u{2014} re-run suggestions.".into());
            return;
        }
        let buffer = self.edit_buffer_mut();
        buffer.seal_undo_group();
        buffer.cursor = buffer.cursor_at_byte(start);
        // Delete the flagged word, then type the correction — the same char-by-char
        // edit path a keystroke takes, so it's undoable and works in both views.
        for _ in word.chars() {
            self.edit_delete_forward();
        }
        for c in replacement.chars() {
            self.edit_insert_raw(c);
        }
        self.edit_buffer_mut().seal_undo_group();
        self.refresh_spell_flags();
        self.message = Some(format!("Replaced '{word}' with '{replacement}'."));
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

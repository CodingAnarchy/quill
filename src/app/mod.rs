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
use crate::history;
use crate::manuscript::Manuscript;
use crate::mode::{
    BookField, Command, Finding, FindingKind, Findings, GitAction, History, HistoryRow,
    ImportPreview, MetaEdit, MetaField, Mode, Overlay, Palette, PaletteItem, Pending, RefFiles,
    VimState,
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
mod quick_switch;
mod sequenced;
mod series_bible;
mod spell;
pub mod sprint; // the writing-sprint state + status segment, read by the UI (#299)
mod tree;
mod vim;
mod wizard;

pub use assistant::{Assistant, Entry, Speaker};
pub use backup::BackupEntry;
pub use folds::{FoldState, FoldView};
// The key-dispatch helpers moved to `dispatch` (#759); re-imported here so the
// `app`-module call sites keep spelling them unqualified / via `super::`.
use dispatch::{build_keymap, scroll_target};
use imports::PendingImportSource;
pub use mouse::{EditorLayout, ListLayout};
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
    /// One-shot *cloud* model the per-action router picked for the next request
    /// (#930): the Claude model this pass's weight + estimate resolved to, set in
    /// `route_request` and consumed by `spawn_ai` so the actual request runs on the
    /// chosen tier (a heavy pass on the top model, a light small one on a cheaper
    /// one). `None` ⇒ the session `[ai] model`, as before — so a pass that never
    /// went through the per-action picker (interactive, local) is unchanged.
    routed_cloud_model: std::cell::Cell<Option<String>>,
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
    /// The single undoable cross-chapter scene move on the manuscript corkboard
    /// (#974). A cross-chapter move rewrites two chapter files at once, outside the
    /// buffer's own rope-level undo, so it captures both chapters' pre-move contents
    /// here; the board's `u` key restores them (and reopens the board on the moved
    /// scene). `None` when there's nothing to undo — one level, cleared once used or
    /// once another structural corkboard edit supersedes it.
    cork_undo: Option<corkboard::CrossMoveUndo>,
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
    /// Heading/scene folding (#975): the collapsed sections, keyed by heading start
    /// byte. Purely display-only — the renderer hides a folded section's body lines
    /// and marks its heading, never touching the rope or the file. Empty by default.
    pub folds: FoldState,
    /// Distraction-free composition mode (#975): a runtime toggle that hides the
    /// chrome (outline pane, status bar, and any split) so only the centered prose
    /// column shows. Composes with focus mode and folding. Off by default; a
    /// per-session view state, never persisted.
    pub distraction_free: bool,
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
    /// The current interval before the fallback re-walks the tree (#869). Starts at
    /// the base and backs off (doubling to a cap) each idle walk that finds the
    /// count unchanged, so an untouched editor stops re-walking on a fixed short
    /// clock; any change (or a save marking the status dirty) resets it to the base.
    vc_status_idle_interval: std::time::Duration,
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
    /// The two pane rectangles `(focused, other)` when a split is drawn (#163/#165),
    /// set by the renderer each frame (`None` when not split or too cramped to show
    /// both). Read by the mouse handler to switch focus on a click in the other pane.
    pub(crate) pane_rects: std::cell::Cell<Option<(ratatui::layout::Rect, ratatui::layout::Rect)>>,
    /// The focused editor's per-visual-row screen layout (#757), recorded by the
    /// renderer each frame and read by the mouse-click handler to invert screen
    /// coordinates back to a buffer position without redoing the layout math. A
    /// `RefCell` because the renderer borrows `App` immutably; empty until the
    /// first draw or when an overlay replaces the editor.
    pub(crate) editor_layout: std::cell::RefCell<EditorLayout>,
    /// The focused editor's cached display-only header blocks (#869), rebuilt only
    /// when an input to them changes (a resize, theme switch, edited epigraph, paged
    /// manuscript window, or entering/leaving epigraph editing) rather than every
    /// frame. A `RefCell` because the renderer borrows `App` immutably.
    pub(crate) header_cache: std::cell::RefCell<crate::ui::HeaderCache>,
    /// The focused list overlay's per-frame row layout (#812), recorded by the
    /// list-overlay renderer so a click can be mapped back to a list index the way
    /// [`editor_layout`](Self::editor_layout) maps a click to a buffer byte. A
    /// `RefCell` (the renderer borrows `App` immutably); empty when no navigable
    /// list overlay is drawn.
    pub(crate) list_layout: std::cell::RefCell<ListLayout>,
    /// The editor viewport's signed scroll offset (#764/#936): visual rows the view
    /// is shifted off the cursor-driven top, decoupled from the cursor. Positive
    /// scrolls down past the cursor (read-ahead); negative scrolls up above it, so
    /// the author can page back through earlier prose even with the cursor at the
    /// bottom of the screen. The renderer honors it and writes back the value
    /// clamped to the content (top of chapter / no blank tail), so a wheel in the
    /// clamped direction responds immediately rather than draining a runaway
    /// accumulation; any cursor move / edit snaps it back to zero, bringing the view
    /// back to the cursor. A `Cell` so the immutable render path can store the
    /// clamped offset.
    pub(crate) scroll_offset: std::cell::Cell<isize>,
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
    /// The `added` / `removed` totals at the start of the current writing session
    /// (#835). A session's output is the growth since this checkpoint; recording a
    /// session (at quit) appends the delta and advances the checkpoint, so the
    /// per-session log accumulates bursts the coalesced day snapshot can't show.
    session_base_added: usize,
    session_base_removed: usize,
    /// The last snapshot [`persist_snapshot`](Self::persist_snapshot) actually wrote
    /// to the history log, so an autosave whose totals are unchanged skips appending
    /// a duplicate day line (#869). A `RefCell` because the best-effort persist runs
    /// off `&self`; `None` until the first write this session.
    last_history_snapshot: std::cell::RefCell<Option<history::Snapshot>>,
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
    /// The active chapter's path when a pull was spawned (#870), so the post-pull
    /// reload targets the chapter that started the pull — not whatever the author
    /// navigated to during a network-slow pull. `Some` only while a pull is in
    /// flight; cleared when the result lands. A pull that lands after the author
    /// moved on (or typed unsaved edits) skips the destructive reload rather than
    /// overwriting a now-active chapter's buffer from the just-pulled disk state.
    pull_origin: Option<PathBuf>,
    /// Per-chapter conflicts from the last two-way sync (#962) — chapters changed on
    /// both machines that sync detected but couldn't merge. Preserved (both versions
    /// intact) and surfaced here for the conflict-resolution UI (#963); empty after a
    /// clean sync. The last sync's result, not a running log.
    sync_conflicts: Vec<crate::vcs::ChapterConflict>,
    /// Automatic off-site backup config (#887): the opt-in `[backup]` section,
    /// layered per project. Drives whether a save / the timed cadence transparently
    /// pushes to the remote. Off by default — no behavior change when disabled.
    pub(crate) backup: crate::config::BackupConfig,
    /// The in-flight automatic backup push (#887), on its own worker task so it
    /// never collides with the author's manual push/pull (`vc_task`). Polled each
    /// loop tick by [`App::poll_backup`].
    backup_task: Task<backup::BackupOutcome>,
    /// Whether a save has landed since the last backup, so the on-save trigger
    /// owes a push (#887). Set on the post-save tail, cleared when a backup fires
    /// (or is gated off for want of a remote); read by the cadence hook.
    backup_pending: bool,
    /// When the last automatic backup was *attempted* (#887) — the clock the timed
    /// interval measures against and the reschedule point after a failure. Advanced
    /// on every attempt, success or not, so a failed push waits out one interval
    /// before retrying rather than hammering an offline remote every tick.
    last_backup_attempt: std::time::Instant,
    /// When the last backup *succeeded* (#887), so the author can see how current
    /// the off-site copy is. `None` until the first success this session; only a
    /// successful push stamps it, so a run of failures never claims the work is
    /// backed up.
    last_backup: Option<std::time::Instant>,
    /// The in-flight scheduled *local* archive (#888), on its own worker task so a
    /// zip never blocks the UI and never collides with the remote push (`backup_task`).
    /// Polled each loop tick by [`App::poll_local_backup`].
    local_backup_task: Task<backup::LocalBackupOutcome>,
    /// When the last local archive was *attempted* (#888) — the clock the local
    /// hourly interval measures against and the reschedule point after a failure.
    /// Advanced on every attempt so a failing archive waits out one interval before
    /// retrying rather than hammering the disk every tick.
    last_local_backup_attempt: std::time::Instant,
    /// When the last local archive *succeeded* this session (#888), so the author
    /// can see how current the on-disk copy is. `None` until the first success;
    /// only a successful archive stamps it.
    last_local_backup: Option<std::time::Instant>,
    /// A background local-model download (#382): `(model id, result)`, polled in
    /// the event loop so a multi-gigabyte fetch never blocks the editor.
    download_task: Task<(String, Result<(), String>)>,
    /// Live progress of the in-flight model download (#813): the shared byte
    /// counter the worker's `hf-hub` fetch drives, plus when the download started
    /// (for the ETA). `Some` only while a download runs; the terminal poll clears
    /// it. Read each tick by [`App::poll_model_download`] to render progress in
    /// Quill's own status line rather than over the TUI (#727).
    download_progress: Option<(Arc<models::DownloadProgress>, std::time::Instant)>,
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
    /// The findings a re-check-all is verifying (#830): every open finding whose
    /// originating pass was re-run (grouped by pass + chapters into one worker
    /// batch), so when the batch lands [`App::apply_recheck_all`] can reconcile
    /// each — clearing the ones the fresh pass no longer produces, keeping the
    /// rest. `Some` only while a re-check-all is in flight; taken when it applies.
    pending_recheck_all: Option<Vec<Finding>>,
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
    /// A durable multi-pass queue run in progress (#923), if any: the ordered
    /// passes (persisted to `bible/.queue.toml`), the cursor, and the shared
    /// sequenced-run bookkeeping. `Some` while the queue runs (including suspended
    /// on an in-flight pass); a crash mid-run leaves the file to resume from.
    pass_queue_run: Option<pass_queue::PassQueueRun>,
    /// Per-pass sub-progress (#923): the `(done, total)` chapter counter the
    /// in-flight fold pass updates on the worker thread as it walks chapters, so
    /// the run's status can read "continuity: chapter 5/12". `None` between the
    /// chapter-granular fold steps and for one-shot passes (which show pass-level
    /// progress only). Shared so the worker can write while the UI thread reads.
    pass_progress: Arc<Mutex<Option<(usize, usize)>>>,
    /// The opt-in background batch bible update's in-flight worker (#950): submits
    /// the full-manuscript rebuild batches, or polls the persisted ones to reconcile
    /// them. Its own task, separate from [`ai_task`](App::ai_task), so a background
    /// submit/reconcile never blocks — nor is blocked by — an interactive AI pass.
    /// Polled each loop tick by [`App::poll_batch_update`].
    batch_task: Task<Vec<batch_update::BatchTaskResult>>,
    /// The book a reconciled batch pass's bible write is being applied for (#950),
    /// so its findings are attributed to that book — the batch counterpart of the
    /// series-refresh / pass-queue `current_pass_book`. Set only while a reconciled
    /// batch is being applied off the normal run path; `None` otherwise.
    batch_apply_book: std::cell::Cell<Option<usize>>,
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
    /// The revision a whole-project restore confirm (#833) targets — a milestone or
    /// commit — held while the y/N prompt is open; taken when the restore runs.
    /// Kept off the `Copy` `Pending` enum so the revision string lives here.
    pending_restore_rev: Option<String>,
    /// The name of the milestone a note prompt (#966) targets — the just-named
    /// milestone awaiting its optional note, or the existing milestone whose note
    /// is being edited. Held while the note input is open; taken when the
    /// milestone is created or re-annotated. Kept off the `Copy` `Pending` enum so
    /// the name string lives here.
    pending_milestone_name: Option<String>,
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
    /// The local backup archive a restore confirm (#934) targets, held while the
    /// y/N prompt is open (from the backup browser, or the on-open damage offer);
    /// taken when the restore runs. Kept off the `Copy` `Pending` enum so the path
    /// lives here.
    pending_restore_backup: Option<PathBuf>,
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
    /// The system-clipboard seam (#811): a yank / delete / Visual copy writes the
    /// text here (in addition to the internal Vim registers), and paste reads it.
    /// The running editor uses the OSC 52 terminal clipboard (works over SSH /
    /// tmux); tests swap in a fake so the suite never touches a real clipboard
    /// (#531). Behind a trait object so the seam is testable and portable.
    pub(crate) clipboard: Box<dyn crate::clipboard::Clipboard>,
    /// The absolute buffer byte a left-button press landed on (#812), held between
    /// the `Down` and the first `Drag` so a drag can anchor a Visual selection
    /// there. `Some` only while the button is held after a press in the editor
    /// text; cleared on button-up. A plain press-and-release (no drag) never uses
    /// it, so a click still just positions the cursor.
    pub(crate) mouse_anchor: Option<usize>,
    /// Remembered group-collapse state per palette (#943), keyed by the palette's
    /// title (e.g. "Run action"). A palette re-reads its saved set on open and
    /// writes the author's collapse/expand changes back, so the groups they
    /// collapsed stay collapsed across opens (within the session). The run-action
    /// palette seeds this all-collapsed the first time it opens.
    pub(crate) palette_collapsed: HashMap<&'static str, std::collections::HashSet<&'static str>>,
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
            should_quit: false,
            focus_width: editor.column_width,
            typewriter,
            focus_mode,
            paragraph_indent,
            folds: FoldState::default(),
            distraction_free: false,
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
            qa_log,
            qa_store,
            pending_qa_question: None,
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
                                  // Drop fold keys whose heading the reparse just deleted or rewrote (#975), so
                                  // a stale fold can never hide live prose.
        self.prune_folds();
        self.refresh_spell_flags();
        self.sync_annotations();
        self.tally_words();
        self.refresh_vc_status_on_idle();
        self.recheck_instance_lock();
        self.capture_recovery();
        // Poll any in-flight background bible batches (#950) — cheap: a no-op unless
        // the opt-in is on and the durable record has batches to reconcile.
        self.reconcile_batch_update();
        // Mark this input burst's end, so autosave fires only after the
        // configured seconds of genuine idle (#409).
        self.last_activity = std::time::Instant::now();
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

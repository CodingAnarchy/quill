//! The top-level [`App`] state struct — the editor's whole in-memory spine.
//!
//! Split out of [`crate::app`] (#459: files under ~1,000 lines) so the module
//! that wires the submodules and holds the lifecycle `impl App` stays lean. This
//! file is *only* the struct definition; every method lives in `app/mod.rs` or a
//! sibling `app/` submodule, all of which spell the type `crate::app::App` (the
//! re-export in `mod.rs` keeps that path resolving exactly as before).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::ai::{self, Backend, Outcome};
use crate::buffer::Buffer;
use crate::history;
use crate::lastpos;
use crate::manuscript::Manuscript;
use crate::mode::{AiAction, Finding, Mode, Overlay, VimState};
use crate::project::Project;
use crate::recovery;

use super::imports::PendingImportSource;
use super::mouse::{EditorLayout, ListLayout};
use super::panes::Split;
use super::task::Task;
use super::vcs_actions::VcMsg;
use super::{
    assist, backup, batch_update, corkboard, findings_store, models, pass_queue, proximity_repeats,
    qa_store, reports, series_bible, spell, sprint, style_flags, workflows, FoldState, GalleyState,
};

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
    pub(super) split: Option<Split>,
    /// The assistant transcript's maximum scroll-up (#307), set by its renderer and
    /// read by its key handler to clamp Up/Down (mirrors `surface_height`). The
    /// conversation itself lives in the split's `OtherPane::Assistant`.
    pub(crate) assistant_scroll_max: std::cell::Cell<u16>,
    /// True between a `Ctrl-w` window prefix and the command key that follows it.
    pub(super) window_pending: bool,
    pub mode: Mode,
    /// The boolean `[editor]` knobs (Vim mode, smart typography, auto-pair,
    /// typewriter scrolling, focus mode, paragraph indent, the annotation margin,
    /// the daily-goal cue), copied whole from the config at startup and on a live
    /// reload. The palette toggles flip them for the session; see
    /// [`SettingBool::SPECS`](crate::mode::SettingBool::SPECS) for each knob.
    pub flags: crate::config::EditorFlags,
    /// The active color theme (#300), from the `[theme]` config. Threaded through
    /// the render path so the palette is data, not hard-coded literals.
    pub theme: crate::theme::Theme,
    /// Byte ranges of misspelled words in the active (edit) buffer (#357),
    /// recomputed on the idle tick (never the keystroke path) and underlined by
    /// the editor. Empty until the first idle pass.
    pub(crate) spell_flags: Vec<(usize, usize)>,
    /// The edit buffer [`revision`](crate::buffer::Buffer::revision) the
    /// `spell_flags` are aligned with. The per-batch settled pass remaps the flags
    /// through each burst's edit deltas (keeping this current) and recomputes them
    /// only when the buffer changed some other way — so a stale flag never
    /// underlines shifted text, without a full spell pass per keystroke.
    pub(crate) spell_rev: u64,
    /// Reusable scratch holding the active buffer's text for the idle spell pass
    /// (#780), so `refresh_spell_flags` doesn't allocate a fresh whole-document
    /// `String` every idle tick — it refills this in place instead.
    pub(super) spell_scratch: String,
    /// What the last full spell judgment ran against (#357): the buffer revision,
    /// the dictionary generation, and the TODO marker. The idle pass skips the
    /// whole-window copy and scan when all three still match. A remap through edit
    /// deltas moves `spell_rev` but never this — remapped flags are not a fresh
    /// judgment, so the next quiet pass after an edit still re-judges.
    pub(super) spell_judged: Option<spell::SpellKey>,
    /// The style-flag category the author is sweeping (#1001), or `None` when the
    /// sweep is off. When set, its occurrences in the active buffer are underlined
    /// and `]r` / `[r` step between them; toggling it off clears the marks.
    pub(crate) style_flag_category: Option<crate::analysis::FlagCategory>,
    /// Byte ranges of the active [`style_flag_category`](Self::style_flag_category)'s
    /// occurrences in the edit buffer (#1001), recomputed on the idle tick (never the
    /// keystroke path or per draw) and underlined by the editor. Empty when no sweep
    /// is active.
    pub(crate) style_flags: Vec<(usize, usize)>,
    /// The inputs `style_flags` were last computed from (buffer revision, category,
    /// run-on threshold, word lists), so an idle pass with nothing changed — a
    /// cursor move, a mouse hover — skips the rescan. `None` forces a recompute.
    pub(super) style_flags_key: Option<style_flags::StyleKey>,
    /// Whether the near-proximity word-repeat marking is on (#1002): a deliberate
    /// revision-pass aid, off by default so it's never always-on noise. When on, the
    /// visible window's content words echoed within a short span dim-mark in place,
    /// recomputed on the idle tick; toggling it off clears the cache and the marks.
    pub(crate) proximity_repeats_on: bool,
    /// Byte ranges of near-proximity repeated content words in the edit buffer
    /// (#1002), recomputed on the idle tick (never the keystroke path or per draw)
    /// and dim-marked by the editor. Empty when the marking is off.
    pub(crate) proximity_repeats: Vec<(usize, usize)>,
    /// The inputs `proximity_repeats` were last computed from (buffer revision and
    /// word lists); an idle pass that matches skips the rescan. `None` forces one.
    pub(super) proximity_key: Option<proximity_repeats::ProximityKey>,
    /// The effective analysis word lists (#119), cached against the manifest's
    /// `[analysis]` section they were built from, so the idle style / proximity
    /// passes don't rebuild the stopword and filter-word sets every time. Compared
    /// by value on each use, so any change to the section (settings edit, config
    /// reload, project switch) rebuilds it.
    pub(super) word_lists: RefCell<Option<reports::WordListsCache>>,
    /// One-shot per-action backend override from the hybrid router (#372): set just
    /// before an AI request, consumed by `backend_spec`. `None` ⇒ the session
    /// backend. A `Cell` so the read-only `backend_spec` can take it.
    pub(super) routed_backend: std::cell::Cell<Option<ai::BackendKind>>,
    /// The backend of the in-flight AI request (#372), for the status indicator:
    /// set when a request is built, cleared when it resolves.
    pub(super) ai_in_flight: std::cell::Cell<Option<ai::BackendKind>>,
    /// One-shot local model the router picked for the next request (#377), set
    /// beside `routed_backend` when routing on-device and consumed by
    /// `backend_spec`. `None` ⇒ the single hardware-recommended model.
    pub(super) routed_model: std::cell::Cell<Option<&'static ai::model_select::Model>>,
    /// One-shot *cloud* model the per-action router picked for the next request
    /// (#930): the Claude model this pass's weight + estimate resolved to, set in
    /// `route_request` and consumed by `spawn_ai` so the actual request runs on the
    /// chosen tier (a heavy pass on the top model, a light small one on a cheaper
    /// one). `None` ⇒ the session `[ai] model`, as before — so a pass that never
    /// went through the per-action picker (interactive, local) is unchanged.
    pub(super) routed_cloud_model: std::cell::Cell<Option<String>>,
    /// A one-line notice the worker sets when a routed-local request couldn't
    /// obtain its model (download/init failed) and fell back to Claude (#725), so
    /// the fallback is never silent. Shared with the worker thread; read and
    /// cleared by `poll_ai` when the outcome lands.
    pub(super) ai_routing_notice: std::sync::Arc<std::sync::Mutex<Option<String>>>,
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
    pub(super) annotations_context: String,
    /// Annotation snapshots paralleling the buffer's undo / redo stacks (#370):
    /// one per edit group, pushed when a group opens and restored alongside the
    /// rope, so undo/redo moves notes back to where the text moved (a rope swap
    /// carries no edit delta to re-anchor through).
    pub(super) anno_undo: std::collections::VecDeque<Vec<crate::annotation::Live>>,
    pub(super) anno_redo: std::collections::VecDeque<Vec<crate::annotation::Live>>,
    /// The single undoable cross-chapter scene move on the manuscript corkboard
    /// (#974). A cross-chapter move rewrites two chapter files at once, outside the
    /// buffer's own rope-level undo, so it captures both chapters' pre-move contents
    /// here; the board's `u` key restores them (and reopens the board on the moved
    /// scene). `None` when there's nothing to undo — one level, cleared once used or
    /// once another structural corkboard edit supersedes it.
    pub(super) cork_undo: Option<corkboard::CrossMoveUndo>,
    /// Transient Vim Normal-mode state (#202): pending count, multi-key command,
    /// and last find. Only meaningful while `vim` is on.
    pub(super) vim_state: VimState,
    /// Vim key remaps (#208): a pressed key → the default key whose action it
    /// should run, built from the `[editor.keys]` config. Empty (no remaps) by
    /// default, so the built-in bindings apply unchanged.
    pub(super) vim_keys: HashMap<char, char>,
    /// A pending case-change operator (`g~` / `gu` / `gU`, #1006) and its count,
    /// awaiting a motion or text object — the parallel to [`VimState::op`] for the
    /// case operators, which don't yank/delete and so aren't `Op`s. `None` when no
    /// case operator is in flight. Lives on `App` (like `vim_state`) so it survives
    /// the multi-key command; consumed by the motion that follows it.
    pub(crate) case_op: Option<(crate::buffer::CaseChange, usize)>,
    pub should_quit: bool,
    /// Distraction-free centering: target column width for the text, in cells.
    pub focus_width: u16,
    /// Heading/scene folding (#975): the collapsed sections, keyed by heading start
    /// byte. Purely display-only — the renderer hides a folded section's body lines
    /// and marks its heading, never touching the rope or the file. Empty by default.
    pub folds: FoldState,
    /// Distraction-free composition mode (#975): a runtime toggle that hides the
    /// chrome (outline pane, status bar, and any split) so only the centered prose
    /// column shows. Composes with focus mode and folding. Off by default; a
    /// per-session view state, never persisted.
    pub distraction_free: bool,
    /// Galley proofreading mode (#1003): `Some` while the read-only, paginated proofing
    /// view is on, carrying the current page's top line and the line-number toggle;
    /// `None` in the normal editor. A per-session view state, never persisted. Read-only
    /// and page-driven, so it never disturbs the edit buffer's cursor or scroll.
    pub galley: Option<GalleyState>,
    /// The galley page geometry the last frame drew at (wrap column, page height in rows)
    /// (#1003): recorded by the galley renderer so the Space/Backspace paging can advance
    /// by exactly one screenful of visual rows without re-deriving the layout. Zero until
    /// the first galley frame paints.
    pub(crate) galley_geometry: std::cell::Cell<(u16, u16)>,
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
    /// Whether this session already checked the one-time "grow into a novel"
    /// offer for a story past novella length (#1242), so the idle tick checks once.
    pub(crate) novella_checked: bool,
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
    pub(super) vc_status_dirty: bool,
    /// When the working-tree status was last recomputed (#779), for the interval
    /// fallback that picks up external edits even when nothing in-app marked the
    /// status dirty.
    pub(super) last_vc_refresh: std::time::Instant,
    /// The current interval before the fallback re-walks the tree (#869). Starts at
    /// the base and backs off (doubling to a cap) each idle walk that finds the
    /// count unchanged, so an untouched editor stops re-walking on a fixed short
    /// clock; any change (or a save marking the status dirty) resets it to the base.
    pub(super) vc_status_idle_interval: std::time::Duration,
    /// The off-thread working-tree status walk (#779): a `.gitignore`-respecting
    /// stat walk is the heaviest thing on the idle tick, so it runs on a worker and
    /// its resulting change count lands via [`Self::refresh_vc_status_on_idle`],
    /// never blocking the UI thread.
    pub(super) vc_status_task: Task<usize>,
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
    /// The outline pane's scroll offset: the first chapter-list row on screen. The
    /// renderer moves it just enough to keep the row that matters in view (the Nav
    /// selection, the focused outline's heading, or the active chapter and its
    /// current section) and writes it back, so the list scrolls a row at a time as
    /// the selection walks past an edge rather than jumping a page. A `Cell` so the
    /// immutable render path can store it.
    pub(crate) nav_scroll: std::cell::Cell<usize>,
    /// Words inserted / deleted today and the count of words moved out of the
    /// manuscript today (deleted chapters). Seeded from the day's snapshot on
    /// load; fed to the progress report and persisted to the history log.
    pub(super) added: usize,
    pub(super) removed: usize,
    pub(super) moved_to_unused: usize,
    /// Today's added / deleted words per part, in manifest part order (empty
    /// when the manifest defines no parts). The global counters above equal the
    /// sums of these plus any edits in chapters outside every part.
    pub(super) part_added: Vec<usize>,
    pub(super) part_removed: Vec<usize>,
    /// The `added` / `removed` totals at the start of the current writing session
    /// (#835). A session's output is the growth since this checkpoint; recording a
    /// session (at quit) appends the delta and advances the checkpoint, so the
    /// per-session log accumulates bursts the coalesced day snapshot can't show.
    pub(super) session_base_added: usize,
    pub(super) session_base_removed: usize,
    /// The calendar day (ISO `YYYY-MM-DD`) the daily counters above are currently
    /// booked to. Set at load and rolled over on the idle tick when the local date
    /// changes ([`roll_day_if_needed`](Self::roll_day_if_needed)), so a session left
    /// open across midnight starts the new day's tally fresh instead of carrying the
    /// previous day's `added`/`removed` into a record dated the new day.
    pub(super) tally_day: String,
    /// The last snapshot [`persist_snapshot`](Self::persist_snapshot) actually wrote
    /// to the history log, so an autosave whose totals are unchanged skips appending
    /// a duplicate day line (#869). A `RefCell` because the best-effort persist runs
    /// off `&self`; `None` until the first write this session.
    pub(super) last_history_snapshot: std::cell::RefCell<Option<history::Snapshot>>,
    /// The resume position [`persist_position`](Self::persist_position) last wrote
    /// (or [`resume_cursor`](Self::resume_cursor) read at launch), keyed by project
    /// root, so a save that leaves the cursor where it was skips rewriting the
    /// sidecar. A `RefCell` because the best-effort persist runs off `&self`.
    pub(super) last_position: RefCell<Option<(PathBuf, lastpos::Position)>>,
    /// The content revision of the focused chapter buffer whose TODO count
    /// [`cache_focus_words`](Self::cache_focus_words) last took, with that count —
    /// so [`count_todo`](Self::count_todo) reuses it rather than re-scanning the
    /// same text (the save path refreshes the cache and then rolls the count up).
    pub(super) focus_todo: Cell<Option<(u64, usize)>>,
    /// Per-chapter word baselines (chapter file → last counted words) the tally
    /// measures against. Keyed by file identity so it survives reordering; an
    /// edit's delta is attributed to the part of the chapter that changed.
    pub(super) baselines: HashMap<String, usize>,
    /// AI configuration (opt-in, off by default), loaded from the user's home
    /// config. Drives whether the "Ask the manuscript" command is reachable.
    pub ai: ai::Settings,
    /// Image-generation configuration (#238): the `[image]` section, opt-in and
    /// key-gated separately from the text `[ai]` settings.
    pub image: ai::image::ImageSettings,
    /// In-flight AI request (#152), run on a worker thread so it never blocks the
    /// editor. Polled each loop tick by [`App::poll_ai`].
    pub(super) ai_task: Task<Outcome>,
    /// In-flight remote push/pull (#134/#152), run on a worker thread. Polled each
    /// loop tick by [`App::poll_vc`].
    pub(super) vc_task: Task<VcMsg>,
    /// The active chapter's path when a pull was spawned (#870), so the post-pull
    /// reload targets the chapter that started the pull — not whatever the author
    /// navigated to during a network-slow pull. `Some` only while a pull is in
    /// flight; cleared when the result lands. A pull that lands after the author
    /// moved on (or typed unsaved edits) skips the destructive reload rather than
    /// overwriting a now-active chapter's buffer from the just-pulled disk state.
    pub(super) pull_origin: Option<PathBuf>,
    /// Per-chapter conflicts from the last two-way sync (#962) — chapters changed on
    /// both machines that sync detected but couldn't merge. Preserved (both versions
    /// intact) and surfaced here for the conflict-resolution UI (#963); empty after a
    /// clean sync. The last sync's result, not a running log.
    pub(super) sync_conflicts: Vec<crate::vcs::ChapterConflict>,
    /// A background sync landed with conflicts (#962) while the author was in a
    /// prompt/overlay/modal, so surfacing the resolution overlay was deferred rather
    /// than clobbering their in-flight state (bug F). `poll_vc` sets this when it
    /// can't open the flow immediately and re-checks each tick, opening it once the
    /// author is back in a plain editing mode. Cleared when the flow finally opens.
    pub(super) pending_sync_conflicts: bool,
    /// The draft merge (#965) a conflict-resolution flow is finishing, as
    /// `(into, from)` draft names. `Some` only while resolving a conflicting draft
    /// merge; it routes [`finalize_conflict_resolution`](App::finalize_conflict_resolution)
    /// to [`crate::vcs::finalize_branch_merge`] instead of the sync's `finalize_sync`,
    /// and is cleared when the merge lands (or the flow is abandoned). `None` for a
    /// sync-conflict resolution, which finalizes against the remote.
    pub(super) pending_draft_merge: Option<(String, String)>,
    /// Automatic off-site backup config (#887): the opt-in `[backup]` section,
    /// layered per project. Drives whether a save / the timed cadence transparently
    /// pushes to the remote. Off by default — no behavior change when disabled.
    pub(crate) backup: crate::config::BackupConfig,
    /// The in-flight automatic backup push (#887), on its own worker task so it
    /// never collides with the author's manual push/pull (`vc_task`). Polled each
    /// loop tick by [`App::poll_backup`].
    pub(super) backup_task: Task<backup::BackupOutcome>,
    /// Whether a save has landed since the last backup, so the on-save trigger
    /// owes a push (#887). Set on the post-save tail, cleared when a backup fires
    /// (or is gated off for want of a remote); read by the cadence hook.
    pub(super) backup_pending: bool,
    /// When the last automatic backup was *attempted* (#887) — the clock the timed
    /// interval measures against and the reschedule point after a failure. Advanced
    /// on every attempt, success or not, so a failed push waits out one interval
    /// before retrying rather than hammering an offline remote every tick.
    pub(super) last_backup_attempt: std::time::Instant,
    /// When the last backup *succeeded* (#887), so the author can see how current
    /// the off-site copy is. `None` until the first success this session; only a
    /// successful push stamps it, so a run of failures never claims the work is
    /// backed up.
    pub(super) last_backup: Option<std::time::Instant>,
    /// The in-flight scheduled *local* archive (#888), on its own worker task so a
    /// zip never blocks the UI and never collides with the remote push (`backup_task`).
    /// Polled each loop tick by [`App::poll_local_backup`].
    pub(super) local_backup_task: Task<backup::LocalBackupOutcome>,
    /// When the last local archive was *attempted* (#888) — the clock the local
    /// hourly interval measures against and the reschedule point after a failure.
    /// Advanced on every attempt so a failing archive waits out one interval before
    /// retrying rather than hammering the disk every tick.
    pub(super) last_local_backup_attempt: std::time::Instant,
    /// When the last local archive *succeeded* this session (#888), so the author
    /// can see how current the on-disk copy is. `None` until the first success;
    /// only a successful archive stamps it.
    pub(super) last_local_backup: Option<std::time::Instant>,
    /// A background local-model download (#382): `(model id, result)`, polled in
    /// the event loop so a multi-gigabyte fetch never blocks the editor.
    pub(super) download_task: Task<(String, Result<(), String>)>,
    /// Live progress of the in-flight model download (#813): the shared byte
    /// counter the worker's `hf-hub` fetch drives, plus when the download started
    /// (for the ETA). `Some` only while a download runs; the terminal poll clears
    /// it. Read each tick by [`App::poll_model_download`] to render progress in
    /// Quill's own status line rather than over the TUI (#727).
    pub(super) download_progress: Option<(Arc<models::DownloadProgress>, std::time::Instant)>,
    /// Model ids known to be on disk (#382), refreshed when the manager opens and
    /// after a download/remove — so the manager shows status without probing the
    /// `hf-hub` cache every frame.
    pub(super) downloaded_models: Vec<String>,
    /// The loaded local backend, cached after first use (loading a model is slow)
    /// and shared with the worker thread. Keyed by model id (#377) so several
    /// downloaded models can be cached and reused as the router picks between them;
    /// empty until the first local request. Type-erased so it needs no
    /// `local`-feature cfg here.
    pub(super) local_backend: Arc<Mutex<HashMap<String, Arc<dyn Backend>>>>,
    /// Cached local image model (#240), loaded once on first use and reused;
    /// empty for the remote image backend. Type-erased like `local_backend`.
    pub(super) image_backend: Arc<Mutex<Option<Arc<dyn ai::image::ImageBackend>>>>,
    /// The portrait targets `(slug, name, prompt)` the author picked (#244),
    /// awaiting the cost confirm; taken when they confirm, cleared on cancel.
    pub(super) pending_portraits: Option<Vec<(String, String, String)>>,
    /// Continuity findings waiting to be shown — stashed while a timeline
    /// conflict is resolved first, then opened by [`App::open_findings`].
    pub(super) pending_findings: Vec<Finding>,
    /// All open findings (continuity / setting / style / naming / gaps),
    /// de-duplicated — the "revision triage" list. Loaded from the project's
    /// durable store at startup and kept in sync with it, so dismissals stick and
    /// the list survives a restart (#591).
    pub(super) triage: Vec<Finding>,
    /// The durable triage store (#591): every finding the project has seen plus
    /// the author's dismiss decisions, persisted as `bible/findings.toml`. Mirrors
    /// dismiss / mark actions so a re-run doesn't re-surface a dismissed finding.
    pub(super) findings_store: findings_store::FindingsStore,
    /// The finding being re-checked (#608): its originating pass is re-running
    /// scoped to its chapters, and when the result lands [`App::apply_recheck`]
    /// clears it if it's gone or keeps it if still flagged. `Some` only while a
    /// recheck request is in flight; taken when its result applies.
    pub(super) pending_recheck: Option<Finding>,
    /// The findings a re-check-all is verifying (#830): every open finding whose
    /// originating pass was re-run (grouped by pass + chapters into one worker
    /// batch), so when the batch lands [`App::apply_recheck_all`] can reconcile
    /// each — clearing the ones the fresh pass no longer produces, keeping the
    /// rest. `Some` only while a re-check-all is in flight; taken when it applies.
    pub(super) pending_recheck_all: Option<Vec<Finding>>,
    /// Per-chapter content digests the open findings were last generated (or
    /// re-verified) against (#1053), keyed `(book, chapter file) → digest`,
    /// exactly the content-addressed hash the incremental bible cache uses (#90).
    /// "Verify on idle" (opt-in) compares each open finding's chapters' current
    /// digests against this baseline on the idle tick; a chapter whose digest
    /// changed marks its findings stale and triggers a scoped re-check of only
    /// those chapters. Recorded when a pass accumulates findings and refreshed
    /// each time a re-check reconciles them, so an unchanged chapter is never
    /// re-analyzed. The key carries the book index, not just the file, because a
    /// multi-book series can hold same-named chapter files at the project root
    /// (subdirs are optional) — keying on the file alone would let one book's
    /// baseline clobber another's and compare a finding against the wrong book.
    pub(super) verify_baselines: std::collections::BTreeMap<(usize, String), String>,
    /// Memoized chapter digests for "verify on idle" (#1053), keyed by `(book,
    /// file)` and stamped with where the text came from (the live buffer's revision,
    /// or the file's size and mtime), so an idle pass re-reads and re-digests only
    /// a chapter that actually changed.
    pub(super) verify_digests: RefCell<HashMap<(usize, String), assist::DigestMemo>>,
    /// The bible pass in flight and the chapter digests it was built from (#90),
    /// recorded to the incremental-scoping state file once the pass applies
    /// cleanly. `(tracker, digests, merge)` — `merge` is `Some(preserve)` for one
    /// book of a series: it folds this book's digests in while keeping the other
    /// books' entries (the `preserve` set) and pruning this book's stale ones,
    /// including deleted chapters (#339, #645); `None` replaces (standalone novel).
    /// The outer `Option` is `None` when no bible pass is pending.
    pub(super) pending_pass: Option<(
        String,
        ai::incremental::Digests,
        Option<std::collections::BTreeSet<String>>,
    )>,
    /// Author-defined workflows (#226) from the `[workflows]` config — named,
    /// ordered sequences of registry actions run as one step. Loaded once at
    /// startup, sorted by name for a stable run-workflow menu.
    pub(super) workflows: Vec<crate::workflow::Workflow>,
    /// Workflow triggers (#228) from the `[triggers]` config — on-save and bound
    /// Vim Normal-mode keys, validated against `workflows` at startup.
    pub(super) triggers: crate::workflow::Triggers,
    /// Author-defined text snippets (#411) from the `[snippets]` config, sorted by
    /// name. Inserted verbatim from the snippet picker; empty when none are set.
    pub(super) snippets: Vec<crate::config::Snippet>,
    /// The steps of an AI-proposed workflow (#232) awaiting the author's
    /// name-confirm prompt; taken when they save it, cleared when they cancel.
    pub(super) pending_workflow: Option<Vec<String>>,
    /// AI-proposed config changes (#303) — validated `(dotted_key, value)` pairs
    /// awaiting the author's y/N confirm; taken on apply, cleared on cancel.
    pub(super) pending_config: Option<Vec<(String, String)>>,
    /// The workflow currently executing (#227), if any: its resolved steps and
    /// where it is. `Some` while a run is in progress — including while it's
    /// suspended waiting on an async AI step. Drives the step state machine.
    pub(super) workflow_run: Option<workflows::WorkflowRun>,
    /// A "Refresh series bible" run in progress (#340), if any: the queue of
    /// per-book bible passes and where it is. `Some` while the book-by-book
    /// refresh is running (including suspended on an in-flight pass).
    pub(super) series_bible_run: Option<series_bible::SeriesBibleRun>,
    /// A durable multi-pass queue run in progress (#923), if any: the ordered
    /// passes (persisted to `bible/.queue.toml`), the cursor, and the shared
    /// sequenced-run bookkeeping. `Some` while the queue runs (including suspended
    /// on an in-flight pass); a crash mid-run leaves the file to resume from.
    pub(super) pass_queue_run: Option<pass_queue::PassQueueRun>,
    /// Per-pass sub-progress (#923): the `(done, total)` chapter counter the
    /// in-flight fold pass updates on the worker thread as it walks chapters, so
    /// the run's status can read "continuity: chapter 5/12". `None` between the
    /// chapter-granular fold steps and for one-shot passes (which show pass-level
    /// progress only). Shared so the worker can write while the UI thread reads.
    pub(super) pass_progress: Arc<Mutex<Option<(usize, usize)>>>,
    /// The opt-in background batch bible update's in-flight worker (#950): submits
    /// the full-manuscript rebuild batches, or polls the persisted ones to reconcile
    /// them. Its own task, separate from [`ai_task`](App::ai_task), so a background
    /// submit/reconcile never blocks — nor is blocked by — an interactive AI pass.
    /// Polled each loop tick by [`App::poll_batch_update`].
    pub(super) batch_task: Task<Vec<batch_update::BatchTaskResult>>,
    /// The durable batch record (`bible/.batch.toml`) as last read, so the idle
    /// reconcile doesn't re-read and re-parse it every pass. Dropped whenever this
    /// process writes the record, and re-read after a coarse interval in case
    /// something else did.
    pub(super) batch_record: Option<batch_update::BatchRecordCache>,
    /// The book a reconciled batch pass's bible write is being applied for (#950),
    /// so its findings are attributed to that book — the batch counterpart of the
    /// series-refresh / pass-queue `current_pass_book`. Set only while a reconciled
    /// batch is being applied off the normal run path; `None` otherwise.
    pub(super) batch_apply_book: std::cell::Cell<Option<usize>>,
    /// Crash recovery (#263): unsaved work found at launch, awaiting the author's
    /// restore/discard choice. `Some` only while the recovery prompt is open;
    /// taken when they restore, cleared when they discard or defer.
    pub(super) pending_recovery: Option<recovery::Recovery>,
    /// Whether a recovery sidecar is on disk — or on its way there through the
    /// writer — for this session's edits, so the idle cadence clears it exactly
    /// once when the buffer goes clean rather than issuing a remove on every
    /// idle tick.
    pub(super) recovery_written: bool,
    /// What the last recovery capture handed the writer: the edit buffer's
    /// content [`revision`](crate::buffer::Buffer::revision) plus the chapter
    /// files it attributed text to. An idle pass whose key matches submits
    /// nothing — the sidecar holds (or is about to hold) exactly this. `None`
    /// when nothing has been captured, the sidecar was cleared, or the write
    /// failed (so the next tick retries it).
    pub(super) recovery_key: Option<super::persistence::RecoveryKey>,
    /// When the last capture was submitted, for the capture throttle
    /// ([`RECOVERY_MIN_INTERVAL`](super::persistence::RECOVERY_MIN_INTERVAL)).
    pub(super) recovery_captured_at: Option<std::time::Instant>,
    /// The background thread that writes and clears the sidecar, so a durable
    /// write on a busy disk never blocks typing.
    pub(super) recovery_writer: recovery::writer::Writer,
    /// The background thread that writes the project's files on save — the
    /// chapters, the annotations side file, the history log, the resume point —
    /// so a save (autosave above all) never blocks typing on a slow disk.
    pub(super) file_writer: super::saving::FileWriter,
    /// Saves handed to [`file_writer`](Self::file_writer) whose outcome hasn't
    /// been applied yet, oldest first.
    pub(super) pending_saves: Vec<super::saving::PendingSave>,
    /// Test seam: settle every file write as soon as it's queued (the default in
    /// tests, so a test can read the disk right after a save); the tests of the
    /// asynchronous behavior turn it off.
    #[cfg(test)]
    pub(super) settle_saves_eagerly: bool,
    /// The project the author chose in the browser, held while the unsaved-
    /// changes confirm is open (#270); taken when they confirm the switch.
    pub(super) pending_switch: Option<PathBuf>,
    /// The project a browser rename / delete prompt targets (#612), held while
    /// the folder-name input or the delete confirm is open; taken when the action
    /// runs. Distinct from `pending_switch` (the switch target) so a manage
    /// action never disturbs a pending switch.
    pub(super) pending_manage: Option<PathBuf>,
    /// The revision a whole-project restore confirm (#833) targets — a milestone or
    /// commit — held while the y/N prompt is open; taken when the restore runs.
    /// Kept off the `Copy` `Pending` enum so the revision string lives here.
    pub(super) pending_restore_rev: Option<String>,
    /// The name of the milestone a note prompt (#966) targets — the just-named
    /// milestone awaiting its optional note, or the existing milestone whose note
    /// is being edited. Held while the note input is open; taken when the
    /// milestone is created or re-annotated. Kept off the `Copy` `Pending` enum so
    /// the name string lives here.
    pub(super) pending_milestone_name: Option<String>,
    /// A setup-wizard setting concept (#282) deferred until the characters draft
    /// finishes — only one AI pass runs at a time, so the second is queued here
    /// and fired by `poll_ai` when the first completes.
    pub(super) wizard_pending_setting: Option<String>,
    /// A label for the notes import in flight (#283) — a file name or a count —
    /// shown in the summary when the pass applies; taken by `apply_import`.
    pub(super) import_source: Option<String>,
    /// The source chosen for a two-step import wizard (#508): a folder of notes
    /// to seed a new project from (#286) or a manuscript to split into chapters
    /// (#298), held between the source prompt and the new-project-path prompt;
    /// taken when the project is created and the import runs.
    pub(super) pending_import_source: Option<PendingImportSource>,
    /// The `.zip` archive chosen to restore a project from (#594), held between
    /// the source prompt and the new-folder-path prompt; taken when the folder is
    /// entered and the archive is unpacked.
    pub(super) pending_archive_source: Option<PathBuf>,
    /// The local backup archive a restore confirm (#934) targets, held while the
    /// y/N prompt is open (from the backup browser, or the on-open damage offer);
    /// taken when the restore runs. Kept off the `Copy` `Pending` enum so the path
    /// lives here.
    pub(super) pending_restore_backup: Option<PathBuf>,
    /// An active writing sprint (#299): a timed session with a live word count.
    /// Session state, never persisted; `None` when no sprint is running.
    pub(crate) sprint: Option<sprint::Sprint>,
    /// An optional per-session word target (#1011): a goal to write toward this
    /// session, with a live indicator that fills as words are added. Session
    /// state, never persisted; `None` until the author sets one. Independent of a
    /// timed sprint — the author can run either, both, or neither.
    pub(crate) session_target: Option<sprint::SessionTarget>,
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
    pub(super) qa_store: qa_store::QaStore,
    /// The question of the "Ask the manuscript" request in flight (#609), captured
    /// when it's sent so the answer can be filed under it in [`qa_log`](Self::qa_log);
    /// taken when the answer applies, `None` when no ask is pending.
    pub(crate) pending_qa_question: Option<String>,
    /// The argument the author submitted for a cost-gated, argument-taking AI pass
    /// — the accuracy concern (#973, "1850s London") or the relationship pair
    /// (#1046, "Alice / Bob") — with the action it belongs to. Captured when the
    /// author submits the argument prompt so the cost-gated run (and any confirm
    /// chained after it) reaches the pass with it intact; taken when that action
    /// runs, `None` when no such pass is pending.
    pub(crate) pending_ai_arg: Option<(AiAction, String)>,
    /// The chapter files an on-demand continuity/setting pass is scoped to (#1054)
    /// — captured from the editor selection (or the cursor's chapter) when the
    /// author opens the AI palette from Visual mode, so the pass re-verifies just
    /// the selected scene(s) instead of the whole book. `None` means no scope was
    /// given, so the pass runs whole-book exactly as before. Taken when a scopeable
    /// pass spawns; cleared when the palette opens without a selection.
    pub(crate) pass_scope: Option<Vec<String>>,
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

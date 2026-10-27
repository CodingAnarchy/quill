//! The unified run-action registry (#225): the [`Action`] enum with its
//! `ALL`/`name`/`group`/`label` accessors and resolvers (all read from the one
//! row-per-action table in `table.rs`), plus the export targets ([`ExportFormat`],
//! [`ExportScope`]) and the version-control [`GitAction`] set. Re-exported from
//! the module root, so callers are unaffected.

use super::table::{rows, ActionSpec, ALL_ACTIONS};
use super::{AiAction, Needs, SettingBool};

/// A single invokable Quill action — the unified registry (#225). The run-action
/// command lists these and runs one by name, and author workflows (#89) will
/// enumerate and invoke them by [`name`](Action::name). Each wraps the dispatch
/// the existing shortcuts already use (save, export, the reports, the AI passes).
///
/// Git is intentionally absent: the version-control palette shows a different set
/// of actions depending on repo/remote state, and several entries are interactive
/// prompts or spawn async work — neither fits the flat, fire-and-forget registry
/// yet. It can join once the registry models context-dependent availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Save the active chapter / continuous window (Ctrl-S).
    Save,
    /// Compile and write an export (Ctrl-E).
    Export(ExportFormat),
    /// Open the progress report (Ctrl-R).
    ProgressReport,
    /// Open the local analysis report (Ctrl-T).
    AnalysisReport,
    /// Open the writing-history view — daily net-words graph, streaks, and
    /// totals from the recorded per-day log (#586).
    WritingHistory,
    /// Run an AI pass / open an AI view (Ctrl-A).
    Ai(AiAction),
    /// Turn on the opt-in AI features in-app (#602): write `[ai] enabled = true`
    /// after a y/N confirm, then chain into the Set-key flow. AI stays off until
    /// the author explicitly confirms.
    EnableAi,
    /// Open the built-in user's guide (F1) (#281).
    UserGuide,
    /// Reopen the first-run quick-start card (#837): the dismissible tour of the
    /// palette, core keys, and the AI on-ramp. Shown once automatically on first
    /// launch; this makes it reachable again from help at any time.
    QuickStart,
    /// Import a Markdown notes file (or a folder of them) into the story bible
    /// (#283).
    ImportNotes,
    /// Create a new project from a folder of notes (#286).
    NewProjectFromNotes,
    /// Import an existing Markdown manuscript as a new project (#298).
    ImportManuscript,
    /// Write a timestamped `.zip` backup of the whole project — an offline,
    /// no-remote archive of the manuscript, bible, and side files (#594).
    ArchiveProject,
    /// Restore a project from a `.zip` archive into a new folder (#594).
    RestoreArchive,
    /// Browse the project's scheduled local backups (#934): the timestamped `.zip`
    /// archives #888 wrote, newest first with sizes and dates, in an overlay from
    /// which the author restores one over the open project (behind a confirm).
    BrowseBackups,
    /// Start a timed writing sprint (#299).
    StartSprint,
    /// Stop the active writing sprint (#299).
    StopSprint,
    /// Set (or clear) a per-session word target (#1011): a goal to write toward
    /// this session, with a live indicator that fills as words are added.
    SetSessionTarget,
    /// Switch the active book of a multi-book series (#329).
    SwitchBook,
    /// Create a series / add / remove / reorder books (#648) — the book-management
    /// palette. For a standalone novel it starts a series (convert + add a book).
    ManageBooks,
    /// Open the series tree navigator (#330).
    SeriesTree,
    /// Rename the open project's folder, matching it to the book title (#424).
    RenameProject,
    /// Turn a one-chapter novel into a short story (#1243), after a y/N confirm
    /// and a restore point.
    ConvertToShortStory,
    /// Set the book's title, subtitle, author, and series — the manifest fields
    /// also reachable from the chapter browser's single-letter keys (#571).
    BookDetails,
    /// Set or change the project's writing goals — target words, target chapters,
    /// and the daily word target (#550).
    SetGoals,
    /// Refresh the whole series' story bible, book by book (#340).
    RefreshSeriesBible,
    /// Refresh all four story bibles (characters, continuity, setting, plot) in one
    /// batch — one combined cost estimate/confirm and combined progress, over every
    /// book of a series (#655).
    RefreshBibles,
    /// Queue the whole bible suite as a durable, resumable run (#923): the five
    /// trackers over every book, one aggregate cost confirm, per-pass progress, and
    /// a crash-safe queue (`bible/.queue.toml`) that resumes rather than losing the
    /// passes already done. `force` rebuilds each bible from the whole manuscript.
    QueuePasses,
    /// Queue a background bible update (#950): opt-in, explicit-action trigger for a
    /// full-manuscript rebuild of every bible through the asynchronous Message
    /// Batches API (half price, a window of up to ~24h). Submits and returns
    /// immediately, persisting the batch ids to `bible/.batch.toml`; the results are
    /// reconciled on a later launch / idle tick, so it survives app restarts and
    /// never blocks writing. Gated on `[ai] background-batch-update` + a cloud
    /// backend; distinct from the synchronous interactive refreshes (#655/#955).
    BackgroundBibleUpdate,
    /// Find & replace across every chapter in the project (#295).
    ProjectFind,
    /// Open the drafts overlay (#965): create / switch / list / merge draft branches
    /// — alternate versions (alt ending, POV split) tracked as first-class drafts.
    /// The one version-control entry the flat run-action registry can host: it just
    /// opens the overlay (no repo/remote-state-dependent availability, no async
    /// spawn), unlike the rest of the context-sensitive Git palette.
    Drafts,
    /// Describe a configuration change in words; the AI proposes settings (#303).
    DescribeConfig,
    /// Accept the flagged word at the cursor into the project dictionary (#358).
    AcceptWord,
    /// Open the project's custom dictionary in a pane to view and edit it (#935).
    ViewDictionary,
    /// Prompt for a line number and move the cursor there (#1010) — a direct
    /// go-to-line outside Vim's `:N`.
    GoToLine,
    /// Step the cursor back to an older spot in the recent-edit ring (#1010) — the
    /// automatic breadcrumb of where the author has been editing.
    RecentEditBack,
    /// Step the cursor forward toward a newer spot in the recent-edit ring (#1010),
    /// the inverse of [`RecentEditBack`](Self::RecentEditBack).
    RecentEditForward,
    /// Jump the cursor to the last insertion point (#1010) — the `gi`-style return
    /// to where the author last added prose.
    LastInsertion,
    /// Jump the cursor to the next misspelled word (#359).
    NextMisspelling,
    /// Jump the cursor to the previous misspelled word (#359).
    PrevMisspelling,
    /// Open the style-flag sweep picker (#1001): pick a flag category (adverbs,
    /// filter words, weak / passive verbs, long sentences, over-used words) to
    /// underline and step through with `]r` / `[r`.
    StyleFlagSweep,
    /// Toggle the near-proximity word-repeat marking (#1002): dim-mark content words
    /// echoed within a short window of the visible text — a deliberate revision-pass
    /// aid, off by default.
    ToggleProximityRepeats,
    /// Flip a boolean `[editor]` knob for this session (typewriter scrolling,
    /// focus mode, paragraph indent, the annotation margin, the daily-goal cue, …)
    /// — one variant per knob with a palette toggle in its
    /// [`SettingBool::SPECS`] row, which supplies the name, label, and runnability.
    Toggle(SettingBool),
    /// Fold every section in the active chapter to its heading (#975).
    FoldAll,
    /// Expand every folded section (#975).
    ExpandAll,
    /// Toggle distraction-free composition mode — hide the chrome (#975).
    ToggleComposition,
    /// Toggle the read-only galley proofreading mode — a paginated, page-at-a-time
    /// read-through of the chapter (#1003).
    ToggleGalley,
    /// Open the read-only corkboard for the active chapter (#589, Phase 1): an
    /// index-card overview built from the chapter's `##`/`###` scene headings —
    /// each card is a heading + its first line of prose — navigable, Enter jumps
    /// the editor cursor to that scene. Phase 2 (per-scene metadata + reorder,
    /// possibly cross-chapter) is a documented follow-up on #589.
    Corkboard,
    /// Open the manuscript-wide corkboard (#701): scene cards across *every*
    /// chapter, grouped by chapter, navigable, with jump-to-scene. Unlike the
    /// per-chapter [`Action::Corkboard`] it is reachable from the continuous view
    /// (it leaves it first). Within-chapter reorder / synopsis / status work per
    /// card; cross-chapter reorder is a documented follow-up.
    ManuscriptCorkboard,
    /// Split the open chapter at the cursor into two (#585) — the same as the
    /// chapter browser's `x`. The live cursor is the seam, so it operates on the
    /// active chapter and prompts for the new chapter's title.
    SplitChapter,
    /// Merge the active chapter into the one above it (#585) — the same as the
    /// chapter browser's `M`, after a y/N confirm. No-op for the first chapter,
    /// which has nothing above it to merge into.
    MergeChapter,
    /// Annotate the selection (or word at the cursor) with a note (#302).
    AddAnnotation,
    /// Open the annotation review list for the active chapter (#302).
    ReviewAnnotations,
    /// Remove the annotation(s) anchored under the cursor, or overlapping the
    /// Visual selection — the editor-side counterpart to the review list's `d`.
    RemoveAnnotation,
    /// Remove every annotation on the loaded chapter(s), after a y/N confirm.
    ClearAnnotations,
    /// Switch the active AI backend between Claude and the local model (#371).
    SwitchBackend,
    /// Open the local AI model manager — choose which models to use (#379).
    ManageModels,
    /// Store the Claude API key in the OS keychain (#530).
    SetClaudeKey,
    /// Store the image (fal.ai) API key in the OS keychain (#530).
    SetImageKey,
    /// Open the authorial assistant pane (#399), the same as `Ctrl-W a`.
    Assistant,
    /// Browse the story bible (#590): list the files under `bible/` — characters,
    /// timeline, setting, outline — and open a selected one in an editable buffer
    /// that saves back to disk. Navigation/editing of the AI's tracking artifacts,
    /// within the AI boundary (the AI authors nothing).
    BrowseBible,
    /// Open the author's research / worldbuilding notebook (#932): `bible/research.md`,
    /// an author-maintained journal of historical refs, magic-system sources, and
    /// worldbuilding rules that the AI's trackers can *cite* but never write. Opens
    /// it in an editable pane, seeding an empty template on first use. Squarely
    /// within the AI boundary — the author authors every note; Quill only cites it.
    ViewResearch,
    /// Open the snippet picker to insert an author-defined snippet (#411).
    InsertSnippet,
    /// Browse the curated open-licensed font library, download a font into the
    /// project, and set it for the designed exports' `[style]` (#494).
    BrowseFonts,
    /// Open the export-style setter — pick a preset and the common `[style]` knobs
    /// (scene break, drop caps, heading alignment, ornaments) and the `[print]`
    /// trim size, persisting each to `book.toml` (#569).
    SetExportStyle,
    /// Open the plain Settings menu — set the everyday `[editor]`/`[theme]` knobs
    /// (theme, column width, smart typography, Vim mode, autosave, typewriter /
    /// focus) and the `[analysis]` stopwords, no AI required (#592).
    Settings,
    /// Export a chapter-ordered, confidence-ranked revision plan (#1055):
    /// `export/revision-plan-<date>.md`, an author-workable to-do built from the
    /// open findings — chapters in reading order, highest cross-pass confidence
    /// (#921) first, dismissed/resolved findings already dropped. Reformats findings
    /// already produced (an off-manuscript artifact like the synopsis); authors no
    /// prose, makes no request. Empty findings yield a clean "nothing to plan".
    RevisionPlan,
    /// Export the manuscript-health trend snapshot (#1052):
    /// `export/health-<date>.md`, the findings grouped by kind/chapter with counts,
    /// headed by the trend since the prior recorded run — which categories fell
    /// (revision working) and which rose. Records this run into the small
    /// `bible/health-trend.toml` side-file and renders from those tallies alone, so
    /// it's pure local aggregation — no request, no manuscript re-scan.
    HealthTrend,
    /// Export the story bible as one clean, shareable Markdown document (#708):
    /// the scattered `bible/` files assembled in order (characters, relationships,
    /// setting, timeline, plot, outline) with the generated/notes markers and YAML
    /// front matter stripped, written to `export/story-bible.md`. Pure local
    /// concatenation of files already on disk — no AI request.
    ExportBible,
    /// Run the export preflight / publish health check (#815): validate the book
    /// against store requirements (ISBN, cover + DPI, copyright page, BCP-47
    /// language, bleed vs. full-bleed cover, empty chapters, emphasis) into one
    /// auditable checklist, shown and saved to `export/preflight-<date>.md`.
    /// Aggregates the export path's own signals; changes no export behavior.
    PreflightExport,
    /// One-click "Prepare for KDP" (#816): set the KDP-sane print defaults (6×9
    /// trim, 0.125" bleed, a default language when none is set), run the preflight
    /// (#815), and — unless it fails — emit the print-interior PDF and the
    /// reflowable EPUB together to `export/`, the KDP publish bundle in one step.
    PrepareForKdp,
    /// One-click "Prepare for IngramSpark" (#816): the same publish bundle as
    /// [`PrepareForKdp`](Self::PrepareForKdp) tuned for IngramSpark — sane print
    /// defaults, the preflight, and the print PDF + reflowable EPUB to `export/`.
    PrepareForIngramSpark,
    /// Export a review copy (#834): a `.docx` with numbered lines and generous
    /// margins for markup, carrying the author's inline annotations (#302) — which
    /// every other export strips — as Word comments anchored to their ranges. The
    /// collaboration/handoff artifact for a beta reader or editor. Pure export.
    ReviewCopy,
    /// Import review comments (#886): the return leg of the review copy (#834) —
    /// read a marked-up `.docx` and turn the reviewer's Word comments into live
    /// annotations anchored to the phrases they marked in the active chapter.
    /// Unmatched comments (the prose drifted) are reported, not silently lost.
    ImportReviewComments,
    /// Export the print **wrap cover** (#819): a single full-bleed PDF at
    /// `2×trim + spine + 2×bleed` with the front-cover art on the front panel, a
    /// reserved barcode/ISBN zone on the back, and a spine sized from the interior
    /// page count × the paper caliper. The other half of "print-ready" beside the
    /// print interior (#464). Standalone only. Pure export.
    PdfWrapCover,
}

impl Action {
    /// Every action, grouped in menu order — the registry, flattened from
    /// the `GROUPS` table. Each [`group`](Action::group) is contiguous, so the run-action
    /// menu emits one header per group (#514). Names are stable regardless of this
    /// order.
    pub(crate) const ALL: &'static [Self] = &ALL_ACTIONS;

    /// This action's registry row and group.
    fn entry(self) -> (&'static str, &'static ActionSpec) {
        #[allow(
            clippy::expect_used,
            reason = "GROUPS lists every Action variant (the completeness test \
                      enumerates the enum exhaustively), so the lookup always resolves"
        )]
        rows()
            .find(|(_, spec)| spec.action == self)
            .expect("every Action has a row in the registry")
    }

    /// The stable name the run-action command matches and workflows (#89)
    /// reference in config — never change these without a migration. The AI
    /// slugs come from `AiAction::SPECS` (#563).
    pub(crate) fn name(self) -> &'static str {
        self.entry().1.name
    }

    /// The collapsible group this action sits under in the run-action menu (#514).
    pub(crate) fn group(self) -> &'static str {
        self.entry().0
    }

    /// Human label for the run-action menu. The AI labels come from
    /// `AiAction::SPECS` (#563).
    pub(crate) fn label(self) -> &'static str {
        self.entry().1.label
    }

    /// The dedicated key binding that runs this action outside the palette, for the
    /// actions that have one — so the run-action menu (Ctrl-K) doubles as a live
    /// cheat sheet (#1103). `None` for the palette-only actions, which then show no
    /// shortcut (no clutter).
    ///
    /// This is the single source of truth for those bindings: [`on_key_ctrl`] and
    /// the F1 handler dispatch *from* this table (see `Action::from_ctrl_key` /
    /// `Action::from_key_event`), so a shown shortcut is exactly the key that runs
    /// the action — it can't drift. Anything without a binding here also has no
    /// dedicated key in dispatch.
    ///
    /// [`on_key_ctrl`]: crate::app::App::on_key_ctrl
    pub(crate) fn shortcut(self) -> Option<&'static str> {
        self.entry().1.shortcut
    }

    /// The `Ctrl-`_letter_ action a top-level editing keypress runs, or `None` for a
    /// `Ctrl-` combo that isn't a registry action (outline, quit, undo, …). The one
    /// place `Ctrl-` shortcuts resolve to an [`Action`]: [`on_key_ctrl`] dispatches
    /// through this so the binding lives once, and [`shortcut`](Action::shortcut) —
    /// which the palette shows — is verified against it (see the parity test), so the
    /// displayed key can't drift from what actually runs.
    ///
    /// `Ctrl-A` resolves to [`Ai`](Self::Ai) (the palette) here; whether it opens the
    /// palette or the enable-AI on-ramp is a readiness decision the caller keeps.
    ///
    /// [`on_key_ctrl`]: crate::app::App::on_key_ctrl
    pub(crate) const fn from_ctrl_key(c: char) -> Option<Self> {
        match c {
            's' => Some(Self::Save),
            'e' => Some(Self::Export(ExportFormat::Markdown)),
            'r' => Some(Self::ProgressReport),
            't' => Some(Self::AnalysisReport),
            'a' => Some(Self::Ai(AiAction::Ask)),
            '\\' => Some(Self::ToggleComposition),
            _ => None,
        }
    }

    /// Look up an action by its stable [`name`](Action::name) — the resolver a
    /// workflow (#226) uses to turn a config step token into an action.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        rows()
            .find(|(_, spec)| spec.name == name)
            .map(|(_, spec)| spec.action)
    }

    /// Whether this action completes synchronously, so a workflow (#226) can run
    /// it and move straight to the next step. The network AI passes are async
    /// (workflow support is #227) and "Ask" waits for typed input; `triage` is
    /// local aggregation, so it counts as synchronous. For AI actions this derives
    /// from the descriptor's [`Needs`]: only the [`Needs::Local`] views are sync.
    pub(crate) fn is_sync(self) -> bool {
        match self {
            Self::Ai(a) => a.needs() == Needs::Local,
            _ => true,
        }
    }

    /// Whether this action needs the `[ai]` backend — the set the enable-AI on-ramp
    /// (#602) intercepts while AI is off. The AI passes say so in their descriptor
    /// ([`Needs::Ai`]; local views and the separately opted-in image passes don't);
    /// the non-palette actions that spawn AI passes or ask the AI for settings are
    /// listed here. `EnableAi` itself is excluded so its own dispatch runs.
    pub(crate) fn needs_ai(self) -> bool {
        match self {
            Self::Ai(a) => a.needs() == Needs::Ai,
            Self::RefreshSeriesBible
            | Self::RefreshBibles
            | Self::QueuePasses
            | Self::DescribeConfig => true,
            _ => false,
        }
    }

    /// Whether the authorial assistant (#307/#392) may run this action as a tool:
    /// the mechanical actions that finish during the call, the analysis passes that
    /// run as a background request and surface findings in the editor (the assistant
    /// parks until each lands, #392), and the text-argument actions it can now
    /// supply an argument to (#390) — Ask, Brainstorm, and the bible drafts.
    /// Excluded: the thesaurus and image passes (they need cursor context the
    /// assistant has no way to give), local-only Triage, the modal pickers (book
    /// switch, tree, project find, annotate, model manager, the notes importers),
    /// and the off-manuscript copy generators, kept out of autonomous reach. A
    /// registry-row flag; the AI passes' value comes from `AiAction::SPECS` (#1069).
    pub(crate) fn assistant_runnable(self) -> bool {
        self.entry().1.assistant_runnable
    }

    /// The JSON property name of this action's single free-text argument (#390),
    /// for the actions that take one — the question for Ask, the premise for
    /// Brainstorm, the concept for the bible drafts. `None` for argument-free
    /// actions. The single source the tool schema and the dispatch both read, so a
    /// tool input never drifts from what the action consumes.
    pub(crate) fn text_arg(self) -> Option<&'static str> {
        // The AI actions' free-text argument is a descriptor field (#563): the
        // question for Ask, the premise for Brainstorm, the concept for the drafts,
        // the concern for Accuracy (#973), the pair for Relationship (#1046) — all in
        // `AiAction::SPECS`, so a tool input can't drift from what the action consumes.
        match self {
            Self::Ai(a) => a.text_arg(),
            _ => None,
        }
    }
}

/// A manuscript export target (#7), chosen from the export palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// A single compiled Markdown document (#190).
    Markdown,
    /// Word `.docx` in standard manuscript format (#191).
    Docx,
    /// A Scrivener `.scriv` project bundle (#192).
    Scriv,
    /// A Word `.docx` tuned for Vellum import — chapter titles as Heading 1 (#193).
    Vellum,
    /// An `.epub` ebook — the standard ebook distribution format (#297).
    Epub,
    /// A `.pdf` — the universal fixed-layout, ready-to-read/print artifact (#463).
    Pdf,
    /// A print-ready interior `.pdf` for print-on-demand (#464): a book trim size,
    /// mirrored margins + gutter, running heads, recto starts, embedded fonts.
    PdfPrint,
}

/// What an export covers: the whole active book (#332), the whole multi-book
/// series (#332), or a *sample* of the active book (#584) — a single chapter, a
/// part, or a contiguous chapter range. The sample scopes carry manifest chapter
/// indices and feed a filtered chapter set to the same writers; only the chapters
/// compiled (and the output filename) differ. A standalone novel only ever
/// exports its one book.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportScope {
    ActiveBook,
    Series,
    /// A single body chapter of the active book, by manifest chapter index (#584).
    Chapter(usize),
    /// A part of the active book (#584): its index into the manifest's parts and
    /// its display name (for the sample filename), plus the inclusive manifest
    /// chapter-index range the part spans.
    Part {
        name: String,
        start: usize,
        end: usize,
    },
    /// A contiguous range of body chapters of the active book (#584), by inclusive
    /// manifest chapter index.
    Range {
        start: usize,
        end: usize,
    },
}

impl ExportScope {
    /// The inclusive manifest chapter-index range a *sample* scope (#584) covers,
    /// or `None` for a whole-book / whole-series export. The single place the three
    /// sample scopes resolve to a chapter span — the export loop filters by it.
    pub(crate) const fn chapter_span(&self) -> Option<std::ops::RangeInclusive<usize>> {
        match self {
            Self::ActiveBook | Self::Series => None,
            Self::Chapter(i) => Some(*i..=*i),
            Self::Part { start, end, .. } | Self::Range { start, end } => Some(*start..=*end),
        }
    }
}

/// The version-control actions reachable from the Git palette (#54).
#[derive(Clone, Copy)]
pub enum GitAction {
    /// Initialize a repository (#130).
    Initialize,
    /// Commit the project with a message (#131).
    Commit,
    /// Show the commit history (#133).
    History,
    /// Show the working-tree diff since HEAD (#135).
    Diff,
    /// Restore the current chapter to its last committed version (#135).
    RestoreChapter,
    /// Create a named milestone — an annotated tag over the whole project (#833).
    Milestone,
    /// Restore the whole project to its last committed version (#833).
    RestoreProject,
    /// Diff the whole working tree against the last commit (#833).
    DiffProject,
    /// Set (or change) the remote URL the project backs up to (#134).
    SetRemote,
    /// Push local commits to the remote (#134).
    Push,
    /// Pull the remote's changes down (#134).
    Pull,
    /// Two-way sync: fetch and reconcile the remote's chapter edits with local work,
    /// detecting per-chapter conflicts (#962).
    Sync,
    /// Open the drafts overlay (#965): create / switch / list / merge draft branches —
    /// alternate versions (alt ending, POV split) tracked as first-class drafts.
    Drafts,
}

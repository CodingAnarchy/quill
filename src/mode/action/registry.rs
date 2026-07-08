//! The unified run-action registry (#225): the [`Action`] enum with its
//! `ALL`/`name`/`group`/`label` tables and resolvers, plus the export targets
//! ([`ExportFormat`], [`ExportScope`]) and the version-control [`GitAction`] set.
//! Split out of `action.rs` to keep each file under the ~1,000-line convention;
//! re-exported from the module root, so callers are unaffected. Pure move.

use super::AiAction;

/// A single invokable Quill action — the unified registry (#225). The run-action
/// command lists these and runs one by name, and author workflows (#89) will
/// enumerate and invoke them by [`name`](Action::name). Each wraps the dispatch
/// the existing shortcuts already use (save, export, the reports, the AI passes).
///
/// Git is intentionally absent: the version-control palette shows a different set
/// of actions depending on repo/remote state, and several entries are interactive
/// prompts or spawn async work — neither fits the flat, fire-and-forget registry
/// yet. It can join once the registry models context-dependent availability.
#[derive(Clone, Copy, Debug)]
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
    /// Start a timed writing sprint (#299).
    StartSprint,
    /// Stop the active writing sprint (#299).
    StopSprint,
    /// Switch the active book of a multi-book series (#329).
    SwitchBook,
    /// Create a series / add / remove / reorder books (#648) — the book-management
    /// palette. For a standalone novel it starts a series (convert + add a book).
    ManageBooks,
    /// Open the series tree navigator (#330).
    SeriesTree,
    /// Rename the open project's folder, matching it to the book title (#424).
    RenameProject,
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
    /// Find & replace across every chapter in the project (#295).
    ProjectFind,
    /// Describe a configuration change in words; the AI proposes settings (#303).
    DescribeConfig,
    /// Accept the flagged word at the cursor into the project dictionary (#358).
    AcceptWord,
    /// Jump the cursor to the next misspelled word (#359).
    NextMisspelling,
    /// Jump the cursor to the previous misspelled word (#359).
    PrevMisspelling,
    /// Toggle typewriter scrolling — pin the cursor line to the page centre (#301).
    ToggleTypewriter,
    /// Toggle focus mode — dim every paragraph but the current one (#301).
    ToggleFocus,
    /// Toggle the first-line paragraph indent in the editor (#722).
    ToggleParagraphIndent,
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
    /// Annotate the selection (or word at the cursor) with a note (#302).
    AddAnnotation,
    /// Open the annotation review list for the active chapter (#302).
    ReviewAnnotations,
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
    /// Every action, grouped in menu order — the registry. Ordered so each
    /// [`group`](Action::group) is contiguous, so the run-action menu emits one
    /// header per group (#514). Names are stable regardless of this order.
    pub(crate) const ALL: &'static [Self] = &[
        // Project
        Self::Save,
        Self::ProjectFind,
        Self::SwitchBook,
        Self::ManageBooks,
        Self::SeriesTree,
        Self::RenameProject,
        Self::BookDetails,
        Self::SetGoals,
        Self::ImportNotes,
        Self::NewProjectFromNotes,
        Self::ImportManuscript,
        Self::ImportReviewComments,
        Self::ArchiveProject,
        Self::RestoreArchive,
        // Export
        Self::Export(ExportFormat::Markdown),
        Self::Export(ExportFormat::Docx),
        Self::Export(ExportFormat::Scriv),
        Self::Export(ExportFormat::Vellum),
        Self::Export(ExportFormat::Epub),
        Self::Export(ExportFormat::Pdf),
        Self::Export(ExportFormat::PdfPrint),
        Self::PdfWrapCover,
        Self::ReviewCopy,
        Self::PreflightExport,
        Self::PrepareForKdp,
        Self::PrepareForIngramSpark,
        Self::ExportBible,
        Self::BrowseFonts,
        Self::SetExportStyle,
        // Reports
        Self::ProgressReport,
        Self::AnalysisReport,
        Self::WritingHistory,
        // AI
        Self::EnableAi,
        Self::Ai(AiAction::Ask),
        Self::Ai(AiAction::Characters),
        Self::Ai(AiAction::Continuity),
        Self::Ai(AiAction::Setting),
        Self::Ai(AiAction::Plot),
        Self::Ai(AiAction::Theme),
        Self::Ai(AiAction::Scaffold),
        Self::Ai(AiAction::Naming),
        Self::Ai(AiAction::Style),
        Self::Ai(AiAction::Brainstorm),
        Self::Ai(AiAction::DraftCharacters),
        Self::Ai(AiAction::DraftSetting),
        Self::Ai(AiAction::Voice),
        Self::Ai(AiAction::Submission),
        Self::Ai(AiAction::Marketing),
        Self::Ai(AiAction::Social),
        Self::Ai(AiAction::AdCreative),
        Self::Ai(AiAction::Portrait),
        Self::Ai(AiAction::Scene),
        Self::Ai(AiAction::Triage),
        Self::Ai(AiAction::Dashboard),
        Self::Ai(AiAction::AskHistory),
        Self::Ai(AiAction::Thesaurus),
        Self::Ai(AiAction::NameGen),
        Self::Ai(AiAction::Editorial),
        Self::RefreshBibles,
        Self::RefreshSeriesBible,
        Self::DescribeConfig,
        Self::SwitchBackend,
        Self::ManageModels,
        Self::SetClaudeKey,
        Self::SetImageKey,
        Self::Assistant,
        Self::BrowseBible,
        // Editing
        Self::AcceptWord,
        Self::NextMisspelling,
        Self::PrevMisspelling,
        Self::AddAnnotation,
        Self::ReviewAnnotations,
        Self::InsertSnippet,
        Self::StartSprint,
        Self::StopSprint,
        // View
        Self::ToggleTypewriter,
        Self::ToggleFocus,
        Self::ToggleParagraphIndent,
        Self::Corkboard,
        Self::ManuscriptCorkboard,
        Self::Settings,
        // Help
        Self::UserGuide,
        Self::QuickStart,
    ];

    /// The stable name the run-action command matches and workflows (#89)
    /// reference in config — never change these without a migration.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Export(ExportFormat::Markdown) => "export:markdown",
            Self::Export(ExportFormat::Docx) => "export:docx",
            Self::Export(ExportFormat::Scriv) => "export:scriv",
            Self::Export(ExportFormat::Vellum) => "export:vellum",
            Self::Export(ExportFormat::Epub) => "export:epub",
            Self::Export(ExportFormat::Pdf) => "export:pdf",
            Self::Export(ExportFormat::PdfPrint) => "export:pdf-print",
            Self::PdfWrapCover => "export:pdf-wrap-cover",
            Self::ProgressReport => "report:progress",
            Self::AnalysisReport => "report:analysis",
            Self::WritingHistory => "report:history",
            // The AI slugs are the single source in `AiAction::SPECS` (#563).
            Self::Ai(a) => a.slug(),
            Self::EnableAi => "ai:enable",
            Self::UserGuide => "guide",
            Self::QuickStart => "quickstart",
            Self::ImportNotes => "import",
            Self::NewProjectFromNotes => "new-from-notes",
            Self::ImportManuscript => "import:manuscript",
            Self::ImportReviewComments => "import:review-comments",
            Self::ArchiveProject => "archive",
            Self::RestoreArchive => "archive:restore",
            Self::StartSprint => "sprint:start",
            Self::StopSprint => "sprint:stop",
            Self::SwitchBook => "book:switch",
            Self::ManageBooks => "book:manage",
            Self::SeriesTree => "tree",
            Self::RenameProject => "project:rename",
            Self::BookDetails => "book:details",
            Self::SetGoals => "goals:set",
            Self::RefreshSeriesBible => "bible:series",
            Self::RefreshBibles => "bible:refresh-all",
            Self::ProjectFind => "find:project",
            Self::DescribeConfig => "config:describe",
            Self::AcceptWord => "dict:accept",
            Self::NextMisspelling => "spell:next",
            Self::PrevMisspelling => "spell:prev",
            Self::ToggleTypewriter => "view:typewriter",
            Self::ToggleFocus => "view:focus",
            Self::ToggleParagraphIndent => "view:paragraph-indent",
            Self::Corkboard => "view:corkboard",
            Self::ManuscriptCorkboard => "view:corkboard-all",
            Self::AddAnnotation => "annotate",
            Self::ReviewAnnotations => "annotate:review",
            Self::SwitchBackend => "ai:backend",
            Self::ManageModels => "ai:models",
            Self::SetClaudeKey => "ai:set-key",
            Self::SetImageKey => "image:set-key",
            Self::Assistant => "assistant",
            Self::BrowseBible => "bible:browse",
            Self::InsertSnippet => "snippet",
            Self::BrowseFonts => "fonts:browse",
            Self::SetExportStyle => "style:set",
            Self::Settings => "settings",
            Self::ExportBible => "export:bible",
            Self::PreflightExport => "export:preflight",
            Self::PrepareForKdp => "export:kdp",
            Self::PrepareForIngramSpark => "export:ingramspark",
            Self::ReviewCopy => "export:review",
        }
    }

    /// The collapsible group this action sits under in the run-action menu (#514).
    /// `ALL` is ordered so each group's actions are contiguous, so the menu emits
    /// one header per group as it iterates. AI passes and the AI-powered helpers
    /// gather under one "AI" heading, the large block the grouping is meant to tame.
    pub(crate) const fn group(self) -> &'static str {
        match self {
            Self::Save
            | Self::ProjectFind
            | Self::SwitchBook
            | Self::ManageBooks
            | Self::SeriesTree
            | Self::RenameProject
            | Self::BookDetails
            | Self::SetGoals
            | Self::ImportNotes
            | Self::NewProjectFromNotes
            | Self::ImportManuscript
            | Self::ImportReviewComments
            | Self::ArchiveProject
            | Self::RestoreArchive => "Project",
            Self::Export(_)
            | Self::PdfWrapCover
            | Self::ReviewCopy
            | Self::PreflightExport
            | Self::PrepareForKdp
            | Self::PrepareForIngramSpark
            | Self::ExportBible
            | Self::BrowseFonts
            | Self::SetExportStyle => "Export",
            Self::ProgressReport | Self::AnalysisReport | Self::WritingHistory => "Reports",
            Self::Ai(_)
            | Self::EnableAi
            | Self::RefreshBibles
            | Self::RefreshSeriesBible
            | Self::DescribeConfig
            | Self::SwitchBackend
            | Self::ManageModels
            | Self::SetClaudeKey
            | Self::SetImageKey
            | Self::Assistant
            | Self::BrowseBible => "AI",
            Self::AcceptWord
            | Self::NextMisspelling
            | Self::PrevMisspelling
            | Self::AddAnnotation
            | Self::ReviewAnnotations
            | Self::InsertSnippet
            | Self::StartSprint
            | Self::StopSprint => "Editing",
            Self::ToggleTypewriter
            | Self::ToggleFocus
            | Self::ToggleParagraphIndent
            | Self::Corkboard
            | Self::ManuscriptCorkboard
            | Self::Settings => "View",
            Self::UserGuide | Self::QuickStart => "Help",
        }
    }

    /// Human label for the run-action menu.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Save => "Save",
            Self::Export(ExportFormat::Markdown) => "Export → Markdown",
            Self::Export(ExportFormat::Docx) => "Export → Word (.docx)",
            Self::Export(ExportFormat::Scriv) => "Export → Scrivener",
            Self::Export(ExportFormat::Vellum) => "Export → Vellum (.docx)",
            Self::Export(ExportFormat::Epub) => "Export → EPUB",
            Self::Export(ExportFormat::Pdf) => "Export → PDF",
            Self::Export(ExportFormat::PdfPrint) => "Export → PDF (print interior)",
            Self::PdfWrapCover => "Export → PDF wrap cover (front + spine + back)",
            Self::ProgressReport => "Progress report",
            Self::AnalysisReport => "Analysis report",
            Self::WritingHistory => "Writing history",
            Self::Ai(AiAction::Ask) => "AI · Ask the manuscript",
            Self::Ai(AiAction::Characters) => "AI · Character bible",
            Self::Ai(AiAction::Continuity) => "AI · Timeline & continuity",
            Self::Ai(AiAction::Setting) => "AI · Setting & worldbuilding",
            Self::Ai(AiAction::Plot) => "AI · Plot & arc tracker",
            Self::Ai(AiAction::Theme) => "AI · Theme & motif tracker",
            Self::Ai(AiAction::Scaffold) => "AI · Scaffold / outline",
            Self::Ai(AiAction::Naming) => "AI · Name & term consistency",
            Self::Ai(AiAction::Style) => "AI · Prose & style",
            Self::Ai(AiAction::Brainstorm) => "AI · Brainstorm ideas",
            Self::Ai(AiAction::DraftCharacters) => "AI · Draft characters",
            Self::Ai(AiAction::DraftSetting) => "AI · Draft setting",
            Self::Ai(AiAction::Voice) => "AI · Character voice",
            Self::Ai(AiAction::Submission) => "AI · Submission material",
            Self::Ai(AiAction::Marketing) => "AI · Marketing copy",
            Self::Ai(AiAction::Social) => "AI · Social posts",
            Self::Ai(AiAction::AdCreative) => "AI · Ad creative",
            Self::Ai(AiAction::Portrait) => "AI · Character portraits",
            Self::Ai(AiAction::Scene) => "AI · Scene illustration",
            Self::Ai(AiAction::Triage) => "Revision triage",
            Self::Ai(AiAction::Dashboard) => "Manuscript health dashboard",
            Self::Ai(AiAction::AskHistory) => "AI · Ask history",
            Self::Ai(AiAction::Thesaurus) => "AI · Word search (thesaurus)",
            Self::Ai(AiAction::NameGen) => "AI · Name generator",
            Self::Ai(AiAction::Editorial) => "AI · Editorial read (flags, no fixes)",
            Self::EnableAi => "Enable AI assistance",
            Self::UserGuide => "User's guide",
            Self::QuickStart => "Quick-start (first-run card)",
            Self::ImportNotes => "Import notes → story bible",
            Self::NewProjectFromNotes => "New project from a folder of notes",
            Self::ImportManuscript => "Import a manuscript → new project",
            Self::ImportReviewComments => "Import review comments (.docx) → annotations",
            Self::ArchiveProject => "Archive project… (.zip backup)",
            Self::RestoreArchive => "Restore project from archive…",
            Self::StartSprint => "Start a writing sprint",
            Self::StopSprint => "Stop the writing sprint",
            Self::SwitchBook => "Switch book",
            Self::ManageBooks => "Manage books (series)…",
            Self::SeriesTree => "Project tree",
            Self::RenameProject => "Rename project",
            Self::BookDetails => "Book details…",
            Self::SetGoals => "Set writing goals",
            Self::RefreshSeriesBible => "Refresh series bible",
            Self::RefreshBibles => "Refresh all bibles",
            Self::ProjectFind => "Find across manuscript",
            Self::DescribeConfig => "Configure by description",
            Self::AcceptWord => "Add word to dictionary",
            Self::NextMisspelling => "Next misspelling",
            Self::PrevMisspelling => "Previous misspelling",
            Self::ToggleTypewriter => "Typewriter scrolling (toggle)",
            Self::ToggleFocus => "Focus mode (toggle)",
            Self::ToggleParagraphIndent => "Paragraph indent (toggle)",
            Self::Corkboard => "Corkboard (scene overview)",
            Self::ManuscriptCorkboard => "Corkboard — whole manuscript",
            Self::AddAnnotation => "Annotate selection",
            Self::ReviewAnnotations => "Review annotations",
            Self::SwitchBackend => "Switch AI backend (Claude / local)",
            Self::ManageModels => "Local AI models",
            Self::SetClaudeKey => "Set Claude API key (keychain)",
            Self::SetImageKey => "Set image API key (keychain)",
            Self::Assistant => "Authorial assistant",
            Self::BrowseBible => "Browse story bible",
            Self::InsertSnippet => "Insert snippet…",
            Self::BrowseFonts => "Browse & download fonts…",
            Self::SetExportStyle => "Set export style…",
            Self::Settings => "Settings…",
            Self::ExportBible => "Export story bible → Markdown",
            Self::PreflightExport => "Export preflight (publish health check)",
            Self::PrepareForKdp => "Prepare for KDP (preflight + print PDF + EPUB)",
            Self::PrepareForIngramSpark => "Prepare for IngramSpark (preflight + print PDF + EPUB)",
            Self::ReviewCopy => "Export review copy (numbered lines + annotations as comments)",
        }
    }

    /// Look up an action by its stable [`name`](Action::name) — the resolver a
    /// workflow (#226) uses to turn a config step token into an action.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.name() == name)
    }

    /// Whether this action completes synchronously, so a workflow (#226) can run
    /// it and move straight to the next step. The network AI passes are async
    /// (workflow support is #227) and "Ask" waits for typed input; `triage` is
    /// local aggregation, so it counts as synchronous.
    pub(crate) const fn is_sync(self) -> bool {
        match self {
            // Local, no network request — like Triage, these run synchronously.
            Self::Ai(AiAction::Triage | AiAction::Dashboard | AiAction::AskHistory) => true,
            Self::Ai(_) => false,
            _ => true,
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
    /// and the off-manuscript copy generators, kept out of autonomous reach.
    pub(crate) const fn assistant_runnable(self) -> bool {
        matches!(
            self,
            Self::Save
                | Self::Export(_)
                | Self::PdfWrapCover
                | Self::ReviewCopy
                | Self::ExportBible
                | Self::PreflightExport
                | Self::PrepareForKdp
                | Self::PrepareForIngramSpark
                | Self::ProgressReport
                | Self::AnalysisReport
                | Self::WritingHistory
                | Self::UserGuide
                | Self::AcceptWord
                | Self::NextMisspelling
                | Self::PrevMisspelling
                | Self::ToggleTypewriter
                | Self::ToggleFocus
                | Self::ToggleParagraphIndent
                | Self::SwitchBackend
                | Self::Ai(
                    AiAction::Characters
                        | AiAction::Continuity
                        | AiAction::Setting
                        | AiAction::Plot
                        | AiAction::Theme
                        | AiAction::Scaffold
                        | AiAction::Naming
                        | AiAction::Style
                        | AiAction::Voice
                        | AiAction::Editorial
                        | AiAction::Ask
                        | AiAction::Brainstorm
                        | AiAction::DraftCharacters
                        | AiAction::DraftSetting
                )
        )
    }

    /// The JSON property name of this action's single free-text argument (#390),
    /// for the actions that take one — the question for Ask, the premise for
    /// Brainstorm, the concept for the bible drafts. `None` for argument-free
    /// actions. The single source the tool schema and the dispatch both read, so a
    /// tool input never drifts from what the action consumes.
    pub(crate) const fn text_arg(self) -> Option<&'static str> {
        match self {
            Self::Ai(AiAction::Ask) => Some("question"),
            Self::Ai(AiAction::Brainstorm) => Some("premise"),
            Self::Ai(AiAction::DraftCharacters | AiAction::DraftSetting) => Some("concept"),
            _ => None,
        }
    }
}

/// A manuscript export target (#7), chosen from the export palette.
#[derive(Clone, Copy, Debug)]
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
}

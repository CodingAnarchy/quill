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
pub(crate) enum Action {
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
}

impl Action {
    /// Every action, grouped in menu order — the registry. Ordered so each
    /// [`group`](Action::group) is contiguous, so the run-action menu emits one
    /// header per group (#514). Names are stable regardless of this order.
    pub(crate) const ALL: &'static [Action] = &[
        // Project
        Action::Save,
        Action::ProjectFind,
        Action::SwitchBook,
        Action::ManageBooks,
        Action::SeriesTree,
        Action::RenameProject,
        Action::BookDetails,
        Action::SetGoals,
        Action::ImportNotes,
        Action::NewProjectFromNotes,
        Action::ImportManuscript,
        Action::ArchiveProject,
        Action::RestoreArchive,
        // Export
        Action::Export(ExportFormat::Markdown),
        Action::Export(ExportFormat::Docx),
        Action::Export(ExportFormat::Scriv),
        Action::Export(ExportFormat::Vellum),
        Action::Export(ExportFormat::Epub),
        Action::Export(ExportFormat::Pdf),
        Action::Export(ExportFormat::PdfPrint),
        Action::ExportBible,
        Action::BrowseFonts,
        Action::SetExportStyle,
        // Reports
        Action::ProgressReport,
        Action::AnalysisReport,
        Action::WritingHistory,
        // AI
        Action::EnableAi,
        Action::Ai(AiAction::Ask),
        Action::Ai(AiAction::Characters),
        Action::Ai(AiAction::Continuity),
        Action::Ai(AiAction::Setting),
        Action::Ai(AiAction::Plot),
        Action::Ai(AiAction::Scaffold),
        Action::Ai(AiAction::Naming),
        Action::Ai(AiAction::Style),
        Action::Ai(AiAction::Brainstorm),
        Action::Ai(AiAction::DraftCharacters),
        Action::Ai(AiAction::DraftSetting),
        Action::Ai(AiAction::Voice),
        Action::Ai(AiAction::Submission),
        Action::Ai(AiAction::Marketing),
        Action::Ai(AiAction::Social),
        Action::Ai(AiAction::AdCreative),
        Action::Ai(AiAction::Portrait),
        Action::Ai(AiAction::Scene),
        Action::Ai(AiAction::Triage),
        Action::Ai(AiAction::AskHistory),
        Action::Ai(AiAction::Thesaurus),
        Action::Ai(AiAction::NameGen),
        Action::Ai(AiAction::Editorial),
        Action::RefreshBibles,
        Action::RefreshSeriesBible,
        Action::DescribeConfig,
        Action::SwitchBackend,
        Action::ManageModels,
        Action::SetClaudeKey,
        Action::SetImageKey,
        Action::Assistant,
        Action::BrowseBible,
        // Editing
        Action::AcceptWord,
        Action::NextMisspelling,
        Action::PrevMisspelling,
        Action::AddAnnotation,
        Action::ReviewAnnotations,
        Action::InsertSnippet,
        Action::StartSprint,
        Action::StopSprint,
        // View
        Action::ToggleTypewriter,
        Action::ToggleFocus,
        Action::ToggleParagraphIndent,
        Action::Corkboard,
        Action::ManuscriptCorkboard,
        Action::Settings,
        // Help
        Action::UserGuide,
    ];

    /// The stable name the run-action command matches and workflows (#89)
    /// reference in config — never change these without a migration.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Action::Save => "save",
            Action::Export(ExportFormat::Markdown) => "export:markdown",
            Action::Export(ExportFormat::Docx) => "export:docx",
            Action::Export(ExportFormat::Scriv) => "export:scriv",
            Action::Export(ExportFormat::Vellum) => "export:vellum",
            Action::Export(ExportFormat::Epub) => "export:epub",
            Action::Export(ExportFormat::Pdf) => "export:pdf",
            Action::Export(ExportFormat::PdfPrint) => "export:pdf-print",
            Action::ProgressReport => "report:progress",
            Action::AnalysisReport => "report:analysis",
            Action::WritingHistory => "report:history",
            // The AI slugs are the single source in `AiAction::SPECS` (#563).
            Action::Ai(a) => a.slug(),
            Action::EnableAi => "ai:enable",
            Action::UserGuide => "guide",
            Action::ImportNotes => "import",
            Action::NewProjectFromNotes => "new-from-notes",
            Action::ImportManuscript => "import:manuscript",
            Action::ArchiveProject => "archive",
            Action::RestoreArchive => "archive:restore",
            Action::StartSprint => "sprint:start",
            Action::StopSprint => "sprint:stop",
            Action::SwitchBook => "book:switch",
            Action::ManageBooks => "book:manage",
            Action::SeriesTree => "tree",
            Action::RenameProject => "project:rename",
            Action::BookDetails => "book:details",
            Action::SetGoals => "goals:set",
            Action::RefreshSeriesBible => "bible:series",
            Action::RefreshBibles => "bible:refresh-all",
            Action::ProjectFind => "find:project",
            Action::DescribeConfig => "config:describe",
            Action::AcceptWord => "dict:accept",
            Action::NextMisspelling => "spell:next",
            Action::PrevMisspelling => "spell:prev",
            Action::ToggleTypewriter => "view:typewriter",
            Action::ToggleFocus => "view:focus",
            Action::ToggleParagraphIndent => "view:paragraph-indent",
            Action::Corkboard => "view:corkboard",
            Action::ManuscriptCorkboard => "view:corkboard-all",
            Action::AddAnnotation => "annotate",
            Action::ReviewAnnotations => "annotate:review",
            Action::SwitchBackend => "ai:backend",
            Action::ManageModels => "ai:models",
            Action::SetClaudeKey => "ai:set-key",
            Action::SetImageKey => "image:set-key",
            Action::Assistant => "assistant",
            Action::BrowseBible => "bible:browse",
            Action::InsertSnippet => "snippet",
            Action::BrowseFonts => "fonts:browse",
            Action::SetExportStyle => "style:set",
            Action::Settings => "settings",
            Action::ExportBible => "export:bible",
        }
    }

    /// The collapsible group this action sits under in the run-action menu (#514).
    /// `ALL` is ordered so each group's actions are contiguous, so the menu emits
    /// one header per group as it iterates. AI passes and the AI-powered helpers
    /// gather under one "AI" heading, the large block the grouping is meant to tame.
    pub(crate) fn group(self) -> &'static str {
        match self {
            Action::Save
            | Action::ProjectFind
            | Action::SwitchBook
            | Action::ManageBooks
            | Action::SeriesTree
            | Action::RenameProject
            | Action::BookDetails
            | Action::SetGoals
            | Action::ImportNotes
            | Action::NewProjectFromNotes
            | Action::ImportManuscript
            | Action::ArchiveProject
            | Action::RestoreArchive => "Project",
            Action::Export(_)
            | Action::ExportBible
            | Action::BrowseFonts
            | Action::SetExportStyle => "Export",
            Action::ProgressReport | Action::AnalysisReport | Action::WritingHistory => "Reports",
            Action::Ai(_)
            | Action::EnableAi
            | Action::RefreshBibles
            | Action::RefreshSeriesBible
            | Action::DescribeConfig
            | Action::SwitchBackend
            | Action::ManageModels
            | Action::SetClaudeKey
            | Action::SetImageKey
            | Action::Assistant
            | Action::BrowseBible => "AI",
            Action::AcceptWord
            | Action::NextMisspelling
            | Action::PrevMisspelling
            | Action::AddAnnotation
            | Action::ReviewAnnotations
            | Action::InsertSnippet
            | Action::StartSprint
            | Action::StopSprint => "Editing",
            Action::ToggleTypewriter
            | Action::ToggleFocus
            | Action::ToggleParagraphIndent
            | Action::Corkboard
            | Action::ManuscriptCorkboard
            | Action::Settings => "View",
            Action::UserGuide => "Help",
        }
    }

    /// Human label for the run-action menu.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Action::Save => "Save",
            Action::Export(ExportFormat::Markdown) => "Export → Markdown",
            Action::Export(ExportFormat::Docx) => "Export → Word (.docx)",
            Action::Export(ExportFormat::Scriv) => "Export → Scrivener",
            Action::Export(ExportFormat::Vellum) => "Export → Vellum (.docx)",
            Action::Export(ExportFormat::Epub) => "Export → EPUB",
            Action::Export(ExportFormat::Pdf) => "Export → PDF",
            Action::Export(ExportFormat::PdfPrint) => "Export → PDF (print interior)",
            Action::ProgressReport => "Progress report",
            Action::AnalysisReport => "Analysis report",
            Action::WritingHistory => "Writing history",
            Action::Ai(AiAction::Ask) => "AI · Ask the manuscript",
            Action::Ai(AiAction::Characters) => "AI · Character bible",
            Action::Ai(AiAction::Continuity) => "AI · Timeline & continuity",
            Action::Ai(AiAction::Setting) => "AI · Setting & worldbuilding",
            Action::Ai(AiAction::Plot) => "AI · Plot & arc tracker",
            Action::Ai(AiAction::Scaffold) => "AI · Scaffold / outline",
            Action::Ai(AiAction::Naming) => "AI · Name & term consistency",
            Action::Ai(AiAction::Style) => "AI · Prose & style",
            Action::Ai(AiAction::Brainstorm) => "AI · Brainstorm ideas",
            Action::Ai(AiAction::DraftCharacters) => "AI · Draft characters",
            Action::Ai(AiAction::DraftSetting) => "AI · Draft setting",
            Action::Ai(AiAction::Voice) => "AI · Character voice",
            Action::Ai(AiAction::Submission) => "AI · Submission material",
            Action::Ai(AiAction::Marketing) => "AI · Marketing copy",
            Action::Ai(AiAction::Social) => "AI · Social posts",
            Action::Ai(AiAction::AdCreative) => "AI · Ad creative",
            Action::Ai(AiAction::Portrait) => "AI · Character portraits",
            Action::Ai(AiAction::Scene) => "AI · Scene illustration",
            Action::Ai(AiAction::Triage) => "Revision triage",
            Action::Ai(AiAction::AskHistory) => "AI · Ask history",
            Action::Ai(AiAction::Thesaurus) => "AI · Word search (thesaurus)",
            Action::Ai(AiAction::NameGen) => "AI · Name generator",
            Action::Ai(AiAction::Editorial) => "AI · Editorial read (flags, no fixes)",
            Action::EnableAi => "Enable AI assistance",
            Action::UserGuide => "User's guide",
            Action::ImportNotes => "Import notes → story bible",
            Action::NewProjectFromNotes => "New project from a folder of notes",
            Action::ImportManuscript => "Import a manuscript → new project",
            Action::ArchiveProject => "Archive project… (.zip backup)",
            Action::RestoreArchive => "Restore project from archive…",
            Action::StartSprint => "Start a writing sprint",
            Action::StopSprint => "Stop the writing sprint",
            Action::SwitchBook => "Switch book",
            Action::ManageBooks => "Manage books (series)…",
            Action::SeriesTree => "Project tree",
            Action::RenameProject => "Rename project",
            Action::BookDetails => "Book details…",
            Action::SetGoals => "Set writing goals",
            Action::RefreshSeriesBible => "Refresh series bible",
            Action::RefreshBibles => "Refresh all bibles",
            Action::ProjectFind => "Find across manuscript",
            Action::DescribeConfig => "Configure by description",
            Action::AcceptWord => "Add word to dictionary",
            Action::NextMisspelling => "Next misspelling",
            Action::PrevMisspelling => "Previous misspelling",
            Action::ToggleTypewriter => "Typewriter scrolling (toggle)",
            Action::ToggleFocus => "Focus mode (toggle)",
            Action::ToggleParagraphIndent => "Paragraph indent (toggle)",
            Action::Corkboard => "Corkboard (scene overview)",
            Action::ManuscriptCorkboard => "Corkboard — whole manuscript",
            Action::AddAnnotation => "Annotate selection",
            Action::ReviewAnnotations => "Review annotations",
            Action::SwitchBackend => "Switch AI backend (Claude / local)",
            Action::ManageModels => "Local AI models",
            Action::SetClaudeKey => "Set Claude API key (keychain)",
            Action::SetImageKey => "Set image API key (keychain)",
            Action::Assistant => "Authorial assistant",
            Action::BrowseBible => "Browse story bible",
            Action::InsertSnippet => "Insert snippet…",
            Action::BrowseFonts => "Browse & download fonts…",
            Action::SetExportStyle => "Set export style…",
            Action::Settings => "Settings…",
            Action::ExportBible => "Export story bible → Markdown",
        }
    }

    /// Look up an action by its stable [`name`](Action::name) — the resolver a
    /// workflow (#226) uses to turn a config step token into an action.
    pub(crate) fn from_name(name: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == name)
    }

    /// Whether this action completes synchronously, so a workflow (#226) can run
    /// it and move straight to the next step. The network AI passes are async
    /// (workflow support is #227) and "Ask" waits for typed input; `triage` is
    /// local aggregation, so it counts as synchronous.
    pub(crate) fn is_sync(self) -> bool {
        match self {
            // Local, no network request — like Triage, these run synchronously.
            Action::Ai(AiAction::Triage | AiAction::AskHistory) => true,
            Action::Ai(_) => false,
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
    pub(crate) fn assistant_runnable(self) -> bool {
        matches!(
            self,
            Action::Save
                | Action::Export(_)
                | Action::ExportBible
                | Action::ProgressReport
                | Action::AnalysisReport
                | Action::WritingHistory
                | Action::UserGuide
                | Action::AcceptWord
                | Action::NextMisspelling
                | Action::PrevMisspelling
                | Action::ToggleTypewriter
                | Action::ToggleFocus
                | Action::ToggleParagraphIndent
                | Action::SwitchBackend
                | Action::Ai(
                    AiAction::Characters
                        | AiAction::Continuity
                        | AiAction::Setting
                        | AiAction::Plot
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
    pub(crate) fn text_arg(self) -> Option<&'static str> {
        match self {
            Action::Ai(AiAction::Ask) => Some("question"),
            Action::Ai(AiAction::Brainstorm) => Some("premise"),
            Action::Ai(AiAction::DraftCharacters | AiAction::DraftSetting) => Some("concept"),
            _ => None,
        }
    }
}

/// A manuscript export target (#7), chosen from the export palette.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExportFormat {
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
pub(crate) enum ExportScope {
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
    pub(crate) fn chapter_span(&self) -> Option<std::ops::RangeInclusive<usize>> {
        match self {
            ExportScope::ActiveBook | ExportScope::Series => None,
            ExportScope::Chapter(i) => Some(*i..=*i),
            ExportScope::Part { start, end, .. } | ExportScope::Range { start, end } => {
                Some(*start..=*end)
            }
        }
    }
}

/// The version-control actions reachable from the Git palette (#54).
#[derive(Clone, Copy)]
pub(crate) enum GitAction {
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
    /// Set (or change) the remote URL the project backs up to (#134).
    SetRemote,
    /// Push local commits to the remote (#134).
    Push,
    /// Pull the remote's changes down (#134).
    Pull,
}

//! The action / command / palette layer (#556): the unified run-action registry
//! ([`Action`]), the menu/command enums the palettes emit ([`Command`] and the
//! export/AI/git target enums), and the collapsible palette types ([`Palette`]).
//! Split out of `mode.rs` to keep the core mode enums under the ~1,000-line
//! convention; re-exported from there, so callers (`crate::mode::{Action, …}`)
//! are unaffected. Pure move.

/// State for an open [`Mode::Palette`](super::Mode::Palette): the entries, the
/// selected row, and which group headings are collapsed. `selected` indexes the
/// *visible* rows ([`Palette::rows`]) — headers plus the items of expanded groups
/// — not `items`, so it tracks what's on screen as groups collapse (#514).
pub struct Palette {
    /// Title shown in the palette's bar (e.g. "AI", "Version control").
    pub(crate) title: &'static str,
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) selected: usize,
    /// Collapsed group headings — their items are hidden until expanded. Empty for
    /// a flat palette (every item ungrouped), which then renders exactly as before.
    pub(crate) collapsed: std::collections::HashSet<&'static str>,
}

/// One palette entry: a label, a one-line description, the command it runs, and an
/// optional group it belongs to (for the collapsible run-action menu, #514).
pub struct PaletteItem {
    pub(crate) label: String,
    pub(crate) desc: String,
    pub(crate) command: Command,
    /// The collapsible group this item sits under, or `None` for a flat (header-
    /// less) palette.
    pub(crate) group: Option<&'static str>,
}

/// A rendered row of an open palette: a collapsible group header, or a runnable
/// item carrying its index into [`Palette::items`] (#514).
pub(crate) enum PaletteRow {
    Header {
        group: &'static str,
        collapsed: bool,
    },
    Item(usize),
}

impl PaletteItem {
    /// An ungrouped palette entry — the default; flat palettes (AI, export, git,
    /// snippets, …) use this and render exactly as before.
    pub(crate) fn new(label: impl Into<String>, desc: impl Into<String>, command: Command) -> Self {
        PaletteItem {
            label: label.into(),
            desc: desc.into(),
            command,
            group: None,
        }
    }

    /// Place this entry under a collapsible group heading (the run-action menu).
    pub(crate) fn in_group(mut self, group: &'static str) -> Self {
        self.group = Some(group);
        self
    }
}

impl Palette {
    /// The rows to show: a header before each group (an item whose group differs
    /// from the one above), then that group's items unless it's collapsed.
    /// Ungrouped items render flat with no header, so a palette with no groups
    /// yields exactly its items — the pre-#514 behavior.
    pub(crate) fn rows(&self) -> Vec<PaletteRow> {
        let mut rows = Vec::with_capacity(self.items.len());
        let mut current: Option<&'static str> = None;
        let mut hidden = false;
        for (i, item) in self.items.iter().enumerate() {
            match item.group {
                Some(g) if current == Some(g) => {
                    if !hidden {
                        rows.push(PaletteRow::Item(i));
                    }
                }
                Some(g) => {
                    current = Some(g);
                    hidden = self.collapsed.contains(g);
                    rows.push(PaletteRow::Header {
                        group: g,
                        collapsed: hidden,
                    });
                    if !hidden {
                        rows.push(PaletteRow::Item(i));
                    }
                }
                None => {
                    current = None;
                    hidden = false;
                    rows.push(PaletteRow::Item(i));
                }
            }
        }
        rows
    }

    /// Collapse (`want = true`) or expand a group heading (#514).
    pub(crate) fn set_collapsed(&mut self, group: &'static str, want: bool) {
        if want {
            self.collapsed.insert(group);
        } else {
            self.collapsed.remove(group);
        }
    }
}

/// A command reachable from a palette — the AI menu (Ctrl-A), the version-control
/// menu (Ctrl-V), and the export menu (Ctrl-E) share one palette mechanism.
#[derive(Clone, Copy)]
pub(crate) enum Command {
    Ai(AiAction),
    Git(GitAction),
    Export(ExportFormat, ExportScope),
    /// Open a styled sample EPUB of the current `[style]` settings in the system
    /// reader (#483), so an author can see a preset + overrides before exporting.
    PreviewStyle,
    /// Run a registry [`Action`] from the unified run-action menu (#225). This is
    /// the going-forward path; `Ai`/`Export` above remain as their own variants
    /// only because the AI and export palettes still emit them directly — folding
    /// those palettes onto the registry is follow-up work (#226).
    Run(Action),
    /// Run the author workflow at this index in `App::workflows` (#226).
    RunWorkflow(usize),
    /// Start building a workflow from a plain-language description (#232).
    NewWorkflowFromPrompt,
    /// Generate portraits for the chosen character(s) (#244).
    Portrait(PortraitChoice),
    /// Make the book at this index the active book (#329).
    SwitchBook(usize),
    /// Insert the author-defined snippet at this index in `App::snippets` (#411).
    InsertSnippet(usize),
}

/// Which character(s) a portrait pass should generate (#244), chosen from the
/// character-portrait picker.
#[derive(Clone, Copy)]
pub(crate) enum PortraitChoice {
    /// Every character in the bible.
    All,
    /// The character at this index in the (sorted) bible list.
    One(usize),
}

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
    /// Run an AI pass / open an AI view (Ctrl-A).
    Ai(AiAction),
    /// Open the built-in user's guide (F1) (#281).
    UserGuide,
    /// Import a Markdown notes file (or a folder of them) into the story bible
    /// (#283).
    ImportNotes,
    /// Create a new project from a folder of notes (#286).
    NewProjectFromNotes,
    /// Import an existing Markdown manuscript as a new project (#298).
    ImportManuscript,
    /// Start a timed writing sprint (#299).
    StartSprint,
    /// Stop the active writing sprint (#299).
    StopSprint,
    /// Switch the active book of a multi-book series (#329).
    SwitchBook,
    /// Open the series tree navigator (#330).
    SeriesTree,
    /// Rename the open project's folder, matching it to the book title (#424).
    RenameProject,
    /// Set or change the project's writing goals — target words, target chapters,
    /// and the daily word target (#550).
    SetGoals,
    /// Refresh the whole series' story bible, book by book (#340).
    RefreshSeriesBible,
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
    /// Open the snippet picker to insert an author-defined snippet (#411).
    InsertSnippet,
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
        Action::SeriesTree,
        Action::RenameProject,
        Action::SetGoals,
        Action::ImportNotes,
        Action::NewProjectFromNotes,
        Action::ImportManuscript,
        // Export
        Action::Export(ExportFormat::Markdown),
        Action::Export(ExportFormat::Docx),
        Action::Export(ExportFormat::Scriv),
        Action::Export(ExportFormat::Vellum),
        Action::Export(ExportFormat::Epub),
        Action::Export(ExportFormat::Pdf),
        Action::Export(ExportFormat::PdfPrint),
        // Reports
        Action::ProgressReport,
        Action::AnalysisReport,
        // AI
        Action::Ai(AiAction::Ask),
        Action::Ai(AiAction::Characters),
        Action::Ai(AiAction::Continuity),
        Action::Ai(AiAction::Setting),
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
        Action::Ai(AiAction::Thesaurus),
        Action::Ai(AiAction::Editorial),
        Action::RefreshSeriesBible,
        Action::DescribeConfig,
        Action::SwitchBackend,
        Action::ManageModels,
        Action::SetClaudeKey,
        Action::SetImageKey,
        Action::Assistant,
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
            Action::Ai(AiAction::Ask) => "ai:ask",
            Action::Ai(AiAction::Characters) => "ai:characters",
            Action::Ai(AiAction::Continuity) => "ai:continuity",
            Action::Ai(AiAction::Setting) => "ai:setting",
            Action::Ai(AiAction::Scaffold) => "ai:scaffold",
            Action::Ai(AiAction::Naming) => "ai:naming",
            Action::Ai(AiAction::Style) => "ai:style",
            Action::Ai(AiAction::Brainstorm) => "ai:brainstorm",
            Action::Ai(AiAction::DraftCharacters) => "ai:draft-characters",
            Action::Ai(AiAction::DraftSetting) => "ai:draft-setting",
            Action::Ai(AiAction::Voice) => "ai:voice",
            Action::Ai(AiAction::Submission) => "ai:submission",
            Action::Ai(AiAction::Marketing) => "ai:marketing",
            Action::Ai(AiAction::Social) => "ai:social",
            Action::Ai(AiAction::AdCreative) => "ai:ad-creative",
            Action::Ai(AiAction::Portrait) => "ai:portrait",
            Action::Ai(AiAction::Scene) => "ai:scene",
            Action::Ai(AiAction::Triage) => "triage",
            Action::Ai(AiAction::Thesaurus) => "ai:thesaurus",
            Action::Ai(AiAction::Editorial) => "ai:editorial",
            Action::UserGuide => "guide",
            Action::ImportNotes => "import",
            Action::NewProjectFromNotes => "new-from-notes",
            Action::ImportManuscript => "import:manuscript",
            Action::StartSprint => "sprint:start",
            Action::StopSprint => "sprint:stop",
            Action::SwitchBook => "book:switch",
            Action::SeriesTree => "tree",
            Action::RenameProject => "project:rename",
            Action::SetGoals => "goals:set",
            Action::RefreshSeriesBible => "bible:series",
            Action::ProjectFind => "find:project",
            Action::DescribeConfig => "config:describe",
            Action::AcceptWord => "dict:accept",
            Action::NextMisspelling => "spell:next",
            Action::PrevMisspelling => "spell:prev",
            Action::ToggleTypewriter => "view:typewriter",
            Action::ToggleFocus => "view:focus",
            Action::AddAnnotation => "annotate",
            Action::ReviewAnnotations => "annotate:review",
            Action::SwitchBackend => "ai:backend",
            Action::ManageModels => "ai:models",
            Action::SetClaudeKey => "ai:set-key",
            Action::SetImageKey => "image:set-key",
            Action::Assistant => "assistant",
            Action::InsertSnippet => "snippet",
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
            | Action::SeriesTree
            | Action::RenameProject
            | Action::SetGoals
            | Action::ImportNotes
            | Action::NewProjectFromNotes
            | Action::ImportManuscript => "Project",
            Action::Export(_) => "Export",
            Action::ProgressReport | Action::AnalysisReport => "Reports",
            Action::Ai(_)
            | Action::RefreshSeriesBible
            | Action::DescribeConfig
            | Action::SwitchBackend
            | Action::ManageModels
            | Action::SetClaudeKey
            | Action::SetImageKey
            | Action::Assistant => "AI",
            Action::AcceptWord
            | Action::NextMisspelling
            | Action::PrevMisspelling
            | Action::AddAnnotation
            | Action::ReviewAnnotations
            | Action::InsertSnippet
            | Action::StartSprint
            | Action::StopSprint => "Editing",
            Action::ToggleTypewriter | Action::ToggleFocus => "View",
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
            Action::Ai(AiAction::Ask) => "AI · Ask the manuscript",
            Action::Ai(AiAction::Characters) => "AI · Character bible",
            Action::Ai(AiAction::Continuity) => "AI · Timeline & continuity",
            Action::Ai(AiAction::Setting) => "AI · Setting & worldbuilding",
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
            Action::Ai(AiAction::Thesaurus) => "AI · Word search (thesaurus)",
            Action::Ai(AiAction::Editorial) => "AI · Editorial read (flags, no fixes)",
            Action::UserGuide => "User's guide",
            Action::ImportNotes => "Import notes → story bible",
            Action::NewProjectFromNotes => "New project from a folder of notes",
            Action::ImportManuscript => "Import a manuscript → new project",
            Action::StartSprint => "Start a writing sprint",
            Action::StopSprint => "Stop the writing sprint",
            Action::SwitchBook => "Switch book",
            Action::SeriesTree => "Project tree",
            Action::RenameProject => "Rename project folder",
            Action::SetGoals => "Set writing goals",
            Action::RefreshSeriesBible => "Refresh series bible",
            Action::ProjectFind => "Find across manuscript",
            Action::DescribeConfig => "Configure by description",
            Action::AcceptWord => "Add word to dictionary",
            Action::NextMisspelling => "Next misspelling",
            Action::PrevMisspelling => "Previous misspelling",
            Action::ToggleTypewriter => "Typewriter scrolling (toggle)",
            Action::ToggleFocus => "Focus mode (toggle)",
            Action::AddAnnotation => "Annotate selection",
            Action::ReviewAnnotations => "Review annotations",
            Action::SwitchBackend => "Switch AI backend (Claude / local)",
            Action::ManageModels => "Local AI models",
            Action::SetClaudeKey => "Set Claude API key (keychain)",
            Action::SetImageKey => "Set image API key (keychain)",
            Action::Assistant => "Authorial assistant",
            Action::InsertSnippet => "Insert snippet…",
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
            Action::Ai(AiAction::Triage) => true,
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
                | Action::ProgressReport
                | Action::AnalysisReport
                | Action::UserGuide
                | Action::AcceptWord
                | Action::NextMisspelling
                | Action::PrevMisspelling
                | Action::ToggleTypewriter
                | Action::ToggleFocus
                | Action::SwitchBackend
                | Action::Ai(
                    AiAction::Characters
                        | AiAction::Continuity
                        | AiAction::Setting
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

/// What an export covers (#332): just the active book, or the whole multi-book
/// series. A standalone novel only ever exports its one book.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportScope {
    ActiveBook,
    Series,
}

/// The AI actions reachable from the AI palette.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AiAction {
    Ask,
    Characters,
    Continuity,
    Setting,
    Scaffold,
    Naming,
    Style,
    /// Brainstorm story ideas from a premise — generative, off-manuscript (#255).
    Brainstorm,
    /// Draft a starter character cast from a concept — generative (#256).
    DraftCharacters,
    /// Draft a starter setting bible from a concept — generative (#257).
    DraftSetting,
    /// Character voice consistency — dialogue & POV/narration (#183).
    Voice,
    Submission,
    /// Marketing copy — blurb variants & taglines, off-manuscript (#236).
    Marketing,
    /// Social posts — per-platform promo copy, off-manuscript (#237).
    Social,
    /// Platform-ready ad creative & copy, off-manuscript (#251).
    AdCreative,
    /// Character portrait images from the character bible (#238).
    Portrait,
    /// A key-scene illustration of the active chapter (#239).
    Scene,
    /// Open the accumulated revision-triage findings (local; no request).
    Triage,
    /// In-context thesaurus / word search for a word or phrase (#290).
    Thesaurus,
    /// Editorial read — flag structural / continuity / line issues, no fixes (#293).
    Editorial,
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

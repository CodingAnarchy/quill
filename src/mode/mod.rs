//! Editor modes: the transient state that drives key handling. Fields the
//! handlers read or write are `pub(crate)`; `label`/`input` are `pub` because
//! the status bar and the edit banner render them.

use std::collections::HashMap;

use ratatui::text::Line;

use crate::buffer::FindKind;

mod states;
pub(crate) use states::*;

/// A full-screen, read-only overlay drawn above any mode: the progress report,
/// the analysis report, or an AI answer. Holds its pre-rendered lines, how to
/// render itself, and the current scroll offset. `App` keeps a single
/// `Option<Overlay>`, so a new overlay is a new value here — not a new field
/// plus a close branch plus a draw arm.
pub struct Overlay {
    pub lines: Vec<Line<'static>>,
    pub kind: OverlayKind,
    /// First visible row; 0 when freshly opened.
    pub scroll: u16,
}

/// How an [`Overlay`] titles and renders itself.
pub enum OverlayKind {
    /// A bordered report box; the label follows the book title in the bar
    /// (e.g. "progress", "analysis"). Not word-wrapped — it holds aligned tables.
    Report(&'static str),
    /// The word-wrapped AI answer / summary panel.
    AiPanel,
    /// The built-in user's guide (#281) — a fixed help document.
    Guide,
}

impl Overlay {
    /// A bordered report overlay labelled `label`, scrolled to the top.
    pub fn report(label: &'static str, lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::Report(label),
            scroll: 0,
        }
    }

    /// The AI answer / summary overlay, scrolled to the top.
    pub fn ai(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::AiPanel,
            scroll: 0,
        }
    }

    /// The user's-guide overlay (#281), scrolled to the top.
    pub fn guide(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::Guide,
            scroll: 0,
        }
    }
}

/// Editor modes. Each variant carries the transient state that only makes sense
/// while in that mode, so illegal combinations aren't representable.
pub enum Mode {
    /// Normal prose entry. Typed characters go straight into the buffer.
    Insert,
    /// Vim Normal mode (#27, opt-in): keys are commands, not text — the cursor
    /// moves and `i`/`a`/`o` enter [`Mode::Insert`]. Only reachable when Vim mode
    /// is enabled in the config.
    Normal,
    /// Vim Visual mode (#205): a live selection from `anchor` (a byte offset, the
    /// fixed end) to the cursor (the active end). Motions extend it; `d`/`c`/`y`
    /// act on it. `linewise` is the `V` selection (whole lines) vs. `v` (chars).
    Visual { anchor: usize, linewise: bool },
    /// A transient prompt at the bottom: a y/N confirmation or a line of text
    /// input.
    Prompt(PromptState),
    /// Browsing the chapter list: arrows move the `selected` chapter, Enter
    /// opens it. `filter` is the type-to-filter / quick-jump query (#410): when
    /// non-empty the list shows only chapters whose title, status, or POV matches
    /// (case-insensitive substring). `typing` is the filter-input sub-mode —
    /// `/` enters it so keystrokes build the query instead of running the
    /// browser's single-key commands; Esc leaves it (keeping the filter) and a
    /// second Esc clears the filter.
    Nav {
        selected: usize,
        filter: String,
        typing: bool,
    },
    /// Editing a multi-line metadata field (an epigraph quotation) in the main
    /// editor: the full editing keys apply to a scratch buffer, Ctrl-S commits,
    /// Esc cancels. Boxed because it carries a whole [`Buffer`], far larger than
    /// the other variants.
    EditMeta(Box<MetaEdit>),
    /// Reviewing story-bible files whose generated sections were hand-edited
    /// before an AI pass would overwrite them: a diff of each, with overwrite /
    /// edit / keep choices. Boxed (carries the conflict queue and a scratch
    /// buffer while editing).
    BibleReview(Box<BibleReview>),
    /// Triaging continuity findings: a navigable list where each can be jumped to
    /// (open its chapter) or dismissed. Read-only over the prose. Boxed (carries
    /// the findings list).
    Findings(Box<Findings>),
    /// Reviewing the active chapter's inline annotations (#302): a navigable list
    /// where each can be jumped to or deleted. The items are read live from
    /// `App::annotations`; this carries only the selected row.
    Annotations { selected: usize },
    /// The local AI model manager (#379): the model catalog with characteristics,
    /// where the author toggles which to keep in their set. Reads the catalog and
    /// the configured set live from `App`; carries only the selected row.
    ModelManager { selected: usize },
    /// The AI command palette: a navigable list of AI actions, one of which runs
    /// on Enter. The single entry point for the AI features.
    Palette(Box<Palette>),
    /// Find / find-and-replace over the active chapter (#144): a query line at
    /// the bottom, matches highlighted in the prose, navigated with n/N. Boxed
    /// (carries the query, replacement, and the match list).
    Search(Box<Search>),
    /// Find / find-and-replace across every chapter in the project (#295): a
    /// query line, a results overlay grouped by chapter, jump-to-match, and a
    /// confirmed manuscript-wide replace. Distinct from the single-chapter
    /// [`Search`]. Boxed (carries the query, replacement, and the match list).
    ProjectSearch(Box<ProjectSearch>),
    /// Browsing the project's reference files — the story bible and loose notes
    /// (#165). Reached with `f` from the chapter browser; Enter opens the selected
    /// file in a side pane. Boxed (carries the file list).
    RefFiles(Box<RefFiles>),
    /// The series tree navigator (#330): the whole project as a series → book →
    /// part → chapter tree, navigable with the keyboard. Enter on a chapter jumps
    /// there (switching books if needed); books expand/collapse. Boxed (carries
    /// the selection + expansion state).
    Tree(Box<Tree>),
    /// Browsing recently-opened projects to switch to (#270): a navigable list;
    /// Enter switches to the selected project, Esc cancels. Boxed (carries the
    /// project list).
    ProjectBrowser(Box<ProjectBrowser>),
    /// Browsing the filesystem to find a project not in the recent list (#271):
    /// a directory navigator. Enter opens a project / descends a folder, `..`
    /// goes up, Esc cancels. Boxed (carries the directory listing).
    DirBrowser(Box<DirBrowser>),
}

/// Transient Vim Normal-mode state (#202): a pending numeric count, a multi-key
/// command awaiting its next key, and the last find for `;` / `,`. Lives on
/// `App` (not inside [`Mode::Normal`]) so `last_find` survives a trip through
/// Insert mode, the way Vim's does.
#[derive(Default)]
pub struct VimState {
    /// Digits typed as a count prefix (`5` in `5j`); `None` when none is pending.
    pub(crate) count: Option<usize>,
    /// A multi-key command awaiting another key (`g…`, an `f`/`t` target, or an
    /// `i`/`a` text-object id while an operator is pending).
    pub(crate) pending: VimPending,
    /// The last `f`/`t`/`F`/`T` motion, replayed by `;` / `,`.
    pub(crate) last_find: Option<Find>,
    /// A pending operator (`d`/`c`/`y`) and its count, awaiting a motion or text
    /// object (`dw`, `ci"`, `yi(`, …).
    pub(crate) op: Option<(Op, usize)>,
    /// The yank/delete registers (#204): the unnamed register plus the named
    /// `"a`–`"z`.
    pub(crate) registers: Registers,
    /// The register `"x` chose for the next yank / delete / put; `None` is the
    /// unnamed register.
    pub(crate) selected_register: Option<char>,
    /// The last `/` / `?` / `*` search, replayed by `n` / `N` (#206).
    pub(crate) last_search: Option<VimSearch>,
}

/// A recorded Vim search, so `n` / `N` can repeat it (#206).
#[derive(Clone)]
pub(crate) struct VimSearch {
    pub(crate) query: String,
    /// The search direction; `n` repeats it, `N` inverts it.
    pub(crate) backward: bool,
}

/// The Vim register store (#204): the unnamed register, written by every yank /
/// delete / change, and the named registers `"a`–`"z`. Numbered registers and
/// the system clipboard are out of scope.
#[derive(Default)]
pub(crate) struct Registers {
    pub(crate) unnamed: Option<Yank>,
    pub(crate) named: HashMap<char, Yank>,
}

/// A Vim Normal-mode command waiting on its next keystroke.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VimPending {
    #[default]
    None,
    /// `g` seen — awaiting the second `g` (for `gg`).
    G,
    /// `f`/`t`/`F`/`T` seen — awaiting the target character.
    Find(FindKind),
    /// `i` / `a` seen with an operator pending — awaiting the text-object id
    /// (`w`, `"`, `(`, …); `around` is the `a` form.
    Object { around: bool },
    /// `"` seen — awaiting the register name (`a`–`z`).
    Register,
    /// `]` / `[` seen — awaiting `s` for the spell-flag jump (#359): `]s` next
    /// misspelling, `[s` previous. `forward` is the `]` form.
    Bracket { forward: bool },
}

/// A Vim operator (#203): delete leaves Normal mode, change enters Insert, yank
/// (#204) copies and stays put.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Delete,
    Change,
    Yank,
}

/// Text captured by a delete / change / yank, for putting back with `p` / `P`.
#[derive(Clone)]
pub(crate) struct Yank {
    pub(crate) text: String,
    /// Whole-line text (from `dd`/`cc`/`yy`/linewise motions), put on its own
    /// lines rather than inline.
    pub(crate) linewise: bool,
}

/// A recorded find-char motion, so `;` / `,` can replay it.
#[derive(Clone, Copy)]
pub(crate) struct Find {
    pub(crate) kind: FindKind,
    pub(crate) target: char,
}

/// State for an open [`Mode::RefFiles`]: the reference files (bible + notes) and
/// the selected row. The list is captured when the browser opens, so it's a
/// stable target for the arrow keys.
pub struct RefFiles {
    pub(crate) files: Vec<std::path::PathBuf>,
    pub(crate) selected: usize,
}

/// State for an open [`Mode::ProjectBrowser`] (#270): the recently-opened
/// projects and the selected row. Captured when the browser opens, so the list
/// is a stable target for the arrow keys.
pub struct ProjectBrowser {
    pub(crate) projects: Vec<crate::recent::RecentProject>,
    pub(crate) selected: usize,
}

/// State for the series tree navigator (#330): the selected visible row and the
/// set of expanded book indices. The visible rows are derived from the project
/// on demand (cheap, from cached metadata) rather than stored, so the tree never
/// goes stale against the manifest.
pub struct Tree {
    pub(crate) selected: usize,
    pub(crate) expanded: std::collections::HashSet<usize>,
}

/// State for an open [`Mode::DirBrowser`] (#271): a filesystem navigator for
/// finding a project that isn't in the recent list. Shows the subdirectories of
/// `cwd` (projects marked), with a `..` entry to go up.
pub struct DirBrowser {
    pub(crate) cwd: std::path::PathBuf,
    pub(crate) entries: Vec<DirEntry>,
    pub(crate) selected: usize,
}

/// One row in the [`DirBrowser`]: a navigable directory.
pub struct DirEntry {
    /// Display label (`..` for the parent, else the directory name).
    pub(crate) label: String,
    /// The directory this row navigates to (the parent for `..`).
    pub(crate) path: std::path::PathBuf,
    /// Whether this directory is a Quill project (holds a `book.toml`), so Enter
    /// opens it rather than descending into it.
    pub(crate) is_project: bool,
    /// Whether this is the `..` (go-up) row.
    pub(crate) is_parent: bool,
}

/// A step in the new-project setup wizard (#282), offered in turn after a
/// project is created. Each is individually skippable; the AI steps are skipped
/// outright when AI is off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WizardStep {
    /// Writing goals → the manifest (drives the progress report).
    Goals,
    /// Draft a starter cast from a concept (AI) → the character bible.
    Characters,
    /// Draft the setting from a concept (AI) → the setting bible.
    Setting,
    /// A free-form note → `notes.md`.
    Notes,
}

impl WizardStep {
    /// The step after this one, or `None` when the wizard is finished.
    pub(crate) fn next(self) -> Option<WizardStep> {
        match self {
            WizardStep::Goals => Some(WizardStep::Characters),
            WizardStep::Characters => Some(WizardStep::Setting),
            WizardStep::Setting => Some(WizardStep::Notes),
            WizardStep::Notes => None,
        }
    }

    /// Whether this step needs the AI backend (so it's skipped when AI is off).
    pub(crate) fn needs_ai(self) -> bool {
        matches!(self, WizardStep::Characters | WizardStep::Setting)
    }
}

/// State for an open [`Mode::Palette`]: the entries, the selected row, and which
/// group headings are collapsed. `selected` indexes the *visible* rows
/// ([`Palette::rows`]) — headers plus the items of expanded groups — not `items`,
/// so it tracks what's on screen as groups collapse (#514).
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

/// What a [`Mode::Prompt`] resolves to when confirmed. `at` is the chapter index
/// the action applies to.
#[derive(Clone, Copy)]
pub(crate) enum Pending {
    /// y/N: quit despite unsaved changes.
    Quit,
    /// y/N: delete the chapter at `at`.
    Delete { at: usize },
    /// Text input: title for a new chapter to insert at `at`.
    NewChapter { at: usize },
    /// Text input: new title for the chapter at `at`.
    Rename { at: usize },
    /// Text input: a question to ask the AI about the manuscript.
    AskAi,
    /// Text input: a word/phrase (and optional filters) for the thesaurus (#290).
    Thesaurus,
    /// Text input: a message for a version-control commit (#131).
    Commit,
    /// Confirm running an AI action whose estimated cost exceeds the threshold.
    /// `force` is whether to force a full bible rebuild (#176); `offer_local` adds
    /// "run locally instead" as a third choice when the hybrid router (#372) found
    /// a usable local backend (`y` Claude · `l` local · anything else cancels).
    ConfirmAi {
        action: AiAction,
        force: bool,
        offer_local: bool,
    },
    /// y/N: generate character portraits (#238) — each image is billed, so the
    /// action confirms before spending.
    ConfirmPortraits,
    /// y/N: generate a scene illustration of the active chapter (#239).
    ConfirmScene,
    /// y/N: restore the current chapter to its last committed version (#135).
    ConfirmRestore,
    /// Crash recovery (#263): unsaved work from a previous session was found.
    /// `y`/`r` restores it, `d` discards the sidecar, Esc keeps it for later.
    ConfirmRecovery,
    /// y/N: there are unsaved changes — save the current project and switch to
    /// the one chosen in the project browser (#270). The target waits in
    /// `App::pending_switch`.
    ConfirmSwitch,
    /// Text input: the path (location + name) for a new project to create from
    /// the project browser (#273). Created and opened on Enter.
    NewProject,
    /// Text input: the new folder name for the open project (#424). On Enter the
    /// project directory is renamed and reopened at the new path.
    RenameProject,
    /// y/N: offer the new-project setup wizard (#282). `y` starts it at the first
    /// step; anything else lands in the editor.
    WizardStart,
    /// Text input for one setup-wizard step (#282): blank skips the step, Esc
    /// dismisses the rest of the wizard.
    Wizard(WizardStep),
    /// Text input: a path to a Markdown notes file (or a folder of them) to
    /// import into the story bible (#283).
    ImportNotes,
    /// Text input: the folder of notes to start a new project from (#286).
    NewFromNotesSource,
    /// Text input: the path for the new project created from notes (#286).
    NewFromNotesDest,
    /// Text input: a Markdown manuscript file to import as a new project (#298).
    ImportManuscriptSource,
    /// Text input: the path for the new project created from a manuscript (#298).
    ImportManuscriptDest,
    /// Text input: how many minutes a writing sprint should run (#299).
    SprintDuration,
    /// Text input: the remote URL to back the project up to (#134).
    SetRemote,
    /// Text input: a Vim Ex command (`:w`, `:q`, `:wq`, …) typed after `:` (#207).
    ExCommand,
    /// Text input: a premise / idea to brainstorm story ideas from (#255).
    Brainstorm,
    /// Text input: a concept to draft a starter character cast from (#256).
    DraftCharacters,
    /// Text input: a concept to draft a starter setting bible from (#257).
    DraftSetting,
    /// Text input: a plain-language description of a workflow to generate (#232).
    DescribeWorkflow,
    /// Text input: confirm or edit the name for the workflow the AI proposed
    /// (#232); the steps wait in `App::pending_workflow`. Enter saves, Esc cancels.
    NameNewWorkflow,
    /// Text input: a plain-language description of a configuration change (#303).
    DescribeConfig,
    /// y/N: apply the configuration changes the AI proposed (#303); they wait in
    /// `App::pending_config`. `y` writes them, anything else cancels.
    ConfirmConfig,
    /// Text input: the note for an inline annotation (#302) anchored to the byte
    /// range `[start, end)` of the active chapter. Enter saves it, Esc cancels.
    Annotate { start: usize, end: usize },
    /// y/N: remove the downloaded local model with this id from disk (#382). The
    /// id is a catalog `&'static str`, so [`Pending`] stays `Copy`.
    ConfirmRemoveModel(&'static str),
    /// Text input editing a metadata field (chapter front matter or book
    /// manifest). One variant for every text-valued metadata field, so adding a
    /// field is a new [`MetaField`] case rather than new prompt control flow.
    SetMeta(MetaField),
}

/// Which metadata field a [`Pending::SetMeta`] prompt writes. Chapter fields
/// carry the chapter index; book fields are project-level.
#[derive(Clone, Copy)]
pub(crate) enum MetaField {
    Summary {
        at: usize,
    },
    Pov {
        at: usize,
    },
    /// The chapter's word-count goal (#146).
    Goal {
        at: usize,
    },
    /// The file's section role (#179): front/back-matter type, or blank for body.
    Role {
        at: usize,
    },
    ChapterEpigraph {
        at: usize,
    },
    ChapterEpigraphBy {
        at: usize,
    },
    BookTitle,
    Subtitle,
    Author,
    Series,
    BookEpigraph,
    BookEpigraphBy,
}

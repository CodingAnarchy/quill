//! Editor modes: the transient state that drives key handling. Fields the
//! handlers read or write are `pub(crate)`; `label`/`input` are `pub` because
//! the status bar and the edit banner render them.

use std::collections::HashMap;

use ratatui::text::Line;

use crate::buffer::FindKind;

mod action;
pub use action::*;

mod states;
pub use states::*;

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
    /// The pre-commit review (#704): the changed-file list and the diff of what a
    /// commit would capture. Unlike the read-only reports, this overlay confirms —
    /// Enter proceeds to the commit-message prompt, Esc (or any other key) cancels.
    CommitReview,
}

impl Overlay {
    /// A bordered report overlay labelled `label`, scrolled to the top.
    pub const fn report(label: &'static str, lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::Report(label),
            scroll: 0,
        }
    }

    /// The AI answer / summary overlay, scrolled to the top.
    pub const fn ai(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::AiPanel,
            scroll: 0,
        }
    }

    /// The user's-guide overlay (#281), scrolled to the top.
    pub const fn guide(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::Guide,
            scroll: 0,
        }
    }

    /// The pre-commit review overlay (#704), scrolled to the top.
    pub const fn commit_review(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            kind: OverlayKind::CommitReview,
            scroll: 0,
        }
    }

    /// Whether this overlay confirms an action (the pre-commit review, #704) rather
    /// than being a press-any-key-to-dismiss read-only surface — so the key handler
    /// knows Enter should proceed and any other key should cancel, not just close.
    pub const fn is_confirm(&self) -> bool {
        matches!(self.kind, OverlayKind::CommitReview)
    }

    /// The report label if this is a [`OverlayKind::Report`] overlay (e.g.
    /// "progress", "analysis") — the base name a saved snapshot (#601) is written
    /// under. `None` for the AI panel and the guide, which don't save to a file.
    pub const fn report_label(&self) -> Option<&'static str> {
        match self.kind {
            OverlayKind::Report(label) => Some(label),
            OverlayKind::AiPanel | OverlayKind::Guide | OverlayKind::CommitReview => None,
        }
    }

    /// The overlay's rendered rows as plain text, one line per row — the styled
    /// spans flattened to their content. This is what a saved report snapshot
    /// (#601) writes, so the file matches what's shown on screen.
    pub fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
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
    /// Browsing the commit history (#587): a navigable list of past commits where
    /// the selected one can be diffed against the current chapter, or the chapter
    /// restored to it (into the editable buffer for review-and-save). Boxed (carries
    /// the commit list).
    History(Box<History>),
    /// Reviewing the active chapter's inline annotations (#302): a navigable list
    /// where each can be jumped to or deleted. The items are read live from
    /// `App::annotations`; this carries only the selected row. `filter`/`typing`
    /// are the type-to-filter query and its input sub-mode (#822): `/` starts
    /// typing, keystrokes narrow the list to notes whose text or anchored snippet
    /// match, Esc leaves input (keeping the filter) and a second Esc clears it.
    Annotations {
        selected: usize,
        filter: String,
        typing: bool,
    },
    /// The focused heading outline (#657): the Ctrl-O outline made navigable.
    /// The outline pane normally just lists the active chapter's headings and
    /// highlights the section the cursor is in; focusing it moves a `selected`
    /// cursor over the headings (arrows / j-k) and Enter jumps the editor cursor
    /// to the selected heading's byte offset, then returns to editing (leaving
    /// the pane up). Read-only over the prose. `selected` indexes the active
    /// chapter's `headings()` in document order — the same list the pane renders
    /// and the corkboard jumps by. `filter`/`typing` are the type-to-filter query
    /// and its input sub-mode (#822): `/` starts typing, keystrokes narrow the
    /// pane to headings whose title matches (case-insensitive substring), Esc
    /// leaves input (keeping the filter) and a second Esc clears it.
    Outline {
        selected: usize,
        filter: String,
        typing: bool,
    },
    /// The read-only corkboard for the active chapter (#589, Phase 1): an
    /// index-card overview of the chapter's `##`/`###` scene headings — each card
    /// is a heading + its first line of prose. Navigable; Enter jumps the editor
    /// cursor to the selected scene and closes the view. The cards are captured
    /// when it opens (from the already-parsed headings), so it's a stable target
    /// for the arrow keys. Boxed (carries the card list). Phase 2 (per-scene
    /// metadata + reorder) is a documented follow-up on #589.
    Corkboard(Box<Corkboard>),
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
    /// (#165), or the `bible/`-only view (#590). Reached with `f` from the chapter
    /// browser (all reference files), or from the Ctrl-K "Browse story bible"
    /// run-action (bible only). Enter opens the selected file in an editable side
    /// pane. Boxed (carries the file list).
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
    /// Previewing a manuscript import before writing it (#702): the chapters the
    /// parser split out (titles + count), the heading level it split on, and any
    /// dropped-element warnings — shown so a mis-split is caught up front. Enter
    /// confirms (write + open), Esc cancels (writes nothing); for a Markdown source
    /// the split level can be toggled and the preview re-parsed. Boxed (carries the
    /// parsed chapters and the source text).
    ImportPreview(Box<ImportPreview>),
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
    /// The system-clipboard register `"+` / `"*` was chosen for the next put
    /// (#811): `p` / `P` then paste the OS clipboard instead of an internal
    /// register. Cleared when the command that uses it completes, like
    /// `selected_register`. (Yank / delete already mirror to the clipboard, so the
    /// flag only needs to redirect the *read* side.)
    pub(crate) clipboard_register: bool,
    /// The last `/` / `?` / `*` search, replayed by `n` / `N` (#206).
    pub(crate) last_search: Option<VimSearch>,
    /// Vim marks (#825): a mark letter → the position `m<x>` recorded, jumped back
    /// to by `` `<x> `` / `'<x>`. Marks are **global** — a position carries its
    /// chapter, so a mark set in one chapter jumps back there even after moving
    /// to another. Survives a trip through Insert (it lives on `App`, like the
    /// other Vim state).
    pub(crate) marks: HashMap<char, JumpPos>,
    /// The jump-back list (#825): the positions the big jumps (outline / corkboard
    /// / find / go-to-chapter, and `` `<x> ``) leave, newest last. `` `` `` /
    /// `''` pops it to return to where a jump started. Bounded (older entries drop)
    /// so it can't grow without limit on the nav path.
    pub(crate) jumplist: std::collections::VecDeque<JumpPos>,
}

/// A recorded editor position for a Vim mark or the jump-back list (#825): which
/// chapter (by its manifest index) and the byte offset within that chapter's
/// buffer. Storing the chapter makes marks and jumps **global** — returning to a
/// mark or popping the jumplist re-opens the right chapter first, then restores
/// the cursor. `Copy` (two `usize`s), so recording one is O(1) with no allocation
/// and no rope work.
#[derive(Clone, Copy)]
pub struct JumpPos {
    /// The chapter's manifest index at the time the position was recorded.
    pub(crate) chapter: usize,
    /// The cursor's byte offset within that chapter's buffer.
    pub(crate) byte: usize,
}

/// A recorded Vim search, so `n` / `N` can repeat it (#206).
#[derive(Clone)]
pub struct VimSearch {
    pub(crate) query: String,
    /// The search direction; `n` repeats it, `N` inverts it.
    pub(crate) backward: bool,
}

/// The Vim register store (#204): the unnamed register, written by every yank /
/// delete / change, and the named registers `"a`–`"z`. Numbered registers and
/// the system clipboard are out of scope.
#[derive(Default)]
pub struct Registers {
    pub(crate) unnamed: Option<Yank>,
    pub(crate) named: HashMap<char, Yank>,
}

/// A Vim Normal-mode command waiting on its next keystroke.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum VimPending {
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
    /// `z` seen — awaiting `=` for the spelling-suggestions pick-list (#647),
    /// Vim's own `z=` spell-suggest binding; any other second key cancels.
    Z,
    /// `m` / `` ` `` / `'` seen — awaiting the mark letter (#825). `set` is the
    /// `m` form (record a mark); otherwise a jump — `` `` `` / `''` back-jumps
    /// through the jumplist, and a letter jumps to that mark.
    Mark { set: bool },
}

/// A Vim operator (#203): delete leaves Normal mode, change enters Insert, yank
/// (#204) copies and stays put.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Delete,
    Change,
    Yank,
}

/// Text captured by a delete / change / yank, for putting back with `p` / `P`.
#[derive(Clone)]
pub struct Yank {
    pub(crate) text: String,
    /// Whole-line text (from `dd`/`cc`/`yy`/linewise motions), put on its own
    /// lines rather than inline.
    pub(crate) linewise: bool,
}

/// A recorded find-char motion, so `;` / `,` can replay it.
#[derive(Clone, Copy)]
pub struct Find {
    pub(crate) kind: FindKind,
    pub(crate) target: char,
}

/// State for an open [`Mode::RefFiles`]: the reference files (bible + notes) and
/// the selected row. The list is captured when the browser opens, so it's a
/// stable target for the arrow keys.
pub struct RefFiles {
    pub(crate) files: Vec<std::path::PathBuf>,
    pub(crate) selected: usize,
    /// Whether this is the `bible/`-only view (#590), opened from the Ctrl-K
    /// "Browse story bible" run-action rather than the chapter browser's `f`. It
    /// changes the header and where Esc returns to (the editor, not the chapter
    /// browser) — everything else, including the editable-pane open, is shared.
    pub(crate) bible_only: bool,
}

/// State for an open [`Mode::ProjectBrowser`] (#270): the recently-opened
/// projects and the selected row. Captured when the browser opens, so the list
/// is a stable target for the arrow keys. `filter` is the type-to-filter query
/// (#612), lifted from the chapter browser: when non-empty the list shows only
/// projects whose title or path matches (case-insensitive substring). `typing`
/// is the filter-input sub-mode — `/` enters it so keystrokes build the query
/// instead of running the browser's single-key commands; Esc leaves it (keeping
/// the filter) and a second Esc clears the filter.
pub struct ProjectBrowser {
    pub(crate) projects: Vec<crate::recent::RecentProject>,
    pub(crate) selected: usize,
    pub(crate) filter: String,
    pub(crate) typing: bool,
}

/// State for the series tree navigator (#330): the selected visible row and the
/// set of expanded book indices. The visible rows are derived from the project
/// on demand (cheap, from cached metadata) rather than stored, so the tree never
/// goes stale against the manifest.
pub struct Tree {
    pub(crate) selected: usize,
    pub(crate) expanded: std::collections::HashSet<usize>,
}

/// State for the corkboard (#589 Phase 1 / #636 Phase 2 / #701): scenes as index
/// cards, plus the selected row. Phase 1's read-only view is an editable planning
/// surface — the author reorders scenes and annotates each with a synopsis +
/// status. The cards are rebuilt (from the already-parsed headings and each
/// chapter's `scenes:` front matter) when the view opens and after every edit, so
/// the byte offsets stay a valid jump target and the metadata stays aligned with
/// the body headings.
///
/// `manuscript` (#701) distinguishes the two scopes. The default per-chapter board
/// (`false`) cards the active buffer's scenes; the manuscript-wide board (`true`)
/// cards every chapter's scenes, grouped by chapter — each card then names its
/// chapter so the render can insert group headers and jump/reorder can target the
/// right file.
pub struct Corkboard {
    pub(crate) cards: Vec<Card>,
    pub(crate) selected: usize,
    /// Whether this is the manuscript-wide board (#701) spanning every chapter,
    /// versus the per-chapter board over just the active buffer.
    pub(crate) manuscript: bool,
}

/// One corkboard index card (#589 / #636): a scene heading and the first line of
/// prose beneath it, plus the heading's byte offset in the active buffer (so
/// Enter can jump there) and the author's editable synopsis + status (Phase 2).
pub struct Card {
    /// The scene heading text (without markers).
    pub(crate) title: String,
    /// The heading's indent level (1–6), so nested scenes read as a hierarchy.
    pub(crate) level: u8,
    /// The first non-empty line of prose under the heading — the fallback shown
    /// when the author hasn't written a synopsis — or empty when the scene has no
    /// body yet.
    pub(crate) prose_line: String,
    /// The author's one-line synopsis (#636), from the `scenes:` front matter;
    /// empty when none is set (the card then falls back to `prose_line`).
    pub(crate) synopsis: String,
    /// The scene's drafting status (#636), shown as a badge and cycled from the
    /// board.
    pub(crate) status: crate::frontmatter::SceneStatus,
    /// Byte offset where the heading begins, within its chapter's body; Enter
    /// moves the cursor here (after opening the card's chapter, on the
    /// manuscript-wide board).
    pub(crate) start: usize,
    /// Which chapter this card's scene belongs to on the manuscript-wide board
    /// (#701) — the index the jump opens and reorder/status/synopsis edits target.
    /// `None` on the per-chapter board, whose cards all belong to the active
    /// buffer. Also groups the manuscript board's cards under chapter headers.
    pub(crate) chapter: Option<usize>,
    /// The card's chapter title, shown as the group header before the chapter's
    /// first card on the manuscript-wide board (#701). Empty on the per-chapter
    /// board, which needs no grouping.
    pub(crate) chapter_title: String,
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
pub enum WizardStep {
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
    pub(crate) const fn next(self) -> Option<Self> {
        match self {
            Self::Goals => Some(Self::Characters),
            Self::Characters => Some(Self::Setting),
            Self::Setting => Some(Self::Notes),
            Self::Notes => None,
        }
    }

    /// Whether this step needs the AI backend (so it's skipped when AI is off).
    pub(crate) const fn needs_ai(self) -> bool {
        matches!(self, Self::Characters | Self::Setting)
    }
}

/// What a [`Mode::Prompt`] resolves to when confirmed. `at` is the chapter index
/// the action applies to.
#[derive(Clone, Copy)]
pub enum Pending {
    /// y/N: quit despite unsaved changes.
    Quit,
    /// y/N: delete the chapter at `at`.
    Delete { at: usize },
    /// y/N: merge the chapter at `at` into the one above it (#585).
    Merge { at: usize },
    /// Text input: title for the new tail chapter when splitting the active
    /// chapter at the cursor (#585); the chapter being split is at `at`.
    SplitChapter { at: usize },
    /// Text input: title for the copy when duplicating the chapter at `at` (#585).
    Duplicate { at: usize },
    /// Text input: a chapter to jump to by number (1-based) or by a title
    /// substring (#588) — a direct go-to that skips scrolling the browser. A
    /// number selects that chapter; otherwise the first chapter whose title
    /// matches (case-insensitive) is opened.
    GoToChapter,
    /// Text input: title for a new chapter to insert at `at`.
    NewChapter { at: usize },
    /// Text input: new title for the chapter at `at`.
    Rename { at: usize },
    /// Text input: a question to ask the AI about the manuscript.
    AskAi,
    /// Text input: a word/phrase (and optional filters) for the thesaurus (#290).
    Thesaurus,
    /// Text input: constraints (origin, syllables, tone, era) for the name
    /// generator (#610). The `Kind` (character / place / term) was chosen from the
    /// picker before the prompt opened.
    NameGen(crate::ai::namegen::Kind),
    /// Text input: a message for a version-control commit (#131).
    Commit,
    /// Text input: the name for a manuscript milestone — an annotated tag over the
    /// whole project (#833). On Enter the milestone is created on the current HEAD.
    Milestone,
    /// Confirm running an AI action whose estimated cost exceeds the threshold.
    /// `force` is whether to force a full bible rebuild (#176); `offer_local` adds
    /// "run locally instead" as a third choice when the hybrid router (#372) found
    /// a usable local backend (`y` Claude · `l` local · anything else cancels).
    ConfirmAi {
        action: AiAction,
        force: bool,
        offer_local: bool,
    },
    /// Confirm the all-bibles refresh (#655) — one combined cost confirm gating
    /// the four bible passes over every book. `force` rebuilds each bible from the
    /// whole manuscript (#176). `y` runs the batch, anything else cancels.
    ConfirmRefreshBibles { force: bool },
    /// y/N: generate character portraits (#238) — each image is billed, so the
    /// action confirms before spending.
    ConfirmPortraits,
    /// y/N: generate a scene illustration of the active chapter (#239).
    ConfirmScene,
    /// y/N: restore the current chapter to its last committed version (#135).
    ConfirmRestore,
    /// y/N: restore the *whole project* to a revision (#833) — a milestone or a
    /// past commit chosen in the history view, or HEAD from the palette. Destructive
    /// to uncommitted changes across the manuscript, so it always asks first. The
    /// target revision waits in `App::pending_restore_rev` (kept off the `Copy`
    /// `Pending` enum), taken when the restore runs.
    ConfirmRestoreProject,
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
    /// Text input: a new folder name for a project chosen in the browser (#612).
    /// The target project's path waits in `App::pending_manage`; on Enter its
    /// directory is renamed (moving the folder, updating recents) without opening
    /// it. Distinct from [`Pending::RenameProject`], which renames the open one.
    RenameBrowserProject,
    /// y/N: delete the project chosen in the browser (#612), removing its folder
    /// from disk. The target's path waits in `App::pending_manage`; `y` removes
    /// it and drops it from the list, anything else cancels. Guarded — the
    /// currently-open project is refused before the prompt ever opens.
    ConfirmDeleteProject,
    /// Text input: the series title when converting a standalone novel into a
    /// multi-book series (#648). The current book becomes book one; a blank entry
    /// uses the book's own title as the series name. On Enter the `[[books]]`
    /// manifest is written and the add-a-book prompt chains after it.
    ConvertToSeries,
    /// Text input: the title for a new book appended to the series (#648). On Enter
    /// the book is created with a starter chapter and made active. Blank re-prompts.
    AddBook,
    /// y/N: remove book `index` from the series (#648), retiring its chapters to
    /// `unused/`. `y` removes it (and re-activates a neighbour if it was active);
    /// anything else cancels. Guarded — a series always keeps at least one book.
    ConfirmRemoveBook { index: usize },
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
    /// Text input: the destination path for a project archive (#594). On Enter a
    /// timestamped `.zip` of the whole project is written there.
    ArchiveProjectDest,
    /// Text input: the `.zip` archive to restore a project from (#594). On Enter
    /// the archive is validated and the new-project destination is asked for.
    RestoreArchiveSource,
    /// Text input: the folder to unpack a restored archive into (#594). On Enter
    /// the archive is unpacked there and the new project is opened.
    RestoreArchiveDest,
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
    /// Text input: the one-line synopsis for a scene on the corkboard (#636 / #701).
    /// The target is resolved when the prompt opens — `chapter` is the card's
    /// chapter on the manuscript-wide board (`None` on the per-chapter board), and
    /// `scene` is its within-chapter scene index — so the write lands on the right
    /// file even after the mode has left `Corkboard` for the prompt. `manuscript`
    /// records which board to re-open. Enter persists it to the chapter's `scenes:`
    /// front matter (a blank entry clears it) and re-opens the board; Esc returns
    /// without a change.
    SetSceneSynopsis {
        chapter: Option<usize>,
        scene: usize,
        manuscript: bool,
    },
    /// y/N: remove the downloaded local model with this id from disk (#382). The
    /// id is a catalog `&'static str`, so [`Pending`] stays `Copy`.
    ConfirmRemoveModel(&'static str),
    /// Text input editing a metadata field (chapter front matter or book
    /// manifest). One variant for every text-valued metadata field, so adding a
    /// field is a new [`MetaField`] case rather than new prompt control flow.
    SetMeta(MetaField),
    /// Numeric input setting one project goal (#550): target words, target
    /// chapters, the daily target, or words-per-page. Persisted to the `[goals]`
    /// table of `book.toml`. The goal prompts chain (`goals:set` walks the
    /// required fields then offers the optional daily target); a blank entry
    /// clears the optional daily goal and is rejected for the required ones.
    SetGoal(crate::project::GoalField),
    /// Text input setting one book-details field (#571): title, subtitle, author,
    /// or series. Persisted to the manifest through the shared metadata setter, so
    /// it reads back identical to the chapter-browser single-letter keys. The
    /// prompts chain (`book:details` walks title → subtitle → author → series);
    /// Esc stops the chain. The required title keeps its current value on blank;
    /// the optional fields clear on blank.
    SetBookField(BookField),
    /// Text input setting one free-text `[style]` knob (#569): the scene-break
    /// glyph or a title/part ornament. Persisted to the `[style]` table of
    /// `book.toml`; a blank entry clears the key. On save the setter re-opens the
    /// style menu so more knobs can be tweaked in one sitting.
    SetStyleText(StyleTextKnob),
    /// Text input setting one numeric / list Settings knob (#592): the column
    /// width, the autosave interval (both `[editor]`, validated + spliced via the
    /// shared config machinery), or the analysis stopwords (`[analysis]
    /// stopwords_add` in `book.toml`). On save the Settings menu re-opens so more
    /// knobs can be tweaked in one sitting; the `[editor]` knobs apply live.
    SetSettingText(SettingTextKnob),
    /// Text input: an API key to store in the OS keychain (#530), for the given
    /// provider. On Enter the key is written to the keychain (never the config
    /// file); a blank entry cancels. The provider determines the keychain entry
    /// and which plaintext field, if any, is then offered for removal.
    SetApiKey(crate::ai::keychain::Key),
    /// y/N: a plaintext `api_key` is now redundant (the key is in the keychain) —
    /// strip it from the config file (#530). `y` removes it; anything else leaves
    /// it (where it stays the lowest-precedence fallback).
    StripPlaintextKey(crate::ai::keychain::Key),
    /// y/N: AI is off — enable it in-app (#602). `y` writes `[ai] enabled = true`
    /// via the config splicer, applies it live, and chains into the Set-key flow
    /// (Claude with no key needs one); anything else leaves AI off. `resume` is the
    /// AI action the author was reaching for, re-run once AI is enabled and keyed,
    /// or `None` for the plain "Enable AI assistance" action.
    ConfirmEnableAi { resume: Option<AiAction> },
}

/// Which metadata field a [`Pending::SetMeta`] prompt writes. Chapter fields
/// carry the chapter index; book fields are project-level.
#[derive(Clone, Copy)]
pub enum MetaField {
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
    /// Publishing metadata (#583): ISBN, publisher, publication date, language tag,
    /// and the rights/copyright line — all optional, clearing on blank.
    Isbn,
    Publisher,
    PubDate,
    Language,
    Rights,
}

impl MetaField {
    /// Whether this field is an epigraph — the quotation or its attribution, at
    /// chapter or book level. These are display prose that ships in the export, so
    /// they get the same smart-typography pass as the body (#519); other metadata
    /// (titles, POV, summaries, goals) is stored verbatim.
    pub(crate) const fn is_epigraph(self) -> bool {
        matches!(
            self,
            Self::ChapterEpigraph { .. }
                | Self::ChapterEpigraphBy { .. }
                | Self::BookEpigraph
                | Self::BookEpigraphBy
        )
    }
}

/// One book-level field the "Book details…" run-action (#571) walks in turn —
/// title, subtitle, author, series. Each maps to the [`MetaField`] the manifest
/// stores it under, so the chained-prompt flow and the chapter-browser
/// single-letter keys persist (and read back) through the one shared setter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BookField {
    Title,
    Subtitle,
    Author,
    Series,
    /// Publishing metadata (#583), appended to the chained flow after the title
    /// fields: ISBN, publisher, publication date, language, and the rights line.
    Isbn,
    Publisher,
    PubDate,
    Language,
    Rights,
}

impl BookField {
    /// The [`MetaField`] this book field is stored as — the single mapping that
    /// keeps the run-action flow and the single-letter keys writing to the same
    /// manifest slot.
    pub(crate) const fn meta(self) -> MetaField {
        match self {
            Self::Title => MetaField::BookTitle,
            Self::Subtitle => MetaField::Subtitle,
            Self::Author => MetaField::Author,
            Self::Series => MetaField::Series,
            Self::Isbn => MetaField::Isbn,
            Self::Publisher => MetaField::Publisher,
            Self::PubDate => MetaField::PubDate,
            Self::Language => MetaField::Language,
            Self::Rights => MetaField::Rights,
        }
    }

    /// The prompt label for this field (matching the chapter-browser key labels).
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Title => "Book title",
            Self::Subtitle => "Subtitle",
            Self::Author => "Author",
            Self::Series => "Series",
            Self::Isbn => "ISBN",
            Self::Publisher => "Publisher",
            Self::PubDate => "Publication date (YYYY or YYYY-MM-DD)",
            Self::Language => "Language (BCP-47 tag, e.g. en, fr)",
            Self::Rights => "Rights / copyright line",
        }
    }
}

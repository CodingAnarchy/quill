//! Editor modes: the transient state that drives key handling. Fields the
//! handlers read or write are `pub(crate)`; `label`/`input` are `pub` because
//! the status bar and the edit banner render them.
//!
//! The [`Mode`] enum lives here; the state each variant carries is split by
//! concern into cohesive submodules (kept under the ~1,000-line convention) and
//! re-exported so callers (`crate::mode::…`) are unaffected: the
//! action/command/palette layer ([`action`]), the per-overlay list-state structs
//! ([`states`]), the read-only [`Overlay`] and annotation-display cycles
//! ([`overlay`]), the navigator/browser state ([`nav`]), the Vim Normal-mode
//! state ([`vim`]), and the prompt-resolution / metadata-field selectors
//! ([`pending`]).

mod action;
pub use action::*;

mod states;
pub use states::*;

mod overlay;
pub use overlay::*;

mod nav;
pub use nav::*;

mod vim;
pub use vim::*;

mod pending;
pub use pending::*;

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
    /// The manuscript-health dashboard (#827): open findings grouped by kind/pass
    /// then chapter, each group headed by its count, with the individual findings
    /// jumpable (Enter opens the chapter). A read of the standing triage state,
    /// read-only over the prose. Boxed (carries the grouped rows).
    Dashboard(Box<Dashboard>),
    /// The motif / character co-occurrence report (#922): co-occurring pairs and
    /// notable absences drawn from the character and theme bibles' chapter-
    /// appearance data, each jumpable to its relevant chapter. A combinatorial read
    /// of the standing bible data, read-only over the prose. Boxed (carries the
    /// row list).
    CoOccurrence(Box<CoOccurrence>),
    /// Browsing the commit history (#587): a navigable list of past commits where
    /// the selected one can be diffed against the current chapter, or the chapter
    /// restored to it (into the editable buffer for review-and-save). Boxed (carries
    /// the commit list).
    History(Box<History>),
    /// Reviewing the active chapter's inline annotations (#302): a navigable list
    /// where each can be jumped to, replied to (#889), resolved/reopened (#889), or
    /// deleted. The items are read live from `App::annotations`; this carries only
    /// the selected row. `filter`/`typing` are the type-to-filter query and its
    /// input sub-mode (#822): `/` starts typing, keystrokes narrow the list to notes
    /// whose text or anchored snippet match, Esc leaves input (keeping the filter)
    /// and a second Esc clears it. `show` (#889) is the resolved/open state filter,
    /// cycled with `f`, applied on top of the text filter.
    ///
    /// `sort` and `reviewer` are the multi-reviewer triage state (#933): `sort`
    /// (cycled with `s`) orders the visible notes by position / status / reviewer;
    /// `reviewer` (cycled with `R`) is the per-reviewer filter (`None` shows every
    /// reviewer), applied alongside the text and status filters. `[`/`]` jump to the
    /// previous / next open thread. These are additive over the existing state — the
    /// filter/resolve behavior is unchanged when they're at their defaults.
    Annotations {
        selected: usize,
        filter: String,
        typing: bool,
        show: AnnoShow,
        sort: AnnoSort,
        reviewer: Option<String>,
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
    /// Previewing a review-comment import before merging it (#886): the reviewer's
    /// Word comments recovered from a marked-up `.docx`, resolved against the active
    /// chapter — which matched the prose (and will become annotations) and which
    /// didn't (the prose drifted, so they're reported rather than lost). Enter
    /// merges the matches into the chapter's annotations, Esc cancels. Boxed
    /// (carries the resolved comments and dropped list).
    ReviewImport(Box<ReviewImport>),
    /// Browsing the project's local backups (#934): the timestamped `.zip` archives
    /// #888 wrote, newest first, each with its size and date. Navigable; Enter
    /// restores the selected backup over the open project (behind a confirm), Esc
    /// closes. Read-only over the working project until a restore is confirmed. Boxed
    /// (carries the backup list).
    BackupBrowser(Box<BackupBrowser>),
    /// The fuzzy chapter/scene quick-switcher (#976): a `Ctrl-P`-style overlay
    /// listing every chapter and every scene heading, narrowed live by a typed
    /// substring; Enter jumps to the highlighted target (reusing the go-to +
    /// jumplist paths). The target index is built once when the switcher opens
    /// (cached metadata + a one-time headings read per chapter), so filtering as
    /// you type reads no files and stays fast at series scale. Boxed (carries the
    /// full target list).
    QuickSwitch(Box<QuickSwitch>),
}

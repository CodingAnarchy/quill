//! Navigator / browser mode state: the reference-file, project, series-tree,
//! corkboard, and directory-browser structs each carried by their
//! [`Mode`](super::Mode) variant. Split out of `mode.rs` to keep the core enum
//! under the ~1,000-line convention; re-exported from there, so callers
//! (`crate::mode::{Tree, …}`) are unaffected. Pure move.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::frontmatter::SceneStatus;
use crate::mode::ListFilter;
use crate::recent::RecentProject;

/// State for an open [`Mode::RefFiles`](super::Mode): the reference files (bible +
/// notes) and the selected row. The list is captured when the browser opens, so
/// it's a stable target for the arrow keys.
pub struct RefFiles {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) selected: usize,
    /// Whether this is the `bible/`-only view (#590), opened from the Ctrl-K
    /// "Browse story bible" run-action rather than the chapter browser's `f`. It
    /// changes the header and where Esc returns to (the editor, not the chapter
    /// browser) — everything else, including the editable-pane open, is shared.
    pub(crate) bible_only: bool,
}

/// State for an open [`Mode::ProjectBrowser`](super::Mode) (#270): the
/// recently-opened projects and the selected row. Captured when the browser opens,
/// so the list is a stable target for the arrow keys. `filter` is the
/// type-to-filter query (#612), lifted from the chapter browser: when non-empty the
/// list shows only projects whose title or path matches (case-insensitive
/// substring). `filter.typing` is the filter-input sub-mode — `/` enters it so
/// keystrokes build the query instead of running the browser's single-key
/// commands; Esc leaves it (keeping the filter) and a second Esc clears the filter.
pub struct ProjectBrowser {
    pub(crate) projects: Vec<RecentProject>,
    pub(crate) selected: usize,
    pub(crate) filter: ListFilter,
}

/// State for the series tree navigator (#330): the selected visible row and the
/// set of expanded book indices. The visible rows are derived from the project
/// on demand (cheap, from cached metadata) rather than stored, so the tree never
/// goes stale against the manifest.
pub struct Tree {
    pub(crate) selected: usize,
    pub(crate) expanded: HashSet<usize>,
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
    pub(crate) status: SceneStatus,
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

/// State for an open [`Mode::DirBrowser`](super::Mode) (#271): a filesystem
/// navigator for finding a project that isn't in the recent list. Shows the
/// subdirectories of `cwd` (projects marked), with a `..` entry to go up.
pub struct DirBrowser {
    pub(crate) cwd: PathBuf,
    pub(crate) entries: Vec<DirEntry>,
    pub(crate) selected: usize,
}

/// One row in the [`DirBrowser`]: a navigable directory.
pub struct DirEntry {
    /// Display label (`..` for the parent, else the directory name).
    pub(crate) label: String,
    /// The directory this row navigates to (the parent for `..`).
    pub(crate) path: PathBuf,
    /// Whether this directory is a Quill project (holds a `book.toml`), so Enter
    /// opens it rather than descending into it.
    pub(crate) is_project: bool,
    /// Whether this is the `..` (go-up) row.
    pub(crate) is_parent: bool,
}

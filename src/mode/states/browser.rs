//! Backup-browser, quick-switcher, and drafts mode state (#934/#976/#965): the
//! local-backup archive list, the fuzzy chapter/scene switcher index, and the
//! branch-as-draft menu. Split out of `states.rs` (pure move).

/// State for an open [`Mode::BackupBrowser`] — the local-backup browser (#934):
/// the project's timestamped `.zip` archives (newest first, each with its size and
/// date), and the selected row. Enter restores the selected backup over the open
/// project (behind a confirm); Esc closes. The list is captured when the browser
/// opens, so it's a stable target for the arrow keys. Read-only over the working
/// project until a restore is confirmed.
pub struct BackupBrowser {
    pub(crate) entries: Vec<crate::app::BackupEntry>,
    pub(crate) selected: usize,
}

/// One jump target in the quick-switcher (#976): a chapter, or a scene heading
/// within a chapter. `chapter` is the manifest index the jump opens; `byte` is
/// the offset within that chapter's body the cursor lands on (0 for a chapter —
/// its top). `title` is the display label; `haystack` is the pre-lowercased text
/// the fuzzy filter matches against (built once when the switcher opens, so
/// per-keystroke filtering never re-lowercases or reads a file). `is_scene`
/// distinguishes a scene heading from the chapter itself, so the row can mark it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickTarget {
    /// The chapter (manifest index) this target jumps to.
    pub(crate) chapter: usize,
    /// The byte offset within the chapter's body to place the cursor (0 = the
    /// chapter's top; a scene heading's start otherwise).
    pub(crate) byte: usize,
    /// The label shown in the list (the chapter or heading title).
    pub(crate) title: String,
    /// The chapter's title, shown dimmed as context on a scene row (empty on a
    /// chapter row, which is already the chapter).
    pub(crate) chapter_title: String,
    /// Pre-lowercased match text — the title, plus the chapter title for a scene —
    /// so the per-keystroke filter is a plain substring scan with no allocation.
    pub(crate) haystack: String,
    /// Whether this is a scene heading (vs. the chapter itself).
    pub(crate) is_scene: bool,
}

/// State for an open [`Mode::QuickSwitch`] — the fuzzy chapter/scene quick-switcher
/// (#976): a `Ctrl-P`-style overlay listing every chapter *and* every scene
/// heading, narrowed live by a typed substring; Enter jumps to the highlighted
/// target (reusing the go-to + jumplist paths). `targets` is the full index, built
/// **once** when the switcher opens from cached metadata + a one-time read of each
/// chapter's headings — so per-keystroke filtering reads no files and is
/// independent of project size. `query` is what's typed; `selected` indexes into
/// the *filtered* view ([`matches`](Self::matches)).
pub struct QuickSwitch {
    /// Every jump target (chapters + scene headings), in project order. Immutable
    /// once built; the filter derives the visible subset without mutating it.
    pub(crate) targets: Vec<QuickTarget>,
    /// The typed substring; empty shows every target.
    pub query: String,
    /// The selected row *within the filtered view* (0-based).
    pub(crate) selected: usize,
}

impl QuickSwitch {
    /// A fresh switcher over `targets`, no query typed, first row selected.
    pub(crate) const fn new(targets: Vec<QuickTarget>) -> Self {
        Self {
            targets,
            query: String::new(),
            selected: 0,
        }
    }

    /// The targets passing the current query, in project order, each with its
    /// index into `targets` (#976). A case-insensitive substring match over the
    /// pre-lowercased `haystack`; an empty query matches everything. Pure over the
    /// in-memory index — no file reads — so it's independent of project size once
    /// the switcher is open, which the perf guard checks.
    pub(crate) fn matches(&self) -> Vec<(usize, &QuickTarget)> {
        let q = self.query.trim().to_lowercase();
        self.targets
            .iter()
            .enumerate()
            .filter(|(_, t)| q.is_empty() || t.haystack.contains(&q))
            .collect()
    }

    /// The target the current selection resolves to, or `None` when the query
    /// filters everything out. Clamped so a shrinking match list can't point past
    /// the end.
    pub(crate) fn current(&self) -> Option<&QuickTarget> {
        let matches = self.matches();
        matches
            .get(self.selected.min(matches.len().saturating_sub(1)))
            .map(|(_, t)| *t)
    }

    /// Move the selection by `delta` rows within the filtered view, clamped to its
    /// bounds (#976). A no-op when nothing matches.
    pub(crate) fn step(&mut self, delta: isize) {
        let len = self.matches().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let last = len - 1;
        let next = (self.selected.min(last) as isize + delta).clamp(0, last as isize);
        self.selected = next as usize;
    }

    /// Append a typed character to the query and reset the selection to the top of
    /// the freshly-narrowed list (#976).
    pub(crate) fn push(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }

    /// Delete the last query character and reset the selection to the top (#976).
    pub(crate) fn pop(&mut self) {
        self.query.pop();
        self.selected = 0;
    }
}

/// State for an open [`Mode::Drafts`] — the branch-as-draft overlay (#965). Lists
/// the project's drafts (local branches), one row each, with the current one
/// marked. Enter switches to the selected draft (the working copy follows), `n`
/// creates a new draft from the current state, `m` merges the selected draft into
/// the current one (a clean merge lands a commit; conflicts route to the #963
/// resolution UI), Esc closes. `drafts` and `current` are captured when the overlay
/// opens, so the list is a stable target for the arrow keys; `selected` indexes into
/// `drafts`.
pub struct DraftsMenu {
    /// Every draft (local branch), sorted, captured when the overlay opened.
    pub(crate) drafts: Vec<String>,
    /// The name of the draft currently checked out — marked in the list and the
    /// target a merge folds a selected draft into.
    pub(crate) current: String,
    /// The selected row (index into `drafts`).
    pub(crate) selected: usize,
}

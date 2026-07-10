//! Commit-history view state (#587/#822/#833): the navigable list of commits and
//! milestones and its type-to-filter. Split out of `states.rs` (pure move).

/// State for the navigable commit-history view (#587): the commits (newest first)
/// and the selected row. From the selected row the author diffs the current chapter
/// against it or restores that chapter to it (`d`/`r`), or — since #833 — diffs or
/// restores the *whole project* against it (`D`/`R`). The list is captured when the
/// view opens, so it's a stable target for the arrow keys. `filter`/`typing` are the
/// type-to-filter query and its input sub-mode (#822): `/` starts typing, keystrokes
/// narrow the list to rows whose summary, id, or date match, Esc leaves input
/// (keeping the filter) and a second Esc clears it.
///
/// `milestones` are the named annotated tags (#833), listed above the commits; the
/// `selected` index spans the combined list ([`History::row`] resolves it), so the
/// project-scoped actions can address a milestone or any commit uniformly.
pub struct History {
    pub(crate) commits: Vec<crate::vcs::CommitMeta>,
    pub(crate) milestones: Vec<crate::vcs::TagMeta>,
    pub(crate) selected: usize,
    pub(crate) filter: String,
    pub(crate) typing: bool,
}

/// One row of the history view (#833): a named milestone or a commit. Milestones
/// lead the list; a chosen row addresses one revision for the diff/restore actions,
/// whichever kind it is.
pub enum HistoryRow<'a> {
    Milestone(&'a crate::vcs::TagMeta),
    Commit(&'a crate::vcs::CommitMeta),
}

impl<'a> HistoryRow<'a> {
    /// The full object id this row addresses — the handle the diff/restore actions
    /// resolve.
    pub(crate) fn rev(&self) -> &'a str {
        match self {
            Self::Milestone(t) => &t.rev,
            Self::Commit(c) => &c.rev,
        }
    }

    /// Whether this row passes the (already-lowercased) history filter (#822): a
    /// case-insensitive substring match on the text the row shows.
    pub(crate) fn matches(&self, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        match self {
            Self::Milestone(t) => {
                t.name.to_lowercase().contains(q)
                    || t.message.to_lowercase().contains(q)
                    || t.when.to_lowercase().contains(q)
            }
            Self::Commit(c) => {
                c.summary.to_lowercase().contains(q)
                    || c.id.to_lowercase().contains(q)
                    || c.when.to_lowercase().contains(q)
            }
        }
    }
}

impl History {
    /// The combined history rows in display order (#833): every milestone, then
    /// every commit. The single sequence the `selected` index, the renderer, and
    /// the filter/selection helpers all iterate, so a milestone and a commit are
    /// addressed the same way.
    pub(crate) fn rows(&self) -> Vec<HistoryRow<'_>> {
        self.milestones
            .iter()
            .map(HistoryRow::Milestone)
            .chain(self.commits.iter().map(HistoryRow::Commit))
            .collect()
    }

    /// The row at `index` in the combined list, or `None` past the end.
    pub(crate) fn row(&self, index: usize) -> Option<HistoryRow<'_>> {
        self.rows().into_iter().nth(index)
    }

    /// The number of rows in the combined list — milestones plus commits.
    pub(crate) const fn len(&self) -> usize {
        self.milestones.len() + self.commits.len()
    }
}

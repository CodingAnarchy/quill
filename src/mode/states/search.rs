//! Find / replace mode state (#144/#295/#570/#977/#1014): the single-chapter
//! search and the manuscript-wide project search, their stages, matches, and the
//! grouped / preview rows. Split out of `states.rs` (pure move).

use std::cell::RefCell;
use std::rc::Rc;

use crate::text::MatchOpts;

/// State for an open [`Mode::Search`] — find or find-and-replace (#144), scoped
/// to the single active chapter. `query`/`replacement` are public for the status
/// bar; the match list and indices are the handler's business.
pub struct Search {
    /// What the user is doing right now.
    pub(crate) stage: SearchStage,
    /// The text being searched for.
    pub query: String,
    /// The replacement text (replace flow only).
    pub replacement: String,
    /// Byte ranges of the current matches in the active buffer, in document
    /// order. Recomputed whenever the query or the document changes.
    pub(crate) matches: Vec<(usize, usize)>,
    /// Index into `matches` of the current (strongly highlighted) match.
    pub(crate) current: usize,
    /// Where the search started, so incremental typing jumps to the match
    /// nearest the original cursor rather than chasing each partial query.
    pub(crate) origin_byte: usize,
    /// How many replacements have been made, reported when the flow ends.
    pub(crate) replaced: usize,
    /// Vim `?` search — match nearest *before* the origin, and `n`/`N` invert
    /// (#206). `false` for forward (`/`) and the Ctrl-F find.
    pub(crate) backward: bool,
    /// Opened by Vim's `/` or `?` (#206): Enter finalizes back to Normal and
    /// records the search for `n`/`N`, rather than entering the find/replace
    /// navigation. Esc restores the cursor to the origin.
    pub(crate) from_vim: bool,
    /// Case-sensitive / whole-word toggles (#570). Default (both off) is the
    /// historic case-insensitive substring rule, byte-for-byte unchanged.
    pub(crate) opts: MatchOpts,
}

impl Search {
    /// A fresh search at the query stage, anchored at `origin_byte`. `backward` /
    /// `from_vim` distinguish the Vim `/`/`?` search (#206) from the Ctrl-F find.
    pub(crate) fn new(origin_byte: usize, backward: bool, from_vim: bool) -> Self {
        Self {
            stage: SearchStage::Query,
            query: String::new(),
            replacement: String::new(),
            matches: Vec::new(),
            current: 0,
            origin_byte,
            replaced: 0,
            backward,
            from_vim,
            opts: MatchOpts::default(),
        }
    }

    /// The current match range, if any.
    pub(crate) fn current_match(&self) -> Option<(usize, usize)> {
        self.matches.get(self.current).copied()
    }
}

/// State for an open [`Mode::ProjectSearch`] — manuscript-wide find & replace
/// (#295). `query`/`replacement` are public for the overlay; the match list and
/// indices are the handler's business. Matches are byte ranges into each
/// chapter's prose **body** (front matter excluded), collected on demand without
/// holding every chapter's text at once.
pub struct ProjectSearch {
    pub(crate) stage: ProjectSearchStage,
    /// The text being searched for, across every chapter.
    pub query: String,
    /// The replacement text (replace flow only).
    pub replacement: String,
    /// Every match across the manuscript, in chapter then document order.
    pub(crate) matches: Vec<ProjectMatch>,
    /// Index into `matches` of the current (highlighted) row.
    pub(crate) selected: usize,
    /// Index into [`grouped`](Self::grouped) of the highlighted chapter row in
    /// the grouped results pane (#977). Kept separate from `selected` so the
    /// flat list and the grouped overview each remember their own cursor.
    pub(crate) group_selected: usize,
    /// How many occurrences have been replaced, reported when the flow ends.
    pub(crate) replaced: usize,
    /// Case-sensitive / whole-word toggles (#570), same default as [`Search`].
    pub(crate) opts: MatchOpts,
    /// The chapter grouping (#977) memoized against `matches.len()`, so the
    /// Grouped/Preview overlays — which call [`grouped`](Self::grouped) (directly
    /// and via `chapter_span`/`preview_rows`) several times per frame — rebuild the
    /// groups and clone each chapter title only when the matches actually change,
    /// not on every draw. `matches` is only ever replaced wholesale (a new search)
    /// or shrunk one entry at a time (a step-replace), both of which change the
    /// length, so it's a sound invalidation key.
    groups: RefCell<Option<(usize, Rc<Vec<ChapterGroup>>)>>,
}

impl ProjectSearch {
    pub(crate) fn new() -> Self {
        Self {
            stage: ProjectSearchStage::Query,
            query: String::new(),
            replacement: String::new(),
            matches: Vec::new(),
            selected: 0,
            group_selected: 0,
            replaced: 0,
            opts: MatchOpts::default(),
            groups: RefCell::new(None),
        }
    }

    /// The number of distinct chapters the current matches span.
    pub(crate) fn chapter_span(&self) -> usize {
        self.grouped().len()
    }

    /// The replace-all preview (#1014): one row per affected chapter, each with
    /// its match count and a context snippet (the first match's line), in chapter
    /// order. Built from the same `matches` the sweep will rewrite, so the preview
    /// reflects exactly what the confirmed replace applies — nothing more, nothing
    /// less. The active chapter's live edits were flushed before `matches` was
    /// collected, so the snippets are drawn from the same bytes the sweep sees.
    pub(crate) fn preview_rows(&self) -> Vec<PreviewRow> {
        self.grouped()
            .iter()
            .map(|g| PreviewRow {
                context: self
                    .matches
                    .get(g.first_match)
                    .map(|m| m.context.clone())
                    .unwrap_or_default(),
                title: g.title.clone(),
                count: g.count,
            })
            .collect()
    }

    /// The current matches grouped by chapter (#977), in chapter order, each
    /// with its per-chapter count and the index of its first match (the jump
    /// target). Matches are already collected in chapter-then-document order, so
    /// each chapter's matches are one contiguous run.
    ///
    /// Memoized against `matches.len()` (#977): the Grouped/Preview overlays call
    /// this several times per frame, so the groups are built (and each title
    /// cloned) once per change to `matches`, then shared via `Rc` on later calls.
    pub(crate) fn grouped(&self) -> Rc<Vec<ChapterGroup>> {
        let key = self.matches.len();
        let mut cache = self.groups.borrow_mut();
        if let Some((cached_key, groups)) = cache.as_ref() {
            if *cached_key == key {
                return Rc::clone(groups);
            }
        }
        let groups = Rc::new(self.build_groups());
        *cache = Some((key, Rc::clone(&groups)));
        groups
    }

    /// Build the chapter grouping from scratch — the allocating half of
    /// [`grouped`](Self::grouped), run only when the memo misses.
    fn build_groups(&self) -> Vec<ChapterGroup> {
        let mut groups: Vec<ChapterGroup> = Vec::new();
        for (i, m) in self.matches.iter().enumerate() {
            match groups.last_mut() {
                Some(g) if g.chapter == m.chapter => g.count += 1,
                _ => groups.push(ChapterGroup {
                    chapter: m.chapter,
                    title: m.title.clone(),
                    count: 1,
                    first_match: i,
                }),
            }
        }
        groups
    }
}

/// One chapter's row in the grouped results pane (#977): which chapter, its
/// title, how many matches fell in it, and the index (into
/// [`ProjectSearch::matches`]) of its first match — the row's jump target.
pub struct ChapterGroup {
    pub(crate) chapter: usize,
    pub(crate) title: String,
    pub(crate) count: usize,
    pub(crate) first_match: usize,
}

/// One chapter's row in the replace-all preview (#1014): the chapter's title, how
/// many matches will be rewritten in it, and a context snippet (the first match's
/// line) so the author can see *where* the sweep lands before committing.
pub struct PreviewRow {
    pub(crate) title: String,
    pub(crate) count: usize,
    pub(crate) context: String,
}

/// One manuscript-wide match (#295): which chapter, the byte range within that
/// chapter's prose body, and a short context snippet for the results list.
pub struct ProjectMatch {
    /// Chapter index (the jump target).
    pub(crate) chapter: usize,
    /// Byte range `[start, end)` within the chapter's body (front matter excluded).
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// The chapter's title, for grouping/labeling the row.
    pub(crate) title: String,
    /// The match's line, trimmed and clipped, shown as context.
    pub(crate) context: String,
}

/// Which step of the manuscript-wide find / replace interaction is active (#295).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProjectSearchStage {
    /// Typing the query; Enter runs the search across every chapter.
    Query,
    /// Results listed; navigate, jump to a match, or start a replace.
    Results,
    /// The grouped results pane (#977): one row per chapter with its match
    /// count and a total. Navigate chapters, jump to a chapter's first match, or
    /// toggle back to the flat list.
    Grouped,
    /// Typing the replacement text.
    Replacement,
    /// Stepping through matches, choosing replace / skip; `a` opens the
    /// replace-all preview.
    Step,
    /// The replace-all preview (#1014): a per-chapter breakdown of how many
    /// matches will be rewritten, each with a context snippet, shown before the
    /// irreversible manuscript-wide sweep. y/Enter confirms; anything else cancels
    /// (changing nothing).
    Preview,
}
/// Which step of the find / replace interaction is active.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchStage {
    /// Typing the search query (find or replace). Matches update as you type.
    Query,
    /// Find: query entered, stepping through matches with n / N.
    Navigate,
    /// Replace: typing the replacement text.
    Replacement,
    /// Replace: stepping through matches, choosing replace / skip / all.
    Step,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(chapter: usize, title: &str) -> ProjectMatch {
        ProjectMatch {
            chapter,
            start: 0,
            end: 1,
            title: title.into(),
            context: format!("ctx {title}"),
        }
    }

    #[test]
    fn grouped_is_memoized_and_invalidates_when_matches_change() {
        // The grouping (#977) is built once per change to `matches` and shared via
        // `Rc` — the Grouped/Preview overlays call it several times per frame, so it
        // must not rebuild + re-clone titles on every draw. The value is identical
        // to computing it fresh.
        let mut ps = ProjectSearch::new();
        ps.matches = vec![m(0, "One"), m(0, "One"), m(1, "Two")];

        let a = ps.grouped();
        let b = ps.grouped();
        assert!(
            Rc::ptr_eq(&a, &b),
            "a second call reuses the memoized groups"
        );

        // The content matches a fresh (unmemoized) build exactly.
        let fresh = ps.build_groups();
        assert_eq!(a.len(), fresh.len());
        for (g, f) in a.iter().zip(&fresh) {
            assert_eq!(
                (g.chapter, g.count, g.first_match),
                (f.chapter, f.count, f.first_match)
            );
            assert_eq!(g.title, f.title);
        }
        // Two chapters, counts 2 and 1, first-match indices 0 and 2.
        assert_eq!(
            a.iter()
                .map(|g| (g.chapter, g.count, g.first_match))
                .collect::<Vec<_>>(),
            vec![(0, 2, 0), (1, 1, 2)]
        );

        // Shrinking `matches` (a step-replace removes one) changes the length, so
        // the memo misses and rebuilds against the new matches.
        ps.matches.remove(2);
        let c = ps.grouped();
        assert!(
            !Rc::ptr_eq(&b, &c),
            "a changed match list rebuilds the groups"
        );
        assert_eq!(c.iter().map(|g| g.chapter).collect::<Vec<_>>(), vec![0]);
    }
}

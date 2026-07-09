//! Transient mode-state structs (#404): the overlay, search, and review state
//! that each [`Mode`](super::Mode) variant carries. Split out of `mode.rs` to keep
//! the core enums under the ~1,000-line convention. Pure move.

use std::path::PathBuf;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, CharClass};
use crate::text::MatchOpts;

use super::{AiAction, MetaField, Pending};

/// State for an open [`Mode::Findings`]: the findings list and the selected row.
/// `filter`/`typing` are the type-to-filter query and its input sub-mode (#822),
/// lifted from the chapter browser: `/` starts typing, keystrokes narrow the list
/// to findings whose summary or chapters match (case-insensitive substring), Esc
/// leaves input (keeping the filter) and a second Esc clears it.
#[derive(Default)]
pub struct Findings {
    pub(crate) items: Vec<Finding>,
    pub(crate) selected: usize,
    pub(crate) filter: String,
    pub(crate) typing: bool,
}

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

/// The category / lens a [`Finding`] belongs to (#827) — which analysis pass it
/// speaks for, kept on the finding so the pass survives the map to a summary
/// string and a round-trip through the durable store (where `action` is lost).
/// The manuscript-health dashboard groups by this, and cross-pass dedup keys on
/// it, so two passes flagging the same span with the same text stay distinct.
///
/// [`Other`](Self::Other) is the catch-all for findings not tied to an analysis
/// pass — the "Ask the manuscript" citations and answer rows — and the fallback
/// for a slug the store can't resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FindingKind {
    Continuity,
    Setting,
    Plot,
    Theme,
    Scaffold,
    Naming,
    Style,
    Editorial,
    Voice,
    /// Echo / near-duplicate passage pairs (#920) — the semantic-repetition scan.
    Echo,
    /// Not a re-runnable analysis pass (Q&A citations / answer rows), or an
    /// unrecognized persisted slug.
    Other,
}

impl FindingKind {
    /// The stable slug persisted in the durable store and matched back on load —
    /// never change one without a migration. Also the dashboard's group key.
    pub(crate) const fn slug(self) -> &'static str {
        match self {
            Self::Continuity => "continuity",
            Self::Setting => "setting",
            Self::Plot => "plot",
            Self::Theme => "theme",
            Self::Scaffold => "scaffold",
            Self::Naming => "naming",
            Self::Style => "style",
            Self::Editorial => "editorial",
            Self::Voice => "voice",
            Self::Echo => "echo",
            Self::Other => "other",
        }
    }

    /// The human label shown in the dashboard's group headers.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Continuity => "Continuity",
            Self::Setting => "Setting",
            Self::Plot => "Plot & arc",
            Self::Theme => "Theme & motif",
            Self::Scaffold => "Scaffold / outline",
            Self::Naming => "Names & terms",
            Self::Style => "Prose & style",
            Self::Editorial => "Editorial",
            Self::Voice => "Character voice",
            Self::Echo => "Echoes & near-duplicates",
            Self::Other => "Other",
        }
    }

    /// Resolve a persisted slug back to a kind; an unknown slug (a future or
    /// corrupt value) falls back to [`Other`](Self::Other) rather than failing.
    pub(crate) fn from_slug(slug: &str) -> Self {
        [
            Self::Continuity,
            Self::Setting,
            Self::Plot,
            Self::Theme,
            Self::Scaffold,
            Self::Naming,
            Self::Style,
            Self::Editorial,
            Self::Voice,
            Self::Echo,
        ]
        .into_iter()
        .find(|k| k.slug() == slug)
        .unwrap_or(Self::Other)
    }
}

/// One triageable finding (a continuity contradiction, style note, …). UI-shaped
/// — the AI layer maps its typed findings into this for display.
#[derive(Clone, Debug)]
pub struct Finding {
    /// One-line summary shown in the list (e.g. "object: the locket …").
    pub(crate) summary: String,
    /// The category / lens this finding belongs to (#827) — which pass it speaks
    /// for. Survives the map to a summary string and the durable-store round-trip
    /// (where `action` is lost), so the dashboard can group by it and cross-pass
    /// dedup can key on it.
    pub(crate) kind: FindingKind,
    /// Chapters involved; the first is the jump target.
    pub(crate) chapters: Vec<String>,
    /// The book the finding belongs to in a multi-book series (#341):
    /// `Some(index)` when a series-bible refresh produced it, so triage can
    /// attribute and jump across books (chapter titles can repeat between
    /// books). `None` targets the active book — every standalone-novel and
    /// active-book pass, unchanged.
    pub(crate) book: Option<usize>,
    /// The pass that produced this finding, so the author can re-check just this
    /// one after a fix by re-running its originating pass scoped to `chapters`
    /// (#608). `None` for findings not tied to a re-runnable pass — Q&A citations
    /// and history rows — and for findings restored from the durable store, which
    /// records content, not the pass (a fresh pass re-attributes them).
    pub(crate) action: Option<AiAction>,
    /// Other passes that independently raised the *same span* (#921): cross-pass
    /// consensus. `kind` is this finding's own pass; `corroborators` holds the
    /// additional passes that flagged the same chapters + summary, so the health
    /// view can weight a finding by agreement ("flagged by continuity + plot").
    /// Empty for a lone finding. Not part of identity — merging corroborators
    /// never changes which finding this *is* — so triage dedup and dismiss-matching
    /// stay keyed on content alone.
    pub(crate) corroborators: Vec<FindingKind>,
}

impl Finding {
    /// Every pass that agrees on this finding (#921): its own `kind` first, then
    /// the corroborating passes, de-duplicated and in first-seen order. One pass
    /// means no consensus; two or more is cross-pass agreement.
    pub(crate) fn agreement(&self) -> Vec<FindingKind> {
        let mut passes = vec![self.kind];
        for &k in &self.corroborators {
            if !passes.contains(&k) {
                passes.push(k);
            }
        }
        passes
    }

    /// Fold `other`'s passes into this finding's corroborators (#921) when a second
    /// pass raises the same span. Records `other`'s own kind and any consensus it
    /// already carried, skipping this finding's own kind and duplicates, so the
    /// agreement set stays a clean union.
    pub(crate) fn corroborate(&mut self, other: &Self) {
        for k in other.agreement() {
            if k != self.kind && !self.corroborators.contains(&k) {
                self.corroborators.push(k);
            }
        }
    }

    /// The cross-pass agreement signal (#921): `None` when a single pass raised
    /// this finding, or `Some("flagged by continuity + plot")` when passes agree —
    /// the consensus weight the findings list and health dashboard show. Passes are
    /// listed in agreement order (the primary first), lower-cased to read as a
    /// fragment. The single place the signal's wording lives, so the list, the
    /// dashboard, and its export stay identical.
    pub(crate) fn agreement_signal(&self) -> Option<String> {
        let passes = self.agreement();
        if passes.len() < 2 {
            return None;
        }
        let labels: Vec<String> = passes.iter().map(|k| k.label().to_lowercase()).collect();
        Some(format!("flagged by {}", labels.join(" + ")))
    }
}

/// Identity is the finding's *content* — kind, summary, chapters, and book — not
/// the originating `action` handle (#608). This mirrors the durable store's
/// content key ([`crate::app`]): a re-produced finding is the same finding, and a
/// finding restored from disk (which carries no `action`, but does carry its
/// persisted `kind`) still matches the one a fresh pass produces, so triage dedup
/// and dismiss-matching stay intact. `kind` sharpens cross-pass dedup (#827): two
/// passes flagging the same span with coincidentally identical text stay distinct.
/// `corroborators` (#921) is *not* part of identity — folding in an agreeing pass
/// weights a finding by consensus without changing which finding it is, so re-check
/// and dismiss-matching keep working on the merged finding.
impl PartialEq for Finding {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.summary == other.summary
            && self.chapters == other.chapters
            && self.book == other.book
    }
}

/// One row of the manuscript-health dashboard (#827): a group header (a
/// kind/pass, or a chapter within it) carrying its open-finding count, or a
/// jumpable finding under the current group. The dashboard is a flat, ordered
/// list of these — headers give the at-a-glance counts, finding rows are the
/// jump targets. Only [`Finding`](Self::Finding) rows are selectable.
pub enum DashboardRow {
    /// A kind/pass group header: the pass label and how many open findings it has.
    Kind { label: &'static str, count: usize },
    /// A chapter sub-header within the current kind group, with its count.
    Chapter { title: String, count: usize },
    /// A jumpable finding under the current kind + chapter.
    Finding(Finding),
}

impl DashboardRow {
    /// The finding this row addresses, or `None` for a header row (headers aren't
    /// selectable or jumpable).
    pub(crate) const fn finding(&self) -> Option<&Finding> {
        match self {
            Self::Finding(f) => Some(f),
            Self::Kind { .. } | Self::Chapter { .. } => None,
        }
    }
}

/// State for an open [`Mode::Dashboard`] — the manuscript-health overview (#827):
/// open findings grouped by kind/pass then chapter, each group headed by its
/// count, with the individual findings jumpable. `rows` is the flat, ordered
/// render/selection list ([`DashboardRow`]); `selected` is the currently
/// highlighted *finding* row (headers are skipped). `total` is the open-finding
/// count shown in the title. Built once when the overview opens — a read of the
/// standing triage state, never a fresh pass.
pub struct Dashboard {
    pub(crate) rows: Vec<DashboardRow>,
    pub(crate) selected: usize,
    pub(crate) total: usize,
}

impl Dashboard {
    /// The indices of the selectable (finding) rows, in order — the stops the
    /// arrow keys move between and the selection resolves against.
    pub(crate) fn finding_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.finding().is_some())
            .map(|(i, _)| i)
            .collect()
    }

    /// The finding at the given row index, if that row is a jumpable finding.
    pub(crate) fn finding_at(&self, row: usize) -> Option<&Finding> {
        self.rows.get(row).and_then(DashboardRow::finding)
    }
}

/// One row of the motif / character co-occurrence report (#922): a section header,
/// a co-occurring pair (with its shared chapters, the jump target), or a notable
/// absence (with the gap chapters). Only the pair and absence rows are jumpable;
/// `chapter` is the row's jump target (its first shared / missing chapter), which
/// the report's Enter activates.
pub enum CoOccurrenceRow {
    /// A section header (e.g. "Co-occurring" / "Notable absences").
    Header(&'static str),
    /// A co-occurring pair: the display line and the chapter it jumps to.
    Pair { line: String, chapter: String },
    /// A notable absence: the display line and the first missing chapter it jumps to.
    Absence { line: String, chapter: String },
}

impl CoOccurrenceRow {
    /// The chapter this row jumps to, or `None` for a header (headers aren't
    /// selectable).
    pub(crate) fn chapter(&self) -> Option<&str> {
        match self {
            Self::Pair { chapter, .. } | Self::Absence { chapter, .. } => Some(chapter),
            Self::Header(_) => None,
        }
    }
}

/// State for an open [`Mode::CoOccurrence`] — the motif / character co-occurrence
/// report (#922): co-occurring pairs and notable absences drawn from the bibles'
/// chapter-appearance data, each jumpable to its relevant chapter. `rows` is the
/// flat, ordered render/selection list; `selected` is the current jumpable row
/// (headers are skipped). A read of the standing bible data — no request.
pub struct CoOccurrence {
    pub(crate) rows: Vec<CoOccurrenceRow>,
    pub(crate) selected: usize,
}

impl CoOccurrence {
    /// The indices of the selectable (jumpable) rows, in order — the stops the
    /// arrow keys move between.
    pub(crate) fn jump_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.chapter().is_some())
            .map(|(i, _)| i)
            .collect()
    }

    /// The jump-target chapter at the given row index, if that row is jumpable.
    pub(crate) fn chapter_at(&self, row: usize) -> Option<&str> {
        self.rows.get(row).and_then(CoOccurrenceRow::chapter)
    }
}

/// State for an open [`Mode::BibleReview`]: the queue of conflicting bible files
/// (front of the queue is the one shown), and a scratch buffer while the author
/// edits the proposed version before saving.
pub struct BibleReview {
    pub(crate) conflicts: Vec<BibleConflict>,
    /// `Some` while editing the proposed version of the front conflict.
    pub(crate) editing: Option<Buffer>,
}

/// One bible file whose on-disk generated block differs from what a fresh pass
/// would write.
pub struct BibleConflict {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) proposed: String,
    /// Pre-rendered unified diff (current → proposed), each line prefixed with
    /// `+`/`-`/` `. Computed once when the conflict is staged so the overlay
    /// isn't re-diffing every frame.
    pub(crate) diff: Vec<String>,
    /// The on-disk entry's readable provenance (#832) — where it was generated
    /// from and that you've edited it — shown above the diff. `None` for a
    /// pre-#832 / foreign file that carries no provenance line.
    pub(crate) provenance: Option<String>,
}

/// State for an open [`Mode::EditMeta`]. `label` is public for the editor's
/// banner; the rest is the handler's business. The scratch `buffer` is reached
/// through `App::edit_buffer`.
pub struct MetaEdit {
    /// Which field is being edited (a quotation field).
    pub(crate) target: MetaField,
    /// Scratch buffer holding the field's text.
    pub(crate) buffer: Buffer,
    /// Banner label, e.g. "chapter epigraph".
    pub label: String,
    /// Chapter to reselect when editing finishes.
    pub(crate) nav_return: Option<usize>,
}

/// State for an open [`Mode::Prompt`]. `label`/`input` are public for the status
/// bar to render; the rest is the handler's business.
pub struct PromptState {
    /// What confirming the prompt does.
    pub(crate) pending: Pending,
    /// The prompt label shown in the status bar.
    pub label: String,
    /// Text typed so far (empty for y/N confirmations).
    pub input: String,
    /// Caret position as a byte offset into `input` (#706). Insertions and
    /// deletions happen here; the arrow keys, Home/End, and word-delete move it.
    /// Always kept on a `char` boundary of `input`.
    pub caret: usize,
    /// Where to return when the prompt closes: `Some(selected)` back to the
    /// chapter browser, `None` to insert mode (e.g. the quit prompt).
    pub(crate) nav_return: Option<usize>,
}

impl PromptState {
    /// A prompt prefilled with `input`, caret at its end (the natural spot to
    /// keep typing or edit from).
    pub(crate) const fn new(pending: Pending, label: String, input: String) -> Self {
        let caret = input.len();
        Self {
            pending,
            label,
            input,
            caret,
            nav_return: None,
        }
    }

    /// Clear the input and reset the caret to the start — used by tests to
    /// replace a prefilled default before typing a fresh value, keeping the
    /// caret invariant (in production the author backspaces through it).
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.input.clear();
        self.caret = 0;
    }

    /// Insert a typed character at the caret and advance past it (#706).
    pub(crate) fn insert(&mut self, c: char) {
        self.input.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    /// Delete the character before the caret (Backspace); a no-op at the start.
    pub(crate) fn backspace(&mut self) {
        if let Some(prev) = self.prev_char_boundary(self.caret) {
            self.input.replace_range(prev..self.caret, "");
            self.caret = prev;
        }
    }

    /// Delete the character at the caret (forward Delete); a no-op at the end.
    pub(crate) fn delete_forward(&mut self) {
        if let Some(next) = self.next_char_boundary(self.caret) {
            self.input.replace_range(self.caret..next, "");
        }
    }

    /// Delete the word before the caret (Ctrl/Alt-Backspace), reusing the
    /// buffer's word classification (#706): skip any whitespace immediately
    /// before the caret, then the run of one character class before that.
    pub(crate) fn delete_word_back(&mut self) {
        let target = self.word_start_before(self.caret);
        if target < self.caret {
            self.input.replace_range(target..self.caret, "");
            self.caret = target;
        }
    }

    /// Move the caret one character left; a no-op at the start.
    pub(crate) fn move_left(&mut self) {
        if let Some(prev) = self.prev_char_boundary(self.caret) {
            self.caret = prev;
        }
    }

    /// Move the caret one character right; a no-op at the end.
    pub(crate) fn move_right(&mut self) {
        if let Some(next) = self.next_char_boundary(self.caret) {
            self.caret = next;
        }
    }

    /// Move the caret to the start of the input.
    pub(crate) const fn move_home(&mut self) {
        self.caret = 0;
    }

    /// Move the caret to the end of the input.
    pub(crate) const fn move_end(&mut self) {
        self.caret = self.input.len();
    }

    /// The byte offset of the character boundary before `at`, or `None` at the
    /// start of the string.
    fn prev_char_boundary(&self, at: usize) -> Option<usize> {
        self.input[..at].char_indices().next_back().map(|(i, _)| i)
    }

    /// The byte offset of the character boundary after `at`, or `None` at the
    /// end of the string.
    fn next_char_boundary(&self, at: usize) -> Option<usize> {
        self.input[at..].chars().next().map(|c| at + c.len_utf8())
    }

    /// The start of the word ending at `at`: skip trailing whitespace, then the
    /// run of the class the preceding character belongs to. Mirrors the editor's
    /// `delete_word_back` (#641) using the shared [`CharClass`].
    fn word_start_before(&self, at: usize) -> usize {
        let mut boundaries = self.input[..at].grapheme_indices(true).rev().peekable();
        // Skip a run of trailing whitespace first.
        let mut cut = at;
        while let Some(&(i, g)) = boundaries.peek() {
            if CharClass::of(g) == CharClass::Space {
                cut = i;
                boundaries.next();
            } else {
                break;
            }
        }
        // Then delete the contiguous run of the next class we hit.
        let Some(&(_, g)) = boundaries.peek() else {
            return cut;
        };
        let class = CharClass::of(g);
        for (i, g) in boundaries {
            if CharClass::of(g) == class {
                cut = i;
            } else {
                break;
            }
        }
        cut
    }
}

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
        }
    }

    /// The number of distinct chapters the current matches span.
    pub(crate) fn chapter_span(&self) -> usize {
        self.grouped().len()
    }

    /// The current matches grouped by chapter (#977), in chapter order, each
    /// with its per-chapter count and the index of its first match (the jump
    /// target). Matches are already collected in chapter-then-document order, so
    /// each chapter's matches are one contiguous run.
    pub(crate) fn grouped(&self) -> Vec<ChapterGroup> {
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
    /// Stepping through matches, choosing replace / skip; `a` triggers the
    /// confirmed replace-all.
    Step,
    /// y/N confirm before the irreversible manuscript-wide replace-all.
    Confirm,
}

/// State for the manuscript-import preview (#702): the split the parser produced,
/// shown to the author before anything is written so a mis-split (scene breaks
/// read as chapters, or one giant chapter) is caught up front rather than after
/// the project exists. Enter confirms and proceeds to the write+open; Esc cancels
/// and writes nothing.
///
/// Everything needed to write is captured here when the preview opens, so the
/// confirm never re-reads the source. For a Markdown / plain-text source the split
/// heading level can be toggled (`levels` lists the levels the manuscript uses)
/// and the preview re-parsed at the chosen level; other formats (`.docx`,
/// `.scriv`, a folder of chapters) don't split on a Markdown level, so `levels` is
/// empty and only confirm/cancel apply.
pub struct ImportPreview {
    /// The source manuscript path (read-only; never moved or altered).
    pub(crate) source: PathBuf,
    /// Where the new project will be created on confirm.
    pub(crate) dest: PathBuf,
    /// The Markdown source text, kept so a level toggle can re-split without
    /// re-reading; `None` for formats that don't re-split (`.docx`/`.scriv`/folder).
    pub(crate) markdown: Option<String>,
    /// The book title mined from the source, if any.
    pub(crate) title: Option<String>,
    /// The book author mined from the source, if any.
    pub(crate) author: Option<String>,
    /// The chapters the current split produced (title + body), in reading order.
    pub(crate) chapters: Vec<(String, String)>,
    /// The heading level the current Markdown split broke on (`None` for the
    /// non-Markdown formats, which don't split on a level).
    pub(crate) level: Option<usize>,
    /// The heading levels the Markdown source actually uses, offered for re-split
    /// (empty for the non-Markdown formats).
    pub(crate) levels: Vec<usize>,
    /// The dropped-element / unreadable-body warning suffix (#511/#646), already
    /// formatted (empty when the parse was clean).
    pub(crate) warning: String,
    /// The first visible chapter row, so a long chapter list scrolls.
    pub(crate) scroll: usize,
}

/// State for the review-comment import preview (#886): the reviewer's Word
/// comments recovered from a marked-up `.docx`, resolved against the active
/// chapter's prose. The matched comments will become live annotations; the
/// unmatched ones (whose commented phrase no longer appears — the prose drifted)
/// are reported, not silently dropped. Enter merges the matches into the chapter's
/// annotations; Esc cancels and writes nothing.
///
/// Everything needed to create the annotations is captured here when the preview
/// opens (the resolved byte spans against the chapter buffer), so the merge never
/// re-reads the source or re-searches the prose.
pub struct ReviewImport {
    /// The source `.docx` path (read-only; never moved or altered).
    pub(crate) source: PathBuf,
    /// The chapter file the comments will be anchored into — the active chapter.
    pub(crate) chapter: String,
    /// The matched comments: `(start, end, note, reviewer)` buffer byte spans
    /// against the active chapter, each becoming one annotation on merge. The
    /// reviewer (#933) is the named editor the comment came from, `None` when the
    /// source didn't attribute it.
    pub(crate) matched: Vec<(usize, usize, String, Option<String>)>,
    /// The unmatched comments: `(phrase, note)` whose commented phrase no longer
    /// appears in the prose — reported so the author knows what didn't carry.
    pub(crate) dropped: Vec<(String, String)>,
    /// The first visible row, so a long list scrolls.
    pub(crate) scroll: usize,
}

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

//! Findings, dashboard, and co-occurrence mode state (#822/#827/#921/#922):
//! the triage list, the manuscript-health dashboard rows, and the motif /
//! character co-occurrence report. Split out of `states.rs` (pure move).

use crate::mode::AiAction;

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
    /// Cultural & historical accuracy findings (#973) — inaccuracy flags for an
    /// author-named concern. Accuracy, not sensitivity / representation.
    Accuracy,
    /// Foreshadow / payoff findings (#1047) — concrete seeded details (an object,
    /// a named threat, a promised meeting, a planted clue) set up then never paid
    /// off. Distinct from the plot pass's thematic-thread tracking.
    Foreshadow,
    /// Research-notebook fact-check findings (#1049) — passages that contradict a
    /// note the author wrote in `bible/research.md`. Grounded in the author's own
    /// ledger; not sensitivity / representation, and distinct from Accuracy.
    FactCheck,
    Voice,
    /// Echo / near-duplicate passage pairs (#920) — the semantic-repetition scan.
    Echo,
    /// Relationship-trajectory inconsistencies (#1046) — a chosen pair's arc
    /// contradicting itself or reversing off-page with no on-page setup.
    Relationship,
    /// Pacing & tension observations (#1048) — located, plain-language readings of
    /// the scene-level tension/quiet rhythm (a sagging middle, a run of high-tension
    /// chapters with no breath, a flat stretch). Observation only; no suggested change.
    Pacing,
    /// Not a re-runnable analysis pass (Q&A citations / answer rows), or an
    /// unrecognized persisted slug.
    Other,
}

impl FindingKind {
    /// Every kind, in a stable, meaningful sequence: the continuity/structure passes
    /// first, prose/voice next, and [`Other`](Self::Other) last (it collects the
    /// non-pass findings — Q&A citations — that rarely reach triage but shouldn't be
    /// dropped if they do). The single enumeration of the set: the dashboard's group
    /// order (#827), the health trend (#1052), and the revision-plan tie-break (#1055)
    /// all derive from it (`health.rs`), so a new kind joins them automatically rather
    /// than being silently dropped by a hand-maintained order list. [`from_slug`](Self::from_slug)
    /// folds over it too, so slug resolution can't drift from this list; [`slug`](Self::slug)
    /// and [`label`](Self::label) are independent exhaustive `match` arms (not folds
    /// over `ALL`), so the compiler forces them to cover every variant directly.
    pub(crate) const ALL: &'static [Self] = &[
        Self::Continuity,
        Self::Setting,
        Self::Plot,
        Self::Theme,
        Self::Scaffold,
        Self::Naming,
        Self::Style,
        Self::Editorial,
        Self::Accuracy,
        Self::Relationship,
        Self::Foreshadow,
        Self::FactCheck,
        Self::Pacing,
        Self::Voice,
        Self::Echo,
        Self::Other,
    ];

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
            Self::Accuracy => "accuracy",
            Self::Foreshadow => "foreshadow",
            Self::FactCheck => "factcheck",
            Self::Voice => "voice",
            Self::Echo => "echo",
            Self::Relationship => "relationship",
            Self::Pacing => "pacing",
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
            Self::Accuracy => "Cultural & historical accuracy",
            Self::Foreshadow => "Foreshadow & payoff",
            Self::FactCheck => "Research-notebook fact-check",
            Self::Voice => "Character voice",
            Self::Echo => "Echoes & near-duplicates",
            Self::Relationship => "Relationship trajectory",
            Self::Pacing => "Pacing & tension",
            Self::Other => "Other",
        }
    }

    /// Resolve a persisted slug back to a kind; an unknown slug (a future or
    /// corrupt value) falls back to [`Other`](Self::Other) rather than failing.
    /// Folds over [`ALL`](Self::ALL), so a new kind is resolvable the moment it
    /// joins the enum's ordering — no separate list to keep in sync.
    pub(crate) fn from_slug(slug: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
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

#[cfg(test)]
mod tests {
    use super::FindingKind;

    /// Every `FindingKind` variant, hand-listed. The completeness test below asserts
    /// this matches [`FindingKind::ALL`] (membership + count), so a variant missing
    /// from `ALL` — which nothing else catches, since the dashboard/plan tests derive
    /// their kinds FROM `ALL` — fails CI. When the enum grows, add the variant here
    /// and to `ALL`.
    const ALL_KINDS: &[FindingKind] = &[
        FindingKind::Continuity,
        FindingKind::Setting,
        FindingKind::Plot,
        FindingKind::Theme,
        FindingKind::Scaffold,
        FindingKind::Naming,
        FindingKind::Style,
        FindingKind::Editorial,
        FindingKind::Accuracy,
        FindingKind::Foreshadow,
        FindingKind::FactCheck,
        FindingKind::Voice,
        FindingKind::Echo,
        FindingKind::Relationship,
        FindingKind::Pacing,
        FindingKind::Other,
    ];

    /// Guards [`FindingKind::ALL`]'s completeness: every variant must appear in `ALL`,
    /// so a kind added to the enum but forgotten in `ALL` (silently dropped from the
    /// dashboard order, health trend, and revision-plan tie-break) fails CI. Existing
    /// coverage can't catch this because it iterates `ALL` itself. The slug round-trip
    /// also confirms `ALL` and `from_slug` agree.
    #[test]
    fn all_lists_every_finding_kind() {
        for kind in ALL_KINDS {
            assert!(
                FindingKind::ALL.contains(kind),
                "{kind:?} is missing from FindingKind::ALL — add it there",
            );
            // Round-trip through the persisted slug: `from_slug` folds over `ALL`, so
            // this also proves the variant is reachable via `ALL`, not just present.
            assert_eq!(
                FindingKind::from_slug(kind.slug()),
                *kind,
                "{kind:?} does not round-trip through slug/from_slug (missing from ALL?)",
            );
        }
        // One entry per variant: a forgotten `ALL` entry (fewer than the variants) or
        // a stray/duplicate entry both fail here, keeping `ALL` and the enum in step.
        assert_eq!(
            FindingKind::ALL.len(),
            ALL_KINDS.len(),
            "FindingKind::ALL has {} entries but there are {} variants — ALL must list \
             every variant exactly once",
            FindingKind::ALL.len(),
            ALL_KINDS.len(),
        );
    }
}

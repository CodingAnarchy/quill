//! Findings, dashboard, and co-occurrence mode state (#822/#827/#921/#922):
//! the triage list, the manuscript-health dashboard rows, and the motif /
//! character co-occurrence report. Split out of `states.rs` (pure move).

pub use crate::ai::{Finding, FindingKind};
use crate::mode::ListFilter;

/// State for an open [`Mode::Findings`]: the findings list and the selected row.
/// `filter` is the type-to-filter query and its input sub-mode (#822),
/// lifted from the chapter browser: `/` starts typing, keystrokes narrow the list
/// to findings whose summary or chapters match (case-insensitive substring), Esc
/// leaves input (keeping the filter) and a second Esc clears it.
#[derive(Default)]
pub struct Findings {
    pub(crate) items: Vec<Finding>,
    pub(crate) selected: usize,
    pub(crate) filter: ListFilter,
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

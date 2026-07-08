//! Motif / character co-occurrence (#922): a combinatorial, observation-only read
//! over the bibles that already track *where* each entry appears — the character
//! bible's per-chapter arc beats (#74) and the theme / motif tracker's
//! appearances (#828). It cross-references those chapter lists and reports which
//! entries **co-occur** (share chapters), how tightly, and which are
//! **conspicuously apart** — a character absent from a whole subplot, a motif that
//! clusters then vanishes. No new model and no manuscript prose: it reads facts
//! the trackers already hold and surfaces the pattern for the author to act on,
//! staying within the AI boundary (observe, never write). See
//! `docs/ai-features.md`.

use std::collections::BTreeSet;

/// One tracked bible entry with the chapters it appears in — the common shape the
/// character bible (arc beats) and the theme bible (appearances) both reduce to,
/// so the co-occurrence analysis is one pure function over both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The entry's display name — a character's name or a motif's name.
    pub name: String,
    /// The kind of entry, so the report can label mixed clusters.
    pub kind: EntryKind,
    /// The chapter titles the entry appears in, in the order the bible recorded
    /// them. De-duplicated on construction so a motif listed twice in one chapter
    /// doesn't skew the overlap counts.
    pub chapters: Vec<String>,
}

/// Whether an [`Entry`] came from the character bible or the theme / motif bible —
/// carried through so a co-occurrence pair can be labelled (character↔character,
/// motif↔motif, or the cross pairing a novelist most wants to see).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Character,
    Motif,
}

impl EntryKind {
    /// The short label shown beside an entry name in the report.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Motif => "motif",
        }
    }
}

impl Entry {
    /// Build an entry, de-duplicating repeated chapter titles while keeping the
    /// first-seen order (so the report reads the way the bible listed them).
    pub(crate) fn new(name: impl Into<String>, kind: EntryKind, chapters: &[String]) -> Self {
        let mut seen = BTreeSet::new();
        let chapters = chapters
            .iter()
            .filter(|c| !c.trim().is_empty())
            .filter(|c| seen.insert((*c).clone()))
            .cloned()
            .collect();
        Self {
            name: name.into(),
            kind,
            chapters,
        }
    }
}

/// A co-occurrence pair: two entries that share at least one chapter, with the
/// overlapping chapters (the jump targets) and how tightly they cluster.
#[derive(Clone, Debug, PartialEq)]
pub struct Pairing {
    pub a: String,
    pub a_kind: EntryKind,
    pub b: String,
    pub b_kind: EntryKind,
    /// The chapters both appear in, in first-seen order — the shared cluster and
    /// the report's jump targets.
    pub shared: Vec<String>,
    /// How tight the pairing is: shared chapters over the union of their chapters
    /// (the Jaccard overlap), 0.0–1.0. A pair that always appears together scores
    /// 1.0; a single shared chapter among many scores low.
    pub tightness: f32,
}

/// A notable absence: an entry conspicuously missing from a stretch of the book —
/// a character absent from a whole subplot, a motif that clusters in Act I then
/// drops out. Reported as the run of consecutive chapters (by manuscript order)
/// the entry is missing from, so the author can see the gap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Absence {
    pub name: String,
    pub kind: EntryKind,
    /// The consecutive run of chapters (in manuscript order) the entry is absent
    /// from — the gap. The first is the jump target.
    pub missing: Vec<String>,
}

/// The full co-occurrence read: the pairings that co-occur (most-tightly-clustered
/// first) and the notable absences (longest gap first). Pure over the inputs, so
/// it's directly testable over sample bible data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub pairings: Vec<Pairing>,
    pub absences: Vec<Absence>,
}

impl Report {
    /// Whether the read found nothing worth reporting — no co-occurring pair and no
    /// notable absence (too little bible data, or everything appears everywhere).
    pub(crate) const fn is_empty(&self) -> bool {
        self.pairings.is_empty() && self.absences.is_empty()
    }
}

/// The minimum shared chapters for a pair to be reported. A single incidental
/// shared chapter is still a co-occurrence the author put there, so the floor is
/// one; the tightness score separates the always-paired from the incidental.
const MIN_SHARED: usize = 1;

/// The minimum consecutive-chapter gap for an absence to be "notable". A one- or
/// two-chapter gap is ordinary pacing; a longer run is the conspicuous-absence the
/// report is for.
const MIN_GAP: usize = 3;

/// Compute the co-occurrence report (#922) over the tracked entries and the
/// manuscript's chapter order.
///
/// `order` is the chapter titles in manuscript order — the spine the absences are
/// measured against (an entry is "absent" from a chapter in `order` it doesn't
/// appear in). Entries with fewer than two appearances can't co-occur meaningfully
/// and can't have a *gap* between appearances, so they contribute nothing.
pub fn analyze(entries: &[Entry], order: &[String]) -> Report {
    let pairings = pairings(entries);
    let absences = absences(entries, order);
    Report { pairings, absences }
}

/// Every co-occurring pair, most-tightly-clustered first (ties broken by shared
/// count, then name) — a stable order so the report reads the same run to run.
fn pairings(entries: &[Entry]) -> Vec<Pairing> {
    let mut out = Vec::new();
    for (i, a) in entries.iter().enumerate() {
        for b in &entries[i + 1..] {
            let shared = shared_chapters(a, b);
            if shared.len() < MIN_SHARED {
                continue;
            }
            let union = union_count(a, b);
            let tightness = if union == 0 {
                0.0
            } else {
                shared.len() as f32 / union as f32
            };
            out.push(Pairing {
                a: a.name.clone(),
                a_kind: a.kind,
                b: b.name.clone(),
                b_kind: b.kind,
                shared,
                tightness,
            });
        }
    }
    out.sort_by(|x, y| {
        y.tightness
            .partial_cmp(&x.tightness)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(y.shared.len().cmp(&x.shared.len()))
            .then(x.a.cmp(&y.a))
            .then(x.b.cmp(&y.b))
    });
    out
}

/// The chapters `a` and `b` both appear in, in `a`'s first-seen order.
fn shared_chapters(a: &Entry, b: &Entry) -> Vec<String> {
    let b_set: BTreeSet<&String> = b.chapters.iter().collect();
    a.chapters
        .iter()
        .filter(|c| b_set.contains(c))
        .cloned()
        .collect()
}

/// The size of the union of `a` and `b`'s chapters — the denominator of the
/// tightness (Jaccard) score.
fn union_count(a: &Entry, b: &Entry) -> usize {
    a.chapters
        .iter()
        .chain(b.chapters.iter())
        .collect::<BTreeSet<_>>()
        .len()
}

/// The notable absences, longest gap first. An entry's gap is measured only within
/// the span of the manuscript it's present in — between its first and last
/// appearance — so a character who simply hasn't entered yet, or has exited for
/// good, isn't flagged; only a conspicuous *drop-out mid-presence* counts.
fn absences(entries: &[Entry], order: &[String]) -> Vec<Absence> {
    let index: std::collections::HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, c)| (c.as_str(), i))
        .collect();
    let mut out = Vec::new();
    for e in entries {
        // Positions in manuscript order the entry appears at (chapters not in the
        // order spine are ignored — they can't anchor a gap).
        let mut present: Vec<usize> = e
            .chapters
            .iter()
            .filter_map(|c| index.get(c.as_str()).copied())
            .collect();
        present.sort_unstable();
        present.dedup();
        if present.len() < 2 {
            continue; // no interior span to have a gap in
        }
        for run in gaps(&present, MIN_GAP) {
            let missing: Vec<String> = run.iter().map(|&i| order[i].clone()).collect();
            out.push(Absence {
                name: e.name.clone(),
                kind: e.kind,
                missing,
            });
        }
    }
    // Longest gap first, ties by name then first missing chapter — stable.
    out.sort_by(|x, y| {
        y.missing
            .len()
            .cmp(&x.missing.len())
            .then(x.name.cmp(&y.name))
            .then(x.missing.first().cmp(&y.missing.first()))
    });
    out
}

/// The runs of consecutive chapter indices strictly between the entry's first and
/// last appearance that it's missing from, each at least `min` long. `present` is
/// sorted, de-duplicated appearance positions.
fn gaps(present: &[usize], min: usize) -> Vec<Vec<usize>> {
    let mut runs = Vec::new();
    for pair in present.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        // The chapters strictly between two consecutive appearances.
        if to > from + 1 {
            let run: Vec<usize> = (from + 1..to).collect();
            if run.len() >= min {
                runs.push(run);
            }
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(titles: &[&str]) -> Vec<String> {
        titles.iter().map(|s| (*s).to_string()).collect()
    }

    fn character(name: &str, chapters: &[&str]) -> Entry {
        Entry::new(name, EntryKind::Character, &ch(chapters))
    }

    fn motif(name: &str, chapters: &[&str]) -> Entry {
        Entry::new(name, EntryKind::Motif, &ch(chapters))
    }

    #[test]
    fn entry_dedups_repeated_chapters_keeping_order() {
        let e = Entry::new("Mirrors", EntryKind::Motif, &ch(&["Two", "One", "Two", ""]));
        assert_eq!(e.chapters, ch(&["Two", "One"]));
    }

    #[test]
    fn pairs_report_shared_chapters_and_tightness() {
        // Two motifs that always appear together score a perfect tightness; the
        // shared chapters are the cluster.
        let entries = vec![
            motif("Mirrors", &["One", "Two", "Three"]),
            motif("Locket", &["One", "Two", "Three"]),
        ];
        let report = analyze(&entries, &ch(&["One", "Two", "Three"]));
        assert_eq!(report.pairings.len(), 1);
        let p = &report.pairings[0];
        assert_eq!(p.a, "Mirrors");
        assert_eq!(p.b, "Locket");
        assert_eq!(p.shared, ch(&["One", "Two", "Three"]));
        assert!((p.tightness - 1.0).abs() < f32::EPSILON, "always paired");
    }

    #[test]
    fn tighter_pairs_sort_first() {
        // Mara & Tomas share every chapter (tight); Mara & the storm share one of
        // several (loose). The tight pair leads the list.
        let entries = vec![
            character("Mara", &["One", "Two", "Three"]),
            character("Tomas", &["One", "Two", "Three"]),
            motif("Storm", &["One", "Four"]),
        ];
        let report = analyze(&entries, &ch(&["One", "Two", "Three", "Four"]));
        assert!(report.pairings.len() >= 2);
        let first = &report.pairings[0];
        assert_eq!((first.a.as_str(), first.b.as_str()), ("Mara", "Tomas"));
        assert!(first.tightness > report.pairings[1].tightness);
    }

    #[test]
    fn no_shared_chapters_means_no_pair() {
        let entries = vec![
            character("Mara", &["One", "Two"]),
            character("Ada", &["Three", "Four"]),
        ];
        let report = analyze(&entries, &ch(&["One", "Two", "Three", "Four"]));
        assert!(report.pairings.is_empty(), "disjoint casts don't pair");
    }

    #[test]
    fn cross_kind_pairs_carry_both_kinds() {
        // A character and a motif that co-occur — the cross pairing, labelled with
        // both kinds.
        let entries = vec![
            character("Mara", &["One", "Two"]),
            motif("Mirrors", &["One", "Two"]),
        ];
        let report = analyze(&entries, &ch(&["One", "Two"]));
        assert_eq!(report.pairings.len(), 1);
        let p = &report.pairings[0];
        assert_eq!(p.a_kind, EntryKind::Character);
        assert_eq!(p.b_kind, EntryKind::Motif);
    }

    #[test]
    fn notable_absence_is_a_mid_presence_gap() {
        // The storm clusters in the opening, drops out across the long middle, and
        // returns at the end: a conspicuous mid-presence gap.
        let order = ch(&["One", "Two", "Three", "Four", "Five", "Six"]);
        let entries = vec![motif("Storm", &["One", "Two", "Six"])];
        let report = analyze(&entries, &order);
        assert_eq!(report.absences.len(), 1);
        let a = &report.absences[0];
        assert_eq!(a.name, "Storm");
        assert_eq!(a.missing, ch(&["Three", "Four", "Five"]));
    }

    #[test]
    fn a_short_gap_is_not_notable() {
        // A two-chapter gap is ordinary pacing, below the notable threshold.
        let order = ch(&["One", "Two", "Three", "Four"]);
        let entries = vec![motif("Storm", &["One", "Four"])];
        let report = analyze(&entries, &order);
        assert!(report.absences.is_empty(), "a 2-chapter gap isn't notable");
    }

    #[test]
    fn absence_ignores_chapters_before_entry_or_after_exit() {
        // Ada enters late and never leaves; the chapters before her entry aren't an
        // absence (she simply hadn't appeared yet).
        let order = ch(&["One", "Two", "Three", "Four", "Five"]);
        let entries = vec![character("Ada", &["Four", "Five"])];
        let report = analyze(&entries, &order);
        assert!(
            report.absences.is_empty(),
            "pre-entry chapters aren't a gap"
        );
    }

    #[test]
    fn longest_gap_sorts_first() {
        let order = ch(&["1", "2", "3", "4", "5", "6", "7", "8"]);
        let entries = vec![
            // A four-chapter gap.
            character("Long", &["1", "6", "7", "8"]),
            // A three-chapter gap.
            character("Short", &["1", "5", "6", "7", "8"]),
        ];
        let report = analyze(&entries, &order);
        assert_eq!(report.absences.len(), 2);
        assert_eq!(report.absences[0].name, "Long");
        assert_eq!(report.absences[0].missing.len(), 4);
        assert_eq!(report.absences[1].name, "Short");
    }

    #[test]
    fn empty_when_nothing_co_occurs_or_gaps() {
        let entries = vec![character("Solo", &["One"])];
        let report = analyze(&entries, &ch(&["One"]));
        assert!(report.is_empty());
    }
}

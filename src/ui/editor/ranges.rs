//! Row-window queries over the byte-range lists the body renderer styles from
//! (conceal markers, search matches, spell / style-flag / proximity hits, and
//! annotations). Each list covers the whole buffer — thousands of entries on a
//! window-sized chapter — while a rendered row spans a few dozen bytes, so the
//! renderer narrows every list to the row's window once per row (a binary search)
//! and the per-character checks then walk only the handful of ranges that can
//! actually touch the row. Scanning the whole lists per character made a frame of
//! emphasized, spell-flagged prose cost milliseconds.

use super::ByteRange;

/// The entries of `list` that overlap `[start, end)`, as a sub-slice. `list` must
/// be start-sorted and pairwise disjoint (each range ends at or before the next
/// begins), which makes the ends sorted too — so both bounds are a binary search
/// and every entry in the returned slice overlaps the window. That holds for the
/// conceal markers (distinct delimiter bytes, fence lines, break lines), search
/// matches (non-overlapping by construction), and the spell / style-flag /
/// proximity hits (distinct words or sentences, in text order). Lists that may
/// overlap — annotations — go through [`Overlapping`] instead.
pub(super) fn disjoint_within(list: &[ByteRange], start: usize, end: usize) -> &[ByteRange] {
    let lo = list.partition_point(|&(_, e)| e <= start);
    let hi = list.partition_point(|&(s, _)| s < end);
    list.get(lo..hi).unwrap_or(&[])
}

/// Whether `list` is start-sorted and pairwise disjoint — the precondition of
/// [`disjoint_within`], checked once per frame in debug builds where a list is
/// handed to the renderer.
pub(super) fn is_sorted_disjoint(list: &[ByteRange]) -> bool {
    list.windows(2).all(|w| w[0].1 <= w[1].0)
}

/// Whether `byte` lies inside any of `ranges` — the per-character test, run over a
/// list already narrowed to the row so it's a handful of compares at most.
pub(super) fn covers(ranges: &[ByteRange], byte: usize) -> bool {
    ranges.iter().any(|&(s, e)| byte >= s && byte < e)
}

/// A start-sorted range list whose entries may overlap or nest (annotations: two
/// notes can share words, and a long note can span many rows). Ends aren't sorted,
/// so [`disjoint_within`]'s end bound doesn't apply; instead `reach[i]` holds the
/// furthest end among `ranges[..=i]`, which *is* non-decreasing. Every entry before
/// the first index whose reach passes a window's start ends at or before it, so
/// the binary search can't skip a long earlier range that still extends into the
/// row — it starts the scan no later than that range.
#[derive(Default)]
pub(super) struct Overlapping {
    ranges: Vec<ByteRange>,
    reach: Vec<usize>,
}

impl Overlapping {
    /// Index `ranges`, sorting them by start first (a no-op pass on the
    /// already-sorted annotation list, so ordering is guaranteed here rather than
    /// trusted from every producer).
    pub(super) fn new(mut ranges: Vec<ByteRange>) -> Self {
        ranges.sort_unstable();
        let mut furthest = 0;
        let reach = ranges
            .iter()
            .map(|&(_, e)| {
                furthest = furthest.max(e);
                furthest
            })
            .collect();
        Self { ranges, reach }
    }

    /// The entries overlapping `[start, end)`, collected into a (usually empty)
    /// small `Vec` — no allocation when nothing overlaps.
    pub(super) fn within(&self, start: usize, end: usize) -> Vec<ByteRange> {
        let lo = self.reach.partition_point(|&r| r <= start);
        let hi = self.ranges.partition_point(|&(s, _)| s < end);
        self.ranges
            .get(lo..hi)
            .unwrap_or(&[])
            .iter()
            .copied()
            .filter(|&(s, e)| s < end && e > start)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_within_returns_exactly_the_overlapping_entries() {
        let list = [(0, 3), (5, 8), (10, 12), (20, 30)];
        assert_eq!(disjoint_within(&list, 4, 11), &[(5, 8), (10, 12)]);
        // A range straddling the window's start is kept; one ending at it is not.
        assert_eq!(disjoint_within(&list, 7, 9), &[(5, 8)]);
        assert_eq!(disjoint_within(&list, 8, 10), &[] as &[ByteRange]);
        assert_eq!(disjoint_within(&list, 25, 26), &[(20, 30)]);
        assert_eq!(disjoint_within(&[], 0, 10), &[] as &[ByteRange]);
        assert!(is_sorted_disjoint(&list));
        assert!(!is_sorted_disjoint(&[(0, 5), (3, 8)]));
    }

    #[test]
    fn overlapping_keeps_a_long_range_that_starts_well_before_the_window() {
        // A long note (0..100) followed by short ones that end before the window:
        // the long one must still be found for a window far past its start.
        let idx = Overlapping::new(vec![(10, 12), (0, 100), (20, 22), (50, 55), (90, 95)]);
        assert_eq!(idx.within(60, 70), vec![(0, 100)]);
        assert_eq!(idx.within(52, 92), vec![(0, 100), (50, 55), (90, 95)]);
        assert_eq!(idx.within(100, 120), Vec::<ByteRange>::new());
        // Nested / shared-start notes are each reported.
        let idx = Overlapping::new(vec![(5, 9), (5, 20), (6, 7)]);
        assert_eq!(idx.within(6, 8), vec![(5, 9), (5, 20), (6, 7)]);
        assert_eq!(idx.within(9, 10), vec![(5, 20)]);
    }
}

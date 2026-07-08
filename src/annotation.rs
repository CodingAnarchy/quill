//! Inline author annotations (#302): a note anchored to a byte range of a
//! chapter's prose, shown while editing and excluded from every export.
//!
//! Annotations live in a plain-text side file ([`crate::project`] writes
//! `annotations.toml`) keyed by chapter file name, so the chapter `.md` stays
//! byte-for-byte clean — which is also why exports never carry them: the
//! exporters read the chapter bodies, which hold no annotation data.
//!
//! On disk each annotation is a chapter-local byte range ([`Annotation`]). While
//! a chapter is open it's held as a [`Live`] range against the edit buffer and
//! re-anchored on the idle tick via [`remap`], so a note stays on its phrase as
//! the prose around it is edited.

use serde::{Deserialize, Serialize};

/// A reply on an annotation thread (#889): a short follow-up the author adds under
/// the opening note, so an annotation (including one imported from a reviewer's
/// comment, #886) can carry a back-and-forth. `author` is a free-form label
/// ("author", or a reviewer's name); `text` is the reply body.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub author: String,
    pub text: String,
}

/// One annotation as stored on disk: a chapter-local byte range plus the note.
/// `chapter` is the chapter's file name — the stable key that survives reordering
/// and retitling.
///
/// `replies` and `resolved` (#889) extend a flat note into a thread: an existing
/// single-note annotation loads with no replies and open, so the on-disk format
/// stays backward-compatible (both fields `#[serde(default)]`, and omitted from
/// output when at their default so a plain note round-trips unchanged).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub chapter: String,
    pub start: usize,
    pub end: usize,
    pub note: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolved: bool,
}

/// The `annotations.toml` side file: a flat array of `[[annotation]]` tables.
/// Flat (rather than nested per chapter) keeps the format trivially portable and
/// hand-editable.
#[derive(Serialize, Deserialize, Default)]
pub struct AnnotationFile {
    #[serde(default, rename = "annotation")]
    pub items: Vec<Annotation>,
}

/// An annotation anchored into the live edit buffer: a byte range plus the note,
/// tagged with the chapter file it belongs to. In the single-chapter view the
/// range is chapter-local; in the continuous view (#370) it's buffer-absolute
/// (offset by the owning segment's start) and `chapter` says where to persist it.
/// Re-mapped as the prose is edited; persisted back to chapter-local offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub chapter: String,
    pub start: usize,
    pub end: usize,
    pub note: String,
    /// Thread replies + resolved state (#889), carried live so the review list can
    /// show and toggle them; persisted back to disk with the note.
    pub replies: Vec<Reply>,
    pub resolved: bool,
}

/// Shift a byte range through one edit `(start, old_end, new_end)` — bytes
/// `[start, old_end)` were replaced by `new_end - start` new bytes. Returns the
/// re-anchored range, or `None` if the edit consumed the anchored text entirely
/// (the note is then dropped). An insertion inside the range extends it; a
/// deletion inside it shrinks it; an edit wholly before it slides it; one wholly
/// after it leaves it untouched.
#[cfg(test)]
pub fn remap(range: (usize, usize), edit: (usize, usize, usize)) -> Option<(usize, usize)> {
    let (ns, ne) = (shift_point(range.0, edit), shift_point(range.1, edit));
    (ns < ne).then_some((ns, ne))
}

/// Shift one byte position `p` through an edit `(start, old_end, new_end)` — the
/// per-point rule [`remap`] applies to each endpoint. A point at/before the edit
/// is untouched; one at/after the replaced span slides by the length change; one
/// strictly inside it collapses to where the replacement begins. Monotonic
/// non-decreasing in `p`, which is what lets [`Remapper`] compose a whole burst.
const fn shift_point(p: usize, (start, old_end, new_end): (usize, usize, usize)) -> usize {
    if p <= start {
        p
    } else if p >= old_end {
        (p as isize + new_end as isize - old_end as isize) as usize
    } else {
        start
    }
}

/// A composed remap of a whole edit burst (#893), so re-anchoring N annotations
/// through E edits costs O(E + N log E) rather than O(E · N) — one folded
/// coordinate map, queried per annotation, instead of walking every annotation
/// once per edit.
///
/// Each edit `(start, old_end, new_end)` is in the buffer's coordinate space
/// *after* the prior edits, exactly as [`remap`] applies them in sequence. The
/// map is a monotonic-non-decreasing step function of the original position,
/// stored as breakpoints on the original axis, and evaluating it at a point is
/// identical to folding [`remap`]'s `shift` across the edits in order. The one
/// exception [`remap`] makes — collapsing a point *inside* a replaced span to the
/// span's start — is reproduced by the `collapse` markers below.
pub struct Remapper {
    /// Breakpoints on the original axis, sorted ascending by `orig`. Between two
    /// breakpoints the map is a straight shift `p + delta`; at a breakpoint whose
    /// `collapse` is set, an original point in `[orig, next_orig)` maps not by the
    /// shift but to the fixed `mapped` value (the replaced span collapsed to its
    /// start). The first breakpoint is always `orig = 0`.
    breaks: Vec<Break>,
}

#[derive(Clone, Copy, Debug)]
struct Break {
    orig: usize,
    delta: isize,
    /// When set, original points in this breakpoint's run all map to `collapse_to`
    /// (they fell inside a replaced span); otherwise they map by `delta`.
    collapse_to: Option<usize>,
}

impl Remapper {
    /// Build the composed map for `edits` applied in order. Empty (identity) when
    /// there are no edits.
    #[must_use]
    pub fn new(edits: &[(usize, usize, usize)]) -> Self {
        let mut m = Self {
            breaks: vec![Break {
                orig: 0,
                delta: 0,
                collapse_to: None,
            }],
        };
        for &edit in edits {
            m.fold(edit);
        }
        m
    }

    /// Fold one edit into the composed map as `shift_e ∘ old_map`: the new map's
    /// output at any point is the old output pushed through this edit's point-shift
    /// (the same shift [`remap`] applies). Introduce breakpoints where the shift's
    /// own boundaries fall on the original axis (the first point whose old output is
    /// `> start`, and the first whose old output is `>= old_end`) so each run's
    /// closed-form rule stays exact, then re-express every run's output through the
    /// shift.
    fn fold(&mut self, (start, old_end, new_end): (usize, usize, usize)) {
        // Original-axis cuts for the shift's boundaries. `cut_lo`: first point whose
        // old output exceeds `start` (below it, output <= start, unchanged).
        // `cut_hi`: first whose old output reaches `old_end` (at/above it, output
        // shifts by `delta`). Between them the output lands inside the replaced span
        // and collapses to `start`.
        let cut_lo = self.preimage(start + 1);
        let cut_hi = self.preimage(old_end);

        // Splice the two cut points in as breakpoints (idempotent if already
        // present), then re-express every run's output through the shift.
        self.split_at(cut_lo);
        self.split_at(cut_hi);
        let edit = (start, old_end, new_end);
        for b in &mut self.breaks {
            if let Some(c) = b.collapse_to {
                // A collapse run outputs a fixed value: push that value through.
                b.collapse_to = Some(shift_point(c, edit));
            } else {
                // A shift run outputs `orig + delta`; if this whole run lands inside
                // the replaced span it becomes a collapse, otherwise it re-shifts.
                let out = (b.orig as isize + b.delta) as usize;
                if out > start && out < old_end {
                    b.collapse_to = Some(start);
                } else {
                    let new_out = shift_point(out, edit);
                    b.delta = new_out as isize - b.orig as isize;
                }
            }
        }
    }

    /// Ensure a breakpoint exists at original position `pos`, splitting the run
    /// that governs it (the new break carries the same output rule, so the map is
    /// unchanged). A no-op when a breakpoint is already there.
    fn split_at(&mut self, pos: usize) {
        let idx = self.breaks.partition_point(|b| b.orig <= pos);
        if idx > 0 && self.breaks[idx - 1].orig == pos {
            return; // already a breakpoint here
        }
        let governing = self.breaks[idx - 1];
        self.breaks.insert(
            idx,
            Break {
                orig: pos,
                ..governing
            },
        );
    }

    /// The map's value at original position `p` (monotonic non-decreasing).
    #[must_use]
    pub fn eval(&self, p: usize) -> usize {
        // The last breakpoint whose `orig <= p` governs `p`.
        let idx = self
            .breaks
            .partition_point(|b| b.orig <= p)
            .saturating_sub(1);
        let b = self.breaks[idx];
        b.collapse_to
            .unwrap_or_else(|| (p as isize + b.delta) as usize)
    }

    /// The smallest original position whose image is `>= target` under the current
    /// map — the original-axis cut for a current-space coordinate. Used to convert
    /// an edit's coordinates back onto the original axis.
    fn preimage(&self, target: usize) -> usize {
        // The map is non-decreasing, so binary-search the original axis for the
        // first `p` with `eval(p) >= target`. Beyond the last breakpoint the map is
        // `p + last_delta`, so `p = target - last_delta` already reaches `target`;
        // bound the search there (plus the last breakpoint's own position and a
        // one-cell cushion) so a negative delta can't cap the search too low.
        let last = self.breaks.last().copied().unwrap_or(Break {
            orig: 0,
            delta: 0,
            collapse_to: None,
        });
        let reach = target as isize - last.delta;
        let hi = reach.max(last.orig as isize).max(target as isize).max(0) as usize + 1;
        let (mut lo, mut hi) = (0usize, hi);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.eval(mid) >= target {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    }

    /// Re-anchor one range `[rs, re)` through the whole burst, or `None` if the
    /// range's text was consumed — identical to folding [`remap`] over the edits.
    #[must_use]
    pub fn remap_range(&self, rs: usize, re: usize) -> Option<(usize, usize)> {
        let (ns, ne) = (self.eval(rs), self.eval(re));
        (ns < ne).then_some((ns, ne))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_before_slides_the_range() {
        // Insert 3 bytes at offset 2: [2,2)->len 3, delta +3.
        assert_eq!(remap((10, 15), (2, 2, 5)), Some((13, 18)));
    }

    #[test]
    fn insertion_after_leaves_it_put() {
        assert_eq!(remap((10, 15), (20, 20, 24)), Some((10, 15)));
    }

    #[test]
    fn insertion_inside_extends_the_end() {
        // Type 4 bytes at offset 12, inside [10,15).
        assert_eq!(remap((10, 15), (12, 12, 16)), Some((10, 19)));
    }

    #[test]
    fn deletion_before_slides_it_back() {
        // Delete [2,5) (3 bytes), delta -3.
        assert_eq!(remap((10, 15), (2, 5, 2)), Some((7, 12)));
    }

    #[test]
    fn deletion_inside_shrinks_it() {
        // Delete [11,13) inside [10,15): end shifts back by 2.
        assert_eq!(remap((10, 15), (11, 13, 11)), Some((10, 13)));
    }

    #[test]
    fn deleting_the_whole_span_drops_it() {
        // Delete [8,18) which covers [10,15) entirely.
        assert_eq!(remap((10, 15), (8, 18, 8)), None);
    }

    #[test]
    fn replacement_overlapping_the_start_clamps() {
        // Replace [8,12) with 1 byte (delta -3): start collapses to 8, end -3.
        assert_eq!(remap((10, 15), (8, 12, 9)), Some((8, 12)));
    }

    /// Fold [`remap`] across an edit sequence the naive way — the O(edits · ranges)
    /// per-edit walk the [`Remapper`] replaces. The batched map must match this
    /// exactly (#893).
    fn naive(
        mut range: Option<(usize, usize)>,
        edits: &[(usize, usize, usize)],
    ) -> Option<(usize, usize)> {
        for &e in edits {
            range = range.and_then(|r| remap(r, e));
        }
        range
    }

    #[test]
    fn remapper_matches_the_naive_fold_on_known_bursts() {
        let bursts: &[&[(usize, usize, usize)]] = &[
            &[(2, 2, 5), (0, 0, 3)],       // two inserts before
            &[(12, 12, 16), (13, 13, 14)], // inserts inside
            &[(2, 5, 2), (11, 13, 11)],    // delete before, delete inside
            &[(8, 12, 9), (0, 0, 4), (30, 34, 30)],
        ];
        for edits in bursts {
            let m = Remapper::new(edits);
            for rs in 0..25usize {
                for re in (rs + 1)..26 {
                    assert_eq!(
                        m.remap_range(rs, re),
                        naive(Some((rs, re)), edits),
                        "range ({rs},{re}) under {edits:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn remapper_matches_the_naive_fold_exhaustively() {
        // A deterministic pseudo-random spread of edit bursts over a small axis,
        // checked against the naive fold for every range — the correctness anchor
        // for the batched remap (#893).
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rng = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..400 {
            let n = (rng() % 6) as usize;
            let mut edits = Vec::new();
            for _ in 0..n {
                let start = (rng() % 20) as usize;
                let old_len = (rng() % 6) as usize;
                let new_len = (rng() % 6) as usize;
                edits.push((start, start + old_len, start + new_len));
            }
            let m = Remapper::new(&edits);
            for rs in 0..22usize {
                for re in (rs + 1)..23 {
                    assert_eq!(
                        m.remap_range(rs, re),
                        naive(Some((rs, re)), &edits),
                        "range ({rs},{re}) under {edits:?}"
                    );
                }
            }
        }
    }
}

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

/// One annotation as stored on disk: a chapter-local byte range plus the note.
/// `chapter` is the chapter's file name — the stable key that survives reordering
/// and retitling.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub chapter: String,
    pub start: usize,
    pub end: usize,
    pub note: String,
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
}

/// Shift a byte range through one edit `(start, old_end, new_end)` — bytes
/// `[start, old_end)` were replaced by `new_end - start` new bytes. Returns the
/// re-anchored range, or `None` if the edit consumed the anchored text entirely
/// (the note is then dropped). An insertion inside the range extends it; a
/// deletion inside it shrinks it; an edit wholly before it slides it; one wholly
/// after it leaves it untouched.
pub fn remap((rs, re): (usize, usize), (start, old_end, new_end): (usize, usize, usize)) -> Option<(usize, usize)> {
    let shift = |p: usize| -> usize {
        if p <= start {
            p
        } else if p >= old_end {
            (p as isize + new_end as isize - old_end as isize) as usize
        } else {
            // Inside the replaced span: collapse to where the replacement begins.
            start
        }
    };
    let (ns, ne) = (shift(rs), shift(re));
    (ns < ne).then_some((ns, ne))
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
}

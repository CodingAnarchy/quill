//! Case-insensitive substring matching over manuscript text (#155): the single
//! definition of "what counts as a match", shared by find/replace (#144), the
//! TODO-marker count, and character-mention counting. One folding rule (per-
//! character Unicode lowercase) so the three features can never disagree.

/// Collapse `s`'s whitespace to single spaces and trim it to `max` characters,
/// appending `…` when shortened — a one-line snippet for prompts and list rows
/// (#302). Char-based, so it never splits a UTF-8 codepoint.
pub fn ellipsize(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

/// Run `f(start, end)` for each non-overlapping, case-insensitive match of
/// `needle` in `text`, as byte ranges `[start, end)`. The one place the match
/// rule lives; [`match_ranges`] and [`count_matches`] are thin wrappers.
fn scan(text: &str, needle: &str, mut f: impl FnMut(usize, usize)) {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return;
    }
    let hay: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if (0..needle.len()).all(|k| char_eq_ci(hay[i + k].1, needle[k])) {
            let start = hay[i].0;
            let last = &hay[i + needle.len() - 1];
            f(start, last.0 + last.1.len_utf8());
            i += needle.len(); // non-overlapping
        } else {
            i += 1;
        }
    }
}

/// Byte ranges of every case-insensitive, non-overlapping match of `needle` in
/// `text`, in order. Empty when the needle is empty. Folding goes through each
/// character's Unicode lowercase mapping, so "José" matches "josé" and "The"
/// matches "the".
pub fn match_ranges(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    scan(text, needle, |start, end| out.push((start, end)));
    out
}

/// How many times `needle` occurs in `text` (case-insensitive, non-overlapping).
/// Counts without allocating the range list.
pub fn count_matches(text: &str, needle: &str) -> usize {
    let mut n = 0;
    scan(text, needle, |_, _| n += 1);
    n
}

/// Case-insensitive single-character comparison via Unicode lowercase folding.
fn char_eq_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_case_insensitive_non_overlapping_matches() {
        let text = "The cat sat on the mat. THE END.";
        let m = match_ranges(text, "the");
        assert_eq!(m.len(), 3, "The / the / THE all match");
        assert_eq!(m[0], (0, 3)); // the leading "The"
        for (s, e) in m {
            assert_eq!(e - s, 3); // each range spans the needle length
        }
    }

    #[test]
    fn empty_needle_matches_nothing() {
        assert!(match_ranges("anything", "").is_empty());
        assert_eq!(count_matches("anything", ""), 0);
    }

    #[test]
    fn non_overlapping_for_repeated_runs() {
        // "aa" in "aaaa" yields two matches, not three.
        assert_eq!(match_ranges("aaaa", "aa"), vec![(0, 2), (2, 4)]);
        assert_eq!(count_matches("aaaa", "aa"), 2);
    }

    #[test]
    fn folds_accented_capitals() {
        let text = "José and JOSÉ";
        assert_eq!(count_matches(text, "josé"), 2);
    }

    #[test]
    fn count_matches_agrees_with_match_ranges() {
        let text = "xxx here, XXX there, and Xxx everywhere";
        assert_eq!(count_matches(text, "xxx"), match_ranges(text, "xxx").len());
        assert_eq!(count_matches(text, "xxx"), 3);
    }
}

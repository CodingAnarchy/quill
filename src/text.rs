//! Substring matching over manuscript text (#155): the single definition of
//! "what counts as a match", shared by find/replace (#144), the TODO-marker
//! count, and character-mention counting. One folding rule (per-character
//! Unicode lowercase) so the three features can never disagree.
//!
//! Find & replace can refine the rule with three toggles (#570, #891): a
//! case-sensitive mode (compare codepoints as typed, no folding), a whole-word
//! mode (match only when both boundaries are non-word characters), and a regex
//! mode (#891) that treats the query as a regular expression rather than a
//! literal. "Word character" is defined for prose as alphanumeric plus the
//! apostrophe `'`, so a whole-word search for "Will" skips "willing" but a
//! search for "don't" still matches the contraction as one word. The other
//! callers keep the default rule (case-insensitive substring) unchanged via
//! [`match_ranges`] / [`count_matches`].
//!
//! Regex matching (#891) is layered on top: the same [`MatchOpts`] carries a
//! `regex` flag, and case-sensitive / whole-word compose with it (a
//! case-insensitive regex, or one anchored at word boundaries). An invalid
//! pattern is a normal outcome — a live find recomputes on every keystroke, so
//! most of the time the query is a half-typed expression — and is reported via
//! [`try_match_ranges`] rather than panicking; the infallible entry points
//! ([`match_ranges_opts`] and friends) treat a bad pattern as "no matches".

use regex::{Regex, RegexBuilder};
use ropey::Rope;

/// The refinements find & replace can apply to the base substring rule (#570,
/// #891). The default — all `false` — is case-insensitive substring matching,
/// byte-for-byte what every caller did before the toggles existed.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct MatchOpts {
    /// Compare codepoints as typed (no Unicode lowercase folding).
    pub case_sensitive: bool,
    /// Match only when both boundaries are non-word characters.
    pub whole_word: bool,
    /// Treat the query as a regular expression rather than a literal (#891).
    pub regex: bool,
}

impl MatchOpts {
    /// A short, stable label for the active modes, e.g. `[Aa]`, `[w]`, `[re]`,
    /// `[Aa w re]`, or empty when all are off — for the find prompt / overlay
    /// (#570, #891).
    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.case_sensitive {
            parts.push("Aa");
        }
        if self.whole_word {
            parts.push("w");
        }
        if self.regex {
            parts.push("re");
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("[{}]", parts.join(" "))
        }
    }
}

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

/// Run `f(start, end)` for each non-overlapping match of `needle` in `text`
/// honoring `opts`, as byte ranges `[start, end)`. The one place the match rule
/// lives; the public range/count helpers are thin wrappers over it.
fn scan(text: &str, needle: &str, opts: MatchOpts, mut f: impl FnMut(usize, usize)) {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return;
    }
    let hay: Vec<(usize, char)> = text.char_indices().collect();
    let char_eq = |a: char, b: char| {
        if opts.case_sensitive {
            a == b
        } else {
            char_eq_ci(a, b)
        }
    };
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let hit = (0..needle.len()).all(|k| char_eq(hay[i + k].1, needle[k]))
            && (!opts.whole_word || word_bounded(&hay, i, needle.len()));
        if hit {
            let start = hay[i].0;
            let last = &hay[i + needle.len() - 1];
            f(start, last.0 + last.1.len_utf8());
            i += needle.len(); // non-overlapping
        } else {
            i += 1;
        }
    }
}

/// Byte ranges of every non-overlapping match of `needle` in `text` under the
/// default rule (case-insensitive substring), in order. Empty when the needle is
/// empty. Folding goes through each character's Unicode lowercase mapping, so
/// "José" matches "josé" and "The" matches "the".
pub fn match_ranges(text: &str, needle: &str) -> Vec<(usize, usize)> {
    match_ranges_opts(text, needle, MatchOpts::default())
}

/// Byte ranges of every non-overlapping match of `needle` in `text` honoring
/// `opts` (#570, #891), in order. With [`MatchOpts::default`] this is exactly
/// [`match_ranges`]. An invalid regex (`opts.regex` on) yields no matches — the
/// infallible path a live/incremental find rides; callers that must surface the
/// error to the author use [`try_match_ranges`].
pub fn match_ranges_opts(text: &str, needle: &str, opts: MatchOpts) -> Vec<(usize, usize)> {
    try_match_ranges(text, needle, opts).unwrap_or_default()
}

/// Byte ranges of every non-overlapping match of `needle` in `text` honoring
/// `opts` (#570, #891), in order — the fallible entry point. Substring modes
/// never fail (`Ok`); regex mode returns `Err(message)` when `needle` is not a
/// valid regular expression, so the find UI can report it instead of silently
/// finding nothing (#891).
pub fn try_match_ranges(
    text: &str,
    needle: &str,
    opts: MatchOpts,
) -> Result<Vec<(usize, usize)>, String> {
    if opts.regex {
        regex_ranges(text, needle, opts)
    } else {
        let mut out = Vec::new();
        scan(text, needle, opts, |start, end| out.push((start, end)));
        Ok(out)
    }
}

/// Byte ranges of every non-overlapping regex match of `needle` in `text` (#891).
/// `case_sensitive` maps to the regex crate's case flag (default off = folded,
/// like the literal path); `whole_word` wraps the pattern in `\b…\b` so it only
/// matches at word boundaries. An empty needle finds nothing (as with the
/// literal path); a compile error is returned as a human-readable message.
fn regex_ranges(text: &str, needle: &str, opts: MatchOpts) -> Result<Vec<(usize, usize)>, String> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = if opts.whole_word {
        format!(r"\b(?:{needle})\b")
    } else {
        needle.to_string()
    };
    let re: Regex = RegexBuilder::new(&pattern)
        .case_insensitive(!opts.case_sensitive)
        .build()
        .map_err(|e| regex_error_message(&e))?;
    // Skip zero-width matches (e.g. `a*` against "bbb"): they'd otherwise yield a
    // run of empty ranges the caller can't navigate to or replace meaningfully.
    Ok(re
        .find_iter(text)
        .filter(|m| m.start() != m.end())
        .map(|m| (m.start(), m.end()))
        .collect())
}

/// A short, single-line message from a regex compile error, for the find prompt.
/// The crate's `Display` is multi-line (it underlines the offending span); the
/// author only needs to know the pattern is invalid, so keep the first line.
fn regex_error_message(err: &regex::Error) -> String {
    let full = err.to_string();
    let first = full.lines().next().unwrap_or("invalid regex").trim();
    format!("Invalid regex: {first}")
}

/// How many times `needle` occurs in `text` under the default rule
/// (case-insensitive, non-overlapping). Counts without allocating the range list.
pub fn count_matches(text: &str, needle: &str) -> usize {
    let mut n = 0;
    scan(text, needle, MatchOpts::default(), |_, _| n += 1);
    n
}

/// Lines `[start, end)` of `rope` as a string (with their newlines), clamped to
/// its length — `Buffer::line_range_text` over a detached rope, for the
/// background writers that slice a snapshot off the UI thread.
pub fn rope_line_range(rope: &Rope, start: usize, end: usize) -> String {
    let last = rope.len_lines();
    let (start, end) = (start.min(last), end.min(last));
    if start >= end {
        return String::new();
    }
    rope.slice(rope.line_to_char(start)..rope.line_to_char(end))
        .to_string()
}

/// Case-insensitive single-character comparison via Unicode lowercase folding.
fn char_eq_ci(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// Whether a "word character" for prose: alphanumeric or an apostrophe, so a
/// whole-word search treats contractions like "don't" as one word (#570).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\''
}

/// Whether the run `hay[i..i + len]` is bounded by non-word characters (or the
/// text edge) on both sides — the whole-word rule (#570).
fn word_bounded(hay: &[(usize, char)], i: usize, len: usize) -> bool {
    let before_ok = i == 0 || !is_word_char(hay[i - 1].1);
    let after_ok = i + len >= hay.len() || !is_word_char(hay[i + len].1);
    before_ok && after_ok
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

    // --- #570: case-sensitive and whole-word toggles ---------------------

    fn opts(case_sensitive: bool, whole_word: bool) -> MatchOpts {
        MatchOpts {
            case_sensitive,
            whole_word,
            regex: false,
        }
    }

    #[test]
    fn default_opts_match_the_plain_helpers() {
        // The both-off path must be identical to the unflagged helpers.
        let text = "Will, will, and willing.";
        assert_eq!(
            match_ranges_opts(text, "will", MatchOpts::default()),
            match_ranges(text, "will"),
        );
    }

    #[test]
    fn case_sensitive_only_matches_exact_case() {
        // Renaming the character "Will" must not touch "will"/"willing" by case.
        let text = "Will met will while willing it.";
        // Case-insensitive (default): Will, will, will(ing) — three substrings.
        assert_eq!(match_ranges(text, "Will").len(), 3);
        // Case-sensitive: only the capitalized "Will" and the "Will" in nothing
        // else here — just the one leading "Will".
        let m = match_ranges_opts(text, "Will", opts(true, false));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0], (0, 4));
    }

    #[test]
    fn whole_word_only_skips_substrings() {
        let text = "Will met will while willing it.";
        // Whole-word, case-insensitive: "Will" and "will" (both standalone),
        // but not the "will" inside "willing" or "while".
        let m = match_ranges_opts(text, "will", opts(false, true));
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], (0, 4)); // "Will"
        assert_eq!(m[1], (9, 13)); // "will"
    }

    #[test]
    fn case_sensitive_and_whole_word_together() {
        let text = "Will met will while willing it.";
        // Exact case AND standalone: only the leading "Will".
        let m = match_ranges_opts(text, "Will", opts(true, true));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0], (0, 4));
    }

    #[test]
    fn whole_word_keeps_contractions_intact() {
        // The apostrophe is a word char, so "don't" is one word, not "don" + "t".
        let text = "I don't and you don't.";
        assert_eq!(match_ranges_opts(text, "don't", opts(false, true)).len(), 2);
        // "don" alone is never a whole word inside "don't".
        assert!(match_ranges_opts(text, "don", opts(false, true)).is_empty());
    }

    #[test]
    fn whole_word_respects_punctuation_and_edges() {
        // Boundaries at string edges and against punctuation both count.
        let text = "cat. (cat) catnap cat";
        let m = match_ranges_opts(text, "cat", opts(false, true));
        assert_eq!(m.len(), 3, "the three standalone cats, not 'catnap'");
    }

    #[test]
    fn label_reflects_active_modes() {
        assert_eq!(MatchOpts::default().label(), "");
        assert_eq!(opts(true, false).label(), "[Aa]");
        assert_eq!(opts(false, true).label(), "[w]");
        assert_eq!(opts(true, true).label(), "[Aa w]");
    }

    // --- #891: regex mode -------------------------------------------------

    fn re_opts(case_sensitive: bool, whole_word: bool) -> MatchOpts {
        MatchOpts {
            case_sensitive,
            whole_word,
            regex: true,
        }
    }

    #[test]
    fn regex_label_reflects_the_mode() {
        assert_eq!(re_opts(false, false).label(), "[re]");
        assert_eq!(re_opts(true, false).label(), "[Aa re]");
        assert_eq!(re_opts(true, true).label(), "[Aa w re]");
    }

    #[test]
    fn regex_matches_a_pattern() {
        // `Chap\. \d+` — the manuscript-cleanup case from the issue.
        let text = "See Chap. 3 and Chap. 12, but not Chapter 4.";
        let m = match_ranges_opts(text, r"Chap\. \d+", re_opts(false, false));
        assert_eq!(m.len(), 2);
        assert_eq!(&text[m[0].0..m[0].1], "Chap. 3");
        assert_eq!(&text[m[1].0..m[1].1], "Chap. 12");
    }

    #[test]
    fn regex_is_case_insensitive_by_default() {
        let text = "Foo foo FOO";
        assert_eq!(
            match_ranges_opts(text, "foo", re_opts(false, false)).len(),
            3
        );
        // Case-sensitive regex only matches the exact casing.
        assert_eq!(
            match_ranges_opts(text, "foo", re_opts(true, false)).len(),
            1
        );
    }

    #[test]
    fn regex_whole_word_anchors_at_boundaries() {
        let text = "cat cats category cat.";
        // Without whole-word, `cat` matches inside "cats"/"category" too.
        assert_eq!(
            match_ranges_opts(text, "cat", re_opts(false, false)).len(),
            4
        );
        // With whole-word, only the two standalone "cat"s.
        assert_eq!(
            match_ranges_opts(text, "cat", re_opts(false, true)).len(),
            2
        );
    }

    #[test]
    fn invalid_regex_is_reported_not_panicked() {
        // An unbalanced group is a compile error, not a panic.
        let err = try_match_ranges("abc", "a(", re_opts(false, false)).unwrap_err();
        assert!(err.starts_with("Invalid regex:"), "got: {err}");
        // The infallible path treats it as no matches.
        assert!(match_ranges_opts("abc", "a(", re_opts(false, false)).is_empty());
    }

    #[test]
    fn zero_width_regex_matches_are_dropped() {
        // `a*` matches the empty string everywhere; those are unnavigable, so the
        // find drops them and keeps only the non-empty run.
        let m = match_ranges_opts("baab", "a*", re_opts(false, false));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0], (1, 3));
    }

    #[test]
    fn empty_regex_matches_nothing() {
        assert!(match_ranges_opts("anything", "", re_opts(false, false)).is_empty());
    }
}

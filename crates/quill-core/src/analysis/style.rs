//! Word-frequency and style flags (#404): the deterministic, frequency-based
//! style signals and the "most-used words"/phrases tallies. Extracted from
//! `analysis/mod.rs` to keep that module under the ~1,000-line convention; a
//! pure move, re-exported by the parent, so the analysis output is unchanged.
//!
//! Heuristic and purely statistical — the AI prose/style pass (#79) does the
//! context-aware work. These lists are intentionally small: the goal is a rough,
//! descriptive signal the author decides on, not a grammar.

use std::collections::{HashMap, HashSet};

use unicode_segmentation::UnicodeSegmentation;

/// "To be" forms — the weak-/passive-verb proxy.
pub const TO_BE: &[&str] = &["am", "is", "are", "was", "were", "be", "been", "being"];

/// Filtering verbs that distance the reader ("she *saw* the door" → "the door").
const FILTER_WORDS: &[&str] = &[
    "see",
    "saw",
    "seen",
    "look",
    "looked",
    "watch",
    "watched",
    "hear",
    "heard",
    "feel",
    "felt",
    "notice",
    "noticed",
    "wonder",
    "wondered",
    "realize",
    "realized",
    "think",
    "thought",
    "know",
    "knew",
    "seem",
    "seemed",
    "decide",
    "decided",
    "remember",
    "remembered",
    "believe",
    "believed",
    "sense",
    "sensed",
];

/// Common `-ly` words that aren't manner adverbs, so they don't inflate the
/// adverb density.
const NON_ADVERB_LY: &[&str] = &[
    "only",
    "family",
    "reply",
    "apply",
    "supply",
    "comply",
    "imply",
    "rely",
    "multiply",
    "ally",
    "bully",
    "rally",
    "jelly",
    "belly",
    "folly",
    "lily",
    "fly",
    "ugly",
    "early",
    "holy",
    "italy",
    "july",
    "assembly",
    "anomaly",
    "melancholy",
    "fully",
];

/// Frequent function words excluded from the "most-used words" list (so the list
/// surfaces an author's content tics, not "the"/"and"). Defaults baked in for
/// now; a per-project override could join `run_on_threshold` later.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "of", "at", "by", "for", "with", "about", "to",
    "from", "in", "on", "into", "over", "after", "before", "as", "is", "are", "was", "were", "be",
    "been", "being", "am", "do", "does", "did", "have", "has", "had", "having", "i", "you", "he",
    "she", "it", "we", "they", "me", "him", "her", "us", "them", "my", "your", "his", "its", "our",
    "their", "this", "that", "these", "those", "there", "here", "then", "than", "so", "not", "no",
    "yes", "up", "down", "out", "off", "all", "any", "some", "what", "which", "who", "whom",
    "when", "where", "why", "how", "would", "could", "should", "will", "can", "may", "might",
    "must", "shall", "just", "very", "too", "more", "most", "now", "back", "got", "get", "one",
    "like", "said",
];

/// The effective frequency word lists for a report (#119): the built-in
/// stopwords and filter words, adjusted by per-project `[analysis]` overrides.
/// Built once per report and passed to the scanners instead of reading the
/// module consts, so a project can tune what counts without touching the binary.
/// Words are stored and matched lowercased.
pub struct WordLists {
    stopwords: HashSet<String>,
    filter_words: HashSet<String>,
}

impl Default for WordLists {
    fn default() -> Self {
        Self::new()
    }
}

impl WordLists {
    /// The built-in defaults (no overrides) — identical to the historical
    /// behavior, so an unconfigured project's report is unchanged.
    pub fn new() -> Self {
        Self {
            stopwords: STOPWORDS
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            filter_words: FILTER_WORDS
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
        }
    }

    /// Apply `add`/`remove` overrides to the stopword list (#119): `remove` first
    /// (drop built-ins), then `add` (extend), both lowercased.
    pub fn override_stopwords(&mut self, add: &[String], remove: &[String]) {
        adjust(&mut self.stopwords, add, remove);
    }

    /// Apply `add`/`remove` overrides to the filter-word list (#119).
    pub fn override_filter_words(&mut self, add: &[String], remove: &[String]) {
        adjust(&mut self.filter_words, add, remove);
    }

    /// Whether `word` (lowercased) is an excluded frequency word. Crate-visible so
    /// the flag-sweep scanner (#1001) shares the report's exact stopword definition.
    pub fn is_stopword(&self, word: &str) -> bool {
        self.stopwords.contains(word)
    }

    /// Whether `word` (lowercased) is a filtering verb. Crate-visible so the
    /// flag-sweep scanner (#1001) flags the same words the report counts.
    pub fn is_filter_word(&self, word: &str) -> bool {
        self.filter_words.contains(word)
    }
}

/// Drop `remove` from `set`, then add `add` — both lowercased. `remove` first so
/// an add always wins a conflict.
fn adjust(set: &mut HashSet<String>, add: &[String], remove: &[String]) {
    for w in remove {
        set.remove(&w.to_lowercase());
    }
    for w in add {
        set.insert(w.to_lowercase());
    }
}

/// Frequency-based style flags for a span of prose, in raw counts (densities are
/// computed against `words` at render). Additive across chapters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StyleFlags {
    pub words: usize,
    /// `-ly` manner adverbs.
    pub adverbs_ly: usize,
    /// Filtering verbs (see [`FILTER_WORDS`]).
    pub filter_words: usize,
    /// "To be" forms — the weak-verb proxy.
    pub weak_verbs: usize,
    /// Estimated passive constructions ("to be" + an `-ed` participle).
    pub passive: usize,
}

impl StyleFlags {
    pub const fn add(&mut self, other: Self) {
        self.words += other.words;
        self.adverbs_ly += other.adverbs_ly;
        self.filter_words += other.filter_words;
        self.weak_verbs += other.weak_verbs;
        self.passive += other.passive;
    }
}

/// Scan a span of prose for the frequency-based style flags, using `lists` for
/// the filter-word set (#119).
pub fn style_flags(text: &str, lists: &WordLists) -> StyleFlags {
    let mut flags = StyleFlags::default();
    let mut prev_to_be = false;
    for word in text.unicode_words() {
        flags.words += 1;
        let lower = word.to_lowercase();
        if is_ly_adverb(&lower) {
            flags.adverbs_ly += 1;
        }
        if lists.is_filter_word(&lower) {
            flags.filter_words += 1;
        }
        let to_be = TO_BE.contains(&lower.as_str());
        if to_be {
            flags.weak_verbs += 1;
        }
        // A "to be" form followed by an -ed word is a likely passive ("was held").
        if prev_to_be && lower.ends_with("ed") {
            flags.passive += 1;
        }
        prev_to_be = to_be;
    }
    flags
}

/// Whether a lowercased word is counted as an `-ly` manner adverb. Visible within
/// the (private) analysis module so the flag-sweep scanner (#1001) marks exactly
/// what the report tallies.
pub fn is_ly_adverb(word: &str) -> bool {
    word.len() > 3 && word.ends_with("ly") && !NON_ADVERB_LY.contains(&word)
}

/// Accumulate non-stopword content-word counts (lowercased) from `text` into
/// `counts` — the basis for the "most-used words" list.
pub fn tally_content_words(text: &str, counts: &mut HashMap<String, usize>, lists: &WordLists) {
    for word in text.unicode_words() {
        let lower = word.to_lowercase();
        if lower.len() > 2 && !lists.is_stopword(&lower) {
            *counts.entry(lower).or_insert(0) += 1;
        }
    }
}

/// Accumulate trigram counts (lowercased, within `text` only so phrases don't
/// span chapters) into `counts`, skipping trigrams that are entirely stopwords
/// (e.g. "out of the") so the list surfaces real repeated phrasing.
pub fn tally_phrases(text: &str, counts: &mut HashMap<String, usize>, lists: &WordLists) {
    let words: Vec<String> = text.unicode_words().map(str::to_lowercase).collect();
    for w in words.windows(3) {
        if w.iter().all(|word| lists.is_stopword(word)) {
            continue;
        }
        *counts.entry(w.join(" ")).or_insert(0) += 1;
    }
}

/// The `n` highest-count entries with count `>= min`, ties broken alphabetically
/// for a deterministic, stable order.
pub fn top_counts(counts: &HashMap<String, usize>, n: usize, min: usize) -> Vec<(String, usize)> {
    let mut items: Vec<(String, usize)> = counts
        .iter()
        .filter(|&(_, &c)| c >= min)
        .map(|(k, &c)| (k.clone(), c))
        .collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items.truncate(n);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_flags_count_adverbs_filters_and_weak_verbs() {
        // "slowly" / "quietly" = -ly adverbs; "only" is excluded. "saw"/"felt" are
        // filter words. "was" is a to-be form; "was locked" (to-be + -ed) is a
        // passive estimate.
        let f = style_flags(
            "She slowly saw the only door and felt it was locked quietly.",
            &WordLists::new(),
        );
        assert_eq!(f.adverbs_ly, 2, "slowly + quietly, not only");
        assert_eq!(f.filter_words, 2, "saw + felt");
        assert_eq!(f.weak_verbs, 1, "was");
        assert_eq!(f.passive, 1, "was locked");
    }

    #[test]
    fn content_words_skip_stopwords_and_short_words() {
        let mut counts = HashMap::new();
        tally_content_words(
            "The locket and the locket on it",
            &mut counts,
            &WordLists::new(),
        );
        assert_eq!(counts.get("locket"), Some(&2));
        assert_eq!(counts.get("the"), None, "stopword excluded");
        assert_eq!(counts.get("on"), None, "short/stopword excluded");
    }

    #[test]
    fn phrases_tally_trigrams_and_skip_all_stopword_runs() {
        let mut counts = HashMap::new();
        let text = "the locket gleamed. the locket gleamed again out of the box.";
        tally_phrases(text, &mut counts, &WordLists::new());
        assert_eq!(counts.get("the locket gleamed"), Some(&2));
        assert_eq!(
            counts.get("out of the"),
            None,
            "all-stopword trigram skipped"
        );
    }

    #[test]
    fn stopword_overrides_extend_and_drop_defaults() {
        let mut lists = WordLists::new();
        // "said" is a built-in stopword — remove it so it can surface; add "locket".
        lists.override_stopwords(&["locket".to_string()], &["Said".to_string()]); // remove is case-insensitive
        let mut counts = HashMap::new();
        tally_content_words("said said locket locket gleamed", &mut counts, &lists);
        assert_eq!(
            counts.get("said"),
            Some(&2),
            "removed from stopwords → now counted"
        );
        assert_eq!(counts.get("locket"), None, "added to stopwords → excluded");
        assert_eq!(counts.get("gleamed"), Some(&1));
    }

    #[test]
    fn filter_word_overrides_extend_and_drop_defaults() {
        let mut lists = WordLists::new();
        // "saw" is a built-in filter word — remove it; add "gazed".
        lists.override_filter_words(&["gazed".to_string()], &["saw".to_string()]);
        let f = style_flags("She saw the door and gazed at it", &lists);
        assert_eq!(f.filter_words, 1, "saw no longer counts, gazed now does");
    }

    #[test]
    fn no_overrides_match_the_built_in_defaults() {
        let mut a = HashMap::new();
        let mut b = HashMap::new();
        let text = "the saw and the locket and the door";
        tally_content_words(text, &mut a, &WordLists::new());
        tally_content_words(text, &mut b, &WordLists::default());
        assert_eq!(a, b);
        assert_eq!(
            a.get("the"),
            None,
            "built-in stopword still excluded by default"
        );
    }

    #[test]
    fn top_counts_orders_by_count_then_name() {
        let counts = HashMap::from([
            ("beta".to_string(), 3),
            ("alpha".to_string(), 3),
            ("rare".to_string(), 1),
        ]);
        // count desc, ties alphabetical; min filters out the single occurrence.
        assert_eq!(
            top_counts(&counts, 10, 2),
            vec![("alpha".to_string(), 3), ("beta".to_string(), 3)]
        );
    }
}

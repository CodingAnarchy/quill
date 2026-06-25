//! Local, offline manuscript analysis: deterministic editing metrics computed
//! from the prose itself, with no network access. This first slice (#67) is the
//! readability surface — Flesch Reading Ease, Flesch–Kincaid grade, Gunning Fog,
//! and SMOG — plus the shared text-stats primitives (sentence segmentation and a
//! syllable estimator) that the rest of the analysis epic (#66) reuses.
//!
//! The formulas are documented in `docs/manuscript-analysis.md`. The word basis
//! matches the progress report's notion of prose words (`buffer::count_words` /
//! Unicode word segmentation), so the two reports agree on how big a chapter is.

use std::collections::{HashMap, HashSet};

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::count_words;

/// Common abbreviations whose trailing period is not a sentence end. Lowercased,
/// without the period. Covers the titles and Latinisms that show up in prose;
/// the segmenter falls back to the structural rule (a boundary needs whitespace
/// then a capital) for everything else, so this list need not be exhaustive.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "st", "sr", "jr", "vs", "etc", "rev", "hon",
    "gen", "col", "sgt", "lt", "capt", "messrs", "mt", "no", "vol", "fig",
];

/// Raw text statistics for a span of prose — the basis for every readability
/// score. Additive across chapters, so the whole-manuscript figures are the sum
/// of the per-chapter ones rather than an average of averages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Prose words (Unicode word segmentation — matches `buffer::count_words`).
    pub words: usize,
    /// Sentences (terminator-delimited; see [`count_sentences`]).
    pub sentences: usize,
    /// Total estimated syllables across all words.
    pub syllables: usize,
    /// Words of three or more syllables ("complex" / polysyllabic), the input to
    /// Gunning Fog and SMOG.
    pub polysyllables: usize,
}

impl Stats {
    /// Accumulate another span's counts (for the whole-manuscript total).
    pub fn add(&mut self, other: Stats) {
        self.words += other.words;
        self.sentences += other.sentences;
        self.syllables += other.syllables;
        self.polysyllables += other.polysyllables;
    }
}

/// The four readability scores derived from a [`Stats`]. Reading Ease rises with
/// easier prose (0–100+); the other three are U.S. grade levels (higher = harder).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Readability {
    pub flesch_reading_ease: f64,
    pub flesch_kincaid_grade: f64,
    pub gunning_fog: f64,
    pub smog: f64,
}

/// A chapter's prose with Markdown heading lines removed, so the analysis
/// measures what a reader actually reads. A chapter title (`# The Dark`) or a
/// scene heading (`## Later`) is structure, not a sentence — left in, its words
/// glue onto the following sentence and skew the length and readability figures.
/// Heading lines become blank lines (paragraph/sentence boundaries are
/// unaffected); every other line is kept verbatim. Front matter is already
/// stripped upstream.
pub fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if !is_heading(line) {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Whether `line` is a Markdown ATX heading (`#`…`######` followed by a space or
/// the end of the line).
fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
    hashes > 0 && matches!(trimmed[hashes..].chars().next(), None | Some(' ' | '\t'))
}

/// Measure a span of prose. Words and syllables come from Unicode word
/// segmentation; sentences from [`count_sentences`]. Markdown emphasis markers
/// (`*`, `_`, …) are punctuation to the segmenter, so they never count as words.
/// Pass [`prose`] so heading lines don't inflate the counts.
pub fn stats(text: &str) -> Stats {
    let mut s = Stats { sentences: count_sentences(text), ..Stats::default() };
    for word in text.unicode_words() {
        let syl = syllables(word);
        s.words += 1;
        s.syllables += syl;
        if syl >= 3 {
            s.polysyllables += 1;
        }
    }
    s
}

/// Compute the readability scores from raw counts. Returns all-zero when there
/// is no prose to score (no words or no sentences), so an empty chapter reads as
/// a flat 0 rather than a divide-by-zero.
pub fn readability(s: Stats) -> Readability {
    if s.words == 0 || s.sentences == 0 {
        return Readability::default();
    }
    let words = s.words as f64;
    let sentences = s.sentences as f64;
    let syllables = s.syllables as f64;
    let poly = s.polysyllables as f64;

    let asl = words / sentences; // average sentence length (words per sentence)
    let asw = syllables / words; // average syllables per word

    Readability {
        flesch_reading_ease: 206.835 - 1.015 * asl - 84.6 * asw,
        flesch_kincaid_grade: 0.39 * asl + 11.8 * asw - 15.59,
        gunning_fog: 0.4 * (asl + 100.0 * (poly / words)),
        smog: 1.0430 * (poly * (30.0 / sentences)).sqrt() + 3.1291,
    }
}

/// Estimate the syllables in a single word: count vowel groups (a, e, i, o, u,
/// and y), drop a silent trailing `e` (but keep the `e` in a `-le` ending like
/// "table"), and never return zero for a word with letters. A heuristic — it
/// misses some irregular words, which is acceptable for an aggregate score.
pub fn syllables(word: &str) -> usize {
    // One streaming pass over the word's letters — no intermediate allocation.
    // Track vowel groups, the letter count, and the last two letters (for the
    // silent-`e` / `-le` rule).
    let mut letters = 0usize;
    let mut groups = 0usize;
    let mut prev_vowel = false;
    let mut last = '\0';
    let mut penult = '\0';
    for ch in word.chars().filter(|c| c.is_alphabetic()) {
        let c = ch.to_ascii_lowercase();
        letters += 1;
        let is_vowel = matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');
        if is_vowel && !prev_vowel {
            groups += 1;
        }
        prev_vowel = is_vowel;
        penult = last;
        last = c;
    }
    if letters == 0 {
        return 0;
    }
    if letters <= 3 {
        return 1; // "the", "cat", "eye" — short words are one beat
    }
    // Silent terminal 'e', except a "-le" ending ("table") which keeps its beat.
    if last == 'e' && penult != 'l' {
        groups = groups.saturating_sub(1);
    }
    groups.max(1)
}

/// Count sentences in prose (see [`sentence_spans`] for the segmentation rules).
pub fn count_sentences(text: &str) -> usize {
    sentence_spans(text).len()
}

/// The sentences in `text`, each trimmed of surrounding whitespace, in order.
/// Empty for empty prose. The basis for sentence-length statistics (#68).
pub fn sentences(text: &str) -> Vec<&str> {
    sentence_spans(text).into_iter().map(|(s, e)| text[s..e].trim()).collect()
}

/// Byte spans `(start, end)` of each sentence in `text`. A run of terminators
/// (`.`, `!`, `?`, `…`) ends a sentence when it is followed by the end of the
/// text or by whitespace and then a capital / digit / opening quote — so
/// mid-sentence ellipses ("I... think"), decimals ("3.14"), dialogue tags
/// (`"Stop!" she said.`), and abbreviations ("Dr. Vance") don't over-split.
/// Trailing prose with no terminator is a final sentence.
fn sentence_spans(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut spans = Vec::new();
    let mut start: Option<usize> = None; // index into `chars` of the sentence start
    let mut word_pending = false; // saw word content since the last boundary
    let mut i = 0;
    while i < n {
        let (_, c) = chars[i];
        if start.is_none() && !c.is_whitespace() {
            start = Some(i);
        }
        if is_terminator(c) {
            let run = i;
            while i < n && is_terminator(chars[i].1) {
                i += 1;
            }
            if word_pending && is_boundary(&chars, run, i) {
                let s = chars[start.take().unwrap()].0;
                let e = if i < n { chars[i].0 } else { text.len() };
                spans.push((s, e));
                word_pending = false;
            }
        } else {
            if c.is_alphanumeric() {
                word_pending = true;
            }
            i += 1;
        }
    }
    if word_pending {
        // A final fragment with no closing punctuation.
        let s = start.map_or(text.len(), |k| chars[k].0);
        spans.push((s, text.len()));
    }
    spans
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// Whether the terminator run `chars[start..end]` actually ends a sentence.
fn is_boundary(chars: &[(usize, char)], start: usize, end: usize) -> bool {
    // A lone '.' between digits is a decimal point, not a full stop.
    if end - start == 1 && chars[start].1 == '.' {
        let prev = start.checked_sub(1).map(|j| chars[j].1);
        let next = chars.get(end).map(|x| x.1);
        if matches!(prev, Some(p) if p.is_ascii_digit()) && matches!(next, Some(x) if x.is_ascii_digit()) {
            return false;
        }
        if preceding_is_abbreviation(chars, start) {
            return false;
        }
    }
    // Skip any closing quotes / brackets that trail the terminator.
    let mut j = end;
    while j < chars.len() && matches!(chars[j].1, '"' | '\'' | '”' | '’' | ')' | ']' | '}') {
        j += 1;
    }
    match chars.get(j).map(|x| x.1) {
        None => true, // end of text
        Some(c) if c.is_whitespace() => {
            // A real boundary: the next non-space starts a new sentence (capital,
            // digit, or an opening quote). Lowercase means it ran on.
            match chars[j..].iter().map(|x| x.1).find(|c| !c.is_whitespace()) {
                None | Some('"' | '\'' | '“' | '‘' | '(') => true,
                Some(c) => c.is_uppercase() || c.is_ascii_digit(),
            }
        }
        _ => false, // glued to the next token, e.g. inside "e.g" — not a boundary
    }
}

/// Whether the alphabetic run ending just before `dot` is a known abbreviation.
fn preceding_is_abbreviation(chars: &[(usize, char)], dot: usize) -> bool {
    let mut start = dot;
    while start > 0 && chars[start - 1].1.is_alphabetic() {
        start -= 1;
    }
    if start == dot {
        return false;
    }
    let word: String = chars[start..dot].iter().flat_map(|(_, c)| c.to_lowercase()).collect();
    ABBREVIATIONS.contains(&word.as_str())
}

/// The number of buckets in the sentence-length distribution: `<10`, `10–19`,
/// `20–29`, `30–39`, `40+` words.
pub const LENGTH_BUCKETS: usize = 5;

/// Sentence-length statistics for a span of prose, measured in words per
/// sentence (#68). Additive figures are not — these summarize one set of
/// lengths, so the overall figures come from the concatenated lengths, not from
/// combining per-chapter summaries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SentenceStats {
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    pub min: usize,
    pub max: usize,
    /// Counts per length bucket (see [`LENGTH_BUCKETS`]).
    pub histogram: [usize; LENGTH_BUCKETS],
    /// Sentences longer than the run-on threshold.
    pub run_ons: usize,
}

/// Summarize a set of sentence lengths (words per sentence). `run_on_threshold`
/// is the length above which a sentence is flagged as a possible run-on. All
/// zeros for an empty set.
pub fn sentence_stats(lengths: &[usize], run_on_threshold: usize) -> SentenceStats {
    let s = spread(lengths);
    let mut histogram = [0usize; LENGTH_BUCKETS];
    for &len in lengths {
        histogram[bucket(len)] += 1;
    }
    SentenceStats {
        count: s.count,
        mean: s.mean,
        median: s.median,
        min: s.min,
        max: s.max,
        histogram,
        run_ons: lengths.iter().filter(|&&l| l > run_on_threshold).count(),
    }
}

/// The distribution bucket for a sentence of `len` words.
fn bucket(len: usize) -> usize {
    (len / 10).min(LENGTH_BUCKETS - 1)
}

/// Human labels for the [`LENGTH_BUCKETS`] distribution buckets.
const BUCKET_LABELS: [&str; LENGTH_BUCKETS] = ["<10", "10–19", "20–29", "30–39", "40+"];

/// The median of an already-sorted slice (0.0 for empty).
fn median_sorted(sorted: &[usize]) -> f64 {
    let n = sorted.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        sorted[n / 2] as f64
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) as f64 / 2.0
    }
}

/// The shared core summary of a set of lengths — count, mean, median, and the
/// extremes — computed with a single sort. Every length metric builds on this:
/// sentences add a histogram + run-ons, chapter lengths add a std-dev + outlier
/// fences, paragraphs read it directly (longest == `max`). All-zero for an empty
/// set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Spread {
    count: usize,
    mean: f64,
    median: f64,
    min: usize,
    max: usize,
}

fn spread(lengths: &[usize]) -> Spread {
    if lengths.is_empty() {
        return Spread::default();
    }
    let count = lengths.len();
    let mut sorted = lengths.to_vec();
    sorted.sort_unstable();
    Spread {
        count,
        mean: lengths.iter().sum::<usize>() as f64 / count as f64,
        median: median_sorted(&sorted),
        min: sorted[0],
        max: sorted[count - 1],
    }
}

/// Chapter-length statistics for pacing balance (#69), in words per chapter.
/// `low_fence` / `high_fence` bound the "normal" range: a chapter under the low
/// fence is unusually short, over the high fence unusually long. The fences are
/// median-relative — half and double the median chapter length — which is robust
/// to a single very long/short chapter (a mean- or quartile-based fence gets
/// dragged by the very outliers it should flag) and easy to explain. With fewer
/// than four chapters there is no meaningful "rest" to compare against, so the
/// fences open to ±∞ (nothing is flagged).
#[derive(Clone, Debug, PartialEq)]
pub struct LengthStats {
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    /// Population standard deviation — the "spread".
    pub std_dev: f64,
    pub min: usize,
    pub max: usize,
    pub low_fence: f64,
    pub high_fence: f64,
}

impl Default for LengthStats {
    fn default() -> Self {
        Self { count: 0, mean: 0.0, median: 0.0, std_dev: 0.0, min: 0, max: 0, low_fence: f64::MIN, high_fence: f64::MAX }
    }
}

/// Summarize per-chapter word counts for the pacing section. All-default for an
/// empty manuscript.
pub fn length_stats(words: &[usize]) -> LengthStats {
    let s = spread(words);
    if s.count == 0 {
        return LengthStats::default(); // keeps the ±∞ fences (nothing to flag)
    }
    let variance = words.iter().map(|&w| (w as f64 - s.mean).powi(2)).sum::<f64>() / s.count as f64;
    // Need a few chapters before "unusually short/long relative to the rest"
    // means anything; below that, flag nothing.
    let (low_fence, high_fence) =
        if s.count < 4 { (f64::MIN, f64::MAX) } else { (s.median * 0.5, s.median * 2.0) };
    LengthStats {
        count: s.count,
        mean: s.mean,
        median: s.median,
        std_dev: variance.sqrt(),
        min: s.min,
        max: s.max,
        low_fence,
        high_fence,
    }
}

/// The pacing flag for a chapter of `words`, relative to the rest — `None` when
/// the chapter is within the normal range.
fn length_flag(words: usize, stats: &LengthStats) -> Option<&'static str> {
    let w = words as f64;
    if w < stats.low_fence {
        Some("short")
    } else if w > stats.high_fence {
        Some("long")
    } else {
        None
    }
}

// --- Style flags (#70): deterministic, frequency-based ----------------------
//
// Heuristic and purely statistical — the AI prose/style pass (#79) does the
// context-aware work. These lists are intentionally small: the goal is a rough,
// descriptive signal the author decides on, not a grammar.

/// "To be" forms — the weak-/passive-verb proxy.
const TO_BE: &[&str] = &["am", "is", "are", "was", "were", "be", "been", "being"];

/// Filtering verbs that distance the reader ("she *saw* the door" → "the door").
const FILTER_WORDS: &[&str] = &[
    "see", "saw", "seen", "look", "looked", "watch", "watched", "hear", "heard",
    "feel", "felt", "notice", "noticed", "wonder", "wondered", "realize", "realized",
    "think", "thought", "know", "knew", "seem", "seemed", "decide", "decided",
    "remember", "remembered", "believe", "believed", "sense", "sensed",
];

/// Common `-ly` words that aren't manner adverbs, so they don't inflate the
/// adverb density.
const NON_ADVERB_LY: &[&str] = &[
    "only", "family", "reply", "apply", "supply", "comply", "imply", "rely",
    "multiply", "ally", "bully", "rally", "jelly", "belly", "folly", "lily",
    "fly", "ugly", "early", "holy", "italy", "july", "assembly", "anomaly",
    "melancholy", "fully",
];

/// Frequent function words excluded from the "most-used words" list (so the list
/// surfaces an author's content tics, not "the"/"and"). Defaults baked in for
/// now; a per-project override could join `run_on_threshold` later.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "of", "at", "by", "for", "with",
    "about", "to", "from", "in", "on", "into", "over", "after", "before", "as",
    "is", "are", "was", "were", "be", "been", "being", "am", "do", "does", "did",
    "have", "has", "had", "having", "i", "you", "he", "she", "it", "we", "they",
    "me", "him", "her", "us", "them", "my", "your", "his", "its", "our", "their",
    "this", "that", "these", "those", "there", "here", "then", "than", "so", "not",
    "no", "yes", "up", "down", "out", "off", "all", "any", "some", "what",
    "which", "who", "whom", "when", "where", "why", "how", "would", "could",
    "should", "will", "can", "may", "might", "must", "shall", "just", "very",
    "too", "more", "most", "now", "back", "got", "get", "one", "like", "said",
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
            stopwords: STOPWORDS.iter().map(|s| s.to_string()).collect(),
            filter_words: FILTER_WORDS.iter().map(|s| s.to_string()).collect(),
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

    fn is_stopword(&self, word: &str) -> bool {
        self.stopwords.contains(word)
    }

    fn is_filter_word(&self, word: &str) -> bool {
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
    pub fn add(&mut self, other: StyleFlags) {
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

/// Whether a lowercased word is counted as an `-ly` manner adverb.
fn is_ly_adverb(word: &str) -> bool {
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
    let mut items: Vec<(String, usize)> =
        counts.iter().filter(|&(_, &c)| c >= min).map(|(k, &c)| (k.clone(), c)).collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items.truncate(n);
    items
}

// --- Dialogue balance & paragraph metrics (#71) -----------------------------

/// The split of a chapter's words between quoted dialogue and narration. A
/// heuristic: words inside double-quote spans (straight `"` or curly `“ ”`) count
/// as dialogue, the rest as narration. Single quotes are left alone — they're
/// ambiguous with apostrophes. Unbalanced quotes run to the end of the chapter.
/// Additive across chapters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dialogue {
    pub dialogue_words: usize,
    pub narration_words: usize,
}

impl Dialogue {
    pub fn add(&mut self, other: Dialogue) {
        self.dialogue_words += other.dialogue_words;
        self.narration_words += other.narration_words;
    }

    /// Share of words that are dialogue, as a percent (0 when there are none).
    pub fn dialogue_pct(&self) -> f64 {
        let total = self.dialogue_words + self.narration_words;
        if total == 0 { 0.0 } else { self.dialogue_words as f64 * 100.0 / total as f64 }
    }
}

/// Split a span of prose into dialogue vs narration word counts (see [`Dialogue`]).
pub fn dialogue_split(text: &str) -> Dialogue {
    let mut d = Dialogue::default();
    let mut in_quote = false;
    let mut span_start = 0;
    let flush = |slice: &str, in_quote: bool, d: &mut Dialogue| {
        let w = count_words(slice);
        if in_quote {
            d.dialogue_words += w;
        } else {
            d.narration_words += w;
        }
    };
    for (i, c) in text.char_indices() {
        let next = match c {
            '"' => Some(!in_quote),    // straight quote: ambiguous, so toggle
            '\u{201C}' => Some(true),  // “ opens
            '\u{201D}' => Some(false), // ” closes
            _ => None,
        };
        if let Some(next_in_quote) = next {
            flush(&text[span_start..i], in_quote, &mut d);
            in_quote = next_in_quote;
            span_start = i + c.len_utf8();
        }
    }
    flush(&text[span_start..], in_quote, &mut d);
    d
}

/// The word length of each paragraph (a run of non-blank lines, blank-line
/// delimited). Heading lines are already dropped by [`prose`], so they don't
/// count as paragraphs. The basis for the paragraph-rhythm statistics.
pub fn paragraph_lengths(text: &str) -> Vec<usize> {
    let mut lengths = Vec::new();
    let mut current = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            if current > 0 {
                lengths.push(current);
                current = 0;
            }
        } else {
            current += count_words(line);
        }
    }
    if current > 0 {
        lengths.push(current);
    }
    lengths
}


// --- Vocabulary richness (#72) ----------------------------------------------

/// The window, in words, for the moving-average type–token ratio.
const MATTR_WINDOW: usize = 100;

/// Lexical variety as a moving-average type–token ratio (MATTR) in `0.0..=1.0`:
/// the average fraction of distinct words across every [`MATTR_WINDOW`]-word
/// window. Unlike a plain type–token ratio it doesn't fall just because a span is
/// longer, so chapters are comparable. Spans shorter than a window fall back to
/// the plain ratio. Takes pre-tokenized, lowercased words (via [`tally_tokens`]),
/// so the caller tokenizes once and measures both a chapter (a slice) and the
/// whole manuscript (the pool) with the same definition.
pub fn lexical_variety_tokens(tokens: &[String]) -> f64 {
    mattr(tokens, MATTR_WINDOW)
}

/// Append a span's lowercased word tokens to `out`. The basis for both the
/// per-chapter and the pooled whole-manuscript lexical-variety figures.
pub fn tally_tokens(text: &str, out: &mut Vec<String>) {
    out.extend(text.unicode_words().map(str::to_lowercase));
}

fn mattr(tokens: &[String], window: usize) -> f64 {
    let n = tokens.len();
    if n == 0 {
        return 0.0;
    }
    if n <= window {
        let distinct = tokens.iter().map(String::as_str).collect::<HashSet<_>>().len();
        return distinct as f64 / n as f64;
    }
    // Slide a fixed window across the tokens, tracking the distinct count
    // incrementally (add the entering word, drop the leaving one).
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut distinct = 0usize;
    for t in &tokens[..window] {
        let c = counts.entry(t.as_str()).or_insert(0);
        if *c == 0 {
            distinct += 1;
        }
        *c += 1;
    }
    let mut sum = distinct as f64;
    for i in window..n {
        let leaving = tokens[i - window].as_str();
        let c = counts.get_mut(leaving).unwrap();
        *c -= 1;
        if *c == 0 {
            distinct -= 1;
        }
        let c = counts.entry(tokens[i].as_str()).or_insert(0);
        if *c == 0 {
            distinct += 1;
        }
        *c += 1;
        sum += distinct as f64;
    }
    // Mean of each window's (distinct / window) ratio.
    sum / ((n - window + 1) * window) as f64
}



mod render;
pub use render::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllables_counts_vowel_groups_with_silent_e() {
        assert_eq!(syllables("cat"), 1);
        assert_eq!(syllables("the"), 1);
        assert_eq!(syllables("code"), 1); // silent terminal e
        assert_eq!(syllables("table"), 2); // -le keeps its beat
        assert_eq!(syllables("reading"), 2);
        assert_eq!(syllables("readability"), 5);
        assert_eq!(syllables(""), 0);
    }

    #[test]
    fn syllables_strips_punctuation_and_never_zero_for_letters() {
        assert_eq!(syllables("dark,"), 1);
        assert_eq!(syllables("\"long\""), 1);
        assert!(syllables("rhythm") >= 1);
    }

    #[test]
    fn count_sentences_handles_terminators() {
        assert_eq!(count_sentences("The night was dark. It was long."), 2);
        assert_eq!(count_sentences("Stop! Who goes there? Nobody."), 3);
        // A trailing fragment with no terminator still counts.
        assert_eq!(count_sentences("Just one clause"), 1);
        assert_eq!(count_sentences(""), 0);
    }

    #[test]
    fn count_sentences_ignores_decimals_abbreviations_and_ellipses() {
        assert_eq!(count_sentences("It cost 3.14 dollars and rose."), 1);
        assert_eq!(count_sentences("Dr. Vance arrived late."), 1);
        assert_eq!(count_sentences("I... think so."), 1); // mid-sentence ellipsis
        // Dialogue: the inner '!' is not a boundary, the trailing '.' is.
        assert_eq!(count_sentences("\"Stop!\" she said."), 1);
    }

    #[test]
    fn prose_drops_heading_lines() {
        let text = "# The Dark\n\nThe night was long.\n\n## Later\n\nDawn came.";
        let p = prose(text);
        assert!(!p.contains("The Dark") && !p.contains("Later"), "{p}");
        assert!(p.contains("The night was long.") && p.contains("Dawn came."));
        // A '#' that isn't a heading (no space after) stays.
        assert!(prose("#hashtag of sorts").contains("#hashtag"));
    }

    #[test]
    fn stats_measure_prose_without_headings() {
        let text = "# Title\n\nThe night was dark. It was very long.";
        let s = stats(&prose(text));
        // Heading words are gone; only the two prose sentences are measured.
        assert_eq!(s.words, crate::buffer::count_words("The night was dark. It was very long."));
        assert_eq!(s.sentences, 2);
        assert!(s.syllables >= s.words); // at least one syllable per word
    }

    #[test]
    fn readability_is_zero_for_empty_prose() {
        assert_eq!(readability(Stats::default()), Readability::default());
        // No sentences → no score (avoids divide-by-zero).
        assert_eq!(readability(Stats { words: 5, ..Stats::default() }), Readability::default());
    }

    #[test]
    fn readability_scores_a_simple_passage() {
        // One sentence, all short words → very easy.
        let r = readability(stats("The cat sat on the mat and ran to the man."));
        assert!(r.flesch_reading_ease > 90.0, "ease {}", r.flesch_reading_ease);
        assert!(r.flesch_kincaid_grade < 4.0, "grade {}", r.flesch_kincaid_grade);
    }

    #[test]
    fn dense_prose_scores_harder_than_simple_prose() {
        let simple = readability(stats("The cat sat. The dog ran. We saw them go."));
        let dense = readability(stats(
            "Consequently, the extraordinarily sophisticated bureaucracy \
             perpetuated institutionalized inefficiencies indefinitely.",
        ));
        assert!(dense.flesch_kincaid_grade > simple.flesch_kincaid_grade);
        assert!(dense.gunning_fog > simple.gunning_fog);
        assert!(dense.flesch_reading_ease < simple.flesch_reading_ease);
    }

    #[test]
    fn total_is_the_sum_of_chapters_not_an_average() {
        let a = stats("The cat sat on the mat.");
        let b = stats("The dog ran fast.");
        let mut total = Stats::default();
        total.add(a);
        total.add(b);
        assert_eq!(total.words, a.words + b.words);
        assert_eq!(total.sentences, a.sentences + b.sentences);
    }

    #[test]
    fn sentences_split_and_measure_by_word_length() {
        let s = sentences("The cat sat. The dog ran away fast.");
        assert_eq!(s, vec!["The cat sat.", "The dog ran away fast."]);
        let lengths: Vec<usize> = s.iter().map(|x| crate::buffer::count_words(x)).collect();
        assert_eq!(lengths, vec![3, 5]);
    }

    #[test]
    fn sentence_stats_summarize_lengths() {
        let lengths = vec![3, 5, 5, 50];
        let s = sentence_stats(&lengths, 40);
        assert_eq!(s.count, 4);
        assert_eq!(s.min, 3);
        assert_eq!(s.max, 50);
        assert_eq!(s.median, 5.0); // (5 + 5) / 2
        assert!((s.mean - 15.75).abs() < 1e-9);
        assert_eq!(s.run_ons, 1); // only the 50-word sentence is over 40
        assert_eq!(s.histogram, [3, 0, 0, 0, 1]); // three <10, one 40+
        assert_eq!(sentence_stats(&[], 40), SentenceStats::default());
    }

    #[test]
    fn longest_sentences_keeps_a_bounded_top_n() {
        let mut top = LongestSentences::new(2);
        top.consider("One", "short", 3);
        top.consider("One", "medium length here", 10);
        top.consider("Two", "the longest of them all by far", 25);
        top.consider("Two", "also short", 4);
        let got = top.into_vec();
        assert_eq!(got.len(), 2, "capped at 2");
        assert_eq!(got[0].words, 25); // longest first
        assert_eq!(got[1].words, 10);
        assert_eq!(got[0].chapter, "Two");
    }

    #[test]
    fn length_stats_summarize_and_fence_outliers() {
        // Mostly ~1000-word chapters with one tiny and one huge outlier.
        let words = vec![1000, 1050, 980, 1020, 1010, 100, 5000];
        let s = length_stats(&words);
        assert_eq!(s.count, 7);
        assert_eq!(s.min, 100);
        assert_eq!(s.max, 5000);
        assert_eq!(s.median, 1010.0);
        assert!(s.std_dev > 0.0);
        assert_eq!(length_flag(100, &s), Some("short"));
        assert_eq!(length_flag(5000, &s), Some("long"));
        assert_eq!(length_flag(1000, &s), None);
        // Too few chapters → no "rest" to compare against, nothing flagged.
        let few = length_stats(&[100, 5000]);
        assert_eq!(length_flag(100, &few), None);
    }

    #[test]
    fn style_flags_count_adverbs_filters_and_weak_verbs() {
        // "slowly" / "quietly" = -ly adverbs; "only" is excluded. "saw"/"felt" are
        // filter words. "was" is a to-be form; "was locked" (to-be + -ed) is a
        // passive estimate.
        let f = style_flags("She slowly saw the only door and felt it was locked quietly.", &WordLists::new());
        assert_eq!(f.adverbs_ly, 2, "slowly + quietly, not only");
        assert_eq!(f.filter_words, 2, "saw + felt");
        assert_eq!(f.weak_verbs, 1, "was");
        assert_eq!(f.passive, 1, "was locked");
    }

    #[test]
    fn content_words_skip_stopwords_and_short_words() {
        let mut counts = HashMap::new();
        tally_content_words("The locket and the locket on it", &mut counts, &WordLists::new());
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
        assert_eq!(counts.get("out of the"), None, "all-stopword trigram skipped");
    }

    #[test]
    fn stopword_overrides_extend_and_drop_defaults() {
        let mut lists = WordLists::new();
        // "said" is a built-in stopword — remove it so it can surface; add "locket".
        lists.override_stopwords(&["locket".to_string()], &["Said".to_string()]); // remove is case-insensitive
        let mut counts = HashMap::new();
        tally_content_words("said said locket locket gleamed", &mut counts, &lists);
        assert_eq!(counts.get("said"), Some(&2), "removed from stopwords → now counted");
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
        assert_eq!(a.get("the"), None, "built-in stopword still excluded by default");
    }

    #[test]
    fn top_counts_orders_by_count_then_name() {
        let counts = HashMap::from([
            ("beta".to_string(), 3),
            ("alpha".to_string(), 3),
            ("rare".to_string(), 1),
        ]);
        // count desc, ties alphabetical; min filters out the single occurrence.
        assert_eq!(top_counts(&counts, 10, 2), vec![("alpha".to_string(), 3), ("beta".to_string(), 3)]);
    }

    #[test]
    fn dialogue_split_counts_quoted_words_as_dialogue() {
        // 4 dialogue words inside the quotes, 4 narration words outside.
        let d = dialogue_split("\"Get out now,\" she said to him calmly.");
        assert_eq!(d.dialogue_words, 3); // Get out now
        assert_eq!(d.narration_words, 5); // she said to him calmly
        assert!((d.dialogue_pct() - 37.5).abs() < 1e-9);
    }

    #[test]
    fn dialogue_split_handles_curly_quotes() {
        let d = dialogue_split("\u{201C}Hello there\u{201D} he replied.");
        assert_eq!(d.dialogue_words, 2); // Hello there
        assert_eq!(d.narration_words, 2); // he replied
        assert_eq!(dialogue_split("plain narration only"), Dialogue { dialogue_words: 0, narration_words: 3 });
    }

    #[test]
    fn paragraph_lengths_split_on_blank_lines() {
        let text = "One two three.\nstill same para.\n\nSecond para here.\n\n\nThird.";
        assert_eq!(paragraph_lengths(text), vec![6, 3, 1]);
        assert!(paragraph_lengths("   \n\n  ").is_empty());
    }

    #[test]
    fn spread_reports_count_mean_median_extremes() {
        let s = spread(&[10, 20, 30, 100]);
        assert_eq!((s.count, s.mean, s.median, s.min, s.max), (4, 40.0, 25.0, 10, 100));
        assert_eq!(spread(&[]), Spread::default());
    }

    /// Lexical variety of a raw string (tokenize-then-measure), for tests.
    fn variety(text: &str) -> f64 {
        let mut tokens = Vec::new();
        tally_tokens(text, &mut tokens);
        lexical_variety_tokens(&tokens)
    }

    #[test]
    fn lexical_variety_falls_back_to_plain_ttr_for_short_text() {
        // 5 tokens, 4 distinct ("the" twice) → 0.8; under the window, so plain TTR.
        assert!((variety("the cat the dog runs") - 0.8).abs() < 1e-9);
        assert_eq!(variety(""), 0.0);
        assert_eq!(variety("word word word"), 1.0 / 3.0);
    }

    #[test]
    fn mattr_is_length_independent_and_rewards_variety() {
        // All-unique tokens → every window is fully distinct → 1.0.
        let varied: Vec<String> = (0..500).map(|i| format!("w{i}")).collect();
        assert!((mattr(&varied, 100) - 1.0).abs() < 1e-9);
        // Heavy repetition → low ratio, and (the point of MATTR) doubling the
        // length doesn't change the score.
        let pattern: Vec<String> = "a b a c a d a e a f".split(' ').map(String::from).collect();
        let short: Vec<String> = pattern.iter().cloned().cycle().take(200).collect();
        let long: Vec<String> = pattern.iter().cloned().cycle().take(400).collect();
        assert!((mattr(&short, 100) - mattr(&long, 100)).abs() < 1e-9);
        assert!(mattr(&short, 100) < 0.7);
    }

}

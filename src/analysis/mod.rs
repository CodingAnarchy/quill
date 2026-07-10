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
use crate::export::is_thematic_break;

mod sentence;
pub use sentence::{count_sentences, sentence_byte_spans, sentences};

mod style;
pub use style::{
    style_flags, tally_content_words, tally_phrases, top_counts, StyleFlags, WordLists,
};

mod flags;
pub use flags::{flag_ranges, FlagCategory};

mod proximity;
pub use proximity::proximity_repeats;

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
    pub const fn add(&mut self, other: Self) {
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

/// A chapter's prose with Markdown heading lines and thematic-break (scene-break)
/// lines removed, so the analysis measures what a reader actually reads. A chapter
/// title (`# The Dark`) or a scene heading (`## Later`) is structure, not a
/// sentence — left in, its words glue onto the following sentence and skew the
/// length and readability figures. A thematic break (`***`, `---`, …) is likewise
/// structure: left in, its marks sit mid-sentence and fuse the paragraph before it
/// to the one after (#910). Both become blank lines, which also separates the
/// surrounding paragraphs so the sentence splitter closes across the break. Every
/// other line is kept verbatim. Front matter is already stripped upstream.
pub fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if !is_heading(line) && !is_thematic_break(line) {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Whether `line` is a Markdown ATX heading (`#`…`######` followed by a space or
/// the end of the line). Visible within the (private) analysis module so the
/// flag-sweep scanner (#1001) masks the same structural lines [`prose`] drops.
pub fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
    hashes > 0 && matches!(trimmed[hashes..].chars().next(), None | Some(' ' | '\t'))
}

/// Measure a span of prose. Words and syllables come from Unicode word
/// segmentation; sentences from [`count_sentences`]. Markdown emphasis markers
/// (`*`, `_`, …) are punctuation to the segmenter, so they never count as words.
/// Pass [`prose`] so heading lines don't inflate the counts.
pub fn stats(text: &str) -> Stats {
    let mut s = Stats {
        sentences: count_sentences(text),
        ..Stats::default()
    };
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
        flesch_reading_ease: 84.6f64.mul_add(-asw, 1.015f64.mul_add(-asl, 206.835)),
        flesch_kincaid_grade: 0.39f64.mul_add(asl, 11.8 * asw) - 15.59,
        gunning_fog: 0.4 * 100.0f64.mul_add(poly / words, asl),
        smog: 1.0430f64.mul_add((poly * (30.0 / sentences)).sqrt(), 3.1291),
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
        Self {
            count: 0,
            mean: 0.0,
            median: 0.0,
            std_dev: 0.0,
            min: 0,
            max: 0,
            low_fence: f64::MIN,
            high_fence: f64::MAX,
        }
    }
}

/// Summarize per-chapter word counts for the pacing section. All-default for an
/// empty manuscript.
pub fn length_stats(words: &[usize]) -> LengthStats {
    let s = spread(words);
    if s.count == 0 {
        return LengthStats::default(); // keeps the ±∞ fences (nothing to flag)
    }
    let variance = words
        .iter()
        .map(|&w| (w as f64 - s.mean).powi(2))
        .sum::<f64>()
        / s.count as f64;
    // Need a few chapters before "unusually short/long relative to the rest"
    // means anything; below that, flag nothing.
    let (low_fence, high_fence) = if s.count < 4 {
        (f64::MIN, f64::MAX)
    } else {
        (s.median * 0.5, s.median * 2.0)
    };
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
    pub const fn add(&mut self, other: Self) {
        self.dialogue_words += other.dialogue_words;
        self.narration_words += other.narration_words;
    }

    /// Share of words that are dialogue, as a percent (0 when there are none).
    pub fn dialogue_pct(&self) -> f64 {
        let total = self.dialogue_words + self.narration_words;
        if total == 0 {
            0.0
        } else {
            self.dialogue_words as f64 * 100.0 / total as f64
        }
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
        let distinct = tokens
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>()
            .len();
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
        // Every token that enters the window was counted in, so one leaving it is
        // always present in the map; `if let` binds it without a panic path.
        if let Some(c) = counts.get_mut(leaving) {
            *c -= 1;
            if *c == 0 {
                distinct -= 1;
            }
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
mod tests;

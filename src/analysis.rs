//! Local, offline manuscript analysis: deterministic editing metrics computed
//! from the prose itself, with no network access. This first slice (#67) is the
//! readability surface — Flesch Reading Ease, Flesch–Kincaid grade, Gunning Fog,
//! and SMOG — plus the shared text-stats primitives (sentence segmentation and a
//! syllable estimator) that the rest of the analysis epic (#66) reuses.
//!
//! The formulas are documented in `docs/manuscript-analysis.md`. The word basis
//! matches the progress report's notion of prose words (`buffer::count_words` /
//! Unicode word segmentation), so the two reports agree on how big a chapter is.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

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
    if lengths.is_empty() {
        return SentenceStats::default();
    }
    let count = lengths.len();
    let sum: usize = lengths.iter().sum();
    let mut sorted = lengths.to_vec();
    sorted.sort_unstable();
    let mut histogram = [0usize; LENGTH_BUCKETS];
    for &len in lengths {
        histogram[bucket(len)] += 1;
    }
    SentenceStats {
        count,
        mean: sum as f64 / count as f64,
        median: median_sorted(&sorted),
        min: sorted[0],
        max: sorted[count - 1],
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
    if words.is_empty() {
        return LengthStats::default();
    }
    let count = words.len();
    let mean = words.iter().sum::<usize>() as f64 / count as f64;
    let variance = words.iter().map(|&w| (w as f64 - mean).powi(2)).sum::<f64>() / count as f64;
    let mut sorted = words.to_vec();
    sorted.sort_unstable();
    let median = median_sorted(&sorted);
    // Need a few chapters before "unusually short/long relative to the rest"
    // means anything; below that, flag nothing.
    let (low_fence, high_fence) =
        if count < 4 { (f64::MIN, f64::MAX) } else { (median * 0.5, median * 2.0) };
    LengthStats {
        count,
        mean,
        median,
        std_dev: variance.sqrt(),
        min: sorted[0],
        max: sorted[count - 1],
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

/// One chapter's analysis inputs: its title, total word count (the rolled-up
/// count, for the pacing section), readability [`Stats`], and the word length of
/// each of its sentences. Gathered by the editor (which owns the manuscript) and
/// handed to [`lines`] to render.
pub struct ChapterAnalysis {
    pub title: String,
    pub words: usize,
    pub stats: Stats,
    pub sentence_lengths: Vec<usize>,
}

/// One part (chapter grouping) for the pacing section: its display name and the
/// 0-based chapter index range it spans (`start..end`, end-exclusive).
pub struct PartSpan {
    pub name: String,
    pub range: std::ops::Range<usize>,
}

/// One of the manuscript's longest sentences, with where it lives, for the
/// run-on list.
pub struct LongSentence {
    pub chapter: String,
    pub words: usize,
    pub text: String,
}

/// A bounded "top-N longest sentences" accumulator, so ranking the longest
/// sentences across the whole manuscript never holds more than N at once and
/// only copies a sentence's text when it actually makes the cut.
pub struct LongestSentences {
    cap: usize,
    items: Vec<LongSentence>, // kept sorted by `words`, descending
}

impl LongestSentences {
    pub fn new(cap: usize) -> Self {
        Self { cap, items: Vec::with_capacity(cap + 1) }
    }

    /// Offer a sentence; kept only if it ranks among the longest seen so far.
    pub fn consider(&mut self, chapter: &str, text: &str, words: usize) {
        if self.items.len() < self.cap || words > self.items.last().map_or(0, |l| l.words) {
            let pos = self.items.partition_point(|l| l.words >= words);
            self.items.insert(pos, LongSentence { chapter: chapter.to_string(), words, text: text.to_string() });
            self.items.truncate(self.cap);
        }
    }

    /// The longest sentences, longest first.
    pub fn into_vec(self) -> Vec<LongSentence> {
        self.items
    }
}

/// Everything the analysis report renders, gathered by the editor. Grouping the
/// inputs keeps [`lines`] to one argument as the epic adds more sections.
pub struct Report<'a> {
    pub title: &'a str,
    pub chapters: &'a [ChapterAnalysis],
    pub longest: &'a [LongSentence],
    /// Chapter groupings for the pacing section (empty when the book has none).
    pub parts: &'a [PartSpan],
    pub run_on_threshold: usize,
}

/// The analysis report's rows: a readability table, a sentence-length section
/// (per-chapter statistics, the overall distribution, and the longest
/// sentences), and a chapter-length / pacing section — each with a
/// whole-manuscript total. Computed once when the report is opened (the scan is
/// O(manuscript), so it stays off the per-frame path).
pub fn lines(report: &Report) -> Vec<Line<'static>> {
    let mut out = vec![bold(format!("{} — analysis", report.title)), Line::raw("")];
    out.extend(readability_section(report.chapters));
    out.push(Line::raw(""));
    out.extend(sentence_section(report.chapters, report.longest, report.run_on_threshold));
    out.push(Line::raw(""));
    out.extend(length_section(report.chapters, report.parts));
    out
}

/// The readability table: one row per chapter plus a whole-manuscript total.
fn readability_section(chapters: &[ChapterAnalysis]) -> Vec<Line<'static>> {
    let mut out = vec![
        dim("Readability"),
        dim(format!("{:<26}{:>9}{:>8}{:>8}{:>8}", "Chapter", "Ease", "F–K", "Fog", "SMOG")),
    ];
    let mut total = Stats::default();
    for c in chapters {
        total.add(c.stats);
        out.push(Line::raw(score_text(&c.title, readability(c.stats))));
    }
    out.push(bold(score_text("Whole manuscript", readability(total))));
    out.push(dim("Reading Ease: higher is easier (60–70 ≈ plain English); the rest are grades."));
    out
}

/// The sentence-length section: per-chapter statistics, the overall distribution,
/// and the longest sentences (possible run-ons).
fn sentence_section(
    chapters: &[ChapterAnalysis],
    longest: &[LongSentence],
    threshold: usize,
) -> Vec<Line<'static>> {
    let mut out = vec![
        dim(format!("Sentence length (words) · run-on threshold {threshold}")),
        dim(format!(
            "{:<26}{:>6}{:>7}{:>7}{:>6}{:>6}{:>9}",
            "Chapter", "Sent", "Mean", "Med", "Min", "Max", "Run-ons"
        )),
    ];
    let mut all = Vec::new();
    for c in chapters {
        all.extend_from_slice(&c.sentence_lengths);
        out.push(Line::raw(sentence_text(&c.title, &sentence_stats(&c.sentence_lengths, threshold))));
    }
    let overall = sentence_stats(&all, threshold);
    out.push(bold(sentence_text("Whole manuscript", &overall)));

    out.push(Line::raw(""));
    out.push(dim("Distribution (whole manuscript)"));
    out.extend(histogram_lines(&overall));

    if !longest.is_empty() {
        out.push(Line::raw(""));
        out.push(dim("Longest sentences"));
        for s in longest {
            let flag = if s.words > threshold { " ⚠" } else { "" };
            out.push(Line::raw(format!(
                "{:>4}w{flag}  [{}] {}",
                s.words,
                truncate(&s.chapter, 18),
                truncate(&s.text, 64)
            )));
        }
    }
    out
}

/// The chapter-length / pacing section: a per-chapter bar chart of word counts
/// with short/long outliers flagged, grouped by part when the book defines them,
/// plus summary statistics (mean, median, spread). Uses the rolled-up chapter
/// word counts (headings included — the size shown in the chapter list), so it
/// matches what the author sees elsewhere.
fn length_section(chapters: &[ChapterAnalysis], parts: &[PartSpan]) -> Vec<Line<'static>> {
    let words: Vec<usize> = chapters.iter().map(|c| c.words).collect();
    let stats = length_stats(&words);
    let peak = stats.max.max(1); // the longest chapter (≥1 so the bar never divides by zero)
    let mut out = vec![dim("Chapter length (words)")];

    if parts.is_empty() {
        for c in chapters {
            out.push(length_bar_row(&c.title, c.words, peak, &stats, 0));
        }
    } else {
        for p in parts {
            out.push(dim(p.name.clone()));
            let mut subtotal = 0;
            for c in p.range.clone().filter_map(|i| chapters.get(i)) {
                subtotal += c.words;
                out.push(length_bar_row(&c.title, c.words, peak, &stats, 2));
            }
            out.push(dim(format!("  {:<22}{subtotal:>7}", "Subtotal")));
        }
    }

    out.push(bold(format!(
        "Whole manuscript: {} chapters · mean {:.0} · median {:.0} · spread ±{:.0} · range {}–{}",
        stats.count, stats.mean, stats.median, stats.std_dev, stats.min, stats.max
    )));
    out
}

/// One chapter's pacing row: indented (truncated) title, word count, a bar scaled
/// to the longest chapter, and a short/long flag for outliers.
fn length_bar_row(title: &str, words: usize, peak: usize, stats: &LengthStats, indent: usize) -> Line<'static> {
    let avail = 24 - indent; // the indent + name region together pad to 24
    let name = truncate(title, avail);
    let flag = length_flag(words, stats).map_or(String::new(), |f| format!("  ⚠ {f}"));
    Line::raw(format!("{:indent$}{name:<avail$}{words:>7}  {}{flag}", "", bar(words, peak)))
}

/// A horizontal bar per length bucket, scaled to the busiest bucket.
fn histogram_lines(stats: &SentenceStats) -> Vec<Line<'static>> {
    let peak = stats.histogram.iter().copied().max().unwrap_or(0).max(1);
    BUCKET_LABELS
        .iter()
        .zip(stats.histogram)
        .map(|(label, count)| Line::raw(format!("{label:>6} {} {count}", bar(count, peak))))
        .collect()
}

/// Width of the bar-chart bars, in cells.
const BAR_WIDTH: usize = 24;

/// A horizontal bar of `value` scaled to `peak` (the longest bar is full width).
fn bar(value: usize, peak: usize) -> String {
    "█".repeat(value * BAR_WIDTH / peak)
}

/// One readability row: a (truncated) name and the four scores to one decimal.
fn score_text(name: &str, r: Readability) -> String {
    let name = truncate(name, 24);
    format!(
        "{name:<26}{:>9.1}{:>8.1}{:>8.1}{:>8.1}",
        r.flesch_reading_ease, r.flesch_kincaid_grade, r.gunning_fog, r.smog
    )
}

/// One sentence-length row: a (truncated) name and the length statistics.
fn sentence_text(name: &str, s: &SentenceStats) -> String {
    let name = truncate(name, 24);
    format!(
        "{name:<26}{:>6}{:>7.1}{:>7.1}{:>6}{:>6}{:>9}",
        s.count, s.mean, s.median, s.min, s.max, s.run_ons
    )
}

/// A bold line and a dim (dark-gray) line, the two row styles the report uses.
fn bold(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), Style::default().add_modifier(Modifier::BOLD)))
}

fn dim(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(text.into(), Style::default().fg(Color::DarkGray)))
}

/// Truncate to `max` characters with an ellipsis, counting by character so a
/// multibyte name isn't split mid-codepoint.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

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

    fn chapter(title: &str, prose_text: &str, words: usize) -> ChapterAnalysis {
        ChapterAnalysis {
            title: title.to_string(),
            words,
            stats: stats(prose_text),
            sentence_lengths: sentences(prose_text)
                .iter()
                .map(|s| crate::buffer::count_words(s))
                .collect(),
        }
    }

    #[test]
    fn lines_render_all_sections_with_totals() {
        let chapters = vec![
            chapter("One", "The cat sat on the mat.", 1200),
            chapter("Two", "The dog ran fast across the yard.", 3400),
        ];
        let longest = vec![LongSentence {
            chapter: "Two".to_string(),
            words: 7,
            text: "The dog ran fast across the yard.".to_string(),
        }];
        let report = Report {
            title: "My Book",
            chapters: &chapters,
            longest: &longest,
            parts: &[],
            run_on_threshold: 40,
        };
        let rows = lines(&report);
        let text: String = rows
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("My Book — analysis"));
        assert!(text.contains("Readability"));
        assert!(text.contains("Sentence length"));
        assert!(text.contains("Distribution"));
        assert!(text.contains("Longest sentences"));
        assert!(text.contains("Chapter length"));
        assert!(text.contains("One") && text.contains("Two"));
        assert_eq!(text.matches("Whole manuscript").count(), 3); // one per section
    }

    #[test]
    fn lines_group_chapter_length_by_part() {
        let chapters = vec![
            chapter("One", "A.", 1000),
            chapter("Two", "B.", 1100),
            chapter("Three", "C.", 900),
        ];
        let parts = vec![
            PartSpan { name: "Act I".to_string(), range: 0..1 },
            PartSpan { name: "Act II".to_string(), range: 1..3 },
        ];
        let report = Report {
            title: "Book",
            chapters: &chapters,
            longest: &[],
            parts: &parts,
            run_on_threshold: 40,
        };
        let text: String = lines(&report)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Act I") && text.contains("Act II"));
        assert!(text.contains("Subtotal"));
    }

    #[test]
    fn truncate_keeps_multibyte_names_intact() {
        let long = "Café ".repeat(10);
        let t = truncate(&long, 24);
        assert!(t.chars().count() <= 24);
        assert!(t.ends_with('…'));
    }
}

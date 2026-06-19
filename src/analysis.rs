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

/// Measure a span of prose. Words and syllables come from Unicode word
/// segmentation (so the count matches the progress report); sentences from
/// [`count_sentences`]. Markdown markup (`#`, `*`, `_`, …) is punctuation to the
/// segmenter, so it never counts as a word — only the prose does.
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

/// Count sentences in prose. A run of terminators (`.`, `!`, `?`, `…`) ends a
/// sentence when it is followed by the end of the text or by whitespace and then
/// a capital / opening quote — so mid-sentence ellipses ("I... think"), decimals
/// ("3.14"), dialogue tags (`"Stop!" she said.`), and abbreviations ("Dr.
/// Vance") don't over-count. Trailing prose with no terminator counts as one
/// final sentence.
pub fn count_sentences(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut count = 0;
    let mut word_pending = false; // saw word content since the last boundary
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if is_terminator(c) {
            let start = i;
            while i < n && is_terminator(chars[i]) {
                i += 1;
            }
            if word_pending && is_boundary(&chars, start, i) {
                count += 1;
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
        count += 1; // a final fragment with no closing punctuation
    }
    count
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// Whether the terminator run `chars[start..end]` actually ends a sentence.
fn is_boundary(chars: &[char], start: usize, end: usize) -> bool {
    // A lone '.' between digits is a decimal point, not a full stop.
    if end - start == 1 && chars[start] == '.' {
        let prev = start.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(end).copied();
        if matches!(prev, Some(p) if p.is_ascii_digit()) && matches!(next, Some(x) if x.is_ascii_digit()) {
            return false;
        }
        if preceding_is_abbreviation(chars, start) {
            return false;
        }
    }
    // Skip any closing quotes / brackets that trail the terminator.
    let mut j = end;
    while j < chars.len() && matches!(chars[j], '"' | '\'' | '”' | '’' | ')' | ']' | '}') {
        j += 1;
    }
    match chars.get(j) {
        None => true, // end of text
        Some(c) if c.is_whitespace() => {
            // A real boundary: the next non-space starts a new sentence (capital,
            // digit, or an opening quote). Lowercase means it ran on.
            match chars[j..].iter().find(|c| !c.is_whitespace()) {
                None | Some('"' | '\'' | '“' | '‘' | '(') => true,
                Some(c) => c.is_uppercase() || c.is_ascii_digit(),
            }
        }
        _ => false, // glued to the next token, e.g. inside "e.g" — not a boundary
    }
}

/// Whether the alphabetic run ending just before `dot` is a known abbreviation.
fn preceding_is_abbreviation(chars: &[char], dot: usize) -> bool {
    let mut start = dot;
    while start > 0 && chars[start - 1].is_alphabetic() {
        start -= 1;
    }
    if start == dot {
        return false;
    }
    let word: String = chars[start..dot].iter().flat_map(|c| c.to_lowercase()).collect();
    ABBREVIATIONS.contains(&word.as_str())
}

/// The analysis report's rows: a readability table with one row per chapter and a
/// whole-manuscript total. Computed once when the report is opened (the scan is
/// O(manuscript), so it stays off the per-frame path).
pub fn lines(title: &str, chapters: &[(String, Stats)]) -> Vec<Line<'static>> {
    let mut out = vec![
        Line::from(Span::styled(
            format!("{title} — readability"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        header_row(),
    ];

    let mut total = Stats::default();
    for (name, stats) in chapters {
        total.add(*stats);
        out.push(Line::raw(score_text(name, readability(*stats))));
    }

    out.push(Line::raw(""));
    out.push(Line::from(Span::styled(
        score_text("Whole manuscript", readability(total)),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    out.push(Line::raw(""));
    out.push(Line::from(Span::styled(
        "Reading Ease: higher is easier (60–70 ≈ plain English). The rest are U.S. grade levels.",
        Style::default().fg(Color::DarkGray),
    )));
    out
}

fn header_row() -> Line<'static> {
    Line::from(Span::styled(
        format!("{:<26}{:>9}{:>8}{:>8}{:>8}", "Chapter", "Ease", "F–K", "Fog", "SMOG"),
        Style::default().fg(Color::DarkGray),
    ))
}

/// One formatted row: a (truncated) name and the four scores to one decimal.
fn score_text(name: &str, r: Readability) -> String {
    let name = truncate(name, 24);
    format!(
        "{name:<26}{:>9.1}{:>8.1}{:>8.1}{:>8.1}",
        r.flesch_reading_ease, r.flesch_kincaid_grade, r.gunning_fog, r.smog
    )
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
    fn stats_match_the_word_count_basis() {
        let text = "# Title\n\nThe night was dark. It was very long.";
        let s = stats(text);
        // "Title The night was dark It was very long" = 9 words (heading text
        // included, matching the progress report's count_words basis).
        assert_eq!(s.words, crate::buffer::count_words(text));
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
    fn lines_render_a_row_per_chapter_plus_a_total() {
        let chapters = vec![
            ("One".to_string(), stats("The cat sat on the mat.")),
            ("Two".to_string(), stats("The dog ran fast across the yard.")),
        ];
        let rows = lines("My Book", &chapters);
        let text: String = rows
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("My Book — readability"));
        assert!(text.contains("One"));
        assert!(text.contains("Two"));
        assert!(text.contains("Whole manuscript"));
    }

    #[test]
    fn truncate_keeps_multibyte_names_intact() {
        let long = "Café ".repeat(10);
        let t = truncate(&long, 24);
        assert!(t.chars().count() <= 24);
        assert!(t.ends_with('…'));
    }
}

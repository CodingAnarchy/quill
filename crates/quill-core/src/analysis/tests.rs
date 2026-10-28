//! Tests for the `src/analysis/mod.rs` module, split out to keep the module under the
//! ~1,000-line convention (#404). Pure move — no behavior change.

// The stats assertions compare exact, hand-computed values (medians, TTR ratios,
// `0.0` sentinels) produced by deterministic integer/float arithmetic — an exact
// `assert_eq!` is the intended check, not a fragile equality.
#![allow(clippy::float_cmp)]

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
fn prose_drops_heading_lines() {
    let text = "# The Dark\n\nThe night was long.\n\n## Later\n\nDawn came.";
    let p = prose(text);
    assert!(!p.contains("The Dark") && !p.contains("Later"), "{p}");
    assert!(p.contains("The night was long.") && p.contains("Dawn came."));
    // A '#' that isn't a heading (no space after) stays.
    assert!(prose("#hashtag of sorts").contains("#hashtag"));
}

#[test]
fn prose_blanks_thematic_break_lines() {
    // A scene break must not sit mid-sentence and fuse the surrounding paragraphs
    // (#910); `prose()` blanks it, leaving the paragraphs separated.
    for br in ["***", "---", "* * *", "___"] {
        let p = prose(&format!("She left.\n\n{br}\n\nHe stayed."));
        assert!(!p.contains(br), "{br:?} should be blanked: {p:?}");
        assert!(p.contains("She left.") && p.contains("He stayed."));
    }
    // "5 * 3" is prose, not a break — it stays.
    assert!(prose("The answer is 5 * 3 = 15.").contains("5 * 3"));
}

#[test]
fn stats_measure_prose_without_headings() {
    let text = "# Title\n\nThe night was dark. It was very long.";
    let s = stats(&prose(text));
    // Heading words are gone; only the two prose sentences are measured.
    assert_eq!(
        s.words,
        crate::buffer::count_words("The night was dark. It was very long.")
    );
    assert_eq!(s.sentences, 2);
    assert!(s.syllables >= s.words); // at least one syllable per word
}

#[test]
fn readability_is_zero_for_empty_prose() {
    assert_eq!(readability(Stats::default()), Readability::default());
    // No sentences → no score (avoids divide-by-zero).
    assert_eq!(
        readability(Stats {
            words: 5,
            ..Stats::default()
        }),
        Readability::default()
    );
}

#[test]
fn readability_scores_a_simple_passage() {
    // One sentence, all short words → very easy.
    let r = readability(stats("The cat sat on the mat and ran to the man."));
    assert!(
        r.flesch_reading_ease > 90.0,
        "ease {}",
        r.flesch_reading_ease
    );
    assert!(
        r.flesch_kincaid_grade < 4.0,
        "grade {}",
        r.flesch_kincaid_grade
    );
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
    assert_eq!(
        dialogue_split("plain narration only"),
        Dialogue {
            dialogue_words: 0,
            narration_words: 3
        }
    );
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
    assert_eq!(
        (s.count, s.mean, s.median, s.min, s.max),
        (4, 40.0, 25.0, 10, 100)
    );
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

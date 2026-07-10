//! Locatable style flags (#1001): the byte spans, *inside the chapter buffer*, of
//! each occurrence the analysis report counts — the adverbs, filter words, weak /
//! passive verbs, over-long sentences, and over-used words. The report gives an
//! author a count (40 passives); this gives them the positions, so the editor can
//! underline each in place and step the cursor from one to the next.
//!
//! It only *observes*: it returns spans for the editor to mark and navigate, and
//! Quill never edits the flagged prose (within the AI/boundary rules) — the author
//! makes every change. It reuses the report's own heuristics ([`style_flags`],
//! [`sentence_byte_spans`], the frequency tallies) so a flag and its count always
//! agree, and it runs off the keystroke path (the caller computes it on the idle
//! tick and caches the result, like the spell pass).
//!
//! Offsets are preserved against the *raw* chapter text, not [`prose`]'s cleaned
//! copy (which drops heading/scene-break content and so shifts every later byte).
//! Instead the scanner masks those structural lines in place — replacing their
//! bytes with spaces of equal length — so a span still addresses the buffer the
//! cursor lives in, while a chapter title or `***` divider is never flagged.

use std::collections::HashMap;

use unicode_segmentation::UnicodeSegmentation;

use super::style::{is_ly_adverb, TO_BE};
use super::{is_heading, sentence_byte_spans, WordLists};
use crate::buffer::count_words;
use crate::export::is_thematic_break;

/// A word must appear at least this many times in a chapter before every one of
/// its occurrences is flagged as over-used (#1001). Matches the report's instinct
/// that a repeated content word is a revision signal; kept modest so the sweep
/// lands on real tics rather than ordinary recurrence.
const OVERUSE_MIN: usize = 4;

/// The length, in words, past which a sentence is flagged as a long / run-on
/// sentence when no per-project `run_on_threshold` is supplied. Mirrors the
/// manifest default so an unconfigured project's sweep matches its report.
const DEFAULT_RUN_ON: usize = 40;

/// A category of style flag the author can sweep through (#1001) — the axes the
/// analysis report already measures, now made locatable. Each maps to one pass of
/// [`flag_ranges`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagCategory {
    /// `-ly` manner adverbs.
    Adverbs,
    /// Filtering verbs that distance the reader ("she *saw*…").
    FilterWords,
    /// "To be" forms — the weak-verb proxy.
    WeakVerbs,
    /// Likely passive constructions ("was held").
    Passive,
    /// Sentences past the run-on length threshold.
    LongSentences,
    /// Content words repeated often enough to read as a tic.
    OverusedWords,
}

impl FlagCategory {
    /// Every category, in the order the picker offers them.
    pub const ALL: &'static [Self] = &[
        Self::Adverbs,
        Self::FilterWords,
        Self::WeakVerbs,
        Self::Passive,
        Self::LongSentences,
        Self::OverusedWords,
    ];

    /// The human label shown in the picker and the status message.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Adverbs => "Adverbs (-ly)",
            Self::FilterWords => "Filter words",
            Self::WeakVerbs => "Weak verbs (to be)",
            Self::Passive => "Passive voice",
            Self::LongSentences => "Long sentences",
            Self::OverusedWords => "Over-used words",
        }
    }
}

/// The byte spans of every occurrence of `category` in `text` (#1001), in
/// ascending order, addressing `text`'s own bytes. `lists` supplies the filter /
/// stopword sets (with the project's `[analysis]` overrides), so a flag matches
/// what the report counts; `run_on_threshold` is the long-sentence cutoff (`None`
/// ⇒ the [default](DEFAULT_RUN_ON)). Pure and allocation-bounded: the caller runs
/// it on the idle tick and caches the result, never per keystroke or per draw.
pub fn flag_ranges(
    text: &str,
    category: FlagCategory,
    lists: &WordLists,
    run_on_threshold: Option<usize>,
) -> Vec<(usize, usize)> {
    // Mask structural lines in place so their offsets survive but their content is
    // never flagged (a chapter title's "The" isn't a weak verb; a `***` isn't a
    // word). Equal-length spaces keep every later byte where it is in the buffer.
    let masked = mask_structure(text);
    match category {
        FlagCategory::LongSentences => long_sentences(&masked, run_on_threshold),
        FlagCategory::OverusedWords => overused_words(&masked, lists),
        _ => word_flags(&masked, category, lists),
    }
}

/// Replace heading and thematic-break lines with spaces of the same byte length,
/// leaving every other byte (and all newlines) exactly where it is — so a span
/// scanned from the result still points at the right place in the original buffer.
fn mask_structure(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let (line, tail) = rest
            .find('\n')
            .map_or((rest, ""), |nl| (&rest[..nl], &rest[nl + 1..]));
        if is_heading(line) || is_thematic_break(line) {
            // Preserve length byte-for-byte (structural lines are ASCII markers, but
            // stay UTF-8-safe by padding to the same byte count).
            out.extend(std::iter::repeat_n(' ', line.len()));
        } else {
            out.push_str(line);
        }
        if line.len() < rest.len() {
            out.push('\n');
        }
        rest = tail;
    }
    out
}

/// Word-level flags (adverbs, filter words, weak verbs, passive): scan the words
/// once, keeping each word's byte span so a hit can be jumped to. Passive pairs a
/// "to be" form with a following `-ed` word and flags the participle — the word the
/// cursor should land on.
fn word_flags(text: &str, category: FlagCategory, lists: &WordLists) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut prev_to_be = false;
    for (start, word) in text.unicode_word_indices() {
        let lower = word.to_lowercase();
        let end = start + word.len();
        let hit = match category {
            FlagCategory::Adverbs => is_ly_adverb(&lower),
            FlagCategory::FilterWords => lists.is_filter_word(&lower),
            FlagCategory::WeakVerbs => TO_BE.contains(&lower.as_str()),
            FlagCategory::Passive => prev_to_be && lower.ends_with("ed"),
            // Handled by dedicated passes.
            FlagCategory::LongSentences | FlagCategory::OverusedWords => false,
        };
        if hit {
            spans.push((start, end));
        }
        prev_to_be = TO_BE.contains(&lower.as_str());
    }
    spans
}

/// Sentences whose word count exceeds the run-on threshold, as their trimmed byte
/// spans — so stepping lands the cursor on the sentence's first word.
fn long_sentences(text: &str, run_on_threshold: Option<usize>) -> Vec<(usize, usize)> {
    let threshold = run_on_threshold.unwrap_or(DEFAULT_RUN_ON);
    let mut spans = Vec::new();
    for (s, e) in sentence_byte_spans(text) {
        let slice = &text[s..e];
        if count_words(slice) > threshold {
            // Trim leading whitespace so the cursor lands on the first word, not the
            // gap before it (the span is otherwise the raw inter-sentence text).
            let lead = slice.len() - slice.trim_start().len();
            let trail = slice.len() - slice.trim_end().len();
            spans.push((s + lead, e - trail));
        }
    }
    spans
}

/// Content words that recur at least [`OVERUSE_MIN`] times in the chapter, every
/// occurrence flagged (#1001): a two-pass scan — tally first, then emit the spans
/// of the words over the cutoff. Uses the same content-word definition as the
/// report's "most-used words" (non-stopword, length > 2, lowercased).
fn overused_words(text: &str, lists: &WordLists) -> Vec<(usize, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for word in text.unicode_words() {
        let lower = word.to_lowercase();
        if is_content_word(&lower, lists) {
            *counts.entry(lower).or_insert(0) += 1;
        }
    }
    let mut spans = Vec::new();
    for (start, word) in text.unicode_word_indices() {
        let lower = word.to_lowercase();
        if is_content_word(&lower, lists) && counts.get(&lower).is_some_and(|&c| c >= OVERUSE_MIN) {
            spans.push((start, start + word.len()));
        }
    }
    spans
}

/// A word that counts toward the over-used tally: long enough and not a stopword.
fn is_content_word(lower: &str, lists: &WordLists) -> bool {
    lower.len() > 2 && !lists.is_stopword(lower)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn spans_text<'a>(text: &'a str, ranges: &[(usize, usize)]) -> Vec<&'a str> {
        ranges.iter().map(|&(s, e)| &text[s..e]).collect()
    }

    #[test]
    fn adverbs_flag_ly_words_and_skip_the_exceptions() {
        let text = "She slowly walked, then only quietly left.";
        let ranges = flag_ranges(text, FlagCategory::Adverbs, &WordLists::new(), None);
        assert_eq!(spans_text(text, &ranges), vec!["slowly", "quietly"]);
    }

    #[test]
    fn filter_words_and_weak_verbs_land_on_the_word() {
        let lists = WordLists::new();
        let f = flag_ranges(
            "She saw the door and felt cold.",
            FlagCategory::FilterWords,
            &lists,
            None,
        );
        assert_eq!(
            spans_text("She saw the door and felt cold.", &f),
            vec!["saw", "felt"]
        );
        let w = flag_ranges(
            "It was cold and they were tired.",
            FlagCategory::WeakVerbs,
            &lists,
            None,
        );
        assert_eq!(
            spans_text("It was cold and they were tired.", &w),
            vec!["was", "were"]
        );
    }

    #[test]
    fn passive_flags_the_participle_after_a_to_be() {
        let text = "The door was locked and the vault was sealed.";
        let ranges = flag_ranges(text, FlagCategory::Passive, &WordLists::new(), None);
        assert_eq!(spans_text(text, &ranges), vec!["locked", "sealed"]);
    }

    #[test]
    fn long_sentences_are_flagged_by_their_first_word() {
        // A short threshold so a small fixture crosses it; the long sentence's span
        // starts on its first word and the short one is left alone.
        let text = "This sentence has quite a few words in a deliberate row here. Short one.";
        let ranges = flag_ranges(
            text,
            FlagCategory::LongSentences,
            &WordLists::new(),
            Some(8),
        );
        assert_eq!(ranges.len(), 1, "{ranges:?}");
        let hit = &text[ranges[0].0..ranges[0].1];
        assert!(hit.starts_with("This sentence"), "{hit:?}");
        assert!(hit.ends_with("here."), "{hit:?}");
    }

    #[test]
    fn overused_words_flag_every_occurrence_over_the_cutoff() {
        // "locket" appears 4 times (the cutoff), so all four are flagged; a word used
        // once is not, and stopwords never count.
        let text = "The locket. The locket gleamed. A locket again, the locket once more.";
        let ranges = flag_ranges(text, FlagCategory::OverusedWords, &WordLists::new(), None);
        assert_eq!(spans_text(text, &ranges), vec!["locket"; 4]);
    }

    #[test]
    fn spans_address_the_raw_buffer_past_a_heading() {
        // A heading line is masked, not removed, so a flag after it still points at
        // the right byte in the original text (the offsets don't shift).
        let text = "# The Slowly Chapter\n\nShe walked quietly away.\n";
        let ranges = flag_ranges(text, FlagCategory::Adverbs, &WordLists::new(), None);
        // "Slowly" in the title is not flagged; "quietly" in the prose is, at its real
        // offset in `text`.
        assert_eq!(spans_text(text, &ranges), vec!["quietly"]);
        let (s, _) = ranges[0];
        assert_eq!(&text[s..s + 7], "quietly");
    }

    #[test]
    fn scene_break_markers_are_never_flagged() {
        // A `***` divider isn't a word; masking it keeps the following prose's
        // offsets intact and never marks the asterisks.
        let text = "She was gone.\n\n***\n\nIt was quiet.\n";
        let ranges = flag_ranges(text, FlagCategory::WeakVerbs, &WordLists::new(), None);
        assert_eq!(spans_text(text, &ranges), vec!["was", "was"]);
    }

    #[test]
    fn ranges_are_sorted_ascending() {
        let text = "It was slowly and quietly and softly done, it was.";
        for cat in FlagCategory::ALL {
            let ranges = flag_ranges(text, *cat, &WordLists::new(), Some(3));
            assert!(
                ranges.windows(2).all(|w| w[0].0 <= w[1].0),
                "{cat:?} out of order: {ranges:?}"
            );
        }
    }

    /// Performance guard (`cargo perf`). The style-flag sweep (#1001) is computed on
    /// the idle tick, never per keystroke or per draw — like the spell pass. One pass
    /// over a window-sized chapter (the realistic worst case, larger than the
    /// ~3-chapter continuous window) must stay well within a frame for *every*
    /// category, so turning the sweep on and stepping it never stutters. The
    /// two-pass over-used-words scan and the sentence segmentation are the heaviest,
    /// so both are timed.
    #[test]
    #[ignore = "release-only perf guard; run via cargo perf"]
    fn flag_pass_is_fast_at_window_size() {
        let para =
            "The night was slowly dark and the road was very long indeed and the captain walked. ";
        let text = para.repeat(700); // ~11k words, larger than the continuous window
        let lists = WordLists::new();
        for cat in FlagCategory::ALL {
            let _ = flag_ranges("warm up", *cat, &lists, None); // warm, then time
            let t = Instant::now();
            let ranges = flag_ranges(&text, *cat, &lists, None);
            let dt = t.elapsed();
            println!(
                "flag pass {cat:?} on {} words = {dt:?} ({} hits)",
                text.split_whitespace().count(),
                ranges.len()
            );
            assert!(dt.as_millis() < 16, "{cat:?} flag pass took {dt:?}");
        }
    }
}

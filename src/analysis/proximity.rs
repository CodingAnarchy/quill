//! Near-proximity word repeats (#1002): the byte spans, *inside the chapter
//! buffer*, of content words echoed twice within a short window — the local
//! repetition ("she turned… he turned… the road turned") the global "most-used
//! words" report can't surface because it counts the whole manuscript, not the few
//! sentences a reader feels the echo across.
//!
//! Purely lexical and offline: a single streaming pass over the words, no
//! embeddings, no AI, no rewrite. It only *observes* — it returns spans for the
//! editor to dim-mark in place so the author sees the echo where it happens and
//! decides every change; Quill never edits the prose (the AI/analysis boundary).
//!
//! It reuses the flag-sweep's structural masking ([`mask_structure`]) so a chapter
//! title or `***` divider never counts, and the report's own content-word
//! definition ([`is_content_word`]) so common function words ("the", "and", "a")
//! are excluded and the signal stays meaningful. Like the spell / flag passes it is
//! pure and allocation-bounded: the caller runs it on the idle tick over the
//! visible window and caches the result, never per keystroke or per draw.

use std::collections::HashMap;

use unicode_segmentation::UnicodeSegmentation;

use super::flags::{is_content_word, mask_structure};
use super::WordLists;

/// The proximity window, in content words: a repeat of the same content word
/// within this many *content words* of its previous occurrence is a near-echo
/// (#1002). Roughly a few sentences of prose — tight enough that the two landings
/// read as a jarring repetition rather than ordinary recurrence across a chapter.
/// Counted over content words (stopwords already dropped), so it isn't diluted by
/// the "the"s and "and"s between two real words.
pub const PROXIMITY_WINDOW: usize = 30;

/// The byte spans of every content word that repeats within [`PROXIMITY_WINDOW`]
/// content words of an earlier occurrence (#1002), in ascending order, addressing
/// `text`'s own bytes. Both the earlier and the later landing of each near-echo are
/// returned, so the author sees the pair. `lists` supplies the stopword set (with
/// the project's `[analysis]` overrides), so the same common function words the
/// report excludes are excluded here.
///
/// Pure and allocation-bounded — one pass tracking each word's most recent
/// position; the caller runs it on the idle tick over the visible window and caches
/// the result, never per keystroke or per draw.
pub fn proximity_repeats(text: &str, lists: &WordLists) -> Vec<(usize, usize)> {
    // Mask structural lines in place (headings, scene breaks) so their words never
    // count and every later byte stays where the cursor sees it — the same masking
    // the flag sweep (#1001) does.
    let masked = mask_structure(text);
    // The most recent occurrence of each content word: its content-word index (to
    // measure the window) and byte span (to emit if the next hit is close). Only the
    // last occurrence matters — a third landing pairs with the second, not the first.
    let mut last: HashMap<String, (usize, (usize, usize))> = HashMap::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut idx = 0usize; // content-word position, the window's unit
    for (start, word) in masked.unicode_word_indices() {
        let lower = word.to_lowercase();
        if !is_content_word(&lower, lists) {
            continue; // stopwords / short words never count toward the window
        }
        let span = (start, start + word.len());
        if let Some(&(prev_idx, prev_span)) = last.get(&lower) {
            if idx - prev_idx <= PROXIMITY_WINDOW {
                // A near-echo: mark both the earlier landing and this one. `prev_span`
                // may already be in `spans` (from an even earlier pairing); dedupe on
                // push so a run of three close repeats doesn't double-mark the middle.
                push_unique(&mut spans, prev_span);
                push_unique(&mut spans, span);
            }
        }
        last.insert(lower, (idx, span));
        idx += 1;
    }
    // Emitted in encounter order, which for the earlier landing is already ascending;
    // a later landing can precede an unrelated earlier one, so sort to guarantee the
    // ascending order the renderer and the stepper rely on.
    spans.sort_unstable();
    spans.dedup();
    spans
}

/// Push `span` onto `spans` unless it's already the last one pushed — the common
/// case of the same earlier landing pairing with two successive close repeats.
/// A full dedup still runs at the end; this just avoids the obvious duplicate.
fn push_unique(spans: &mut Vec<(usize, usize)>, span: (usize, usize)) {
    if spans.last() != Some(&span) {
        spans.push(span);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;
    use std::time::Instant;

    fn spans_text<'a>(text: &'a str, ranges: &[(usize, usize)]) -> Vec<&'a str> {
        ranges.iter().map(|&(s, e)| &text[s..e]).collect()
    }

    #[test]
    fn a_close_repeat_marks_both_landings() {
        // "turned" lands twice within a few words — both occurrences are marked.
        let text = "She turned away, and then he turned back.";
        let ranges = proximity_repeats(text, &WordLists::new());
        assert_eq!(spans_text(text, &ranges), vec!["turned", "turned"]);
        // Each span addresses the raw buffer at the real word.
        for &(s, e) in &ranges {
            assert_eq!(&text[s..e], "turned");
        }
    }

    #[test]
    fn a_distant_repeat_is_not_marked() {
        // The same content word repeated far apart (well past the window) is ordinary
        // recurrence, not a near-echo. Distinct filler words (each numbered, so none
        // repeat) push the second "turned" past the window from the first.
        let mut filler = String::new();
        for i in 0..40 {
            write!(filler, "filler{i} ").unwrap();
        }
        let text = format!("She turned away. {filler} He turned back.");
        let ranges = proximity_repeats(&text, &WordLists::new());
        assert!(
            ranges.is_empty(),
            "distant repeat should not mark: {:?}",
            spans_text(&text, &ranges)
        );
    }

    #[test]
    fn common_function_words_are_excluded() {
        // "the" and "and" repeat within the window but are stopwords, so the signal
        // stays meaningful — only the real content echo ("river") is marked.
        let text = "The river and the boat and the river.";
        let ranges = proximity_repeats(text, &WordLists::new());
        assert_eq!(spans_text(text, &ranges), vec!["river", "river"]);
    }

    #[test]
    fn short_words_do_not_count() {
        // Words of two letters or fewer are below the content-word floor (like the
        // report's most-used-words basis), so a repeated "at" isn't a proximity flag.
        let text = "He was at the door at last.";
        let ranges = proximity_repeats(text, &WordLists::new());
        assert!(ranges.is_empty(), "{:?}", spans_text(text, &ranges));
    }

    #[test]
    fn a_run_of_three_close_repeats_marks_each_once() {
        // Three landings inside one window: every occurrence is marked, and the
        // middle one is not double-emitted.
        let text = "The shadow moved. A shadow again. Still the shadow.";
        let ranges = proximity_repeats(text, &WordLists::new());
        assert_eq!(
            spans_text(text, &ranges),
            vec!["shadow", "shadow", "shadow"]
        );
    }

    #[test]
    fn spans_address_the_raw_buffer_past_a_heading() {
        // A heading line is masked, not removed, so a repeat after it still points at
        // the right byte in the original text, and the title's words never count.
        let text = "# The Turning Chapter\n\nShe turned, then he turned.\n";
        let ranges = proximity_repeats(text, &WordLists::new());
        assert_eq!(spans_text(text, &ranges), vec!["turned", "turned"]);
    }

    #[test]
    fn scene_break_markers_never_count() {
        // A `***` divider isn't a word; masking it keeps the surrounding prose's
        // offsets intact and the marks are never flagged.
        let text = "The candle burned.\n\n***\n\nThe candle guttered.\n";
        let ranges = proximity_repeats(text, &WordLists::new());
        // "candle" repeats across the break within the window and is marked; the
        // asterisks are not.
        assert_eq!(spans_text(text, &ranges), vec!["candle", "candle"]);
    }

    #[test]
    fn ranges_are_sorted_ascending() {
        let text = "Fog fog fog rolled. The pier waited. Fog returned to the pier.";
        let ranges = proximity_repeats(text, &WordLists::new());
        assert!(
            ranges.windows(2).all(|w| w[0].0 <= w[1].0),
            "out of order: {ranges:?}"
        );
    }

    /// Performance guard (`cargo perf`). The proximity-repeat pass (#1002) is
    /// computed on the idle tick over the visible window, never per keystroke or per
    /// draw — like the spell pass and the #1001 flag sweep. One pass over a
    /// window-sized chapter (the realistic worst case, larger than the ~3-chapter
    /// continuous window) must stay well within a frame, so turning the marking on
    /// never stutters and it stays off the keystroke path.
    #[test]
    #[ignore = "release-only perf guard; run via cargo perf"]
    fn proximity_pass_is_fast_at_window_size() {
        let para =
            "The night was slowly dark and the road was very long indeed and the captain walked. ";
        let text = para.repeat(700); // ~11k words, larger than the continuous window
        let lists = WordLists::new();
        let _ = proximity_repeats("warm up here now", &lists); // warm, then time
        let t = Instant::now();
        let ranges = proximity_repeats(&text, &lists);
        let dt = t.elapsed();
        println!(
            "proximity pass on {} words = {dt:?} ({} hits)",
            text.split_whitespace().count(),
            ranges.len()
        );
        assert!(dt.as_millis() < 16, "proximity pass took {dt:?}");
    }
}

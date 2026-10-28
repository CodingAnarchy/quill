//! Offline spell-checking (#357): flag words not found in a bundled dictionary.
//! It only *observes* — it returns the byte ranges of suspect words for the
//! editor to underline; it never changes the text (within Quill's AI/boundary
//! rules). The check runs on the idle tick, never the keystroke path.
//!
//! The dictionary is the SCOWL-derived en_US Hunspell dictionary, bundled under
//! `assets/dictionary/` (see its `LICENSE.txt`) and read through the pure-Rust
//! `spellbook` Hunspell engine, so it's fully offline — no network, no API key.

use std::sync::LazyLock;

use spellbook::Dictionary;

/// The bundled en_US dictionary, parsed once on first use (kept off the keystroke
/// path, so the one-time parse cost lands on the first idle spell pass).
static DICT: LazyLock<Dictionary> = LazyLock::new(|| {
    #[allow(
        clippy::expect_used,
        reason = "the .aff/.dic are `include_str!`d into the binary at build time, so \
                  parsing the bundled dictionary can't fail at runtime"
    )]
    Dictionary::new(
        include_str!("../../../assets/dictionary/en_US.aff"),
        include_str!("../../../assets/dictionary/en_US.dic"),
    )
    .expect("the bundled en_US dictionary parses")
});

/// The byte ranges of misspelled words in `text` (#357) — words not in the
/// bundled dictionary and not accepted by `accept` (the configured TODO marker,
/// and the per-project custom dictionary, #358/#935). The per-project word list
/// lives in `Project` (durable in `dictionary.txt`, viewable/editable in-app) and
/// reaches this pure function only through the `accept` predicate, so the bundled
/// engine here stays global and stateless. Tokens containing digits aren't
/// spell-checkable and are skipped, not flagged. Pure: the caller runs it on idle
/// and caches the result per chapter.
pub fn misspellings(text: &str, accept: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let dict = &*DICT;
    let mut flags = Vec::new();
    for_each_word(text, |start, word| {
        if accept(word) || dict.check(word) {
            return;
        }
        // Smart-quote contractions ("don’t") may be stored with an ASCII
        // apostrophe in the dictionary; try that form before flagging.
        if word.contains('\u{2019}') && dict.check(&word.replace('\u{2019}', "'")) {
            return;
        }
        flags.push((start, start + word.len()));
    });
    flags
}

/// Offline correction candidates for a misspelled `word` (#647), from the same
/// bundled dictionary the flagging uses — `spellbook`'s `Dictionary::suggest`, so
/// there's no network and no API key. Ordered best-first by the engine; the list is
/// empty when it finds no near match. Called only on a word the flagging already
/// marked misspelled (the author singled it out and asked), on demand — never on
/// the keystroke or idle path.
pub fn suggestions(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    DICT.suggest(word, &mut out);
    out
}

/// Call `emit(byte_offset, word)` for each spell-checkable token in `text`. A token
/// is a run of letters/digits/apostrophes that contains at least one letter and no
/// digit (so "3rd", "1990s", and bare numbers are skipped, not flagged); surrounding
/// apostrophes are trimmed. Letters are Unicode-aware.
///
/// Iterates `char_indices()` directly rather than collecting the whole document into
/// a `Vec<(usize, char)>` (#780): the idle spell pass runs on every input burst, so
/// it does no O(document) allocation of its own.
fn for_each_word(text: &str, mut emit: impl FnMut(usize, &str)) {
    let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '\u{2019}';
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if !is_word(c) {
            chars.next();
            continue;
        }
        let (mut end, mut letter, mut digit) = (start, false, false);
        while let Some(&(i, c)) = chars.peek() {
            if !is_word(c) {
                break;
            }
            letter |= c.is_alphabetic();
            digit |= c.is_ascii_digit();
            end = i + c.len_utf8();
            chars.next();
        }
        if !letter || digit {
            continue;
        }
        let apostrophes = ['\'', '\u{2019}'];
        let w = &text[start..end];
        let trimmed = w.trim_matches(apostrophes);
        if !trimmed.is_empty() {
            let off = w.len() - w.trim_start_matches(apostrophes).len();
            emit(start + off, trimmed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use crate::testutil::perf_budget_ms;

    /// Nothing is accepted beyond the dictionary.
    fn nothing(_: &str) -> bool {
        false
    }

    #[test]
    fn flags_a_misspelling_and_passes_real_words() {
        let text = "The quikc brown fox";
        let flags = misspellings(text, nothing);
        assert_eq!(flags.len(), 1, "{flags:?}");
        let (s, e) = flags[0];
        assert_eq!(&text[s..e], "quikc");
    }

    #[test]
    fn real_words_are_not_flagged() {
        assert!(misspellings("the quick brown fox jumps over", nothing).is_empty());
    }

    #[test]
    fn accepted_words_are_not_flagged() {
        // An invented name is flagged by the dictionary but cleared by `accept`.
        let flagged = misspellings("Vashti walked on", nothing);
        assert_eq!(flagged.len(), 1);
        let accepted = misspellings("Vashti walked on", |w| w == "Vashti");
        assert!(accepted.is_empty(), "accepting the name clears its flag");
    }

    #[test]
    fn a_per_project_word_list_clears_only_its_words() {
        // The durable per-project dictionary reaches the checker as an `accept`
        // predicate over a set (#358/#935): words in the list clear, words outside
        // it — invented or misspelled — are still flagged, and the bundled engine
        // is unchanged.
        let list: std::collections::HashSet<&str> = ["Vashti", "Aerendil"].into_iter().collect();
        let accept = |w: &str| list.contains(w);
        // Two invented names in the list clear; a third, absent from it, is flagged.
        let text = "Vashti met Aerendil but not Zzqxx";
        let flags = misspellings(text, accept);
        assert_eq!(flags.len(), 1, "{flags:?}");
        let (s, e) = flags[0];
        assert_eq!(&text[s..e], "Zzqxx");
        // The list never suppresses a real typo, and real words still pass.
        assert_eq!(
            misspellings("Vashti quikc", accept).len(),
            1,
            "typo still flagged"
        );
    }

    #[test]
    fn tokens_with_digits_and_pure_numbers_are_skipped() {
        assert!(misspellings("in 1990 the 3rd time 42 came", nothing).is_empty());
    }

    #[test]
    fn contractions_with_smart_quotes_are_accepted() {
        assert!(misspellings("it\u{2019}s don\u{2019}t can\u{2019}t", nothing).is_empty());
    }

    #[test]
    fn word_offsets_point_at_the_word() {
        let text = "a zzqzz b";
        let flags = misspellings(text, nothing);
        assert_eq!(flags.len(), 1);
        assert_eq!(&text[flags[0].0..flags[0].1], "zzqzz");
    }

    #[test]
    fn capitalized_real_words_pass() {
        // Hunspell handles capitalization: a sentence-start capital matches.
        assert!(misspellings("The Quick Brown Fox", nothing).is_empty());
    }

    #[test]
    fn suggestions_offers_the_correct_spelling() {
        // A near-miss typo should have its correct spelling among the candidates
        // the offline engine returns (#647).
        let candidates = suggestions("quikc");
        assert!(!candidates.is_empty(), "a typo yields candidates");
        assert!(
            candidates.iter().any(|s| s == "quick"),
            "the correction is offered: {candidates:?}"
        );
    }

    #[test]
    fn suggestions_for_pure_gibberish_are_bounded() {
        // A string with no near match yields no runaway candidate list — at worst a
        // short, best-effort set. (The correction path only ever calls this on a
        // flagged word, so an exact-match case isn't exercised.)
        assert!(suggestions("zzzqqqxxx").len() <= 8);
    }

    /// Performance guard (`cargo perf`). The idle spell pass over a window-sized
    /// chapter (#357) stays well within a frame, so checking on idle never
    /// stutters. Excludes the one-time dictionary parse (warmed first).
    #[test]
    #[ignore = "release-only perf guard; run via cargo perf"]
    fn spell_pass_is_fast_at_window_size() {
        let para =
            "The night was dark and the road was very long indeed and the captain walked on. ";
        let text = para.repeat(700); // ~10k words, larger than the ~3-chapter window
        let _ = misspellings("warm up the dictionary", nothing); // pay the parse once
        let t = Instant::now();
        let flags = misspellings(&text, nothing);
        let dt = t.elapsed();
        println!(
            "spell pass on {} words = {dt:?}",
            text.split_whitespace().count()
        );
        assert!(flags.is_empty(), "all real words");
        assert!(
            dt.as_millis() < perf_budget_ms(16),
            "spell pass took {dt:?}"
        );
    }
}

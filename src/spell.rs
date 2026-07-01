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
    Dictionary::new(
        include_str!("../assets/dictionary/en_US.aff"),
        include_str!("../assets/dictionary/en_US.dic"),
    )
    .expect("the bundled en_US dictionary parses")
});

/// The byte ranges of misspelled words in `text` (#357) — words not in the
/// bundled dictionary and not accepted by `accept` (the configured TODO marker,
/// and the per-project custom dictionary once #358 lands). Tokens containing
/// digits aren't spell-checkable and are skipped, not flagged. Pure: the caller
/// runs it on idle and caches the result per chapter.
pub fn misspellings(text: &str, accept: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let dict = &*DICT;
    let mut flags = Vec::new();
    for (start, word) in words(text) {
        if accept(word) || dict.check(word) {
            continue;
        }
        // Smart-quote contractions ("don’t") may be stored with an ASCII
        // apostrophe in the dictionary; try that form before flagging.
        if word.contains('\u{2019}') && dict.check(&word.replace('\u{2019}', "'")) {
            continue;
        }
        flags.push((start, start + word.len()));
    }
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

/// Split `text` into `(byte_offset, word)` tokens for spell-checking. A token is a
/// run of letters/digits/apostrophes that contains at least one letter and no
/// digit (so "3rd", "1990s", and bare numbers are skipped, not flagged);
/// surrounding apostrophes are trimmed. Letters are Unicode-aware.
fn words(text: &str) -> Vec<(usize, &str)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '\u{2019}';
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        if !is_word(chars[k].1) {
            k += 1;
            continue;
        }
        let start = chars[k].0;
        let (mut end, mut letter, mut digit) = (start, false, false);
        while k < chars.len() && is_word(chars[k].1) {
            letter |= chars[k].1.is_alphabetic();
            digit |= chars[k].1.is_ascii_digit();
            end = chars[k].0 + chars[k].1.len_utf8();
            k += 1;
        }
        if !letter || digit {
            continue;
        }
        let apostrophes = ['\'', '\u{2019}'];
        let w = &text[start..end];
        let trimmed = w.trim_matches(apostrophes);
        if !trimmed.is_empty() {
            let off = w.len() - w.trim_start_matches(apostrophes).len();
            out.push((start + off, trimmed));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

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
    #[ignore]
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
        assert!(dt.as_millis() < 16, "spell pass took {dt:?}");
    }
}

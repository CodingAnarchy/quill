//! The lexical search backing the `search_manuscript` tool: its hit ordering and
//! limit, case-insensitive matching over the original text, and the excerpt
//! window — which must stay on char boundaries when the body holds multibyte
//! characters or a lowercase mapping that isn't length-preserving.

use super::super::search::excerpt_around;
use super::super::tools::SEARCH_LIMIT;
use super::*;

#[test]
fn search_manuscript_interface_is_exercised_via_the_lexical_impl() {
    // The `search:manuscript` seam (#831) is driven through the lexical impl:
    // query → ranked locations with jumpable chapter citations.
    let search = LexicalSearch::new(oversized_corpus());
    let hits = search.search("locket", SEARCH_LIMIT);
    assert!(!hits.is_empty(), "the lexical impl finds the term");
    assert_eq!(hits[0].chapter, "Chapter 7", "cites the chapter it's in");
    assert!(
        hits[0].excerpt.contains("locket"),
        "the excerpt shows the match"
    );
    // An empty query yields nothing (no panic).
    assert!(search.search("   ", SEARCH_LIMIT).is_empty());
}

#[test]
fn search_does_not_panic_on_non_length_preserving_lowercase_before_the_match() {
    // Regression (#utf8-panic): a chapter whose body contains a NON-length-preserving
    // lowercase character BEFORE a searchable match. `İ` (U+0130) is 2 bytes in the
    // original but lowercases to `i` + combining-dot = 3 bytes, so each `İ` shifts the
    // first-match byte offset found in the CACHED lowercased body forward by 1 relative
    // to `ch.body`. The old code sliced `ch.body` with that misaligned offset; here the
    // shift lands mid-codepoint of the `é` in "résumé" → a `byte index N is not a char
    // boundary` panic. This test PANICS before the fix and passes after: the search
    // returns the match with a correct, readable excerpt (original cased prose).
    let body = "İzmir. İstanbul. She read his résumé by the locket.";
    let search = LexicalSearch::new(vec![chapter("Chapter 1", body)]);
    let hits = search.search("résumé", SEARCH_LIMIT);
    assert_eq!(hits.len(), 1, "the match is found despite the İ before it");
    assert_eq!(hits[0].chapter, "Chapter 1");
    assert!(
        hits[0].excerpt.contains("résumé"),
        "the excerpt is the original readable text around the match: {}",
        hits[0].excerpt
    );
    // The excerpt is original prose, not the lowercased cache — the İ survives cased.
    assert!(
        hits[0].excerpt.contains("İstanbul"),
        "the excerpt keeps the original casing: {}",
        hits[0].excerpt
    );

    // A match placed AFTER the İ but with only ASCII around it is also found correctly
    // (the İ shift no longer corrupts the offset).
    let body2 = "İstanbul at dawn. Mara pressed the silver locket to her chest.";
    let search2 = LexicalSearch::new(vec![chapter("Chapter 1", body2)]);
    let hits2 = search2.search("locket", SEARCH_LIMIT);
    assert_eq!(hits2.len(), 1);
    assert!(hits2[0].excerpt.contains("locket"));
    assert!(hits2[0].excerpt.contains("İstanbul"));
}

#[test]
fn search_matches_are_case_insensitive_over_original_text() {
    // Case-insensitive detection still holds when the query casing differs from the
    // body, and the excerpt is drawn from the original (cased) prose.
    let search = LexicalSearch::new(vec![chapter(
        "Chapter 1",
        "The LOCKET gleamed. Mara held the Locket close.",
    )]);
    let hits = search.search("locket", SEARCH_LIMIT);
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].excerpt.contains("LOCKET"),
        "excerpt is original cased prose: {}",
        hits[0].excerpt
    );
}

#[test]
fn ordinary_ascii_search_excerpt_is_unchanged() {
    // Ordinary ASCII bodies are unaffected by the fix: the excerpt around the match is
    // exactly what the length-preserving path produced.
    let filler = "Mara walked the quiet road. ".repeat(200);
    let body = format!("{filler}\nMara pressed the silver locket to her chest.\n{filler}");
    let search = LexicalSearch::new(vec![chapter("Chapter 1", &body)]);
    let hits = search.search("locket", SEARCH_LIMIT);
    assert_eq!(hits.len(), 1);
    // Same excerpt the pre-fix code produced for pure-ASCII input: the offset found in
    // the (byte-identical) lowercased body is directly valid for the ASCII original.
    let at = body.to_lowercase().find("locket").unwrap();
    let expected = excerpt_around(&body, at, 160);
    assert_eq!(
        hits[0].excerpt, expected,
        "ASCII excerpt is unchanged by the fix"
    );
    assert!(hits[0].excerpt.contains("locket"));
}

#[test]
fn excerpt_renders_a_multibyte_char_inside_the_window_without_slicing_mid_codepoint() {
    // A multibyte char INSIDE the excerpt window must render whole — never sliced
    // mid-codepoint. `excerpt_around` snaps its start/end to char boundaries, so even
    // when the radius edge lands inside a multibyte char the slice stays valid.
    let body = "café résumé naïve façade — the locket — señor jalapeño piñata Zürich";
    let search = LexicalSearch::new(vec![chapter("Chapter 1", body)]);
    // Radius chosen so the window edges fall near the accented characters.
    let hits = search.search("locket", SEARCH_LIMIT);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].excerpt.contains("locket"));

    // Directly exercise excerpt_around with a tight radius so its start/end edges land
    // amid multibyte chars — it must not panic and must yield valid UTF-8.
    let at = body.find("locket").unwrap();
    for radius in 0..body.chars().count() {
        let excerpt = excerpt_around(body, at, radius);
        assert!(!excerpt.is_empty() || radius == 0);
    }
}

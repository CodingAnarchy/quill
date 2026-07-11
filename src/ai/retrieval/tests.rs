use super::*;
use crate::ai::embed::Chapter;
use crate::ai::model_select::STANDARD_CONTEXT_TOKENS;
use crate::ai::tool_loop::{ToolCall, MAX_TURNS};
use crate::ai::AiError;
use serde_json::{json, Value};
use std::cell::RefCell;

use super::answer::compact;
use super::search::excerpt_around;
use super::seed::{seed_message, Excerpt};
use super::tools::{run_calls, Tools, SEARCH_LIMIT};
use crate::ai::tool_loop::{Converse, Turn};

fn chapter(title: &str, body: &str) -> Chapter {
    Chapter {
        title: title.to_string(),
        file: format!("{title}.md"),
        body: body.to_string(),
    }
}

/// A big corpus that would blow the on-device window if sent whole — the
/// oversized-offline case the loop exists for.
fn oversized_corpus() -> Vec<Chapter> {
    (1..=40)
        .map(|i| {
            let filler = "Mara walked the quiet road. ".repeat(200);
            let body = if i == 7 {
                format!("{filler}\nMara pressed the silver locket to her chest.\n{filler}")
            } else if i == 12 {
                format!("{filler}\nShe finally spoke of her father, long gone.\n{filler}")
            } else {
                filler
            };
            chapter(&format!("Chapter {i}"), &body)
        })
        .collect()
}

fn bible_with_sections() -> BibleSeed {
    BibleSeed::new(vec![
        (
            "characters".to_string(),
            "Mara — protagonist; carries a silver locket introduced in Chapter 7.".to_string(),
        ),
        (
            "timeline".to_string(),
            "Ch.7: the locket. Ch.12: Mara speaks of her father.".to_string(),
        ),
    ])
}

/// A mock [`Converse`] that replays a canned script of turns, so a test drives the
/// loop with no live model, no network, no keychain (project rule #531).
struct MockConverse {
    script: RefCell<Vec<Turn>>,
    /// Every request body the loop sent, for assertions on seed / compaction.
    bodies: RefCell<Vec<Value>>,
}

impl MockConverse {
    fn new(script: Vec<Turn>) -> Self {
        Self {
            script: RefCell::new(script.into_iter().rev().collect()),
            bodies: RefCell::new(Vec::new()),
        }
    }
}

impl Converse for MockConverse {
    fn respond(&self, body: &Value) -> Result<Turn, AiError> {
        self.bodies.borrow_mut().push(body.clone());
        self.script
            .borrow_mut()
            .pop()
            .ok_or_else(|| AiError::ApiMalformed("mock script exhausted".into()))
    }
}

fn call(name: &str, argument: &str) -> Turn {
    Turn::ToolCalls(vec![ToolCall {
        id: format!("t-{name}"),
        name: name.to_string(),
        input: json!({ "argument": argument }),
    }])
}

#[test]
fn loop_reads_tools_then_answers_with_citations_offline_oversized() {
    // The model searches, reads the chapter it found, then answers — over a corpus
    // far too big to send whole to the 40 960-token local window.
    let corpus = oversized_corpus();
    let search = LexicalSearch::new(corpus);
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(SEARCH_MANUSCRIPT, "locket"),
        call(READ_CHAPTER, "Chapter 7"),
        Turn::Final(
            r#"{"answer":"The locket is introduced in Chapter 7.","citations":["Chapter 7"]}"#
                .to_string(),
        ),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Where is the locket introduced?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    assert!(answer.answer.contains("Chapter 7"));
    assert!(
        answer.citations.contains(&"Chapter 7".to_string()),
        "cites the chapter it actually read: {:?}",
        answer.citations
    );
    // Read-only disclosure so a holistic answer stays hedged (#856): it read 1 of
    // the 40 chapters, so the disclosure says so rather than reading as exhaustive.
    assert!(
        answer.answer.contains("1 of 40 chapters read"),
        "discloses it did not read the whole manuscript: {}",
        answer.answer
    );
}

#[test]
fn citations_point_at_the_chapters_actually_read() {
    // The model reads Ch.7 and Ch.12; even if it forgets to list a citation, the
    // gathered reads supply it — a citation always points at a touched chapter.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        call(READ_CHAPTER, "Chapter 12"),
        // Answer omits citations entirely — the loop backfills from the reads.
        Turn::Final(r#"{"answer":"Yes; see the two chapters.","citations":[]}"#.to_string()),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Does Mara mention her father and carry a locket?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    assert_eq!(
        answer.citations,
        vec!["Chapter 7".to_string(), "Chapter 12".to_string()],
        "citations are exactly the chapters read, in order"
    );
}

#[test]
fn loop_compacts_when_excerpts_grow() {
    // Reading many big chapters overflows the excerpt budget, so old raw dumps are
    // compacted to notes — the running context stays bounded rather than growing
    // unboundedly with every read.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let reads: Vec<Turn> = (1..=8)
        .map(|i| call(READ_CHAPTER, &format!("Chapter {i}")))
        .collect();
    let mut script = reads;
    script.push(Turn::Final(
        r#"{"answer":"Done.","citations":[]}"#.to_string(),
    ));
    let answer = run_retrieval(
        &MockConverse::new(script),
        &search,
        &bible,
        "Summarize the opening.",
        RETRIEVAL_MAX_TURNS,
        // A small window forces compaction after a couple of big reads.
        2048,
    )
    .unwrap();
    // The gathered reads still contribute their citations even after compaction.
    assert!(
        answer.citations.contains(&"Chapter 1".to_string()),
        "an early, compacted read still cites its chapter: {:?}",
        answer.citations
    );
    // Directly assert compaction bounded the retained bodies.
    let mut messages = vec![seed_message(&bible, &search, "q")];
    let mut excerpts: Vec<Excerpt> = (1..=8)
        .map(|i| Excerpt {
            tool: READ_CHAPTER.to_string(),
            argument: format!("Chapter {i}"),
            body: "x".repeat(4000),
            citations: vec![format!("Chapter {i}")],
        })
        .collect();
    compact(&mut messages, &mut excerpts, 4096);
    let retained: usize = excerpts.iter().map(|e| e.body.len()).sum();
    assert!(
        retained <= 4096 + 4000,
        "compaction bounds retained bodies near the budget (kept newest verbatim): {retained}"
    );
    assert!(
        excerpts[0].body.contains("earlier read:chapter"),
        "the oldest excerpt is compacted to a note"
    );
    assert!(
        excerpts[0].body.contains("Chapter 1"),
        "the note keeps the citation"
    );
}

#[test]
fn a_bible_answerable_question_does_not_over_read() {
    // A question the bible seed already answers is closed on the FIRST turn with no
    // chapter reads — the loop doesn't over-read when the global index suffices.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![Turn::Final(
        r#"{"answer":"Mara is the protagonist.","citations":[]}"#.to_string(),
    )]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Who is the protagonist?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    assert!(answer.answer.contains("Mara"));
    // Exactly one model call — no tool round-trips.
    assert_eq!(
        mock.bodies.borrow().len(),
        1,
        "answered from the seed alone"
    );
}

#[test]
fn no_bible_still_runs_search_only_and_discloses() {
    // With no story bible the loop still runs (search-only) and the answer discloses
    // the weaker coverage (#856 acceptance criteria).
    let search = LexicalSearch::new(oversized_corpus());
    let bible = BibleSeed::default();
    assert!(bible.is_empty());
    let mock = MockConverse::new(vec![
        call(SEARCH_MANUSCRIPT, "locket"),
        Turn::Final(r#"{"answer":"Chapter 7.","citations":["Chapter 7"]}"#.to_string()),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Where is the locket?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    assert!(
        answer.answer.contains("no story bible"),
        "discloses the missing bible: {}",
        answer.answer
    );
}

#[test]
fn turn_cap_still_answers_and_discloses_incomplete() {
    // A model that keeps calling tools forever is stopped at the cap, but the loop
    // still answers with what it gathered and discloses incompleteness — never
    // errors out or returns nothing (#856).
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    // The final "best answer" request (empty tools) also returns a tool call, so
    // the loop must synthesize the disclosed fallback.
    let script: Vec<Turn> = (0..RETRIEVAL_MAX_TURNS + 1)
        .map(|_| call(READ_CHAPTER, "Chapter 7"))
        .collect();
    let mock = MockConverse::new(script);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Trace every thread.",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    assert!(
        answer.answer.contains("incomplete"),
        "discloses the cap was hit: {}",
        answer.answer
    );
    assert!(
        answer.citations.contains(&"Chapter 7".to_string()),
        "still cites what it gathered"
    );
}

#[test]
fn answer_is_never_manuscript_prose_only_answer_and_citations() {
    // The output is an Answer (analysis) + chapter-title citations — never a chunk
    // of manuscript prose. Even if the model echoes prose, the loop returns the
    // structured answer, and citations are titles, not bodies.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        Turn::Final(
            r#"{"answer":"The locket appears in Chapter 7.","citations":["Chapter 7"]}"#
                .to_string(),
        ),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Where is the locket?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    // Citations are chapter titles, not prose bodies.
    for c in &answer.citations {
        assert!(
            !c.contains("Mara walked the quiet road"),
            "a citation is a title, not prose"
        );
    }
    // The answer is the analysis + disclosure, not the chapter's raw filler.
    assert!(!answer
        .answer
        .contains(&"Mara walked the quiet road. ".repeat(10)));
}

#[test]
fn coverage_metadata_records_the_read_set_and_a_coverage_sense() {
    // #1056: the answer carries structured coverage — WHICH chapters it read and
    // HOW COMPLETE that was — as metadata, asserted separately from the prose.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        call(READ_CHAPTER, "Chapter 12"),
        Turn::Final(
            r#"{"answer":"Both threads appear.","citations":["Chapter 7","Chapter 12"]}"#
                .to_string(),
        ),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Trace the locket and the father thread.",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    let coverage = answer
        .coverage
        .as_ref()
        .expect("offline answer has coverage");
    // The read-set: exactly the two chapters it read, in order.
    assert_eq!(
        coverage.chapters_read,
        vec!["Chapter 7".to_string(), "Chapter 12".to_string()],
        "coverage records the chapters actually read"
    );
    // The coverage sense: 2 of 40, a bible was present, the cap wasn't hit.
    assert_eq!(coverage.chapters_total, 40, "the whole corpus size");
    assert!(!coverage.no_bible);
    assert!(!coverage.cap_reached);
    // Metadata, not prose: assert the coverage independently of the answer text.
    assert!(coverage.is_hedged(), "2 of 40 is a partial read, so hedged");
}

#[test]
fn a_holistic_partial_read_is_hedged_in_metadata_and_disclosure() {
    // A holistic question the loop can't fully cover (it read a handful of 40
    // chapters) is hedged accordingly — both in the coverage metadata and in the
    // derived, prose-independent disclosure (#1056).
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        Turn::Final(
            r#"{"answer":"The book is broadly about a journey.","citations":["Chapter 7"]}"#
                .to_string(),
        ),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "What is the book about, overall?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    let coverage = answer.coverage.as_ref().unwrap();
    assert!(
        coverage.chapters_read.len() < coverage.chapters_total,
        "read fewer than all chapters"
    );
    assert!(coverage.is_hedged(), "a partial read is hedged");
    // The disclosure is derived metadata, appended after the prose — it discloses
    // the fraction read and does NOT alter the answer prose itself.
    assert!(
        answer.answer.contains("1 of 40 chapters read"),
        "discloses the coverage fraction: {}",
        answer.answer
    );
    assert!(
        answer.answer.contains("not the whole manuscript"),
        "hedges a holistic answer: {}",
        answer.answer
    );
}

#[test]
fn disclosure_is_metadata_and_never_alters_the_answer_prose() {
    // #1056: the coverage disclosure is metadata about the read — it's appended
    // after the model's prose and never rewrites it. The answer prose the model
    // produced survives verbatim ahead of the disclosure.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let prose = "The locket is introduced in Chapter 7.";
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        Turn::Final(format!(
            r#"{{"answer":"{prose}","citations":["Chapter 7"]}}"#
        )),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Where is the locket introduced?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    // The model's prose is intact and leads the answer; the disclosure follows it.
    assert!(
        answer.answer.starts_with(prose),
        "the answer prose is unaltered: {}",
        answer.answer
    );
    // The coverage read-set matches the disclosure fraction — one source of truth.
    let coverage = answer.coverage.as_ref().unwrap();
    assert_eq!(coverage.chapters_read, vec!["Chapter 7".to_string()]);
    let (read, total) = (coverage.chapters_read.len(), coverage.chapters_total);
    assert!(answer
        .answer
        .contains(&format!("{read} of {total} chapters read")));
}

#[test]
fn a_well_covered_answer_is_not_spuriously_hedged() {
    // When the loop reads EVERY chapter of a small book, with a bible and no cap,
    // coverage is complete — the answer is not hedged (#1056). Guards against a
    // spurious "may be incomplete" on an answer that genuinely covered the book.
    let small = vec![
        chapter("One", "Mara found the locket by the shore."),
        chapter("Two", "She spoke of her father at last."),
    ];
    let search = LexicalSearch::new(small);
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "One"),
        call(READ_CHAPTER, "Two"),
        Turn::Final(
            r#"{"answer":"The locket and the father both appear.","citations":["One","Two"]}"#
                .to_string(),
        ),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Do the locket and the father both appear?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    let coverage = answer.coverage.as_ref().unwrap();
    assert_eq!(
        coverage.chapters_read.len(),
        coverage.chapters_total,
        "read all"
    );
    assert!(!coverage.is_hedged(), "a full read is not hedged");
    assert!(
        answer.answer.contains("full read of the manuscript"),
        "an unhedged answer says so plainly: {}",
        answer.answer
    );
    assert!(
        !answer.answer.contains("may be partial") && !answer.answer.contains("incomplete"),
        "not spuriously hedged: {}",
        answer.answer
    );
}

#[test]
fn hallucinated_citations_do_not_inflate_coverage_into_a_false_full_read() {
    // #qa-citation-coverage (HIGH): the loop reads ONE chapter, but the model's final
    // reply lists citations for chapters it never read (real Chapters 1..40) plus an
    // invented title that isn't a chapter at all. Before the fix, `finish` chained the
    // model's unvalidated citations into the read-set, so `chapters_read.len()` hit
    // 40 ≥ 40 and `is_hedged()` went FALSE, emitting "a full read of the manuscript" — a
    // false completeness claim — and the invented title became a phantom jumpable
    // citation. The fix keeps a model citation only when the loop ACTUALLY read that
    // chapter, so the read-set stays honest (just Chapter 7) and coverage stays hedged.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    // The model reads exactly one chapter, then hallucinates a full-book citation list
    // (Chapter 1..40) plus an invented title that names no real chapter.
    let mut cited: Vec<String> = (1..=40).map(|i| format!("Chapter {i}")).collect();
    cited.push("The Secret Epilogue".to_string());
    let citations_json = serde_json::to_string(&cited).unwrap();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        Turn::Final(format!(
            r#"{{"answer":"The whole book is about the locket.","citations":{citations_json}}}"#
        )),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "What is the whole book about?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    let coverage = answer.coverage.as_ref().unwrap();
    // The read-set is only the ONE chapter the loop actually read — not the 40 the
    // model claimed. The invented title is dropped entirely (no phantom citation).
    assert_eq!(
        coverage.chapters_read,
        vec!["Chapter 7".to_string()],
        "only the real, actually-read chapter counts: {:?}",
        coverage.chapters_read
    );
    assert_eq!(
        answer.citations,
        vec!["Chapter 7".to_string()],
        "the invented title is not a jumpable citation: {:?}",
        answer.citations
    );
    assert!(
        !answer
            .citations
            .contains(&"The Secret Epilogue".to_string()),
        "a phantom title never becomes a citation"
    );
    // Coverage stays HONEST: 1 of 40 is a partial read, so it's hedged, not a full read.
    assert!(
        coverage.is_hedged(),
        "a one-chapter read stays hedged despite the hallucinated citations"
    );
    assert!(
        answer.answer.contains("1 of 40 chapters read")
            && answer.answer.contains("not the whole manuscript"),
        "the disclosure reflects the true partial coverage, not a full read: {}",
        answer.answer
    );
    assert!(
        !answer.answer.contains("full read of the manuscript"),
        "no false completeness claim: {}",
        answer.answer
    );
}

#[test]
fn coverage_survives_the_response_round_trip() {
    // The offline answer is serialized to a Response and re-parsed by the shared
    // Q&A dispatch (#856); the coverage metadata must survive that round-trip so
    // the presenter can surface it (#1056).
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        Turn::Final(r#"{"answer":"Chapter 7.","citations":["Chapter 7"]}"#.to_string()),
    ]);
    let answer = run_retrieval(
        &mock,
        &search,
        &bible,
        "Where is the locket?",
        RETRIEVAL_MAX_TURNS,
        STANDARD_CONTEXT_TOKENS,
    )
    .unwrap();
    let response = answer_response(&answer);
    let reparsed = crate::ai::parse_answer(&response.text);
    assert_eq!(
        reparsed.coverage, answer.coverage,
        "coverage survives serialize → parse_answer"
    );
}

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

#[test]
fn retrieval_cap_is_larger_than_the_assistant_cap() {
    // The retrieval loop gets a higher cap than the assistant's 8, tunable, but the
    // shared assistant cap is unchanged (#856).
    const { assert!(RETRIEVAL_MAX_TURNS > MAX_TURNS) };
    assert_eq!(MAX_TURNS, 8, "the assistant cap stays 8");
}

#[test]
fn retrieval_tools_are_read_only_lookups_with_an_argument() {
    let tools = retrieval_tools();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(
        names,
        vec![READ_CHAPTER, SEARCH_MANUSCRIPT, READ_BIBLE, LIST_CHAPTERS]
    );
    // No tool writes prose — the catalog IS the allowlist (the boundary in code).
    for t in &tools {
        let name = t["name"].as_str().unwrap();
        assert!(
            name.starts_with("read:") || name.starts_with("search:") || name.starts_with("list:"),
            "every tool is a read-only lookup: {name}"
        );
        assert_eq!(
            t["input_schema"]["properties"]["argument"]["type"],
            "string"
        );
    }
}

// --- Tool-executor error arms (#856) -------------------------------------
//
// Every arm of `Tools::run` that a non-compliant model can hit must degrade
// gracefully — a plain note excerpt, never an error that kills the loop. These
// drive the executor directly through `run_calls` (the same path the loop uses)
// and assert the retained excerpt for each arm. The closed allowlist means an
// unknown tool is a note, not a capability.

/// A raw `tool_use` call, as the model would emit it, for driving `run_calls`.
fn tool_call(name: &str, input: Value) -> ToolCall {
    ToolCall {
        id: format!("t-{name}"),
        name: name.to_string(),
        input,
    }
}

/// Drive one batch of calls through the executor and return the retained
/// excerpts — mirroring how the loop records tool results, but hermetic (no model).
fn run_batch(search: &LexicalSearch, bible: &BibleSeed, calls: &[ToolCall]) -> Vec<Excerpt> {
    let tools = Tools { search, bible };
    let mut messages = Vec::new();
    let mut excerpts = Vec::new();
    run_calls(&tools, calls, &mut messages, &mut excerpts);
    // The batch always appends the tool_use turn and the tool_result turn.
    assert_eq!(
        messages.len(),
        2,
        "one tool_use turn and one tool_result turn"
    );
    excerpts
}

#[test]
fn read_chapter_arm_notes_a_missing_chapter_without_killing_the_loop() {
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let excerpts = run_batch(
        &search,
        &bible,
        &[tool_call(
            READ_CHAPTER,
            json!({ "argument": "Chapter 999" }),
        )],
    );
    assert_eq!(excerpts.len(), 1);
    assert!(
        excerpts[0]
            .body
            .contains("No chapter titled \"Chapter 999\""),
        "a missing chapter is a plain note: {}",
        excerpts[0].body
    );
    assert!(
        excerpts[0].citations.is_empty(),
        "a missed read cites nothing"
    );
}

#[test]
fn unknown_tool_arm_returns_a_note_not_a_capability() {
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    // A tool that isn't in the closed allowlist (e.g. a hallucinated "write:" tool)
    // must resolve to a note — the executor never grants an off-allowlist capability.
    let excerpts = run_batch(
        &search,
        &bible,
        &[tool_call(
            "write:chapter",
            json!({ "argument": "anything" }),
        )],
    );
    assert_eq!(excerpts.len(), 1);
    assert!(
        excerpts[0].body.contains("Unknown tool \"write:chapter\""),
        "an off-allowlist tool is a note: {}",
        excerpts[0].body
    );
    assert!(excerpts[0].citations.is_empty());
}

#[test]
fn missing_argument_field_defaults_to_empty_and_does_not_panic() {
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    // The model omits the `argument` field entirely. `run_calls` defaults it to "",
    // so a read arm sees an empty title and returns the missing-chapter note — no
    // panic on the absent field.
    let excerpts = run_batch(&search, &bible, &[tool_call(READ_CHAPTER, json!({}))]);
    assert_eq!(excerpts.len(), 1);
    assert_eq!(
        excerpts[0].argument, "",
        "the absent argument defaults to empty"
    );
    assert!(
        excerpts[0].body.contains("No chapter titled \"\""),
        "an empty title is a plain note: {}",
        excerpts[0].body
    );
}

#[test]
fn read_bible_arm_notes_a_missing_section_and_lists_what_exists() {
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let excerpts = run_batch(
        &search,
        &bible,
        &[tool_call(
            READ_BIBLE,
            json!({ "argument": "no-such-section" }),
        )],
    );
    assert_eq!(excerpts.len(), 1);
    assert!(
        excerpts[0]
            .body
            .contains("No bible section \"no-such-section\""),
        "a missing bible section is a note: {}",
        excerpts[0].body
    );
    // The note lists the sections that DO exist, so the model can recover.
    assert!(
        excerpts[0].body.contains("characters") && excerpts[0].body.contains("timeline"),
        "the note discloses the available sections: {}",
        excerpts[0].body
    );
}

#[test]
fn empty_corpus_search_and_list_arms_disclose_nothing_rather_than_panicking() {
    // total_chapters == 0: an empty manuscript. Search finds nothing and list is
    // empty — both are plain, non-panicking notes rather than a crash.
    let search = LexicalSearch::new(Vec::new());
    let bible = BibleSeed::default();
    let excerpts = run_batch(
        &search,
        &bible,
        &[
            tool_call(SEARCH_MANUSCRIPT, json!({ "argument": "locket" })),
            tool_call(LIST_CHAPTERS, json!({ "argument": "" })),
        ],
    );
    assert_eq!(excerpts.len(), 2);
    assert!(
        excerpts[0].body.contains("No matches for \"locket\""),
        "search over an empty corpus discloses no matches: {}",
        excerpts[0].body
    );
    assert!(excerpts[0].citations.is_empty());
    assert!(
        excerpts[1].body.trim().is_empty(),
        "listing an empty corpus yields an empty list, not a panic: {:?}",
        excerpts[1].body
    );
}

#[test]
fn search_arm_sorts_and_truncates_to_the_search_limit() {
    // More than SEARCH_LIMIT (6) chapters match, so the arm must truncate to the
    // top SEARCH_LIMIT and cite exactly those — never dump every hit.
    let corpus: Vec<Chapter> = (1..=10)
        .map(|i| {
            chapter(
                &format!("Chapter {i}"),
                "Mara held the silver locket close.",
            )
        })
        .collect();
    let search = LexicalSearch::new(corpus);
    let bible = bible_with_sections();
    let excerpts = run_batch(
        &search,
        &bible,
        &[tool_call(
            SEARCH_MANUSCRIPT,
            json!({ "argument": "locket" }),
        )],
    );
    assert_eq!(excerpts.len(), 1);
    assert_eq!(
        excerpts[0].citations.len(),
        SEARCH_LIMIT,
        "the search arm truncates to SEARCH_LIMIT hits: {:?}",
        excerpts[0].citations
    );
    // The result body lists exactly SEARCH_LIMIT located excerpts, one per line.
    let lines = excerpts[0]
        .body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    assert_eq!(
        lines, SEARCH_LIMIT,
        "one line per located excerpt, capped at the limit"
    );
}

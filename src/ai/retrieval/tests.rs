use super::*;
use crate::ai::embed::Chapter;
use crate::ai::model_select::STANDARD_CONTEXT_TOKENS;
use crate::ai::tool_loop::{ToolCall, MAX_TURNS};
use crate::ai::AiError;
use serde_json::{json, Value};
use std::cell::RefCell;

use super::answer::compact;
use super::seed::{seed_message, Excerpt};
use super::tools::SEARCH_LIMIT;
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

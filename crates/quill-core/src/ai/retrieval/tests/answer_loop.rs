//! The retrieval loop end to end, driven by a scripted mock model: it reads
//! tools before answering, cites the chapters it actually read, compacts the
//! excerpt set when it grows, short-circuits on a bible-answerable question,
//! runs search-only with no bible, still answers (and discloses) at the turn
//! cap, and returns an answer plus citations — never manuscript prose.

use super::super::answer::compact;
use super::super::seed::{seed_message, Excerpt};
use super::*;
use crate::ai::model_select::STANDARD_CONTEXT_TOKENS;
use crate::ai::tool_loop::Turn;

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

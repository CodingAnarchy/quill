//! The coverage metadata and its disclosure: what the loop records about the
//! read set and how well it covered the question, when a partial read of a
//! holistic question is hedged (and when a well-covered answer must not be),
//! that the disclosure is metadata and never edits the answer prose, that
//! hallucinated citations can't inflate coverage into a false full read, that a
//! mis-cased but valid citation is canonicalized so the jump works, and that all
//! of it survives the response round trip.

use super::*;
use crate::ai::model_select::STANDARD_CONTEXT_TOKENS;
use crate::ai::tool_loop::Turn;

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
fn a_mis_cased_but_valid_citation_stores_the_canonical_title_and_jumps() {
    // #1145 (Bug): the loop genuinely reads "Chapter 7", but the model cites it as
    // "chapter 7" (different casing). Case-insensitive validation accepted it, but the
    // stored citation kept the MODEL's casing, so the exact-match jump ("chapter 7" vs
    // the real "Chapter 7") failed "Chapter not found". The fix stores the CANONICAL
    // read-side title, so a mis-cased-but-valid citation resolves to the real chapter
    // and both the displayed citation and the coverage read-set use the canonical title.
    let search = LexicalSearch::new(oversized_corpus());
    let bible = bible_with_sections();
    let mock = MockConverse::new(vec![
        call(READ_CHAPTER, "Chapter 7"),
        // The model cites the chapter it read, but with lowercase casing.
        Turn::Final(
            r#"{"answer":"The locket is introduced in chapter 7.","citations":["chapter 7"]}"#
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
    // The stored citation is the canonical read-side title, so an exact-match jump lands.
    assert_eq!(
        answer.citations,
        vec!["Chapter 7".to_string()],
        "a mis-cased citation is stored with the canonical read-side casing: {:?}",
        answer.citations
    );
    assert!(
        !answer.citations.contains(&"chapter 7".to_string()),
        "the model's raw lowercase casing is not stored: {:?}",
        answer.citations
    );
    // Coverage uses the canonical title too — one source of truth with the citations.
    let coverage = answer.coverage.as_ref().unwrap();
    assert_eq!(
        coverage.chapters_read,
        vec!["Chapter 7".to_string()],
        "coverage read-set uses the canonical title: {:?}",
        coverage.chapters_read
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

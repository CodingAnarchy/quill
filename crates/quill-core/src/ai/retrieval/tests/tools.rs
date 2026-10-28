//! The tool layer: the retrieval turn cap against the assistant's, the tool
//! definitions staying read-only lookups that take one argument, and every arm
//! of the executor a non-compliant model can hit.

use super::super::seed::Excerpt;
use super::super::tools::{run_calls, Tools, SEARCH_LIMIT};
use super::*;
use crate::ai::tool_loop::{ToolCall, MAX_TURNS};
use serde_json::{json, Value};

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

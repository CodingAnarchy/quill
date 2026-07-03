//! AI Q&A ("Ask the manuscript"): the mock backend harness, answering with and
//! without citations, jumping to cited chapters, and the persistent Q&A log.

use super::*;

// --- AI: "Ask the manuscript" ----------------------------------------------

/// A backend that returns a canned answer (or an error) with no network — lets
/// the Q&A flow be tested deterministically and offline.
struct MockBackend {
    result: Result<String, ()>,
}

impl Backend for MockBackend {
    fn complete(&self, _req: &ai::Request) -> Result<ai::Response, ai::AiError> {
        match &self.result {
            Ok(text) => Ok(ai::Response {
                text: text.clone(),
                usage: ai::Usage::default(),
            }),
            Err(()) => Err(ai::AiError::Network {
                status: None,
                message: "boom".into(),
            }),
        }
    }
}

#[test]
fn ask_with_ai_off_explains_how_to_enable() {
    let (mut app, _dir) = two_chapter_app("ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.ask_ai("who is the protagonist?");
    assert!(
        !app.ai_task.is_busy(),
        "nothing should be sent when AI is off"
    );
    assert!(app.overlay.is_none());
    assert!(
        app.message.as_deref().unwrap_or("").contains("off"),
        "{:?}",
        app.message
    );
}

#[test]
#[cfg(not(feature = "local"))]
fn offline_mode_selects_the_local_backend() {
    let (mut app, _dir) = two_chapter_app("ai-offline");
    app.ai = ai::Settings {
        enabled: true,
        offline: true,
        ..ai::Settings::default()
    };
    // Offline forces the local backend, even with a key present.
    app.ai.api_key = Some("k".into());
    assert_eq!(app.ai.backend(), ai::BackendKind::Local);
    app.ask_ai("anything?");
    // This build has no `local` feature, so it can't run — nothing is sent and the
    // message explains how to enable it (rather than silently using Claude).
    assert!(!app.ai_task.is_busy());
    assert!(
        app.message.as_deref().unwrap_or("").contains("local"),
        "{:?}",
        app.message
    );
}

#[test]
fn manuscript_text_includes_every_chapter() {
    let (app, _dir) = two_chapter_app("ai-manuscript");
    let m = app.manuscript_text();
    assert!(m.contains("# One") && m.contains("first chapter"), "{m}");
    assert!(m.contains("# Two") && m.contains("second chapter"), "{m}");
}

#[test]
fn qa_answer_opens_the_overlay() {
    let (mut app, _dir) = two_chapter_app("ai-answer");
    // Drive the Q&A core with a mock backend (no network): build the request from
    // the manuscript, complete it, and apply the result like the async path does.
    let backend = MockBackend {
        result: Ok("Chapter One is about a locket.".into()),
    };
    let req = ai::build_qa_request(
        app.manuscript_text(),
        "what is chapter one about?",
        &app.ai.model,
    );
    app.apply_qa_result(backend.complete(&req));

    let panel = app.overlay_lines().expect("answer overlay should open");
    let text: String = panel
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(text.contains("about a locket"), "{text}");
}

#[test]
fn qa_error_sets_a_message_not_a_panel() {
    let (mut app, _dir) = two_chapter_app("ai-err");
    let backend = MockBackend { result: Err(()) };
    let req = ai::build_qa_request(app.manuscript_text(), "q", &app.ai.model);
    app.apply_qa_result(backend.complete(&req));
    assert!(app.overlay.is_none());
    assert!(
        app.message.as_deref().unwrap_or("").contains("boom"),
        "{:?}",
        app.message
    );
}

/// Drive a structured Q&A reply (JSON with citations) through `apply_qa_result`
/// like the async path does, recording the question first as `ask_ai` would.
fn answer_with_citations(app: &mut App, question: &str, json: &str) {
    let backend = MockBackend {
        result: Ok(json.to_string()),
    };
    let req = ai::build_qa_request(app.manuscript_text(), question, &app.ai.model);
    app.pending_qa_question = Some(question.to_string());
    app.apply_qa_result(backend.complete(&req));
}

#[test]
fn qa_answer_with_citations_renders_jumpable_entries() {
    let (mut app, _dir) = two_chapter_app("ai-cite");
    answer_with_citations(
        &mut app,
        "where is the locket?",
        r#"{"answer":"The locket appears in chapter Two.","citations":["Two"]}"#,
    );
    match &app.mode {
        Mode::Findings(f) => {
            // The answer prose leads as non-jumpable context …
            let text: String = f.items.iter().map(|i| i.summary.clone()).collect();
            assert!(text.contains("appears in chapter Two"), "{text}");
            let context = &f.items[f.selected.min(1).saturating_sub(1)];
            assert!(
                context.chapters.is_empty(),
                "answer rows aren't jump targets"
            );
            // … and the selected entry is the jumpable citation for the cited chapter.
            let jump = &f.items[f.selected];
            assert_eq!(jump.chapters, vec!["Two".to_string()]);
            assert!(jump.summary.contains("Two"), "{}", jump.summary);
        }
        _ => panic!("citations should open a jumpable findings list"),
    }
}

#[test]
fn qa_citation_jump_opens_the_cited_chapter() {
    let (mut app, _dir) = two_chapter_app("ai-cite-jump");
    answer_with_citations(
        &mut app,
        "where is the locket?",
        r#"{"answer":"See chapter Two.","citations":["Two"]}"#,
    );
    app.on_key(key(KeyCode::Enter)); // jump to the cited chapter
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1, "jumped to chapter Two");
}

#[test]
fn qa_answer_without_citations_stays_a_plain_overlay() {
    let (mut app, _dir) = two_chapter_app("ai-cite-none");
    answer_with_citations(
        &mut app,
        "is there a dragon?",
        r#"{"answer":"The manuscript never mentions a dragon.","citations":[]}"#,
    );
    assert!(app.overlay.is_some(), "the answer still shows");
    assert!(
        !matches!(app.mode, Mode::Findings(_)),
        "no citations → no jumpable list"
    );
}

#[test]
fn qa_log_records_asks_and_is_browsable() {
    let (mut app, _dir) = two_chapter_app("ai-log");
    answer_with_citations(
        &mut app,
        "who is in chapter one?",
        r#"{"answer":"Mara, in chapter One.","citations":["One"]}"#,
    );
    answer_with_citations(
        &mut app,
        "and chapter two?",
        r#"{"answer":"Nobody new.","citations":["Two"]}"#,
    );
    assert_eq!(app.qa_log.len(), 2, "both asks are logged");
    assert_eq!(app.qa_log[0].question, "who is in chapter one?");
    assert_eq!(app.qa_log[1].answer.citations, vec!["Two".to_string()]);

    // Browsing the history opens a jumpable list of every past ask's citations,
    // newest first, labeled with the question.
    app.open_qa_history();
    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 2, "one row per cited chapter, both asks");
            assert!(
                f.items[0].summary.contains("and chapter two?"),
                "newest ask first: {}",
                f.items[0].summary
            );
            assert_eq!(f.items[0].chapters, vec!["Two".to_string()]);
        }
        _ => panic!("history should open a jumpable findings list"),
    }
    app.on_key(key(KeyCode::Enter)); // jump from the history
    assert_eq!(
        app.project.active_index(),
        1,
        "jumped to the cited chapter Two"
    );
}

#[test]
fn qa_log_persists_across_reload_with_citations() {
    // Ask once (injected result, no network), then reopen the same project dir:
    // the Q&A log — question, answer, citations — reloads from `bible/qa-log.toml`
    // instead of rebuilding empty (#654).
    let (mut app, dir) = two_chapter_app("ai-log-persist");
    answer_with_citations(
        &mut app,
        "what did I establish about the locket?",
        r#"{"answer":"It changes color across chapters One and Two.","citations":["One","Two"]}"#,
    );
    assert_eq!(app.qa_log.len(), 1, "the ask is logged in-session");
    drop(app); // close the editor; the store stays on disk

    // Reopen the project from the same directory: the persisted ask comes back.
    let reopened = open_test_app(&dir);
    assert_eq!(reopened.qa_log.len(), 1, "the ask reloaded from disk");
    let entry = &reopened.qa_log[0];
    assert_eq!(entry.question, "what did I establish about the locket?");
    assert!(
        entry.answer.answer.contains("changes color"),
        "{}",
        entry.answer.answer
    );
    assert_eq!(
        entry.answer.citations,
        vec!["One".to_string(), "Two".to_string()],
        "citations survive the reload"
    );

    // The reloaded history is browsable and its citations re-jump — one row per
    // cited chapter.
    let mut reopened = reopened;
    reopened.open_qa_history();
    match &reopened.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 2, "one jumpable row per cited chapter");
            assert!(
                f.items[0].summary.contains("what did I establish"),
                "{}",
                f.items[0].summary
            );
        }
        _ => panic!("reloaded history should open a jumpable findings list"),
    }
}

#[test]
fn qa_history_is_empty_before_any_ask() {
    let (mut app, _dir) = two_chapter_app("ai-log-empty");
    app.open_qa_history();
    assert!(!matches!(app.mode, Mode::Findings(_)), "nothing to browse");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No questions"),
        "{:?}",
        app.message
    );
}

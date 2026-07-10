//! Cultural & historical accuracy pass (#973): author-invoked, per a named concern,
//! findings only — accuracy, NOT sensitivity / representation. Hermetic throughout:
//! a canned mock backend drives the pass, so no test makes a live LLM call, hits the
//! network, downloads a model, or reads the keychain (project rule #531).

use super::*;

use std::sync::Mutex;

use crate::ai::accuracy::{self, Accuracy, AccuracyFinding};
use crate::ai::Backend;
use crate::mode::{Action, AiAction, FindingKind};

/// A backend that returns a canned reply (or an error) with no network — lets the
/// accuracy pass be driven deterministically and offline. It also records the last
/// request it saw, so a test can assert the author's named concern reached the model.
struct MockBackend {
    reply: Result<String, ()>,
    seen_system: Mutex<Option<String>>,
}

impl MockBackend {
    fn ok(reply: &str) -> Self {
        Self {
            reply: Ok(reply.to_string()),
            seen_system: Mutex::new(None),
        }
    }
}

impl Backend for MockBackend {
    fn complete(&self, req: &ai::Request) -> Result<ai::Response, ai::AiError> {
        *self.seen_system.lock().unwrap() = req.system.clone();
        match &self.reply {
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

/// Drive the accuracy pass core with a mock backend the way the async worker path
/// does: build the request from the manuscript + the named concern, complete it,
/// parse, and apply the result. Returns the mock so the caller can inspect the
/// request it saw (e.g. that the concern was baked into the prompt).
fn run_accuracy(app: &mut App, concern: &str, reply: &str) -> MockBackend {
    let backend = MockBackend::ok(reply);
    let req = accuracy::build_request(app.manuscript_text(), concern, &app.ai.model);
    let parsed = backend
        .complete(&req)
        .and_then(|r| accuracy::parse(&r.text));
    app.apply_accuracy(parsed);
    backend
}

// --- The pass surfaces findings in the shared triage surface -----------------

#[test]
fn accuracy_findings_open_the_overlay_and_land_in_triage() {
    let (mut app, _dir) = two_chapter_app("accuracy");
    let mock = run_accuracy(
        &mut app,
        "1850s London",
        r#"{"findings":[
            {"passage":"he checked his wristwatch","detail":"wristwatches postdate the 1850s","chapters":["Two"]}
        ]}"#,
    );

    // The named concern reached the model (baked into the system prompt).
    let system = mock.seen_system.lock().unwrap().clone().unwrap_or_default();
    assert!(system.contains("1850s London"), "{system}");

    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            // The finding carries the flagged passage and the why-it-may-be-off note.
            assert!(
                f.items[0].summary.contains("wristwatches postdate"),
                "why-it-may-be-off note: {}",
                f.items[0].summary
            );
            assert!(
                f.items[0].summary.contains("checked his wristwatch"),
                "quotes the flagged passage: {}",
                f.items[0].summary
            );
            assert_eq!(f.items[0].chapters, ["Two"], "anchored to its chapter");
            // Classified under the accuracy kind, so the health dashboard groups it.
            assert_eq!(f.items[0].kind, FindingKind::Accuracy);
            assert_eq!(f.items[0].action, Some(AiAction::Accuracy));
        }
        _ => panic!("expected the findings overlay"),
    }
    // It feeds revision triage like every other analysis pass.
    assert_eq!(app.triage.len(), 1);
    assert_eq!(app.triage[0].kind, FindingKind::Accuracy);
}

#[test]
fn accuracy_with_no_findings_shows_a_clean_state() {
    let (mut app, _dir) = two_chapter_app("accuracy-clean");
    run_accuracy(&mut app, "18th-century sailing", r#"{"findings":[]}"#);
    // An empty result is a clean "no findings" state, not a findings list.
    assert!(app.overlay.is_some(), "a clean summary overlay opens");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.triage.is_empty(), "nothing enters triage");
}

#[test]
fn a_malformed_reply_reports_an_error_not_findings() {
    let (mut app, _dir) = two_chapter_app("accuracy-bad");
    // A reply that ignores the schema is reported, never surfaced as findings.
    run_accuracy(&mut app, "Heian-era court protocol", "not json at all");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(
        app.message.as_deref().unwrap_or("").contains("AI:"),
        "{:?}",
        app.message
    );
    assert!(app.triage.is_empty());
}

// --- The pass produces findings only — never prose ---------------------------

#[test]
fn the_accuracy_pass_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("accuracy-safe");
    let before = std::fs::read_to_string(dir.join("one.md")).unwrap();
    run_accuracy(
        &mut app,
        "1850s London",
        r#"{"findings":[{"passage":"x","detail":"anachronism","chapters":["One"]}]}"#,
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "an accuracy pass flags, it never edits chapter prose"
    );
}

// --- Author-invoked: the palette prompts for the named concern ---------------

/// Enable AI with a key so the cost-gated palette flow can run, deterministic
/// regardless of env / config (test builds read neither, #531).
fn enable_ai(app: &mut App) {
    app.ai = ai::Settings::default();
    app.ai.enabled = true;
    app.ai.api_key = Some("k".into());
}

#[test]
fn the_action_prompts_for_a_concern() {
    let (mut app, _dir) = two_chapter_app("accuracy-prompt");
    enable_ai(&mut app);
    app.run_ai_action(AiAction::Accuracy, false);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::AccuracyConcern));
            assert!(
                s.label.contains("Accuracy concern"),
                "prompt asks for the named concern: {}",
                s.label
            );
        }
        _ => panic!("the accuracy action prompts for a concern first"),
    }
}

#[test]
fn submitting_a_concern_runs_the_pass_and_captures_it() {
    let (mut app, _dir) = two_chapter_app("accuracy-submit");
    enable_ai(&mut app);
    app.ai.cost_threshold = 1_000.0; // cheap enough to skip the cost confirm
    app.run_ai_action(AiAction::Accuracy, false); // opens the concern prompt
    type_str(&mut app, "1850s London");
    app.on_key(key(KeyCode::Enter)); // submit → captures the concern, dispatches
                                     // The concern was consumed by the spawned pass (taken, not left dangling), and
                                     // the request kicked off on the worker (no live call — the key is a dummy, so it
                                     // resolves to a request in flight rather than a not-ready refusal).
    assert!(
        app.accuracy_concern.is_none(),
        "the concern is taken by the run, not left pending"
    );
    assert!(!matches!(app.mode, Mode::Prompt(_)), "the prompt closes");
}

#[test]
fn a_blank_concern_cancels() {
    let (mut app, _dir) = two_chapter_app("accuracy-blank");
    enable_ai(&mut app);
    app.run_ai_action(AiAction::Accuracy, false);
    app.on_key(key(KeyCode::Enter)); // empty concern
    assert!(app.accuracy_concern.is_none());
    assert!(!app.ai_task.is_busy(), "nothing runs without a concern");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("name a concern"),
        "{:?}",
        app.message
    );
}

// --- The two-layer prose guard: read-only, allowlisted, no prose -------------

#[test]
fn accuracy_is_an_allowlisted_read_only_analysis_action() {
    // The assistant may run it (it produces findings, no prose), and it carries a
    // free-text "concern" argument the assistant supplies (#390/#973).
    let action = Action::Ai(AiAction::Accuracy);
    assert!(
        action.assistant_runnable(),
        "read-only accuracy analysis is allowlisted"
    );
    assert_eq!(action.text_arg(), Some("concern"));

    // It's a registry action that round-trips by its stable slug.
    assert_eq!(action.name(), "ai:accuracy");
    assert!(
        matches!(
            Action::from_name("ai:accuracy"),
            Some(Action::Ai(AiAction::Accuracy))
        ),
        "the slug round-trips back to the accuracy action"
    );

    // Its tool definition declares the required concern argument.
    let tools = crate::ai::tools::assistant_tools();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "ai:accuracy")
        .expect("accuracy is offered to the assistant");
    assert_eq!(
        tool["input_schema"]["properties"]["concern"]["type"],
        "string"
    );
    assert_eq!(
        tool["input_schema"]["required"],
        serde_json::json!(["concern"])
    );
}

#[test]
fn the_prompt_and_schema_forbid_rewriting_prose() {
    // The pass is analysis-only by construction: its system prompt tells the model to
    // observe and flag, never to rewrite or supply replacement prose, and its schema
    // has no field that could carry edited manuscript text — only the flagged
    // passage, a why-note, and chapters.
    let req = accuracy::build_request("m".into(), "1850s London", "claude-opus-4-8");
    let system = req.system.as_deref().unwrap_or("");
    assert!(system.contains("Do NOT rewrite"), "flags only: {system}");
    assert!(
        system.contains("NOT a sensitivity"),
        "accuracy, not sensitivity: {system}"
    );

    let schema = accuracy::schema();
    let props = &schema["properties"]["findings"]["items"]["properties"];
    let keys: Vec<&str> = props
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        ["chapters", "detail", "passage"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "no field carries edited prose — only observation fields: {keys:?}"
    );
}

// A dedicated typed-result construction exercising the finding shape end-to-end,
// mirroring the style/voice pass tests (apply the parsed result directly).
#[test]
fn apply_accuracy_maps_the_typed_finding_shape() {
    let (mut app, _dir) = two_chapter_app("accuracy-typed");
    app.apply_accuracy(Ok(Accuracy {
        findings: vec![AccuracyFinding {
            passage: "a potato in ancient Rome".into(),
            detail: "potatoes reach Europe only after 1492".into(),
            chapters: vec!["One".into(), "Two".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(f.items[0].summary.contains("potatoes reach Europe"));
            assert_eq!(f.items[0].chapters, ["One", "Two"]);
        }
        _ => panic!("expected the findings overlay"),
    }
}

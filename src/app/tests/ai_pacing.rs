//! Pacing & tension reading (#1048): author-invoked, findings only — it POINTS OUT
//! the manuscript's scene-level tension/quiet rhythm as located, plain-language
//! observations and NEVER suggests a cut, an addition, a reorder, or reworded prose.
//! This pass sits closest to the co-writer line, so the prose-boundary guard is
//! central here. Hermetic throughout: a canned mock backend drives the pass, so no
//! test makes a live LLM call, hits the network, downloads a model, or reads the
//! keychain (project rule #531).

use super::*;

use std::sync::Mutex;

use crate::ai::pacing::{self, Pacing, PacingObservation};
use crate::ai::Backend;
use crate::mode::{Action, AiAction, FindingKind};

/// A backend that returns a canned reply (or an error) with no network — lets the
/// pacing pass be driven deterministically and offline. It records the last request
/// it saw, so a test can assert what reached the model (the prompt + the grounding
/// signal).
struct MockBackend {
    reply: Result<String, ()>,
    seen_system: Mutex<Option<String>>,
    seen_context: Mutex<Option<String>>,
}

impl MockBackend {
    fn ok(reply: &str) -> Self {
        Self {
            reply: Ok(reply.to_string()),
            seen_system: Mutex::new(None),
            seen_context: Mutex::new(None),
        }
    }
}

impl Backend for MockBackend {
    fn complete(&self, req: &ai::Request) -> Result<ai::Response, ai::AiError> {
        *self.seen_system.lock().unwrap() = req.system.clone();
        *self.seen_context.lock().unwrap() = Some(req.cached_context.clone());
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

/// Drive the pacing pass core with a mock backend the way the async worker path
/// does: build the request from the manuscript + the locally-computed pacing signal,
/// complete it, parse, and apply the result. Returns the mock so the caller can
/// inspect the request it saw.
fn run_pacing(app: &mut App, reply: &str) -> MockBackend {
    let backend = MockBackend::ok(reply);
    // Ground the reading with the deterministic signal, exactly as `analyze_pacing`
    // does — a compact per-chapter measure prepended to the manuscript context.
    let signal = pacing::signal_context(&[
        pacing::chapter_signal("One", &app.chapter_text(0)),
        pacing::chapter_signal("Two", &app.chapter_text(1)),
    ]);
    let req = pacing::build_request(app.manuscript_text(), &signal, &app.ai.model);
    let parsed = backend.complete(&req).and_then(|r| pacing::parse(&r.text));
    app.apply_pacing(parsed);
    backend
}

// --- The pass surfaces located observations in the shared triage surface -----

#[test]
fn a_pacing_observation_lands_in_triage_with_its_kind_and_named_stretch() {
    let (mut app, _dir) = two_chapter_app("pacing");
    let mock = run_pacing(
        &mut app,
        r#"{"observations":[
            {"observation":"three chapters of rising tension with no quiet beat to breathe","chapters":["One","Two"]}
        ]}"#,
    );

    // The pacing system prompt reached the model, and it's grounded on the measured
    // per-chapter signal.
    let system = mock.seen_system.lock().unwrap().clone().unwrap_or_default();
    assert!(system.contains("PACING & TENSION"), "{system}");
    let context = mock
        .seen_context
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default();
    assert!(
        context.contains("Per-chapter pacing signal"),
        "the reading is grounded on the measured signal: {context}"
    );

    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            // The finding carries the plain-language rhythm observation.
            assert!(
                f.items[0].summary.contains("no quiet beat to breathe"),
                "the rhythm observation: {}",
                f.items[0].summary
            );
            // It names the stretch it refers to.
            assert_eq!(
                f.items[0].chapters,
                ["One", "Two"],
                "names the stretch (chapters) it refers to"
            );
            // Classified under the pacing kind, so the health dashboard groups it.
            assert_eq!(f.items[0].kind, FindingKind::Pacing);
            assert_eq!(f.items[0].action, Some(AiAction::Pacing));
        }
        _ => panic!("expected the findings overlay"),
    }
    // It feeds revision triage like every other analysis pass.
    assert_eq!(app.triage.len(), 1);
    assert_eq!(app.triage[0].kind, FindingKind::Pacing);
}

#[test]
fn an_even_rhythm_yields_no_findings() {
    let (mut app, _dir) = two_chapter_app("pacing-even");
    // A manuscript whose tension/quiet rhythm reads even → the reading names nothing.
    run_pacing(&mut app, r#"{"observations":[]}"#);
    // An empty result is a clean "no findings" state, not a findings list.
    assert!(app.overlay.is_some(), "a clean summary overlay opens");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.triage.is_empty(), "nothing enters triage");
}

#[test]
fn a_deliberate_slow_passage_is_described_neutrally() {
    let (mut app, _dir) = two_chapter_app("pacing-slow");
    // A quiet interlude is reported as an observation of its rhythm, not as a problem
    // to fix — the model returns a neutral, located reading and it lands as-is.
    run_pacing(
        &mut app,
        r#"{"observations":[
            {"observation":"this stretch reads low-tension and reflective after the reveal","chapters":["Two"]}
        ]}"#,
    );
    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            let summary = &f.items[0].summary;
            // Described neutrally: what the rhythm reads like, its location.
            assert!(summary.contains("low-tension"), "{summary}");
            assert_eq!(f.items[0].chapters, ["Two"]);
            // Not framed as a problem/fix — no imperative to cut/add/tighten.
            for banned in [
                "cut ",
                "should ",
                "consider ",
                "tighten",
                "add a",
                "rewrite",
            ] {
                assert!(
                    !summary.to_lowercase().contains(banned),
                    "a deliberate slow passage is neutral, never a fix ({banned:?}): {summary}"
                );
            }
        }
        _ => panic!("expected the findings overlay"),
    }
}

#[test]
fn a_malformed_reply_reports_an_error_not_findings() {
    let (mut app, _dir) = two_chapter_app("pacing-bad");
    // A reply that ignores the schema is reported, never surfaced as findings.
    run_pacing(&mut app, "not json at all");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(
        app.message.as_deref().unwrap_or("").contains("AI:"),
        "{:?}",
        app.message
    );
    assert!(app.triage.is_empty());
}

// --- The pass produces observations only — never prose, never a suggested change

#[test]
fn the_pacing_pass_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("pacing-safe");
    let before = std::fs::read_to_string(dir.join("one.md")).unwrap();
    run_pacing(
        &mut app,
        r#"{"observations":[{"observation":"the middle sags","chapters":["One"]}]}"#,
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "a pacing reading observes rhythm, it never edits chapter prose"
    );
    // It writes no bible artifact either — a one-shot observation-only pass.
    assert!(
        !dir.join("bible/pacing.md").exists(),
        "the pacing pass keeps no bible file"
    );
}

#[test]
fn the_prompt_and_schema_forbid_suggesting_a_change() {
    // THE prose-boundary guard for the pass closest to the co-writer line: its system
    // prompt tells the model to observe rhythm and NEVER propose a change, and its
    // schema has NO field that could carry a suggested cut/addition/reorder/reword or
    // any prose — only the observation and the chapters it locates.
    let signal = pacing::signal_context(&[pacing::ChapterSignal {
        title: "One",
        words: 900,
        mean_sentence_words: 12.0,
        dialogue_pct: 20.0,
    }]);
    let req = pacing::build_request("m".into(), &signal, "claude-opus-4-8");
    let system = req.system.as_deref().unwrap_or("");
    assert!(
        system.contains("Do NOT propose ANY change"),
        "observation only, no proposed change: {system}"
    );
    assert!(
        system.contains("Do NOT suggest a cut"),
        "no cut / addition / reorder / reword: {system}"
    );
    // A deliberate slow passage is described neutrally, never as a problem to fix.
    assert!(
        system.contains("DELIBERATE") && system.contains("NEUTRALLY"),
        "deliberate slow passages are neutral observations: {system}"
    );

    // The schema carries only the observation + its location — no fix/prose field.
    let schema = pacing::schema();
    let props = &schema["properties"]["observations"]["items"]["properties"];
    let keys: std::collections::BTreeSet<&str> = props
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["chapters", "observation"].into_iter().collect(),
        "no field carries a suggested change or prose — only observation fields: {keys:?}"
    );
    for banned in [
        "suggestion",
        "fix",
        "rewrite",
        "revision",
        "change",
        "prose",
    ] {
        assert!(
            !props.as_object().unwrap().contains_key(banned),
            "the schema must not carry a `{banned}` field the model could fill with a change"
        );
    }
}

// --- Author-invoked, allowlisted, read-only ----------------------------------

#[test]
fn pacing_is_an_allowlisted_read_only_analysis_action() {
    // The assistant may run it (it produces observations, no prose and no suggested
    // change), and it takes no free-text argument — it reads the whole manuscript.
    let action = Action::Ai(AiAction::Pacing);
    assert!(
        action.assistant_runnable(),
        "read-only pacing analysis is allowlisted"
    );
    assert_eq!(
        action.text_arg(),
        None,
        "a whole-manuscript reading, no argument"
    );

    // It's a registry action that round-trips by its stable slug.
    assert_eq!(action.name(), "ai:pacing");
    assert!(
        matches!(
            Action::from_name("ai:pacing"),
            Some(Action::Ai(AiAction::Pacing))
        ),
        "the slug round-trips back to the pacing action"
    );

    // It's offered to the assistant as a no-argument tool.
    let tools = crate::ai::tools::assistant_tools();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "ai:pacing")
        .expect("pacing is offered to the assistant");
    assert!(
        tool["input_schema"]["properties"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "no free-text argument on the tool: {tool}"
    );
}

// A dedicated typed-result construction exercising the finding shape end-to-end,
// mirroring the foreshadow / accuracy pass tests (apply the parsed result directly).
#[test]
fn apply_pacing_maps_the_typed_observation_shape() {
    let (mut app, _dir) = two_chapter_app("pacing-typed");
    app.apply_pacing(Ok(Pacing {
        observations: vec![PacingObservation {
            observation: "the midpoint reveal is followed by a flat low-tension stretch".into(),
            chapters: vec!["One".into(), "Two".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(f.items[0].summary.contains("flat low-tension stretch"));
            assert_eq!(f.items[0].chapters, ["One", "Two"]);
            assert_eq!(f.items[0].kind, FindingKind::Pacing);
        }
        _ => panic!("expected the findings overlay"),
    }
}

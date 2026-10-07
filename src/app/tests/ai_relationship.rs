//! Relationship-trajectory pass (#1046): author-invoked, per a chosen character
//! pair, findings only — it tracks the arc and flags inconsistencies, never
//! rewrites the relationship. Hermetic throughout: a canned mock backend drives the
//! pass, so no test makes a live LLM call, hits the network, downloads a model, or
//! reads the keychain (project rule #531).

use super::*;

use std::sync::Mutex;

use crate::ai::relationship::{self, PairState, Trajectory, TrajectoryFinding};
use crate::ai::Backend;
use crate::mode::{Action, AiAction, FindingKind};

/// A backend that returns a canned reply (or an error) with no network — lets the
/// relationship pass be driven deterministically and offline. It records the last
/// system prompt it saw, so a test can assert the chosen pair reached the model.
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

/// Drive the relationship pass core with a mock backend the way the async worker
/// path does: build the request from the manuscript + the chosen pair, complete it,
/// parse, and apply the result. Returns the mock so the caller can inspect the
/// request it saw (e.g. that the pair was baked into the prompt).
fn run_relationship(app: &mut App, pair: &str, reply: &str) -> MockBackend {
    let backend = MockBackend::ok(reply);
    let req = relationship::build_request(app.manuscript_text(), pair, &app.ai.model);
    let parsed = backend
        .complete(&req)
        .and_then(|r| relationship::parse(&r.text));
    app.apply_relationship(parsed);
    backend
}

// --- The pass surfaces inconsistency findings in the shared triage surface ----

#[test]
fn relationship_inconsistencies_open_the_overlay_and_land_in_triage() {
    let (mut app, _dir) = two_chapter_app("relationship");
    let mock = run_relationship(
        &mut app,
        "Alice / Bob",
        r#"{
            "pair":"Alice / Bob",
            "states":[
                {"chapter":"One","state":"allied"},
                {"chapter":"Two","state":"estranged"}
            ],
            "findings":[
                {"kind":"off-page reversal","detail":"they turn estranged in Two with no on-page break","chapters":["One","Two"]}
            ]
        }"#,
    );

    // The chosen pair reached the model (baked into the system prompt).
    let system = mock.seen_system.lock().unwrap().clone().unwrap_or_default();
    assert!(system.contains("Alice / Bob"), "{system}");

    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            // The finding carries the inconsistency kind + the drift note.
            assert!(
                f.items[0].summary.contains("off-page reversal"),
                "names the kind of inconsistency: {}",
                f.items[0].summary
            );
            assert!(
                f.items[0].summary.contains("no on-page break"),
                "the drift note: {}",
                f.items[0].summary
            );
            // Names the chapters the drift spans.
            assert_eq!(f.items[0].chapters, ["One", "Two"], "names the chapters");
            // Classified under the relationship kind, so the dashboard groups it.
            assert_eq!(f.items[0].kind, FindingKind::Relationship);
            assert_eq!(f.items[0].action, Some(AiAction::Relationship));
        }
        _ => panic!("expected the findings overlay"),
    }
    // It feeds revision triage like every other analysis pass.
    assert_eq!(app.triage.len(), 1);
    assert_eq!(app.triage[0].kind, FindingKind::Relationship);
}

#[test]
fn a_consistent_arc_yields_no_findings() {
    let (mut app, _dir) = two_chapter_app("relationship-clean");
    // A trajectory that reads as consistent — states, but no inconsistencies.
    run_relationship(
        &mut app,
        "Alice / Bob",
        r#"{"pair":"Alice / Bob","states":[{"chapter":"One","state":"allied"},{"chapter":"Two","state":"warmer"}],"findings":[]}"#,
    );
    // An empty findings result is a clean "no findings" state, not a findings list.
    assert!(app.overlay.is_some(), "a clean summary overlay opens");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(
        app.triage.is_empty(),
        "nothing enters triage for a consistent arc"
    );
}

#[test]
fn a_malformed_reply_reports_an_error_not_findings() {
    let (mut app, _dir) = two_chapter_app("relationship-bad");
    // A reply that ignores the schema is reported, never surfaced as findings.
    run_relationship(&mut app, "Alice / Bob", "not json at all");
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
fn the_relationship_pass_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("relationship-safe");
    let before = std::fs::read_to_string(dir.join("one.md")).unwrap();
    run_relationship(
        &mut app,
        "Alice / Bob",
        r#"{"pair":"Alice / Bob","states":[{"chapter":"One","state":"allied"}],"findings":[{"kind":"contradiction","detail":"x","chapters":["One"]}]}"#,
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "a relationship pass flags, it never edits chapter prose"
    );
    // It writes no bible file either — pure analysis (contrast the character pass).
    assert!(
        !dir.join("bible").join("relationships.md").exists(),
        "the trajectory pass surfaces findings only; it writes no bible artifact"
    );
}

// --- Author-invoked: the palette prompts for the chosen pair ------------------

/// Enable AI with a key so the cost-gated palette flow can run, deterministic
/// regardless of env / config (test builds read neither, #531).
fn enable_ai(app: &mut App) {
    app.ai = ai::Settings::default();
    app.ai.enabled = true;
    app.ai.api_key = Some("k".into());
}

#[test]
fn the_action_prompts_for_a_pair() {
    let (mut app, _dir) = two_chapter_app("relationship-prompt");
    enable_ai(&mut app);
    app.run_ai_action(AiAction::Relationship, false);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::AiArg(AiAction::Relationship)));
            assert!(
                s.label.contains("Character pair"),
                "prompt asks for the chosen pair: {}",
                s.label
            );
        }
        _ => panic!("the relationship action prompts for a pair first"),
    }
}

#[test]
fn submitting_a_pair_runs_the_pass_and_captures_it() {
    let (mut app, _dir) = two_chapter_app("relationship-submit");
    enable_ai(&mut app);
    app.ai.cost_threshold = 1_000.0; // cheap enough to skip the cost confirm
    app.run_ai_action(AiAction::Relationship, false); // opens the pair prompt
    type_str(&mut app, "Alice / Bob");
    app.on_key(key(KeyCode::Enter)); // submit → captures the pair, dispatches
    assert!(
        app.pending_ai_arg.is_none(),
        "the pair is taken by the run, not left pending"
    );
    assert!(!matches!(app.mode, Mode::Prompt(_)), "the prompt closes");
}

#[test]
fn a_blank_pair_cancels() {
    let (mut app, _dir) = two_chapter_app("relationship-blank");
    enable_ai(&mut app);
    app.run_ai_action(AiAction::Relationship, false);
    app.on_key(key(KeyCode::Enter)); // empty pair
    assert!(app.pending_ai_arg.is_none());
    assert!(!app.ai_task.is_busy(), "nothing runs without a pair");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("character pair"),
        "{:?}",
        app.message
    );
}

// --- The two-layer prose guard: read-only, allowlisted, no prose -------------

#[test]
fn relationship_is_an_allowlisted_read_only_analysis_action() {
    // The assistant may run it (it produces findings, no prose), and it carries a
    // free-text "pair" argument the assistant supplies (#390/#1046).
    let action = Action::Ai(AiAction::Relationship);
    assert!(
        action.assistant_runnable(),
        "read-only relationship analysis is allowlisted"
    );
    assert_eq!(action.text_arg(), Some("pair"));

    // It's a registry action that round-trips by its stable slug.
    assert_eq!(action.name(), "ai:relationship");
    assert!(
        matches!(
            Action::from_name("ai:relationship"),
            Some(Action::Ai(AiAction::Relationship))
        ),
        "the slug round-trips back to the relationship action"
    );

    // Its tool definition declares the required pair argument.
    let tools = crate::ai::tools::assistant_tools();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "ai:relationship")
        .expect("relationship is offered to the assistant");
    assert_eq!(tool["input_schema"]["properties"]["pair"]["type"], "string");
    assert_eq!(
        tool["input_schema"]["required"],
        serde_json::json!(["pair"])
    );
}

#[test]
fn the_prompt_and_schema_forbid_rewriting_the_arc() {
    // The pass is analysis-only by construction: its system prompt tells the model to
    // observe and flag, never to rewrite or propose an arc, and its schema has no
    // field that could carry edited manuscript text — only tracking state labels and
    // observation fields.
    let req = relationship::build_request("m".into(), "Alice / Bob", "claude-opus-4-8");
    let system = req.system.as_deref().unwrap_or("");
    assert!(
        system.contains("Do NOT rewrite the arc"),
        "flags only: {system}"
    );
    assert!(
        system.contains("propose"),
        "the prompt forbids proposing a fixed arc: {system}"
    );

    let schema = relationship::schema();
    // No state field carries prose.
    let state_keys: Vec<&str> = schema["properties"]["states"]["items"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        state_keys
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        ["chapter", "state"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "states carry only a chapter + a short label: {state_keys:?}"
    );
    // No finding field carries prose.
    let finding_keys: Vec<&str> = schema["properties"]["findings"]["items"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        finding_keys
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        ["chapters", "detail", "kind"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "no field carries edited prose — only observation fields: {finding_keys:?}"
    );
}

// A dedicated typed-result construction exercising the finding shape end-to-end,
// mirroring the accuracy pass test (apply the parsed result directly).
#[test]
fn apply_relationship_maps_the_typed_finding_shape() {
    let (mut app, _dir) = two_chapter_app("relationship-typed");
    app.apply_relationship(Ok(Trajectory {
        pair: "Alice / Bob".into(),
        states: vec![
            PairState {
                chapter: "One".into(),
                state: "allied".into(),
            },
            PairState {
                chapter: "Two".into(),
                state: "estranged".into(),
            },
        ],
        findings: vec![TrajectoryFinding {
            kind: "contradiction".into(),
            detail: "cold in Two with no prior break".into(),
            chapters: vec!["One".into(), "Two".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(f.items[0].summary.contains("contradiction"));
            assert!(f.items[0].summary.contains("cold in Two"));
            assert_eq!(f.items[0].chapters, ["One", "Two"]);
        }
        _ => panic!("expected the findings overlay"),
    }
}

//! Research-notebook fact-check pass (#1049): author-invoked, findings only —
//! flags manuscript passages that contradict a note the author wrote in
//! `bible/research.md` (#932). Grounded solely in the author's own ledger — it
//! introduces no external facts and makes no sensitivity / representation judgment,
//! which keeps it distinct from the cultural & historical accuracy pass (#973).
//!
//! Hermetic throughout: a canned mock backend drives the pass, so no test makes a
//! live LLM call, hits the network, downloads a model, or reads the keychain
//! (project rule #531). The manuscript and the seeded notebook are the only inputs.

use super::*;

use std::fs;
use std::sync::Mutex;

use crate::ai::bible;
use crate::ai::factcheck::{self, FactCheck, FactCheckFinding};
use crate::ai::Backend;
use crate::mode::{Action, AiAction, FindingKind};

/// A backend that returns a canned reply (or an error) with no network — lets the
/// fact-check pass be driven deterministically and offline. It records the last
/// request it saw, so a test can assert the author's notebook reached the model and
/// that nothing external was smuggled in.
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

/// Seed the author's research notebook with `notes` under `bible/research.md`, the
/// same author-editable file the notebook view writes (#932).
fn seed_research(dir: &std::path::Path, notes: &str) {
    let path = bible::research_path(dir);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, notes).unwrap();
}

/// Drive the fact-check pass core with a mock backend the way the async worker path
/// does: read the seeded notebook, build the request from the manuscript + notes,
/// complete it, parse, and apply. Returns the mock so the caller can inspect the
/// request it saw (e.g. that the notebook was baked into the prompt).
fn run_factcheck(app: &mut App, dir: &std::path::Path, reply: &str) -> MockBackend {
    let backend = MockBackend::ok(reply);
    let research = fs::read_to_string(bible::research_path(dir)).unwrap_or_default();
    let req = factcheck::build_request(app.manuscript_text(), &research, &app.ai.model);
    let parsed = backend
        .complete(&req)
        .and_then(|r| factcheck::parse(&r.text));
    app.apply_factcheck(parsed);
    backend
}

// --- A contradiction of a note lands, citing the note + chapter --------------

#[test]
fn a_contradiction_of_a_note_lands_citing_the_note_and_chapter() {
    let (mut app, dir) = two_chapter_app("factcheck-hit");
    seed_research(
        &dir,
        "# Research notebook\n\n## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n",
    );
    let mock = run_factcheck(
        &mut app,
        &dir,
        r#"{"findings":[
            {"note":"Feudal Timeline","passage":"Lord Grey, ruling since 1470","detail":"the note dates his reign 1432-1456","chapters":["Two"]}
        ]}"#,
    );

    // The author's notebook content reached the model (baked into the system
    // prompt) — the pass checks against the author's own notes.
    let system = mock.seen_system.lock().unwrap().clone().unwrap_or_default();
    assert!(system.contains("Feudal Timeline"), "{system}");
    assert!(system.contains("Lord Grey ruled 1432-1456"), "{system}");

    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            // The finding cites the contradicted research note by title…
            assert!(
                f.items[0]
                    .summary
                    .contains("per research note 'Feudal Timeline'"),
                "cites the note: {}",
                f.items[0].summary
            );
            // …explains the contradiction…
            assert!(
                f.items[0].summary.contains("1432-1456"),
                "explains the contradiction: {}",
                f.items[0].summary
            );
            // …and quotes the offending passage.
            assert!(
                f.items[0].summary.contains("ruling since 1470"),
                "quotes the flagged passage: {}",
                f.items[0].summary
            );
            assert_eq!(f.items[0].chapters, ["Two"], "anchored to its chapter");
            // Classified under the fact-check kind, so the health dashboard groups it.
            assert_eq!(f.items[0].kind, FindingKind::FactCheck);
            assert_eq!(f.items[0].action, Some(AiAction::FactCheck));
        }
        _ => panic!("expected the findings overlay"),
    }
    // It feeds revision triage like every other analysis pass.
    assert_eq!(app.triage.len(), 1);
    assert_eq!(app.triage[0].kind, FindingKind::FactCheck);
}

// --- A manuscript consistent with the notes yields no findings ---------------

#[test]
fn prose_consistent_with_the_notes_yields_no_findings() {
    let (mut app, dir) = two_chapter_app("factcheck-clean");
    seed_research(
        &dir,
        "# Research notebook\n\n## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n",
    );
    run_factcheck(&mut app, &dir, r#"{"findings":[]}"#);
    // An empty result is a clean "no findings" state, not a findings list.
    assert!(app.overlay.is_some(), "a clean summary overlay opens");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.triage.is_empty(), "nothing enters triage");
}

#[test]
fn a_malformed_reply_reports_an_error_not_findings() {
    let (mut app, dir) = two_chapter_app("factcheck-bad");
    seed_research(&dir, "# Research notebook\n\n## N\n\nbody\n");
    // A reply that ignores the schema is reported, never surfaced as findings.
    run_factcheck(&mut app, &dir, "not json at all");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(
        app.message.as_deref().unwrap_or("").contains("AI:"),
        "{:?}",
        app.message
    );
    assert!(app.triage.is_empty());
}

// --- Grounded only in the notebook: no external facts, no notebook = no run ---

#[test]
fn the_pass_checks_only_the_authors_notes_never_external_facts() {
    // The prompt is seeded with the author's notes and told to check ONLY against
    // them, introducing no facts of its own — the line that keeps this from being
    // (or drifting into) the accuracy pass's external-knowledge check.
    let (app, dir) = two_chapter_app("factcheck-grounded");
    seed_research(
        &dir,
        "# Research notebook\n\n## Magic\n\nSpells cost blood.\n",
    );
    let research = fs::read_to_string(bible::research_path(&dir)).unwrap();
    let req = factcheck::build_request(app.manuscript_text(), &research, &app.ai.model);
    let system = req.system.as_deref().unwrap_or("");
    // Only the author's ledger is the source of truth.
    assert!(
        system.contains("Spells cost blood"),
        "seeds the notes: {system}"
    );
    assert!(
        system.contains("Do NOT introduce facts of your own"),
        "no external facts: {system}"
    );
    // Explicitly NOT a sensitivity review — distinct from the accuracy pass.
    assert!(system.contains("NOT a sensitivity"), "{system}");
}

#[test]
fn an_empty_notebook_is_a_no_op_with_a_nudge() {
    // With no authored notes there is no basis to check against, so the pass never
    // fires a (costed) request — it nudges the author to write notes first.
    let (mut app, _dir) = two_chapter_app("factcheck-empty");
    enable_ai(&mut app);
    app.ai.cost_threshold = 1_000.0;
    app.analyze_factcheck();
    assert!(
        !app.ai_task.is_busy(),
        "nothing runs without authored notes"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("research notebook"),
        "{:?}",
        app.message
    );
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

// --- The pass produces findings only — never prose ---------------------------

#[test]
fn the_factcheck_pass_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("factcheck-safe");
    seed_research(&dir, "# Research notebook\n\n## N\n\nX happened in 1200.\n");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    run_factcheck(
        &mut app,
        &dir,
        r#"{"findings":[{"note":"N","passage":"y","detail":"contradicts the note","chapters":["One"]}]}"#,
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "a fact-check pass flags, it never edits chapter prose"
    );
}

#[test]
fn the_notebook_itself_is_never_written() {
    // The notebook is the author's — the pass reads it to check against, never edits
    // it (the no-AI-write invariant #932 the whole feature rides on).
    let (mut app, dir) = two_chapter_app("factcheck-notebook-safe");
    let notes = "# Research notebook\n\n## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n";
    seed_research(&dir, notes);
    run_factcheck(
        &mut app,
        &dir,
        r#"{"findings":[{"note":"Feudal Timeline","passage":"1470","detail":"off","chapters":["Two"]}]}"#,
    );
    assert_eq!(
        fs::read_to_string(bible::research_path(&dir)).unwrap(),
        notes,
        "the fact-check reads the notebook, it never rewrites it"
    );
}

// --- Author-invoked, no argument, allowlisted read-only ----------------------

/// Enable AI with a key so the cost-gated palette flow can run, deterministic
/// regardless of env / config (test builds read neither, #531).
fn enable_ai(app: &mut App) {
    app.ai = ai::Settings::default();
    app.ai.enabled = true;
    app.ai.api_key = Some("k".into());
}

#[test]
fn the_action_takes_no_argument_and_runs_against_the_notebook() {
    // Unlike the accuracy pass (which prompts for a named external concern), the
    // fact-check has no argument — it checks against the author's own notebook, so
    // running it fires the pass directly (no prompt).
    let (mut app, dir) = two_chapter_app("factcheck-noarg");
    seed_research(&dir, "# Research notebook\n\n## N\n\nnote body\n");
    enable_ai(&mut app);
    app.ai.cost_threshold = 1_000.0; // cheap enough to skip the cost confirm
    app.run_ai_action(AiAction::FactCheck, false);
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no argument prompt opens"
    );
}

#[test]
fn factcheck_is_an_allowlisted_read_only_analysis_action() {
    // The assistant may run it (it produces findings, no prose), and it takes no
    // free-text argument (it reads `bible/research.md` directly).
    let action = Action::Ai(AiAction::FactCheck);
    assert!(
        action.assistant_runnable(),
        "read-only fact-check analysis is allowlisted"
    );
    assert_eq!(
        action.text_arg(),
        None,
        "no argument — it reads the notebook"
    );

    // It's a registry action that round-trips by its stable slug.
    assert_eq!(action.name(), "ai:factcheck");
    assert!(
        matches!(
            Action::from_name("ai:factcheck"),
            Some(Action::Ai(AiAction::FactCheck))
        ),
        "the slug round-trips back to the fact-check action"
    );

    // Its tool definition is offered to the assistant with no required argument.
    let tools = crate::ai::tools::assistant_tools();
    assert!(
        tools.iter().any(|t| t["name"] == "ai:factcheck"),
        "fact-check is offered to the assistant"
    );
}

#[test]
fn the_prompt_and_schema_forbid_rewriting_prose() {
    // The pass is analysis-only by construction: its system prompt tells the model to
    // observe and flag, never to rewrite or supply replacement prose, and its schema
    // has no field that could carry edited manuscript text — only the cited note, the
    // flagged passage, a why-note, and chapters.
    let req = factcheck::build_request("m".into(), "## N\n\nbody\n", "claude-opus-4-8");
    let system = req.system.as_deref().unwrap_or("");
    assert!(system.contains("Do NOT rewrite"), "flags only: {system}");

    let schema = factcheck::schema();
    let props = &schema["properties"]["findings"]["items"]["properties"];
    let keys: std::collections::BTreeSet<&str> = props
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["chapters", "detail", "note", "passage"]
            .into_iter()
            .collect(),
        "no field carries edited prose — only observation fields: {keys:?}"
    );
}

// A dedicated typed-result construction exercising the finding shape end-to-end,
// mirroring the style/voice/accuracy pass tests (apply the parsed result directly).
#[test]
fn apply_factcheck_maps_the_typed_finding_shape() {
    let (mut app, _dir) = two_chapter_app("factcheck-typed");
    app.apply_factcheck(Ok(FactCheck {
        findings: vec![FactCheckFinding {
            note: "Feudal Timeline".into(),
            passage: "he took the throne in 1470".into(),
            detail: "the note dates his reign 1432-1456".into(),
            chapters: vec!["One".into(), "Two".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(f.items[0]
                .summary
                .contains("per research note 'Feudal Timeline'"));
            assert!(f.items[0].summary.contains("1432-1456"));
            assert_eq!(f.items[0].chapters, ["One", "Two"]);
        }
        _ => panic!("expected the findings overlay"),
    }
}

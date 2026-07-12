//! Foreshadow / payoff scan (#1047): author-invoked, findings only — it flags a
//! concrete seeded detail that never pays off and NEVER drafts the payoff. Hermetic
//! throughout: a canned mock backend drives the pass, so no test makes a live LLM
//! call, hits the network, downloads a model, or reads the keychain (project rule
//! #531). It is distinct from — and must not duplicate — the thematic-thread
//! tracking in `plot.rs`.

use super::*;

use std::sync::Mutex;

use crate::ai::foreshadow::{self, Foreshadow, UnpaidSetup};
use crate::ai::Backend;
use crate::mode::{Action, AiAction, FindingKind};

/// A backend that returns a canned reply (or an error) with no network — lets the
/// foreshadow pass be driven deterministically and offline. It records the last
/// system prompt it saw so a test can assert what reached the model.
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

/// Drive the foreshadow pass core with a mock backend the way the async worker path
/// does: build the request from the manuscript, complete it, parse, and apply the
/// result. Returns the mock so the caller can inspect the request it saw.
fn run_foreshadow(app: &mut App, reply: &str) -> MockBackend {
    let backend = MockBackend::ok(reply);
    let req = foreshadow::build_request(app.manuscript_text(), &app.ai.model);
    let parsed = backend
        .complete(&req)
        .and_then(|r| foreshadow::parse(&r.text));
    app.apply_foreshadow(parsed);
    backend
}

// --- The pass surfaces findings in the shared triage surface -----------------

#[test]
fn an_unpaid_setup_finding_lands_in_triage_with_its_kind_passage_and_chapter() {
    let (mut app, _dir) = two_chapter_app("foreshadow");
    let mock = run_foreshadow(
        &mut app,
        r#"{"findings":[
            {"setup":"she pocketed the brass key","detail":"the brass key is planted in ch. 1 but never used or mentioned again","chapters":["One"]}
        ]}"#,
    );

    // The foreshadow system prompt reached the model — a concrete-detail scan.
    let system = mock.seen_system.lock().unwrap().clone().unwrap_or_default();
    assert!(system.contains("CONCRETE"), "{system}");

    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            // The finding carries the why-it's-unpaid note and quotes the setup passage.
            assert!(
                f.items[0].summary.contains("never used or mentioned again"),
                "why-it's-unpaid note: {}",
                f.items[0].summary
            );
            assert!(
                f.items[0].summary.contains("pocketed the brass key"),
                "quotes the setup passage: {}",
                f.items[0].summary
            );
            // Anchored to the setup chapter, and classified under the foreshadow kind.
            assert_eq!(f.items[0].chapters, ["One"], "names the setup chapter");
            assert_eq!(f.items[0].kind, FindingKind::Foreshadow);
            assert_eq!(f.items[0].action, Some(AiAction::Foreshadow));
        }
        _ => panic!("expected the findings overlay"),
    }
    // It feeds revision triage like every other analysis pass.
    assert_eq!(app.triage.len(), 1);
    assert_eq!(app.triage[0].kind, FindingKind::Foreshadow);
}

#[test]
fn a_book_where_everything_pays_off_yields_no_findings() {
    let (mut app, _dir) = two_chapter_app("foreshadow-clean");
    // Everything set up is paid off → the model returns no unpaid setups.
    run_foreshadow(&mut app, r#"{"findings":[]}"#);
    // An empty result is a clean "no findings" state, not a findings list.
    assert!(app.overlay.is_some(), "a clean summary overlay opens");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.triage.is_empty(), "nothing enters triage");
}

#[test]
fn a_malformed_reply_reports_an_error_not_findings() {
    let (mut app, _dir) = two_chapter_app("foreshadow-bad");
    // A reply that ignores the schema is reported, never surfaced as findings.
    run_foreshadow(&mut app, "not json at all");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(
        app.message.as_deref().unwrap_or("").contains("AI:"),
        "{:?}",
        app.message
    );
    assert!(app.triage.is_empty());
}

// --- Distinct from plot.rs: concrete detail, not a thematic thread -----------

#[test]
fn foreshadow_findings_are_kept_distinct_from_plot_thread_findings() {
    let (mut app, dir) = two_chapter_app("foreshadow-vs-plot");

    // The plot pass tracks a *thematic* thread and writes a bible artifact.
    app.apply_plot(Ok(plot::Plot {
        threads: vec![plot::Thread {
            name: "Trust vs. duty".into(),
            status: plot::ThreadStatus::Open,
            setup: "framed in ch. 1".into(),
            payoff: String::new(),
            last_seen: "One".into(),
            notes: String::new(),
        }],
        findings: vec![plot::ThreadFinding {
            thread: "Trust vs. duty".into(),
            issue: "throughline left open".into(),
            chapters: vec!["One".into()],
        }],
    }));
    assert!(
        dir.join("bible/plot.md").exists(),
        "the plot pass writes its thematic-thread bible"
    );

    // The foreshadow pass flags a *concrete* dropped detail — a different lens.
    run_foreshadow(
        &mut app,
        r#"{"findings":[{"setup":"a loaded revolver on the mantel","detail":"the revolver is never fired or referenced again","chapters":["One"]}]}"#,
    );

    // The foreshadow finding is classified under its own kind, never Plot — the two
    // stay distinct in triage, so a concrete dropped detail is not folded into the
    // thematic-thread tracker's findings (and vice-versa).
    let foreshadow: Vec<_> = app
        .triage
        .iter()
        .filter(|f| f.kind == FindingKind::Foreshadow)
        .collect();
    let plot_count = app
        .triage
        .iter()
        .filter(|f| f.kind == FindingKind::Plot)
        .count();
    assert_eq!(foreshadow.len(), 1, "one concrete unpaid-setup finding");
    assert_eq!(plot_count, 1, "one thematic-thread finding, kept separate");
    assert!(
        foreshadow[0].summary.contains("revolver"),
        "the concrete detail, not the theme: {}",
        foreshadow[0].summary
    );
    // The foreshadow pass writes no bible artifact of its own — it is a one-shot
    // scan, not a persistent thread tracker like plot.rs.
    assert!(
        !dir.join("bible/foreshadow.md").exists(),
        "the foreshadow scan keeps no promise-ledger bible file"
    );
}

// --- The pass produces findings only — never prose, never a drafted payoff ---

#[test]
fn the_foreshadow_pass_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("foreshadow-safe");
    let before = std::fs::read_to_string(dir.join("one.md")).unwrap();
    run_foreshadow(
        &mut app,
        r#"{"findings":[{"setup":"x","detail":"an unpaid clue","chapters":["One"]}]}"#,
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "a foreshadow scan flags, it never edits chapter prose"
    );
}

#[test]
fn the_prompt_and_schema_forbid_drafting_the_payoff() {
    // The pass is analysis-only by construction: its system prompt tells the model to
    // observe and flag, never to draft or suggest the payoff, and its schema has no
    // field that could carry a drafted payoff or edited prose — only the flagged
    // setup passage, a why-note, and chapters.
    let req = foreshadow::build_request("m".into(), "claude-opus-4-8");
    let system = req.system.as_deref().unwrap_or("");
    assert!(
        system.contains("Do NOT draft, suggest, or write the payoff"),
        "flags only, never a drafted payoff: {system}"
    );
    // The distinction from the plot-thread pass is in the prompt.
    assert!(
        system.contains("Do NOT flag broad thematic threads"),
        "distinct from the plot pass: {system}"
    );

    let schema = foreshadow::schema();
    let props = &schema["properties"]["findings"]["items"]["properties"];
    let keys: std::collections::BTreeSet<&str> = props
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["chapters", "detail", "setup"].into_iter().collect(),
        "no field carries a drafted payoff or edited prose — only observation fields: {keys:?}"
    );
}

// --- Author-invoked, allowlisted, read-only ----------------------------------

#[test]
fn foreshadow_is_an_allowlisted_read_only_analysis_action() {
    // The assistant may run it (it produces findings, no prose and no drafted
    // payoff), and it takes no free-text argument — it scans the whole manuscript.
    let action = Action::Ai(AiAction::Foreshadow);
    assert!(
        action.assistant_runnable(),
        "read-only foreshadow analysis is allowlisted"
    );
    assert_eq!(
        action.text_arg(),
        None,
        "a whole-manuscript scan, no argument"
    );

    // It's a registry action that round-trips by its stable slug.
    assert_eq!(action.name(), "ai:foreshadow");
    assert!(
        matches!(
            Action::from_name("ai:foreshadow"),
            Some(Action::Ai(AiAction::Foreshadow))
        ),
        "the slug round-trips back to the foreshadow action"
    );

    // It's offered to the assistant as a no-argument tool.
    let tools = crate::ai::tools::assistant_tools();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "ai:foreshadow")
        .expect("foreshadow is offered to the assistant");
    assert!(
        tool["input_schema"]["properties"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "no free-text argument on the tool: {tool}"
    );
}

// A dedicated typed-result construction exercising the finding shape end-to-end,
// mirroring the accuracy / style pass tests (apply the parsed result directly).
#[test]
fn apply_foreshadow_maps_the_typed_finding_shape() {
    let (mut app, _dir) = two_chapter_app("foreshadow-typed");
    app.apply_foreshadow(Ok(Foreshadow {
        findings: vec![UnpaidSetup {
            setup: "the sealed letter in the drawer".into(),
            detail: "the sealed letter is never opened or its contents revealed".into(),
            chapters: vec!["One".into(), "Two".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(f.items[0].summary.contains("never opened"));
            assert!(f.items[0].summary.contains("sealed letter in the drawer"));
            assert_eq!(f.items[0].chapters, ["One", "Two"]);
        }
        _ => panic!("expected the findings overlay"),
    }
}

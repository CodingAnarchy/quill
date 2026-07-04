//! Tests for the authorial assistant pane (#307): opening / closing it via the
//! window system, focus and input handling, and the UI-thread turn machine
//! (final answers, tool execution, and the async-pass batch of #392), driven
//! without a live backend.

use super::*;
use crate::ai::router::Policy;
use crate::ai::tool_loop::{ToolCall, Turn};
use crate::ai::{AiError, BackendKind};
use crate::app::{Assistant, Speaker, SplitContent};

/// Send a Vim window command: the `Ctrl-w` prefix then `c`.
fn window_cmd(app: &mut App, c: char) {
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char(c)));
}

/// The docked assistant pane's conversation (the state now lives in the split).
fn conv(app: &App) -> &Assistant {
    match app.split_view().expect("a docked pane").content {
        SplitContent::Assistant { conv, .. } => conv,
        _ => panic!("the docked pane is not the assistant"),
    }
}

/// An assistant pane open on a fresh two-chapter project, with no API key so the
/// turn machine never spawns a real network call in tests.
fn assistant_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.ai.enabled = true;
    app.ai.api_key = None; // resolved_key() is None → no off-thread network turn
    window_cmd(&mut app, 'a');
    (app, dir)
}

/// An assistant pane where every AI pass is "over threshold" (cost 0), so a pass
/// always asks for confirmation in the conversation (#392). A key is present so
/// the pass is *available* and reaches the cost confirm; tests null it before any
/// step that would actually spawn, so nothing networks.
fn confirm_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.ai.enabled = true;
    app.ai.api_key = Some("test-key".into());
    app.ai.offline = false;
    app.ai.backend = BackendKind::Claude;
    app.ai.routing = Policy::Configured;
    app.ai.cost_threshold = 0.0;
    window_cmd(&mut app, 'a');
    (app, dir)
}

/// An assistant pane on the local backend (offline), where AI actions are
/// *available* without an API key — so the argument checks run — but the actual
/// spawn no-ops in a build without the `local` feature, so nothing networks.
fn offline_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.ai.enabled = true;
    app.ai.offline = true;
    window_cmd(&mut app, 'a');
    (app, dir)
}

/// A tool call with an empty input, the registry-derived shape.
fn call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        input: serde_json::json!({}),
    }
}

/// A tool call carrying typed arguments (#390).
fn call_with(id: &str, name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        input,
    }
}

#[test]
fn ctrl_w_a_opens_the_pane_and_ctrl_w_c_closes_it() {
    let (mut app, _dir) = two_chapter_app("assistant-open-close");
    assert!(app.split_view().is_none());
    window_cmd(&mut app, 'a');
    assert!(
        app.split_view().is_some(),
        "Ctrl-w a docks the assistant pane"
    );
    assert!(app.assistant_pane_focused(), "the new pane takes focus");
    window_cmd(&mut app, 'c');
    assert!(app.split_view().is_none(), "Ctrl-w c closes the pane");
    assert!(!app.assistant_pane_focused());
}

#[test]
fn typing_edits_the_request_line_and_esc_hands_focus_back() {
    let (mut app, _dir) = assistant_app("assistant-typing");
    type_str(&mut app, "hello");
    assert_eq!(conv(&app).input(), "hello");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(conv(&app).input(), "hell");
    // Esc (idle) unfocuses the pane but keeps it open and its conversation intact.
    app.on_key(key(KeyCode::Esc));
    assert!(
        !app.assistant_pane_focused(),
        "Esc hands keys back to the editor"
    );
    assert!(app.split_view().is_some(), "the pane stays docked");
}

#[test]
fn a_final_answer_is_shown_in_the_transcript() {
    let (mut app, _dir) = assistant_app("assistant-final");
    app.apply_assistant_turn(Ok(Turn::Final("All set.".into())));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::Assistant && e.text == "All set."),
        "the model's reply is shown",
    );
    assert!(!c.is_running(), "the request is done");
}

#[test]
fn a_tool_call_runs_the_action_and_records_it() {
    let (mut app, _dir) = assistant_app("assistant-tool");
    let before = app.focus_mode;
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "view:focus")])));
    assert_ne!(app.focus_mode, before, "the toggle action actually ran");
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::Tool && e.text.contains("view:focus")),
        "the pane shows what it ran",
    );
}

#[test]
fn an_unrunnable_tool_is_refused_without_running() {
    let (mut app, _dir) = assistant_app("assistant-refuse");
    // `book:switch` is a modal picker, outside the assistant's catalog; if the
    // model asks for it anyway, it's refused rather than dispatched.
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "book:switch")])));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::Tool && e.text.contains("not a permitted")),
        "an action outside the catalog is declined",
    );
}

#[test]
fn an_error_turn_is_surfaced() {
    let (mut app, _dir) = assistant_app("assistant-error");
    app.apply_assistant_turn(Err(AiError::Network {
        status: Some(500),
        message: "boom".into(),
    }));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::System && e.text.contains("boom")),
        "the network error is shown",
    );
    assert!(!c.is_running());
}

#[test]
fn the_turn_cap_stops_a_runaway_loop() {
    let (mut app, _dir) = assistant_app("assistant-cap");
    // Keep asking for a tool past the cap; the machine stops and says so.
    for _ in 0..(crate::ai::tool_loop::MAX_TURNS + 1) {
        app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "view:focus")])));
    }
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::System && e.text.contains("Stopped after")),
        "the loop is bounded by MAX_TURNS",
    );
}

#[test]
fn submitting_with_ai_off_explains_rather_than_failing() {
    let (mut app, _dir) = two_chapter_app("assistant-ai-off");
    app.ai.enabled = false;
    window_cmd(&mut app, 'a');
    type_str(&mut app, "do something");
    app.on_key(key(KeyCode::Enter));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::System && e.text.contains("unavailable")),
        "the pane explains AI is off instead of erroring obscurely",
    );
    assert!(!c.is_running(), "nothing was spawned");
}

#[test]
fn the_catalog_is_the_runnable_subset_of_the_registry() {
    use crate::ai::tools::assistant_tools;
    use crate::mode::Action;

    let tools = assistant_tools();
    let runnable = Action::ALL
        .iter()
        .filter(|a| a.assistant_runnable())
        .count();
    assert_eq!(tools.len(), runnable, "one tool per runnable action");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"save"), "a synchronous action is offered");
    assert!(
        names.contains(&"ai:continuity"),
        "an analysis pass is offered (#392)"
    );
    assert!(
        names.contains(&"ai:ask"),
        "a text-argument action is offered (#390)"
    );
    assert!(
        !names.contains(&"ai:thesaurus"),
        "a cursor-context AI action is not offered"
    );
    assert!(
        !names.contains(&"book:switch"),
        "a modal picker is not offered"
    );
}

#[test]
fn an_ai_pass_asks_for_cost_confirmation_in_the_conversation() {
    let (mut app, _dir) = confirm_app("assistant-pass-confirm");
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "ai:continuity")])));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::System && e.text.contains("Reply 'y'")),
        "a costly pass asks for confirmation in the pane rather than firing silently",
    );
    assert!(
        !c.is_running(),
        "nothing is running while it waits for the answer"
    );
}

#[test]
fn skipping_a_pass_records_it_and_continues() {
    let (mut app, _dir) = confirm_app("assistant-pass-skip");
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "ai:continuity")])));
    app.ai.api_key = None; // the post-skip turn must not network
                           // Decline the confirm: the input is the answer, not a new request.
    type_str(&mut app, "n");
    app.on_key(key(KeyCode::Enter));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::Tool && e.text.contains("skipped")),
        "declining the cost confirm skips the pass",
    );
}

#[test]
fn canceling_a_pending_confirm_clears_it() {
    let (mut app, _dir) = confirm_app("assistant-pass-cancel");
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "ai:continuity")])));
    app.on_key(key(KeyCode::Esc)); // cancels the whole request, dropping the confirm
    app.ai.api_key = None; // the fresh request must not network
                           // A following line is a fresh request, not the (now-cleared) confirm answer.
    type_str(&mut app, "hello");
    app.on_key(key(KeyCode::Enter));
    let c = conv(&app);
    assert!(
        c.entries().iter().any(|e| e.text == "Canceled."),
        "Esc cancels the pending pass"
    );
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::System && e.text.contains("unavailable")),
        "the next line is treated as a new request, so the confirm was cleared",
    );
}

// --- Per-action tool inputs (#390) ---------------------------------------

#[test]
fn text_argument_tools_declare_a_typed_input_schema() {
    use crate::ai::tools::assistant_tools;
    let tools = assistant_tools();
    let by = |name: &str| tools.iter().find(|t| t["name"] == name).cloned().unwrap();
    // Ask takes a required "question"; an argument-free action keeps the empty schema.
    let ask = by("ai:ask");
    assert_eq!(
        ask["input_schema"]["properties"]["question"]["type"],
        "string"
    );
    assert_eq!(
        ask["input_schema"]["required"],
        serde_json::json!(["question"])
    );
    let save = by("save");
    assert_eq!(save["input_schema"]["properties"], serde_json::json!({}));
}

#[test]
fn a_text_action_without_its_argument_is_refused() {
    let (mut app, _dir) = offline_app("assistant-arg-missing");
    // `ai:ask` is available offline but carries no question — refused, not fired.
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "ai:ask")])));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::Tool && e.text.contains("needs a 'question'")),
        "a text action with no argument is refused rather than opening a prompt",
    );
}

#[test]
fn a_question_answer_comes_back_in_the_conversation() {
    use crate::ai::{Outcome, Response, Usage};
    let (mut app, _dir) = assistant_app("assistant-ask-answer");
    // A Q&A (ai:ask) is in flight; its answer should be the tool result, in full.
    app.test_install_pass(
        call_with(
            "q",
            "ai:ask",
            serde_json::json!({ "question": "Who is the villain?" }),
        ),
        vec![],
    );
    let resp = Response {
        text: "Lord Marrow is the villain.".into(),
        usage: Usage::default(),
    };
    let summary = app
        .assistant_pass_summary(&Outcome::Answer(Ok(resp)))
        .expect("a summary while awaiting");
    assert_eq!(
        summary, "Lord Marrow is the villain.",
        "the answer itself comes back, not a generic note"
    );
}

// --- Assistant as a registry action (#399) -------------------------------

#[test]
fn the_assistant_is_a_registry_action_that_opens_the_pane() {
    use crate::mode::Action;
    let (mut app, _dir) = two_chapter_app("assistant-action-open");
    assert!(
        Action::from_name("assistant").is_some(),
        "resolvable by name for Ctrl-K / workflows"
    );
    assert!(app.split_view().is_none());
    app.dispatch_action(Action::Assistant, false); // the run-action / workflow path
    assert!(
        app.assistant_pane_focused(),
        "running the action opens the assistant pane"
    );
}

#[test]
fn the_assistant_action_is_not_offered_to_the_assistant() {
    use crate::ai::tools::assistant_tools;
    use crate::mode::Action;
    // The recursion guard: opening the pane is a registry action, but the model is
    // never given it as a tool, so it can't open panes recursively.
    assert!(
        !Action::Assistant.assistant_runnable(),
        "the assistant can't run itself"
    );
    let tools = assistant_tools();
    let mut names = tools.iter().map(|t| t["name"].as_str().unwrap());
    assert!(
        !names.any(|n| n == "assistant"),
        "the open-assistant action isn't in the tool catalog"
    );
}

#[test]
fn the_assistant_action_is_unavailable_in_the_continuous_view() {
    use crate::mode::Action;
    let (mut app, _dir) = three_chapter_app("assistant-action-continuous");
    assert!(
        app.action_status(Action::Assistant).is_none(),
        "available in the single-chapter view"
    );
    app.on_key(ctrl('l')); // enter the continuous manuscript view
    assert!(
        app.action_status(Action::Assistant).is_some(),
        "the run-action menu shows the assistant as unavailable in the continuous view",
    );
}

// --- Multi-turn cost control (#309) --------------------------------------

#[test]
// A local run costs exactly `0.0` — the free-run sentinel is an exact value, so an
// exact compare is the intended check.
#[allow(clippy::float_cmp)]
fn an_on_device_run_is_free_so_never_gated() {
    let (app, _dir) = offline_app("budget-local");
    assert_eq!(
        app.assistant_turn_estimate(),
        0.0,
        "on-device runs cost nothing to gate"
    );
}

#[test]
fn a_cloud_run_estimates_a_nonzero_cost() {
    let (app, _dir) = assistant_app("budget-cloud"); // Claude backend
    assert!(
        app.assistant_turn_estimate() > 0.0,
        "a cloud turn has an estimable cost"
    );
}

#[test]
fn a_run_over_budget_asks_before_continuing() {
    let (mut app, _dir) = assistant_app("budget-over"); // Claude, api_key None
    app.ai.cost_threshold = 0.20;
    app.test_set_spent(0.50); // this run has already spent past the threshold
    app.test_spawn_turn(); // the next turn must ask first
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::System && e.text.contains("Reply 'y' to continue")),
        "a run over the threshold asks before spending more",
    );
}

#[test]
fn declining_the_budget_confirm_stops_the_run() {
    let (mut app, _dir) = assistant_app("budget-stop");
    app.ai.cost_threshold = 0.20;
    app.test_set_spent(0.50);
    app.test_spawn_turn(); // posts the budget confirm; pane keeps focus
    type_str(&mut app, "n"); // decline
    app.on_key(key(KeyCode::Enter));
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.text.contains("Stopped to stay within budget")),
        "declining the budget confirm stops the run cleanly",
    );
}

// --- Prose-boundary guardrail (#308) -------------------------------------

#[test]
fn the_allowlist_is_exactly_the_vetted_safe_set() {
    use crate::mode::Action;
    // An allowlist, not a denylist: a new action is excluded until it's explicitly
    // added here, so nothing — least of all a prose-writing action — joins by
    // accident. None of these writes or edits chapter prose.
    let mut got: Vec<&str> = Action::ALL
        .iter()
        .filter(|a| a.assistant_runnable())
        .map(|a| a.name())
        .collect();
    got.sort_unstable();
    let mut want = vec![
        "save",
        "export:markdown",
        "export:docx",
        "export:scriv",
        "export:vellum",
        "export:epub",
        "export:pdf",
        "export:pdf-print",
        "export:bible",
        "export:preflight",
        "export:kdp",
        "export:ingramspark",
        "report:progress",
        "report:analysis",
        "report:history",
        "guide",
        "dict:accept",
        "spell:next",
        "spell:prev",
        "view:typewriter",
        "view:focus",
        "view:paragraph-indent",
        "ai:backend",
        "ai:characters",
        "ai:continuity",
        "ai:setting",
        "ai:plot",
        "ai:scaffold",
        "ai:naming",
        "ai:style",
        "ai:voice",
        "ai:editorial",
        "ai:ask",
        "ai:brainstorm",
        "ai:draft-characters",
        "ai:draft-setting",
    ];
    want.sort_unstable();
    assert_eq!(
        got, want,
        "the assistant's allowlist is the vetted safe set"
    );
}

#[test]
fn an_unknown_prose_tool_is_blocked_in_code() {
    let (mut app, _dir) = assistant_app("assistant-guard-unknown");
    let before = app.buffer.text();
    // The model invents a tool to fulfil "rewrite this paragraph". No such action
    // exists, so it's blocked at the execution layer and nothing is written.
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "rewrite_paragraph")])));
    assert_eq!(
        app.buffer.text(),
        before,
        "a hallucinated prose tool edits nothing"
    );
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::Tool && e.text.contains("no action")),
        "the unknown tool is refused in code",
    );
}

#[test]
fn an_off_list_tool_is_blocked_in_code() {
    let (mut app, _dir) = assistant_app("assistant-guard-offlist");
    let before = app.buffer.text();
    // `annotate` is a real action but off the assistant's allowlist; even if the
    // model emits it, the execution layer refuses it regardless of the prompt.
    app.apply_assistant_turn(Ok(Turn::ToolCalls(vec![call("a", "annotate")])));
    assert_eq!(app.buffer.text(), before);
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.who == Speaker::Tool && e.text.contains("not a permitted")),
        "an off-allowlist action is blocked at the execution layer",
    );
}

#[test]
fn no_allowlisted_tool_edits_chapter_prose() {
    use crate::mode::Action;
    let (mut app, _dir) = assistant_app("assistant-guard-noedit");
    let before = app.buffer.text();
    // Run every synchronous allowlisted tool in one batch (the AI passes are async
    // and gated separately, and also never edit prose): none touches the chapter.
    let calls: Vec<ToolCall> = Action::ALL
        .iter()
        .filter(|a| a.assistant_runnable() && !matches!(a, Action::Ai(_)))
        .enumerate()
        .map(|(i, a)| call(&i.to_string(), a.name()))
        .collect();
    app.apply_assistant_turn(Ok(Turn::ToolCalls(calls)));
    assert_eq!(
        app.buffer.text(),
        before,
        "no assistant tool edits the chapter prose"
    );
}

#[test]
fn a_finished_pass_resumes_the_batch() {
    let (mut app, _dir) = assistant_app("assistant-pass-resume");
    // A pass is in flight, with a synchronous call queued behind it.
    app.test_install_pass(call("p", "ai:continuity"), vec![call("s", "view:focus")]);
    let before = app.focus_mode;
    app.resume_assistant_pass("Done — Continuity. The findings are in the editor.".into());
    assert_ne!(
        app.focus_mode, before,
        "the queued sync call ran after the pass resumed"
    );
    let c = conv(&app);
    assert!(
        c.entries()
            .iter()
            .any(|e| e.text.contains("ai:continuity") && e.text.contains("Done")),
        "the pass result is reported in the conversation",
    );
    assert!(
        c.entries().iter().any(|e| e.text.contains("view:focus")),
        "the rest of the batch continues",
    );
}

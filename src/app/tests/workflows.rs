//! Tests for the author-workflow runner and the run-workflow menu (#226, #227): a
//! workflow runs its steps in order through the registry, stops and reports on the
//! first unavailable step (earlier effects standing), refuses a cycle, the menu
//! lists the defined workflows, and async AI steps skip when AI is off, advance in
//! order, stop on error, cancel cleanly, and time out.

use super::*;
use crate::ai;
use crate::mode::{Action, AiAction};
use crate::workflow::Workflow;

fn wf(name: &str, steps: &[&str]) -> Workflow {
    Workflow { name: name.into(), steps: steps.iter().map(|s| (*s).to_string()).collect() }
}

fn ai_off(app: &mut App) {
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
}

#[test]
fn run_workflow_runs_each_step_in_order() {
    let (mut app, dir) = two_chapter_app("wf-run");
    app.workflows = vec![wf("finish", &["save", "report:progress"])];
    app.on_key(key(KeyCode::Char('X'))); // edit chapter one
    app.run_workflow(0);
    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(saved.starts_with('X'), "the save step ran: {saved:?}");
    assert!(app.overlay.is_some(), "the report step ran");
    assert!(
        app.message.as_deref().unwrap_or("").contains("Ran workflow 'finish'"),
        "final summary: {:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn workflow_stops_at_the_first_unavailable_step() {
    let (mut app, dir) = two_chapter_app("wf-stop");
    app.workflows = vec![wf("publish", &["save", "export:markdown"])];
    app.on_key(key(KeyCode::Char('X')));
    app.toggle_continuous(); // export is unavailable from the continuous view
    assert!(app.manuscript.is_some());
    app.run_workflow(0);
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("stopped at step 2"), "{msg}");
    assert!(msg.contains("export:markdown"), "names the failed step: {msg}");
    // The earlier save step's effect stands; the export never ran.
    assert!(!dir.join("export").join("book.md").exists(), "no export written");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_workflow_reports_a_cycle() {
    let (mut app, dir) = two_chapter_app("wf-cycle");
    app.workflows = vec![wf("loop", &["save", "loop"])];
    app.run_workflow(0);
    assert!(
        app.message.as_deref().unwrap_or("").contains("cycle"),
        "a self-referential workflow is refused: {:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_workflow_reports_an_unknown_step() {
    let (mut app, dir) = two_chapter_app("wf-unknown");
    app.workflows = vec![wf("oops", &["save", "frobnicate"])];
    app.run_workflow(0);
    assert!(
        app.message.as_deref().unwrap_or("").contains("frobnicate"),
        "an unknown step is reported, not dropped: {:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_menu_lists_the_defined_workflows() {
    let (mut app, dir) = two_chapter_app("wf-menu");
    app.workflows = vec![wf("a", &["save"]), wf("b", &["report:analysis"])];
    app.open_workflow_palette();
    match &app.mode {
        Mode::Palette(p) => {
            assert_eq!(p.title, "Run workflow");
            assert_eq!(p.items.len(), 2);
        }
        _ => panic!("expected the run-workflow menu"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_menu_annotates_an_invalid_workflow() {
    let (mut app, dir) = two_chapter_app("wf-menu-bad");
    app.workflows = vec![wf("bad", &["nope"])];
    app.open_workflow_palette();
    match &app.mode {
        Mode::Palette(p) => assert!(p.items[0].desc.contains('⚠'), "{}", p.items[0].desc),
        _ => panic!("expected the run-workflow menu"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_config_explains_how_to_add_a_workflow() {
    let (mut app, dir) = two_chapter_app("wf-empty");
    app.workflows = Vec::new();
    app.open_workflow_palette();
    assert!(!matches!(app.mode, Mode::Palette(_)), "no empty menu opens");
    assert!(
        app.message.as_deref().unwrap_or("").contains("No workflows"),
        "{:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_x_opens_the_workflow_menu() {
    let (mut app, dir) = two_chapter_app("wf-ctrl-x");
    app.workflows = vec![wf("a", &["save"])];
    app.on_key(ctrl('x'));
    assert!(matches!(app.mode, Mode::Palette(_)), "Ctrl-x opens the run-workflow menu");
    fs::remove_dir_all(&dir).ok();
}

// --- #227: async AI steps ---------------------------------------------------

#[test]
fn an_ai_step_is_skipped_when_ai_is_off() {
    let (mut app, dir) = two_chapter_app("wf-ai-off");
    ai_off(&mut app);
    app.workflows = vec![wf("deep", &["save", "ai:characters"])];
    app.on_key(key(KeyCode::Char('X')));
    app.run_workflow(0);
    // The save ran; the AI step was skipped, not failed — the run completed.
    assert!(fs::read_to_string(dir.join("one.md")).unwrap().starts_with('X'));
    assert!(app.workflow_run.is_none(), "the run finished");
    assert!(!app.ai_task.is_busy(), "no request was sent");
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("Ran workflow 'deep'") && msg.contains("1 skipped"), "{msg}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_async_step_completing_advances_to_the_next_step() {
    let (mut app, dir) = two_chapter_app("wf-advance");
    // Stand the run up at an AI step in flight, then deliver its result.
    app.begin_workflow_for_test("pass", vec![Action::Ai(AiAction::Continuity), Action::Save]);
    app.on_key(key(KeyCode::Char('Y'))); // edit so the trailing save has something to write
    app.advance_after_ai(None); // the AI step finished cleanly
    // It advanced past the AI step and ran the save, then finished.
    assert!(app.workflow_run.is_none(), "run finished after the trailing save");
    assert!(fs::read_to_string(dir.join("one.md")).unwrap().starts_with('Y'), "save step ran");
    assert!(app.message.as_deref().unwrap_or("").contains("Ran workflow 'pass'"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_async_step_error_stops_the_run() {
    let (mut app, dir) = two_chapter_app("wf-ai-err");
    app.begin_workflow_for_test("pass", vec![Action::Ai(AiAction::Continuity), Action::Save]);
    app.advance_after_ai(Some("network unreachable".into()));
    assert!(app.workflow_run.is_none(), "the run stopped");
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("stopped at step 1") && msg.contains("network unreachable"), "{msg}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_cancelled_run_stops_after_the_current_step() {
    let (mut app, dir) = two_chapter_app("wf-cancel");
    app.begin_workflow_for_test("pass", vec![Action::Ai(AiAction::Continuity), Action::Save]);
    app.cancel_workflow(); // requested while the AI step is in flight
    assert!(app.workflow_run.is_some(), "still running until the step finishes");
    app.advance_after_ai(None); // the in-flight step finishes
    assert!(app.workflow_run.is_none(), "stopped after the current step");
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("cancelled") && msg.contains("1 step ran"), "{msg}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn esc_cancels_a_running_workflow() {
    let (mut app, dir) = two_chapter_app("wf-esc");
    app.begin_workflow_for_test("pass", vec![Action::Ai(AiAction::Continuity)]);
    app.on_key(key(KeyCode::Esc));
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelling"),
        "Esc requests cancellation: {:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_declined_cost_confirm_cancels_the_run() {
    let (mut app, dir) = two_chapter_app("wf-decline");
    app.begin_workflow_for_test("pass", vec![Action::Ai(AiAction::Continuity)]);
    app.decline_workflow();
    assert!(app.workflow_run.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("cancelled"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_stuck_step_times_out() {
    let (mut app, dir) = two_chapter_app("wf-timeout");
    // A spawned-but-unpolled task keeps `ai_task` busy (simulating a request in
    // flight); the run is backdated past the per-step timeout.
    app.ai_task.spawn(|| ai::Outcome::Error(ai::AiError::Network("stuck".into())));
    app.begin_workflow_for_test("pass", vec![Action::Ai(AiAction::Continuity)]);
    app.expire_workflow_step();
    app.tick_workflow_timeout();
    assert!(app.workflow_run.is_none(), "the stuck run was stopped");
    assert!(!app.ai_task.is_busy(), "the stuck request was dropped");
    assert!(app.message.as_deref().unwrap_or("").contains("timed out"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_workflow_is_refused_while_one_is_already_running() {
    let (mut app, dir) = two_chapter_app("wf-reentrant");
    app.workflows = vec![wf("a", &["save"])];
    app.begin_workflow_for_test("busy", vec![Action::Ai(AiAction::Continuity)]);
    app.run_workflow(0);
    assert!(
        app.message.as_deref().unwrap_or("").contains("already running"),
        "{:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

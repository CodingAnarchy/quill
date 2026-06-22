//! Tests for the author-workflow runner and the run-workflow menu (#226): a
//! workflow runs its steps in order through the registry, stops and reports on
//! the first unavailable step (earlier effects standing), refuses a cycle, and
//! the menu lists the defined workflows.

use super::*;
use crate::workflow::Workflow;

fn wf(name: &str, steps: &[&str]) -> Workflow {
    Workflow { name: name.into(), steps: steps.iter().map(|s| (*s).to_string()).collect() }
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

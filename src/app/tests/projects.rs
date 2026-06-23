//! Project browser & in-place switching (#270): open the browser over recent
//! projects, switch to one (saving the current first), guard unsaved work, and
//! reject re-opening the project already open.

use super::*;
use std::path::Path;
use std::sync::Mutex;

/// These tests redirect the global config dir (where the recent-projects list
/// lives) to a temp dir via `XDG_CONFIG_HOME`. The env var is process-global, so
/// serialize them — and isolate each one's recents — under this lock.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Point `XDG_CONFIG_HOME` at a fresh temp dir so `recent::record`/`list` touch
/// scratch state, never the developer's real `~/.config/quill`.
fn isolate(tag: &str) {
    let dir = std::env::temp_dir().join(format!("quill-xdg-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &dir);
}

/// A one-chapter project on disk with the given title and body.
fn make_project(tag: &str, title: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-proj270-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        format!("title = \"{title}\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n"),
    )
    .unwrap();
    fs::write(dir.join("one.md"), body).unwrap();
    dir
}

fn open_app(dir: &Path) -> App {
    let project = Project::open(dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.vim = false;
    app.mode = Mode::Insert;
    app.workflows = Vec::new();
    app.triggers = Default::default();
    app
}

#[test]
fn ctrl_b_lists_recent_projects_newest_first() {
    let _g = ENV_LOCK.lock().unwrap();
    isolate("list");
    let a = make_project("list-a", "Alpha", "alpha\n");
    let b = make_project("list-b", "Beta", "beta\n");
    recent::record(&a, "Alpha");
    recent::record(&b, "Beta");

    let mut app = open_app(&b);
    app.on_key(ctrl('b'));

    match &app.mode {
        Mode::ProjectBrowser(s) => {
            let titles: Vec<_> = s.projects.iter().map(|p| p.title.as_str()).collect();
            assert!(titles.contains(&"Alpha") && titles.contains(&"Beta"), "both listed: {titles:?}");
            assert_eq!(s.projects[0].title, "Beta", "the most recently recorded is first");
        }
        _ => panic!("Ctrl-b should open the project browser"),
    }
}

#[test]
fn switching_swaps_the_active_project_in_place() {
    let _g = ENV_LOCK.lock().unwrap();
    isolate("swap");
    let a = make_project("swap-a", "Alpha", "alpha prose\n");
    let b = make_project("swap-b", "Beta", "beta prose\n");

    let mut app = open_app(&a);
    assert_eq!(app.project.manifest.title, "Alpha");
    app.switch_to(b.clone());

    assert_eq!(app.project.manifest.title, "Beta", "the active project is now Beta");
    assert!(app.edit_buffer().text().contains("beta prose"), "the buffer shows Beta's chapter");
    assert!(app.manuscript.is_none() && !app.edit_dirty(), "fresh per-project state");
}

#[test]
fn switching_with_unsaved_work_prompts_then_saves_and_switches() {
    let _g = ENV_LOCK.lock().unwrap();
    isolate("dirty");
    let a = make_project("dirty-a", "Alpha", "alpha\n");
    let b = make_project("dirty-b", "Beta", "beta\n");

    let mut app = open_app(&a);
    type_str(&mut app, "EDIT ");
    assert!(app.edit_dirty());

    app.request_switch(b.clone());
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ConfirmSwitch)),
        "an unsaved switch confirms first"
    );

    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(app.project.manifest.title, "Beta", "y saves and switches");
    let a_body = fs::read_to_string(a.join("one.md")).unwrap();
    assert!(a_body.contains("EDIT"), "the current project was saved before switching");
}

#[test]
fn cancelling_the_switch_keeps_the_current_project_and_its_unsaved_work() {
    let _g = ENV_LOCK.lock().unwrap();
    isolate("cancel");
    let a = make_project("cancel-a", "Alpha", "alpha\n");
    let b = make_project("cancel-b", "Beta", "beta\n");

    let mut app = open_app(&a);
    type_str(&mut app, "EDIT ");
    app.request_switch(b.clone());
    app.on_key(key(KeyCode::Esc));

    assert_eq!(app.project.manifest.title, "Alpha", "cancel stays on the current project");
    assert!(app.edit_dirty(), "the unsaved work is still there");
    assert!(app.pending_switch.is_none(), "the pending target is cleared");
}

#[test]
fn switching_to_the_already_open_project_is_a_no_op_with_a_message() {
    let _g = ENV_LOCK.lock().unwrap();
    isolate("same");
    let a = make_project("same-a", "Alpha", "alpha\n");

    let mut app = open_app(&a);
    app.request_switch(a.clone());

    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm for the open project");
    assert!(app.message.as_deref().unwrap_or("").contains("already open"));
}

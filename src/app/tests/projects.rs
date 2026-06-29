//! Project browser & in-place switching (#270): open the browser over recent
//! projects, switch to one (saving the current first), guard unsaved work, and
//! reject re-opening the project already open.

use super::*;
use std::path::Path;

use crate::recent;

#[test]
fn ctrl_b_lists_recent_projects_newest_first() {
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
            assert!(
                titles.contains(&"Alpha") && titles.contains(&"Beta"),
                "both listed: {titles:?}"
            );
            assert_eq!(
                s.projects[0].title, "Beta",
                "the most recently recorded is first"
            );
        }
        _ => panic!("Ctrl-b should open the project browser"),
    }
}

#[test]
fn switching_swaps_the_active_project_in_place() {
    isolate("swap");
    let a = make_project("swap-a", "Alpha", "alpha prose\n");
    let b = make_project("swap-b", "Beta", "beta prose\n");

    let mut app = open_app(&a);
    assert_eq!(app.project.manifest.title, "Alpha");
    app.switch_to(b.clone());

    assert_eq!(
        app.project.manifest.title, "Beta",
        "the active project is now Beta"
    );
    assert!(
        app.edit_buffer().text().contains("beta prose"),
        "the buffer shows Beta's chapter"
    );
    assert!(
        app.manuscript.is_none() && !app.edit_dirty(),
        "fresh per-project state"
    );
}

#[test]
fn switching_with_unsaved_work_prompts_then_saves_and_switches() {
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
    assert!(
        a_body.contains("EDIT"),
        "the current project was saved before switching"
    );
}

#[test]
fn cancelling_the_switch_keeps_the_current_project_and_its_unsaved_work() {
    isolate("cancel");
    let a = make_project("cancel-a", "Alpha", "alpha\n");
    let b = make_project("cancel-b", "Beta", "beta\n");

    let mut app = open_app(&a);
    type_str(&mut app, "EDIT ");
    app.request_switch(b.clone());
    app.on_key(key(KeyCode::Esc));

    assert_eq!(
        app.project.manifest.title, "Alpha",
        "cancel stays on the current project"
    );
    assert!(app.edit_dirty(), "the unsaved work is still there");
    assert!(
        app.pending_switch.is_none(),
        "the pending target is cleared"
    );
}

#[test]
fn switching_to_the_already_open_project_is_a_no_op_with_a_message() {
    isolate("same");
    let a = make_project("same-a", "Alpha", "alpha\n");

    let mut app = open_app(&a);
    app.request_switch(a.clone());

    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no confirm for the open project"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("already open"));
}

// --- Filesystem navigator (#271) ------------------------------------------

fn project_in(parent: &Path, name: &str, title: &str, body: &str) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        format!("title = \"{title}\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n"),
    )
    .unwrap();
    fs::write(dir.join("one.md"), body).unwrap();
    dir
}

fn dir_entry_at<'a>(app: &'a App, label: &str) -> &'a crate::mode::DirEntry {
    match &app.mode {
        Mode::DirBrowser(s) => s
            .entries
            .iter()
            .find(|e| e.label == label)
            .expect("entry present"),
        _ => panic!("not in the directory browser"),
    }
}

fn select_entry(app: &mut App, label: &str) {
    if let Mode::DirBrowser(s) = &mut app.mode {
        s.selected = s
            .entries
            .iter()
            .position(|e| e.label == label)
            .expect("entry present");
    }
}

#[test]
fn browsing_files_is_reachable_even_with_no_recent_projects() {
    isolate("no-recents");
    let parent = books_dir("nr");
    let a = project_in(&parent, "alpha", "Alpha", "alpha\n");

    // open_app uses App::new directly, so it records nothing — recents are empty.
    let mut app = open_app(&a);
    app.on_key(ctrl('b'));
    assert!(
        matches!(app.mode, Mode::ProjectBrowser(_)),
        "the browser still opens with no recents"
    );
    app.on_key(key(KeyCode::Char('b')));
    assert!(
        matches!(app.mode, Mode::DirBrowser(_)),
        "b reaches the filesystem navigator"
    );
}

#[test]
fn navigator_lists_subfolders_and_marks_projects() {
    isolate("list-dirs");
    let parent = books_dir("ld");
    let a = project_in(&parent, "alpha", "Alpha", "alpha\n");
    project_in(&parent, "beta", "Beta", "beta\n");
    fs::create_dir_all(parent.join("plain")).unwrap();

    let mut app = open_app(&a);
    app.browse_into(parent.clone());

    let labels: Vec<String> = match &app.mode {
        Mode::DirBrowser(s) => s.entries.iter().map(|e| e.label.clone()).collect(),
        _ => panic!("browse_into should open the directory browser"),
    };
    assert!(
        labels.contains(&"..".to_string()),
        "a parent entry to go up"
    );
    assert!(
        ["alpha", "beta", "plain"]
            .iter()
            .all(|n| labels.contains(&n.to_string())),
        "{labels:?}"
    );
    assert!(
        dir_entry_at(&app, "beta").is_project,
        "a project subdir is marked openable"
    );
    assert!(
        !dir_entry_at(&app, "plain").is_project,
        "a plain subdir is not a project"
    );
}

#[test]
fn entering_a_project_in_the_navigator_switches_to_it() {
    isolate("enter-proj");
    let parent = books_dir("ep");
    let a = project_in(&parent, "alpha", "Alpha", "alpha\n");
    project_in(&parent, "beta", "Beta", "beta prose\n");

    let mut app = open_app(&a);
    app.browse_into(parent.clone());
    select_entry(&mut app, "beta");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(
        app.project.manifest.title, "Beta",
        "Enter on a project opens it"
    );
    assert!(app.edit_buffer().text().contains("beta prose"));
}

#[test]
fn entering_a_plain_folder_descends_into_it() {
    isolate("descend");
    let parent = books_dir("de");
    let a = project_in(&parent, "alpha", "Alpha", "alpha\n");
    fs::create_dir_all(parent.join("plain").join("inner")).unwrap();

    let mut app = open_app(&a);
    app.browse_into(parent.clone());
    select_entry(&mut app, "plain");
    app.on_key(key(KeyCode::Enter));

    match &app.mode {
        Mode::DirBrowser(s) => assert_eq!(
            s.cwd,
            parent.join("plain"),
            "descended into the plain folder"
        ),
        _ => panic!("descending stays in the directory browser"),
    }
}

#[test]
fn the_parent_entry_navigates_up() {
    isolate("up");
    let parent = books_dir("up");
    let a = project_in(&parent, "alpha", "Alpha", "alpha\n");

    let mut app = open_app(&a);
    app.browse_into(parent.clone());
    select_entry(&mut app, "..");
    app.on_key(key(KeyCode::Enter));

    match &app.mode {
        Mode::DirBrowser(s) => assert_eq!(s.cwd, parent.parent().unwrap(), ".. goes up a level"),
        _ => panic!("still browsing"),
    }
}

#[test]
fn an_unreadable_directory_shows_a_message_without_crashing() {
    isolate("unreadable");
    let a = make_project("unreadable-a", "Alpha", "alpha\n");
    let mut app = open_app(&a);

    let missing = std::env::temp_dir().join(format!("quill-nope-{}", std::process::id()));
    let _ = fs::remove_dir_all(&missing);
    app.browse_into(missing);

    assert!(
        !matches!(app.mode, Mode::DirBrowser(_)),
        "an unreadable dir doesn't enter the browser"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Can't open"),
        "a clear message: {:?}",
        app.message
    );
}

// --- Projects-home auto-list (#272) ---------------------------------------

/// Point the `[projects] home` config at `home` (call after `isolate`, which set
/// the config dir to a temp location).
fn set_projects_home(home: &Path) {
    let cfg_dir = crate::config::config_dir().unwrap();
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.toml"),
        format!("[projects]\nhome = \"{}\"\n", home.display()),
    )
    .unwrap();
}

fn browser_titles(app: &App) -> Vec<String> {
    match &app.mode {
        Mode::ProjectBrowser(s) => s.projects.iter().map(|p| p.title.clone()).collect(),
        _ => panic!("not in the project browser"),
    }
}

#[test]
fn projects_under_the_configured_home_appear_in_the_browser() {
    isolate("home");
    let lib = books_dir("home-lib");
    project_in(&lib, "alpha", "Alpha", "a\n");
    project_in(&lib, "beta", "Beta", "b\n");
    set_projects_home(&lib);

    // An app on an unrelated project, with no recents.
    let other = make_project("home-other", "Other", "o\n");
    let mut app = open_app(&other);
    app.on_key(ctrl('b'));

    let titles = browser_titles(&app);
    assert!(
        titles.contains(&"Alpha".to_string()) && titles.contains(&"Beta".to_string()),
        "home books listed: {titles:?}"
    );
}

#[test]
fn a_home_project_already_in_recents_is_not_duplicated() {
    isolate("home-dedup");
    let lib = books_dir("dedup-lib");
    let a = project_in(&lib, "alpha", "Alpha", "a\n");
    set_projects_home(&lib);
    recent::record(&a, "Alpha"); // now both a recent and under the home

    let other = make_project("dedup-other", "Other", "o\n");
    let mut app = open_app(&other);
    app.on_key(ctrl('b'));

    let count = browser_titles(&app)
        .iter()
        .filter(|t| *t == "Alpha")
        .count();
    assert_eq!(
        count, 1,
        "a project that's both recent and under home shows once"
    );
}

#[test]
fn with_no_projects_home_only_recents_are_listed() {
    isolate("no-home");
    let a = make_project("nohome-a", "Alpha", "a\n");
    recent::record(&a, "Alpha");

    let mut app = open_app(&a);
    app.on_key(ctrl('b'));

    assert_eq!(
        browser_titles(&app),
        vec!["Alpha".to_string()],
        "no home configured — just recents"
    );
}

// --- New project (#273) ----------------------------------------------------

#[test]
fn creating_a_new_project_initializes_and_opens_it() {
    isolate("create");
    let parent = books_dir("create-parent");
    let cur = make_project("create-cur", "Current", "c\n");

    let mut app = open_app(&cur);
    let target = parent.join("Fresh Book");
    app.create_project_at(target.clone());

    assert!(
        Project::manifest_path(&target).exists(),
        "the project is initialized on disk"
    );
    assert_eq!(
        app.project.manifest.title, "Fresh Book",
        "it becomes the active project"
    );
    let titles: Vec<String> = recent::list().into_iter().map(|r| r.title).collect();
    assert!(
        titles.contains(&"Fresh Book".to_string()),
        "it's added to recents: {titles:?}"
    );
}

#[test]
fn creating_refuses_to_clobber_an_existing_project() {
    isolate("clobber");
    let existing = make_project("clobber-existing", "Existing", "important prose\n");
    let cur = make_project("clobber-cur", "Current", "c\n");

    let mut app = open_app(&cur);
    app.create_project_at(existing.clone());

    assert_eq!(
        app.project.manifest.title, "Current",
        "the current project is untouched"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("already exists"));
    assert!(
        fs::read_to_string(existing.join("one.md"))
            .unwrap()
            .contains("important prose"),
        "the existing project's files are not clobbered"
    );
}

#[test]
fn n_in_the_browser_prompts_for_a_new_project_path() {
    isolate("prompt");
    let cur = make_project("prompt-cur", "Current", "c\n");

    let mut app = open_app(&cur);
    app.on_key(ctrl('b'));
    app.on_key(key(KeyCode::Char('n')));

    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::NewProject));
            assert!(
                s.input.ends_with('/'),
                "prefilled with a base location: {:?}",
                s.input
            );
        }
        _ => panic!("n should open the new-project prompt"),
    }
}

#[test]
fn the_new_project_prompt_creates_and_opens_on_enter() {
    isolate("prompt-create");
    let parent = books_dir("pc-parent");
    let cur = make_project("pc-cur", "Current", "c\n");

    let mut app = open_app(&cur);
    app.prompt_new_project(parent.clone());
    type_str(&mut app, "Brand New");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.manifest.title, "Brand New");
    assert!(Project::manifest_path(&parent.join("Brand New")).exists());
}

#[test]
fn a_new_project_path_with_no_name_is_rejected() {
    isolate("empty-name");
    let parent = books_dir("en-parent");
    let cur = make_project("en-cur", "Current", "c\n");

    let mut app = open_app(&cur);
    app.prompt_new_project(parent.clone()); // input is "<parent>/", no name appended
    app.on_key(key(KeyCode::Enter));

    assert_eq!(
        app.project.manifest.title, "Current",
        "nothing is created without a name"
    );
    assert!(app.message.as_deref().unwrap_or("").contains("folder name"));
}

// --- Rename the active project (#424) -------------------------------------

#[test]
fn renaming_moves_the_folder_and_keeps_unsaved_work() {
    isolate("rename");
    let dir = make_project("rename-src", "Working Title", "the opening line\n");
    let mut app = open_app(&dir);

    // An unsaved edit, to prove nothing is lost across the move.
    app.edit_buffer_mut().insert_char('Z');
    assert!(app.edit_dirty());

    let new_root = dir.parent().unwrap().join("Final Title");
    let _ = fs::remove_dir_all(&new_root); // a stale run mustn't trip the collision guard
    app.rename_project("Final Title");

    assert_eq!(
        app.project.root.canonicalize().unwrap(),
        new_root.canonicalize().unwrap()
    );
    assert!(!dir.exists(), "the old folder was moved away");
    // The unsaved edit was flushed before the move and lives at the new path.
    assert!(fs::read_to_string(new_root.join("one.md"))
        .unwrap()
        .starts_with('Z'));

    // Saving from here lands in the new location.
    app.edit_buffer_mut().insert_char('Q');
    app.save_edit();
    assert!(fs::read_to_string(new_root.join("one.md"))
        .unwrap()
        .starts_with("QZ"));

    fs::remove_dir_all(&new_root).ok();
}

#[test]
fn renaming_refuses_to_clobber_an_existing_folder() {
    isolate("rename-collide");
    let dir = make_project("rename-collide", "Book", "body\n");
    let mut app = open_app(&dir);
    let taken = dir.parent().unwrap().join("Taken Name");
    fs::create_dir_all(&taken).unwrap();

    let before = app.project.root.clone();
    app.rename_project("Taken Name");

    assert_eq!(
        app.project.root, before,
        "the project is untouched on a collision"
    );
    assert!(dir.exists(), "the original folder is intact");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("already exists"),
        "{:?}",
        app.message
    );

    fs::remove_dir_all(&taken).ok();
}

#[test]
fn renaming_rejects_an_empty_name() {
    isolate("rename-empty");
    let dir = make_project("rename-empty", "Book", "body\n");
    let mut app = open_app(&dir);
    let before = app.project.root.clone();

    app.rename_project("   ");

    assert_eq!(app.project.root, before, "no move on an empty name");
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Enter a folder name"));
}

#[test]
fn renaming_updates_recents_to_the_new_path() {
    isolate("rename-recent");
    let dir = make_project("rename-recent", "Book", "body\n");
    recent::record(&dir, "Book"); // the pre-rename entry
    let mut app = open_app(&dir);

    let new_root = dir.parent().unwrap().join("Renamed Book");
    let _ = fs::remove_dir_all(&new_root);
    app.rename_project("Renamed Book");

    let paths: Vec<PathBuf> = recent::list().into_iter().map(|r| r.path).collect();
    let new_canon = new_root.canonicalize().unwrap();
    assert!(
        paths.contains(&new_canon),
        "the new path is recorded: {paths:?}"
    );
    assert!(
        !paths.contains(&dir),
        "the old path self-prunes (its folder is gone): {paths:?}"
    );

    fs::remove_dir_all(&new_root).ok();
}

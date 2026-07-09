//! Tests for the branch-as-draft flow (#965): the drafts overlay reached from the
//! Git palette — create a draft (it exists and becomes current), switch drafts (the
//! working copy and active chapter follow), list drafts, merge a clean draft, and a
//! conflicting draft merge routing through the conflict-resolution UI (#963). All
//! hermetic (#531) over a scratch repo — no network.

use super::*;

/// A one-chapter project already under version control with a first commit, so the
/// drafts overlay (which branches off the current commit) has something to work
/// with. Returns the built app and the temp project guard.
fn committed_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "mainline chapter\n").unwrap();
    crate::vcs::init(&dir).unwrap();
    crate::vcs::commit(&dir, "first").unwrap();

    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// Open the Git palette and run the item labelled `label` — index-independent, so a
/// new palette entry never renumbers these tests.
fn git_palette_run(app: &mut App, label: &str) {
    app.on_key(ctrl('v'));
    let index = match &app.mode {
        Mode::Palette(p) => p
            .items
            .iter()
            .position(|it| it.label == label)
            .unwrap_or_else(|| panic!("no Git-palette item labelled {label:?}")),
        _ => panic!("expected the Git palette to open"),
    };
    for _ in 0..index {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
}

/// The draft names shown in the open drafts overlay.
fn draft_names(app: &App) -> Vec<String> {
    match &app.mode {
        Mode::Drafts(menu) => menu.drafts.clone(),
        _ => panic!("expected the drafts overlay to be open"),
    }
}

#[test]
fn creating_a_draft_makes_it_exist_and_current() {
    let (mut app, dir) = committed_app("drafts-create");
    let main = crate::vcs::current_branch(&dir).unwrap();

    git_palette_run(&mut app, "Drafts");
    assert!(matches!(app.mode, Mode::Drafts(_)), "the overlay opened");

    // `n` → the name prompt; type a name and submit.
    app.on_key(key(KeyCode::Char('n')));
    type_str(&mut app, "alt-ending");
    app.on_key(key(KeyCode::Enter));

    // The draft was created and switched to — it's now the current branch.
    assert_eq!(crate::vcs::current_branch(&dir).unwrap(), "alt-ending");
    let drafts = crate::vcs::list_branches(&dir).unwrap();
    assert!(drafts.contains(&"alt-ending".to_string()), "{drafts:?}");
    assert!(
        drafts.contains(&main),
        "the mainline still exists: {drafts:?}"
    );
    // The overlay reopened, marking the new draft current.
    assert!(matches!(app.mode, Mode::Drafts(_)));
    assert!(draft_names(&app).contains(&"alt-ending".to_string()));
}

#[test]
fn the_overlay_lists_every_draft_with_the_current_one_marked() {
    let (mut app, dir) = committed_app("drafts-list");
    let main = crate::vcs::current_branch(&dir).unwrap();
    crate::vcs::create_branch(&dir, "pov-b").unwrap();
    crate::vcs::create_branch(&dir, "what-if").unwrap();

    git_palette_run(&mut app, "Drafts");
    let names = draft_names(&app);
    assert!(names.contains(&main), "{names:?}");
    assert!(names.contains(&"pov-b".to_string()), "{names:?}");
    assert!(names.contains(&"what-if".to_string()), "{names:?}");
    if let Mode::Drafts(menu) = &app.mode {
        assert_eq!(menu.current, main, "the current draft is the mainline");
    }
}

#[test]
fn switching_drafts_updates_the_working_copy_and_active_chapter() {
    let (_app, dir) = committed_app("drafts-switch");
    // A side draft with its own version of the chapter.
    crate::vcs::create_branch(&dir, "alt").unwrap();
    crate::vcs::checkout_branch(&dir, "alt").unwrap();
    fs::write(dir.join("one.md"), "alt-draft chapter\n").unwrap();
    crate::vcs::commit(&dir, "alt-edit").unwrap();
    crate::vcs::checkout_branch(
        &dir,
        &crate::vcs::list_branches(&dir)
            .unwrap()
            .into_iter()
            .find(|b| b != "alt")
            .unwrap(),
    )
    .unwrap();
    // Reopen the app now that we're back on the mainline, so its buffer is the
    // mainline chapter.
    let mut app = open_test_app(&dir);
    assert!(app.buffer.text().contains("mainline chapter"));

    // Open the overlay, select the alt draft, and switch to it.
    git_palette_run(&mut app, "Drafts");
    if let Mode::Drafts(menu) = &mut app.mode {
        menu.selected = menu.drafts.iter().position(|d| d == "alt").unwrap();
    }
    app.on_key(key(KeyCode::Enter));

    // The working copy on disk and the editor buffer both follow the alt draft.
    assert_eq!(crate::vcs::current_branch(&dir).unwrap(), "alt");
    assert!(
        fs::read_to_string(dir.join("one.md"))
            .unwrap()
            .contains("alt-draft chapter"),
        "the working copy switched"
    );
    assert!(
        app.buffer.text().contains("alt-draft chapter"),
        "the active chapter reloaded: {:?}",
        app.buffer.text()
    );
    assert!(matches!(app.mode, Mode::Insert), "back in the editor");
}

#[test]
fn switching_drafts_guards_unsaved_work() {
    let (mut app, dir) = committed_app("drafts-switch-dirty");
    crate::vcs::create_branch(&dir, "alt").unwrap();

    // Type an unsaved edit into the buffer, then try to switch.
    type_str(&mut app, "UNSAVED");
    git_palette_run(&mut app, "Drafts");
    if let Mode::Drafts(menu) = &mut app.mode {
        menu.selected = menu.drafts.iter().position(|d| d == "alt").unwrap();
    }
    app.on_key(key(KeyCode::Enter));

    // The switch is refused — still on the mainline — and the edit survives on disk.
    assert_ne!(crate::vcs::current_branch(&dir).unwrap(), "alt");
    assert!(
        fs::read_to_string(dir.join("one.md"))
            .unwrap()
            .contains("UNSAVED"),
        "the unsaved edit was preserved"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains("commit"),
        "the guard tells the author to commit: {:?}",
        app.message
    );
}

#[test]
fn merging_a_clean_draft_folds_it_back_in() {
    let (_app, dir) = committed_app("drafts-merge-clean");
    let main = crate::vcs::current_branch(&dir).unwrap();
    // A side draft that adds a new chapter file (disjoint from the mainline).
    crate::vcs::create_branch(&dir, "side").unwrap();
    crate::vcs::checkout_branch(&dir, "side").unwrap();
    fs::write(dir.join("two.md"), "side-added chapter\n").unwrap();
    crate::vcs::commit(&dir, "side-adds-two").unwrap();
    crate::vcs::checkout_branch(&dir, &main).unwrap();
    let mut app = open_test_app(&dir);

    git_palette_run(&mut app, "Drafts");
    if let Mode::Drafts(menu) = &mut app.mode {
        menu.selected = menu.drafts.iter().position(|d| d == "side").unwrap();
    }
    app.on_key(key(KeyCode::Char('m'))); // merge the side draft into the mainline

    // A clean merge lands: the side draft's file is now on the mainline, and the
    // flow returned to the editor with a merge commit recorded.
    assert!(matches!(app.mode, Mode::Insert), "back in the editor");
    assert!(dir.join("two.md").exists(), "the side chapter folded in");
    let summaries: Vec<String> = crate::vcs::log(&dir, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(
        summaries.iter().any(|s| s.contains("Merge draft")),
        "a merge commit landed: {summaries:?}"
    );
}

#[test]
fn merging_a_conflicting_draft_opens_the_resolution_ui() {
    let (_app, dir) = committed_app("drafts-merge-conflict");
    let main = crate::vcs::current_branch(&dir).unwrap();
    // Both drafts edit the same chapter differently.
    crate::vcs::create_branch(&dir, "alt").unwrap();
    crate::vcs::checkout_branch(&dir, "alt").unwrap();
    fs::write(dir.join("one.md"), "alt rewrite\n").unwrap();
    crate::vcs::commit(&dir, "alt-edit").unwrap();
    crate::vcs::checkout_branch(&dir, &main).unwrap();
    fs::write(dir.join("one.md"), "mainline rewrite\n").unwrap();
    crate::vcs::commit(&dir, "main-edit").unwrap();
    let mut app = open_test_app(&dir);

    git_palette_run(&mut app, "Drafts");
    if let Mode::Drafts(menu) = &mut app.mode {
        menu.selected = menu.drafts.iter().position(|d| d == "alt").unwrap();
    }
    app.on_key(key(KeyCode::Char('m'))); // merge → conflict

    // The same conflict-resolution UI a sync uses (#963) opened, nothing committed.
    assert!(
        matches!(app.mode, Mode::ConflictReview(_)),
        "the resolution overlay opened"
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "mainline rewrite\n",
        "the target draft is untouched until the author resolves"
    );

    // Resolve: keep the mainline (target) version — the merge lands as a commit.
    app.on_key(key(KeyCode::Char('l')));
    assert!(matches!(app.mode, Mode::Insert), "the flow completed");
    let summaries: Vec<String> = crate::vcs::log(&dir, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(
        summaries.iter().any(|s| s.contains("Merge draft")),
        "the resolved merge committed: {summaries:?}"
    );
}

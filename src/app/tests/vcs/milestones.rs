//! Milestones and whole-project restore/diff (#833/#966): naming a milestone (with
//! an optional note), listing it in history, editing the note in place, and the
//! project-wide restore/diff against a commit or milestone. Pure move from `vcs.rs`.
use super::*;

// --- Milestones + whole-project restore/diff (#833) -------------------------

/// Create a named milestone through the Git palette — the "Milestone" item prompts
/// for a name, then for an optional note (#966); both are typed and submitted. The
/// note is left blank here; [`make_milestone_with_note`] exercises the note path.
fn make_milestone(app: &mut App, name: &str) {
    make_milestone_with_note(app, name, "");
}

/// Create a named milestone with a free-text note (#966): name → Enter → note →
/// Enter, driving the two-step prompt the palette opens.
fn make_milestone_with_note(app: &mut App, name: &str, note: &str) {
    git_menu_label(app, "Milestone");
    assert!(matches!(app.mode, Mode::Prompt(_)), "milestone prompts");
    type_str(app, name);
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the note prompt follows the name"
    );
    type_str(app, note);
    app.on_key(key(KeyCode::Enter));
}

#[test]
fn milestone_is_created_and_listed_in_history() {
    let (mut app, _dir) = two_chapter_app("vc-milestone");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "first draft");

    make_milestone(&mut app, "beta-readers");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("beta-readers"),
        "the milestone is confirmed: {:?}",
        app.message
    );

    // The history view lists the milestone alongside the commit.
    git_menu_label(&mut app, "History");
    let names: Vec<String> = match &app.mode {
        Mode::History(h) => h.milestones.iter().map(|t| t.name.clone()).collect(),
        _ => panic!("history should be open"),
    };
    assert!(
        names.contains(&"beta-readers".to_string()),
        "the milestone lists in history: {names:?}"
    );
}

#[test]
fn milestone_needs_a_nonempty_name() {
    let (mut app, _dir) = two_chapter_app("vc-milestone-empty");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "first");
    git_menu_label(&mut app, "Milestone");
    app.on_key(key(KeyCode::Enter)); // blank → cancelled
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelled"),
        "a blank name cancels: {:?}",
        app.message
    );
    assert!(
        crate::vcs::list_tags(&app.project.root).unwrap().is_empty(),
        "no milestone was created"
    );
}

#[test]
fn restore_project_from_the_palette_rewinds_the_whole_tree() {
    let (mut app, dir) = two_chapter_app("vc-restore-project");
    git_menu(&mut app, 0); // Initialize

    // Commit a baseline for both chapters, then edit and save the active one.
    commit_via_menu(&mut app, "baseline");
    type_str(&mut app, "an uncommitted edit");
    app.on_key(ctrl('s'));
    assert!(
        fs::read_to_string(dir.join("one.md"))
            .unwrap()
            .contains("uncommitted edit"),
        "the edit is on disk before the restore"
    );

    // Restore project → confirm; the working tree returns to the last commit and
    // the active chapter reloads (no uncommitted edit).
    git_menu_label(&mut app, "Restore project");
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "restore confirms first"
    );
    app.on_key(key(KeyCode::Char('y')));
    assert!(
        !app.buffer.text().contains("uncommitted edit"),
        "the buffer reloaded the restored version: {:?}",
        app.buffer.text()
    );
    assert!(
        !fs::read_to_string(dir.join("one.md"))
            .unwrap()
            .contains("uncommitted edit"),
        "the file on disk is rewound"
    );
    assert_eq!(app.vc_changes, 0, "the tree is clean after the restore");
}

#[test]
fn project_changes_diffs_the_whole_tree() {
    let (mut app, _dir) = two_chapter_app("vc-project-diff");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "baseline");

    // No changes yet → the project diff reports a clean tree.
    git_menu_label(&mut app, "Project changes");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No uncommitted changes"),
        "clean tree: {:?}",
        app.message
    );

    // Edit and save, then the project diff shows the change.
    type_str(&mut app, "a manuscript-wide edit");
    app.on_key(ctrl('s'));
    git_menu_label(&mut app, "Project changes");
    assert!(
        overlay_text(&app).contains("manuscript-wide edit"),
        "the project diff shows the edit"
    );
}

#[test]
fn history_restores_the_whole_project_to_a_milestone() {
    let (mut app, dir) = two_chapter_app("vc-hist-project-restore");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "draft done");
    make_milestone(&mut app, "first-draft");

    // Move the manuscript on past the milestone.
    type_str(&mut app, "post-milestone words");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "later work");
    assert!(app.buffer.text().contains("post-milestone words"));

    // Open history; the milestone leads the list (row 0). Restore the whole
    // project to it with `R`, confirm.
    git_menu_label(&mut app, "History");
    assert!(
        matches!(&app.mode, Mode::History(h) if !h.milestones.is_empty()),
        "the milestone is in the list"
    );
    app.on_key(key(KeyCode::Char('R'))); // whole-project restore from row 0
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "restore confirms first"
    );
    app.on_key(key(KeyCode::Char('y')));

    assert!(
        !app.buffer.text().contains("post-milestone words"),
        "the buffer reloaded the milestone version: {:?}",
        app.buffer.text()
    );
    assert!(
        !fs::read_to_string(dir.join("one.md"))
            .unwrap()
            .contains("post-milestone words"),
        "the file on disk is rewound to the milestone"
    );
}

#[test]
fn history_project_diff_spans_the_whole_manuscript() {
    let (mut app, _dir) = two_chapter_app("vc-hist-project-diff");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "baseline");
    make_milestone(&mut app, "mark");

    // Edit and save past the milestone, then diff the whole project against it.
    type_str(&mut app, "changed since the milestone");
    app.on_key(ctrl('s'));
    git_menu_label(&mut app, "History");
    app.on_key(key(KeyCode::Char('D'))); // whole-project diff against row 0 (the milestone)
    assert!(
        overlay_text(&app).contains("changed since the milestone"),
        "the project diff against the milestone shows the edit"
    );
}

/// The note a milestone carries (#966), or `None` when it isn't in the open
/// history view. Reads the stored annotation the history view lists.
fn milestone_note(app: &App, name: &str) -> Option<String> {
    match &app.mode {
        Mode::History(h) => h
            .milestones
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.message.clone()),
        _ => None,
    }
}

#[test]
fn milestone_note_persists_shows_and_can_be_edited() {
    // #966: a milestone can carry a free-text note set at creation, the history
    // view lists it, it survives reopening, and it can be edited in place with `n`
    // without disturbing the revision the milestone marks.
    let (mut app, dir) = two_chapter_app("vc-milestone-note");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "first draft");

    make_milestone_with_note(&mut app, "beta", "sent to beta readers");

    // The note persists on disk and lists back in the history view.
    git_menu_label(&mut app, "History");
    assert_eq!(
        milestone_note(&app, "beta").as_deref(),
        Some("sent to beta readers"),
        "the note is stored on the milestone"
    );

    // Edit the note in place with `n`: the milestone leads the list (row 0), so it
    // is the selection. Enter opens the note prompt prefilled; retype and submit.
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "n opens the note editor"
    );
    // Clear the prefilled note, then type the new one.
    for _ in 0..40 {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut app, "submission draft");
    app.on_key(key(KeyCode::Enter));

    // The edit reopens history; the new note is listed and the rev is unchanged.
    assert!(matches!(app.mode, Mode::History(_)), "history reopens");
    assert_eq!(
        milestone_note(&app, "beta").as_deref(),
        Some("submission draft"),
        "the note was edited in place: {:?}",
        app.message
    );

    // Restore-from-milestone still works after the note edit. Move past it, then
    // restore the whole project to the milestone with `R`.
    app.on_key(key(KeyCode::Esc)); // close history to edit the manuscript
    type_str(&mut app, "words after the milestone");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "more work");

    git_menu_label(&mut app, "History");
    app.on_key(key(KeyCode::Char('D'))); // whole-project diff against the milestone
    assert!(
        overlay_text(&app).contains("words after the milestone"),
        "diff-from-milestone still spans the manuscript after the note edit"
    );
    app.on_key(key(KeyCode::Esc)); // close the diff overlay, back to history

    app.on_key(key(KeyCode::Char('R'))); // restore the whole project to the milestone
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "restore confirms first"
    );
    app.on_key(key(KeyCode::Char('y')));
    assert!(
        !fs::read_to_string(dir.join("one.md"))
            .unwrap()
            .contains("words after the milestone"),
        "restore-from-milestone still rewinds the tree after the note edit"
    );
}

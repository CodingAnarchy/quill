//! Chapter navigation, structure editing (create / rename / reorder / delete), and chapter & book metadata.

use super::*;
use crate::project::Project;
use std::fs;

fn nav_sel(app: &App) -> usize {
    match &app.mode {
        Mode::Nav { selected, .. } => *selected,
        _ => panic!("expected Nav mode"),
    }
}

fn prompt_input(app: &App) -> &str {
    match &app.mode {
        Mode::Prompt(state) => state.input.as_str(),
        _ => panic!("expected Prompt mode"),
    }
}

#[test]
fn ctrl_n_and_ctrl_p_switch_chapters() {
    let (mut app, _dir) = two_chapter_app("nav");
    assert_eq!(app.project.active_index(), 0);
    assert!(app.buffer.line(0).contains("first"));

    app.on_key(ctrl('n'));
    assert_eq!(app.project.active_index(), 1);
    assert!(app.buffer.line(0).contains("second"));

    app.on_key(ctrl('p'));
    assert_eq!(app.project.active_index(), 0);
    assert!(app.buffer.line(0).contains("first"));
}

#[test]
fn next_previous_clamp_at_the_ends() {
    let (mut app, _dir) = two_chapter_app("clamp");
    app.on_key(ctrl('p')); // already at first — no-op
    assert_eq!(app.project.active_index(), 0);

    app.on_key(ctrl('n'));
    app.on_key(ctrl('n')); // already at last — no-op
    assert_eq!(app.project.active_index(), 1);
}

#[test]
fn switching_chapters_saves_pending_edits() {
    let (mut app, dir) = two_chapter_app("autosave");
    // Edit chapter one, then jump away without an explicit save.
    app.on_key(key(KeyCode::Char('X')));
    assert!(app.buffer.dirty);

    app.on_key(ctrl('n'));
    assert_eq!(app.project.active_index(), 1);

    // Chapter one was persisted on the way out.
    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(
        saved.starts_with('X'),
        "expected edit saved, got: {saved:?}"
    );
}

#[test]
fn nav_mode_selects_and_opens_a_chapter() {
    let (mut app, _dir) = two_chapter_app("navmode");
    app.on_key(ctrl('g')); // focus the chapter pane
    assert!(matches!(app.mode, Mode::Nav { .. }));
    assert_eq!(nav_sel(&app), 0);

    app.on_key(key(KeyCode::Down)); // select chapter two
    assert_eq!(nav_sel(&app), 1);

    app.on_key(key(KeyCode::Enter)); // open it
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1);
    assert!(app.buffer.line(0).contains("second"));
}

/// Esc out of the chapter browser (#529) dismisses the pane Ctrl-G opened in the
/// same keystroke: the outline is gone and focus is back in the editor.
#[test]
fn esc_from_browser_hides_the_pane_ctrl_g_opened() {
    use crate::ui::render_to_string as screen;
    let (mut app, _dir) = two_chapter_app("esc-hides-pane");
    assert!(!app.outline_visible, "pane starts hidden");

    app.on_key(ctrl('g')); // open the browser
    assert!(matches!(app.mode, Mode::Nav { .. }));
    assert!(app.outline_visible, "Ctrl-G shows the pane");
    assert!(
        screen(&app, 100, 20).contains("CHAPTERS"),
        "the browser pane is drawn while open"
    );

    app.on_key(key(KeyCode::Esc)); // one Esc dismisses it
    assert!(
        matches!(app.mode, Mode::Insert),
        "focus is back in the editor"
    );
    assert!(!app.outline_visible, "the pane is hidden again");
    assert!(
        !screen(&app, 100, 20).contains("CHAPTERS"),
        "the browser pane is no longer drawn after Esc"
    );
}

/// Esc restores the *pre-open* visibility: an outline the author had already
/// toggled on with Ctrl-O stays open after browsing and pressing Esc (#529).
#[test]
fn esc_from_browser_keeps_an_outline_opened_with_ctrl_o() {
    let (mut app, _dir) = two_chapter_app("esc-keeps-outline");
    app.on_key(ctrl('o')); // author opens the outline on purpose
    assert!(app.outline_visible);

    app.on_key(ctrl('g')); // browse, with the pane already up
    app.on_key(key(KeyCode::Esc)); // leave the browser
    assert!(matches!(app.mode, Mode::Insert));
    assert!(
        app.outline_visible,
        "the outline the author opened with Ctrl-O survives Esc"
    );
}

/// Opening a chapter (the other browser exit) is unaffected (#529): the active
/// chapter changes and focus returns to the editor, as before.
#[test]
fn opening_a_chapter_still_works_after_the_esc_fix() {
    let (mut app, _dir) = two_chapter_app("open-chapter-exit");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter two
    app.on_key(key(KeyCode::Enter)); // open it
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1);
    assert!(app.buffer.line(0).contains("second"));
}

fn project_titles(project: &Project) -> Vec<String> {
    project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.clone())
        .collect()
}

fn titles(app: &App) -> Vec<String> {
    project_titles(&app.project)
}

#[test]
fn create_chapter_inserts_after_the_selection() {
    let (mut app, dir) = two_chapter_app("create");
    app.on_key(ctrl('g')); // Nav, selection on chapter 0
    app.on_key(key(KeyCode::Char('a'))); // add after selection
    assert!(matches!(app.mode, Mode::Prompt(_)));
    type_str(&mut app, "Interlude");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(titles(&app), vec!["One", "Interlude", "Two"]);
    assert!(matches!(app.mode, Mode::Nav { .. }));
    // Active chapter (was 0) is unchanged; insertion was after it.
    assert_eq!(app.project.active_index(), 0);
    // Persisted.
    assert_eq!(project_titles(&Project::open(&dir).unwrap()), titles(&app));
}

#[test]
fn rename_chapter_edits_the_prefilled_title() {
    let (mut app, dir) = two_chapter_app("rename");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('r'))); // prefilled with "One"
    assert_eq!(prompt_input(&app), "One");
    for _ in 0..3 {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut app, "Uno");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(titles(&app)[0], "Uno");
    assert_eq!(project_titles(&Project::open(&dir).unwrap())[0], "Uno");
}

#[test]
fn move_selected_reorders_and_active_follows() {
    let (mut app, _dir) = two_chapter_app("move");
    app.on_key(ctrl('g')); // selection 0 == active 0
    app.on_key(key(KeyCode::Char('J'))); // move chapter 0 down

    assert_eq!(titles(&app), vec!["Two", "One"]);
    assert_eq!(nav_sel(&app), 1);
    // The active chapter ("One") followed its content to index 1.
    assert_eq!(app.project.active_title(), Some("One"));
}

#[test]
fn delete_moves_file_to_unused_and_keeps_active_consistent() {
    let (mut app, dir) = two_chapter_app("delete");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 ("Two")
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y'))); // confirm

    assert_eq!(titles(&app), vec!["One"]);
    assert!(dir.join("unused").join("two.md").exists());
    // Active was chapter 0; untouched by deleting a later chapter.
    assert_eq!(app.project.active_index(), 0);
    assert!(app.buffer.line(0).contains("first"));
}

#[test]
fn deleting_the_active_chapter_reloads_a_neighbor() {
    let (mut app, _dir) = two_chapter_app("delete-active");
    app.on_key(ctrl('g')); // selection 0 == active 0 ("One")
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y')));

    assert_eq!(titles(&app), vec!["Two"]);
    assert_eq!(app.project.active_index(), 0);
    // Buffer was reloaded to the surviving chapter.
    assert!(app.buffer.line(0).contains("second"));
}

#[test]
fn cannot_delete_the_only_chapter() {
    let (mut app, _dir) = two_chapter_app("last");
    // Remove one chapter, leaving a single chapter.
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(app.chapter_count(), 1);

    // Attempt to delete the last one — refused.
    app.on_key(key(KeyCode::Char('d')));
    assert!(matches!(app.mode, Mode::Nav { .. })); // no confirm prompt opened
    assert_eq!(app.chapter_count(), 1);
}

#[test]
fn active_chapter_survives_insert_before_it() {
    let (mut app, _dir) = two_chapter_app("identity");
    // Active is chapter 0 ("One"). Insert a chapter ahead of it.
    app.create_chapter(0, "Prologue");

    // Tracked by identity: "One" is now at index 1 and still active, with
    // its buffer untouched (no reload, no index arithmetic).
    assert_eq!(app.project.active_index(), 1);
    assert_eq!(app.project.active_title(), Some("One"));
    assert!(app.buffer.line(0).contains("first"));
}

#[test]
fn editing_book_metadata_persists_to_the_manifest() {
    let (mut app, dir) = two_chapter_app("book-meta");
    app.on_key(ctrl('g')); // chapter browser

    app.on_key(key(KeyCode::Char('t'))); // book title (prefilled "Book")
    type_str(&mut app, " of Dust"); // -> "Book of Dust"
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('w'))); // author
    type_str(&mut app, "A. Writer");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.manifest.title, "Book of Dust");
    assert_eq!(app.project.manifest.author.as_deref(), Some("A. Writer"));

    // Persisted to the manifest and survives a reopen.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.title, "Book of Dust");
    assert_eq!(reopened.manifest.author.as_deref(), Some("A. Writer"));
}

#[test]
fn clearing_an_optional_book_field_removes_it() {
    let (mut app, _dir) = two_chapter_app("book-meta-clear");
    app.project.manifest.subtitle = Some("placeholder".into());
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('b'))); // subtitle prompt (prefilled)
                                         // Clear the prefilled text, then confirm empty.
    for _ in 0.."placeholder".len() {
        app.on_key(key(KeyCode::Backspace));
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.project.manifest.subtitle, None);
}

#[test]
fn setting_a_chapter_epigraph_edits_in_the_editor_then_chains_attribution() {
    let (mut app, dir) = two_chapter_app("ch-epigraph");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 (non-active → writes now)
    app.on_key(key(KeyCode::Char('g'))); // opens the editor on the quotation
    assert!(matches!(app.mode, Mode::EditMeta(_)));
    type_str(&mut app, "Call me Ishmael.");
    app.on_key(ctrl('s')); // commit
                           // Commit chains into the single-line attribution prompt.
    assert!(matches!(app.mode, Mode::Prompt(_)));
    type_str(&mut app, "Melville");
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Nav { .. }));

    let on_disk = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        on_disk.contains("epigraph: Call me Ishmael."),
        "got:\n{on_disk}"
    );
    assert!(on_disk.contains("epigraph_by: Melville"), "got:\n{on_disk}");
    assert!(on_disk.contains("second chapter"));
}

#[test]
fn a_multiline_chapter_epigraph_is_stored_as_a_block() {
    let (mut app, dir) = two_chapter_app("ch-epigraph-multi");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // chapter 1
    app.on_key(key(KeyCode::Char('g'))); // editor
    type_str(&mut app, "Roses are red");
    app.on_key(key(KeyCode::Enter)); // a real line break in the editor
    type_str(&mut app, "Violets are blue");
    app.on_key(ctrl('s'));
    app.on_key(key(KeyCode::Esc)); // skip the attribution

    let on_disk = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        on_disk.contains("epigraph: |\n  Roses are red\n  Violets are blue\n"),
        "expected a literal block, got:\n{on_disk}"
    );
    // Reopen and confirm the line break survived.
    let reopened = Project::open(&dir).unwrap();
    let buf = Buffer::from_path(reopened.chapter_path(1)).unwrap();
    assert_eq!(
        buf.front_matter()
            .and_then(crate::frontmatter::FrontMatter::epigraph)
            .unwrap()
            .text,
        "Roses are red\nViolets are blue"
    );
}

#[test]
fn setting_a_book_epigraph_persists_to_the_manifest() {
    let (mut app, dir) = two_chapter_app("book-epigraph");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('G'))); // book epigraph editor
    type_str(&mut app, "Begin at the beginning.");
    app.on_key(ctrl('s')); // commit → chains to attribution
    type_str(&mut app, "The King");
    app.on_key(key(KeyCode::Enter));

    let ep = app.project.manifest.epigraph().unwrap();
    assert_eq!(ep.text, "Begin at the beginning.");
    assert_eq!(ep.attribution.as_deref(), Some("The King"));
    // Persisted to the manifest.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(
        reopened.manifest.epigraph().unwrap().attribution.as_deref(),
        Some("The King")
    );
}

#[test]
fn cycling_status_on_the_active_chapter_persists_to_front_matter() {
    let (mut app, dir) = two_chapter_app("status-active");
    app.on_key(ctrl('g')); // Nav, selection on chapter 0 (the active one)
    app.on_key(key(KeyCode::Char('s'))); // none → Draft
    assert_eq!(
        app.project.chapter_status(0),
        Some(crate::frontmatter::Status::Draft)
    );
    app.on_key(key(KeyCode::Char('s'))); // Draft → Revised
    assert_eq!(
        app.project.chapter_status(0),
        Some(crate::frontmatter::Status::Revised)
    );

    // Active-chapter metadata persists on save (like prose).
    app.on_key(key(KeyCode::Enter)); // open the selected chapter (back to Insert)
    app.on_key(ctrl('s'));
    let on_disk = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(
        on_disk.starts_with("---\nstatus: revised\n---\n"),
        "got:\n{on_disk}"
    );
    assert!(on_disk.contains("first chapter"));
}

#[test]
fn editing_a_non_active_chapters_summary_writes_immediately() {
    let (mut app, dir) = two_chapter_app("summary-nonactive");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 (not active)
    app.on_key(key(KeyCode::Char('m'))); // summary prompt
    type_str(&mut app, "The reckoning: part one");
    app.on_key(key(KeyCode::Enter));

    // Non-active edits hit disk right away, with quoting, prose untouched.
    let on_disk = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        on_disk.starts_with("---\nsummary: \"The reckoning: part one\"\n---\n"),
        "got:\n{on_disk}"
    );
    assert!(on_disk.contains("second chapter"));
    assert_eq!(
        app.project.chapter_summary(1),
        Some("The reckoning: part one")
    );
}

#[test]
fn setting_a_chapter_word_goal_persists_and_clears() {
    let (mut app, dir) = two_chapter_app("goal-set");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 (not active)
    app.on_key(key(KeyCode::Char('l'))); // word-goal prompt
    type_str(&mut app, "2500");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.chapter_goal(1), Some(2500));
    assert!(fs::read_to_string(dir.join("two.md"))
        .unwrap()
        .contains("goal: 2500"));
    // Survives a reopen (read back from front matter).
    assert_eq!(Project::open(&dir).unwrap().chapter_goal(1), Some(2500));

    // Clearing the field (blank input) removes the goal.
    app.on_key(key(KeyCode::Char('l')));
    for _ in 0..6 {
        app.on_key(key(KeyCode::Backspace)); // clear the prefilled "2500"
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.project.chapter_goal(1), None, "blank clears the goal");
    assert!(!fs::read_to_string(dir.join("two.md"))
        .unwrap()
        .contains("goal:"));
}

#[test]
fn setting_a_section_role_persists_clears_and_survives_reopen() {
    use crate::frontmatter::Role;
    let (mut app, dir) = two_chapter_app("role-set"); // "One" (active), "Two"
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter "Two"
    app.on_key(key(KeyCode::Char('S'))); // section-role prompt
    type_str(&mut app, "prologue");
    app.on_key(key(KeyCode::Enter));

    // "Two" became a prologue → it re-sorts ahead of the body (#180), and the
    // browser selection follows it to index 0.
    assert_eq!(
        app.project.index_of_file("two.md"),
        Some(0),
        "front matter sorts first"
    );
    assert_eq!(app.project.chapter_role(0), Some(Role::Prologue));
    assert_eq!(nav_sel(&app), 0, "the selection follows the moved chapter");
    assert!(fs::read_to_string(dir.join("two.md"))
        .unwrap()
        .contains("role: prologue"));
    // Read back from front matter on reopen (and re-sorted the same way).
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.index_of_file("two.md"), Some(0));
    assert_eq!(reopened.chapter_role(0), Some(Role::Prologue));

    // Blank input clears it back to a body chapter (the selection is on it now).
    app.on_key(key(KeyCode::Char('S')));
    for _ in 0..8 {
        app.on_key(key(KeyCode::Backspace)); // clear the prefilled "prologue"
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.project
            .chapter_role(app.project.index_of_file("two.md").unwrap()),
        None
    );
    assert!(!fs::read_to_string(dir.join("two.md"))
        .unwrap()
        .contains("role:"));
}

#[test]
fn an_unknown_section_role_is_rejected_without_changing_it() {
    let (mut app, _dir) = two_chapter_app("role-bad");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('S')));
    type_str(&mut app, "chapterish");
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Unknown section role"),
        "{:?}",
        app.message
    );
    assert_eq!(
        app.project.chapter_role(1),
        None,
        "an unknown role leaves it a body chapter"
    );
}

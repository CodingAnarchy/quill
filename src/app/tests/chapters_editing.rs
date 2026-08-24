//! Chapter navigation, structure editing (create / rename / reorder / delete), and chapter & book metadata.

use super::*;
use crate::frontmatter::Role;
use crate::project::Project;
use crate::ui::render_to_string as screen;
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

/// Deleting the ACTIVE chapter persists its latest edits first so the copy moved
/// to `unused/` is current. If that save FAILS, the moved copy would be stale and
/// the unsaved edits silently lost while the flow reports success — so a save
/// failure must ABORT the delete, leaving the chapter (and its edits) in place.
#[test]
fn deleting_the_active_chapter_aborts_when_the_pre_delete_save_fails() {
    let (mut app, dir) = two_chapter_app("delete-active-save-fail");
    // Make the active chapter dirty so the pre-delete save actually runs.
    app.on_key(key(KeyCode::Char('X')));
    assert!(app.buffer.dirty);

    // Fail that save (the next atomic write, before its rename).
    crate::atomic::fail_point::arm_pre_rename_failure();
    app.on_key(ctrl('g')); // selection 0 == active 0 ("One")
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y'))); // confirm

    // The failure is reported, not swallowed as "Moved chapter to unused/".
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Save failed"),
        "the save failure aborts the delete: {:?}",
        app.message
    );
    // The chapter is NOT moved — both survive and the active file is intact.
    assert_eq!(titles(&app), vec!["One", "Two"]);
    assert!(!dir.join("unused").join("one.md").exists());
    assert!(dir.join("one.md").exists());
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

// --- Split / merge / duplicate (#585) ----------------------------------------

/// The active chapter "first chapter" splits at the cursor: the head stays, the
/// tail becomes a new chapter inserted right after, each with the right body and
/// word count, and the manifest persists the new order.
#[test]
fn split_active_chapter_at_the_cursor_makes_two() {
    let (mut app, dir) = two_chapter_app("split"); // ch0 "first chapter\n"
                                                   // Place the cursor after "first " (6 graphemes) so the seam is mid-body.
    for _ in 0..6 {
        app.on_key(key(KeyCode::Right));
    }
    app.on_key(ctrl('g')); // browse, selection on the active chapter 0
    app.on_key(key(KeyCode::Char('x')));
    assert!(matches!(app.mode, Mode::Prompt(_)));
    type_str(&mut app, "Tail");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(titles(&app), vec!["One", "Tail", "Two"]);
    // The browser selects the new tail chapter.
    assert_eq!(nav_sel(&app), 1);
    // Head and tail bodies, on disk and in the cached counts.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(project_titles(&reopened), vec!["One", "Tail", "Two"]);
    assert_eq!(reopened.chapter_body(0).trim(), "first");
    assert_eq!(reopened.chapter_body(1).trim(), "chapter");
    assert_eq!(reopened.chapter_words(0), 1);
    assert_eq!(reopened.chapter_words(1), 1);
    // The two halves still total the original two words, booked as neither added
    // nor removed — a split is a structural move, not writing.
    app.on_idle();
    assert_eq!(app.added, 0);
    assert_eq!(app.removed, 0);
}

/// Splitting with nothing after the cursor is refused (there's no tail to make).
#[test]
fn split_with_an_empty_tail_is_refused() {
    let (mut app, _dir) = two_chapter_app("split-empty"); // ch0 "first chapter\n"
                                                          // Cursor at the very end (End-of-buffer): everything is head, no tail.
    for _ in 0..20 {
        app.on_key(key(KeyCode::Right));
    }
    for _ in 0..5 {
        app.on_key(key(KeyCode::Down)); // past the last line, lands at the end
    }
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('x')));
    type_str(&mut app, "Tail");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(titles(&app), vec!["One", "Two"], "no chapter was added");
}

/// Splitting only applies to the open chapter; choosing a different one in the
/// browser explains that rather than prompting for a title that can't apply.
#[test]
fn split_a_non_active_chapter_is_refused() {
    let (mut app, _dir) = two_chapter_app("split-nonactive");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1, which is not active
    app.on_key(key(KeyCode::Char('x')));
    assert!(matches!(app.mode, Mode::Nav { .. }), "no prompt opened");
    assert_eq!(titles(&app), vec!["One", "Two"]);
}

/// Splitting the active chapter while a reference pane (a story-bible entry, #165)
/// is focused must be a no-op: the split reads `self.buffer`, which is the bible
/// file — not the active chapter — so going ahead would truncate the bible to its
/// head and forge a bogus chapter from its tail (data loss). The bible file stays
/// byte-for-byte intact, no chapter is inserted, and a message explains why.
#[test]
fn split_with_a_focused_reference_pane_is_a_no_op() {
    let (mut app, dir) = two_chapter_app("split-ref-focus"); // active = "One"
    fs::create_dir_all(dir.join("bible")).unwrap();
    let mara = dir.join("bible").join("mara.md");
    let bible_before = "# Mara\n\n## Physical\n\ntall\n\n## Voice\n\nlow\n";
    fs::write(&mara, bible_before).unwrap();

    // Open the bible entry in a reference pane and focus it (Ctrl-w w). `active`
    // stays on chapter One, but the focused buffer is now the bible file.
    app.on_key(ctrl('g')); // chapter browser
    app.on_key(key(KeyCode::Char('f'))); // reference files
    app.on_key(key(KeyCode::Enter)); // open the (only) entry in a pane
    app.on_key(ctrl('w')); // Ctrl-w w focuses the other pane
    app.on_key(key(KeyCode::Char('w')));
    assert!(
        !app.focus_shows_active_chapter(),
        "the focused pane is the bible entry, not the active chapter"
    );

    // Drive the split path for the active chapter (the reachable Ctrl-K action).
    app.start_split_chapter();
    if matches!(app.mode, Mode::Prompt(_)) {
        type_str(&mut app, "Tail");
        app.on_key(key(KeyCode::Enter));
    }

    // No chapter was forged, and the bible file is untouched on disk.
    assert_eq!(titles(&app), vec!["One", "Two"], "no chapter was added");
    assert_eq!(
        fs::read_to_string(&mara).unwrap(),
        bible_before,
        "the focused bible file was not truncated"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Focus a chapter"),
        "the split explains it needs a focused chapter: {:?}",
        app.message
    );
}

/// Merging chapter 1 into chapter 0 combines their bodies into the first file,
/// retires the merged file to `unused/`, and leaves the manuscript's words intact
/// (nothing booked as removed — the prose only moved).
#[test]
fn merge_combines_into_the_chapter_above_and_keeps_the_words() {
    let (mut app, dir) = two_chapter_app("merge");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 ("Two")
    app.on_key(key(KeyCode::Char('M')));
    assert!(matches!(app.mode, Mode::Prompt(_)));
    app.on_key(key(KeyCode::Char('y'))); // confirm

    assert_eq!(titles(&app), vec!["One"]);
    assert_eq!(nav_sel(&app), 0);
    assert!(dir.join("unused").join("two.md").exists());
    let reopened = Project::open(&dir).unwrap();
    let body = reopened.chapter_body(0);
    assert!(body.contains("first chapter"), "{body:?}");
    assert!(body.contains("second chapter"), "{body:?}");
    // All four words survive in the combined chapter; none are booked as removed.
    assert_eq!(reopened.chapter_words(0), 4);
    app.on_idle();
    assert_eq!(app.removed, 0);
    assert_eq!(
        app.moved_to_unused, 0,
        "a merge keeps the prose in the book"
    );
}

/// Merging the active chapter into the one above reloads the combined chapter
/// into the buffer so the editor shows the merged result.
#[test]
fn merge_reloads_the_buffer_when_the_active_chapter_is_involved() {
    let (mut app, _dir) = two_chapter_app("merge-active");
    app.on_key(ctrl('n')); // make chapter 1 active
    assert_eq!(app.project.active_index(), 1);
    app.on_key(ctrl('g')); // selection follows to chapter 1
    app.on_key(key(KeyCode::Char('M')));
    app.on_key(key(KeyCode::Char('y')));

    assert_eq!(titles(&app), vec!["One"]);
    assert_eq!(app.project.active_index(), 0);
    let text = app.buffer.text();
    assert!(text.contains("first chapter"), "{text:?}");
    assert!(text.contains("second chapter"), "{text:?}");
}

/// The first chapter has nothing above it to merge into; the key is refused.
#[test]
fn merge_the_first_chapter_is_refused() {
    let (mut app, _dir) = two_chapter_app("merge-first");
    app.on_key(ctrl('g')); // selection on chapter 0
    app.on_key(key(KeyCode::Char('M')));
    assert!(matches!(app.mode, Mode::Nav { .. }), "no prompt opened");
    assert_eq!(titles(&app), vec!["One", "Two"]);
}

/// A merge whose SOURCE chapter can't be read must ABORT — never append nothing,
/// retire the source to `unused/`, and report success, silently losing the
/// source's real prose (#565/#871). The old swallowing `chapter_body` collapsed a
/// hard read failure to `""`; a chmod-000 source is a regular file whose bytes
/// `read_to_string` cannot read — the "unreadable but present" case. A genuinely
/// empty source (`Ok("")`) still merges; only a read *error* aborts. Unix-only.
#[cfg(unix)]
#[test]
fn merge_aborts_when_the_source_cannot_be_read() {
    // Scoped to this `#[cfg(unix)]` test: the trait doesn't exist on other platforms.
    use std::os::unix::fs::PermissionsExt;

    let (mut app, dir) = two_chapter_app("merge-source-read-fail");
    let two_before = fs::read_to_string(dir.join("two.md")).unwrap();
    let one_before = fs::read_to_string(dir.join("one.md")).unwrap();

    // Make the SOURCE (chapter 1, "Two") unreadable: a regular file with no read
    // permission — its path resolves but its bytes can't be read.
    fs::set_permissions(dir.join("two.md"), fs::Permissions::from_mode(0o000)).unwrap();
    // Under root the read bit is bypassed, so nothing to assert — skip rather than
    // false-pass. Checked before the merge, since a buggy blanking merge would
    // retire the file and leave a readable state behind.
    if fs::read_to_string(dir.join("two.md")).is_ok() {
        fs::set_permissions(dir.join("two.md"), fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }

    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 ("Two")
    app.on_key(key(KeyCode::Char('M')));
    app.on_key(key(KeyCode::Char('y'))); // confirm

    // Restore perms so the file reads back and the temp dir cleans up.
    fs::set_permissions(dir.join("two.md"), fs::Permissions::from_mode(0o644)).unwrap();

    // The failure is reported, not swallowed as "Merged chapters".
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Merge failed"),
        "the read failure aborts the merge: {:?}",
        app.message
    );
    // Both chapters survive — the source is NOT retired, the target unchanged.
    assert_eq!(titles(&app), vec!["One", "Two"]);
    assert!(!dir.join("unused").join("two.md").exists());
    assert_eq!(fs::read_to_string(dir.join("two.md")).unwrap(), two_before);
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), one_before);
}

/// A merge whose SOURCE is genuinely empty (`Ok("")`, not a read error) still
/// merges as before: it appends nothing and retires the empty source — only a
/// real read *failure* aborts.
#[test]
fn merge_with_an_empty_source_still_combines() {
    let (mut app, dir) = two_chapter_app("merge-empty-source");
    // Blank the source's body on disk (leave it readable).
    fs::write(dir.join("two.md"), "").unwrap();

    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 ("Two")
    app.on_key(key(KeyCode::Char('M')));
    app.on_key(key(KeyCode::Char('y'))); // confirm

    assert_eq!(app.message.as_deref(), Some("Merged chapters"));
    assert_eq!(titles(&app), vec!["One"]);
    assert!(dir.join("unused").join("two.md").exists());
    let reopened = Project::open(&dir).unwrap();
    assert!(reopened.chapter_body(0).contains("first chapter"));
}

/// Duplicating a chapter inserts a verbatim copy right after it, with the typed
/// title, and the copy carries the same body and word count.
#[test]
fn duplicate_adds_a_copy_after_the_original() {
    let (mut app, dir) = two_chapter_app("dup");
    app.on_key(ctrl('g')); // selection on chapter 0 ("One", "first chapter")
    app.on_key(key(KeyCode::Char('c')));
    assert_eq!(prompt_input(&app), "One (copy)"); // prefilled
    app.on_key(key(KeyCode::Enter));

    assert_eq!(titles(&app), vec!["One", "One (copy)", "Two"]);
    assert_eq!(nav_sel(&app), 1);
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(project_titles(&reopened), vec!["One", "One (copy)", "Two"]);
    assert_eq!(reopened.chapter_body(1).trim(), "first chapter");
    assert_eq!(reopened.chapter_words(1), 2);
    // A structural copy isn't authored prose — not booked as words added.
    app.on_idle();
    assert_eq!(app.added, 0);
}

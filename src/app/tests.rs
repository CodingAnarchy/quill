use super::*;
use crate::ai::naming::{self, Naming};
use crate::ai::scaffold::{self, Scaffold};
use crate::project::Project;
use std::fs;
use std::path::PathBuf;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Open the Git palette (Ctrl-v), move to item `index`, and run it.
fn git_menu(app: &mut App, index: usize) {
    app.on_key(ctrl('v'));
    for _ in 0..index {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
}

/// Type a string into the active prompt.
fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn nav_sel(app: &App) -> usize {
    match &app.mode {
        Mode::Nav { selected } => *selected,
        _ => panic!("expected Nav mode"),
    }
}

fn prompt_input(app: &App) -> &str {
    match &app.mode {
        Mode::Prompt(state) => state.input.as_str(),
        _ => panic!("expected Prompt mode"),
    }
}

/// A two-chapter project on disk with distinct content per chapter.
fn two_chapter_app(name: &str) -> (App, PathBuf) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-app-{}-{}", std::process::id(), name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    (App::new(buffer, project), dir)
}

#[test]
fn ctrl_n_and_ctrl_p_switch_chapters() {
    let (mut app, dir) = two_chapter_app("nav");
    assert_eq!(app.project.active_index(), 0);
    assert!(app.buffer.line(0).contains("first"));

    app.on_key(ctrl('n'));
    assert_eq!(app.project.active_index(), 1);
    assert!(app.buffer.line(0).contains("second"));

    app.on_key(ctrl('p'));
    assert_eq!(app.project.active_index(), 0);
    assert!(app.buffer.line(0).contains("first"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn next_previous_clamp_at_the_ends() {
    let (mut app, dir) = two_chapter_app("clamp");
    app.on_key(ctrl('p')); // already at first — no-op
    assert_eq!(app.project.active_index(), 0);

    app.on_key(ctrl('n'));
    app.on_key(ctrl('n')); // already at last — no-op
    assert_eq!(app.project.active_index(), 1);

    fs::remove_dir_all(&dir).ok();
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
    assert!(saved.starts_with('X'), "expected edit saved, got: {saved:?}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn nav_mode_selects_and_opens_a_chapter() {
    let (mut app, dir) = two_chapter_app("navmode");
    app.on_key(ctrl('g')); // focus the chapter pane
    assert!(matches!(app.mode, Mode::Nav { .. }));
    assert_eq!(nav_sel(&app), 0);

    app.on_key(key(KeyCode::Down)); // select chapter two
    assert_eq!(nav_sel(&app), 1);

    app.on_key(key(KeyCode::Enter)); // open it
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1);
    assert!(app.buffer.line(0).contains("second"));

    fs::remove_dir_all(&dir).ok();
}

fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
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
    typed(&mut app, "Interlude");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(titles(&app), vec!["One", "Interlude", "Two"]);
    assert!(matches!(app.mode, Mode::Nav { .. }));
    // Active chapter (was 0) is unchanged; insertion was after it.
    assert_eq!(app.project.active_index(), 0);
    // Persisted.
    assert_eq!(project_titles(&Project::open(&dir).unwrap()), titles(&app));

    fs::remove_dir_all(&dir).ok();
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
    typed(&mut app, "Uno");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(titles(&app)[0], "Uno");
    assert_eq!(project_titles(&Project::open(&dir).unwrap())[0], "Uno");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn move_selected_reorders_and_active_follows() {
    let (mut app, dir) = two_chapter_app("move");
    app.on_key(ctrl('g')); // selection 0 == active 0
    app.on_key(key(KeyCode::Char('J'))); // move chapter 0 down

    assert_eq!(titles(&app), vec!["Two", "One"]);
    assert_eq!(nav_sel(&app), 1);
    // The active chapter ("One") followed its content to index 1.
    assert_eq!(app.project.active_title(), Some("One"));

    fs::remove_dir_all(&dir).ok();
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

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn deleting_the_active_chapter_reloads_a_neighbor() {
    let (mut app, dir) = two_chapter_app("delete-active");
    app.on_key(ctrl('g')); // selection 0 == active 0 ("One")
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y')));

    assert_eq!(titles(&app), vec!["Two"]);
    assert_eq!(app.project.active_index(), 0);
    // Buffer was reloaded to the surviving chapter.
    assert!(app.buffer.line(0).contains("second"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cannot_delete_the_only_chapter() {
    let (mut app, dir) = two_chapter_app("last");
    // Remove one chapter, leaving a single chapter.
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(app.chapter_count(), 1);

    // Attempt to delete the last one — refused.
    app.on_key(key(KeyCode::Char('d')));
    assert!(matches!(app.mode, Mode::Nav { .. })); // no confirm prompt opened
    assert_eq!(app.chapter_count(), 1);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn active_chapter_survives_insert_before_it() {
    let (mut app, dir) = two_chapter_app("identity");
    // Active is chapter 0 ("One"). Insert a chapter ahead of it.
    app.create_chapter(0, "Prologue");

    // Tracked by identity: "One" is now at index 1 and still active, with
    // its buffer untouched (no reload, no index arithmetic).
    assert_eq!(app.project.active_index(), 1);
    assert_eq!(app.project.active_title(), Some("One"));
    assert!(app.buffer.line(0).contains("first"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn editing_book_metadata_persists_to_the_manifest() {
    let (mut app, dir) = two_chapter_app("book-meta");
    app.on_key(ctrl('g')); // chapter browser

    app.on_key(key(KeyCode::Char('t'))); // book title (prefilled "Book")
    typed(&mut app, " of Dust"); // -> "Book of Dust"
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('w'))); // author
    typed(&mut app, "A. Writer");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.manifest.title, "Book of Dust");
    assert_eq!(app.project.manifest.author.as_deref(), Some("A. Writer"));

    // Persisted to the manifest and survives a reopen.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.title, "Book of Dust");
    assert_eq!(reopened.manifest.author.as_deref(), Some("A. Writer"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn clearing_an_optional_book_field_removes_it() {
    let (mut app, dir) = two_chapter_app("book-meta-clear");
    app.project.manifest.subtitle = Some("placeholder".into());
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('b'))); // subtitle prompt (prefilled)
    // Clear the prefilled text, then confirm empty.
    for _ in 0.."placeholder".len() {
        app.on_key(key(KeyCode::Backspace));
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.project.manifest.subtitle, None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn setting_a_chapter_epigraph_edits_in_the_editor_then_chains_attribution() {
    let (mut app, dir) = two_chapter_app("ch-epigraph");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 (non-active → writes now)
    app.on_key(key(KeyCode::Char('g'))); // opens the editor on the quotation
    assert!(matches!(app.mode, Mode::EditMeta(_)));
    typed(&mut app, "Call me Ishmael.");
    app.on_key(ctrl('s')); // commit
    // Commit chains into the single-line attribution prompt.
    assert!(matches!(app.mode, Mode::Prompt(_)));
    typed(&mut app, "Melville");
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Nav { .. }));

    let on_disk = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(on_disk.contains("epigraph: Call me Ishmael."), "got:\n{on_disk}");
    assert!(on_disk.contains("epigraph_by: Melville"), "got:\n{on_disk}");
    assert!(on_disk.contains("second chapter"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_multiline_chapter_epigraph_is_stored_as_a_block() {
    let (mut app, dir) = two_chapter_app("ch-epigraph-multi");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // chapter 1
    app.on_key(key(KeyCode::Char('g'))); // editor
    typed(&mut app, "Roses are red");
    app.on_key(key(KeyCode::Enter)); // a real line break in the editor
    typed(&mut app, "Violets are blue");
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
        buf.front_matter().and_then(crate::frontmatter::FrontMatter::epigraph).unwrap().text,
        "Roses are red\nViolets are blue"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn setting_a_book_epigraph_persists_to_the_manifest() {
    let (mut app, dir) = two_chapter_app("book-epigraph");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('G'))); // book epigraph editor
    typed(&mut app, "Begin at the beginning.");
    app.on_key(ctrl('s')); // commit → chains to attribution
    typed(&mut app, "The King");
    app.on_key(key(KeyCode::Enter));

    let ep = app.project.manifest.epigraph().unwrap();
    assert_eq!(ep.text, "Begin at the beginning.");
    assert_eq!(ep.attribution.as_deref(), Some("The King"));
    // Persisted to the manifest.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.epigraph().unwrap().attribution.as_deref(), Some("The King"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cycling_status_on_the_active_chapter_persists_to_front_matter() {
    let (mut app, dir) = two_chapter_app("status-active");
    app.on_key(ctrl('g')); // Nav, selection on chapter 0 (the active one)
    app.on_key(key(KeyCode::Char('s'))); // none → Draft
    assert_eq!(app.project.chapter_status(0), Some(crate::frontmatter::Status::Draft));
    app.on_key(key(KeyCode::Char('s'))); // Draft → Revised
    assert_eq!(app.project.chapter_status(0), Some(crate::frontmatter::Status::Revised));

    // Active-chapter metadata persists on save (like prose).
    app.on_key(key(KeyCode::Enter)); // open the selected chapter (back to Insert)
    app.on_key(ctrl('s'));
    let on_disk = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(on_disk.starts_with("---\nstatus: revised\n---\n"), "got:\n{on_disk}");
    assert!(on_disk.contains("first chapter"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn editing_a_non_active_chapters_summary_writes_immediately() {
    let (mut app, dir) = two_chapter_app("summary-nonactive");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 (not active)
    app.on_key(key(KeyCode::Char('m'))); // summary prompt
    typed(&mut app, "The reckoning: part one");
    app.on_key(key(KeyCode::Enter));

    // Non-active edits hit disk right away, with quoting, prose untouched.
    let on_disk = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(on_disk.starts_with("---\nsummary: \"The reckoning: part one\"\n---\n"), "got:\n{on_disk}");
    assert!(on_disk.contains("second chapter"));
    assert_eq!(app.project.chapter_summary(1), Some("The reckoning: part one"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn setting_a_chapter_word_goal_persists_and_clears() {
    let (mut app, dir) = two_chapter_app("goal-set");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 (not active)
    app.on_key(key(KeyCode::Char('l'))); // word-goal prompt
    typed(&mut app, "2500");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.chapter_goal(1), Some(2500));
    assert!(fs::read_to_string(dir.join("two.md")).unwrap().contains("goal: 2500"));
    // Survives a reopen (read back from front matter).
    assert_eq!(Project::open(&dir).unwrap().chapter_goal(1), Some(2500));

    // Clearing the field (blank input) removes the goal.
    app.on_key(key(KeyCode::Char('l')));
    for _ in 0..6 {
        app.on_key(key(KeyCode::Backspace)); // clear the prefilled "2500"
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.project.chapter_goal(1), None, "blank clears the goal");
    assert!(!fs::read_to_string(dir.join("two.md")).unwrap().contains("goal:"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn metadata_survives_reopening_the_project() {
    let (mut app, dir) = two_chapter_app("meta-reopen");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // chapter 1
    app.on_key(key(KeyCode::Char('s'))); // Draft
    app.on_key(key(KeyCode::Char('v'))); // POV prompt
    typed(&mut app, "Mara");
    app.on_key(key(KeyCode::Enter));

    // Reopen from disk: the cached metadata is read back from front matter.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.chapter_status(1), Some(crate::frontmatter::Status::Draft));
    assert_eq!(reopened.chapter_pov(1), Some("Mara"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn continuous_view_edits_persist_to_the_right_chapter() {
    let (mut app, dir) = two_chapter_app("continuous"); // "first chapter", "second chapter"
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());

    // Move into chapter two and append to it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::End));
    typed(&mut app, "Z");

    app.on_key(ctrl('l')); // leave continuous — saves on the way out
    assert!(app.manuscript.is_none());

    // The edit landed in chapter two's file; chapter one is untouched.
    assert_eq!(fs::read_to_string(dir.join("two.md")).unwrap(), "second chapterZ\n");
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), "first chapter\n");
    // Back in single-chapter view at the chapter the cursor was in.
    assert_eq!(app.project.active_index(), 1);
    assert!(app.buffer.line(0).contains("second"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn entering_continuous_flushes_the_single_chapter() {
    let (mut app, dir) = two_chapter_app("continuous-flush");
    // Edit chapter one in single view, then enter continuous without saving.
    app.on_key(key(KeyCode::Char('X')));
    assert!(app.buffer.dirty);

    app.on_key(ctrl('l')); // entering must flush the dirty single chapter
    assert!(fs::read_to_string(dir.join("one.md")).unwrap().starts_with('X'));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_r_opens_the_report_and_any_key_closes_it() {
    let (mut app, dir) = two_chapter_app("report");
    app.project.manifest.start_date = Some("2026-01-11".into());
    app.project.manifest.goals = Some(crate::project::Goals {
        words: 150_000,
        chapters: 60,
        words_per_page: 250,
    });

    app.on_key(ctrl('r'));
    let report = app.overlay_lines().expect("report should be open");
    // It rendered the story line against the goals (styled rows → span text).
    let line_text = |l: &Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
    assert!(report.iter().any(|l| line_text(l).contains("/ 150,000  words")), "no story line");

    app.on_key(key(KeyCode::Char('x'))); // any key closes it (no edit)
    assert!(app.overlay.is_none());
    assert!(!app.buffer.line(0).contains('x'), "the dismissing key must not edit");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn idle_tallies_words_added_then_removed() {
    let (mut app, dir) = two_chapter_app("tally"); // ch0 "first chapter" (2 words)
    typed(&mut app, "new words "); // → "new words first chapter" (4 words): +2
    app.on_idle();
    assert_eq!(app.added, 2);
    assert_eq!(app.removed, 0);

    for _ in 0..10 {
        app.on_key(key(KeyCode::Backspace)); // delete "new words " back out
    }
    app.on_idle();
    assert_eq!(app.added, 2); // unchanged
    assert_eq!(app.removed, 2);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn deleting_a_chapter_books_moved_to_unused_not_removed() {
    let (mut app, dir) = two_chapter_app("moved"); // ch1 "second chapter" (2 words)
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y'))); // confirm delete
    assert_eq!(app.moved_to_unused, 2);
    assert_eq!(app.removed, 0);

    app.on_idle(); // the structural drop is not re-booked as removed
    assert_eq!(app.removed, 0);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn momentum_persists_and_resumes_across_a_reopen() {
    let (mut app, dir) = two_chapter_app("resume");
    app.added = 100;
    app.removed = 5;
    app.moved_to_unused = 3;
    app.persist_snapshot();

    // A fresh session over the same project resumes today's counters.
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut reopened = App::new(buffer, project);
    reopened.load_history();
    assert_eq!(reopened.added, 100);
    assert_eq!(reopened.removed, 5);
    assert_eq!(reopened.moved_to_unused, 3);

    fs::remove_dir_all(&dir).ok();
}

/// A two-chapter project where each chapter is its own part ("Beginning",
/// "End"), for exercising per-part attribution.
fn parts_app(name: &str) -> (App, PathBuf) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-app-{}-{}", std::process::id(), name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\npart_label = \"part\"\n\n\
         [[parts]]\nname = \"Beginning\"\nchapters = [1, 1]\n\n\
         [[parts]]\nname = \"End\"\nchapters = [2, 2]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    (App::new(buffer, project), dir)
}

#[test]
fn edits_are_attributed_to_their_part() {
    let (mut app, dir) = parts_app("attribute");
    // Type in chapter 0 (the "Beginning" part).
    typed(&mut app, "alpha beta "); // +2 words in ch0
    app.on_idle();
    assert_eq!(app.part_added, vec![2, 0]);

    // Switch to chapter 1 (the "End" part) and type — switching flushes ch0,
    // so its cached count matches its baseline and isn't re-booked.
    app.on_key(ctrl('n'));
    typed(&mut app, "gamma "); // +1 word in ch1
    app.on_idle();
    assert_eq!(app.part_added, vec![2, 1]);

    // The global counters equal the sums of the parts.
    assert_eq!(app.added, app.part_added.iter().sum::<usize>());
    assert_eq!(app.removed, 0);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn deleting_a_chapter_does_not_book_a_part_removal() {
    let (mut app, dir) = parts_app("part-delete");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter 1 ("End")
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y'))); // confirm delete
    app.on_idle();
    // Structural drop → moved_to_unused, not a per-part removal.
    assert_eq!(app.moved_to_unused, 2);
    assert_eq!(app.part_removed, vec![0, 0]);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn per_part_counters_persist_and_resume() {
    let (mut app, dir) = parts_app("part-resume");
    app.part_added = vec![40, 12];
    app.part_removed = vec![3, 1];
    app.persist_snapshot();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut reopened = App::new(buffer, project);
    reopened.load_history();
    assert_eq!(reopened.part_added, vec![40, 12]);
    assert_eq!(reopened.part_removed, vec![3, 1]);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn resume_ignores_per_part_counts_when_the_parts_config_changed() {
    // Persist a two-part day, then reopen with a one-part manifest: the
    // mismatched per-part counters are dropped rather than mis-applied.
    let (mut app, dir) = parts_app("part-mismatch");
    app.part_added = vec![40, 12];
    app.persist_snapshot();

    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[parts]]\nname = \"Whole\"\nchapters = [1, 2]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut reopened = App::new(buffer, project);
    reopened.load_history();
    assert_eq!(reopened.part_added, vec![0]); // one part, fresh

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn report_counts_todo_markers_across_chapters() {
    let (mut app, dir) = two_chapter_app("report-todo");
    // Add markers (mixed case) and reload the active chapter's buffer.
    fs::write(dir.join("one.md"), "first xxx chapter XXX\n").unwrap();
    fs::write(dir.join("two.md"), "second xxx chapter\n").unwrap();
    app.buffer = Buffer::from_path(app.project.active_path()).unwrap();

    // Active chapter (one.md) from the buffer: 2; chapter two from disk: 1.
    assert_eq!(app.count_todo(), 3);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn word_counts_roll_up_and_update_live() {
    let (mut app, dir) = two_chapter_app("counts"); // "first chapter", "second chapter"
    assert_eq!(app.chapter_word_count(0), 2); // active, live from buffer
    assert_eq!(app.chapter_word_count(1), 2); // cached from file
    assert_eq!(app.project_word_count(), 4);

    // Add a word to the active chapter; its count and the total grow live.
    app.on_key(key(KeyCode::Char('X')));
    app.on_key(key(KeyCode::Char(' ')));
    assert_eq!(app.chapter_word_count(0), 3);
    assert_eq!(app.project_word_count(), 5);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn word_count_cache_tracks_add_and_delete() {
    let (mut app, dir) = two_chapter_app("counts-structure");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('a'))); // add an empty chapter after ch.0
    typed(&mut app, "New");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.chapter_count(), 3);
    assert_eq!(app.project_word_count(), 4); // the new chapter is empty

    // Delete it (the new chapter is the current selection).
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(app.chapter_count(), 2);
    assert_eq!(app.project_word_count(), 4);

    fs::remove_dir_all(&dir).ok();
}

// --- AI: "Ask the manuscript" ----------------------------------------------

/// A backend that returns a canned answer (or an error) with no network — lets
/// the Q&A flow be tested deterministically and offline.
struct MockBackend {
    result: Result<String, ()>,
}

impl Backend for MockBackend {
    fn complete(&self, _req: &ai::Request) -> Result<ai::Response, ai::AiError> {
        match &self.result {
            Ok(text) => Ok(ai::Response { text: text.clone(), usage: Default::default() }),
            Err(()) => Err(ai::AiError::Api("boom".into())),
        }
    }
}

#[test]
fn ask_with_ai_off_explains_how_to_enable() {
    let (mut app, dir) = two_chapter_app("ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.ask_ai("who is the protagonist?");
    assert!(!app.ai_task.is_busy(), "nothing should be sent when AI is off");
    assert!(app.overlay.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("off"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(not(feature = "local"))]
fn offline_mode_selects_the_local_backend() {
    let (mut app, dir) = two_chapter_app("ai-offline");
    app.ai = ai::Settings { enabled: true, offline: true, ..ai::Settings::default() };
    // Offline forces the local backend, even with a key present.
    app.ai.api_key = Some("k".into());
    assert_eq!(app.ai.backend(), ai::BackendKind::Local);
    app.ask_ai("anything?");
    // This build has no `local` feature, so it can't run — nothing is sent and the
    // message explains how to enable it (rather than silently using Claude).
    assert!(!app.ai_task.is_busy());
    assert!(app.message.as_deref().unwrap_or("").contains("local"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

fn palette_labels(app: &App) -> Vec<&'static str> {
    match &app.mode {
        Mode::Palette(p) => p.items.iter().map(|it| it.label).collect(),
        _ => panic!("expected a palette to be open"),
    }
}

fn overlay_text(app: &App) -> String {
    app.overlay_lines()
        .expect("an overlay should be open")
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn git_palette_initializes_version_control() {
    let (mut app, dir) = two_chapter_app("vcs-init");
    assert!(!app.under_vc, "fresh project isn't under version control");
    git_menu(&mut app, 0); // the only item before init is "Initialize"
    assert!(app.under_vc, "the project is now under version control");
    assert!(dir.join(".git").exists() && dir.join(".gitignore").exists());
    assert!(app.message.as_deref().unwrap_or("").contains("Initialized"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn vc_status_reflects_uncommitted_changes() {
    let (mut app, dir) = two_chapter_app("vc-status");
    git_menu(&mut app, 0); // Initialize → the project's files are uncommitted
    assert!(app.vc_changes > 0, "a fresh repo has uncommitted files");

    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "snapshot");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.vc_changes, 0, "clean working tree after a commit");

    // Edit a chapter and save → the tree is dirty again (refreshed on idle).
    app.on_key(key(KeyCode::Char('X')));
    app.on_key(ctrl('s'));
    app.on_idle();
    assert!(app.vc_changes > 0, "dirty after a saved edit");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_palette_shows_commit_history() {
    let (mut app, dir) = two_chapter_app("vc-log");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 1); // History — before any commit
    assert!(overlay_text(&app).contains("No commits yet"));
    app.on_key(key(KeyCode::Esc)); // close the overlay

    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "first draft");
    app.on_key(key(KeyCode::Enter));

    git_menu(&mut app, 1); // History
    assert!(overlay_text(&app).contains("first draft"), "history lists the commit");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_palette_commits_with_a_message() {
    let (mut app, dir) = two_chapter_app("vcs-commit");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 0); // Commit
    assert!(matches!(app.mode, Mode::Prompt(_)), "a commit-message prompt opens");
    type_str(&mut app, "first draft");
    app.on_key(key(KeyCode::Enter));
    assert!(app.message.as_deref().unwrap_or("").starts_with("Committed"), "{:?}", app.message);
    // Everything was captured → the tree is now clean.
    assert_eq!(crate::vcs::commit(&app.project.root, "x").unwrap(), None);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_palette_shows_changes_diff() {
    let (mut app, dir) = two_chapter_app("vc-diff");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "base");
    app.on_key(key(KeyCode::Enter));

    // Clean tree → a message, no overlay.
    git_menu(&mut app, 2); // Changes
    assert!(app.overlay.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("No uncommitted changes"));

    // Edit + save → the diff overlay shows the changed file.
    app.on_key(key(KeyCode::Char('Q')));
    app.on_key(ctrl('s'));
    git_menu(&mut app, 2); // Changes
    assert!(overlay_text(&app).contains("one.md"), "diff names the changed file");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_palette_restores_a_chapter() {
    let (mut app, dir) = two_chapter_app("vc-restore");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 0); // Commit
    type_str(&mut app, "base");
    app.on_key(key(KeyCode::Enter));

    // Edit + save, then restore.
    app.on_key(key(KeyCode::Char('Z')));
    app.on_key(ctrl('s'));
    assert!(app.buffer.text().contains('Z'), "the edit landed");
    git_menu(&mut app, 3); // Restore chapter
    assert!(matches!(app.mode, Mode::Prompt(_)), "restore asks for confirmation");
    app.on_key(key(KeyCode::Char('y')));
    assert!(!app.buffer.text().contains('Z'), "the edit was reverted: {:?}", app.buffer.text());
    assert!(app.message.as_deref().unwrap_or("").contains("Restored"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn git_palette_configures_a_remote_then_offers_sync() {
    let (mut app, dir) = two_chapter_app("vc-remote");
    git_menu(&mut app, 0); // Initialize

    // Before a remote: the menu offers "Set remote" but not Push/Pull.
    app.on_key(ctrl('v'));
    let labels = palette_labels(&app);
    assert!(labels.contains(&"Set remote"), "{labels:?}");
    assert!(!labels.contains(&"Push") && !labels.contains(&"Pull"), "{labels:?}");
    app.on_key(key(KeyCode::Esc));

    // Set a remote through the palette (Commit, History, Changes, Restore, Set remote = idx 4).
    git_menu(&mut app, 4);
    assert!(matches!(app.mode, Mode::Prompt(_)), "set-remote prompts for a URL");
    type_str(&mut app, "https://example.com/book.git");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        crate::vcs::remote_url(&app.project.root).as_deref(),
        Some("https://example.com/book.git"),
    );

    // Now the menu offers Push and Pull, and "Change remote" instead of "Set remote".
    app.on_key(ctrl('v'));
    let labels = palette_labels(&app);
    assert!(labels.contains(&"Push") && labels.contains(&"Pull"), "{labels:?}");
    assert!(labels.contains(&"Change remote") && !labels.contains(&"Set remote"), "{labels:?}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_z_undoes_and_ctrl_y_redoes_typing() {
    let (mut app, dir) = two_chapter_app("undo");
    let original = app.buffer.text();
    type_str(&mut app, "ZZZ");
    app.on_idle(); // seal the typing burst into one undo group
    assert!(app.buffer.text().contains("ZZZ"), "the edit landed");

    app.on_key(ctrl('z'));
    assert_eq!(app.buffer.text(), original, "undo restores the original text");
    app.on_key(ctrl('y'));
    assert!(app.buffer.text().contains("ZZZ"), "redo reapplies the edit");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn continuous_view_refuses_to_open_with_an_unreadable_chapter() {
    let (mut app, dir) = two_chapter_app("cont-unreadable");
    fs::remove_file(dir.join("two.md")).unwrap(); // a chapter file vanishes
    app.on_key(ctrl('l')); // try to enter the continuous view
    assert!(app.manuscript.is_none(), "stays in the single-chapter view");
    assert!(
        app.message.as_deref().unwrap_or("").contains("Couldn't open continuous view"),
        "the failure is surfaced: {:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_is_guarded_in_the_continuous_view() {
    let (mut app, dir) = two_chapter_app("undo-continuous");
    app.on_key(ctrl('l')); // enter the continuous-manuscript view
    assert!(app.manuscript.is_some());
    app.on_key(ctrl('z'));
    assert!(
        app.message.as_deref().unwrap_or("").contains("continuous view"),
        "{:?}",
        app.message
    );
    fs::remove_dir_all(&dir).ok();
}

// --- Window splits (#163) --------------------------------------------------

/// Send a Vim window command: the `Ctrl-w` prefix then `c`.
fn window_cmd(app: &mut App, c: char) {
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char(c)));
}

#[test]
fn ctrl_w_v_splits_and_ctrl_w_c_closes() {
    let (mut app, dir) = two_chapter_app("split-open-close");
    assert!(app.split_view().is_none());
    window_cmd(&mut app, 'v'); // vertical split
    assert!(app.split_view().is_some(), "Ctrl-w v splits");
    window_cmd(&mut app, 'c'); // close
    assert!(app.split_view().is_none(), "Ctrl-w c closes the split");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn split_panes_edit_different_chapters_independently() {
    let (mut app, dir) = two_chapter_app("split-edit"); // active = "One"
    window_cmd(&mut app, 'v');
    // Focus the other pane and move it to chapter Two; edit + save there.
    window_cmd(&mut app, 'w');
    app.on_key(ctrl('n')); // other pane → "Two"
    type_str(&mut app, "TWO-EDIT");
    app.on_key(ctrl('s'));
    // Back to the first pane (still "One"); edit + save there.
    window_cmd(&mut app, 'w');
    type_str(&mut app, "ONE-EDIT");
    app.on_key(ctrl('s'));

    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(one.contains("ONE-EDIT") && two.contains("TWO-EDIT"), "one:{one:?} two:{two:?}");
    assert!(!one.contains("TWO-EDIT"), "panes didn't cross-corrupt: {one:?}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn editing_the_same_chapter_in_both_panes_stays_consistent() {
    let (mut app, dir) = two_chapter_app("split-same"); // both panes start on "One"
    window_cmd(&mut app, 'v');
    type_str(&mut app, "AAA"); // edit the focused pane
    window_cmd(&mut app, 'w'); // switch: flush, then reload the sibling on the same chapter
    assert!(app.buffer.text().contains("AAA"), "the other pane reflects the saved edit: {:?}", app.buffer.text());
    type_str(&mut app, "BBB");
    window_cmd(&mut app, 'w'); // switch back
    let text = app.buffer.text();
    assert!(text.contains("AAA") && text.contains("BBB"), "edits accumulate, no clobber: {text:?}");
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(one.contains("AAA") && one.contains("BBB"), "{one}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn split_is_guarded_in_the_continuous_view() {
    let (mut app, dir) = two_chapter_app("split-in-cont");
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_some());
    window_cmd(&mut app, 'v');
    assert!(app.split_view().is_none(), "can't split the continuous view");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn entering_continuous_closes_a_split() {
    let (mut app, dir) = two_chapter_app("cont-closes-split");
    window_cmd(&mut app, 'v');
    assert!(app.split_view().is_some());
    app.on_key(ctrl('l')); // continuous view takes the editor region
    assert!(app.split_view().is_none());
    assert!(app.manuscript.is_some());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn ctrl_w_docks_an_open_report_beside_the_editor() {
    let (mut app, dir) = two_chapter_app("dock-report");
    app.on_key(ctrl('r')); // open the progress report full-screen
    assert!(app.overlay.is_some());
    window_cmd(&mut app, 'v'); // dock it into a split (#164)
    assert!(app.overlay.is_none(), "the full-screen overlay moved into the pane");
    assert!(matches!(app.split_view(), Some(SplitView::Surface { .. })), "now a surface split");
    // The editor stays editable in the other pane.
    type_str(&mut app, "EDIT");
    assert!(app.buffer.text().contains("EDIT"));
    // Closing the pane returns to the single editor.
    window_cmd(&mut app, 'c');
    assert!(app.split_view().is_none());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn focusing_a_docked_surface_scrolls_it() {
    let (mut app, dir) = two_chapter_app("dock-scroll");
    app.on_key(ctrl('t')); // analysis report — tall enough to scroll
    window_cmd(&mut app, 'v'); // dock
    app.surface_height.set(5); // the renderer sets this; supply it for the test
    window_cmd(&mut app, 'w'); // focus the surface for scrolling
    assert!(app.surface_pane_focused());
    app.on_key(key(KeyCode::Char('j'))); // scroll down one row
    let scrolled = matches!(app.split_view(), Some(SplitView::Surface { overlay, .. }) if overlay.scroll == 1);
    assert!(scrolled, "j scrolls the docked surface");
    app.on_key(key(KeyCode::Esc)); // Esc returns focus to the editor
    assert!(!app.surface_pane_focused());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn docking_is_guarded_in_the_continuous_view() {
    let (mut app, dir) = two_chapter_app("dock-cont");
    app.on_key(ctrl('l')); // continuous view
    app.on_key(ctrl('r')); // report opens full-screen
    window_cmd(&mut app, 'v'); // try to dock
    assert!(app.split_view().is_none(), "no docking in the continuous view");
    assert!(app.overlay.is_some(), "the overlay stays full-screen");
    fs::remove_dir_all(&dir).ok();
}

// --- Reference files in a pane (#165) --------------------------------------

/// The title a reference pane shows for the non-focused buffer.
fn other_pane_title(app: &App) -> String {
    match app.split_view() {
        Some(SplitView::Editors { other, .. }) => app.pane_title_for(other),
        _ => panic!("expected two editor panes"),
    }
}

/// Add a couple of bible files to a project on disk (created after open, which is
/// fine — `reference_files` reads the filesystem live).
fn write_bible(dir: &std::path::Path) {
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(dir.join("bible").join("setting.md"), "# Setting\n\nthe city\n").unwrap();
    fs::write(chars.join("mara.md"), "# Mara\n\na sailor\n").unwrap();
}

#[test]
fn reference_files_list_the_bible_and_skip_chapters() {
    let (app, dir) = two_chapter_app("ref-list");
    write_bible(&dir);
    let files = app.project.reference_files();
    let names: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(names.contains(&"bible/characters/mara.md".to_string()), "{names:?}");
    assert!(names.contains(&"bible/setting.md".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.ends_with("one.md") || n.ends_with("two.md")), "chapters excluded: {names:?}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn f_in_the_browser_opens_the_reference_file_list() {
    let (mut app, dir) = two_chapter_app("ref-browse");
    write_bible(&dir);
    app.on_key(ctrl('g')); // chapter browser
    app.on_key(key(KeyCode::Char('f'))); // switch to reference files
    let count = match &app.mode {
        Mode::RefFiles(state) => state.files.len(),
        _ => panic!("expected RefFiles mode"),
    };
    assert_eq!(count, 2, "both bible files listed");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reference_browser_reports_when_there_are_no_files() {
    let (mut app, dir) = two_chapter_app("ref-empty");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('f')));
    assert!(matches!(app.mode, Mode::Nav { .. }), "stays in the chapter browser");
    assert!(app.message.as_deref().unwrap_or("").contains("No reference files"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn opening_a_reference_file_puts_it_in_a_pane() {
    let (mut app, dir) = two_chapter_app("ref-open");
    write_bible(&dir);
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('f')));
    app.on_key(key(KeyCode::Enter)); // open the first file (bible/characters/mara.md, sorted)
    assert!(app.split_view().is_some(), "a reference pane opened");
    assert_eq!(other_pane_title(&app), "mara", "the pane is titled by the file");
    assert!(matches!(app.mode, Mode::Insert), "focus returns to the editor");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn o_in_the_browser_opens_the_selected_chapter_in_a_pane() {
    let (mut app, dir) = two_chapter_app("ref-chapter-pane"); // active = "One"
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select "Two"
    app.on_key(key(KeyCode::Char('o')));
    assert_eq!(other_pane_title(&app), "Two", "the other chapter opens in the pane");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_reference_pane_saves_independently() {
    let (mut app, dir) = two_chapter_app("ref-save");
    write_bible(&dir);
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('f')));
    app.on_key(key(KeyCode::Enter)); // mara.md into the reference pane
    window_cmd(&mut app, 'w'); // focus it (swap into the editor)
    type_str(&mut app, "NOTE");
    app.on_key(ctrl('s'));
    let mara = fs::read_to_string(dir.join("bible").join("characters").join("mara.md")).unwrap();
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(mara.contains("NOTE"), "the bible file saved its edit: {mara:?}");
    assert!(!one.contains("NOTE"), "the chapter is untouched: {one:?}");
    fs::remove_dir_all(&dir).ok();
}

/// Match count and current index while in search mode.
fn search_state(app: &App) -> (usize, usize) {
    match &app.mode {
        Mode::Search(s) => (s.matches.len(), s.current),
        _ => panic!("expected to be in search mode"),
    }
}

#[test]
fn ctrl_f_finds_and_navigates_matches() {
    let (mut app, dir) = two_chapter_app("find");
    // Active chapter is "first chapter" — "r" occurs in "first" and "chapter".
    app.on_key(ctrl('f'));
    assert!(matches!(app.mode, Mode::Search(_)), "Ctrl-f enters search");
    type_str(&mut app, "r");
    let (count, _) = search_state(&app);
    assert_eq!(count, 2, "two matches for 'r'");

    app.on_key(key(KeyCode::Enter)); // move to navigation
    let (_, before) = search_state(&app);
    app.on_key(key(KeyCode::Char('n')));
    let (_, after) = search_state(&app);
    assert_ne!(before, after, "n advances to the next match");
    // Wrap-around: from the last match, n returns to the first.
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(search_state(&app).1, before, "n wraps around");

    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert), "Esc leaves search");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn find_then_replace_all() {
    let (mut app, dir) = two_chapter_app("replace-all");
    app.on_key(ctrl('f'));
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter)); // navigate
    app.on_key(key(KeyCode::Char('r'))); // promote to replace
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // begin stepping
    app.on_key(key(KeyCode::Char('a'))); // replace all

    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.buffer.text(), "first section\n");
    assert!(app.message.as_deref().unwrap_or("").contains("Replaced 1"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn replace_current_match_only_with_y() {
    let (mut app, dir) = two_chapter_app("replace-y");
    // Two matches for "r"; replace only the first, then bail.
    app.on_key(ctrl('f'));
    type_str(&mut app, "r");
    app.on_key(key(KeyCode::Enter)); // navigate
    app.on_key(key(KeyCode::Char('r'))); // promote to replace
    type_str(&mut app, "R");
    app.on_key(key(KeyCode::Enter)); // begin stepping at the first match
    app.on_key(key(KeyCode::Char('y'))); // replace this one, advance
    app.on_key(key(KeyCode::Esc)); // done

    assert_eq!(app.buffer.text(), "fiRst chapter\n", "only the first match was replaced");
    assert!(app.message.as_deref().unwrap_or("").contains("Replaced 1"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn replace_is_undoable() {
    let (mut app, dir) = two_chapter_app("replace-undo");
    app.on_key(ctrl('f'));
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('a')));
    assert_eq!(app.buffer.text(), "first section\n");
    // The replace went through the normal edit path, so undo reverses it (#145).
    app.on_key(ctrl('z'));
    assert_eq!(app.buffer.text(), "first chapter\n", "undo restores the pre-replace text");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn manuscript_text_includes_every_chapter() {
    let (app, dir) = two_chapter_app("ai-manuscript");
    let m = app.manuscript_text();
    assert!(m.contains("# One") && m.contains("first chapter"), "{m}");
    assert!(m.contains("# Two") && m.contains("second chapter"), "{m}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn qa_answer_opens_the_overlay() {
    let (mut app, dir) = two_chapter_app("ai-answer");
    // Drive the Q&A core with a mock backend (no network): build the request from
    // the manuscript, complete it, and apply the result like the async path does.
    let backend = MockBackend { result: Ok("Chapter One is about a locket.".into()) };
    let req = ai::build_qa_request(app.manuscript_text(), "what is chapter one about?", &app.ai.model);
    app.apply_qa_result(backend.complete(&req));

    let panel = app.overlay_lines().expect("answer overlay should open");
    let text: String = panel
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(text.contains("about a locket"), "{text}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn qa_error_sets_a_message_not_a_panel() {
    let (mut app, dir) = two_chapter_app("ai-err");
    let backend = MockBackend { result: Err(()) };
    let req = ai::build_qa_request(app.manuscript_text(), "q", &app.ai.model);
    app.apply_qa_result(backend.complete(&req));
    assert!(app.overlay.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("boom"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

// --- AI: character tracking + story bible -----------------------------------

#[test]
fn enrich_mentions_counts_names_and_last_chapter() {
    let (app, dir) = two_chapter_app("char-mentions"); // "first chapter" / "second chapter"
    let mut c = Character { name: "chapter".into(), ..Default::default() };
    app.enrich_mentions(&mut c);
    assert_eq!(c.mentions, 2); // once in each chapter body
    assert_eq!(c.last_chapter.as_deref(), Some("Two"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn character_pass_writes_bible_files_and_summarizes() {
    let (mut app, dir) = two_chapter_app("char-write");
    let chars = vec![Character {
        name: "Mara Vance".into(),
        goals: vec!["escape".into()],
        ..Default::default()
    }];
    app.apply_characters(Ok(chars));

    let file = dir.join("bible/characters/mara-vance.md");
    let content = fs::read_to_string(&file).expect("profile written");
    assert!(content.contains("name: Mara Vance"));
    assert!(content.contains("escape"));
    assert!(app.overlay.is_some(), "summary overlay should open");
    assert!(!matches!(app.mode, Mode::BibleReview(_)), "no conflicts → no review");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_bible_write_is_reported_not_counted_as_success() {
    let (mut app, dir) = two_chapter_app("char-write-fail");
    // Make `bible` a regular file so the `bible/characters` dir can't be created
    // — every profile write then fails.
    fs::write(dir.join("bible"), "x").unwrap();

    app.apply_characters(Ok(vec![Character { name: "Mara".into(), ..Default::default() }]));

    assert!(
        app.message.as_deref().unwrap_or("").contains("Couldn't write"),
        "a failed write is surfaced: {:?}",
        app.message
    );
    assert!(app.overlay.is_none(), "no success summary when the write failed");
    assert!(!matches!(app.mode, Mode::BibleReview(_)), "a write failure isn't a conflict");
    fs::remove_dir_all(&dir).ok();
}

/// Set up a project with a clean, then hand-edited, Mara profile; a second pass
/// with new content must not overwrite it.
fn conflict_app(name: &str) -> (App, PathBuf, PathBuf) {
    let (mut app, dir) = two_chapter_app(name);
    app.apply_characters(Ok(vec![Character {
        name: "Mara".into(),
        physical: vec!["green eyes".into()],
        ..Default::default()
    }]));
    let file = dir.join("bible/characters/mara.md");
    let edited = fs::read_to_string(&file).unwrap().replace("green eyes", "blue eyes");
    fs::write(&file, edited).unwrap();
    // Second pass, different content → conflict.
    app.apply_characters(Ok(vec![Character {
        name: "Mara".into(),
        physical: vec!["hazel eyes".into()],
        ..Default::default()
    }]));
    (app, dir, file)
}

#[test]
fn hand_edited_profile_enters_review_without_clobbering() {
    let (app, dir, file) = conflict_app("char-conflict");
    assert!(matches!(app.mode, Mode::BibleReview(_)), "should queue a review");
    assert!(
        fs::read_to_string(&file).unwrap().contains("blue eyes"),
        "the hand edit must survive until resolved"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn review_overwrite_writes_the_proposed_version() {
    let (mut app, dir, file) = conflict_app("char-resolve-o");
    app.on_key(key(KeyCode::Char('o')));
    assert!(matches!(app.mode, Mode::Insert), "single conflict resolved → done");
    assert!(fs::read_to_string(&file).unwrap().contains("hazel eyes"), "proposed written");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn review_keep_preserves_the_authors_version() {
    let (mut app, dir, file) = conflict_app("char-resolve-k");
    app.on_key(key(KeyCode::Char('k')));
    assert!(matches!(app.mode, Mode::Insert));
    assert!(fs::read_to_string(&file).unwrap().contains("blue eyes"), "kept the hand edit");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn review_edit_then_save_writes_the_edited_version() {
    let (mut app, dir, file) = conflict_app("char-resolve-e");
    app.on_key(key(KeyCode::Char('e'))); // load proposed into the scratch editor
    assert!(matches!(&app.mode, Mode::BibleReview(r) if r.editing.is_some()));
    app.edit_buffer_mut().insert_char('!'); // tweak the proposed version
    app.on_key(ctrl('s')); // save
    assert!(matches!(app.mode, Mode::Insert));
    let saved = fs::read_to_string(&file).unwrap();
    assert!(saved.contains("hazel eyes") && saved.contains('!'), "edited proposal saved");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: timeline & continuity ---------------------------------------------

fn cont_with(findings: Vec<continuity::Finding>) -> Continuity {
    Continuity {
        events: vec![continuity::Event {
            description: "alpha event".into(),
            chapter: "One".into(),
            story_order: 1,
        }],
        findings,
    }
}

#[test]
fn continuity_writes_timeline_and_opens_findings() {
    let (mut app, dir) = two_chapter_app("cont-write");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    let tl = fs::read_to_string(dir.join("bible/timeline.md")).expect("timeline written");
    assert!(tl.contains("alpha event"));
    assert!(matches!(app.mode, Mode::Findings(_)), "findings overlay should open");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn continuity_with_no_findings_summarizes() {
    let (mut app, dir) = two_chapter_app("cont-clean");
    app.apply_continuity(Ok(cont_with(vec![])));
    assert!(dir.join("bible/timeline.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn findings_dismiss_removes_and_closes_when_empty() {
    let (mut app, dir) = two_chapter_app("cont-dismiss");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "age".into(),
        detail: "Mara is 30 then 28".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('d'))); // dismiss the only finding
    assert!(matches!(app.mode, Mode::Insert), "empty list closes");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn findings_jump_activates_the_chapter() {
    let (mut app, dir) = two_chapter_app("cont-jump");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "location".into(),
        detail: "two places at once".into(),
        chapters: vec!["Two".into()],
    }])));
    app.on_key(key(KeyCode::Enter)); // jump to the finding's chapter
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1, "jumped to chapter Two");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn hand_edited_timeline_reviews_then_shows_findings() {
    let (mut app, dir) = two_chapter_app("cont-conflict");
    // First pass: clean timeline (with a finding so findings are pending later too).
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "first".into(),
        chapters: vec!["One".into()],
    }])));
    let tl = dir.join("bible/timeline.md");
    // Hand-edit the generated block.
    let edited = fs::read_to_string(&tl).unwrap().replace("alpha event", "TAMPERED");
    fs::write(&tl, edited).unwrap();

    // Second pass with findings → timeline conflict first.
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "second".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::BibleReview(_)), "timeline conflict reviewed first");

    app.on_key(key(KeyCode::Char('k'))); // keep mine → drains queue → findings open
    assert!(matches!(app.mode, Mode::Findings(_)), "findings follow the review");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: setting & worldbuilding -------------------------------------------

#[test]
fn setting_writes_bible_and_opens_rule_findings() {
    let (mut app, dir) = two_chapter_app("setting-write");
    let s = Setting {
        categories: vec![setting::Category {
            name: "Magic system".into(),
            entries: vec![setting::Entry { name: "Casting".into(), detail: "needs a focus".into() }],
        }],
        gazetteer: vec!["Aldermoor".into()],
        findings: vec![setting::RuleFinding {
            rule: "magic needs a focus".into(),
            violation: "casts barehanded".into(),
            chapters: vec!["Two".into()],
        }],
    };
    app.apply_setting(Ok(s));
    let file = fs::read_to_string(dir.join("bible/setting.md")).expect("setting written");
    assert!(file.contains("## Magic system"));
    assert!(file.contains("## Gazetteer"));
    assert!(matches!(app.mode, Mode::Findings(_)), "rule findings overlay opens");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn setting_with_no_violations_summarizes() {
    let (mut app, dir) = two_chapter_app("setting-clean");
    let s = Setting {
        categories: vec![setting::Category { name: "Geography".into(), entries: vec![] }],
        gazetteer: vec![],
        findings: vec![],
    };
    app.apply_setting(Ok(s));
    assert!(dir.join("bible/setting.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: scaffolding (outline + gaps + xxx insertion) ----------------------

#[test]
fn scaffold_writes_outline_and_opens_gap_findings() {
    let (mut app, dir) = two_chapter_app("scaffold");
    let s = Scaffold {
        outline: vec![scaffold::OutlineItem { chapter: "One".into(), beats: vec!["a beat".into()] }],
        gaps: vec![scaffold::Gap { detail: "no clear goal".into(), chapter: "Two".into() }],
    };
    app.apply_scaffold(Ok(s));
    let file = fs::read_to_string(dir.join("bible/outline.md")).expect("outline written");
    assert!(file.contains("## One") && file.contains("a beat"));
    assert!(file.contains("## Gaps"));
    assert!(matches!(app.mode, Mode::Findings(_)), "gap findings overlay opens");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn marking_a_finding_inserts_an_xxx_todo_in_a_nonactive_chapter() {
    let (mut app, dir) = two_chapter_app("mark-todo"); // active is "One"
    app.apply_scaffold(Ok(Scaffold {
        outline: vec![],
        gaps: vec![scaffold::Gap { detail: "needs a hook".into(), chapter: "Two".into() }],
    }));
    let before = app.count_todo();
    app.on_key(key(KeyCode::Char('i'))); // mark xxx in "Two" (non-active → file)
    assert!(app.count_todo() > before, "the report's xxx count should rise");
    assert!(fs::read_to_string(dir.join("two.md")).unwrap().contains("xxx"), "marker on disk");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn marking_a_finding_in_the_active_chapter_updates_the_buffer() {
    let (mut app, dir) = two_chapter_app("mark-active"); // active is "One"
    app.apply_scaffold(Ok(Scaffold {
        outline: vec![],
        gaps: vec![scaffold::Gap { detail: "weak open".into(), chapter: "One".into() }],
    }));
    app.on_key(key(KeyCode::Char('i')));
    assert!(app.buffer.text().contains("xxx"), "marker shows in the active buffer");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: naming & term consistency -----------------------------------------

#[test]
fn known_entities_reads_gazetteer_and_character_names() {
    let (app, dir) = two_chapter_app("entities");
    fs::create_dir_all(dir.join("bible/characters")).unwrap();
    fs::write(
        dir.join("bible/setting.md"),
        "---\n---\n\n## Gazetteer\n- Aldermoor\n- lightsteel\n\n## Other\n- nope\n",
    )
    .unwrap();
    fs::write(dir.join("bible/characters/mara.md"), "---\nname: Mara Vance\n---\n").unwrap();

    let e = app.known_entities();
    assert!(e.contains(&"Aldermoor".to_string()));
    assert!(e.contains(&"lightsteel".to_string()));
    assert!(e.contains(&"Mara Vance".to_string()));
    assert!(!e.contains(&"nope".to_string()), "only the Gazetteer section is read");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn naming_findings_open_the_overlay() {
    let (mut app, dir) = two_chapter_app("naming");
    app.apply_naming(Ok(Naming {
        findings: vec![naming::NameFinding {
            name: "Mara".into(),
            variants: vec!["Marah".into()],
            chapters: vec!["Two".into()],
        }],
    }));
    assert!(matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn naming_with_no_variants_summarizes() {
    let (mut app, dir) = two_chapter_app("naming-clean");
    app.apply_naming(Ok(Naming { findings: vec![] }));
    assert!(app.overlay.is_some());
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: command palette ----------------------------------------------------

#[test]
fn palette_opens_with_the_ai_actions() {
    let (mut app, dir) = two_chapter_app("palette");
    app.open_ai_palette();
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.items.len(), 9, "all AI actions listed"),
        _ => panic!("expected the palette to open"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn palette_down_moves_the_selection() {
    let (mut app, dir) = two_chapter_app("palette-nav");
    app.open_ai_palette();
    app.on_key(key(KeyCode::Down));
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.selected, 1),
        _ => panic!("expected the palette"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn palette_enter_on_ask_opens_the_question_prompt() {
    let (mut app, dir) = two_chapter_app("palette-ask");
    app.open_ai_palette(); // "Ask the manuscript" is the first item
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Prompt(_)), "Ask opens the question prompt");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn palette_esc_closes_to_insert() {
    let (mut app, dir) = two_chapter_app("palette-esc");
    app.open_ai_palette();
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: prose & style ------------------------------------------------------

#[test]
fn style_findings_open_the_overlay() {
    let (mut app, dir) = two_chapter_app("style");
    app.apply_style(Ok(ai::style::Style {
        findings: vec![ai::style::StyleFinding {
            kind: "POV".into(),
            detail: "head-hop in the duel scene".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    assert!(matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn style_with_no_findings_summarizes() {
    let (mut app, dir) = two_chapter_app("style-clean");
    app.apply_style(Ok(ai::style::Style { findings: vec![] }));
    assert!(app.overlay.is_some());
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: submission material ------------------------------------------------

#[test]
fn submission_writes_three_artifacts_and_summarizes() {
    let (mut app, dir) = two_chapter_app("submission");
    app.apply_submission(Ok(ai::submission::Submission {
        synopsis: "Mara finds a locket and confronts her brother.".into(),
        blurb: "A locket. A secret. A reckoning.".into(),
        query: "Dear Agent, RED STATE EARTH is …".into(),
    }));
    assert!(fs::read_to_string(dir.join("submission/synopsis.md")).unwrap().contains("confronts her brother"));
    assert!(fs::read_to_string(dir.join("submission/blurb.md")).unwrap().contains("reckoning"));
    assert!(fs::read_to_string(dir.join("submission/query.md")).unwrap().contains("Dear Agent"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::BibleReview(_)), "no conflicts on a fresh write");
    fs::remove_dir_all(&dir).ok();
}

// --- Local analysis report --------------------------------------------------

#[test]
fn ctrl_t_opens_the_analysis_report_with_a_row_per_chapter() {
    let (mut app, dir) = two_chapter_app("analysis-open");
    assert!(app.overlay.is_none());
    app.on_key(ctrl('t'));
    let lines = app.overlay_lines().expect("analysis report opens");
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Readability"), "{text}");
    assert!(text.contains("Sentence length"), "sentence section: {text}");
    assert!(text.contains("One") && text.contains("Two"), "both chapters listed: {text}");
    assert!(text.contains("Whole manuscript"), "overall row: {text}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_non_scroll_key_closes_the_analysis_report() {
    let (mut app, dir) = two_chapter_app("analysis-close");
    app.viewport_height = 5;
    app.on_key(ctrl('t'));
    app.on_key(key(KeyCode::Down)); // scroll a bit first
    assert!(app.overlay_scroll() > 0);
    app.on_key(key(KeyCode::Char('x'))); // not a scroll key → close
    assert!(app.overlay.is_none(), "a non-scroll key dismisses the report");
    assert_eq!(app.overlay_scroll(), 0, "scroll resets on close");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_analysis_report_scrolls_and_clamps() {
    let (mut app, dir) = two_chapter_app("analysis-scroll");
    app.viewport_height = 5; // tiny screen → 3-row interior
    app.on_key(ctrl('t'));
    let len = app.overlay_lines().unwrap().len() as u16;
    let max = len - 3; // content rows beyond the interior
    assert!(max > 0, "report is taller than the viewport");

    app.on_key(key(KeyCode::Down));
    assert_eq!(app.overlay_scroll(), 1, "Down scrolls one row");
    app.on_key(key(KeyCode::End));
    assert_eq!(app.overlay_scroll(), max, "End jumps to the bottom");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.overlay_scroll(), max, "can't scroll past the end");
    assert!(app.overlay.is_some(), "scroll keys don't close the overlay");
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.overlay_scroll(), 0, "Home jumps back to the top");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn submission_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("submission-safe");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_submission(Ok(ai::submission::Submission {
        synopsis: "S".into(),
        blurb: "B".into(),
        query: "Q".into(),
    }));
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: cost confirm & revision triage -------------------------------------

fn findings_len(app: &App) -> usize {
    match &app.mode {
        Mode::Findings(f) => f.items.len(),
        _ => panic!("expected the findings overlay"),
    }
}

#[test]
fn an_expensive_pass_asks_for_confirmation_first() {
    let (mut app, dir) = two_chapter_app("cost-confirm");
    app.ai.cost_threshold = 0.0; // any non-empty manuscript exceeds it
    app.start_ai_action(crate::mode::AiAction::Continuity);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::ConfirmAi(_)), "cost confirm opens");
            assert!(s.label.contains("run?"), "{}", s.label);
        }
        _ => panic!("expected the cost confirm prompt"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_cheap_pass_runs_without_confirmation() {
    let (mut app, dir) = two_chapter_app("cost-cheap");
    app.ai.cost_threshold = 1_000.0; // nothing this small could exceed it
    app.start_ai_action(crate::mode::AiAction::Style);
    // It dispatched straight through (no key configured → a status message),
    // never stopping at a confirm prompt.
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm for a cheap pass");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn confirming_an_expensive_pass_dispatches_it() {
    let (mut app, dir) = two_chapter_app("cost-yes");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity);
    assert!(matches!(app.mode, Mode::Prompt(_)));
    app.on_key(key(KeyCode::Char('y'))); // confirm → dispatch (no key → message)
    assert!(!matches!(app.mode, Mode::Prompt(_)), "confirm closes the prompt");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn declining_an_expensive_pass_cancels_it() {
    let (mut app, dir) = two_chapter_app("cost-no");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity);
    app.on_key(key(KeyCode::Char('n')));
    assert!(matches!(app.mode, Mode::Insert), "declining returns to editing");
    assert!(!app.ai_task.is_busy(), "nothing was sent");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn triage_accumulates_findings_across_passes_deduped() {
    let (mut app, dir) = two_chapter_app("triage-accum");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    app.apply_style(Ok(ai::style::Style {
        findings: vec![ai::style::StyleFinding {
            kind: "POV".into(),
            detail: "head-hop".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    // A repeat of the continuity finding must not double-count.
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    app.open_triage();
    assert_eq!(findings_len(&app), 2, "two distinct findings, deduped");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn triage_with_no_findings_shows_a_message() {
    let (mut app, dir) = two_chapter_app("triage-empty");
    app.open_triage();
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.message.as_deref().unwrap_or("").contains("No findings"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

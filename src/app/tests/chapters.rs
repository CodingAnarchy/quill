//! Tests for chapter navigation, structure editing, metadata, and the
//! continuous view / word-count tracking.

use super::*;
use crate::project::Project;
use std::fs;

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
fn setting_a_section_role_persists_clears_and_survives_reopen() {
    use crate::frontmatter::Role;
    let (mut app, dir) = two_chapter_app("role-set"); // "One" (active), "Two"
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select chapter "Two"
    app.on_key(key(KeyCode::Char('S'))); // section-role prompt
    typed(&mut app, "prologue");
    app.on_key(key(KeyCode::Enter));

    // "Two" became a prologue → it re-sorts ahead of the body (#180), and the
    // browser selection follows it to index 0.
    assert_eq!(app.project.index_of_file("two.md"), Some(0), "front matter sorts first");
    assert_eq!(app.project.chapter_role(0), Some(Role::Prologue));
    assert_eq!(nav_sel(&app), 0, "the selection follows the moved chapter");
    assert!(fs::read_to_string(dir.join("two.md")).unwrap().contains("role: prologue"));
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
    assert_eq!(app.project.chapter_role(app.project.index_of_file("two.md").unwrap()), None);
    assert!(!fs::read_to_string(dir.join("two.md")).unwrap().contains("role:"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unknown_section_role_is_rejected_without_changing_it() {
    let (mut app, dir) = two_chapter_app("role-bad");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('S')));
    typed(&mut app, "chapterish");
    app.on_key(key(KeyCode::Enter));
    assert!(app.message.as_deref().unwrap_or("").contains("Unknown section role"), "{:?}", app.message);
    assert_eq!(app.project.chapter_role(1), None, "an unknown role leaves it a body chapter");
    fs::remove_dir_all(&dir).ok();
}

/// A project on disk with three files listed body / prologue / appendix in the
/// manifest (deliberately out of band order) — front-matter and back-matter
/// front matter set. Returns its dir.
fn sectioned_project(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-sect-{}-{}", std::process::id(), name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"Body\"\nfile = \"b.md\"\n\n\
         [[chapters]]\ntitle = \"Pro\"\nfile = \"p.md\"\n\n\
         [[chapters]]\ntitle = \"App\"\nfile = \"a.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("b.md"), "the body\n").unwrap();
    fs::write(dir.join("p.md"), "---\nrole: prologue\n---\nbefore\n").unwrap();
    fs::write(dir.join("a.md"), "---\nrole: appendix\n---\nnotes\n").unwrap();
    dir
}

#[test]
fn front_and_back_matter_sort_around_the_body_on_load() {
    use crate::frontmatter::Role;
    let dir = sectioned_project("order");
    let p = Project::open(&dir).unwrap();
    // Reading order: prologue (front), body, appendix (back) — not manifest order.
    assert_eq!(p.index_of_file("p.md"), Some(0));
    assert_eq!(p.index_of_file("b.md"), Some(1));
    assert_eq!(p.index_of_file("a.md"), Some(2));
    assert_eq!(p.chapter_role(0), Some(Role::Prologue));
    assert_eq!(p.body_chapter_count(), 1, "only the body file counts as a chapter");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_pane_groups_sections_under_band_headers() {
    let dir = sectioned_project("pane-group"); // [prologue Pro, body Body, appendix App]
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.on_key(ctrl('g')); // show the nav pane in browse mode
    let screen = crate::ui::render_to_string(&app, 80, 20);

    let f = screen.find("FRONT MATTER").expect("front-matter header");
    let c = screen.find("CHAPTERS").expect("body header");
    let b = screen.find("BACK MATTER").expect("back-matter header");
    assert!(f < c && c < b, "bands shown in reading order:\n{screen}");
    assert!(screen.contains("1. Body"), "the body chapter is numbered:\n{screen}");
    // The total separates body words from section words (#181).
    assert!(screen.contains("in sections"), "section words kept clear of the body total:\n{screen}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_all_body_pane_keeps_the_flat_unnumbered_list() {
    let (mut app, dir) = two_chapter_app("pane-flat"); // "One", "Two", no roles
    app.on_key(ctrl('g'));
    let screen = crate::ui::render_to_string(&app, 80, 16);
    assert!(screen.contains("CHAPTERS"), "still has the body header:\n{screen}");
    assert!(!screen.contains("FRONT MATTER") && !screen.contains("BACK MATTER"), "no section headers:\n{screen}");
    assert!(!screen.contains("1. One"), "no numbers without sections:\n{screen}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_compiles_the_manuscript_to_a_single_markdown_file() {
    let dir = sectioned_project("export"); // loads as [prologue Pro, body Body, appendix App]
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);

    app.export_markdown();
    assert!(app.message.as_deref().unwrap_or("").contains("Exported to export/"), "{:?}", app.message);

    let out = fs::read_to_string(dir.join("export").join("book.md")).unwrap();
    assert!(out.starts_with("# Book\n"), "book title heading:\n{out}");
    // Chapters compiled in reading order (front → body → back), front matter gone.
    let pro = out.find("## Pro").unwrap();
    let body = out.find("## Body").unwrap();
    let appendix = out.find("## App").unwrap();
    assert!(pro < body && body < appendix, "reading order preserved:\n{out}");
    assert!(out.contains("before") && !out.contains("role: prologue"), "prose without front matter:\n{out}");

    // The compiled output isn't itself a reference file (#165).
    assert!(
        !app.project.reference_files().iter().any(|p| p.to_string_lossy().contains("export")),
        "export/ is excluded from the reference-file browser"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parts_map_to_body_chapters_not_sections() {
    // A project with a prologue + 4 body chapters + an appendix, listed out of
    // band order, and two parts over the body chapters (#185).
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-parts-sect-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[parts]]\nname = \"Act I\"\nchapters = [1, 2]\n\n\
         [[parts]]\nname = \"Act II\"\nchapters = [3, 4]\n\n\
         [[chapters]]\ntitle = \"B1\"\nfile = \"b1.md\"\n\n\
         [[chapters]]\ntitle = \"B2\"\nfile = \"b2.md\"\n\n\
         [[chapters]]\ntitle = \"Pro\"\nfile = \"pro.md\"\n\n\
         [[chapters]]\ntitle = \"B3\"\nfile = \"b3.md\"\n\n\
         [[chapters]]\ntitle = \"B4\"\nfile = \"b4.md\"\n\n\
         [[chapters]]\ntitle = \"App\"\nfile = \"ap.md\"\n",
    )
    .unwrap();
    for f in ["b1.md", "b2.md", "b3.md", "b4.md"] {
        fs::write(dir.join(f), "body words here\n").unwrap();
    }
    fs::write(dir.join("pro.md"), "---\nrole: prologue\n---\nbefore\n").unwrap();
    fs::write(dir.join("ap.md"), "---\nrole: appendix\n---\nnotes\n").unwrap();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let app = App::new(buffer, project);

    // Load order: [pro, b1, b2, b3, b4, ap]. The parts cover body chapters only.
    let ranges = app.part_ranges();
    assert_eq!(ranges[0].1, 1..3, "Act I = the first two body chapters, not the prologue");
    assert_eq!(ranges[1].1, 3..5, "Act II = the next two body chapters, not the appendix");
    // And part membership: sections belong to no part.
    assert_eq!(app.project.manifest.part_of(0), None, "prologue is in no part");
    assert_eq!(app.project.manifest.part_of(1), Some(0), "first body chapter is in Act I");
    assert_eq!(app.project.manifest.part_of(4), Some(1), "fourth body chapter is in Act II");
    assert_eq!(app.project.manifest.part_of(5), None, "appendix is in no part");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reordering_is_clamped_within_a_band() {
    let dir = sectioned_project("clamp"); // loads as [prologue p, body b, appendix a]
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.on_key(ctrl('g')); // selection on index 0 (the prologue, the active file)

    // The prologue can't move down into the body.
    app.on_key(key(KeyCode::Char('J')));
    assert_eq!(app.project.index_of_file("p.md"), Some(0), "front matter stays ahead of the body");
    assert!(app.message.as_deref().unwrap_or("").contains("stays within"));

    // The body can't move up into front matter.
    app.on_key(key(KeyCode::Down)); // select the body (index 1)
    app.on_key(key(KeyCode::Char('K')));
    assert_eq!(app.project.index_of_file("b.md"), Some(1), "the body stays out of front matter");
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
fn the_report_counts_body_words_not_section_words() {
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-rep-sect-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[goals]\nwords = 100\nchapters = 5\n\n\
         [[chapters]]\ntitle = \"Body\"\nfile = \"b.md\"\n\n\
         [[chapters]]\ntitle = \"Appendix\"\nfile = \"a.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("b.md"), "one two three four five\n").unwrap(); // 5 body words
    fs::write(dir.join("a.md"), "---\nrole: appendix\n---\nalpha beta gamma\n").unwrap(); // 3 section words

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let app = App::new(buffer, project);

    assert_eq!(app.body_word_count(), 5);
    assert_eq!(app.section_word_count(), 3);

    let report: String = app
        .report_lines()
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(report.contains("5 / 100  words"), "the goal measures body words only:\n{report}");
    assert!(report.contains("(+3 words in front/back matter, not counted)"), "{report}");
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


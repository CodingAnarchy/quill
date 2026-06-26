//! Section roles and bands, parts, and export.

use super::*;
use crate::project::Project;
use std::fs;

/// A project on disk with three files listed body / prologue / appendix in the
/// manifest (deliberately out of band order) — front-matter and back-matter
/// front matter set. Returns its dir.
fn sectioned_project(name: &str) -> TempProject {
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
    TempProject::new(dir)
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
}

#[test]
fn an_all_body_pane_keeps_the_flat_unnumbered_list() {
    let (mut app, _dir) = two_chapter_app("pane-flat"); // "One", "Two", no roles
    app.on_key(ctrl('g'));
    let screen = crate::ui::render_to_string(&app, 80, 16);
    assert!(screen.contains("CHAPTERS"), "still has the body header:\n{screen}");
    assert!(!screen.contains("FRONT MATTER") && !screen.contains("BACK MATTER"), "no section headers:\n{screen}");
    assert!(!screen.contains("1. One"), "no numbers without sections:\n{screen}");
}

#[test]
fn export_compiles_the_manuscript_to_a_single_markdown_file() {
    let dir = sectioned_project("export"); // loads as [prologue Pro, body Body, appendix App]
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);

    app.run_export(crate::mode::ExportFormat::Markdown, crate::mode::ExportScope::ActiveBook);
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
}

#[test]
fn a_part_range_past_the_chapters_exports_and_trees_without_panicking() {
    // A valid-TOML but structurally-odd manifest (#407): a [[parts]] whose range
    // points past the only chapter. The part just never matches a chapter, so the
    // export compile and the series tree must report/skip it, not panic.
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-oddparts-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[parts]]\nname = \"Ghost\"\nchapters = [5, 9]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "the only chapter\n").unwrap();
    let dir = TempProject::new(dir);

    let project = Project::open(&dir).expect("an out-of-range part range still opens");
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);

    app.run_export(crate::mode::ExportFormat::Markdown, crate::mode::ExportScope::ActiveBook);
    let out = fs::read_to_string(dir.join("export").join("book.md")).unwrap();
    assert!(out.contains("the only chapter"), "the chapter still compiles:\n{out}");

    app.open_tree(); // builds the series tree rows over the same odd parts
    assert!(matches!(app.mode, Mode::Tree(_)), "the tree opens without panicking");
}

#[test]
fn ctrl_e_opens_the_export_menu() {
    let (mut app, _dir) = two_chapter_app("export-menu");
    app.on_key(ctrl('e'));
    let Mode::Palette(p) = &app.mode else { panic!("expected the export palette") };
    assert_eq!(p.title, "Export");
}

#[test]
fn export_writes_a_docx_file() {
    let dir = sectioned_project("export-docx");
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.run_export(crate::mode::ExportFormat::Docx, crate::mode::ExportScope::ActiveBook);
    assert!(app.message.as_deref().unwrap_or("").contains("Exported to export/"), "{:?}", app.message);
    let bytes = fs::read(dir.join("export").join("book.docx")).unwrap();
    assert_eq!(&bytes[..2], b"PK", "a .docx is a zip container");
}

#[test]
fn export_is_guarded_in_the_continuous_view() {
    let (mut app, dir) = two_chapter_app("export-cont");
    app.on_key(ctrl('l')); // enter the continuous view
    app.run_export(crate::mode::ExportFormat::Markdown, crate::mode::ExportScope::ActiveBook);
    assert!(app.message.as_deref().unwrap_or("").contains("Leave the continuous view"), "{:?}", app.message);
    assert!(!dir.join("export").exists(), "no file written from the continuous view");
}

#[test]
fn parts_map_to_body_chapters_not_sections() {
    // A project with a prologue + 4 body chapters + an appendix, listed out of
    // band order, and two parts over the body chapters (#185).
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-parts-sect-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dir = TempProject::new(dir);
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
}

#[test]
fn metadata_survives_reopening_the_project() {
    let (mut app, dir) = two_chapter_app("meta-reopen");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // chapter 1
    app.on_key(key(KeyCode::Char('s'))); // Draft
    app.on_key(key(KeyCode::Char('v'))); // POV prompt
    type_str(&mut app, "Mara");
    app.on_key(key(KeyCode::Enter));

    // Reopen from disk: the cached metadata is read back from front matter.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.chapter_status(1), Some(crate::frontmatter::Status::Draft));
    assert_eq!(reopened.chapter_pov(1), Some("Mara"));
}

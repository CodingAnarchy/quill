//! Import & setup-wizard flows (#282/#283/#286/#298): the new-project setup
//! wizard, importing a folder of notes into the bible, creating a new project
//! from a folder of notes, and importing an existing manuscript (Markdown,
//! `.txt`, `.docx`, `.scriv`, or a folder of `.md` chapters) into a chaptered
//! project.

use super::*;
use crate::mode::WizardStep;

// --- Setup wizard (#282) ---------------------------------------------------

fn at_step(app: &App, step: WizardStep) -> bool {
    matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::Wizard(st) if st == step))
}

/// Create a new project (with the wizard enabled) and return the app sitting at
/// the wizard's opening y/N prompt.
fn app_at_new_project(tag: &str) -> (App, PathBuf) {
    isolate(tag);
    let parent = books_dir(&format!("{tag}-p"));
    let cur = make_project(&format!("{tag}-cur"), "Current", "c\n");
    let mut app = open_app(&cur);
    let target = parent.join("New Book");
    app.create_project_at(target.clone());
    pick_template(&mut app, "Empty"); // clear the structure-template picker (#611)
    (app, target)
}

#[test]
fn creating_a_project_offers_the_setup_wizard() {
    let (app, target) = app_at_new_project("wiz-offer");
    assert_eq!(
        app.project.manifest.title, "New Book",
        "the project was created and opened"
    );
    assert!(Project::manifest_path(&target).exists());
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::WizardStart)),
        "wizard offered"
    );
}

#[test]
fn declining_the_wizard_lands_in_the_editor() {
    let (mut app, _t) = app_at_new_project("wiz-decline");
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "declining lands in the editor"
    );
}

#[test]
fn the_goals_step_records_goals_in_the_manifest() {
    let (mut app, _t) = app_at_new_project("wiz-goals");
    app.on_key(key(KeyCode::Char('y'))); // start → Goals
    assert!(at_step(&app, WizardStep::Goals));
    type_str(&mut app, "80000 30");
    app.on_key(key(KeyCode::Enter));

    let goals = app.project.manifest.goals.clone().expect("goals recorded");
    assert_eq!((goals.words, goals.chapters), (80000, 30));
    let manifest = fs::read_to_string(Project::manifest_path(&app.project.root)).unwrap();
    assert!(manifest.contains("80000"), "goals persisted to book.toml");
}

#[test]
fn ai_steps_are_skipped_when_ai_is_off() {
    let (mut app, _t) = app_at_new_project("wiz-noai");
    app.on_key(key(KeyCode::Char('y'))); // → Goals
    app.on_key(key(KeyCode::Enter)); // skip goals (blank)
                                     // AI is off in tests, so Characters and Setting are skipped → straight to Notes.
    assert!(
        at_step(&app, WizardStep::Notes),
        "AI steps skipped when AI is off"
    );
}

#[test]
fn the_notes_step_writes_notes_md_and_finishes() {
    let (mut app, _t) = app_at_new_project("wiz-notes");
    app.on_key(key(KeyCode::Char('y')));
    app.on_key(key(KeyCode::Enter)); // skip goals → Notes (AI off)
    assert!(at_step(&app, WizardStep::Notes));
    type_str(&mut app, "remember the locket");
    app.on_key(key(KeyCode::Enter));

    assert!(!matches!(app.mode, Mode::Prompt(_)), "the wizard finishes");
    let notes = fs::read_to_string(app.project.root.join("notes.md")).unwrap();
    assert!(notes.contains("remember the locket"));
}

#[test]
fn the_notes_step_never_clobbers_an_existing_notes_md() {
    let (mut app, _t) = app_at_new_project("wiz-notes-keep");
    // An author already has a notes.md (e.g. hand-created before setup).
    let existing = app.project.root.join("notes.md");
    fs::write(&existing, "# Notes\n\nthe original locket\n").unwrap();

    app.on_key(key(KeyCode::Char('y')));
    app.on_key(key(KeyCode::Enter)); // skip goals → Notes (AI off)
    assert!(at_step(&app, WizardStep::Notes));
    type_str(&mut app, "a brand-new note");
    app.on_key(key(KeyCode::Enter));

    // The original is untouched; the new note lands in a unique file.
    assert_eq!(
        fs::read_to_string(&existing).unwrap(),
        "# Notes\n\nthe original locket\n",
        "the pre-existing notes.md is preserved verbatim"
    );
    let fresh = fs::read_to_string(app.project.root.join("notes-1.md")).unwrap();
    assert!(
        fresh.contains("a brand-new note"),
        "the wizard's note is written to a unique name"
    );
}

#[test]
fn esc_dismisses_the_rest_of_the_wizard() {
    let (mut app, _t) = app_at_new_project("wiz-esc");
    app.on_key(key(KeyCode::Char('y'))); // → Goals
    app.on_key(key(KeyCode::Esc));
    assert!(!matches!(app.mode, Mode::Prompt(_)), "Esc ends the wizard");
}

#[test]
fn the_wizard_can_be_disabled_in_config() {
    isolate("wiz-off");
    let cfg_dir = crate::config::config_dir().unwrap();
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(
        cfg_dir.join("config.toml"),
        "[projects]\nsetup_wizard = false\n",
    )
    .unwrap();

    let parent = books_dir("wiz-off-p");
    let cur = make_project("wiz-off-cur", "Current", "c\n");
    let mut app = open_app(&cur);
    app.create_project_at(parent.join("New Book"));
    pick_template(&mut app, "Empty"); // the picker still shows; the wizard is what's off

    assert_eq!(
        app.project.manifest.title, "New Book",
        "still created and opened"
    );
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no wizard when disabled"
    );
}

// --- Notes import (#283) ---------------------------------------------------

#[test]
fn importing_with_ai_off_preserves_the_source_file() {
    isolate("import-off"); // no [ai] config → AI is off
    let cur = make_project("import-off-cur", "Book", "c\n");
    let mut app = open_app(&cur);

    let notes = std::env::temp_dir().join(format!("quill-notes-{}-a.md", std::process::id()));
    fs::write(&notes, "# Mara\n\nA sailor with a locket.\n").unwrap();
    app.import_notes(notes.to_str().unwrap());

    let preserved = app
        .project
        .root
        .join("bible")
        .join("sources")
        .join(notes.file_name().unwrap());
    assert!(
        preserved.exists(),
        "the source is preserved even with AI off"
    );
    assert!(fs::read_to_string(&preserved).unwrap().contains("locket"));
    assert!(
        app.message.as_deref().unwrap_or("").contains("enable AI"),
        "tells the author AI is needed: {:?}",
        app.message
    );
    fs::remove_file(&notes).ok();
}

#[test]
fn importing_a_directory_preserves_all_markdown_collectively() {
    isolate("import-dir");
    let cur = make_project("import-dir-cur", "Book", "c\n");
    let mut app = open_app(&cur);

    let notesdir = std::env::temp_dir().join(format!("quill-notesdir-{}", std::process::id()));
    let _ = fs::remove_dir_all(&notesdir);
    fs::create_dir_all(&notesdir).unwrap();
    let notesdir = TempProject::new(notesdir);
    fs::write(notesdir.join("chars.md"), "# Mara\n").unwrap();
    fs::write(notesdir.join("world.md"), "# Saltmarsh\n").unwrap();
    fs::write(notesdir.join("ignore.txt"), "not markdown\n").unwrap();

    app.import_notes(notesdir.to_str().unwrap());

    let srcdir = app.project.root.join("bible").join("sources");
    assert!(
        srcdir.join("chars.md").exists() && srcdir.join("world.md").exists(),
        "every .md preserved"
    );
    assert!(
        !srcdir.join("ignore.txt").exists(),
        "non-markdown is skipped"
    );
}

#[test]
fn importing_a_second_source_with_a_colliding_name_keeps_both() {
    isolate("import-collide"); // AI off → sources preserved, no generation
    let cur = make_project("import-collide-cur", "Book", "c\n");
    let mut app = open_app(&cur);

    // Two separate folders each with a characters.md — keying only by filename
    // would let the second silently overwrite the first.
    let first = std::env::temp_dir().join(format!("quill-collide-{}-a", std::process::id()));
    let second = std::env::temp_dir().join(format!("quill-collide-{}-b", std::process::id()));
    for d in [&first, &second] {
        let _ = fs::remove_dir_all(d);
        fs::create_dir_all(d).unwrap();
    }
    fs::write(first.join("characters.md"), "# Mara\n\nfirst folder\n").unwrap();
    fs::write(second.join("characters.md"), "# Bram\n\nsecond folder\n").unwrap();

    app.import_notes(first.to_str().unwrap());
    app.import_notes(second.to_str().unwrap());

    let srcdir = app.project.root.join("bible").join("sources");
    assert_eq!(
        fs::read_to_string(srcdir.join("characters.md")).unwrap(),
        "# Mara\n\nfirst folder\n",
        "the first source is left intact"
    );
    assert_eq!(
        fs::read_to_string(srcdir.join("characters-1.md")).unwrap(),
        "# Bram\n\nsecond folder\n",
        "the colliding second source is preserved under a unique name"
    );

    let _ = fs::remove_dir_all(&first);
    let _ = fs::remove_dir_all(&second);
}

#[test]
fn importing_a_missing_path_reports_no_files() {
    isolate("import-missing");
    let cur = make_project("import-missing-cur", "Book", "c\n");
    let mut app = open_app(&cur);

    let missing = std::env::temp_dir().join(format!("quill-nope-{}-x.md", std::process::id()));
    let _ = fs::remove_file(&missing);
    app.import_notes(missing.to_str().unwrap());

    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No Markdown files"));
}

#[test]
fn applying_an_import_writes_the_bible_files() {
    isolate("import-apply");
    let cur = make_project("import-apply-cur", "Book", "c\n");
    let mut app = open_app(&cur);

    let imported = crate::ai::import::parse(
        r#"{
            "characters":[{"name":"Mara"}],
            "setting":{"categories":[{"name":"Magic","entries":[{"name":"Tidecalling","detail":"pull the sea"}]}],"gazetteer":["Saltmarsh"],"findings":[]},
            "timeline":{"events":[{"description":"Mara finds the locket","chapter":"One","story_order":1}],"findings":[]},
            "outline":{"outline":[{"chapter":"One","beats":["Mara arrives home"]}],"gaps":[]}
        }"#,
    )
    .unwrap();
    app.apply_import(Ok(imported));

    let bible = app.project.root.join("bible");
    assert!(
        bible.join("characters").join("mara.md").exists(),
        "character bible written"
    );
    assert!(
        bible.join("setting.md").exists(),
        "setting bible written (incl. magic system)"
    );
    assert!(bible.join("timeline.md").exists(), "timeline written");
    assert!(bible.join("outline.md").exists(), "outline written");
}

// --- New project from a folder of notes (#286) -----------------------------

/// A folder of Markdown notes (not a project), for the new-from-notes flow.
fn notes_folder(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-notesrc-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (name, body) in files {
        fs::write(dir.join(name), body).unwrap();
    }
    dir
}

#[test]
fn new_from_notes_creates_the_project_and_preserves_the_sources() {
    isolate("nfn"); // AI off → sources preserved, bible generation deferred
    let cur = make_project("nfn-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    let notes = notes_folder(
        "nfn",
        &[("chars.md", "# Mara\n"), ("world.md", "# Saltmarsh\n")],
    );
    let dest = books_dir("nfn-dest").join("From Notes");

    // Folder prompt → validates and asks for the project path.
    app.begin_new_from_notes(notes.to_str().unwrap());
    assert!(matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::NewFromNotesDest)));
    // Project-path prompt → create + import.
    app.finish_new_from_notes(dest.to_str().unwrap());

    assert_eq!(
        app.project.manifest.title, "From Notes",
        "the new project is created and opened"
    );
    let srcdir = dest.join("bible").join("sources");
    assert!(
        srcdir.join("chars.md").exists() && srcdir.join("world.md").exists(),
        "every note preserved in the new project"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("enable AI"),
        "AI off → bible generation deferred: {:?}",
        app.message
    );
}

#[test]
fn new_from_notes_rejects_a_folder_with_no_markdown() {
    isolate("nfn-empty");
    let cur = make_project("nfn-empty-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    let empty = notes_folder("nfn-empty", &[("notes.txt", "not markdown\n")]);
    app.begin_new_from_notes(empty.to_str().unwrap());

    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no project-path prompt for an empty folder"
    );
    assert_eq!(
        app.project.manifest.title, "Current",
        "the current project is untouched"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No Markdown notes"));
}

#[test]
fn new_from_notes_action_opens_the_folder_prompt() {
    isolate("nfn-action");
    let cur = make_project("nfn-action-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    app.run_action(crate::mode::Action::NewProjectFromNotes, false);
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::NewFromNotesSource))
    );
}

// --- Import an existing manuscript (#298) ---------------------------------

#[test]
fn importing_a_markdown_manuscript_creates_a_chaptered_project() {
    isolate("import-ms");
    let cur = make_project("import-ms-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // A Quill-export-shaped manuscript: title + author, two `##` chapters.
    let source = std::env::temp_dir().join(format!("quill-ms-{}.md", std::process::id()));
    let manuscript = "# Salt and Iron\n\nby J. Reader\n\n## One\n\nThe sea was *grey*.\n\n## Two\n\nAnd it **ended**.\n";
    fs::write(&source, manuscript).unwrap();
    let dest = books_dir("import-ms-dest").join("Salt and Iron");

    // File prompt → validates and asks for the project path.
    app.begin_import_manuscript(source.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ImportManuscriptDest))
    );
    // Project-path prompt → the import preview (#702), before anything is written.
    app.finish_import_manuscript(dest.to_str().unwrap());
    let Mode::ImportPreview(preview) = &app.mode else {
        panic!("expected the import preview before writing");
    };
    let previewed: Vec<&str> = preview.chapters.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        previewed,
        vec!["One", "Two"],
        "the preview lists the chapters (titles + count) before writing"
    );
    assert_eq!(preview.title.as_deref(), Some("Salt and Iron"));
    assert_eq!(preview.level, Some(2), "the guessed split level is shown");
    assert!(
        !Project::manifest_path(&dest).exists(),
        "nothing is written while the preview is up"
    );
    // Confirm the preview → create + split into chapters.
    app.confirm_import_preview();

    // A valid project: manifest title/author from the source, one file per chapter.
    assert_eq!(app.project.root, dest);
    assert_eq!(app.project.manifest.title, "Salt and Iron");
    assert_eq!(app.project.manifest.author.as_deref(), Some("J. Reader"));
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["One", "Two"],
        "split on chapter headings, titles carried over"
    );
    let ch1 = fs::read_to_string(dest.join(&app.project.manifest.chapters[0].file)).unwrap();
    assert_eq!(
        ch1, "The sea was *grey*.\n",
        "chapter body written, emphasis preserved"
    );

    // The original source is left untouched.
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        manuscript,
        "import copies in; the source is read-only"
    );
    let _ = fs::remove_file(&source);
}

#[test]
fn importing_a_docx_manuscript_creates_a_chaptered_project() {
    use crate::export::docx::{to_docx, Profile};
    use crate::export::{Chapter, Document, Part, PubMeta};
    isolate("import-docx");
    let cur = make_project("import-docx-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // Build a real .docx (Vellum profile → chapter titles as Heading 1).
    let doc = Document {
        title: "Salt and Iron".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![
                Chapter {
                    title: "One".into(),
                    epigraph: None,
                    body: "The sea was *grey*.".into(),
                    role: None,
                },
                Chapter {
                    title: "Two".into(),
                    epigraph: None,
                    body: "And it **ended**.".into(),
                    role: None,
                },
            ],
        }],
        back: vec![],
    };
    let source = std::env::temp_dir().join(format!("quill-ms-{}.docx", std::process::id()));
    fs::write(&source, to_docx(&doc, Profile::Vellum).unwrap()).unwrap();
    let original = fs::read(&source).unwrap();
    let dest = books_dir("import-docx-dest").join("Salt and Iron");

    app.begin_import_manuscript(source.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ImportManuscriptDest)),
        "a .docx is accepted"
    );
    app.finish_import_manuscript(dest.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::ImportPreview(_)),
        "the import preview (#702) is shown before writing"
    );
    app.confirm_import_preview(); // confirm the preview → write + open

    assert_eq!(app.project.root, dest);
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["One", "Two"],
        "split on Heading 1, titles carried over"
    );
    let ch1 = fs::read_to_string(dest.join(&app.project.manifest.chapters[0].file)).unwrap();
    assert_eq!(ch1, "The sea was *grey*.\n", "emphasis converted from runs");

    // The original .docx is read-only.
    assert_eq!(
        fs::read(&source).unwrap(),
        original,
        "the source is left untouched"
    );
    let _ = fs::remove_file(&source);
}

#[test]
fn importing_a_scriv_bundle_creates_a_chaptered_project() {
    use crate::export::scriv::write_scriv;
    use crate::export::{Chapter, Document, Part, PubMeta};
    isolate("import-scriv");
    let cur = make_project("import-scriv-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // A real .scriv bundle (the inverse of what we import): a part folder + chapters.
    let doc = Document {
        title: "Salt and Iron".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: Some("Part One".into()),
            chapters: vec![
                Chapter {
                    title: "One".into(),
                    epigraph: None,
                    body: "The sea was *grey*.".into(),
                    role: None,
                },
                Chapter {
                    title: "Two".into(),
                    epigraph: None,
                    body: "And it **ended**.".into(),
                    role: None,
                },
            ],
        }],
        back: vec![],
    };
    let source = books_dir("import-scriv-src").join("Salt and Iron.scriv");
    write_scriv(&doc, &source).unwrap();
    let dest = books_dir("import-scriv-dest").join("Salt and Iron");

    // A `.scriv` bundle directory is accepted.
    app.begin_import_manuscript(source.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ImportManuscriptDest)),
        "a .scriv bundle is accepted"
    );
    app.finish_import_manuscript(dest.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::ImportPreview(_)),
        "the import preview (#702) is shown before writing"
    );
    app.confirm_import_preview(); // confirm the preview → write + open

    assert_eq!(app.project.root, dest);
    assert_eq!(
        app.project.manifest.title, "Salt and Iron",
        "title from the bundle name"
    );
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["One", "Two"], "binder order, folder flattened");
    let ch1 = fs::read_to_string(dest.join(&app.project.manifest.chapters[0].file)).unwrap();
    assert_eq!(ch1, "The sea was *grey*.\n", "emphasis converted from RTF");

    // The original bundle still has its binder — read-only.
    assert!(
        source.join("Salt and Iron.scrivx").exists(),
        "the source bundle is untouched"
    );
}

#[test]
fn importing_rejects_an_unsupported_file() {
    isolate("import-ms-bad");
    let cur = make_project("import-ms-bad-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // A `.pdf` (an unsupported format) is rejected; `.txt` is now importable.
    let bad = std::env::temp_dir().join(format!("quill-ms-bad-{}.pdf", std::process::id()));
    fs::write(&bad, "not importable\n").unwrap();
    app.begin_import_manuscript(bad.to_str().unwrap());

    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no project-path prompt for an unsupported file"
    );
    assert_eq!(
        app.project.manifest.title, "Current",
        "the current project is untouched"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Not an importable manuscript"));
    let _ = fs::remove_file(&bad);
}

#[test]
fn importing_a_txt_manuscript_splits_into_chapters() {
    isolate("import-txt");
    let cur = make_project("import-txt-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // A plain-text manuscript with `Chapter N` heading lines.
    let source = std::env::temp_dir().join(format!("quill-ms-{}.txt", std::process::id()));
    let manuscript = "Chapter 1\n\nThe sea was grey.\n\nChapter 2\n\nAnd it ended.\n";
    fs::write(&source, manuscript).unwrap();
    let dest = books_dir("import-txt-dest").join("Grey Sea");

    app.begin_import_manuscript(source.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ImportManuscriptDest)),
        "a .txt is accepted"
    );
    app.finish_import_manuscript(dest.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::ImportPreview(_)),
        "the import preview (#702) is shown before writing"
    );
    app.confirm_import_preview(); // confirm the preview → write + open

    assert_eq!(app.project.root, dest);
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Chapter 1", "Chapter 2"], "split on Chapter N");
    let ch1 = fs::read_to_string(dest.join(&app.project.manifest.chapters[0].file)).unwrap();
    assert_eq!(ch1, "The sea was grey.\n", "chapter body written");

    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        manuscript,
        "the source is read-only"
    );
    let _ = fs::remove_file(&source);
}

#[test]
fn importing_a_folder_of_markdown_makes_one_chapter_per_file() {
    isolate("import-mdfolder");
    let cur = make_project("import-mdfolder-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // A folder of per-chapter `.md` files: filename order, `#` heading as title.
    let folder = std::env::temp_dir().join(format!("quill-msdir-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("02-two.md"), "# Two\n\nSecond.\n").unwrap();
    fs::write(folder.join("01-one.md"), "# One\n\nFirst.\n").unwrap();
    fs::write(folder.join("notes.txt"), "not a chapter\n").unwrap();
    let dest = books_dir("import-mdfolder-dest").join("From Folder");

    app.begin_import_manuscript(folder.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ImportManuscriptDest)),
        "a folder of .md is accepted"
    );
    app.finish_import_manuscript(dest.to_str().unwrap());
    assert!(
        matches!(&app.mode, Mode::ImportPreview(_)),
        "the import preview (#702) is shown before writing"
    );
    app.confirm_import_preview(); // confirm the preview → write + open

    assert_eq!(app.project.root, dest);
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["One", "Two"],
        "one chapter per .md, in filename order, titled by its heading"
    );
    let ch1 = fs::read_to_string(dest.join(&app.project.manifest.chapters[0].file)).unwrap();
    assert_eq!(ch1, "First.\n", "the heading is dropped from the body");

    let _ = fs::remove_dir_all(&folder);
}

#[test]
fn cancelling_the_import_preview_writes_nothing() {
    isolate("import-cancel");
    let cur = make_project("import-cancel-cur", "Current", "c\n");
    let mut app = open_app(&cur);
    let cur_root = app.project.root.clone();

    let source = std::env::temp_dir().join(format!("quill-cancel-{}.md", std::process::id()));
    fs::write(&source, "# One\n\nAlpha.\n\n# Two\n\nBeta.\n").unwrap();
    let dest = books_dir("import-cancel-dest").join("Book");

    app.begin_import_manuscript(source.to_str().unwrap());
    app.finish_import_manuscript(dest.to_str().unwrap());
    assert!(matches!(&app.mode, Mode::ImportPreview(_)), "preview shown");

    // Esc cancels: no project is created and the current one stays open.
    app.on_key_import_preview(key(KeyCode::Esc));
    assert!(
        !Project::manifest_path(&dest).exists(),
        "cancelling the preview writes nothing"
    );
    assert!(!dest.exists(), "the destination folder is never created");
    assert_eq!(app.project.root, cur_root, "the current project stays open");
    assert_eq!(app.message.as_deref(), Some("Import cancelled."));
    let _ = fs::remove_file(&source);
}

#[test]
fn confirming_the_import_preview_via_enter_writes_and_opens() {
    isolate("import-confirm");
    let cur = make_project("import-confirm-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    let source = std::env::temp_dir().join(format!("quill-confirm-{}.md", std::process::id()));
    fs::write(&source, "# One\n\nAlpha.\n\n# Two\n\nBeta.\n").unwrap();
    let dest = books_dir("import-confirm-dest").join("Book");

    app.begin_import_manuscript(source.to_str().unwrap());
    app.finish_import_manuscript(dest.to_str().unwrap());
    // Enter through the key handler confirms the import.
    app.on_key_import_preview(key(KeyCode::Enter));

    assert_eq!(app.project.root, dest, "the imported project is opened");
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["One", "Two"]);
    let _ = fs::remove_file(&source);
}

#[test]
fn re_splitting_the_preview_at_another_level_re_previews_without_writing() {
    isolate("import-relevel");
    let cur = make_project("import-relevel-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // A lone `#` title over three `##` chapters, each with a `###` scene. The
    // parser guesses level 2 (three chapters); re-splitting on level 3 yields the
    // three scene headings as chapters instead.
    let source = std::env::temp_dir().join(format!("quill-relevel-{}.md", std::process::id()));
    let manuscript = "# Book\n\n## One\n\n### Scene A\n\na\n\n## Two\n\n### Scene B\n\nb\n\n## Three\n\n### Scene C\n\nc\n";
    fs::write(&source, manuscript).unwrap();
    let dest = books_dir("import-relevel-dest").join("Book");

    app.begin_import_manuscript(source.to_str().unwrap());
    app.finish_import_manuscript(dest.to_str().unwrap());

    let Mode::ImportPreview(preview) = &app.mode else {
        panic!("preview expected");
    };
    assert_eq!(preview.level, Some(2), "guessed the lone-title/## shape");
    let at_two: Vec<&str> = preview.chapters.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(at_two, vec!["One", "Two", "Three"]);
    assert!(
        preview.levels.contains(&1) && preview.levels.contains(&2) && preview.levels.contains(&3),
        "the levels the manuscript uses are offered"
    );

    // Toggle to level 3 → the scene headings become chapters; nothing is written.
    app.on_key_import_preview(key(KeyCode::Char('3')));
    let Mode::ImportPreview(preview) = &app.mode else {
        panic!("still previewing after a re-split");
    };
    assert_eq!(preview.level, Some(3));
    let at_three: Vec<&str> = preview.chapters.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        at_three,
        vec!["Scene A", "Scene B", "Scene C"],
        "re-split on level 3"
    );
    assert!(
        !Project::manifest_path(&dest).exists(),
        "re-previewing writes nothing"
    );

    // Confirm at the re-chosen level → that split is what gets written.
    app.confirm_import_preview();
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Scene A", "Scene B", "Scene C"]);
    let _ = fs::remove_file(&source);
}

#[test]
fn import_manuscript_action_opens_the_file_prompt() {
    isolate("import-ms-action");
    let cur = make_project("import-ms-action-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    app.run_action(crate::mode::Action::ImportManuscript, false);
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ImportManuscriptSource))
    );
}

//! Import & setup-wizard flows (#282/#283/#286/#298): the new-project setup
//! wizard, importing a folder of notes into the bible, creating a new project
//! from a folder of notes, and importing an existing manuscript (Markdown,
//! `.docx`, or `.scriv`) into a chaptered project.

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
    // Project-path prompt → create + split into chapters.
    app.finish_import_manuscript(dest.to_str().unwrap());

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
    use crate::export::{Chapter, Document, Part};
    isolate("import-docx");
    let cur = make_project("import-docx-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // Build a real .docx (Vellum profile → chapter titles as Heading 1).
    let doc = Document {
        title: "Salt and Iron".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![
                Chapter {
                    title: "One".into(),
                    epigraph: None,
                    body: "The sea was *grey*.".into(),
                },
                Chapter {
                    title: "Two".into(),
                    epigraph: None,
                    body: "And it **ended**.".into(),
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
    use crate::export::{Chapter, Document, Part};
    isolate("import-scriv");
    let cur = make_project("import-scriv-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    // A real .scriv bundle (the inverse of what we import): a part folder + chapters.
    let doc = Document {
        title: "Salt and Iron".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: Some("Part One".into()),
            chapters: vec![
                Chapter {
                    title: "One".into(),
                    epigraph: None,
                    body: "The sea was *grey*.".into(),
                },
                Chapter {
                    title: "Two".into(),
                    epigraph: None,
                    body: "And it **ended**.".into(),
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
fn importing_rejects_a_non_markdown_file() {
    isolate("import-ms-bad");
    let cur = make_project("import-ms-bad-cur", "Current", "c\n");
    let mut app = open_app(&cur);

    let txt = std::env::temp_dir().join(format!("quill-ms-bad-{}.txt", std::process::id()));
    fs::write(&txt, "not markdown\n").unwrap();
    app.begin_import_manuscript(txt.to_str().unwrap());

    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no project-path prompt for a non-.md file"
    );
    assert_eq!(
        app.project.manifest.title, "Current",
        "the current project is untouched"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Not a Markdown"));
    let _ = fs::remove_file(&txt);
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

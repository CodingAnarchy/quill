use std::fs;

use quill_core::testutil::scratch_dir;
use quill_core::vcs;

use super::*;
use crate::ProjectForm;

#[test]
fn a_new_novel_opens_on_its_first_chapter_with_history() {
    let dir = scratch_dir("ffi-new", "novel");
    let path = create_project("The Lamp Room", ProjectForm::Novel, &dir.to_string_lossy())
        .expect("create");
    assert!(path.ends_with("The Lamp Room"), "{path}");

    let project = QuillProject::open(&path).expect("open");
    let info = project.info().expect("info");
    assert_eq!(info.title, "The Lamp Room");
    assert_eq!(info.form, ProjectForm::Novel);
    assert_eq!(
        project.chapters().expect("chapters").len(),
        1,
        "one chapter to type in"
    );

    let history = vcs::log(Path::new(&path), 10).expect("log");
    assert_eq!(history.len(), 1, "history from the start");
    assert!(vcs::is_auto_history(Path::new(&path)));
}

#[test]
fn a_new_short_story_opens_on_the_story() {
    let dir = scratch_dir("ffi-new", "story");
    let path = create_project("Low Tide", ProjectForm::ShortStory, &dir.to_string_lossy())
        .expect("create");
    let info = QuillProject::open(&path)
        .expect("open")
        .info()
        .expect("info");
    assert_eq!(info.form, ProjectForm::ShortStory);
    assert_eq!(info.title, "Low Tide");
    assert!(
        info.story_chapter.is_some(),
        "the app opens straight into the story"
    );
}

#[test]
fn a_title_keeps_its_slash_though_the_folder_cannot() {
    let dir = scratch_dir("ffi-new", "slash");
    let path =
        create_project("Either/Or", ProjectForm::Novel, &dir.to_string_lossy()).expect("create");
    assert!(path.ends_with("Either-Or"), "{path}");
    let info = QuillProject::open(&path)
        .expect("open")
        .info()
        .expect("info");
    assert_eq!(info.title, "Either/Or");
}

#[test]
fn an_existing_book_is_never_overwritten() {
    let dir = scratch_dir("ffi-new", "twice");
    let projects = dir.to_string_lossy();
    create_project("Twice", ProjectForm::Novel, &projects).expect("create");
    let err = create_project("Twice", ProjectForm::ShortStory, &projects).expect_err("refused");
    assert!(err.to_string().contains("already a book"), "{err}");
}

#[test]
fn a_title_is_needed_and_a_collection_is_not_made_empty() {
    let dir = scratch_dir("ffi-new", "refusals");
    let projects = dir.to_string_lossy();
    let err = create_project("  ", ProjectForm::Novel, &projects).expect_err("no title");
    assert!(err.to_string().contains("title"), "{err}");
    let err =
        create_project("Stories", ProjectForm::Collection, &projects).expect_err("collection");
    assert!(err.to_string().contains("short story"), "{err}");
    assert!(!dir.join("Stories").exists(), "nothing left behind");
}

#[test]
fn snapshots_go_into_a_history_quill_started_and_nowhere_else() {
    let dir = scratch_dir("ffi-new", "snapshot");
    let path = create_project("Tide", ProjectForm::Novel, &dir.to_string_lossy()).expect("create");
    let project = QuillProject::open(&path).expect("open");
    assert!(!project.snapshot().expect("clean"), "nothing changed yet");

    let chapter = fs::read_dir(&path)
        .expect("list")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "md"))
        .expect("the chapter file");
    fs::write(&chapter, "The sea came in.\n").expect("write");
    assert!(
        project.snapshot().expect("snapshot"),
        "the edit was snapshotted"
    );
    assert_eq!(vcs::log(Path::new(&path), 10).expect("log").len(), 2);

    // A repository the author made gets no snapshots.
    let hand = scratch_dir("ffi-new", "hand");
    fs::write(
        hand.join("book.toml"),
        "title = \"Hand\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .expect("manifest");
    fs::write(hand.join("one.md"), "Text.\n").expect("chapter");
    vcs::init(&hand).expect("init");
    let project = QuillProject::open(&hand.to_string_lossy()).expect("open");
    assert!(!project.start_history().expect("start"), "left alone");
    assert!(!project.snapshot().expect("snapshot"));
    assert!(vcs::log(&hand, 10).expect("log").is_empty());
}

#[test]
fn opening_a_book_without_history_starts_one() {
    let dir = scratch_dir("ffi-new", "sample");
    fs::write(
        dir.join("book.toml"),
        "title = \"Sample\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .expect("manifest");
    fs::write(dir.join("one.md"), "Text.\n").expect("chapter");
    let project = QuillProject::open(&dir.to_string_lossy()).expect("open");
    assert!(project.start_history().expect("start"), "started");
    assert!(!project.start_history().expect("again"), "only once");
}

#[test]
fn a_chapters_versions_can_be_listed_and_read_back() {
    let dir = scratch_dir("ffi-versions", "read");
    let path = create_project("Tide", ProjectForm::Novel, &dir.to_string_lossy()).expect("create");
    let project = QuillProject::open(&path).expect("open");
    let file = Path::new(&path).join(project.lock().get().expect("open").chapter_file(0));
    fs::write(&file, "---\nstatus: draft\n---\nThe sea came in.\n").expect("write");
    project.snapshot().expect("snapshot");
    fs::write(&file, "---\nstatus: draft\n---\nThe sea went out.\n").expect("write");
    project.snapshot().expect("snapshot");

    let versions = project.chapter_versions(0).expect("versions");
    assert_eq!(versions.len(), 3, "the start and two snapshots");
    assert!(versions[0].summary.starts_with("Snapshot, "));
    assert_eq!(
        project.chapter_text_at(0, &versions[1].rev).expect("read"),
        "The sea came in.\n",
        "the prose as it was, without its front matter"
    );
    assert!(project.chapter_versions(9).is_err(), "no such chapter");
}

#[test]
fn a_book_without_a_history_has_no_versions() {
    let dir = scratch_dir("ffi-versions", "none");
    fs::write(
        dir.join("book.toml"),
        "title = \"Bare\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .expect("manifest");
    fs::write(dir.join("one.md"), "Text.\n").expect("chapter");
    let project = QuillProject::open(&dir.to_string_lossy()).expect("open");
    assert!(project.chapter_versions(0).expect("versions").is_empty());
}

#[test]
fn comparing_versions_marks_what_changed() {
    let runs = compare_versions("The sea came in.", "The sea went out.");
    let kinds: Vec<ChangeKind> = runs.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [ChangeKind::Same, ChangeKind::Removed, ChangeKind::Added]
    );
    assert_eq!(runs[1].text, "came in.");
    assert_eq!(runs[2].text, "went out.");
}

#[test]
fn a_chapter_changed_elsewhere_is_noticed_and_its_version_kept() {
    let dir = scratch_dir("ffi-synced", "changed");
    let path = create_project("Tide", ProjectForm::Novel, &dir.to_string_lossy()).expect("create");
    let project = QuillProject::open(&path).expect("open");
    let session = project.clone().edit_chapter(0).expect("edit");
    assert!(!session.changed_on_disk(), "as opened");

    let file = Path::new(&path).join(project.lock().get().expect("open").chapter_file(0));
    fs::write(&file, "The Mac's words.\n").expect("another device writes");
    assert!(session.changed_on_disk());
    let copy = session.keep_disk_version().expect("keep");
    assert!(copy.ends_with(" 2.md"), "{copy}");
    let copies = project.conflict_copies().expect("copies");
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].copy, copy);
}

#[test]
fn a_snapshot_names_the_device_it_came_from() {
    let dir = scratch_dir("ffi-synced", "device");
    let path = create_project("Tide", ProjectForm::Novel, &dir.to_string_lossy()).expect("create");
    let project = QuillProject::open(&path).expect("open");
    let file = Path::new(&path).join(project.lock().get().expect("open").chapter_file(0));
    fs::write(&file, "Words.\n").expect("write");
    assert!(project.snapshot_from("iPhone").expect("snapshot"));
    let versions = project.chapter_versions(0).expect("versions");
    assert!(
        versions[0].summary.starts_with("Snapshot on iPhone, "),
        "{}",
        versions[0].summary
    );
    assert!(!project.is_synced().expect("synced"), "a local book");
}

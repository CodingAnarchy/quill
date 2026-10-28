use std::fs;
use std::path::PathBuf;

use quill_core::testutil::scratch_dir;
use quill_core::vcs;

use super::*;

/// A bare "remote" holding a two-chapter novel, plus the desktop's working
/// copy that pushed it.
fn remote_with_novel(name: &str) -> (PathBuf, PathBuf) {
    let remote = scratch_dir("ffi-git", &format!("{name}-remote"));
    git2::Repository::init_bare(&remote).unwrap();
    let desk = scratch_dir("ffi-git", &format!("{name}-desk"));
    vcs::init(&desk).unwrap();
    fs::write(
        desk.join("book.toml"),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(desk.join("one.md"), "First words.\n").unwrap();
    fs::write(desk.join("two.md"), "Second.\n").unwrap();
    vcs::commit(&desk, "start").unwrap();
    vcs::set_remote(&desk, remote.to_str().unwrap()).unwrap();
    vcs::push(&desk).unwrap();
    (remote, desk)
}

#[test]
fn only_https_remotes_are_accepted() {
    let dir = scratch_dir("ffi-git", "https-only");
    let err = clone_project("git@github.com:me/novel.git", &dir.to_string_lossy()).unwrap_err();
    assert!(err.to_string().contains("https://"), "{err}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_destination_is_named_for_the_repository() {
    assert_eq!(
        clone_destination("https://github.com/me/my-novel.git", "/p"),
        "/p/my-novel"
    );
}

#[test]
fn pull_reopens_the_project_with_the_desktops_edits() {
    let (remote, desk) = remote_with_novel("pull");
    // The phone's copy: a clone of the remote, opened through the FFI.
    let phone_parent = scratch_dir("ffi-git", "pull-phone");
    let phone = phone_parent.join("book");
    vcs::clone(remote.to_str().unwrap(), &phone).unwrap();
    let project = QuillProject::open(&phone.to_string_lossy()).unwrap();
    assert_eq!(project.chapters().unwrap()[0].words, 2);

    // The desktop edits chapter one and pushes.
    fs::write(desk.join("one.md"), "First words, and then many more.\n").unwrap();
    vcs::commit(&desk, "more").unwrap();
    vcs::push(&desk).unwrap();

    let message = Arc::clone(&project).pull().unwrap();
    assert!(message.contains("Pulled"), "{message}");
    assert_eq!(
        project.chapter_text(0).unwrap(),
        "First words, and then many more.\n"
    );
    // The cached count is current: the project was reopened after the pull.
    assert_eq!(project.chapters().unwrap()[0].words, 6);
    assert_eq!(project.remote_url().unwrap().as_deref(), remote.to_str());

    for d in [&remote, &desk, &phone_parent] {
        fs::remove_dir_all(d).ok();
    }
}

#[test]
fn sync_sends_phone_edits_and_reports_a_diversion() {
    let (remote, desk) = remote_with_novel("sync");
    let phone_parent = scratch_dir("ffi-git", "sync-phone");
    let phone = phone_parent.join("book");
    vcs::clone(remote.to_str().unwrap(), &phone).unwrap();
    let project = QuillProject::open(&phone.to_string_lossy()).unwrap();

    // A phone edit to chapter two goes out.
    let session = Arc::clone(&project).edit_chapter(1).unwrap();
    session.apply_edit(0, 6, "Third");
    session.save().unwrap();
    let report = Arc::clone(&project).sync("Edits on iPhone").unwrap();
    assert_eq!(report.draft, None);
    assert!(
        report.message.contains("sent your edits"),
        "{}",
        report.message
    );
    vcs::sync(&desk).unwrap();
    assert_eq!(fs::read_to_string(desk.join("two.md")).unwrap(), "Third.\n");

    // Both edit chapter one: the phone's work is parked on a draft.
    fs::write(desk.join("one.md"), "Desk words.\n").unwrap();
    vcs::commit(&desk, "desk").unwrap();
    vcs::push(&desk).unwrap();
    let session = Arc::clone(&project).edit_chapter(0).unwrap();
    session.apply_edit(0, 5, "Phone");
    session.save().unwrap();
    let report = Arc::clone(&project).sync("Edits on iPhone").unwrap();
    let draft = report.draft.expect("diverted");
    assert!(report.message.contains(&draft), "{}", report.message);
    assert!(report.reload);
    assert_eq!(project.chapter_text(0).unwrap(), "Desk words.\n");
    assert_eq!(project.chapters().unwrap()[0].words, 2);

    for d in [&remote, &desk, &phone_parent] {
        fs::remove_dir_all(d).ok();
    }
}

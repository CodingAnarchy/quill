//! Cloning a project onto a second device (#1231): the clone lands whole or
//! leaves nothing behind. A bare repo stands in for the hosted remote.

use std::fs;
use std::path::Path;

use super::super::remote::classify;
use super::temp_dir;
use crate::vcs::*;

/// A bare remote holding a one-chapter Quill project.
fn remote_with_project(name: &str) -> std::path::PathBuf {
    let remote = temp_dir(&format!("{name}-bare"));
    git2::Repository::init_bare(&remote).unwrap();
    let work = temp_dir(&format!("{name}-work"));
    init(&work).unwrap();
    fs::write(
        work.join("book.toml"),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(work.join("one.md"), "Draft.\n").unwrap();
    commit(&work, "first").unwrap();
    set_remote(&work, remote.to_str().unwrap()).unwrap();
    push(&work).unwrap();
    fs::remove_dir_all(&work).ok();
    remote
}

/// Nothing named like `dest` (the clone or its staging folder) is left.
fn nothing_left(dest: &Path) -> bool {
    let parent = dest.parent().unwrap();
    let stem = dest.file_name().unwrap().to_string_lossy().into_owned();
    !fs::read_dir(parent)
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().contains(&stem))
}

#[test]
fn clones_a_project_and_pulls_later_edits() {
    let remote = remote_with_project("clone-ok");
    let parent = temp_dir("clone-ok-dest");
    let dest = parent.join("book");

    clone(remote.to_str().unwrap(), &dest).unwrap();
    assert_eq!(fs::read_to_string(dest.join("one.md")).unwrap(), "Draft.\n");
    assert!(!parent.join(".book.cloning").exists());

    // An edit pushed from elsewhere reaches the clone on its next pull.
    let other = temp_dir("clone-ok-other");
    fs::remove_dir_all(&other).ok();
    git2::Repository::clone(remote.to_str().unwrap(), &other).unwrap();
    fs::write(other.join("one.md"), "Revised.\n").unwrap();
    commit(&other, "revise").unwrap();
    push(&other).unwrap();
    pull(&dest).unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("one.md")).unwrap(),
        "Revised.\n"
    );

    for d in [&remote, &parent, &other] {
        fs::remove_dir_all(d).ok();
    }
}

#[test]
fn a_repository_without_a_manifest_leaves_nothing_behind() {
    let remote = temp_dir("clone-notquill-bare");
    git2::Repository::init_bare(&remote).unwrap();
    let work = temp_dir("clone-notquill-work");
    init(&work).unwrap();
    fs::write(work.join("README.md"), "Not a novel.\n").unwrap();
    commit(&work, "first").unwrap();
    set_remote(&work, remote.to_str().unwrap()).unwrap();
    push(&work).unwrap();

    let parent = temp_dir("clone-notquill-dest");
    let dest = parent.join("thing");
    let err = clone(remote.to_str().unwrap(), &dest).unwrap_err();
    assert!(err.contains("isn't a Quill project"), "{err}");
    assert!(nothing_left(&dest));

    for d in [&remote, &work, &parent] {
        fs::remove_dir_all(d).ok();
    }
}

#[test]
fn an_unreachable_remote_leaves_nothing_behind() {
    let parent = temp_dir("clone-missing");
    let dest = parent.join("book");
    let err = clone(parent.join("no-such-remote").to_str().unwrap(), &dest).unwrap_err();
    assert!(!err.is_empty());
    assert!(nothing_left(&dest));
    fs::remove_dir_all(&parent).ok();
}

#[test]
fn refuses_to_clone_over_an_existing_folder() {
    let remote = remote_with_project("clone-exists");
    let parent = temp_dir("clone-exists-dest");
    let dest = parent.join("book");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("keep.md"), "mine\n").unwrap();
    let err = clone(remote.to_str().unwrap(), &dest).unwrap_err();
    assert!(err.contains("already a project"), "{err}");
    assert_eq!(fs::read_to_string(dest.join("keep.md")).unwrap(), "mine\n");
    for d in [&remote, &parent] {
        fs::remove_dir_all(d).ok();
    }
}

#[test]
fn editor_state_stays_out_of_a_repo_set_up_without_quills_gitignore() {
    // A repository made with plain git: no `.gitignore` naming `.quill/`.
    let dir = temp_dir("clone-exclude");
    let repo = git2::Repository::init(&dir).unwrap();
    drop(repo);
    fs::write(dir.join("one.md"), "Draft.\n").unwrap();
    commit(&dir, "first").unwrap();

    exclude_editor_state(&dir).unwrap();
    exclude_editor_state(&dir).unwrap(); // idempotent
    let exclude = fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
    assert_eq!(exclude.matches("/.quill/").count(), 1, "{exclude}");

    // The lock and caches Quill writes on open no longer count as changes.
    fs::create_dir_all(dir.join(".quill")).unwrap();
    fs::write(dir.join(".quill/lock"), "123").unwrap();
    assert_eq!(commit(&dir, "nothing new").unwrap(), None);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn folder_names_come_from_the_repository_name() {
    assert_eq!(
        folder_name("https://github.com/me/my-novel.git"),
        "my-novel"
    );
    assert_eq!(folder_name("https://github.com/me/my-novel/"), "my-novel");
    assert_eq!(folder_name("git@github.com:me/novel.git"), "novel");
    assert_eq!(folder_name("novel"), "novel");
    assert_eq!(folder_name(""), "project");
}

#[test]
fn a_supplied_token_changes_the_auth_advice() {
    let auth = classify(&git2::Error::new(
        git2::ErrorCode::Auth,
        git2::ErrorClass::Http,
        "authentication required",
    ));
    assert!(auth.message_for(false).contains("SSH key"));
    let phone = auth.message_for(true);
    assert!(phone.contains("access token"), "{phone}");
    assert!(!phone.contains("SSH"), "{phone}");
}

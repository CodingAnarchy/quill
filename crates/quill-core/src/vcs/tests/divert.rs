//! Phone-style sync (#1233): commit, sync, and push, or park conflicting work
//! on a `phone/<date>` draft for the desktop to merge. Two working copies (the
//! desk and the phone) share a bare repo standing in for the remote.

use std::fs;
use std::path::{Path, PathBuf};

use super::temp_dir;
use crate::vcs::*;

struct Devices {
    remote: PathBuf,
    desk: PathBuf,
    phone: PathBuf,
}

impl Devices {
    /// A remote with a two-chapter project, cloned onto the phone.
    fn new(name: &str) -> Self {
        let remote = temp_dir(&format!("{name}-remote"));
        git2::Repository::init_bare(&remote).unwrap();
        let desk = temp_dir(&format!("{name}-desk"));
        init(&desk).unwrap();
        fs::write(desk.join("book.toml"), "title = \"Book\"\n").unwrap();
        fs::write(desk.join("one.md"), "One.\n").unwrap();
        fs::write(desk.join("two.md"), "Two.\n").unwrap();
        commit(&desk, "start").unwrap();
        set_remote(&desk, remote.to_str().unwrap()).unwrap();
        push(&desk).unwrap();
        let phone = temp_dir(&format!("{name}-phone")).join("book");
        clone(remote.to_str().unwrap(), &phone).unwrap();
        Self {
            remote,
            desk,
            phone,
        }
    }

    fn desk_writes(&self, file: &str, text: &str) {
        fs::write(self.desk.join(file), text).unwrap();
        commit(&self.desk, "desk edit").unwrap();
        push(&self.desk).unwrap();
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        for d in [&self.remote, &self.desk, self.phone.parent().unwrap()] {
            fs::remove_dir_all(d).ok();
        }
    }
}

fn read(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap()
}

fn phone_sync(d: &Devices) -> DeviceSync {
    sync_or_divert(&d.phone, "Edits on iPhone").unwrap()
}

#[test]
fn edits_to_different_chapters_merge_both_ways() {
    let d = Devices::new("divert-clean");
    d.desk_writes("one.md", "One, from the desk.\n");
    fs::write(d.phone.join("two.md"), "Two, from the phone.\n").unwrap();

    let out = phone_sync(&d);
    assert!(
        matches!(out, DeviceSync::Synced { reload: true, .. }),
        "{out:?}"
    );
    assert_eq!(read(&d.phone, "one.md"), "One, from the desk.\n");
    assert_eq!(read(&d.phone, "two.md"), "Two, from the phone.\n");

    // The desk's next sync brings in the phone's chapter.
    sync(&d.desk).unwrap();
    assert_eq!(read(&d.desk, "two.md"), "Two, from the phone.\n");
    // The phone's commit says where it came from.
    let messages: Vec<String> = log(&d.desk, 10)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("on iPhone")),
        "{messages:?}"
    );
}

#[test]
fn a_conflict_parks_the_phones_work_on_a_draft() {
    let d = Devices::new("divert-conflict");
    d.desk_writes("one.md", "One, the desk's way.\n");
    fs::write(d.phone.join("one.md"), "One, the phone's way.\n").unwrap();

    let out = phone_sync(&d);
    let DeviceSync::Diverted { draft } = out else {
        panic!("expected a diversion, got {out:?}");
    };
    assert!(draft.starts_with("phone/"), "{draft}");
    // The phone is back on the remote's text, with nothing uncommitted.
    assert_eq!(read(&d.phone, "one.md"), "One, the desk's way.\n");
    assert_eq!(current_branch(&d.phone).unwrap(), "master");
    // The phone's version is on the draft, on the remote.
    let remote = git2::Repository::open_bare(&d.remote).unwrap();
    let parked = remote
        .find_branch(&draft, git2::BranchType::Local)
        .unwrap()
        .get()
        .peel_to_tree()
        .unwrap();
    let blob = parked
        .get_path(Path::new("one.md"))
        .unwrap()
        .to_object(&remote)
        .unwrap();
    assert_eq!(
        blob.as_blob().unwrap().content(),
        b"One, the phone's way.\n"
    );

    // The desk's sync lists the draft (and says so), and merging it surfaces
    // the conflict for the resolver instead of picking a side.
    let desk = sync(&d.desk).unwrap();
    assert!(desk.summary.contains(&draft), "{}", desk.summary);
    assert!(list_branches(&d.desk).unwrap().contains(&draft));
    match merge_branch(&d.desk, "master", &draft).unwrap() {
        MergeOutcome::Conflicts { conflicts } => {
            assert_eq!(conflicts[0].path, "one.md");
            assert_eq!(conflicts[0].remote, "One, the phone's way.\n");
        }
        MergeOutcome::Clean { .. } => panic!("the conflict must reach the resolver"),
    }
}

#[test]
fn a_second_conflict_the_same_day_gets_its_own_draft() {
    let d = Devices::new("divert-twice");
    for n in 1..=2 {
        d.desk_writes("one.md", &format!("Desk {n}.\n"));
        fs::write(d.phone.join("one.md"), format!("Phone {n}.\n")).unwrap();
        phone_sync(&d);
    }
    let drafts: Vec<String> = list_branches(&d.phone)
        .unwrap()
        .into_iter()
        .filter(|b| b.starts_with("phone/"))
        .collect();
    assert_eq!(drafts.len(), 2, "{drafts:?}");
    assert!(drafts[1].ends_with("-2"), "{drafts:?}");
}

#[test]
fn offline_work_is_kept_and_goes_out_next_time() {
    let d = Devices::new("divert-offline");
    fs::write(d.phone.join("two.md"), "Written on the train.\n").unwrap();

    // The remote is unreachable: the sync fails, but the edit is committed.
    let away = d.remote.with_extension("away");
    fs::rename(&d.remote, &away).unwrap();
    assert!(sync_or_divert(&d.phone, "Edits on iPhone").is_err());
    assert_eq!(status(&d.phone).unwrap().count(), 0, "committed, not lost");

    fs::rename(&away, &d.remote).unwrap();
    let out = phone_sync(&d);
    assert!(matches!(out, DeviceSync::Synced { .. }), "{out:?}");
    sync(&d.desk).unwrap();
    assert_eq!(read(&d.desk, "two.md"), "Written on the train.\n");
}

#[test]
fn after_the_desk_merges_the_draft_the_phone_gets_the_merged_text() {
    let d = Devices::new("divert-resolved");
    d.desk_writes("one.md", "Desk.\n");
    fs::write(d.phone.join("one.md"), "Phone.\n").unwrap();
    let DeviceSync::Diverted { draft } = phone_sync(&d) else {
        panic!("expected a diversion");
    };

    // The desk merges the draft by hand (the resolver's job) and pushes.
    sync(&d.desk).unwrap();
    let MergeOutcome::Conflicts { .. } = merge_branch(&d.desk, "master", &draft).unwrap() else {
        panic!("expected a conflict");
    };
    let mut resolved = std::collections::BTreeMap::new();
    resolved.insert("one.md".to_string(), "Desk, then phone.\n".to_string());
    finalize_branch_merge(&d.desk, "master", &draft, &resolved).unwrap();
    push(&d.desk).unwrap();

    let out = phone_sync(&d);
    assert!(
        matches!(out, DeviceSync::Synced { reload: true, .. }),
        "{out:?}"
    );
    assert_eq!(read(&d.phone, "one.md"), "Desk, then phone.\n");
}

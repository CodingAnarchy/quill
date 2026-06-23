//! The recent-projects list (#270): the projects the author has opened, newest
//! first, persisted in the global config directory (`~/.config/quill`) — never
//! in any one project, since it spans them. The project browser opens to this
//! list so switching between books is a keystroke, not a relaunch.
//!
//! Best-effort and non-critical: a missing or unreadable list just means an
//! empty browser, never an error. Entries whose `book.toml` has since vanished
//! are pruned when the list is read, so the browser never offers a dead project.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config;
use crate::project::Project;

/// How many projects to remember. Old enough to cover any working set; small
/// enough that the browser is a glance, not a scroll.
const MAX: usize = 20;

const FILE: &str = "recent-projects.json";

/// One remembered project: where it lives and its title at the time it was
/// opened (cached so the browser needn't read every manifest to draw the list).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RecentProject {
    pub path: PathBuf,
    pub title: String,
}

/// The recent projects, newest first, with any whose project no longer exists
/// pruned out.
pub fn list() -> Vec<RecentProject> {
    config::config_dir().map(|d| list_in(&d)).unwrap_or_default()
}

/// Record `root` (with display `title`) as the most-recently-opened project:
/// move it to the front, de-duplicated by path, capped to [`MAX`]. Best-effort.
pub fn record(root: &Path, title: &str) {
    if let Some(dir) = config::config_dir() {
        record_in(&dir, root, title);
    }
}

// --- Internals, parameterized by the config directory so they're testable
// without touching the real `~/.config` --------------------------------------

fn store_path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

fn list_in(config_dir: &Path) -> Vec<RecentProject> {
    let text = std::fs::read_to_string(store_path(config_dir)).unwrap_or_default();
    let mut items: Vec<RecentProject> = serde_json::from_str(&text).unwrap_or_default();
    items.retain(|r| Project::manifest_path(&r.path).exists());
    items
}

fn record_in(config_dir: &Path, root: &Path, title: &str) {
    // Canonicalize so the same project reached by different paths (".", a
    // relative dir, a symlink) is recognized as one entry.
    let path = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let entry = RecentProject { path, title: title.to_string() };
    let updated = promote(list_in(config_dir), entry);

    let _ = std::fs::create_dir_all(config_dir);
    if let Ok(json) = serde_json::to_string_pretty(&updated) {
        let _ = std::fs::write(store_path(config_dir), json);
    }
}

/// Put `entry` at the front of `items`, drop any earlier copy of the same path,
/// and cap the result to [`MAX`]. Pure, so the ordering rules are unit-tested.
fn promote(mut items: Vec<RecentProject>, entry: RecentProject) -> Vec<RecentProject> {
    items.retain(|r| r.path != entry.path);
    items.insert(0, entry);
    items.truncate(MAX);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rp(path: &str, title: &str) -> RecentProject {
        RecentProject { path: PathBuf::from(path), title: title.to_string() }
    }

    #[test]
    fn promote_moves_a_repeat_to_the_front_without_duplicating() {
        let items = vec![rp("/a", "A"), rp("/b", "B"), rp("/c", "C")];
        let out = promote(items, rp("/b", "B2"));
        assert_eq!(out.iter().map(|r| r.path.to_str().unwrap()).collect::<Vec<_>>(), ["/b", "/a", "/c"]);
        assert_eq!(out[0].title, "B2", "the title refreshes to the latest open");
    }

    #[test]
    fn promote_caps_the_list() {
        let mut items = Vec::new();
        for i in 0..MAX {
            items.push(rp(&format!("/p{i}"), "x"));
        }
        let out = promote(items, rp("/new", "new"));
        assert_eq!(out.len(), MAX);
        assert_eq!(out[0].path.as_path(), Path::new("/new"));
        let oldest = format!("/p{}", MAX - 1);
        assert!(!out.iter().any(|r| r.path.as_path() == Path::new(&oldest)), "the oldest fell off");
    }

    #[test]
    fn record_then_list_round_trips_through_the_store() {
        let dir = std::env::temp_dir().join(format!("quill-recent-{}-rt", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // A real project so it survives the existence prune.
        let proj = dir.join("book");
        Project::create(&proj).unwrap();

        record_in(&dir, &proj, "My Book");
        let listed = list_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "My Book");
        assert_eq!(listed[0].path, proj.canonicalize().unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_prunes_a_project_that_no_longer_exists() {
        let dir = std::env::temp_dir().join(format!("quill-recent-{}-prune", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A recorded entry pointing at a project that isn't there.
        let json = serde_json::to_string(&vec![rp("/nowhere/ghost", "Ghost")]).unwrap();
        std::fs::write(store_path(&dir), json).unwrap();

        assert!(list_in(&dir).is_empty(), "a vanished project is pruned from the list");
        std::fs::remove_dir_all(&dir).ok();
    }
}

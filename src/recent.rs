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
use crate::project::{Manifest, Project};

/// How many projects to remember. Old enough to cover any working set; small
/// enough that the browser is a glance, not a scroll.
const MAX: usize = 20;

/// Cap on directory entries examined when scanning a projects-home folder, so a
/// pathologically large directory can never hang the browser (#272).
const SCAN_LIMIT: usize = 1000;

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
    config::config_dir()
        .map(|d| list_in(&d))
        .unwrap_or_default()
}

/// Record `root` (with display `title`) as the most-recently-opened project:
/// move it to the front, de-duplicated by path, capped to [`MAX`]. Best-effort.
pub fn record(root: &Path, title: &str) {
    if let Some(dir) = config::config_dir() {
        record_in(&dir, root, title);
    }
}

/// Every project directly under `dir` (#272): a shallow, bounded scan — each
/// immediate subdirectory holding a `book.toml` is a project, titled from its
/// manifest (falling back to the folder name). Sorted by title. An unreadable
/// `dir` yields an empty list rather than an error. Never recurses, and examines
/// at most [`SCAN_LIMIT`] entries, so it stays responsive on a huge folder.
pub fn scan(dir: &Path) -> Vec<RecentProject> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<RecentProject> = read
        .flatten()
        .take(SCAN_LIMIT)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|path| {
            // Title from the manifest alone — never read the chapters, so a
            // library of many books still scans in a blink.
            let text = std::fs::read_to_string(Project::manifest_path(&path)).ok()?;
            let title = toml::from_str::<Manifest>(&text)
                .map(|m| m.title)
                .ok()
                .filter(|t| !t.is_empty())
                .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let path = path.canonicalize().unwrap_or(path);
            Some(RecentProject { path, title })
        })
        .collect();
    found.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    found
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
    let entry = RecentProject {
        path,
        title: title.to_string(),
    };
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
        RecentProject {
            path: PathBuf::from(path),
            title: title.to_string(),
        }
    }

    #[test]
    fn promote_moves_a_repeat_to_the_front_without_duplicating() {
        let items = vec![rp("/a", "A"), rp("/b", "B"), rp("/c", "C")];
        let out = promote(items, rp("/b", "B2"));
        assert_eq!(
            out.iter()
                .map(|r| r.path.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["/b", "/a", "/c"]
        );
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
        assert!(
            !out.iter().any(|r| r.path.as_path() == Path::new(&oldest)),
            "the oldest fell off"
        );
    }

    #[test]
    fn record_then_list_round_trips_through_the_store() {
        let dir = crate::testutil::scratch_dir("recent", "rt");
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
        let dir = crate::testutil::scratch_dir("recent", "prune");
        // A recorded entry pointing at a project that isn't there.
        let json = serde_json::to_string(&vec![rp("/nowhere/ghost", "Ghost")]).unwrap();
        std::fs::write(store_path(&dir), json).unwrap();

        assert!(
            list_in(&dir).is_empty(),
            "a vanished project is pruned from the list"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_finds_projects_and_skips_plain_folders() {
        let home = crate::testutil::scratch_dir("home", "scan");
        Project::create(home.join("alpha")).unwrap();
        Project::create(home.join("beta")).unwrap();
        std::fs::create_dir_all(home.join("not-a-project")).unwrap();
        // Give beta a real title so the manifest title (not the folder name) is used.
        std::fs::write(
            Project::manifest_path(&home.join("beta")),
            "title = \"Zebra\"\n",
        )
        .unwrap();

        let found = scan(&home);
        let titles: Vec<_> = found.iter().map(|r| r.title.as_str()).collect();
        // Sorted by title: "Chapter…"? alpha keeps its created title; check membership + sort.
        assert_eq!(
            found.len(),
            2,
            "only the two projects, not the plain folder: {titles:?}"
        );
        assert!(
            titles.contains(&"Zebra"),
            "title comes from the manifest: {titles:?}"
        );
        assert!(
            titles
                .windows(2)
                .all(|w| w[0].to_lowercase() <= w[1].to_lowercase()),
            "sorted by title"
        );

        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn scan_of_a_missing_dir_is_empty() {
        // Deliberately a path that does NOT exist — not `scratch_dir`, which would
        // create it and turn this into the empty-dir case instead of the missing one.
        let missing =
            std::env::temp_dir().join(format!("quill-home-{}-missing", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        assert!(scan(&missing).is_empty());
    }
}

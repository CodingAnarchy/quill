//! The recent-projects list (#270): the projects the author has opened, newest
//! first, persisted in the global config directory (`~/.config/quill`) — never
//! in any one project, since it spans them. The project browser opens to this
//! list so switching between books is a keystroke, not a relaunch.
//!
//! Best-effort and non-critical: a missing or unreadable list just means an
//! empty browser, never an error. Entries whose `book.toml` has since vanished
//! are pruned when the list is read, so the browser never offers a dead project.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;
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

/// Every openable project, newest-recent first then the rest of the library: the
/// recent list (#270) followed by every project under `home` (#272) that isn't
/// already a recent, de-duplicated by path. `home` is the configured projects
/// home, or `None` when unset — then this is just the recent list. The single
/// source for both the in-editor browser (Ctrl-B) and the startup chooser (#526),
/// so the two never offer different libraries.
pub fn openable(home: Option<&Path>) -> Vec<RecentProject> {
    let mut projects = list();
    if let Some(home) = home {
        let known: HashSet<PathBuf> = projects.iter().map(|p| p.path.clone()).collect();
        projects.extend(scan(home).into_iter().filter(|p| !known.contains(&p.path)));
    }
    projects
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
    found.sort_by_key(|a| a.title.to_lowercase());
    found
}

// --- Internals, parameterized by the config directory so they're testable
// without touching the real `~/.config` --------------------------------------

fn store_path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// Read the stored list. A missing file is the legitimate empty case; any other
/// read error is surfaced as `Err` so the recorder can abort its rewrite rather
/// than overwrite a list it couldn't read with just the one new entry (#565).
fn read_store(config_dir: &Path) -> std::io::Result<Vec<RecentProject>> {
    let text = match std::fs::read_to_string(store_path(config_dir)) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut items: Vec<RecentProject> = serde_json::from_str(&text).unwrap_or_default();
    items.retain(|r| Project::manifest_path(&r.path).exists());
    Ok(items)
}

fn list_in(config_dir: &Path) -> Vec<RecentProject> {
    // Best-effort for the browser: an unreadable list is an empty one.
    read_store(config_dir).unwrap_or_default()
}

fn record_in(config_dir: &Path, root: &Path, title: &str) {
    // A failed read of the existing list aborts: rewriting from an empty base
    // would discard every other remembered project, keeping only this one (#565).
    let Ok(existing) = read_store(config_dir) else {
        return;
    };
    // Canonicalize so the same project reached by different paths (".", a
    // relative dir, a symlink) is recognized as one entry.
    let path = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let entry = RecentProject {
        path,
        title: title.to_string(),
    };
    let updated = promote(existing, entry);

    let _ = std::fs::create_dir_all(config_dir);
    if let Ok(json) = serde_json::to_string_pretty(&updated) {
        let _ = atomic::write(&store_path(config_dir), json.as_bytes());
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

    // The store write routes through the atomic layer (#644), so an interrupted
    // record leaves the previously saved list byte-for-byte intact rather than
    // blanking the library view. We drive the same pre-rename fault seam `atomic`
    // uses for its own crash test (#265), hermetically against a scratch dir.
    #[test]
    fn a_failed_record_preserves_the_previous_list() {
        let dir = crate::testutil::scratch_dir("recent", "atomic-preserve");
        // Two real projects so both survive the existence prune on read-back.
        let first = dir.join("first");
        let second = dir.join("second");
        Project::create(&first).unwrap();
        Project::create(&second).unwrap();

        record_in(&dir, &first, "First");
        let before = std::fs::read_to_string(store_path(&dir)).unwrap();

        // The next record is armed to fail before its rename; the stored list must
        // be untouched, not truncated or emptied.
        atomic::fail_point::arm_pre_rename_failure();
        record_in(&dir, &second, "Second");
        assert_eq!(
            std::fs::read_to_string(store_path(&dir)).unwrap(),
            before,
            "the previous recent list survives a failed record"
        );
        // And the good list still reads back as the single first project.
        let listed = list_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "First");

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
    fn openable_lists_recents_first_then_the_library_deduped() {
        // A hermetic config dir (#528) so `list()` reads our store, not the real
        // `~/.config`. A recent project that also lives under the library home must
        // appear once (as a recent), with the rest of the library following.
        let cfg = crate::testutil::scratch_dir("recent", "openable-cfg");
        let home = crate::testutil::scratch_dir("recent", "openable-home");
        config::set_config_dir(Some(cfg.clone()));

        Project::create(home.join("shared")).unwrap();
        Project::create(home.join("other")).unwrap();
        let shared = home.join("shared").canonicalize().unwrap();
        record_in(&cfg, &shared, "Shared");

        let out = openable(Some(&home));
        let paths: Vec<_> = out.iter().map(|p| p.path.clone()).collect();
        // The recent leads; "shared" isn't duplicated by the scan; "other" follows.
        assert_eq!(out.len(), 2, "deduped to two projects: {paths:?}");
        assert_eq!(out[0].path, shared, "the recent comes first");
        assert!(
            paths.contains(&home.join("other").canonicalize().unwrap()),
            "the library's other project is included: {paths:?}"
        );

        config::set_config_dir(None);
        std::fs::remove_dir_all(&cfg).ok();
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn openable_with_no_home_is_just_the_recents() {
        // No library home configured: openable is exactly the recent list.
        let cfg = crate::testutil::scratch_dir("recent", "openable-norecents");
        config::set_config_dir(Some(cfg.clone()));
        assert!(
            openable(None).is_empty(),
            "no recents, no home → nothing to open"
        );
        config::set_config_dir(None);
        std::fs::remove_dir_all(&cfg).ok();
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

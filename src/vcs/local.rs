//! The local, offline half of version control (#659): everything that reads or
//! writes the working tree and this repository's own history — init/status/log,
//! reading a file out of a past commit, the diff helpers, restore, and commit.
//! Nothing here touches the network; the remote/auth side lives in
//! [`super::remote`].

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::remote::{
    adopt, blob_text, collect_blobs, index_from_tree, stage_blob, ChapterConflict,
};

/// Editor state and scratch that should never be committed — Quill's own
/// metadata and OS cruft. Everything else (chapter `.md` files, `book.toml`, the
/// `bible/` and `submission/` artifacts) is tracked.
const GITIGNORE: &str = "\
# Quill editor state — not part of the manuscript
/.quill/
/unused/
/bible/.state.toml
/bible/findings.toml

# OS / editor cruft
.DS_Store
*.swp
*.tmp
";

/// Whether `root` is already the top of a Git repository.
pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

/// Initialize a Git repository at `root` and write a default `.gitignore` (only
/// if one isn't already present, so a hand-tuned file is never clobbered).
/// Idempotent: re-initializing an existing repository is safe.
pub fn init(root: &Path) -> Result<(), String> {
    git2::Repository::init(root).map_err(|e| format!("git init: {e}"))?;
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, GITIGNORE).map_err(|e| format!("write .gitignore: {e}"))?;
    }
    Ok(())
}

/// One commit for the history view (#133): short id, first line of the message,
/// and the date it was made (in the commit's recorded timezone). `rev` is the full
/// object id — the unambiguous handle the diff/restore-from-a-commit actions (#587)
/// resolve, so a chosen row addresses exactly one commit even in a large history.
///
/// `parents` are the full object ids of this commit's parents (#964) — one for a
/// normal commit, two (or more) for a merge, none for the root; they carry the
/// revision topology the history view draws as a branch/merge graph. `graph` is the
/// pre-rendered Unicode graph column for this row (the active lanes and this
/// commit's own `●`), laid out once over the whole log by [`graph_rows`] so the
/// renderer just prepends it. `changed` is the manuscript paths this commit touched
/// versus its first parent (#964) — the chapters that changed here.
pub struct CommitMeta {
    pub id: String,
    pub rev: String,
    pub summary: String,
    pub when: String,
    pub parents: Vec<String>,
    pub graph: String,
    pub changed: Vec<String>,
}

/// The most recent commits, newest first (up to `limit`). Empty for a repository
/// with no commits yet. Each commit carries its parent topology and the paths it
/// changed (#964), and the log is post-processed to attach the branch/merge graph
/// column each row renders behind its text.
///
/// The walk seeds from every local branch (#965), not just `HEAD`, so alternate
/// drafts (branches whose commits aren't ancestors of the current one) appear in the
/// history graph as their own lanes — the author sees an alt ending or POV split
/// standing beside the mainline, not hidden until it merges.
pub fn log(root: &Path, limit: usize) -> Result<Vec<CommitMeta>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    // Seed from HEAD *and* every local branch tip, so drafts that have diverged from
    // the current branch still show. `push_glob` silently adds nothing when there are
    // no branch refs yet; HEAD covers the unborn-branch and detached cases.
    let seeded_head = walk.push_head().is_ok();
    let seeded_branches = walk.push_glob("refs/heads/*").is_ok();
    if !seeded_head && !seeded_branches {
        return Ok(Vec::new()); // unborn HEAD, no branches — no commits yet
    }
    // Topological *and* time order: a child always precedes its parents (so the
    // graph lanes are well-formed), newest-first among independent branches.
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for oid in walk.take(limit) {
        let oid = oid.map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let changed = commit_changed_paths(&repo, &commit);
        out.push(CommitMeta {
            id: oid.to_string().chars().take(7).collect(),
            rev: oid.to_string(),
            summary: commit.summary().unwrap_or("(no message)").to_string(),
            when: format_date(commit.time()),
            parents,
            graph: String::new(), // stamped in below by graph_rows
            changed,
        });
    }
    // Lay out the branch/merge graph over the whole (bounded) log and stamp each
    // row's column onto its commit, so the renderer only prepends a string.
    let revs: Vec<String> = out.iter().map(|c| c.rev.clone()).collect();
    let parents: Vec<Vec<String>> = out.iter().map(|c| c.parents.clone()).collect();
    for (commit, row) in out.iter_mut().zip(graph_rows(&revs, &parents)) {
        commit.graph = row;
    }
    Ok(out)
}

/// The manuscript paths a commit changed versus its first parent (#964) — the
/// chapters (and other tracked files) that differ. The root commit (no parent)
/// reports every path in its tree. Best-effort: a diff that can't be computed
/// yields an empty list rather than failing the whole log.
fn commit_changed_paths(repo: &git2::Repository, commit: &git2::Commit) -> Vec<String> {
    let Ok(new_tree) = commit.tree() else {
        return Vec::new();
    };
    let old_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let Ok(diff) = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None) else {
        return Vec::new();
    };
    let mut paths: Vec<String> = Vec::new();
    for delta in diff.deltas() {
        if let Some(p) = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .and_then(Path::to_str)
        {
            let p = p.to_string();
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
    }
    paths
}

/// Lay out a Unicode commit graph, one column string per commit (#964). `revs[i]`
/// is the i-th commit's full id and `parents[i]` its parent ids, in the log's
/// display order (a child always before its parents). Returns a graph prefix for
/// each row: the active branch lanes as `│`, this commit's own node as `●` in its
/// lane. Every string is padded to a common width so the summaries that follow
/// them line up in a column.
///
/// Standard single-pass lane assignment: a list of "active lanes", each holding the
/// id of the next commit expected to occupy it. For each commit we find (or open)
/// its lane, mark its node, then hand that lane to the commit's first parent and
/// open fresh lanes for any extra (merge) parents. A lane waiting on a still-unseen
/// commit carries a straight `│` down through the row.
fn graph_rows(revs: &[String], parents: &[Vec<String>]) -> Vec<String> {
    // Each lane holds the id of the commit that will next land in it, or `None` for
    // a free slot. Newest-first order means we always meet a commit before its
    // parents, so a lane is opened when a child names the parent and closed (or
    // reused) when that parent's own row is reached.
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut out = Vec::with_capacity(revs.len());

    for (rev, ps) in revs.iter().zip(parents.iter()) {
        // The lane this commit sits in: the first already expecting it, else a new
        // one appended on the right.
        let col = lanes
            .iter()
            .position(|l| l.as_deref() == Some(rev.as_str()))
            .unwrap_or_else(|| {
                lanes.push(None);
                lanes.len() - 1
            });

        // Draw the row: every active lane is a `│`, except this commit's, a `●`.
        let mut row = String::new();
        for (i, lane) in lanes.iter().enumerate() {
            if i == col {
                row.push('●');
            } else if lane.is_some() {
                row.push('│');
            } else {
                row.push(' ');
            }
            row.push(' ');
        }
        out.push(row);

        // Advance the lanes past this commit. Clear every lane that was waiting on
        // *this* commit (a commit can head more than one lane where branches merge
        // back together), then route the parents.
        for lane in &mut lanes {
            if lane.as_deref() == Some(rev.as_str()) {
                *lane = None;
            }
        }
        // The first parent continues this commit's lane.
        if let Some(p) = ps.first() {
            place_parent(&mut lanes, col, p);
        }
        // Extra merge parents each take a lane of their own (or reuse a free one).
        for p in ps.iter().skip(1) {
            let c = lanes.iter().position(Option::is_none).unwrap_or_else(|| {
                lanes.push(None);
                lanes.len() - 1
            });
            place_parent(&mut lanes, c, p);
        }
        // Trim trailing empty lanes so the graph keeps a clean right edge.
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
    }

    // Pad every row to the widest so the summaries after them align in a column.
    // Count characters, not bytes: `●`/`│` are one display cell but several bytes,
    // and each lane is one glyph + one space, so a char count is the cell width.
    let width = out.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    for row in &mut out {
        for _ in row.chars().count()..width {
            row.push(' ');
        }
    }
    out
}

/// Assign parent id `p` to lane `col`, unless another lane already expects it — a
/// parent shared by two branches keeps its single lane, so the two draw as lines
/// converging rather than a duplicate column.
fn place_parent(lanes: &mut [Option<String>], col: usize, p: &str) {
    if lanes.iter().any(|l| l.as_deref() == Some(p)) {
        return; // already tracked elsewhere; leave `col` free for reuse/trimming
    }
    if let Some(slot) = lanes.get_mut(col) {
        *slot = Some(p.to_string());
    }
}

/// One named milestone (#833) — an annotated tag marking a whole-manuscript
/// version ("first draft", "sent to beta readers"). `rev` is the full object id
/// the tag points at, the handle the whole-project diff/restore resolve; `message`
/// is the annotation the author gave it, and `when` the date it was created.
pub struct TagMeta {
    pub name: String,
    pub rev: String,
    pub message: String,
    pub when: String,
}

/// Create an annotated tag `name` on the current `HEAD` (#833), recording
/// `message` as its annotation — a named milestone for the whole manuscript.
/// Fails when there's no commit to tag yet, or a tag by that name already exists,
/// so an author never silently overwrites an earlier milestone.
pub fn create_tag(root: &Path, name: &str, message: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let head = repo
        .head()
        .and_then(|h| h.peel(git2::ObjectType::Commit))
        .map_err(|_| "No commit to tag yet — commit first.".to_string())?;
    let sig = signature(&repo)?;
    let msg = if message.trim().is_empty() {
        name
    } else {
        message
    };
    repo.tag(name, &head, &sig, msg, false)
        .map_err(|e| format!("create milestone: {e}"))?;
    Ok(())
}

/// Re-annotate an existing milestone (#966) with a new note, leaving it pointing
/// at the same commit — the edit path for a milestone's free-text note. Recreates
/// the annotated tag in place (`force`) on its current target, so the name, the
/// revision, and the whole-project diff/restore that resolve through it are all
/// unchanged; only the annotation the author reads changes. Errors when no
/// milestone by that name exists, rather than creating one silently.
pub fn set_tag_message(root: &Path, name: &str, message: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    // Resolve the tag to the commit it marks, so the re-tag keeps the same target.
    let target = repo
        .revparse_single(&format!("refs/tags/{name}"))
        .and_then(|obj| obj.peel(git2::ObjectType::Commit))
        .map_err(|_| format!("No milestone named '{name}'."))?;
    let sig = signature(&repo)?;
    // An empty note falls back to the name, matching create — a milestone always
    // carries an annotation, so `list_tags` keeps listing it.
    let msg = if message.trim().is_empty() {
        name
    } else {
        message
    };
    repo.tag(name, &target, &sig, msg, true)
        .map_err(|e| format!("edit milestone note: {e}"))?;
    Ok(())
}

/// Every annotated milestone (#833), newest first by the tag's own creation time.
/// Empty when the project has no milestones. Lightweight tags (created outside
/// Quill) are skipped — a milestone always carries a name and annotation.
pub fn list_tags(root: &Path) -> Result<Vec<TagMeta>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let mut out = Vec::new();
    let names = repo.tag_names(None).map_err(|e| e.to_string())?;
    for name in names.iter().flatten() {
        // Resolve the tag *ref* to its object without peeling through it: an
        // annotated tag is a tag object carrying the message + tagger, whereas a
        // lightweight tag resolves straight to a commit. `revparse_single` on the
        // tag ref gives the tag object itself, which we can then narrow.
        let Ok(obj) = repo.revparse_single(&format!("refs/tags/{name}")) else {
            continue;
        };
        // Skip lightweight tags (created outside Quill) — they carry no annotation.
        let Some(tag) = obj.as_tag() else {
            continue;
        };
        let Ok(target) = tag.target() else {
            continue;
        };
        out.push(TagMeta {
            name: name.to_string(),
            rev: target.id().to_string(),
            message: tag.message().unwrap_or("").trim().to_string(),
            when: format_date(
                tag.tagger()
                    .map_or_else(|| git2::Time::new(0, 0), |t| t.when()),
            ),
        });
    }
    // Newest first, by the tag's own creation date, so a fresh milestone leads.
    out.sort_by(|a, b| b.when.cmp(&a.when));
    Ok(out)
}

/// Restore the *entire* working tree to the state at `rev` (#833) — a whole-project
/// restore to a milestone or past commit, the counterpart to the per-chapter
/// [`restore`]. Forces every tracked path to that revision's version and resets the
/// index to match, so the working tree is exactly that snapshot. Destructive to
/// uncommitted changes (the palette flow confirms first); `.gitignore`d editor
/// state is untouched, since it was never part of the tree.
pub fn restore_all(root: &Path, rev: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let tree = repo
        .revparse_single(rev)
        .and_then(|obj| obj.peel_to_tree())
        .map_err(|e| format!("resolve {rev}: {e}"))?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    // Force overwrites working-tree edits; remove_untracked drops files added since
    // the target so the tree matches the snapshot exactly, not a superset of it.
    checkout.force().remove_untracked(true);
    repo.checkout_tree(tree.as_object(), Some(&mut checkout))
        .map_err(|e| format!("restore project: {e}"))?;
    // Point the index at the restored tree so status reads clean against it.
    let mut index = repo.index().map_err(|e| e.to_string())?;
    index
        .read_tree(&tree)
        .map_err(|e| format!("reset index: {e}"))?;
    index.write().map_err(|e| e.to_string())?;
    Ok(())
}

/// A unified diff of the *whole* working tree against `rev` (#833) — what has
/// changed across the entire manuscript since that milestone or commit, the
/// counterpart to the per-chapter [`diff_file_to_worktree`]. Working-tree edits
/// included; `.gitignore`d paths excluded. Empty string when the tree matches the
/// revision.
pub fn diff_tree_to_worktree(root: &Path, rev: &str) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let tree = repo
        .revparse_single(rev)
        .and_then(|obj| obj.peel_to_tree())
        .map_err(|e| format!("resolve {rev}: {e}"))?;
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Format a commit's time as `YYYY-MM-DD` in the timezone it was committed in.
fn format_date(t: git2::Time) -> String {
    let offset = time::UtcOffset::from_whole_seconds(t.offset_minutes() * 60)
        .unwrap_or(time::UtcOffset::UTC);
    time::OffsetDateTime::from_unix_timestamp(t.seconds()).map_or_else(
        |_| "????-??-??".to_string(),
        |dt| {
            let (y, m, d) = dt.to_offset(offset).to_calendar_date();
            format!("{y:04}-{:02}-{d:02}", u8::from(m))
        },
    )
}

/// The working-tree status: which tracked-or-untracked (non-ignored) files differ
/// from the last commit. The basis for the uncommitted-changes indicator (#132),
/// and the per-file change list the diff/restore (#135) and AI incremental
/// scoping (#90) reuse.
pub struct Status {
    pub changed: Vec<PathBuf>,
}

impl Status {
    pub const fn count(&self) -> usize {
        self.changed.len()
    }
}

/// Compute the working-tree status for `root` — the non-ignored paths that differ
/// from `HEAD` (added, modified, deleted, or untracked). Stat-based via libgit2,
/// so it's cheap enough to refresh on idle, not per keystroke.
pub fn status(root: &Path) -> Result<Status, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let changed = changed_paths(&repo)?;
    Ok(Status { changed })
}

/// The non-ignored paths that differ from `HEAD` (added, modified, deleted, or
/// untracked). Shared by [`status`] and the dirty-tree guard in
/// [`super::remote::pull`].
fn changed_paths(repo: &git2::Repository) -> Result<Vec<PathBuf>, String> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    Ok(statuses
        .iter()
        .filter(|e| !e.status().is_ignored())
        .filter_map(|e| e.path().map(PathBuf::from))
        .collect())
}

/// Whether the working tree has uncommitted changes (added, modified, deleted, or
/// untracked non-ignored files). The guard [`super::remote::pull`] uses to avoid
/// clobbering edits.
pub(super) fn is_dirty(repo: &git2::Repository) -> Result<bool, String> {
    Ok(!changed_paths(repo)?.is_empty())
}

/// The working-tree diff against `HEAD` as a unified patch (added/modified/
/// deleted/untracked). Empty string when clean. For a repo with no commits yet,
/// everything reads as newly added.
pub fn diff(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        // Content lines need their origin marker (+/-/space) prepended; file and
        // hunk headers already carry their own text.
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// The contents of `rel_path` as they were at commit `rev` (a short or full id,
/// or any revision spec `git2` can resolve). `Ok(None)` when that revision has no
/// such file — it was added later, or lived under a different name. The basis for
/// the in-app "diff a chapter against a past commit" and "restore a chapter to a
/// past commit" (#587), which read a single file out of history without touching
/// the working tree.
pub fn show_file_at(root: &Path, rev: &str, rel_path: &str) -> Result<Option<String>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let commit = repo
        .revparse_single(rev)
        .and_then(|obj| obj.peel_to_commit())
        .map_err(|e| format!("resolve {rev}: {e}"))?;
    let tree = commit.tree().map_err(|e| e.to_string())?;
    // The file isn't in that commit.
    let Ok(entry) = tree.get_path(Path::new(rel_path)) else {
        return Ok(None);
    };
    let blob = entry
        .to_object(&repo)
        .map_err(|e| e.to_string())?
        .peel_to_blob()
        .map_err(|e| e.to_string())?;
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

/// A unified diff of `rel_path` between two commits (#587): `rev_a` → `rev_b`.
/// Empty string when the file is identical (or absent) in both. Reads only the two
/// blobs from history, never the working tree, so it answers "this chapter then vs.
/// then" against any past pair. Pass `"HEAD"` as one side to anchor on the last
/// commit; the working-tree comparison the history view runs is
/// [`diff_file_to_worktree`].
// The commit-to-commit primitive of the pair (#587); the UI wires the
// working-tree variant, so in this bin crate this one is reached only from tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn diff_file(root: &Path, rev_a: &str, rev_b: &str, rel_path: &str) -> Result<String, String> {
    let old = show_file_at(root, rev_a, rel_path)?.unwrap_or_default();
    let new = show_file_at(root, rev_b, rel_path)?.unwrap_or_default();
    diff_bytes(old.as_bytes(), new.as_bytes(), rel_path)
}

/// A unified diff of `rel_path` from commit `rev` to its *current* on-disk version
/// (#587) — what has changed in this one chapter since that commit, working-tree
/// edits included. Empty when unchanged. The comparison the history view's "diff
/// this chapter" runs, so a chosen commit is measured against what the author is
/// looking at now, not the last commit. The between-two-commits variant is
/// [`diff_file`].
pub fn diff_file_to_worktree(root: &Path, rev: &str, rel_path: &str) -> Result<String, String> {
    let old = show_file_at(root, rev, rel_path)?.unwrap_or_default();
    let new = fs::read(root.join(rel_path)).unwrap_or_default();
    diff_bytes(old.as_bytes(), &new, rel_path)
}

/// Render a unified patch for one path from `old` to `new` bytes. The shared body
/// of the file-diff helpers, so the origin-marker handling lives in one place.
fn diff_bytes(old: &[u8], new: &[u8], rel_path: &str) -> Result<String, String> {
    let mut out = String::new();
    let path = Some(Path::new(rel_path));
    git2::Patch::from_buffers(old, path, new, path, None)
        .map_err(|e| e.to_string())?
        .print(&mut |_delta, _hunk, line| {
            // Content lines need their origin marker (+/-/space) prepended; the
            // file and hunk headers already carry their own text.
            if matches!(line.origin(), '+' | '-' | ' ') {
                out.push(line.origin());
            }
            out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            true
        })
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Restore the file at `rel_path` (relative to `root`) to its last committed
/// version, discarding working-tree changes to it. Returns `false` if there's no
/// committed version to restore to (no commits yet, or the file is new). Touches
/// only that one file.
pub fn restore(root: &Path, rel_path: &str) -> Result<bool, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let Some(head_tree) = repo.head().ok().and_then(|h| h.peel_to_tree().ok()) else {
        return Ok(false); // no commits → nothing to restore to
    };
    if head_tree.get_path(Path::new(rel_path)).is_err() {
        return Ok(false); // the file isn't in the last commit
    }
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force().path(rel_path);
    repo.checkout_head(Some(&mut checkout))
        .map_err(|e| format!("restore: {e}"))?;
    Ok(true)
}

/// Stage the whole project and commit it with `message`. Returns the new commit's
/// short id, or `None` when there's nothing to commit (a clean working tree).
///
/// Staging respects `.gitignore` and captures additions, modifications, and
/// deletions (e.g. a chapter moved to `unused/`). Identity comes from the git
/// config when set, else a generic default — so an author who has never used git
/// can still commit without configuring anything.
pub fn commit(root: &Path, message: &str) -> Result<Option<String>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let sig = signature(&repo)?;

    let mut index = repo.index().map_err(|e| e.to_string())?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| e.to_string())?;
    index
        .update_all(["*"].iter(), None)
        .map_err(|e| e.to_string())?; // stage deletions of tracked files
    index.write().map_err(|e| e.to_string())?;
    let tree_id = index.write_tree().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;

    // Parent = current HEAD, unless this is the first (unborn HEAD) commit.
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().map_err(|e| e.to_string())?),
        Err(_) => None,
    };
    // Nothing changed since the last commit → don't create an empty commit.
    if let Some(p) = &parent {
        if p.tree_id() == tree_id {
            return Ok(None);
        }
    }

    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| format!("commit: {e}"))?;
    Ok(Some(oid.to_string().chars().take(7).collect()))
}

/// The identity to record on a commit — the author's git config when set, else a
/// generic Quill default, so someone who has never configured git can still commit
/// (and merge). Shared by [`commit`] and the merge commit [`super::remote::pull`]
/// creates when a diverged branch merges cleanly.
pub(super) fn signature(repo: &git2::Repository) -> Result<git2::Signature<'static>, String> {
    repo.signature()
        .or_else(|_| git2::Signature::now("Quill Author", "author@quill.local"))
        .map_err(|e| format!("signature: {e}"))
}

// ---------------------------------------------------------------------------
// Branch-as-draft (#965): a named branch is a "draft" — an alternate version
// (alt ending, POV split, an experiment) tracked as a first-class line beside
// the mainline. Create one from the current state, switch between them (the
// working copy follows), list them, and merge one draft back into another. A
// merge reuses the chapter-level three-way the two-way sync (#962) does, so a
// clean merge lands a two-parent commit and a conflicting one surfaces the same
// per-chapter [`ChapterConflict`]s the resolution UI (#963) already resolves.
// ---------------------------------------------------------------------------

/// The name of the draft (branch) currently checked out (#965), e.g. `main` or
/// `alt-ending`. Errors on a detached or unborn HEAD (no draft to name — nothing
/// is committed yet). The mirror of [`super::remote`]'s `current_branch`, exposed
/// here so the drafts overlay can show which draft is active.
pub fn current_branch(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let head = repo
        .head()
        .map_err(|_| "No commits yet — commit before working with drafts.".to_string())?;
    head.shorthand()
        .map(str::to_string)
        .ok_or_else(|| "Not on a draft (detached HEAD).".to_string())
}

/// Every draft (local branch) in the project (#965), sorted by name. Empty when
/// nothing is committed yet (no branches exist). The list the drafts overlay
/// renders, one row per draft, with the current one marked.
pub fn list_branches(root: &Path) -> Result<Vec<String>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let branches = repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for branch in branches {
        let (branch, _) = branch.map_err(|e| e.to_string())?;
        if let Some(name) = branch
            .name()
            .map_err(|e| e.to_string())?
            .map(str::to_string)
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Create a new draft `name` from the current state (#965) — a branch at the
/// current `HEAD`. Does *not* switch to it (the caller decides), so an author can
/// spin off an alt version and keep writing the current one, or create-and-switch
/// in one gesture. Fails when nothing is committed yet (no commit to branch from)
/// or a draft by that name already exists, so an existing draft is never silently
/// moved out from under its own history.
pub fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|_| "No commits yet — commit before creating a draft.".to_string())?;
    if repo.find_branch(name, git2::BranchType::Local).is_ok() {
        return Err(format!("A draft named '{name}' already exists."));
    }
    repo.branch(name, &head, false)
        .map_err(|e| format!("create draft: {e}"))?;
    Ok(())
}

/// Switch to the draft `name` (#965): point `HEAD` at that branch and bring the
/// working tree up to its version, so the editor now shows that draft's chapters.
/// Refuses up front when the working tree is dirty (#639) — the force checkout
/// would clobber uncommitted edits — so the guard mirrors [`super::remote::pull`]:
/// the caller commits (or restores) first. A no-op switch to the current draft is
/// rejected with a clear message rather than a churny re-checkout. The caller
/// reloads the active chapter afterwards, since the files on disk have changed.
pub fn checkout_branch(root: &Path, name: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if current_branch(root).ok().as_deref() == Some(name) {
        return Err(format!("Already on draft '{name}'."));
    }
    if is_dirty(&repo)? {
        return Err(SWITCH_DIRTY.to_string());
    }
    let branch = repo
        .find_branch(name, git2::BranchType::Local)
        .map_err(|_| format!("No draft named '{name}'."))?;
    let refname = branch
        .get()
        .name()
        .ok_or_else(|| "draft has no ref name".to_string())?
        .to_string();
    let object = repo
        .revparse_single(&refname)
        .map_err(|e| format!("resolve draft: {e}"))?;
    // Force overwrites any working-tree edits (already known clean by the guard) and
    // remove_untracked drops files the target draft doesn't have, so the tree is
    // exactly that draft's snapshot, not a superset of it.
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force().remove_untracked(true);
    repo.checkout_tree(&object, Some(&mut checkout))
        .map_err(|e| format!("switch draft: {e}"))?;
    repo.set_head(&refname)
        .map_err(|e| format!("switch draft: {e}"))?;
    Ok(())
}

/// The result of merging one draft into another (#965): a clean merge that landed a
/// commit, or a set of per-chapter conflicts that stopped it. On [`Conflicts`] the
/// repo is left exactly as it was — nothing committed, both drafts intact — so the
/// resolution UI (#963) can resolve them and [`finalize_branch_merge`] finish the job.
///
/// [`Conflicts`]: MergeOutcome::Conflicts
pub enum MergeOutcome {
    /// The merge landed cleanly. `merged` lists the chapters adopted from the source
    /// draft (empty when only the target draft had changed since they split).
    Clean { merged: Vec<String> },
    /// The merge stopped on chapters changed differently on both drafts. Each carries
    /// all three versions, for the resolution UI (#963) — nothing was written.
    Conflicts { conflicts: Vec<ChapterConflict> },
}

/// Merge the draft `from` into the draft `into` (#965) — fold an alt version back
/// into the mainline (or vice-versa). `into` is checked out afterwards, so the
/// author lands on the draft that received the merge. Reuses the two-way sync's
/// chapter-level three-way (#962): base = the drafts' shared ancestor, "ours" =
/// `into`, "theirs" = `from`. A chapter only `from` changed is adopted; one only
/// `into` changed is kept; one both changed differently is a
/// [conflict](ChapterConflict). A clean merge records a two-parent commit (both
/// drafts' history reachable) and updates the working tree; a conflicting one
/// changes nothing and returns the conflicts for [`finalize_branch_merge`] to land
/// once resolved.
///
/// Refuses a dirty working tree up front (#639), like [`checkout_branch`], so the
/// merge checkout can't overwrite an uncommitted edit. `into` must be the current
/// draft (the working tree is already its snapshot) — the caller switches to it
/// first — so a clean merge lands on disk without an extra checkout dance.
pub fn merge_branch(root: &Path, into: &str, from: &str) -> Result<MergeOutcome, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if into == from {
        return Err("A draft can't merge into itself.".to_string());
    }
    if is_dirty(&repo)? {
        return Err(SWITCH_DIRTY.to_string());
    }
    if current_branch(root).ok().as_deref() != Some(into) {
        return Err(format!("Switch to draft '{into}' before merging into it."));
    }

    let (ours, theirs, base_tree) = merge_bases(&repo, into, from)?;
    let ours_tree = ours.tree().map_err(|e| e.to_string())?;
    let theirs_tree = theirs.tree().map_err(|e| e.to_string())?;

    // Reconcile chapter-by-chapter, filling no conflicts (this pass only *detects*
    // them). An empty resolution map means a both-sides-changed chapter is reported,
    // never written.
    let recon = reconcile_trees(
        &repo,
        base_tree.as_ref(),
        &ours_tree,
        &theirs_tree,
        &BTreeMap::new(),
    )?;
    if !recon.conflicts.is_empty() {
        // Never write a half-merged tree — hand the conflicts back for resolution.
        return Ok(MergeOutcome::Conflicts {
            conflicts: recon.conflicts,
        });
    }

    land_merge(&repo, into, from, &ours, &theirs, recon.tree)?;
    Ok(MergeOutcome::Clean {
        merged: recon.merged,
    })
}

/// Complete a draft merge the author resolved by hand (#965) — the branch-merge
/// counterpart to [`super::remote::finalize_sync`]. A [`merge_branch`] that hit
/// conflicts left the repo untouched; once the author has chosen a final text for
/// each conflicting chapter (`resolutions`: path → chosen text), this re-derives the
/// same three-way, layers the clean source-only edits and the author's resolutions
/// onto the target draft's tree, and records the two-parent merge commit. `into`
/// must still be the current draft. Refuses a dirty tree up front. Returns the count
/// of chapters landed for the caller's summary.
pub fn finalize_branch_merge(
    root: &Path,
    into: &str,
    from: &str,
    resolutions: &BTreeMap<String, String>,
) -> Result<usize, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if is_dirty(&repo)? {
        return Err(SWITCH_DIRTY.to_string());
    }
    if current_branch(root).ok().as_deref() != Some(into) {
        return Err(format!("Switch to draft '{into}' before merging into it."));
    }

    let (ours, theirs, base_tree) = merge_bases(&repo, into, from)?;
    let ours_tree = ours.tree().map_err(|e| e.to_string())?;
    let theirs_tree = theirs.tree().map_err(|e| e.to_string())?;

    let recon = reconcile_trees(
        &repo,
        base_tree.as_ref(),
        &ours_tree,
        &theirs_tree,
        resolutions,
    )?;
    // Any conflict left unresolved means the caller under-supplied resolutions; the
    // three-way filled the resolved ones already, so a leftover is a real gap.
    if !recon.conflicts.is_empty() {
        return Err(format!(
            "{} chapter(s) still need resolving before the merge can complete.",
            recon.conflicts.len()
        ));
    }
    land_merge(&repo, into, from, &ours, &theirs, recon.tree)?;
    Ok(resolutions.len())
}

/// Resolve the two draft tips and their shared-ancestor tree for a merge (#965).
/// Returns `(into_commit, from_commit, base_tree)`; the base is `None` for unrelated
/// histories (treated as an empty ancestor, so every touched chapter reads as added
/// on both drafts).
fn merge_bases<'r>(
    repo: &'r git2::Repository,
    into: &str,
    from: &str,
) -> Result<(git2::Commit<'r>, git2::Commit<'r>, Option<git2::Tree<'r>>), String> {
    let ours = branch_commit(repo, into)?;
    let theirs = branch_commit(repo, from)?;
    let base_tree = match repo.merge_base(ours.id(), theirs.id()) {
        Ok(base) => Some(
            repo.find_commit(base)
                .and_then(|c| c.tree())
                .map_err(|e| e.to_string())?,
        ),
        Err(_) => None,
    };
    Ok((ours, theirs, base_tree))
}

/// The commit a local draft points at, or a clear error naming the missing draft.
fn branch_commit<'r>(repo: &'r git2::Repository, name: &str) -> Result<git2::Commit<'r>, String> {
    repo.find_branch(name, git2::BranchType::Local)
        .map_err(|_| format!("No draft named '{name}'."))?
        .get()
        .peel_to_commit()
        .map_err(|e| format!("resolve draft '{name}': {e}"))
}

/// The outcome of a chapter-level three-way over two trees (#965): the merged tree
/// (written to the odb) plus the chapters adopted from "theirs" and any unresolved
/// conflicts. The shared body of [`merge_branch`] (detect) and
/// [`finalize_branch_merge`] (apply resolutions), so both walk the trees identically.
struct Reconciled {
    tree: git2::Oid,
    merged: Vec<String>,
    conflicts: Vec<ChapterConflict>,
}

/// Reconcile `ours` and `theirs` against their `base` chapter-by-chapter (#965) — the
/// same three-way the two-way sync (#962) runs, over two local drafts. Starts the
/// merged tree from `ours` (so an ours-only edit and a keep-target resolution are
/// already in place), adopts a theirs-only edit, writes any `resolutions` verbatim,
/// and records a [`ChapterConflict`] for a both-changed chapter not in `resolutions`.
/// The tree is always written (cheap, unreferenced until a commit points at it); the
/// caller ignores it when conflicts remain.
fn reconcile_trees(
    repo: &git2::Repository,
    base: Option<&git2::Tree>,
    ours: &git2::Tree,
    theirs: &git2::Tree,
    resolutions: &BTreeMap<String, String>,
) -> Result<Reconciled, String> {
    let mut base_blobs = BTreeMap::new();
    if let Some(t) = base {
        collect_blobs(t, &mut base_blobs);
    }
    let mut our_blobs = BTreeMap::new();
    collect_blobs(ours, &mut our_blobs);
    let mut their_blobs = BTreeMap::new();
    collect_blobs(theirs, &mut their_blobs);

    let mut paths: Vec<String> = base_blobs
        .keys()
        .chain(our_blobs.keys())
        .chain(their_blobs.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();

    let mut merged = Vec::new();
    let mut conflicts = Vec::new();
    // An index (not a flat TreeBuilder) so nested chapter paths merge correctly.
    let mut index = index_from_tree(ours)?;

    for path in paths {
        let b = base_blobs.get(&path).copied();
        let o = our_blobs.get(&path).copied();
        let t = their_blobs.get(&path).copied();

        if let Some(chosen) = resolutions.get(&path) {
            // A resolved conflict: write the author's final text verbatim.
            let oid = repo
                .blob(chosen.as_bytes())
                .map_err(|e| format!("write resolution: {e}"))?;
            stage_blob(&mut index, &path, oid)?;
            continue;
        }
        if o == t {
            continue; // both drafts agree (including both absent) — nothing to do
        }
        if o == b {
            // Only the source draft changed this chapter — adopt its version.
            adopt(&mut index, &path, t)?;
            merged.push(path);
        } else if t == b {
            // Only the target draft changed it — keep ours (already in the index).
        } else {
            // Both changed it, differently — a per-chapter conflict (#965).
            conflicts.push(ChapterConflict {
                path: path.clone(),
                local: blob_text(repo, o),
                remote: blob_text(repo, t),
                base: blob_text(repo, b),
            });
        }
    }

    let tree = index.write_tree_to(repo).map_err(|e| e.to_string())?;
    Ok(Reconciled {
        tree,
        merged,
        conflicts,
    })
}

/// Record the two-parent merge commit for a clean (or resolved) draft merge and
/// bring the working tree up to it (#965). `into` is the current draft, so the
/// commit lands on its ref and the force checkout is safe (the caller confirmed the
/// tree clean). Both drafts' commits stay reachable through the two parents.
fn land_merge(
    repo: &git2::Repository,
    into: &str,
    from: &str,
    ours: &git2::Commit,
    theirs: &git2::Commit,
    tree: git2::Oid,
) -> Result<(), String> {
    let tree = repo.find_tree(tree).map_err(|e| e.to_string())?;
    let sig = signature(repo)?;
    let message = format!("Merge draft '{from}' into '{into}'");
    let refname = format!("refs/heads/{into}");
    repo.commit(Some(&refname), &sig, &sig, &message, &tree, &[ours, theirs])
        .map_err(|e| format!("merge commit: {e}"))?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Shown when switching drafts or merging is refused because the working tree has
/// uncommitted edits the checkout would overwrite (#965). Commit (or restore) first.
const SWITCH_DIRTY: &str =
    "You have uncommitted changes — commit them before switching or merging drafts.";

#[cfg(test)]
mod graph_tests {
    use super::graph_rows;

    /// The `●` sits in the column named by `col`; every earlier active lane draws a
    /// `│`. A helper so the expectations read as "node in lane N".
    fn node_col(row: &str) -> usize {
        row.chars().step_by(2).position(|c| c == '●').unwrap_or(999)
    }

    #[test]
    fn a_linear_history_is_a_single_lane() {
        // c -> b -> a (newest first). One straight lane: every node in column 0.
        let revs = vec!["c".to_string(), "b".to_string(), "a".to_string()];
        let parents = vec![
            vec!["b".to_string()],
            vec!["a".to_string()],
            vec![], // root
        ];
        let rows = graph_rows(&revs, &parents);
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(node_col(r), 0, "linear history stays in lane 0: {r:?}");
            assert!(r.contains('●'));
        }
    }

    #[test]
    fn a_branch_and_merge_opens_and_closes_a_second_lane() {
        // A merge `m` with two parents `x` (mainline) and `y` (a side branch),
        // which both descend from base `a`:
        //   m ─┬─ x ─── a
        //      └─ y ────┘
        // Newest first: m, x, y, a. The merge row must show a node plus a second
        // lane, and while the branch is live (rows for x and y) two lanes are drawn.
        let revs = vec![
            "m".to_string(),
            "x".to_string(),
            "y".to_string(),
            "a".to_string(),
        ];
        let parents = vec![
            vec!["x".to_string(), "y".to_string()], // merge: two parents
            vec!["a".to_string()],
            vec!["a".to_string()],
            vec![],
        ];
        let rows = graph_rows(&revs, &parents);
        assert_eq!(rows.len(), 4);
        // The merge row (row 0) is itself a single node; its extra-parent lane opens
        // on the rows *below* it, git-graph style.
        assert!(rows[0].contains('●'), "the merge is a node: {:?}", rows[0]);
        // Once the branch has fanned out, two lanes are drawn while both the
        // mainline (x) and side (y) commits are still unresolved.
        let live_lanes = |r: &str| r.matches('│').count() + r.matches('●').count();
        assert!(
            live_lanes(&rows[1]) >= 2 || live_lanes(&rows[2]) >= 2,
            "a second lane is drawn while the branch is live: {:?}",
            &rows[1..3]
        );
        // The shared base collapses back to a single lane (node in column 0).
        assert_eq!(
            node_col(&rows[3]),
            0,
            "the base rejoins lane 0: {:?}",
            rows[3]
        );
        // Every row is padded to the same display width for column alignment.
        let w = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == w), "aligned width");
    }

    #[test]
    fn an_empty_log_yields_no_rows() {
        assert!(graph_rows(&[], &[]).is_empty());
    }
}

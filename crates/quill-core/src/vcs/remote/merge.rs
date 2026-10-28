//! The chapter-level three-way merge primitives shared by the two-way [`sync`]
//! (#962) and the branch-merge (#965): collecting a tree's blobs, seeding an
//! in-memory index from a tree, and staging/adopting a chapter into that merged
//! tree. Chapters are atomic files, so reconciliation is per-chapter (whole-blob
//! adopt), never per-line — these helpers are what both callers walk their trees
//! with.
//!
//! [`sync`]: super::sync

use std::collections::BTreeMap;
use std::path::Path;

/// Write the remote's version of `path` into the merged tree index (or stage its
/// removal when the remote deleted the chapter). The chapter is adopted wholesale —
/// chapters are atomic files, so there's no sub-file merge to do. Shared with the
/// branch merge (#965), which reconciles two local branches with the same
/// chapter-level three-way.
///
/// The merged tree is assembled through a [`git2::Index`] rather than a flat
/// `TreeBuilder`: libgit2's single-level `TreeBuilder` rejects any entry name
/// containing `/`, so nested chapter paths (`bible/setting.md`, `submission/…`)
/// abort the whole merge. An index seeded with `read_tree` handles the full path
/// natively and writes the nested subtrees for us.
pub(in crate::vcs) fn adopt(
    index: &mut git2::Index,
    path: &str,
    theirs: Option<git2::Oid>,
) -> Result<(), String> {
    match theirs {
        Some(oid) => stage_blob(index, path, oid),
        None => index
            .remove_path(Path::new(path))
            .map_err(|e| e.to_string()),
    }
}

/// Stage `oid` at `path` in the merged tree `index` as a regular-file blob. Builds
/// the [`git2::IndexEntry`] by hand (rather than `add_path`, which would read the
/// working directory) so the merge stays purely in-memory; the timestamp/ownership
/// fields are zeroed since only the path, mode, and blob id matter for `write_tree`.
pub(in crate::vcs) fn stage_blob(
    index: &mut git2::Index,
    path: &str,
    oid: git2::Oid,
) -> Result<(), String> {
    let entry = git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: git2::FileMode::Blob.into(),
        uid: 0,
        gid: 0,
        file_size: 0,
        id: oid,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    };
    index.add(&entry).map_err(|e| e.to_string())
}

/// Seed a fresh in-memory index with the blobs of `tree`, the starting point for a
/// three-way merge. Kept off the repository's on-disk index so the merge never
/// touches the working index or the checkout; the caller layers adoptions/removals
/// on top and calls [`git2::Index::write_tree_to`] to persist the merged tree.
pub(in crate::vcs) fn index_from_tree(tree: &git2::Tree) -> Result<git2::Index, String> {
    let mut index = git2::Index::new().map_err(|e| e.to_string())?;
    index.read_tree(tree).map_err(|e| e.to_string())?;
    Ok(index)
}

/// Collect every blob (file) in `tree`, keyed by its full path, as `path -> blob id`.
/// Recurses into subtrees so nested paths are compared too; non-blob entries (the
/// subtree objects themselves) are skipped, since only files carry chapter text.
/// Shared with the branch merge (#965).
pub(in crate::vcs) fn collect_blobs(tree: &git2::Tree, out: &mut BTreeMap<String, git2::Oid>) {
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            if let Some(name) = entry.name() {
                out.insert(format!("{dir}{name}"), entry.id());
            }
        }
        git2::TreeWalkResult::Ok
    });
}

/// The text of a blob by id, lossily decoded as UTF-8 (chapters are Markdown).
/// Empty string when the blob is absent (the chapter didn't exist on that side) or
/// can't be read — a missing ancestor reads as "added", the caller's intent. Shared
/// with the branch merge (#965).
pub(in crate::vcs) fn blob_text(repo: &git2::Repository, oid: Option<git2::Oid>) -> String {
    oid.and_then(|oid| repo.find_blob(oid).ok())
        .map(|blob| String::from_utf8_lossy(blob.content()).into_owned())
        .unwrap_or_default()
}

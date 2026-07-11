//! Chapter structure editing (#316): add / rename / swap / remove a chapter.
//! Each follows the same invariant — a file operation, then a manifest mutation,
//! then a save — so they're grouped out of `project.rs`. Removing a chapter
//! retires its file to `unused/` rather than deleting it, so nothing the author
//! wrote is lost.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::atomic;
use crate::frontmatter::{self, Band, FrontMatter};

use super::{read_existing, shift_parts_for_delete, shift_parts_for_insert, ChapterEntry, Project};

impl Project {
    /// The 1-based **body position** a chapter at raw index `index` occupies —
    /// the mapping the part ranges live in (#185): its offset past the front
    /// matter, plus one. Only meaningful for a Body chapter (front/back matter
    /// belongs to no part). Uses `saturating_sub` so a Body chapter that ever
    /// sits before the front count doesn't underflow; callers gate on the band.
    fn body_position(&self, index: usize) -> usize {
        index.saturating_sub(self.manifest.front_chapter_count()) + 1
    }

    /// After inserting a Body chapter at raw index `at`, grow/shift the part
    /// ranges so each part keeps grouping its own chapters (#316 — parts follow
    /// structural edits). A non-body insert (front/back matter) leaves the body
    /// positions unchanged, so parts are untouched. Call *after* the chapter is
    /// in the manifest; `at` is its raw index.
    fn shift_parts_after_insert(&mut self, at: usize) {
        if Band::of(self.manifest.chapters.get(at).and_then(|c| c.role)) != Band::Body {
            return;
        }
        let body_pos = self.body_position(at);
        if let Some(parts) = self.manifest.parts.as_deref_mut() {
            shift_parts_for_insert(parts, body_pos);
        }
    }

    /// Insert a new empty chapter titled `title` at position `at` (clamped to
    /// the end). Its file is created on disk.
    pub fn add_chapter(&mut self, at: usize, title: &str) -> std::io::Result<()> {
        self.insert_chapter_with_body(at, title, "")
    }

    /// Insert a new chapter titled `title` at position `at` (clamped to the end)
    /// whose file holds `body` (a plain prose body, no front matter), then read
    /// its cache back so the new chapter's word count is populated like any other
    /// (#585 — the tail half of a split). The file is written first, then the
    /// manifest entry, then the save, keeping the add-a-chapter invariant.
    pub fn insert_chapter_with_body(
        &mut self,
        at: usize,
        title: &str,
        body: &str,
    ) -> std::io::Result<()> {
        let at = at.min(self.manifest.chapters.len());
        let file = self.unique_chapter_file();
        atomic::write(&self.root.join(&file), body.as_bytes())?;
        self.manifest
            .chapters
            .insert(at, ChapterEntry::new(title.to_string(), file));
        self.read_chapter_cache(at);
        self.shift_parts_after_insert(at);
        self.save_manifest()
    }

    /// Duplicate chapter `from`, inserting the copy titled `title` immediately
    /// after it (#585). The copy is a verbatim file copy — front matter included —
    /// so it opens identically; its cache is read back so its word count and
    /// metadata match the original. The source is read through [`read_existing`]
    /// so a failed read aborts rather than writing an empty copy (#565).
    pub fn duplicate_chapter(&mut self, from: usize, title: &str) -> std::io::Result<()> {
        let text = read_existing(&self.chapter_path(from))?;
        let at = from + 1;
        let file = self.unique_chapter_file();
        atomic::write(&self.root.join(&file), text.as_bytes())?;
        self.manifest
            .chapters
            .insert(at, ChapterEntry::new(title.to_string(), file));
        self.read_chapter_cache(at);
        self.shift_parts_after_insert(at);
        self.save_manifest()
    }

    /// Append `body` to the prose of chapter `target` (#585 — the surviving half
    /// of a merge), preserving the target's front matter, and refresh its cached
    /// word count. A blank line separates the existing body from the appended
    /// text so the two chapters' prose doesn't run together. The file is read
    /// through [`read_existing`] first so a failed read aborts rather than
    /// blanking the chapter (#565).
    pub fn append_to_chapter(&mut self, target: usize, body: &str) -> std::io::Result<()> {
        let path = self.chapter_path(target);
        let text = read_existing(&path)?;
        let (front, existing) = frontmatter::split(&text);
        let existing = existing.trim_end_matches(['\n', '\r']);
        let appended = body.trim_start_matches(['\n', '\r']);
        let merged = if existing.is_empty() {
            appended.to_string()
        } else if appended.is_empty() {
            existing.to_string()
        } else {
            format!("{existing}\n\n{appended}")
        };
        let front = front.map(FrontMatter::from_raw);
        let content = frontmatter::join(front.as_ref().map(FrontMatter::as_str), &merged);
        atomic::write(&path, content.as_bytes())?;
        self.read_chapter_cache(target);
        self.save_manifest()
    }

    /// Rename the chapter at `at` (title only; its file is unchanged).
    pub fn rename_chapter(&mut self, at: usize, title: &str) -> std::io::Result<()> {
        if let Some(chapter) = self.manifest.chapters.get_mut(at) {
            chapter.title = title.to_string();
        }
        self.save_manifest()
    }

    /// Swap two chapters' positions in the order.
    pub fn swap_chapters(&mut self, a: usize, b: usize) -> std::io::Result<()> {
        self.manifest.chapters.swap(a, b);
        self.save_manifest()
    }

    /// Remove the chapter at `at`, moving its file into the project's `unused/`
    /// directory rather than deleting it, so nothing the author wrote is lost.
    pub fn remove_chapter(&mut self, at: usize) -> std::io::Result<()> {
        let src = self.chapter_path(at);
        if src.exists() {
            let unused = self.root.join("unused");
            fs::create_dir_all(&unused)?;
            let dest = unique_dest(&unused, &self.manifest.chapters[at].file);
            fs::rename(&src, &dest)?;
        }
        // Drop the removed chapter's words from the running project total (#867).
        self.words_total = self
            .words_total
            .saturating_sub(self.manifest.chapters[at].words);
        // A removed Body chapter vacates a part position: shrink/shift/drop the
        // affected parts so each keeps grouping its own chapters (#316 — parts
        // follow structural edits). Compute the body position before the entry
        // leaves the manifest; a front/back-matter removal leaves body positions
        // (and so the parts) unchanged.
        let removed_body_pos = (Band::of(self.manifest.chapters[at].role) == Band::Body)
            .then(|| self.body_position(at));
        self.manifest.chapters.remove(at);
        if let (Some(body_pos), Some(parts)) = (removed_body_pos, self.manifest.parts.as_mut()) {
            shift_parts_for_delete(parts, body_pos);
        }
        self.save_manifest()
    }

    /// Smallest `chapter-{n}.md` not already used by the manifest or on disk.
    fn unique_chapter_file(&self) -> String {
        let used: HashSet<&str> = self
            .manifest
            .chapters
            .iter()
            .map(|c| c.file.as_str())
            .collect();
        // `used` and the on-disk names are finite, so all but finitely many of the
        // distinct `chapter-{n}.md` names are free — `find` always terminates.
        #[allow(
            clippy::maybe_infinite_iter,
            clippy::expect_used,
            reason = "the unbounded `1..` range over a finite used/on-disk name set \
                      always finds a free `chapter-{n}.md`, so `find` yields `Some`"
        )]
        (1..)
            .map(|n| format!("chapter-{n}.md"))
            .find(|f| !used.contains(f.as_str()) && !self.root.join(f).exists())
            .expect("an unbounded range always yields a free name")
    }
}

/// A path inside `dir` for `file`, appending `-1`, `-2`, … to the stem if a file
/// of that name already exists there (so moving to `unused/` never clobbers).
pub fn unique_dest(dir: &Path, file: &str) -> PathBuf {
    let candidate = dir.join(file);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(file);
    let ext = path.extension().and_then(|s| s.to_str());
    // Each `k` yields a distinct candidate and the on-disk names are finite, so all
    // but finitely many candidates don't exist — `find` always terminates.
    #[allow(
        clippy::maybe_infinite_iter,
        clippy::expect_used,
        reason = "the unbounded `1..` range over a finite set of on-disk names always \
                  finds a non-existent candidate, so `find` yields `Some`"
    )]
    (1..)
        .map(|k| {
            ext.map_or_else(
                || dir.join(format!("{stem}-{k}")),
                |e| dir.join(format!("{stem}-{k}.{e}")),
            )
        })
        .find(|c| !c.exists())
        .expect("an unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_dest_avoids_clobbering() {
        let dir = std::env::temp_dir().join(format!("quill-unique-dest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.md"), "").unwrap();
        assert_eq!(unique_dest(&dir, "b.md"), dir.join("b.md")); // free
        assert_eq!(unique_dest(&dir, "a.md"), dir.join("a-1.md")); // taken → suffixed
        let _ = fs::remove_dir_all(&dir);
    }
}

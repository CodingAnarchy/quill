//! Chapter structure editing (#316): add / rename / swap / remove a chapter.
//! Each follows the same invariant — a file operation, then a manifest mutation,
//! then a save — so they're grouped out of `project.rs`. Removing a chapter
//! retires its file to `unused/` rather than deleting it, so nothing the author
//! wrote is lost.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::atomic;
use crate::frontmatter::{self, FrontMatter};

use super::{read_existing, ChapterEntry, Project};

impl Project {
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
        self.manifest.chapters.remove(at);
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
        (1..)
            .map(|n| format!("chapter-{n}.md"))
            .find(|f| !used.contains(f.as_str()) && !self.root.join(f).exists())
            .expect("an unbounded range always yields a free name")
    }
}

/// A path inside `dir` for `file`, appending `-1`, `-2`, … to the stem if a file
/// of that name already exists there (so moving to `unused/` never clobbers).
fn unique_dest(dir: &Path, file: &str) -> PathBuf {
    let candidate = dir.join(file);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(file);
    let ext = path.extension().and_then(|s| s.to_str());
    (1..)
        .map(|k| match ext {
            Some(e) => dir.join(format!("{stem}-{k}.{e}")),
            None => dir.join(format!("{stem}-{k}")),
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

//! Chapter structure editing (#316): add / rename / swap / remove a chapter.
//! Each follows the same invariant — a file operation, then a manifest mutation,
//! then a save — so they're grouped out of `project.rs`. Removing a chapter
//! retires its file to `unused/` rather than deleting it, so nothing the author
//! wrote is lost.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{ChapterEntry, Project};

impl Project {
    /// Insert a new empty chapter titled `title` at position `at` (clamped to
    /// the end). Its file is created on disk.
    pub fn add_chapter(&mut self, at: usize, title: &str) -> std::io::Result<()> {
        let at = at.min(self.manifest.chapters.len());
        let file = self.unique_chapter_file();
        fs::write(self.root.join(&file), "")?;
        self.manifest
            .chapters
            .insert(at, ChapterEntry::new(title.to_string(), file));
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

//! A Quill project: a directory of per-chapter Markdown files plus a root
//! `book.toml` manifest that records the chapter order.
//!
//! The unit of work is always a project — there is no single-file mode. The
//! manifest is the source of truth for chapter order and titles; the chapter
//! text lives in the individual `.md` files. This module owns loading, creating,
//! and saving that structure; the active chapter's text is held in a [`Buffer`]
//! alongside, not here.
//!
//! This is the foundation (issue #22). Navigation between chapters (#23),
//! structure editing (#24), rolled-up counts (#25), and the continuous view
//! (#26) build on it. The manifest is intentionally simple — a flat ordered
//! chapter list (a standalone novel is one book) — and will grow act/book/series
//! grouping as those tickets land.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The manifest file at the root of every project.
pub const MANIFEST_NAME: &str = "book.toml";

/// One chapter's entry in the manifest: its display title and the file that
/// holds its prose (relative to the project root).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChapterEntry {
    pub title: String,
    pub file: String,
}

/// The on-disk manifest (`book.toml`), deserialized.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub title: String,
    #[serde(default)]
    pub chapters: Vec<ChapterEntry>,
}

/// A loaded project: where it lives, its manifest, and which chapter is active.
/// The active chapter's text is edited through a separate [`Buffer`]; this type
/// only tracks structure.
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    /// Index into `manifest.chapters` of the chapter currently being edited.
    pub active: usize,
}

impl Project {
    /// The manifest path for a project directory.
    pub fn manifest_path(dir: &Path) -> PathBuf {
        dir.join(MANIFEST_NAME)
    }

    /// Open an existing project: read its manifest and select the first chapter.
    /// If the manifest somehow has no chapters, one is added so the project is
    /// always editable.
    pub fn open(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = dir.as_ref().to_path_buf();
        let text = std::fs::read_to_string(Self::manifest_path(&root))?;
        let manifest: Manifest = toml::from_str(&text).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid {MANIFEST_NAME}: {e}"))
        })?;

        let mut project = Self {
            root,
            manifest,
            active: 0,
        };
        if project.manifest.chapters.is_empty() {
            project.add_default_chapter()?;
        }
        Ok(project)
    }

    /// Initialize a new project in `dir` (creating the directory if needed): a
    /// manifest plus one empty starter chapter, ready to edit.
    pub fn create(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;

        let title = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());

        let mut project = Self {
            root,
            manifest: Manifest {
                title,
                chapters: Vec::new(),
            },
            active: 0,
        };
        project.add_default_chapter()?;
        Ok(project)
    }

    /// Append a starter chapter (creating its file if absent) and persist the
    /// manifest. Used by `create`, and by `open` as a safety net.
    fn add_default_chapter(&mut self) -> std::io::Result<()> {
        let n = self.manifest.chapters.len() + 1;
        let entry = ChapterEntry {
            title: format!("Chapter {n}"),
            file: format!("chapter-{n}.md"),
        };
        let path = self.root.join(&entry.file);
        if !path.exists() {
            std::fs::write(&path, "")?;
        }
        self.manifest.chapters.push(entry);
        self.active = self.manifest.chapters.len() - 1;
        self.save_manifest()
    }

    /// Path to a chapter's file by index.
    pub fn chapter_path(&self, index: usize) -> PathBuf {
        self.root.join(&self.manifest.chapters[index].file)
    }

    /// Path to the active chapter's file.
    pub fn active_path(&self) -> PathBuf {
        self.chapter_path(self.active)
    }

    /// Title of the active chapter, for the status bar.
    pub fn active_title(&self) -> Option<&str> {
        self.manifest
            .chapters
            .get(self.active)
            .map(|c| c.title.as_str())
    }

    /// Write the manifest back to `book.toml`.
    pub fn save_manifest(&self) -> std::io::Result<()> {
        let text = toml::to_string_pretty(&self.manifest).map_err(|e| {
            std::io::Error::other(format!("could not serialize manifest: {e}"))
        })?;
        std::fs::write(Self::manifest_path(&self.root), text)
    }

    // --- Structure editing -------------------------------------------------
    //
    // These mutate the manifest and chapter files but never touch `active` —
    // keeping the active index and the live buffer in sync is the caller's job
    // (see `App`). Each does the fallible filesystem work first, then mutates
    // the manifest and persists, so a failed file op leaves the project intact.

    /// Insert a new empty chapter titled `title` at position `at` (clamped to
    /// the end). Its file is created on disk.
    pub fn add_chapter(&mut self, at: usize, title: &str) -> std::io::Result<()> {
        let at = at.min(self.manifest.chapters.len());
        let file = self.unique_chapter_file();
        std::fs::write(self.root.join(&file), "")?;
        self.manifest.chapters.insert(
            at,
            ChapterEntry {
                title: title.to_string(),
                file,
            },
        );
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
            std::fs::create_dir_all(&unused)?;
            let dest = unique_dest(&unused, &self.manifest.chapters[at].file);
            std::fs::rename(&src, &dest)?;
        }
        self.manifest.chapters.remove(at);
        self.save_manifest()
    }

    /// Smallest `chapter-{n}.md` not already used by the manifest or on disk.
    fn unique_chapter_file(&self) -> String {
        let used: std::collections::HashSet<&str> =
            self.manifest.chapters.iter().map(|c| c.file.as_str()).collect();
        (1..)
            .map(|n| format!("chapter-{n}.md"))
            .find(|f| !used.contains(f.as_str()) && !self.root.join(f).exists())
            .expect("an unbounded range always yields a free name")
    }

    /// A minimal in-memory project (one chapter, no files on disk) for tests and
    /// headless rendering. Like real projects, it always has an active chapter.
    #[cfg(test)]
    pub fn scratch() -> Self {
        Self {
            root: PathBuf::from("."),
            manifest: Manifest {
                title: "untitled".into(),
                chapters: vec![ChapterEntry {
                    title: "Chapter 1".into(),
                    file: "chapter-1.md".into(),
                }],
            },
            active: 0,
        }
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
    use crate::buffer::Buffer;

    fn temp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("quill-proj-{}-{}", std::process::id(), name));
        p
    }

    #[test]
    fn create_initializes_manifest_and_first_chapter() {
        let dir = temp_dir("create");
        let _ = std::fs::remove_dir_all(&dir);
        let project = Project::create(&dir).unwrap();

        assert_eq!(project.manifest.chapters.len(), 1);
        assert_eq!(project.active_title(), Some("Chapter 1"));
        assert!(Project::manifest_path(&dir).exists());
        assert!(project.active_path().exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_then_open_round_trips_structure_and_text() {
        let dir = temp_dir("roundtrip");
        let _ = std::fs::remove_dir_all(&dir);

        // Create, write some prose into the active chapter, save it.
        let project = Project::create(&dir).unwrap();
        let mut buf = Buffer::from_path(project.active_path()).unwrap();
        for c in "# Opening\n\nThe night was dark.".chars() {
            buf.insert_char(c);
        }
        buf.save().unwrap();

        // Reopen and confirm structure + content survived.
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.manifest, project.manifest);
        let text = std::fs::read_to_string(reopened.active_path()).unwrap();
        assert!(text.contains("The night was dark."));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_parses_a_hand_written_manifest() {
        let dir = temp_dir("handwritten");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"Red State Earth\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "first\n").unwrap();
        std::fs::write(dir.join("two.md"), "second\n").unwrap();

        let project = Project::open(&dir).unwrap();
        assert_eq!(project.manifest.title, "Red State Earth");
        assert_eq!(project.manifest.chapters.len(), 2);
        assert_eq!(project.active, 0);
        assert_eq!(project.active_title(), Some("One"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_repairs_a_chapterless_manifest() {
        let dir = temp_dir("empty-manifest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Project::manifest_path(&dir), "title = \"Empty\"\n").unwrap();

        let project = Project::open(&dir).unwrap();
        assert_eq!(project.manifest.chapters.len(), 1);
        assert!(project.active_path().exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_rename_swap_remove_persist() {
        let dir = temp_dir("structure");
        let _ = std::fs::remove_dir_all(&dir);
        let mut project = Project::create(&dir).unwrap(); // "Chapter 1"

        // Add a second chapter after the first; file is created with a fresh name.
        project.add_chapter(1, "Second").unwrap();
        assert_eq!(project.manifest.chapters.len(), 2);
        assert_ne!(
            project.manifest.chapters[0].file,
            project.manifest.chapters[1].file
        );
        assert!(project.chapter_path(1).exists());

        // Rename and reorder.
        project.rename_chapter(0, "First").unwrap();
        project.swap_chapters(0, 1).unwrap();
        let order: Vec<&str> = project
            .manifest
            .chapters
            .iter()
            .map(|c| c.title.as_str())
            .collect();
        assert_eq!(order, vec!["Second", "First"]);

        // Reopen: every change persisted.
        let reopened = Project::open(&dir).unwrap();
        let order: Vec<String> = reopened
            .manifest
            .chapters
            .iter()
            .map(|c| c.title.clone())
            .collect();
        assert_eq!(order, vec!["Second".to_string(), "First".to_string()]);

        // Remove the first; its file moves to unused/ rather than vanishing.
        let removed_file = reopened.manifest.chapters[0].file.clone();
        let mut reopened = reopened;
        reopened.remove_chapter(0).unwrap();
        assert_eq!(reopened.manifest.chapters.len(), 1);
        assert!(dir.join("unused").join(&removed_file).exists());
        assert!(!dir.join(&removed_file).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unique_dest_avoids_clobbering() {
        let dir = temp_dir("unused-dest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.md"), "x").unwrap();

        assert_eq!(unique_dest(&dir, "b.md"), dir.join("b.md")); // free
        assert_eq!(unique_dest(&dir, "a.md"), dir.join("a-1.md")); // taken → suffixed

        std::fs::remove_dir_all(&dir).ok();
    }
}

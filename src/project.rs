//! A Quill project: a directory of per-chapter Markdown files plus a root
//! `book.toml` manifest that records the chapter order.
//!
//! The unit of work is always a project — there is no single-file mode. The
//! manifest is the source of truth for chapter order, titles, and a cached
//! per-chapter word count; the chapter text lives in the individual `.md` files.
//! This module owns loading, creating, and saving that structure. It does not
//! hold chapter text — the active chapter's text lives in a [`Buffer`] alongside
//! — but it does read each file once at open to populate the word counts (a
//! transient read per chapter, never N live buffers).
//!
//! This is the foundation (issue #22). Navigation between chapters (#23),
//! structure editing (#24), rolled-up counts (#25), and the continuous view
//! (#26) build on it. The manifest is intentionally simple — a flat ordered
//! chapter list (a standalone novel is one book) — and will grow act/book/series
//! grouping as those tickets land.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::buffer::count_words;
use crate::frontmatter::{self, Epigraph, FrontMatter, Status};

/// The manifest file at the root of every project.
pub const MANIFEST_NAME: &str = "book.toml";

/// One chapter's entry in the manifest: its display title, the file that holds
/// its prose (relative to the project root), and a cached word count.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChapterEntry {
    pub title: String,
    pub file: String,
    /// Cached word count, populated from the file at open and refreshed as the
    /// chapter is edited. Transient — never written to `book.toml` (`serde(skip)`),
    /// but it does participate in equality (compare structure explicitly if you
    /// need to ignore counts).
    #[serde(skip)]
    pub words: usize,
    /// Cached chapter metadata, read from the file's front matter at open and
    /// kept current as the author edits it. Transient like `words` — the chapter
    /// files are the source of truth; these power the pane without re-reading.
    #[serde(skip)]
    pub status: Option<Status>,
    #[serde(skip)]
    pub summary: Option<String>,
    #[serde(skip)]
    pub pov: Option<String>,
}

/// The on-disk manifest (`book.toml`), deserialized.
///
/// `title` is the book title; `subtitle`, `author`, and `series` are optional
/// book/series metadata. New optional fields default to absent so manifests
/// written before they existed still parse, and unset fields aren't written
/// back. Scalars are declared before `chapters` so the TOML array-of-tables
/// stays last (a TOML requirement).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    /// Book epigraph: a quotation (`epigraph`) and optional attribution
    /// (`epigraph_by`), rendered at the start of the manuscript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epigraph: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epigraph_by: Option<String>,
    #[serde(default)]
    pub chapters: Vec<ChapterEntry>,
}

impl Manifest {
    /// The book epigraph, if a quotation is set.
    pub fn epigraph(&self) -> Option<Epigraph> {
        Epigraph::from_parts(self.epigraph.clone(), self.epigraph_by.clone())
    }
}

/// A loaded project: where it lives, its manifest (structure + cached word
/// counts), and which chapter is active. The active chapter's text is edited
/// through a separate [`Buffer`]; this type holds no chapter text.
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    /// The active chapter, tracked by file identity rather than index: a
    /// structural edit (insert/move/remove) reorders the list but the active
    /// chapter is still the same file, so [`active_index`](Self::active_index)
    /// re-derives the position with no per-op bookkeeping.
    active: String,
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
        let text = fs::read_to_string(Self::manifest_path(&root))?;
        let manifest: Manifest = toml::from_str(&text).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid {MANIFEST_NAME}: {e}"))
        })?;

        let mut project = Self {
            root,
            manifest,
            active: String::new(),
        };
        if project.manifest.chapters.is_empty() {
            project.add_default_chapter()?;
        } else {
            project.active = project.manifest.chapters[0].file.clone();
            // Populate per-chapter caches (word count + front-matter metadata)
            // from disk: one transient read each, never N live buffers.
            for i in 0..project.manifest.chapters.len() {
                project.read_chapter_cache(i);
            }
        }
        Ok(project)
    }

    /// Initialize a new project in `dir` (creating the directory if needed): a
    /// manifest plus one empty starter chapter, ready to edit.
    pub fn create(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = dir.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;

        let title = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());

        let mut project = Self {
            root,
            manifest: Manifest {
                title,
                subtitle: None,
                author: None,
                series: None,
                epigraph: None,
                epigraph_by: None,
                chapters: Vec::new(),
            },
            active: String::new(),
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
            words: 0,
            status: None,
            summary: None,
            pov: None,
        };
        let path = self.root.join(&entry.file);
        if !path.exists() {
            fs::write(&path, "")?;
        }
        self.active = entry.file.clone();
        self.manifest.chapters.push(entry);
        self.save_manifest()
    }

    /// Path to a chapter's file by index.
    pub fn chapter_path(&self, index: usize) -> PathBuf {
        self.root.join(&self.manifest.chapters[index].file)
    }

    /// Index of the active chapter — derived from its file identity, so it stays
    /// correct after any structural edit. Falls back to 0 if the active file is
    /// momentarily absent (e.g. just after deleting it, before a new one is set).
    pub fn active_index(&self) -> usize {
        self.manifest
            .chapters
            .iter()
            .position(|c| c.file == self.active)
            .unwrap_or(0)
    }

    /// Make chapter `index` the active one.
    pub fn set_active(&mut self, index: usize) {
        if let Some(chapter) = self.manifest.chapters.get(index) {
            self.active = chapter.file.clone();
        }
    }

    /// Cached word count for chapter `index` (0 if out of range).
    pub fn chapter_words(&self, index: usize) -> usize {
        self.manifest.chapters.get(index).map_or(0, |c| c.words)
    }

    /// Refresh the cached word count for chapter `index` (no-op if out of range).
    /// The continuous view uses this to update each chapter it saves.
    pub fn set_chapter_words(&mut self, index: usize, words: usize) {
        if let Some(chapter) = self.manifest.chapters.get_mut(index) {
            chapter.words = words;
        }
    }

    /// Read chapter `index` from disk once and refresh its cached word count and
    /// front-matter metadata (status / summary / POV). Used at open.
    fn read_chapter_cache(&mut self, index: usize) {
        let text = fs::read_to_string(self.chapter_path(index)).unwrap_or_default();
        let (front, body) = frontmatter::split(&text);
        let words = count_words(body);
        let fm = front.map(FrontMatter::from_raw);
        if let Some(chapter) = self.manifest.chapters.get_mut(index) {
            chapter.words = words;
        }
        // One place owns the front-matter → cache mapping.
        self.cache_chapter_meta(index, fm.as_ref());
    }

    /// The front matter of chapter `index` as written on disk (`None` if the
    /// file has no block). For reading a chapter's metadata that isn't cached —
    /// keep it off the per-frame path.
    pub fn chapter_front_matter(&self, index: usize) -> Option<FrontMatter> {
        let text = fs::read_to_string(self.chapter_path(index)).unwrap_or_default();
        let (front, _) = frontmatter::split(&text);
        front.map(FrontMatter::from_raw)
    }

    /// Cached metadata accessors for the pane (no file I/O).
    pub fn chapter_status(&self, index: usize) -> Option<Status> {
        self.manifest.chapters.get(index).and_then(|c| c.status)
    }

    pub fn chapter_summary(&self, index: usize) -> Option<&str> {
        self.manifest
            .chapters
            .get(index)
            .and_then(|c| c.summary.as_deref())
    }

    pub fn chapter_pov(&self, index: usize) -> Option<&str> {
        self.manifest
            .chapters
            .get(index)
            .and_then(|c| c.pov.as_deref())
    }

    /// Copy a chapter's managed metadata into its cache from a [`FrontMatter`]
    /// (used after the active chapter's front matter is edited in its buffer).
    pub fn cache_chapter_meta(&mut self, index: usize, front: Option<&FrontMatter>) {
        if let Some(chapter) = self.manifest.chapters.get_mut(index) {
            chapter.status = front.and_then(|f| f.status());
            chapter.summary = front.and_then(|f| f.summary());
            chapter.pov = front.and_then(|f| f.pov());
        }
    }

    /// Edit a (non-active) chapter's front matter on disk: read the file, apply
    /// `f` to its [`FrontMatter`], write it back leaving the prose body untouched,
    /// and refresh the cache. The body is preserved verbatim — only the front
    /// matter region changes.
    pub fn edit_chapter_front_matter(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut FrontMatter),
    ) -> std::io::Result<()> {
        let path = self.chapter_path(index);
        let text = fs::read_to_string(&path).unwrap_or_default();
        let (front, body) = frontmatter::split(&text);
        let mut fm = front.map(FrontMatter::from_raw).unwrap_or_default();
        f(&mut fm);
        let front = (!fm.is_empty()).then(|| fm.as_str());
        fs::write(&path, frontmatter::join(front, body).as_bytes())?;
        self.cache_chapter_meta(index, Some(&fm));
        Ok(())
    }

    /// Refresh the active chapter's cached word count (called after editing it).
    pub fn set_active_words(&mut self, words: usize) {
        self.set_chapter_words(self.active_index(), words);
    }

    /// Path to the active chapter's file.
    pub fn active_path(&self) -> PathBuf {
        self.chapter_path(self.active_index())
    }

    /// Title of the active chapter, for the status bar.
    pub fn active_title(&self) -> Option<&str> {
        self.chapter_title(self.active_index())
    }

    /// Title of chapter `index` (None if out of range).
    pub fn chapter_title(&self, index: usize) -> Option<&str> {
        self.manifest.chapters.get(index).map(|c| c.title.as_str())
    }

    /// Write the manifest back to `book.toml`.
    pub fn save_manifest(&self) -> std::io::Result<()> {
        let text = toml::to_string_pretty(&self.manifest).map_err(|e| {
            std::io::Error::other(format!("could not serialize manifest: {e}"))
        })?;
        fs::write(Self::manifest_path(&self.root), text)
    }

    // --- Structure editing -------------------------------------------------
    //
    // These mutate the manifest and chapter files but never touch `active` — it
    // tracks the active chapter by file identity, so the index re-derives itself
    // (the one exception, deleting the active chapter, is handled in `App`). Each
    // does the fallible filesystem work first, then mutates the manifest and
    // persists, so a failed file op leaves the project intact.

    /// Insert a new empty chapter titled `title` at position `at` (clamped to
    /// the end). Its file is created on disk.
    pub fn add_chapter(&mut self, at: usize, title: &str) -> std::io::Result<()> {
        let at = at.min(self.manifest.chapters.len());
        let file = self.unique_chapter_file();
        fs::write(self.root.join(&file), "")?;
        self.manifest.chapters.insert(
            at,
            ChapterEntry {
                title: title.to_string(),
                file,
                words: 0,
                status: None,
                summary: None,
                pov: None,
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
            fs::create_dir_all(&unused)?;
            let dest = unique_dest(&unused, &self.manifest.chapters[at].file);
            fs::rename(&src, &dest)?;
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
        Self::scratch_with(&["Chapter 1"])
    }

    /// An in-memory project with the given chapter titles (no files on disk),
    /// for tests and headless rendering. Always has an active chapter (the first).
    #[cfg(test)]
    pub fn scratch_with(titles: &[&str]) -> Self {
        let chapters: Vec<ChapterEntry> = titles
            .iter()
            .enumerate()
            .map(|(i, title)| ChapterEntry {
                title: (*title).to_string(),
                file: format!("chapter-{}.md", i + 1),
                words: 0,
                status: None,
                summary: None,
                pov: None,
            })
            .collect();
        let active = chapters[0].file.clone();
        Self {
            root: PathBuf::from("."),
            manifest: Manifest {
                title: "untitled".into(),
                subtitle: None,
                author: None,
                series: None,
                epigraph: None,
                epigraph_by: None,
                chapters,
            },
            active,
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
        let _ = fs::remove_dir_all(&dir);
        let project = Project::create(&dir).unwrap();

        assert_eq!(project.manifest.chapters.len(), 1);
        assert_eq!(project.active_title(), Some("Chapter 1"));
        assert!(Project::manifest_path(&dir).exists());
        assert!(project.active_path().exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_then_open_round_trips_structure_and_text() {
        let dir = temp_dir("roundtrip");
        let _ = fs::remove_dir_all(&dir);

        // Create, write some prose into the active chapter, save it.
        let project = Project::create(&dir).unwrap();
        let mut buf = Buffer::from_path(project.active_path()).unwrap();
        for c in "# Opening\n\nThe night was dark.".chars() {
            buf.insert_char(c);
        }
        buf.save().unwrap();

        // Reopen and confirm the structure (titles + files) and content
        // survived. Compare the persisted projection, not the whole manifest:
        // `reopened` has word counts from disk while `project`'s are still 0.
        let reopened = Project::open(&dir).unwrap();
        let structure = |p: &Project| -> Vec<(String, String)> {
            p.manifest
                .chapters
                .iter()
                .map(|c| (c.title.clone(), c.file.clone()))
                .collect()
        };
        assert_eq!(reopened.manifest.title, project.manifest.title);
        assert_eq!(structure(&reopened), structure(&project));
        let text = fs::read_to_string(reopened.active_path()).unwrap();
        assert!(text.contains("The night was dark."));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_parses_a_hand_written_manifest() {
        let dir = temp_dir("handwritten");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Project::manifest_path(&dir),
            "title = \"Red State Earth\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
        )
        .unwrap();
        fs::write(dir.join("one.md"), "first\n").unwrap();
        fs::write(dir.join("two.md"), "second\n").unwrap();

        let project = Project::open(&dir).unwrap();
        assert_eq!(project.manifest.title, "Red State Earth");
        assert_eq!(project.manifest.chapters.len(), 2);
        assert_eq!(project.active_index(), 0);
        assert_eq!(project.active_title(), Some("One"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn book_metadata_persists_and_old_manifests_still_parse() {
        let dir = temp_dir("book-meta");
        let _ = fs::remove_dir_all(&dir);

        // A manifest written before these fields existed still opens.
        let mut project = Project::create(&dir).unwrap();
        assert_eq!(project.manifest.subtitle, None);

        // Set the new fields and persist.
        project.manifest.subtitle = Some("A Novel".into());
        project.manifest.author = Some("A. Writer".into());
        project.manifest.series = Some("The Saga".into());
        project.save_manifest().unwrap();

        // The written TOML carries them, and they survive a reopen.
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(toml.contains("author = \"A. Writer\""), "manifest:\n{toml}");
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.manifest.subtitle.as_deref(), Some("A Novel"));
        assert_eq!(reopened.manifest.author.as_deref(), Some("A. Writer"));
        assert_eq!(reopened.manifest.series.as_deref(), Some("The Saga"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn book_epigraph_persists_and_builds() {
        let dir = temp_dir("book-epigraph");
        let _ = fs::remove_dir_all(&dir);
        let mut project = Project::create(&dir).unwrap();
        project.manifest.epigraph = Some("Call me Ishmael.".into());
        project.manifest.epigraph_by = Some("Melville".into());
        project.save_manifest().unwrap();

        let reopened = Project::open(&dir).unwrap();
        let ep = reopened.manifest.epigraph().unwrap();
        assert_eq!(ep.text, "Call me Ishmael.");
        assert_eq!(ep.attribution.as_deref(), Some("Melville"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_multiline_book_epigraph_round_trips_through_the_manifest() {
        let dir = temp_dir("book-epigraph-multi");
        let _ = fs::remove_dir_all(&dir);
        let mut project = Project::create(&dir).unwrap();
        project.manifest.epigraph = Some("Line one\nLine two\nLine three".into());
        project.save_manifest().unwrap();

        // toml serializes the multi-line string as a readable `"""` block.
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(toml.contains("epigraph = \"\"\""), "expected a multi-line literal:\n{toml}");

        let reopened = Project::open(&dir).unwrap();
        assert_eq!(
            reopened.manifest.epigraph().unwrap().text,
            "Line one\nLine two\nLine three"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_repairs_a_chapterless_manifest() {
        let dir = temp_dir("empty-manifest");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(Project::manifest_path(&dir), "title = \"Empty\"\n").unwrap();

        let project = Project::open(&dir).unwrap();
        assert_eq!(project.manifest.chapters.len(), 1);
        assert!(project.active_path().exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_rename_swap_remove_persist() {
        let dir = temp_dir("structure");
        let _ = fs::remove_dir_all(&dir);
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

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unique_dest_avoids_clobbering() {
        let dir = temp_dir("unused-dest");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.md"), "x").unwrap();

        assert_eq!(unique_dest(&dir, "b.md"), dir.join("b.md")); // free
        assert_eq!(unique_dest(&dir, "a.md"), dir.join("a-1.md")); // taken → suffixed

        fs::remove_dir_all(&dir).ok();
    }

    /// Performance guard (run with `cargo test --release -- --ignored
    /// --nocapture`). Opening a project reads and parses every chapter file once
    /// (word count + front matter); even a large novel must open near-instantly.
    /// Catches a regression to per-chapter re-reads or O(n²) work at open.
    #[test]
    #[ignore]
    fn open_is_fast_for_a_large_novel() {
        use std::time::Instant;

        let dir = temp_dir("perf-open");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut manifest = String::from("title = \"Big\"\n\n");
        for i in 0..150 {
            let file = format!("ch{i}.md");
            manifest.push_str(&format!("[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"));
            let mut body = String::from("---\nstatus: draft\npov: Mara\n---\n# Chapter\n\n");
            for _ in 0..50 {
                body.push_str("The night was dark and the road was very long indeed.\n");
            }
            fs::write(dir.join(&file), body).unwrap();
        }
        fs::write(Project::manifest_path(&dir), manifest).unwrap();

        let t = Instant::now();
        let project = Project::open(&dir).unwrap();
        let dt = t.elapsed();

        println!("open 150 chapters (front matter + word counts) = {dt:?}");
        assert_eq!(project.manifest.chapters.len(), 150);
        assert!(dt.as_millis() < 250, "open took {dt:?}");

        fs::remove_dir_all(&dir).ok();
    }
}

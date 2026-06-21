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
use crate::frontmatter::{self, Band, Epigraph, FrontMatter, Role, Status};

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
    /// Cached chapter word-count goal (#146), read from front matter at open and
    /// kept current as the author edits it. Transient like the fields above — the
    /// chapter file is the source of truth.
    #[serde(skip)]
    pub goal: Option<usize>,
    /// Cached section role (#179): front- or back-matter type, or `None` for a
    /// plain body chapter. Transient — read from front matter at open.
    #[serde(skip)]
    pub role: Option<Role>,
}

/// The author's writing goals (see [`docs/progress-reports.md`]). Pages are
/// always derived from words via `words_per_page`, never tracked directly.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Goals {
    /// Target word count.
    pub words: usize,
    /// Target chapter count.
    pub chapters: usize,
    /// Words per page (pages = words / words_per_page). Defaults to a standard
    /// manuscript page (250); override for e.g. mass-market or children's books.
    #[serde(default = "default_words_per_page")]
    pub words_per_page: usize,
}

fn default_words_per_page() -> usize {
    250
}

/// A named, inclusive range of chapters — a grouping layer between book and
/// chapter (see [`docs/progress-reports.md`]). `chapters` is `[start, end]`,
/// 1-based and inclusive. `name` is optional; the report auto-labels an unnamed
/// part `"{label} {n}"` using [`Manifest::part_label`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Part {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Inclusive 1-based chapter range `[start, end]`.
    pub chapters: [usize; 2],
    /// Optional word-count goal for the part (#146), set in `book.toml`. When
    /// present, the progress report shows the part's words against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<usize>,
}

/// The on-disk manifest (`book.toml`), deserialized.
///
/// `title` is the book title; `subtitle`, `author`, and `series` are optional
/// book/series metadata. New optional fields default to absent so manifests
/// written before they existed still parse, and unset fields aren't written
/// back. Scalar keys are declared before the `goals` table and the `chapters`
/// array, so the TOML tables stay last (a TOML requirement).
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
    /// Date the project was started (`YYYY-MM-DD`), anchoring the progress
    /// report's "days since start" and writing rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    /// TODO token counted on the report's `xxx:` line (case-insensitive).
    /// Defaults to `xxx` (see [`Manifest::todo_marker`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_marker: Option<String>,
    /// The displayed term for a chapter grouping ("part", "act", "book", …).
    /// Defaults to `part` (see [`Manifest::part_label`]). A scalar key, so it
    /// must precede the `goals`/`parts`/`chapters` tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_label: Option<String>,
    /// Sentence length (in words) above which the analysis report flags a
    /// possible run-on. Defaults to 40 (see [`Manifest::run_on_threshold`]). A
    /// scalar key — keep it before the `goals`/`parts`/`chapters` tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_on_threshold: Option<usize>,
    /// The author's writing goals; the progress report is empty without them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goals: Option<Goals>,
    /// Optional grouping of chapters into named, inclusive ranges. When absent,
    /// the report's per-part block is not rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<Part>>,
    #[serde(default)]
    pub chapters: Vec<ChapterEntry>,
}

impl Manifest {
    /// The book epigraph, if a quotation is set.
    pub fn epigraph(&self) -> Option<Epigraph> {
        Epigraph::from_parts(self.epigraph.clone(), self.epigraph_by.clone())
    }

    /// How many leading chapters are front matter (#185) — the offset of the
    /// contiguous body block after the band reorder. The single source for the
    /// front-to-body mapping the parts logic relies on.
    pub fn front_chapter_count(&self) -> usize {
        self.chapters.iter().filter(|c| Band::of(c.role) == Band::Front).count()
    }

    /// The configured TODO marker, defaulting to `xxx`.
    pub fn todo_marker(&self) -> &str {
        self.todo_marker.as_deref().unwrap_or("xxx")
    }

    /// The displayed term for a chapter grouping, defaulting to `part`.
    pub fn part_label(&self) -> &str {
        self.part_label.as_deref().unwrap_or("part")
    }
    /// The run-on flag threshold for the analysis report (default 40 words).
    pub fn run_on_threshold(&self) -> usize {
        self.run_on_threshold.unwrap_or(40)
    }

    /// The index of the part containing chapter `chapter_index` (0-based), or
    /// `None` if it's not in any part. Part ranges are 1-based positions among the
    /// **body chapters** (#185) — front/back-matter sections belong to no part.
    pub fn part_of(&self, chapter_index: usize) -> Option<usize> {
        if Band::of(self.chapters.get(chapter_index)?.role) != Band::Body {
            return None; // sections aren't part of a part
        }
        // Body chapters are contiguous after the band reorder, so the body
        // position is the offset past the front matter (saturating: a Body chapter
        // always sits at or after `front`, but don't panic if that ever drifts).
        let n = chapter_index.saturating_sub(self.front_chapter_count()) + 1;
        self.parts.as_ref()?.iter().position(|p| p.chapters[0] <= n && n <= p.chapters[1])
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
            // Populate per-chapter caches (word count + front-matter metadata,
            // including the section role) from disk: one transient read each,
            // never N live buffers.
            for i in 0..project.manifest.chapters.len() {
                project.read_chapter_cache(i);
            }
            // Order front matter → body → back matter in memory, whatever order
            // the manifest listed them in (#180).
            project.reorder_by_band();
            project.active = project.manifest.chapters[0].file.clone();
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
                start_date: None,
                todo_marker: None,
                part_label: None,
                run_on_threshold: None,
                goals: None,
                parts: None,
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
            goal: None,
            role: None,
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

    /// The chapter's file name relative to the project root ("" if out of
    /// range) — a stable identity for keying per-chapter bookkeeping.
    pub fn chapter_file(&self, index: usize) -> &str {
        self.manifest.chapters.get(index).map_or("", |c| c.file.as_str())
    }

    /// Index of the active chapter — derived from its file identity, so it stays
    /// correct after any structural edit. Falls back to 0 if the active file is
    /// momentarily absent (e.g. just after deleting it, before a new one is set).
    pub fn active_index(&self) -> usize {
        self.index_of_file(&self.active).unwrap_or(0)
    }

    /// The index of the chapter whose file is `file`, if any.
    pub fn index_of_file(&self, file: &str) -> Option<usize> {
        self.manifest.chapters.iter().position(|c| c.file == file)
    }

    /// The project's reference files (#165): every Markdown file under the root —
    /// the story bible (`bible/**`) and any loose notes — that isn't a chapter or
    /// in `unused/`. These are the files an author can open in a side pane to read
    /// or edit alongside the prose. Sorted by path for a stable list.
    pub fn reference_files(&self) -> Vec<PathBuf> {
        let chapters: Vec<PathBuf> =
            (0..self.manifest.chapters.len()).map(|i| self.chapter_path(i)).collect();
        let mut found = Vec::new();
        collect_markdown(&self.root, &chapters, &mut found);
        found.sort();
        found
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
        let text = self.read_chapter(index);
        let (front, body) = frontmatter::split(&text);
        let words = count_words(body);
        let fm = front.map(FrontMatter::from_raw);
        if let Some(chapter) = self.manifest.chapters.get_mut(index) {
            chapter.words = words;
        }
        // One place owns the front-matter → cache mapping.
        self.cache_chapter_meta(index, fm.as_ref());
    }

    /// Read chapter `index`'s file from disk, or "" if it can't be read. The
    /// shared front of the off-the-per-frame-path chapter readers.
    fn read_chapter(&self, index: usize) -> String {
        fs::read_to_string(self.chapter_path(index)).unwrap_or_default()
    }

    /// The front matter of chapter `index` as written on disk (`None` if the
    /// file has no block). For reading a chapter's metadata that isn't cached —
    /// keep it off the per-frame path.
    pub fn chapter_front_matter(&self, index: usize) -> Option<FrontMatter> {
        let text = self.read_chapter(index);
        let (front, _) = frontmatter::split(&text);
        front.map(FrontMatter::from_raw)
    }

    /// The prose body of chapter `index` from disk (front matter stripped),
    /// empty if it can't be read. Off the per-frame path (used by the report).
    pub fn chapter_body(&self, index: usize) -> String {
        frontmatter::split(&self.read_chapter(index)).1.to_string()
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

    /// The chapter's word-count goal (#146), if set.
    pub fn chapter_goal(&self, index: usize) -> Option<usize> {
        self.manifest.chapters.get(index).and_then(|c| c.goal)
    }

    /// The file's section role (#179), if set; `None` is a plain body chapter.
    pub fn chapter_role(&self, index: usize) -> Option<Role> {
        self.manifest.chapters.get(index).and_then(|c| c.role)
    }

    /// The ordering band a file sits in (#180): front matter, body, or back matter.
    pub fn chapter_band(&self, index: usize) -> Band {
        Band::of(self.chapter_role(index))
    }

    /// How many files are body chapters — the count the progress report measures
    /// against the chapter goal, excluding front/back matter (#180).
    pub fn body_chapter_count(&self) -> usize {
        self.manifest.chapters.iter().filter(|c| Band::of(c.role) == Band::Body).count()
    }

    /// Sort chapters into reading order — front matter, body, back matter (#180) —
    /// preserving the author's order within each band (a stable sort). Returns
    /// whether the order actually changed.
    ///
    /// The body block lands contiguous after the front matter, which is what lets
    /// the report's `parts` ranges map cleanly to body-chapter positions (#185).
    pub fn reorder_by_band(&mut self) -> bool {
        // A stable sort, so "changed" is exactly "wasn't already in band order" —
        // checked without cloning the file list.
        if self.manifest.chapters.is_sorted_by_key(|c| Band::of(c.role)) {
            return false;
        }
        self.manifest.chapters.sort_by_key(|c| Band::of(c.role));
        true
    }

    /// Copy a chapter's managed metadata into its cache from a [`FrontMatter`]
    /// (used after the active chapter's front matter is edited in its buffer).
    pub fn cache_chapter_meta(&mut self, index: usize, front: Option<&FrontMatter>) {
        if let Some(chapter) = self.manifest.chapters.get_mut(index) {
            chapter.status = front.and_then(|f| f.status());
            chapter.summary = front.and_then(|f| f.summary());
            chapter.pov = front.and_then(|f| f.pov());
            chapter.goal = front.and_then(|f| f.goal());
            chapter.role = front.and_then(|f| f.role());
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
                goal: None,
                role: None,
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
                goal: None,
                role: None,
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
                start_date: None,
                todo_marker: None,
                part_label: None,
                run_on_threshold: None,
                goals: None,
                parts: None,
                chapters,
            },
            active,
        }
    }
}

/// Recursively gather `.md` files under `dir` into `out`, skipping the `unused/`
/// subtree and any `chapters` (which the chapter browser already lists). Best
/// effort: an unreadable directory is silently skipped.
fn collect_markdown(dir: &Path, chapters: &[PathBuf], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Generated/scratch trees aren't reference material: `unused/` (deleted
            // chapters) and `export/` (compiled output, #190).
            let name = path.file_name().and_then(|n| n.to_str());
            if !matches!(name, Some("unused") | Some("export")) {
                collect_markdown(&path, chapters, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") && !chapters.contains(&path) {
            out.push(path);
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
    fn goals_round_trip_through_the_manifest() {
        let dir = temp_dir("goals");
        let _ = fs::remove_dir_all(&dir);
        let mut project = Project::create(&dir).unwrap();
        assert!(project.manifest.goals.is_none());
        assert_eq!(project.manifest.todo_marker(), "xxx"); // default

        project.manifest.start_date = Some("2026-01-11".into());
        project.manifest.goals = Some(Goals {
            words: 150_000,
            chapters: 60,
            words_per_page: 250,
        });
        project.save_manifest().unwrap();

        // The `[goals]` table serializes after the scalars and before
        // `[[chapters]]` (valid TOML).
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(
            toml.find("[goals]").unwrap() < toml.find("[[chapters]]").unwrap(),
            "goals must precede chapters:\n{toml}"
        );

        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.manifest.start_date.as_deref(), Some("2026-01-11"));
        assert_eq!(reopened.manifest.goals.unwrap().words, 150_000);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parts_round_trip_and_map_chapters_to_ranges() {
        let dir = temp_dir("parts");
        let _ = fs::remove_dir_all(&dir);
        let mut project = Project::create(&dir).unwrap(); // 1 starter chapter
        assert!(project.manifest.parts.is_none());
        assert_eq!(project.manifest.part_label(), "part"); // default
        assert_eq!(project.manifest.part_of(0), None); // no parts → no mapping

        // Four body chapters for the two parts to cover.
        project.add_chapter(1, "Two").unwrap();
        project.add_chapter(2, "Three").unwrap();
        project.add_chapter(3, "Four").unwrap();
        project.manifest.part_label = Some("act".into());
        project.manifest.parts = Some(vec![
            Part { name: Some("The Gathering".into()), chapters: [1, 2], goal: Some(20_000) },
            Part { name: None, chapters: [3, 4], goal: None },
        ]);
        project.save_manifest().unwrap();

        // `[[parts]]` serializes after `part_label` (a scalar) and before
        // `[[chapters]]` (valid TOML: scalars precede arrays-of-tables).
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(
            toml.find("part_label").unwrap() < toml.find("[[parts]]").unwrap(),
            "part_label must precede [[parts]]:\n{toml}"
        );
        assert!(
            toml.find("[[parts]]").unwrap() < toml.find("[[chapters]]").unwrap(),
            "parts must precede chapters:\n{toml}"
        );

        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.manifest.part_label(), "act");
        let parts = reopened.manifest.parts.as_ref().unwrap();
        assert_eq!(parts[0].name.as_deref(), Some("The Gathering"));
        assert_eq!(parts[0].chapters, [1, 2]);
        assert_eq!(parts[0].goal, Some(20_000), "part goal round-trips (#146)");
        assert_eq!(parts[1].goal, None);
        // Chapter index 0 (chapter 1) is in part 0; index 2 (chapter 3) in part 1.
        assert_eq!(reopened.manifest.part_of(0), Some(0));
        assert_eq!(reopened.manifest.part_of(2), Some(1));
        // A chapter outside every range maps to no part.
        assert_eq!(reopened.manifest.part_of(9), None);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn words_per_page_defaults_to_a_standard_page() {
        let dir = temp_dir("wpp-default");
        let _ = fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A goals block that omits words_per_page (the common case).
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"B\"\n\n[goals]\nwords = 100000\nchapters = 40\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "body\n").unwrap();

        let project = Project::open(&dir).unwrap();
        assert_eq!(project.manifest.goals.unwrap().words_per_page, 250);

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

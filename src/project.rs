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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::buffer::count_words;
use crate::frontmatter::{self, Band, Epigraph, FrontMatter, Role, Status};

mod series;
mod structure;

use series::Book;

/// The manifest file at the root of every project.
pub const MANIFEST_NAME: &str = "book.toml";

/// The per-project custom spell-check dictionary file (#358), at the project root.
pub const DICTIONARY_NAME: &str = "dictionary.txt";

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

impl ChapterEntry {
    /// A new entry with the given title and file; the transient caches start
    /// empty (populated from the file at open and as the chapter is edited).
    fn new(title: String, file: String) -> Self {
        Self { title, file, words: 0, status: None, summary: None, pov: None, goal: None, role: None }
    }

    /// Refresh the cached front-matter metadata from `front` (none clears it).
    /// The chapter file is the source of truth; this populates the transient
    /// caches that power the pane, counts, and tree without re-reading.
    fn cache_meta(&mut self, front: Option<&FrontMatter>) {
        self.status = front.and_then(FrontMatter::status);
        self.summary = front.and_then(FrontMatter::summary);
        self.pov = front.and_then(FrontMatter::pov);
        self.goal = front.and_then(FrontMatter::goal);
        self.role = front.and_then(FrontMatter::role);
    }
}

/// Read `entry`'s file under `root` and refresh its cached word count and
/// front-matter metadata (#331). Shared by the active book's caching and the
/// other books of a series, so every book's counts/roles are populated at open
/// without re-reading later. One file read; never holds the body text.
fn cache_chapter(root: &Path, entry: &mut ChapterEntry) {
    let text = fs::read_to_string(root.join(&entry.file)).unwrap_or_default();
    let (front, body) = frontmatter::split(&text);
    entry.words = count_words(body);
    entry.cache_meta(front.map(FrontMatter::from_raw).as_ref());
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
    /// Optional daily word-count target (#277) — today's progress is shown
    /// against it on the progress report. Absent when unset (the report is then
    /// unchanged); old manifests without the key still parse and round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<usize>,
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
///
/// For a multi-book series (#328), this holds the *active* book's structure;
/// `title` is then the active book's title, `series` is the series title, and the
/// full book list lives on [`Project`].
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
        front_chapter_count(&self.chapters)
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
        part_at_body_position(self.parts.as_deref()?, n)
    }
}

/// Count of leading front-matter chapters (#185) in a band-ordered chapter list.
/// Shared by [`Manifest::front_chapter_count`] and the series tree (#330).
pub(crate) fn front_chapter_count(chapters: &[ChapterEntry]) -> usize {
    chapters.iter().filter(|c| Band::of(c.role) == Band::Front).count()
}

/// The 0-based part containing 1-based **body** position `body_pos`, if any. Part
/// ranges are inclusive 1-based positions among the body chapters (#185). Shared
/// by [`Manifest::part_of`] and the series tree (#330) so the mapping can't drift.
pub(crate) fn part_at_body_position(parts: &[Part], body_pos: usize) -> Option<usize> {
    parts.iter().position(|p| p.chapters[0] <= body_pos && body_pos <= p.chapters[1])
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
    /// Files whose cached word count changed since the daily tally last drained
    /// this (#220). [`set_chapter_words`](Self::set_chapter_words) is the only
    /// place counts change, so recording here lets the tally touch just the
    /// chapters that moved — not the whole project — every idle tick. Keyed by
    /// file so it survives reordering.
    words_changed: HashSet<String>,
    /// The books of a multi-book series (#328), in series order. **Empty for a
    /// standalone novel** — its single book lives entirely in `manifest`. When
    /// non-empty, the entry at `active_book` mirrors `manifest` (kept in sync on
    /// save); the others hold their structure until activated (#329).
    books: Vec<Book>,
    /// Index of the active book within `books` (0 for a standalone novel).
    active_book: usize,
    /// The per-project custom spell-check dictionary (#358): accepted words —
    /// invented names, places, coined terms — lowercased for case-insensitive
    /// matching. Loaded from `dictionary.txt` at the project root at open.
    dictionary: HashSet<String>,
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
        let invalid = |e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid {MANIFEST_NAME}: {e}"))
        };
        // A `books` array marks a multi-book series (#328); anything else is the
        // flat single-book manifest, parsed and saved exactly as before. Peek via
        // `toml::Value` so the single-book path is byte-for-byte unchanged. The
        // series shape lives in `series.rs` (#350).
        let value: toml::Value = toml::from_str(&text).map_err(invalid)?;
        let (manifest, books, active_book) = Manifest::from_toml_value(value).map_err(invalid)?;

        let dictionary = load_dictionary(&root);
        let mut project = Self {
            root,
            manifest,
            active: String::new(),
            words_changed: HashSet::new(),
            books,
            active_book,
            dictionary,
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
        // Cache the other books' chapters too (#331), so book and series totals
        // are real without re-reading later. One file read per chapter; the
        // active book is already cached above (it lives in the manifest).
        let active = project.active_book;
        let root = project.root.clone();
        for (b, book) in project.books.iter_mut().enumerate() {
            if b == active {
                continue;
            }
            for entry in &mut book.chapters {
                cache_chapter(&root, entry);
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
                start_date: None,
                todo_marker: None,
                part_label: None,
                run_on_threshold: None,
                goals: None,
                parts: None,
                chapters: Vec::new(),
            },
            active: String::new(),
            words_changed: HashSet::new(),
            books: Vec::new(),
            active_book: 0,
            dictionary: HashSet::new(),
        };
        project.add_default_chapter()?;
        Ok(project)
    }

    /// Append a starter chapter (creating its file if absent) and persist the
    /// manifest. Used by `create`, and by `open` as a safety net.
    fn add_default_chapter(&mut self) -> std::io::Result<()> {
        let n = self.manifest.chapters.len() + 1;
        let entry = ChapterEntry::new(format!("Chapter {n}"), format!("chapter-{n}.md"));
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

    /// The active chapter's file (its identity), independent of its index.
    pub fn active_file(&self) -> &str {
        &self.active
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
            if chapter.words != words {
                chapter.words = words;
                // Record the change so the daily tally re-checks just this chapter
                // (#220) rather than the whole project.
                self.words_changed.insert(chapter.file.clone());
            }
        }
    }

    /// Take the files whose cached word count changed since the last call — what
    /// the daily tally needs to re-check (#220).
    pub fn take_words_changed(&mut self) -> HashSet<String> {
        std::mem::take(&mut self.words_changed)
    }

    /// Discard any pending word-count changes — used when the daily tally
    /// re-baselines everything (a structural edit), so nothing stale carries over.
    pub fn clear_words_changed(&mut self) {
        self.words_changed.clear();
    }

    /// Read chapter `index` from disk once and refresh its cached word count and
    /// front-matter metadata (status / summary / POV). Used at open.
    fn read_chapter_cache(&mut self, index: usize) {
        if index < self.manifest.chapters.len() {
            cache_chapter(&self.root, &mut self.manifest.chapters[index]);
        }
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
            chapter.cache_meta(front);
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
        atomic::write(&path, frontmatter::join(front, body).as_bytes())?;
        self.cache_chapter_meta(index, Some(&fm));
        Ok(())
    }

    /// Rewrite chapter `index`'s prose body on disk (#295), replacing each byte
    /// range in `ranges` with `replacement`, with front matter preserved and the
    /// cached word count refreshed. Ranges are byte offsets into the body and are
    /// applied right-to-left so earlier offsets stay valid. The inverse of
    /// [`edit_chapter_front_matter`](Self::edit_chapter_front_matter), which edits
    /// the front matter and preserves the body. Used by manuscript-wide replace
    /// for chapters other than the active one (whose live buffer is edited
    /// directly so its change stays undoable).
    pub(crate) fn replace_chapter_body(&mut self, index: usize, ranges: &[(usize, usize)], replacement: &str) {
        let path = self.chapter_path(index);
        let text = self.read_chapter(index);
        let (front, body) = frontmatter::split(&text);
        let mut body = body.to_string();
        let mut ranges = ranges.to_vec();
        ranges.sort_by_key(|&(s, _)| s);
        for &(start, end) in ranges.iter().rev() {
            if start <= end && end <= body.len() {
                body.replace_range(start..end, replacement);
            }
        }
        let front = front.map(FrontMatter::from_raw);
        let content = frontmatter::join(front.as_ref().map(FrontMatter::as_str), &body);
        if atomic::write(&path, content.as_bytes()).is_ok() {
            self.read_chapter_cache(index); // refresh the cached word count
        }
    }

    /// Whether `word` is in the project's custom dictionary (#358),
    /// case-insensitively — the seam the spell-checker's `accept` predicate uses.
    pub fn is_accepted(&self, word: &str) -> bool {
        self.dictionary.contains(&word.to_lowercase())
    }

    /// Accept `word` into the project's custom dictionary (#358): add it to the
    /// in-memory set and append it to `dictionary.txt` at the project root (one
    /// word per line, original case preserved for readability), so it's never
    /// flagged again here and travels in git. A no-op (returns `Ok(false)`) when
    /// the word — compared case-insensitively — is already accepted. Shared by the
    /// accept-flagged-word action and #303's "add these names" path.
    pub fn accept_word(&mut self, word: &str) -> std::io::Result<bool> {
        let key = word.trim().to_lowercase();
        if key.is_empty() || !self.dictionary.insert(key) {
            return Ok(false);
        }
        let path = self.root.join(DICTIONARY_NAME);
        let mut existing = fs::read_to_string(&path).unwrap_or_default();
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(word.trim());
        existing.push('\n');
        atomic::write(&path, existing.as_bytes())?;
        Ok(true)
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

    /// Write the manifest back to `book.toml`. A standalone novel serializes the
    /// flat manifest exactly as before; a multi-book series (#328) folds the live
    /// active book back into the series list and writes the `[[books]]` form.
    pub fn save_manifest(&self) -> std::io::Result<()> {
        let err = |e| std::io::Error::other(format!("could not serialize manifest: {e}"));
        let text = if self.books.is_empty() {
            toml::to_string_pretty(&self.manifest).map_err(err)?
        } else {
            series::series_to_toml(&self.manifest, &self.books, self.active_book).map_err(err)?
        };
        atomic::write(&Self::manifest_path(&self.root), text.as_bytes())
    }

    // --- Structure editing -------------------------------------------------
    //
    // These mutate the manifest and chapter files but never touch `active` — it
    // tracks the active chapter by file identity, so the index re-derives itself
    // (the one exception, deleting the active chapter, is handled in `App`). Each
    // does the fallible filesystem work first, then mutates the manifest and
    // persists, so a failed file op leaves the project intact.

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
            .map(|(i, title)| ChapterEntry::new((*title).to_string(), format!("chapter-{}.md", i + 1)))
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
            words_changed: HashSet::new(),
            books: Vec::new(),
            active_book: 0,
            dictionary: HashSet::new(),
        }
    }
}

/// Read the project's custom dictionary (#358) from `dictionary.txt` at `root`
/// into a lowercased set for case-insensitive matching. Best-effort: a missing
/// file yields an empty set; blank lines are ignored.
fn load_dictionary(root: &Path) -> HashSet<String> {
    fs::read_to_string(root.join(DICTIONARY_NAME))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
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
    fn accepts_words_into_the_project_dictionary() {
        let dir = temp_dir("dict");
        let _ = fs::remove_dir_all(&dir);
        let mut p = Project::create(&dir).unwrap();

        assert!(!p.is_accepted("Vashti"));
        assert!(p.accept_word("Vashti").unwrap(), "newly added");
        assert!(p.is_accepted("Vashti") && p.is_accepted("vashti"), "matched case-insensitively");
        assert!(!p.accept_word("vashti").unwrap(), "deduped case-insensitively");

        // Persisted to dictionary.txt (original case) and respected after reload.
        let file = fs::read_to_string(dir.join(DICTIONARY_NAME)).unwrap();
        assert!(file.contains("Vashti"), "{file}");
        assert!(Project::open(&dir).unwrap().is_accepted("VASHTI"), "loaded on reopen");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn accepting_a_word_preserves_existing_dictionary_entries() {
        let dir = temp_dir("dict-append");
        let _ = fs::remove_dir_all(&dir);
        let mut p = Project::create(&dir).unwrap();
        fs::write(dir.join(DICTIONARY_NAME), "Aerendil\n").unwrap();
        p.dictionary = load_dictionary(&dir); // re-read what we just wrote
        p.accept_word("Vashti").unwrap();
        let file = fs::read_to_string(dir.join(DICTIONARY_NAME)).unwrap();
        assert!(file.contains("Aerendil") && file.contains("Vashti"), "both kept: {file}");
        fs::remove_dir_all(&dir).ok();
    }

    /// Write an `n`-chapter project corpus (each chapter has front matter and a
    /// ~2.7 KB body) under `dir`, returning the manifest text for the caller to
    /// write. Shared by the open-performance guards.
    fn write_chapter_project(dir: &Path, n: usize, title: &str) -> String {
        let mut manifest = format!("title = \"{title}\"\n\n");
        for i in 0..n {
            let file = format!("ch{i}.md");
            manifest.push_str(&format!("[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"));
            let mut body = String::from("---\nstatus: draft\npov: Mara\n---\n# Chapter\n\n");
            for _ in 0..50 {
                body.push_str("The night was dark and the road was very long indeed.\n");
            }
            fs::write(dir.join(&file), body).unwrap();
        }
        manifest
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
    fn daily_goal_parses_and_round_trips_and_is_optional() {
        // A [goals] block with a daily target parses it.
        let toml = "title = \"B\"\n\n[goals]\nwords = 90000\nchapters = 40\ndaily = 1000\n";
        let m: Manifest = toml::from_str(toml).unwrap();
        assert_eq!(m.goals.as_ref().unwrap().daily, Some(1000));

        // Round-trips when set...
        let out = toml::to_string_pretty(&m).unwrap();
        assert!(out.contains("daily = 1000"), "daily persists: {out}");

        // ...and is omitted entirely when unset (old manifests still parse).
        let none: Manifest = toml::from_str("title = \"B\"\n\n[goals]\nwords = 90000\nchapters = 40\n").unwrap();
        assert_eq!(none.goals.as_ref().unwrap().daily, None);
        assert!(!toml::to_string_pretty(&none).unwrap().contains("daily"), "no daily key when unset");
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
            daily: None,
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
    fn open_reports_a_malformed_manifest_without_panicking() {
        let dir = temp_dir("bad-manifest");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Broken TOML — an unterminated array-of-tables header.
        fs::write(Project::manifest_path(&dir), "title = \"X\"\n[[chapters]\n").unwrap();

        let err = Project::open(&dir).err().expect("a malformed manifest is an error, not a panic");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains(MANIFEST_NAME), "clear message: {err}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_tolerates_garbage_front_matter() {
        let dir = temp_dir("bad-frontmatter");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(Project::manifest_path(&dir), "title = \"B\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n").unwrap();
        // A fenced block whose contents are not valid YAML at all.
        fs::write(dir.join("one.md"), "---\n: : : not yaml [}{\n\tstatus\n---\nThe prose body.\n").unwrap();

        // Opening caches each chapter's front matter — garbage must not panic; the
        // typed fields just come back empty, and the prose body is intact.
        let project = Project::open(&dir).expect("garbage front matter should not fail the open");
        assert_eq!(project.chapter_body(0), "The prose body.\n");
        assert!(project.manifest.chapters[0].status.is_none());

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
        let manifest = write_chapter_project(&dir, 150, "Big");
        fs::write(Project::manifest_path(&dir), manifest).unwrap();

        let t = Instant::now();
        let project = Project::open(&dir).unwrap();
        let dt = t.elapsed();

        println!("open 150 chapters (front matter + word counts) = {dt:?}");
        assert_eq!(project.manifest.chapters.len(), 150);
        assert!(dt.as_millis() < 250, "open took {dt:?}");

        fs::remove_dir_all(&dir).ok();
    }

    /// Performance guard (`cargo perf`). Opening a series-sized project (many
    /// books' worth of chapters) stays within budget — the per-chapter cache read
    /// is the only work proportional to the project, and it's bounded. See
    /// `docs/performance.md`. Catches a regression to O(n²) work at open or to
    /// re-reading files after the initial cache pass.
    #[test]
    #[ignore]
    fn open_is_fast_for_a_series_sized_project() {
        use std::time::Instant;

        let n = 1500;
        let dir = temp_dir("perf-open-series");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let manifest = write_chapter_project(&dir, n, "Series");
        fs::write(Project::manifest_path(&dir), manifest).unwrap();

        let t = Instant::now();
        let project = Project::open(&dir).unwrap();
        let dt = t.elapsed();

        println!("open {n} chapters (series-sized) = {dt:?}");
        assert_eq!(project.manifest.chapters.len(), n);
        assert!(dt.as_millis() < 500, "open took {dt:?}");

        fs::remove_dir_all(&dir).ok();
    }
}

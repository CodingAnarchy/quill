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
use std::sync::atomic::{AtomicU64, Ordering};

use crate::annotation::Annotation;
use crate::atomic;
use crate::frontmatter::{Band, FrontMatter, Role, Status};

mod annotations;
pub mod extra_keys;
mod goals;
mod io;
mod lock;
mod manifest;
mod series;
mod structure;
mod template;
mod words;

use annotations::load_annotations;
#[cfg(test)]
pub use annotations::write_count as annotation_writes;
pub use annotations::AnnotationsWrite;
pub use goals::{GoalField, Goals, DEFAULT_WORDS_PER_PAGE};
pub use io::read_existing;
use io::{cache_chapter, record_read};
use lock::{InstanceLock, LockOutcome, RecheckOutcome};
// Re-export the manifest types and helpers so every caller keeps using
// `crate::project::…`. A glob so the whole manifest surface (including
// `Analysis`, which only the analysis tests name) rides through without a
// per-item unused-in-binary diagnostic.
pub use manifest::*;
use series::Book;
pub use structure::unique_dest;
pub use template::{ProjectTemplate, TEMPLATES};

/// The manifest file at the root of every project.
pub const MANIFEST_NAME: &str = "book.toml";

/// The per-project custom spell-check dictionary file (#358), at the project root.
pub const DICTIONARY_NAME: &str = "dictionary.txt";

/// The per-project inline-annotations side file (#302), at the project root.
pub const ANNOTATIONS_NAME: &str = "annotations.toml";

/// A filesystem-safe single folder name from an author-typed project name —
/// trimmed, with path separators replaced and leading/trailing dots stripped, so
/// the name stays one folder (no `..`, no nesting). Shared by the no-path startup
/// (#423) and renaming the open project (#424). The trailing trim runs last
/// because stripping an outer dot can expose a space (`"book . "` → `"book"`).
pub fn sanitize_dir_name(input: &str) -> String {
    let flattened = input.trim().replace(['/', '\\'], "-");
    flattened.trim_matches('.').trim().to_string()
}

#[cfg(test)]
mod sanitize_tests {
    use super::sanitize_dir_name;

    #[test]
    fn trims_and_flattens_path_separators() {
        assert_eq!(
            sanitize_dir_name("  The Silmarillion \n"),
            "The Silmarillion"
        );
        assert_eq!(sanitize_dir_name("Vol/1"), "Vol-1");
        // Leading/trailing dots can't smuggle in `..` or a hidden folder.
        assert_eq!(sanitize_dir_name("../escape"), "-escape");
        assert_eq!(sanitize_dir_name("."), "");
        assert_eq!(sanitize_dir_name("   "), "");
        // Stripping a trailing dot must not leave a dangling space.
        assert_eq!(sanitize_dir_name("Book . "), "Book");
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
    /// Files whose cached word count changed since the daily tally last drained
    /// this (#220). [`set_chapter_words`](Self::set_chapter_words) is the only
    /// place counts change, so recording here lets the tally touch just the
    /// chapters that moved — not the whole project — every idle tick. Keyed by
    /// file so it survives reordering.
    words_changed: HashSet<String>,
    /// Running sum of the active book's cached per-chapter word counts (#867), so
    /// the save-path snapshot's project word total is read from a maintained total
    /// rather than re-summed across every chapter on each save. Kept in step with
    /// the caches: the single-chapter cache writes ([`set_chapter_words`](Self::set_chapter_words),
    /// [`read_chapter_cache`](Self::read_chapter_cache)) adjust it by the delta, and
    /// bulk re-caches (open, a book switch, an import) recompute it wholesale via
    /// [`sync_words_total`](Self::sync_words_total). Excludes the active chapter's
    /// live buffer edits — the snapshot folds those in from the buffer.
    words_total: usize,
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
    /// A process-unique id for the dictionary's current contents, re-issued
    /// whenever the set changes (open, reload, accept) — the cheap key the idle
    /// spell pass uses to tell that an accepted word could change its flags.
    dictionary_gen: u64,
    /// Inline author annotations (#302): notes anchored to chapter byte ranges,
    /// keyed by chapter file name. Loaded from `annotations.toml` at the project
    /// root at open; kept out of the chapter `.md` files so exports stay clean.
    annotations: Vec<Annotation>,
    /// Set while a change to `annotations` hasn't reached `annotations.toml` yet
    /// (a write failed), so the next write-back retries even when the notes it's
    /// handed already match memory (see [`Project::set_annotations`]).
    annotations_unsaved: bool,
    /// The advisory single-instance lock (#871), held from open until the project
    /// is dropped. `Some` when this process owns the project; `None` when it was
    /// opened read-only because another instance already held it (see `read_only`),
    /// or for a project created in-process (no contention to guard). Dropping it
    /// releases the lockfile — the lifecycle lives with the project, never in the
    /// quit path.
    #[allow(
        dead_code,
        reason = "held purely for its Drop (RAII lock release); never read"
    )]
    lock: Option<InstanceLock>,
    /// Whether this project was opened read-only because another live instance
    /// already holds it (#871). Writes are refused rather than racing the other
    /// instance's saves and clobbering the shared sidecars. `busy_holder` carries a
    /// human-readable description of the other instance for a warning to the author.
    read_only: bool,
    /// A description of the instance that already holds the project, set alongside
    /// `read_only`, for the open-time warning. `None` when this instance owns it.
    busy_holder: Option<String>,
    /// Chapter files (root-relative, across every book) whose last read found
    /// them missing or unreadable (#934). Filled as a side effect of the
    /// per-chapter caching at open (and refreshed by any later re-cache of a
    /// chapter), so the on-open damage check is a lookup over cached state rather
    /// than a second open plus a re-read of every chapter file.
    damaged_chapters: HashSet<String>,
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
        // Name the manifest path in any read failure. A bare `io::Error` from
        // `read_to_string` carries only "Permission denied"/"No such file" with no
        // path, so an unreadable project directory (chmod 0) or a missing manifest
        // would surface an error the author can't act on. Wrapping it keeps the
        // original `ErrorKind` (callers switch on `NotFound` vs. the rest) while the
        // message names the file — degrading gracefully, never a panic (#1101).
        let manifest_path = Self::manifest_path(&root);
        let text = fs::read_to_string(&manifest_path).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("could not read {}: {e}", manifest_path.display()),
            )
        })?;
        let invalid = |e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid {MANIFEST_NAME}: {e}"),
            )
        };
        // A `books` array marks a multi-book series (#328); anything else is the
        // flat single-book manifest, parsed and saved exactly as before. Peek via
        // `toml::Value` so the single-book path is byte-for-byte unchanged. The
        // series shape lives in `series.rs` (#350).
        let value: toml::Value = toml::from_str(&text).map_err(invalid)?;
        let (mut manifest, mut books, active_book) =
            Manifest::from_toml_value(value).map_err(invalid)?;

        // Refuse a manifest written by a *newer* build (#1144): a future
        // `format_version` may carry fields this build can't represent, and
        // `migrate` is a no-op at or above the current version — so opening would
        // load a lossy view and the next save would silently strip the future
        // fields. Fail with a clear, version-naming message instead. This uses
        // `InvalidData` (not `NotFound`), so the `NotFound` re-create path callers
        // switch on stays intact; current-and-older versions fall through to
        // `migrate` and open exactly as before.
        let found = manifest.format_version();
        if found > manifest::CURRENT_FORMAT_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{MANIFEST_NAME} format version {found} is newer than this build supports \
                     (version {}); upgrade Quill to open this project",
                    manifest::CURRENT_FORMAT_VERSION
                ),
            ));
        }

        // Migrate an older on-disk format forward to the current version (#1097)
        // before anything reads the manifest. Today this only stamps the version
        // field (there's been no breaking change), but it's the defined upgrade
        // seam a future breaking format change hangs its translation on, so the
        // format can evolve without a hard break at load.
        manifest.migrate();

        // Repair any hand-broken part ranges (inverted or zero-based) to the
        // `Part` invariant before any mapping code reads them — an inverted range
        // would otherwise silently map to no part (#185). Applies to the active
        // manifest and every series book, since both feed `part_at_body_position`.
        if let Some(parts) = manifest.parts.as_mut() {
            normalize_parts(parts);
        }
        for book in &mut books {
            if let Some(parts) = book.parts.as_mut() {
                normalize_parts(parts);
            }
        }

        // A chapter entry with an empty `file` is corruption (a half-written or
        // hand-broken manifest): the path would resolve to the project dir, so we
        // report it rather than silently opening an empty buffer at a stray path.
        if manifest.chapters.iter().any(|c| c.file.trim().is_empty()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid {MANIFEST_NAME}: a chapter entry has an empty file path"),
            ));
        }

        let dictionary = load_dictionary(&root);
        let annotations = load_annotations(&root);
        // Take the advisory instance lock (#871). If another live instance already
        // holds the project, open read-only rather than racing its writes and
        // clobbering the shared sidecars; the App warns the author. A crashed
        // instance's stale lock is reclaimed inside `acquire`.
        let (lock, read_only, busy_holder) = match lock::acquire(&root) {
            LockOutcome::Held(l) => (Some(l), false, None),
            LockOutcome::Busy { holder } => (None, true, Some(holder)),
        };
        let mut project = Self {
            root,
            manifest,
            active: String::new(),
            words_changed: HashSet::new(),
            words_total: 0,
            books,
            active_book,
            dictionary,
            dictionary_gen: next_dictionary_gen(),
            annotations,
            annotations_unsaved: false,
            lock,
            read_only,
            busy_holder,
            damaged_chapters: HashSet::new(),
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
            project
                .active
                .clone_from(&project.manifest.chapters[0].file);
        }
        // Cache the other books' chapters too (#331), so book and series totals
        // are real without re-reading later. One file read per chapter; the
        // active book is already cached above (it lives in the manifest).
        let active = project.active_book;
        let root = project.root.clone();
        // The TODO marker is series-wide (folded into the active manifest), so it's
        // the right marker for every book's cached counts (#867).
        let marker = project.manifest.todo_marker().to_string();
        for (b, book) in project.books.iter_mut().enumerate() {
            if b == active {
                continue;
            }
            for entry in &mut book.chapters {
                let outcome = cache_chapter(&root, entry, &marker);
                record_read(&mut project.damaged_chapters, &entry.file, outcome);
            }
        }
        Ok(project)
    }

    /// Whether every chapter file of the active book read cleanly the last time
    /// it was cached (#934) — at open, for an untouched project. A chapter whose
    /// file was missing (the manifest still lists it) or failed to read (a hard
    /// error: permissions, an I/O fault, invalid UTF-8) makes this `false`. Reads
    /// only the record [`cache_chapter`] left during open, so it's O(chapters) in
    /// memory with no file I/O.
    pub fn chapters_intact(&self) -> bool {
        self.damaged_chapters.is_empty()
            || self
                .manifest
                .chapters
                .iter()
                .all(|c| !self.damaged_chapters.contains(&c.file))
    }

    /// Whether this project was opened read-only because another live instance
    /// already holds it (#871). Writes are refused in that state to avoid racing
    /// the other instance and clobbering the shared sidecars.
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// A human-readable description of the instance that already holds the project,
    /// for the open-time warning — `None` when this instance owns it (#871).
    pub fn busy_holder(&self) -> Option<&str> {
        self.busy_holder.as_deref()
    }

    /// Re-check a read-only project's instance lock and reclaim it if it has gone
    /// stale since open (#890): the holder we deferred to at open crashed while we
    /// were running read-only, so its lock is now a dead same-host pid (or a torn
    /// file). If so, install the reclaimed lock, clear the read-only state, and
    /// return `true` so the caller can note that a stale lock was cleared.
    ///
    /// A no-op unless we're currently read-only — a project we already own has
    /// nothing to reclaim. A genuinely-live holder (or a different-host /
    /// undecidable lock) leaves us read-only exactly as at open, so this never
    /// steals a lock from a live instance. Called off the keystroke path (the idle
    /// tick), never per-write.
    pub fn recheck_lock(&mut self) -> bool {
        if !self.read_only {
            return false;
        }
        match lock::recheck(&self.root) {
            RecheckOutcome::Reclaimed(lock) => {
                self.lock = Some(lock);
                self.read_only = false;
                self.busy_holder = None;
                true
            }
            RecheckOutcome::StillBusy => false,
        }
    }

    /// The error a write returns when the project is read-only (another instance
    /// holds it), so a save reports "not saved" rather than racing the owner.
    fn read_only_error() -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "project is open in another Quill instance (read-only); not saved",
        )
    }

    /// Initialize a new project in `dir` (creating the directory if needed): a
    /// manifest plus one empty starter chapter, ready to edit. The historic
    /// "Empty" behavior, kept as the default; [`Project::create_with_template`]
    /// scaffolds a richer structure.
    pub fn create(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::create_with_template(dir, ProjectTemplate::at(0))
    }

    /// Initialize a new project in `dir`, scaffolding `template`'s structure
    /// (#611). For the "Empty" template this is one blank starter chapter (the
    /// historic default); for a structured template it writes a `[[parts]]`
    /// grouping plus the template's titled empty chapter stubs, walking them
    /// through the same `add_chapter`/manifest path an author would.
    pub fn create_with_template(
        dir: impl AsRef<Path>,
        template: &ProjectTemplate,
    ) -> std::io::Result<Self> {
        let root = dir.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;

        let title = root
            .file_name()
            .map_or_else(|| "Untitled".into(), |n| n.to_string_lossy().into_owned());

        // A freshly created project is owned by this process: hold the lock so a
        // second instance that opens it right after creation is serialized too.
        let lock = match lock::acquire(&root) {
            LockOutcome::Held(l) => Some(l),
            LockOutcome::Busy { .. } => None,
        };
        let mut project = Self {
            root,
            manifest: Manifest {
                title,
                // Stamp the current on-disk format version (#1097) so a freshly
                // created project records the format it was written at.
                format_version: Some(manifest::CURRENT_FORMAT_VERSION),
                ..Default::default()
            },
            active: String::new(),
            words_changed: HashSet::new(),
            words_total: 0,
            books: Vec::new(),
            active_book: 0,
            dictionary: HashSet::new(),
            dictionary_gen: next_dictionary_gen(),
            annotations: Vec::new(),
            annotations_unsaved: false,
            lock,
            read_only: false,
            busy_holder: None,
            damaged_chapters: HashSet::new(),
        };
        if template.is_empty() {
            project.add_default_chapter()?;
        } else {
            project.scaffold_template(template)?;
        }
        Ok(project)
    }

    /// Scaffold a non-empty template's structure (#611): append each part's
    /// titled empty chapter stubs in order (reusing the `add_chapter` path so the
    /// files are created and the cache read back), then record the derived
    /// `[[parts]]` ranges on the manifest. The first chapter is made active so the
    /// editor opens on it, exactly as a fresh project does.
    fn scaffold_template(&mut self, template: &ProjectTemplate) -> std::io::Result<()> {
        let mut parts = Vec::with_capacity(template.parts.len());
        let mut next = 1; // 1-based chapter index of the next stub
        for part in template.parts {
            let start = next;
            for stub in part.chapters {
                self.add_chapter(self.manifest.chapters.len(), stub)?;
                next += 1;
            }
            // A part with no stubs contributes no range; skip it rather than
            // recording an inverted [start, start-1].
            if next > start {
                parts.push(Part {
                    name: Some(part.name.to_string()),
                    chapters: [start, next - 1],
                    goal: None,
                });
            }
        }
        // A template with only empty parts still needs a chapter to edit.
        if self.manifest.chapters.is_empty() {
            self.add_default_chapter()?;
        } else {
            self.active = self.manifest.chapters[0].file.clone();
        }
        if !parts.is_empty() {
            self.manifest.parts = Some(parts);
        }
        self.save_manifest()
    }

    /// Append a starter chapter (creating its file if absent) and persist the
    /// manifest. Used by `create`, and by `open` as a safety net.
    fn add_default_chapter(&mut self) -> std::io::Result<()> {
        let n = self.manifest.chapters.len() + 1;
        let entry = ChapterEntry::new(format!("Chapter {n}"), format!("chapter-{n}.md"));
        let path = self.root.join(&entry.file);
        if !path.exists() {
            atomic::write(&path, b"")?;
        }
        self.active.clone_from(&entry.file);
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
        self.manifest
            .chapters
            .get(index)
            .map_or("", |c| c.file.as_str())
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
        let chapters: Vec<PathBuf> = (0..self.manifest.chapters.len())
            .map(|i| self.chapter_path(i))
            .collect();
        let mut found = Vec::new();
        collect_markdown(&self.root, &chapters, &mut found);
        found.sort();
        found
    }

    /// The story-bible files (#590): every Markdown file under `bible/` — the
    /// characters, timeline, setting, and outline the AI passes track and the
    /// author edits as ground truth. The `bible/`-scoped subset of
    /// [`reference_files`](Self::reference_files), for the bible browser that opens
    /// one in an editable buffer. Sorted by path for a stable list; empty when the
    /// project has no `bible/` directory yet.
    pub fn bible_files(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        // The bible holds no chapters, so nothing to exclude from the walk.
        collect_markdown(&self.root.join("bible"), &[], &mut found);
        found.sort();
        found
    }

    /// Make chapter `index` the active one.
    pub fn set_active(&mut self, index: usize) {
        if let Some(chapter) = self.manifest.chapters.get(index) {
            self.active = chapter.file.clone();
        }
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
        self.manifest
            .chapters
            .iter()
            .filter(|c| Band::of(c.role) == Band::Body)
            .count()
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
        if self
            .manifest
            .chapters
            .is_sorted_by_key(|c| Band::of(c.role))
        {
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

    /// Whether `word` is in the project's custom dictionary (#358),
    /// case-insensitively — the seam the spell-checker's `accept` predicate uses.
    ///
    /// Called once per dictionary-rejected word on the idle spell pass (#780), so
    /// it avoids allocating: the empty-dictionary case (no custom words — the
    /// common one) short-circuits at zero cost, and an already-lowercase word is
    /// looked up by borrow. Only a word that actually has uppercase pays for the
    /// lowercased key it needs to match the set (whose keys are stored lowercased).
    pub fn is_accepted(&self, word: &str) -> bool {
        if self.dictionary.is_empty() {
            return false;
        }
        // No uppercase ⇒ the word is already its own lowercase key: look it up by
        // borrow, no `String`. (Covers the overwhelming majority of tokens.)
        if !word.chars().any(char::is_uppercase) {
            return self.dictionary.contains(word);
        }
        self.dictionary.contains(&word.to_lowercase())
    }

    /// The dictionary's content generation: process-unique, re-issued whenever the
    /// accepted-word set changes (and fresh for every opened project), so two equal
    /// reads mean [`is_accepted`](Self::is_accepted) answers identically.
    pub const fn dictionary_generation(&self) -> u64 {
        self.dictionary_gen
    }

    /// The path to the project's custom dictionary file (#935), whether or not it
    /// exists yet — the seam the in-app view/edit path opens in a pane so the author
    /// can read and hand-edit the accepted-word list directly.
    pub fn dictionary_path(&self) -> PathBuf {
        self.root.join(DICTIONARY_NAME)
    }

    /// Re-read the custom dictionary from disk into the in-memory set (#935), so an
    /// edit made outside `accept_word` — the author hand-editing `dictionary.txt`, or
    /// the in-app view/edit pane saving it — takes effect without reopening the
    /// project. Kept off the idle/keystroke path: the caller runs it only when the
    /// dictionary was actually touched (e.g. its edit pane closed), never per tick.
    pub fn reload_dictionary(&mut self) {
        self.dictionary = load_dictionary(&self.root);
        self.dictionary_gen = next_dictionary_gen();
    }

    /// Accept `word` into the project's custom dictionary (#358): add it to the
    /// in-memory set and append it to `dictionary.txt` at the project root (one
    /// word per line, original case preserved for readability), so it's never
    /// flagged again here and travels in git. A no-op (returns `Ok(false)`) when
    /// the word — compared case-insensitively — is already accepted. Shared by the
    /// accept-flagged-word action and #303's "add these names" path.
    pub fn accept_word(&mut self, word: &str) -> std::io::Result<bool> {
        let key = word.trim().to_lowercase();
        if key.is_empty() || self.dictionary.contains(&key) {
            return Ok(false);
        }
        let path = self.root.join(DICTIONARY_NAME);
        // Read before touching in-memory state: a failed read must abort, since
        // appending onto an empty base would shrink the dictionary to just this
        // word while reporting success (#565). The in-memory set stays in sync
        // with disk because the insert only happens after a successful write.
        let mut existing = read_existing(&path)?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(word.trim());
        existing.push('\n');
        atomic::write(&path, existing.as_bytes())?;
        self.dictionary.insert(key);
        self.dictionary_gen = next_dictionary_gen();
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
            .map(|(i, title)| {
                ChapterEntry::new((*title).to_string(), format!("chapter-{}.md", i + 1))
            })
            .collect();
        let active = chapters[0].file.clone();
        Self {
            root: PathBuf::from("."),
            manifest: Manifest {
                title: "untitled".into(),
                chapters,
                ..Default::default()
            },
            active,
            words_changed: HashSet::new(),
            words_total: 0,
            books: Vec::new(),
            active_book: 0,
            dictionary: HashSet::new(),
            dictionary_gen: next_dictionary_gen(),
            annotations: Vec::new(),
            annotations_unsaved: false,
            // An in-memory scratch project owns nothing on disk, so it holds no
            // instance lock and is writable (never read-only).
            lock: None,
            read_only: false,
            busy_holder: None,
            damaged_chapters: HashSet::new(),
        }
    }
}

/// A fresh, never-before-issued [`Project::dictionary_generation`]: process-wide,
/// so a switched-to project never shares a generation with the one it replaced.
fn next_dictionary_gen() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
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
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Generated/scratch trees aren't reference material: `unused/` (deleted
            // chapters) and `export/` (compiled output, #190).
            let name = path.file_name().and_then(|n| n.to_str());
            if !matches!(name, Some("unused" | "export")) {
                collect_markdown(&path, chapters, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md")
            && !chapters.contains(&path)
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod hardening_tests;

#[cfg(test)]
mod adverse_fs_tests;

//! Book management on a multi-book series (#328–#341, #648): the `Project`-level
//! accessors and mutators that operate over the series book list — switching the
//! active book, converting a standalone novel into a series, adding/removing/
//! reordering books, and the per-book read accessors. These sit on top of the
//! manifest ⇄ book hoisting owned by the parent [`super`] module.

use std::fs;
use std::path::Path;

use crate::atomic;
use crate::frontmatter::{self, Epigraph, FrontMatter};

use super::super::io::cache_chapter;
use super::super::structure::unique_dest;
use super::super::{ChapterEntry, Part, Project};
use super::{Book, BookPublishing};

impl Project {
    /// How many books the project holds — 1 for a standalone novel (#328).
    pub fn book_count(&self) -> usize {
        self.books.len().max(1)
    }

    /// The index of the active book within the series (0 for a standalone novel).
    pub const fn active_book(&self) -> usize {
        self.active_book
    }

    /// Switch the active book to `index` (#329): stash the live active book back
    /// into the series, hoist the target book into the manifest, cache its
    /// chapters, and select its first chapter. A no-op for a standalone novel or
    /// an out-of-range / unchanged index. The caller flushes the outgoing book's
    /// buffer and reloads the editor afterward.
    pub fn set_active_book(&mut self, index: usize) {
        if self.books.is_empty() || index >= self.books.len() || index == self.active_book {
            return;
        }
        // Fold the live active book back into the series, then hoist the target.
        self.books[self.active_book] = self.manifest.to_book();
        self.manifest.load_book(&self.books[index]);
        self.active_book = index;
        // Cache the new book's chapters (word count + front matter) once, then
        // band-order them — the same work `open` does for the first book.
        self.activate_hoisted_book(true);
    }

    /// The bookkeeping every book activation shares once the target book has been
    /// hoisted into the manifest and `active_book` set: the outgoing book's
    /// pending count changes no longer apply, so drop them; optionally cache the
    /// new book's chapters (word count + front matter) and band-order them — the
    /// bounded work `open` does for the first book — then select the new book's
    /// first chapter. `add_book` passes `cache = false` because its lone starter
    /// chapter is already cached and its single-chapter list needs no reorder.
    fn activate_hoisted_book(&mut self, cache: bool) {
        // The old book's pending count changes no longer apply.
        self.words_changed.clear();
        if cache {
            // The running word total (#867) tracks the active book; reset it so the
            // per-chapter delta path rebuilds it from the newly hoisted book.
            self.words_total = 0;
            for i in 0..self.manifest.chapters.len() {
                self.read_chapter_cache(i);
            }
            self.reorder_by_band();
        }
        self.active = self
            .manifest
            .chapters
            .first()
            .map_or(String::new(), |c| c.file.clone());
    }

    /// Convert a standalone novel into a multi-book series (#648): the current
    /// single book becomes book one, and `series_title` names the series. A no-op
    /// (returning `Ok(())`) if the project is already a series. Persists the new
    /// `[[books]]` manifest.
    ///
    /// The existing book keeps its chapters, files, and every per-book field; only
    /// the on-disk shape changes (flat manifest → `[[books]]`). The active book
    /// stays the current one, so the live buffer and selection are untouched.
    pub fn convert_to_series(&mut self, series_title: &str) -> std::io::Result<()> {
        if !self.books.is_empty() {
            return Ok(()); // already a series
        }
        // Hoist the live single book into the series list as book one, and record
        // the series title on the manifest (the series-level `title` on save).
        self.manifest.series = Some(series_title.trim().to_string());
        self.books = vec![self.manifest.to_book()];
        self.active_book = 0;
        self.save_manifest()
    }

    /// Add a new book titled `title` to the series (#648), with one empty starter
    /// chapter so it's immediately editable, and make it the active book. Returns
    /// the index of the new book. Errors if the project isn't a series yet — the
    /// caller converts first. Persists the `[[books]]` manifest and the starter
    /// chapter file.
    ///
    /// The new book's chapter lives in its own uniquely-named file at the project
    /// root, so it never collides with another book's chapters. The switch mirrors
    /// [`set_active_book`]: the outgoing live book is folded back first.
    pub fn add_book(&mut self, title: &str) -> std::io::Result<usize> {
        debug_assert!(
            !self.books.is_empty(),
            "add_book on a standalone novel — convert_to_series first"
        );
        // Fold the live active book back before we append, so its buffered
        // structural state is captured in the series list (mirrors set_active_book).
        self.books[self.active_book] = self.manifest.to_book();
        let file = self.unique_book_chapter_file();
        atomic::write(&self.root.join(&file), b"")?;
        let mut chapter = ChapterEntry::new("Chapter 1".to_string(), file);
        cache_chapter(&self.root, &mut chapter, self.manifest.todo_marker());
        let index = self.books.len();
        self.books.push(Book {
            title: title.trim().to_string(),
            chapters: vec![chapter],
            ..Default::default()
        });
        // Hoist the new book into the active manifest so the editor opens on it.
        self.manifest.load_book(&self.books[index]);
        self.active_book = index;
        // The starter chapter is already cached and its lone-chapter list needs
        // no reorder, so skip the cache pass `set_active_book` runs.
        self.activate_hoisted_book(false);
        // The new active book is the empty starter, so its running word total (#867)
        // is just that chapter's cached count — reset from the outgoing book's.
        self.sync_words_total();
        self.save_manifest()?;
        Ok(index)
    }

    /// Remove book `index` from the series (#648), retiring its chapter files to
    /// the project's `unused/` directory (never deleting them, so nothing the
    /// author wrote is lost — the same guarantee [`remove_chapter`] gives).
    /// Guarded: the last remaining book of a series can't be removed (a series
    /// always has at least one book), and an out-of-range index is a no-op.
    ///
    /// The active book stays consistent: removing a book before the active one
    /// shifts the active index down; removing the active book itself activates the
    /// book that shifts into its place (clamped to the new last book). Persists the
    /// manifest, and returns whether the active book changed (so the caller can
    /// reload the editor).
    pub fn remove_book(&mut self, index: usize) -> std::io::Result<bool> {
        if index >= self.books.len() || self.books.len() <= 1 {
            return Ok(false);
        }
        // Fold the live active book back so a non-active removal keeps its edits.
        self.books[self.active_book] = self.manifest.to_book();
        // Retire the removed book's chapter files to `unused/` rather than deleting.
        // A book's chapters may live in a per-book subdirectory (e.g. `b2/one.md`);
        // retire them by base name so they land directly in `unused/` (never a
        // never-created nested subdir), matching `remove_chapter`'s flat retirement.
        let unused = self.root.join("unused");
        for chapter in &self.books[index].chapters {
            let src = self.root.join(&chapter.file);
            if src.exists() {
                fs::create_dir_all(&unused)?;
                let base = Path::new(&chapter.file).file_name().map_or_else(
                    || chapter.file.clone(),
                    |n| n.to_string_lossy().into_owned(),
                );
                let dest = unique_dest(&unused, &base);
                fs::rename(&src, &dest)?;
            }
        }
        self.books.remove(index);
        // Keep the active book pointing at the right book. Removing one before it
        // shifts it down; removing the active one lands on whatever took its slot
        // (clamped to the last book).
        let was_active = index == self.active_book;
        if index < self.active_book {
            self.active_book -= 1;
        } else if was_active {
            self.active_book = self.active_book.min(self.books.len() - 1);
        }
        // If the active book moved, hoist the new one into the manifest.
        let active_changed = was_active;
        if active_changed {
            self.manifest.load_book(&self.books[self.active_book]);
            self.activate_hoisted_book(true);
        }
        self.save_manifest()?;
        Ok(active_changed)
    }

    /// Move book `index` one place toward the front (`up`) or back of the series
    /// (#648), just reordering the `[[books]]` list — no files move. A no-op at the
    /// ends or for an out-of-range index. Keeps the active book pointing at the
    /// same book (its index follows the swap), so the live buffer is untouched.
    /// Persists the manifest and returns whether a swap happened.
    pub fn move_book(&mut self, index: usize, up: bool) -> std::io::Result<bool> {
        if index >= self.books.len() {
            return Ok(false);
        }
        let Some(target) = (if up {
            index.checked_sub(1)
        } else {
            (index + 1 < self.books.len()).then_some(index + 1)
        }) else {
            return Ok(false);
        };
        // Fold the live active book back so its edits ride along with the reorder.
        self.books[self.active_book] = self.manifest.to_book();
        self.books.swap(index, target);
        // The active book keeps its identity: follow it to its new slot.
        if self.active_book == index {
            self.active_book = target;
        } else if self.active_book == target {
            self.active_book = index;
        }
        self.save_manifest()?;
        Ok(true)
    }

    /// The smallest `chapter-{n}.md` name free across *every* book of the series
    /// and on disk (#648) — a series-wide version of the single-book
    /// `unique_chapter_file`, so a new book's starter chapter never collides with
    /// another book's file.
    fn unique_book_chapter_file(&self) -> String {
        let used: std::collections::HashSet<&str> = self
            .books
            .iter()
            .flat_map(|b| b.chapters.iter())
            .map(|c| c.file.as_str())
            .chain(self.manifest.chapters.iter().map(|c| c.file.as_str()))
            .collect();
        // `used` is finite and disk names are finite, so all but finitely many of
        // the distinct `chapter-{n}.md` names are free — `find` always terminates.
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

    /// Whether book `i` is the one currently hoisted into the live manifest, so
    /// its per-book fields are read from the manifest rather than the stored
    /// series list. A standalone novel (`books` empty) has its single book at
    /// index 0, which is always the active one; any other index is out of range.
    const fn is_active_book(&self, i: usize) -> bool {
        if self.books.is_empty() {
            i == 0
        } else {
            i == self.active_book
        }
    }

    /// The title of book `i`. The active book's live title comes from the
    /// manifest; the others from the stored series list. `None` if out of range.
    pub fn book_title(&self, i: usize) -> Option<&str> {
        if self.is_active_book(i) {
            Some(self.manifest.title.as_str())
        } else {
            self.books.get(i).map(|b| b.title.as_str())
        }
    }

    /// The chapters of book `i` (#330): the live manifest list for the active
    /// book (band-ordered), or the stored book's list otherwise. Empty if out of
    /// range.
    pub fn book_chapters(&self, i: usize) -> &[ChapterEntry] {
        if self.is_active_book(i) {
            &self.manifest.chapters
        } else {
            self.books.get(i).map_or(&[], |b| b.chapters.as_slice())
        }
    }

    /// The `parts` overlay of book `i`, if any (#330).
    pub fn book_parts(&self, i: usize) -> Option<&[Part]> {
        let parts = if self.is_active_book(i) {
            self.manifest.parts.as_ref()
        } else {
            self.books.get(i).and_then(|b| b.parts.as_ref())
        };
        parts.map(Vec::as_slice)
    }

    /// Total cached words in book `i` (#331), summed from its chapters' cached
    /// counts. The App layer counts the *active* book from the live buffer
    /// instead (its active chapter), so a live series total is built there — see
    /// `App::book_word_count` / `App::series_word_count`.
    pub fn book_word_count(&self, i: usize) -> usize {
        self.book_chapters(i).iter().map(|c| c.words).sum()
    }

    /// Book `i`'s subtitle, if any (#332): the active book's from the manifest,
    /// the others from the stored series list.
    pub fn book_subtitle(&self, i: usize) -> Option<&str> {
        if self.is_active_book(i) {
            self.manifest.subtitle.as_deref()
        } else {
            self.books.get(i).and_then(|b| b.subtitle.as_deref())
        }
    }

    /// Book `i`'s resolved publishing metadata (#810): its own per-book ISBN,
    /// publisher, publication date, and rights line, each falling back to the
    /// series/active-book value (the live [`Manifest`]) when the book leaves that
    /// field unset. So a book with no per-book metadata reprints the series-level
    /// metadata (the pre-#810 behavior), while a book that sets its own gets it.
    ///
    /// The active book reads its live values straight from the manifest; the
    /// others read their stored per-book fields, then fall back to the manifest.
    /// Language is a package-level concern kept once on the manifest, so it is not
    /// part of this per-book resolution — the caller reads it from the manifest.
    pub fn book_publishing(&self, i: usize) -> BookPublishing {
        let m = &self.manifest;
        if self.is_active_book(i) {
            // The active book's live values already live on the manifest.
            return BookPublishing {
                isbn: m.isbn.clone(),
                publisher: m.publisher.clone(),
                pub_date: m.pub_date.clone(),
                rights: m.rights.clone(),
            };
        }
        let book = self.books.get(i);
        // Per-book value when set, else the series/active-book value from the manifest.
        let resolve = |book_field: Option<&str>, series_field: &Option<String>| {
            book_field
                .map(str::to_string)
                .or_else(|| series_field.clone())
        };
        BookPublishing {
            isbn: resolve(book.and_then(|b| b.isbn.as_deref()), &m.isbn),
            publisher: resolve(book.and_then(|b| b.publisher.as_deref()), &m.publisher),
            pub_date: resolve(book.and_then(|b| b.pub_date.as_deref()), &m.pub_date),
            rights: resolve(book.and_then(|b| b.rights.as_deref()), &m.rights),
        }
    }

    /// Book `i`'s epigraph, if any (#332).
    pub fn book_epigraph(&self, i: usize) -> Option<Epigraph> {
        if self.is_active_book(i) {
            self.manifest.epigraph()
        } else {
            self.books
                .get(i)
                .and_then(|b| Epigraph::from_parts(b.epigraph.clone(), b.epigraph_by.clone()))
        }
    }

    /// Read a chapter file's prose body and front matter from disk (#332) — for
    /// books other than the active one, whose chapters aren't the live manifest.
    /// Best-effort: an unreadable file yields empty prose.
    pub fn read_chapter_parts(&self, file: &str) -> (String, Option<FrontMatter>) {
        let text = fs::read_to_string(self.root.join(file)).unwrap_or_default();
        let (front, body) = frontmatter::split(&text);
        (body.to_string(), front.map(FrontMatter::from_raw))
    }

    /// The grouping term ("part", "act", …) for book `i`, defaulting to "part".
    pub fn book_part_label(&self, i: usize) -> &str {
        if self.is_active_book(i) {
            self.manifest.part_label()
        } else {
            self.books
                .get(i)
                .and_then(|b| b.part_label.as_deref())
                .unwrap_or("part")
        }
    }

    /// The display name of part `part_index` in book `i`: its given name, or the
    /// auto-label `"{part_label} {n}"` for an unnamed part. Shared so the nav
    /// tree, the export, and the continuous-view header (#412) name parts the
    /// same way.
    pub fn book_part_display_name(&self, i: usize, part_index: usize) -> String {
        self.book_parts(i)
            .and_then(|ps| ps.get(part_index))
            .and_then(|p| p.name.clone())
            .unwrap_or_else(|| format!("{} {}", self.book_part_label(i), part_index + 1))
    }

    /// The series title, when this project is a multi-book series (#328); `None`
    /// for a standalone novel (whose `series` field, if any, is just a label).
    pub fn series_title(&self) -> Option<&str> {
        if self.books.is_empty() {
            None
        } else {
            self.manifest.series.as_deref()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Instant;

    use super::super::tests::{temp_dir, write_series, write_series_project};
    use super::Project;

    #[test]
    fn set_active_book_hoists_the_target_and_round_trips() {
        let dir = temp_dir("series-switch");
        write_series(&dir);
        let mut p = Project::open(&dir).unwrap();

        p.set_active_book(1);
        assert_eq!(p.active_book(), 1);
        assert_eq!(p.manifest.title, "Book Two");
        let titles: Vec<&str> = p
            .manifest
            .chapters
            .iter()
            .map(|c| c.title.as_str())
            .collect();
        assert_eq!(titles, ["Dusk"]);
        assert_eq!(p.active_file(), "b2/one.md");
        assert_eq!(p.book_title(0), Some("Book One")); // the stored book is preserved
        assert_eq!(p.book_title(1), Some("Book Two")); // the active book reads from the manifest

        // Switching back restores book one; saving keeps both books.
        p.set_active_book(0);
        assert_eq!(p.manifest.title, "Book One");
        p.save_manifest().unwrap();
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(
            toml.contains("Book One") && toml.contains("Book Two") && toml.contains("b2/one.md"),
            "{toml}"
        );

        // A no-op for an unchanged or out-of-range index.
        p.set_active_book(0);
        assert_eq!(p.active_book(), 0);
        p.set_active_book(9);
        assert_eq!(p.active_book(), 0);

        fs::remove_dir_all(&dir).ok();
    }

    /// Switching to a book reads its chapters once (word count + front matter),
    /// the same bounded work `open` does for the first book — never the whole
    /// series. Catches a regression that re-reads or re-parses superlinearly.
    #[test]
    #[ignore = "release-only perf guard; run via cargo perf"]
    fn switching_books_is_fast_in_a_series() {
        let (books, per_book) = (2, 750);
        let dir = temp_dir("perf-switch-book");
        fs::create_dir_all(&dir).unwrap();
        let manifest = write_series_project(&dir, books, per_book);
        fs::write(Project::manifest_path(&dir), manifest).unwrap();
        let mut project = Project::open(&dir).unwrap();

        let t = Instant::now();
        project.set_active_book(1);
        let dt = t.elapsed();

        println!("switch to a {per_book}-chapter book = {dt:?}");
        assert_eq!(project.active_book(), 1);
        assert_eq!(project.manifest.chapters.len(), per_book);
        assert!(dt.as_millis() < 500, "book switch took {dt:?}");

        fs::remove_dir_all(&dir).ok();
    }

    /// Converting a standalone novel into a series (#648) turns its single book
    /// into book one and writes the `[[books]]` manifest, without disturbing the
    /// book's chapters, files, or the active selection. Reopening reads it back as
    /// a two-index-capable series.
    #[test]
    fn convert_to_series_hoists_the_single_book() {
        let dir = temp_dir("series-convert");
        let mut project = Project::create(&dir).unwrap(); // flat single-book manifest
        let book_title = project.manifest.title.clone();
        assert_eq!(project.book_count(), 1);

        project.convert_to_series("The Chronicles").unwrap();
        assert_eq!(project.book_count(), 1); // still one book, now in the series list
        assert_eq!(project.series_title(), Some("The Chronicles"));
        assert_eq!(project.book_title(0), Some(book_title.as_str()));

        // The manifest is now the `[[books]]` shape, and reopens as a series.
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(toml.contains("[[books]]"), "series shape:\n{toml}");
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.series_title(), Some("The Chronicles"));

        // Converting again is a no-op (already a series).
        project.convert_to_series("Ignored").unwrap();
        assert_eq!(project.series_title(), Some("The Chronicles"));

        fs::remove_dir_all(&dir).ok();
    }

    /// Adding a book (#648) appends it with a starter chapter file, makes it
    /// active, and persists to the `[[books]]` manifest; reopening sees both books.
    #[test]
    fn add_book_appends_and_activates() {
        let dir = temp_dir("series-add");
        write_series(&dir);
        let mut project = Project::open(&dir).unwrap();
        assert_eq!(project.book_count(), 2);

        let index = project.add_book("Book Three").unwrap();
        assert_eq!(index, 2);
        assert_eq!(project.book_count(), 3);
        assert_eq!(project.active_book(), 2);
        assert_eq!(project.book_title(2), Some("Book Three"));
        // The starter chapter file exists and is the active chapter.
        assert_eq!(project.manifest.chapters.len(), 1);
        let file = project.manifest.chapters[0].file.clone();
        assert!(dir.join(&file).exists(), "starter chapter file created");

        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(toml.contains("Book Three"), "new book persisted:\n{toml}");
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.book_count(), 3);
        assert_eq!(reopened.book_title(2), Some("Book Three"));

        fs::remove_dir_all(&dir).ok();
    }

    /// Removing a book (#648) is guarded (the last book can't go), retires its
    /// chapter files to `unused/` rather than deleting them, and keeps the active
    /// book consistent.
    #[test]
    fn remove_book_is_guarded_and_retires_files() {
        let dir = temp_dir("series-remove");
        write_series(&dir);
        let mut project = Project::open(&dir).unwrap();

        // Remove the inactive second book: active book (0) is unchanged.
        let active_changed = project.remove_book(1).unwrap();
        assert!(!active_changed);
        assert_eq!(project.book_count(), 1);
        assert_eq!(project.active_book(), 0);
        assert_eq!(project.book_title(0), Some("Book One"));
        // Book two's chapter file was retired, not deleted.
        assert!(!dir.join("b2/one.md").exists(), "chapter moved out");
        assert!(dir.join("unused/one.md").exists(), "retired to unused/");

        // A one-book series refuses to remove its last book.
        assert!(!project.remove_book(0).unwrap());
        assert_eq!(project.book_count(), 1);

        // The manifest persisted the removal.
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.book_count(), 1);
        assert_eq!(reopened.book_title(0), Some("Book One"));

        fs::remove_dir_all(&dir).ok();
    }

    /// Removing the *active* book (#648) activates a neighbour and signals the
    /// change so the caller can reload the editor.
    #[test]
    fn remove_active_book_reactivates_a_neighbour() {
        let dir = temp_dir("series-remove-active");
        write_series(&dir);
        let mut project = Project::open(&dir).unwrap();
        assert_eq!(project.active_book(), 0);

        let active_changed = project.remove_book(0).unwrap();
        assert!(
            active_changed,
            "removing the active book changes the active"
        );
        assert_eq!(project.book_count(), 1);
        assert_eq!(project.active_book(), 0);
        assert_eq!(project.book_title(0), Some("Book Two"));
        // The manifest was hoisted to the surviving book.
        assert_eq!(project.manifest.title, "Book Two");
        assert_eq!(project.active_file(), "b2/one.md");

        fs::remove_dir_all(&dir).ok();
    }

    /// Reordering books (#648) swaps their series order without moving files, and
    /// keeps the active book pointing at the same book. Persists to `[[books]]`.
    #[test]
    fn move_book_reorders_and_keeps_the_active_book() {
        let dir = temp_dir("series-reorder");
        write_series(&dir);
        let mut project = Project::open(&dir).unwrap();
        assert_eq!(project.active_book(), 0); // Book One is active

        // Move Book One down: order becomes [Book Two, Book One]; the active book
        // (Book One) follows to index 1, and the live manifest is untouched.
        assert!(project.move_book(0, false).unwrap());
        assert_eq!(project.book_title(0), Some("Book Two"));
        assert_eq!(project.book_title(1), Some("Book One"));
        assert_eq!(project.active_book(), 1, "active follows the moved book");
        assert_eq!(project.manifest.title, "Book One");

        // No-op at the ends.
        assert!(!project.move_book(0, true).unwrap());
        assert!(!project.move_book(1, false).unwrap());

        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.book_title(0), Some("Book Two"));
        assert_eq!(reopened.book_title(1), Some("Book One"));

        fs::remove_dir_all(&dir).ok();
    }
}

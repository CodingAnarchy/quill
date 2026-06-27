//! The multi-book series layer (#328–#341, extracted in #350). A project can
//! hold several books; the *active* book is hoisted into [`Project::manifest`] so
//! every per-chapter path keeps working on it, while the other books are held on
//! [`Project`] and written back on save. This module owns the on-disk series
//! shape ([`SeriesFile`]/[`Book`]), the manifest ⇄ book hoisting, and the
//! per-book accessors. A standalone novel has no `Book` values at all — its
//! single book lives directly in the [`Manifest`], so its `book.toml` and every
//! code path here is unchanged (the empty-`books` branch).

use std::fs;

use serde::{Deserialize, Serialize};

use crate::frontmatter::{self, Epigraph, FrontMatter};

use super::{ChapterEntry, Goals, Manifest, Part, Project};

/// One book within a multi-book series (#276/#328). Holds the same per-book
/// structure a standalone novel carries — title, optional book metadata, the
/// optional `parts` overlay, and the ordered `chapters` — but not the
/// series-level fields (`author`, `start_date`, `todo_marker`), which live once
/// at the series root. A standalone novel has no `Book` values at all: its single
/// book lives directly in the [`Manifest`] (so its `book.toml` is unchanged).
///
/// Scalar keys precede the `goals`/`parts` tables and the `chapters` array, so
/// the TOML tables stay last (a TOML requirement) — same ordering rule as
/// `Manifest`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Book {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epigraph: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epigraph_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_on_threshold: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goals: Option<Goals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<Part>>,
    #[serde(default)]
    pub chapters: Vec<ChapterEntry>,
}

/// A multi-book project file: series-level metadata plus the ordered list of
/// [`Book`]s (#328). This is the on-disk shape *only* when a project holds more
/// than one book — recognized by a `books` array in `book.toml`. A standalone
/// novel keeps the flat [`Manifest`] shape, so its file is untouched.
///
/// In memory the *active* book is hoisted into [`Project::manifest`] (so every
/// existing per-chapter path keeps working on it); the other books are held on
/// [`Project`] and written back here on save.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
struct SeriesFile {
    /// The series title (the top-level `title` of a multi-book `book.toml`).
    title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    todo_marker: Option<String>,
    books: Vec<Book>,
}

impl Manifest {
    /// Parse a `book.toml` value into the active manifest plus the series book
    /// list and active-book index (#328). A non-empty `books` array marks a
    /// multi-book series; anything else is the flat single-book manifest, parsed
    /// exactly as before. Returns `(manifest, books, active_book)`. Called by
    /// [`Project::open`].
    pub(super) fn from_toml_value(value: toml::Value) -> Result<(Manifest, Vec<Book>, usize), toml::de::Error> {
        let is_series = value.get("books").and_then(|b| b.as_array()).is_some_and(|b| !b.is_empty());
        if is_series {
            let series: SeriesFile = value.try_into()?;
            // `is_series` was true, so the `[[books]]` array was non-empty and
            // deserializes to a non-empty Vec — a malformed/empty one would have
            // failed `try_into` above, not reached here.
            let first = series.books.first().expect("is_series guarantees a non-empty [[books]] array");
            let manifest = Manifest::from_series(&series, first);
            Ok((manifest, series.books, 0))
        } else {
            Ok((value.try_into()?, Vec::new(), 0))
        }
    }

    /// Build the active-book manifest from a series file and one of its books
    /// (#328): series-level fields come from the file root, per-book fields from
    /// the book. The inverse of [`Manifest::to_book`].
    fn from_series(series: &SeriesFile, book: &Book) -> Self {
        // Series-level fields come from the file root; the per-book fields are
        // filled in by `load_book`, and everything else is the manifest default.
        let mut manifest = Manifest {
            author: series.author.clone(),
            series: Some(series.title.clone()),
            start_date: series.start_date.clone(),
            todo_marker: series.todo_marker.clone(),
            ..Default::default()
        };
        manifest.load_book(book);
        manifest
    }

    /// Replace this manifest's per-book fields with `book`'s, leaving series-level
    /// fields (author, series, start_date, todo_marker) untouched. The inverse of
    /// [`to_book`](Manifest::to_book); hoists a book into the active manifest when
    /// opening (#328) or switching books (#329).
    fn load_book(&mut self, book: &Book) {
        self.title = book.title.clone();
        self.subtitle = book.subtitle.clone();
        self.epigraph = book.epigraph.clone();
        self.epigraph_by = book.epigraph_by.clone();
        self.part_label = book.part_label.clone();
        self.run_on_threshold = book.run_on_threshold;
        self.goals = book.goals.clone();
        self.parts = book.parts.clone();
        self.chapters = book.chapters.clone();
    }

    /// The per-book fields of this (active) manifest, for writing back into the
    /// series on save (#328). Series-level fields are dropped — they're held once
    /// at the series root.
    fn to_book(&self) -> Book {
        Book {
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            epigraph: self.epigraph.clone(),
            epigraph_by: self.epigraph_by.clone(),
            part_label: self.part_label.clone(),
            run_on_threshold: self.run_on_threshold,
            goals: self.goals.clone(),
            parts: self.parts.clone(),
            chapters: self.chapters.clone(),
        }
    }
}

/// Serialize a multi-book project to the `[[books]]` form (#328): fold the live
/// active book back into the series list and write the series file. Called by
/// [`Project::save_manifest`] for a multi-book project.
pub(super) fn series_to_toml(manifest: &Manifest, books: &[Book], active_book: usize) -> Result<String, toml::ser::Error> {
    let mut books = books.to_vec();
    books[active_book] = manifest.to_book();
    let series = SeriesFile {
        title: manifest.series.clone().unwrap_or_default(),
        author: manifest.author.clone(),
        start_date: manifest.start_date.clone(),
        todo_marker: manifest.todo_marker.clone(),
        books,
    };
    toml::to_string_pretty(&series)
}

impl Project {
    /// How many books the project holds — 1 for a standalone novel (#328).
    pub fn book_count(&self) -> usize {
        self.books.len().max(1)
    }

    /// The index of the active book within the series (0 for a standalone novel).
    pub fn active_book(&self) -> usize {
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
        // The old book's pending count changes no longer apply.
        self.words_changed.clear();
        // Cache the new book's chapters (word count + front matter) once, then
        // band-order them — the same work `open` does for the first book.
        for i in 0..self.manifest.chapters.len() {
            self.read_chapter_cache(i);
        }
        self.reorder_by_band();
        self.active = self.manifest.chapters.first().map_or(String::new(), |c| c.file.clone());
    }

    /// The title of book `i`. The active book's live title comes from the
    /// manifest; the others from the stored series list. `None` if out of range.
    pub fn book_title(&self, i: usize) -> Option<&str> {
        if self.books.is_empty() {
            (i == 0).then_some(self.manifest.title.as_str())
        } else if i == self.active_book {
            Some(self.manifest.title.as_str())
        } else {
            self.books.get(i).map(|b| b.title.as_str())
        }
    }

    /// The chapters of book `i` (#330): the live manifest list for the active
    /// book (band-ordered), or the stored book's list otherwise. Empty if out of
    /// range.
    pub fn book_chapters(&self, i: usize) -> &[ChapterEntry] {
        if self.books.is_empty() || i == self.active_book {
            &self.manifest.chapters
        } else {
            self.books.get(i).map_or(&[], |b| b.chapters.as_slice())
        }
    }

    /// The `parts` overlay of book `i`, if any (#330).
    pub fn book_parts(&self, i: usize) -> Option<&[Part]> {
        let parts = if self.books.is_empty() || i == self.active_book {
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
        if self.books.is_empty() || i == self.active_book {
            self.manifest.subtitle.as_deref()
        } else {
            self.books.get(i).and_then(|b| b.subtitle.as_deref())
        }
    }

    /// Book `i`'s epigraph, if any (#332).
    pub fn book_epigraph(&self, i: usize) -> Option<Epigraph> {
        if self.books.is_empty() || i == self.active_book {
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
        if self.books.is_empty() || i == self.active_book {
            self.manifest.part_label()
        } else {
            self.books.get(i).and_then(|b| b.part_label.as_deref()).unwrap_or("part")
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
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    fn temp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("quill-series-{}-{}", std::process::id(), name));
        p
    }

    /// Every per-book field hoisted into the active manifest by `load_book` must
    /// come back out unchanged via `to_book` (#427). `to_book`'s struct literal is
    /// already compiler-checked, so this specifically guards the assignment-based
    /// `load_book`: a `Book` field added without a matching `load_book` line fails
    /// loudly here instead of silently dropping on a series save.
    #[test]
    fn book_round_trips_through_the_active_manifest() {
        let original = Book {
            title: "Book Two".into(),
            subtitle: Some("The Reckoning".into()),
            epigraph: Some("In the deep.".into()),
            epigraph_by: Some("Anon".into()),
            part_label: Some("act".into()),
            run_on_threshold: Some(33),
            goals: Some(Goals { words: 90_000, chapters: 30, words_per_page: 250, daily: Some(800) }),
            parts: Some(vec![Part { name: Some("Act I".into()), chapters: [1, 5], goal: Some(20_000) }]),
            chapters: vec![ChapterEntry::new("One".into(), "one.md".into())],
        };

        let mut manifest = Manifest::default();
        manifest.load_book(&original);
        let round_tripped = manifest.to_book();

        assert_eq!(round_tripped, original, "every Book field must survive load_book → to_book");
    }

    /// Write a `books`-shaped series manifest with `books` books of `per_book`
    /// chapters each (and their files), for the multi-book perf guard.
    fn write_series_project(dir: &Path, books: usize, per_book: usize) -> String {
        let mut manifest = String::from("title = \"Series\"\n\n");
        for b in 0..books {
            fs::create_dir_all(dir.join(format!("b{b}"))).unwrap();
            manifest.push_str(&format!("[[books]]\ntitle = \"Book {b}\"\n\n"));
            for i in 0..per_book {
                let file = format!("b{b}/ch{i}.md");
                manifest.push_str(&format!("[[books.chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"));
                let mut body = String::from("---\nstatus: draft\npov: Mara\n---\n# Chapter\n\n");
                for _ in 0..50 {
                    body.push_str("The night was dark and the road was very long indeed.\n");
                }
                fs::write(dir.join(&file), body).unwrap();
            }
        }
        manifest
    }

    /// Opening a series-sized *multi-book* project (the new `[[books]]` format,
    /// #328) stays within the same budget as the single-book series guard. It
    /// runs the books-array parse, the active-book hoist, and (since #331) one
    /// read per chapter across every book to cache word counts for series
    /// rollups — the same total file work as a single book of the same size, so
    /// it stays linear. Catches a regression to O(n²) parse/cache work on the
    /// larger manifest.
    #[test]
    #[ignore]
    fn open_is_fast_for_a_multi_book_series() {

        let (books, per_book) = (6, 250); // 1500 chapters across the series
        let dir = temp_dir("perf-open-multibook");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let manifest = write_series_project(&dir, books, per_book);
        fs::write(Project::manifest_path(&dir), manifest).unwrap();

        let t = Instant::now();
        let project = Project::open(&dir).unwrap();
        let dt = t.elapsed();

        println!("open {books}-book series ({} chapters total) = {dt:?}", books * per_book);
        assert_eq!(project.book_count(), books);
        assert_eq!(project.manifest.chapters.len(), per_book, "active book is hoisted");
        assert!(dt.as_millis() < 500, "open took {dt:?}");

        fs::remove_dir_all(&dir).ok();
    }

    /// Write a two-book series manifest (and its chapter files) to `dir`.
    fn write_series(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
        for sub in ["b1", "b2"] {
            fs::create_dir_all(dir.join(sub)).unwrap();
        }
        for file in ["b1/one.md", "b1/two.md", "b2/one.md"] {
            fs::write(dir.join(file), "prose\n").unwrap();
        }
        let manifest = "\
title = \"The Saga\"
author = \"A. Writer\"

[[books]]
title = \"Book One\"

[[books.chapters]]
title = \"Dawn\"
file = \"b1/one.md\"

[[books.chapters]]
title = \"Noon\"
file = \"b1/two.md\"

[[books]]
title = \"Book Two\"

[[books.chapters]]
title = \"Dusk\"
file = \"b2/one.md\"
";
        fs::write(Project::manifest_path(dir), manifest).unwrap();
    }

    #[test]
    fn a_multi_book_series_opens_on_the_first_book() {
        let dir = temp_dir("series-open");
        write_series(&dir);

        let p = Project::open(&dir).unwrap();
        assert_eq!(p.book_count(), 2);
        assert_eq!(p.active_book(), 0);
        assert_eq!(p.series_title(), Some("The Saga"));
        assert_eq!(p.book_title(0), Some("Book One"));
        assert_eq!(p.book_title(1), Some("Book Two"));
        assert_eq!(p.book_title(2), None);
        // The active book's structure is hoisted into the manifest, so every
        // existing per-chapter path works on book one unchanged.
        assert_eq!(p.manifest.title, "Book One");
        assert_eq!(p.manifest.author.as_deref(), Some("A. Writer"));
        let titles: Vec<&str> = p.manifest.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Dawn", "Noon"]);
        assert_eq!(p.active_file(), "b1/one.md");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_series_round_trips_through_save_and_reopen() {
        let dir = temp_dir("series-roundtrip");
        write_series(&dir);

        // Open, save without structural change, reopen.
        Project::open(&dir).unwrap().save_manifest().unwrap();
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        // Series-level scalars precede the [[books]] tables (valid TOML), and the
        // inactive second book is preserved verbatim.
        assert!(toml.find("title =").unwrap() < toml.find("[[books]]").unwrap(), "{toml}");
        assert!(toml.contains("Book Two") && toml.contains("b2/one.md"), "inactive book preserved:\n{toml}");

        let p = Project::open(&dir).unwrap();
        assert_eq!(p.book_count(), 2);
        assert_eq!(p.book_title(0), Some("Book One"));
        assert_eq!(p.book_title(1), Some("Book Two"));
        assert_eq!(p.series_title(), Some("The Saga"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_active_book_hoists_the_target_and_round_trips() {
        let dir = temp_dir("series-switch");
        write_series(&dir);
        let mut p = Project::open(&dir).unwrap();

        p.set_active_book(1);
        assert_eq!(p.active_book(), 1);
        assert_eq!(p.manifest.title, "Book Two");
        let titles: Vec<&str> = p.manifest.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Dusk"]);
        assert_eq!(p.active_file(), "b2/one.md");
        assert_eq!(p.book_title(0), Some("Book One")); // the stored book is preserved
        assert_eq!(p.book_title(1), Some("Book Two")); // the active book reads from the manifest

        // Switching back restores book one; saving keeps both books.
        p.set_active_book(0);
        assert_eq!(p.manifest.title, "Book One");
        p.save_manifest().unwrap();
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(toml.contains("Book One") && toml.contains("Book Two") && toml.contains("b2/one.md"), "{toml}");

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
    #[ignore]
    fn switching_books_is_fast_in_a_series() {

        let (books, per_book) = (2, 750);
        let dir = temp_dir("perf-switch-book");
        let _ = fs::remove_dir_all(&dir);
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

    #[test]
    fn a_standalone_novel_is_unchanged_by_the_series_model() {
        let dir = temp_dir("series-single");
        let _ = fs::remove_dir_all(&dir);
        let mut project = Project::create(&dir).unwrap(); // flat single-book manifest
        project.manifest.series = Some("A label, not a series".into());
        project.save_manifest().unwrap();

        // No book lives in the series list, so it's not a multi-book project.
        assert_eq!(project.book_count(), 1);
        assert_eq!(project.active_book(), 0);
        assert_eq!(project.series_title(), None); // a standalone `series` is just a label
        assert_eq!(project.book_title(0), Some(project.manifest.title.as_str()));

        // The flat manifest is serialized exactly as before — no books table.
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(!toml.contains("[[books]]") && !toml.contains("\nbooks "), "no series table:\n{toml}");
        assert!(Project::open(&dir).is_ok());

        fs::remove_dir_all(&dir).ok();
    }
}

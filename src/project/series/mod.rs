//! The multi-book series layer (#328–#341, extracted in #350). A project can
//! hold several books; the *active* book is hoisted into [`Project::manifest`] so
//! every per-chapter path keeps working on it, while the other books are held on
//! [`Project`] and written back on save. This module owns the on-disk series
//! shape ([`SeriesFile`]/[`Book`]) and the manifest ⇄ book hoisting; the
//! `Project`-level book management (add/remove/reorder/switch and the per-book
//! accessors) lives in the sibling [`books`] submodule. A standalone novel has no
//! `Book` values at all — its single book lives directly in the [`Manifest`], so
//! its `book.toml` and every code path here is unchanged (the empty-`books`
//! branch).

use serde::{Deserialize, Serialize};

use super::{ChapterEntry, Goals, Manifest, Part};

mod books;

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
    /// Per-book publishing metadata (#810): each book in a series is a distinct
    /// product with its own ISBN (KDP/IngramSpark treat each ISBN as a separate
    /// title) and often its own copyright year / rights holder, so these ride per
    /// `[[books]]` entry rather than once at the series root. All optional; a book
    /// that sets none falls back to the series/active-book metadata at export time
    /// (see [`Project::book_publishing`]). Language is a package-level concern, so
    /// it stays series-level on the [`Manifest`] and is not repeated here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goals: Option<Goals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<Part>>,
    #[serde(default)]
    pub chapters: Vec<ChapterEntry>,
}

/// A book's resolved publishing metadata (#810) — each field already resolved
/// per book, falling back to the series/active-book value when the book leaves it
/// unset. Produced by [`Project::book_publishing`] and turned into an
/// `export::PubMeta` by the export layer (which adds the package-level language).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BookPublishing {
    pub isbn: Option<String>,
    pub publisher: Option<String>,
    pub pub_date: Option<String>,
    pub rights: Option<String>,
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
    pub(super) fn from_toml_value(
        value: toml::Value,
    ) -> Result<(Self, Vec<Book>, usize), toml::de::Error> {
        let is_series = value
            .get("books")
            .and_then(|b| b.as_array())
            .is_some_and(|b| !b.is_empty());
        if is_series {
            let series: SeriesFile = value.try_into()?;
            // `is_series` was true, so the `[[books]]` array was non-empty and
            // deserializes to a non-empty Vec; `if let` binds the first book without
            // a panic path. An (impossible) empty Vec has no active book to hoist, so
            // fall back to an empty single-book manifest rather than panicking.
            if let Some(first) = series.books.first() {
                let manifest = Self::from_series(&series, first);
                return Ok((manifest, series.books, 0));
            }
            return Ok((Self::default(), series.books, 0));
        }
        Ok((value.try_into()?, Vec::new(), 0))
    }

    /// Build the active-book manifest from a series file and one of its books
    /// (#328): series-level fields come from the file root, per-book fields from
    /// the book. The inverse of [`Manifest::to_book`].
    fn from_series(series: &SeriesFile, book: &Book) -> Self {
        // Series-level fields come from the file root; the per-book fields are
        // filled in by `load_book`, and everything else is the manifest default.
        let mut manifest = Self {
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
        self.title.clone_from(&book.title);
        self.subtitle.clone_from(&book.subtitle);
        self.epigraph.clone_from(&book.epigraph);
        self.epigraph_by.clone_from(&book.epigraph_by);
        self.part_label.clone_from(&book.part_label);
        self.run_on_threshold = book.run_on_threshold;
        self.isbn.clone_from(&book.isbn);
        self.publisher.clone_from(&book.publisher);
        self.pub_date.clone_from(&book.pub_date);
        self.rights.clone_from(&book.rights);
        self.goals.clone_from(&book.goals);
        self.parts.clone_from(&book.parts);
        self.chapters.clone_from(&book.chapters);
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
            isbn: self.isbn.clone(),
            publisher: self.publisher.clone(),
            pub_date: self.pub_date.clone(),
            rights: self.rights.clone(),
            goals: self.goals.clone(),
            parts: self.parts.clone(),
            chapters: self.chapters.clone(),
        }
    }
}

/// Serialize a multi-book project to the `[[books]]` form (#328): fold the live
/// active book back into the series list and write the series file. Called by
/// [`Project::save_manifest`] for a multi-book project.
pub(super) fn series_to_toml(
    manifest: &Manifest,
    books: &[Book],
    active_book: usize,
) -> Result<String, toml::ser::Error> {
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

#[cfg(test)]
pub(super) mod tests {
    use std::fmt::Write;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use super::super::Project;
    use super::{Book, ChapterEntry, Goals, Manifest, Part};

    pub(super) fn temp_dir(name: &str) -> PathBuf {
        crate::testutil::scratch_dir("series", name)
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
            isbn: Some("978-1-11-111111-1".into()),
            publisher: Some("Second Press".into()),
            pub_date: Some("2026".into()),
            rights: Some("© 2026 A. Writer".into()),
            goals: Some(Goals {
                words: 90_000,
                chapters: 30,
                words_per_page: 250,
                daily: Some(800),
                deadline: None,
            }),
            parts: Some(vec![Part {
                name: Some("Act I".into()),
                chapters: [1, 5],
                goal: Some(20_000),
            }]),
            chapters: vec![ChapterEntry::new("One".into(), "one.md".into())],
        };

        let mut manifest = Manifest::default();
        manifest.load_book(&original);
        let round_tripped = manifest.to_book();

        assert_eq!(
            round_tripped, original,
            "every Book field must survive load_book → to_book"
        );
    }

    /// Write a `books`-shaped series manifest with `books` books of `per_book`
    /// chapters each (and their files), for the multi-book perf guard.
    pub(super) fn write_series_project(dir: &Path, books: usize, per_book: usize) -> String {
        let mut manifest = String::from("title = \"Series\"\n\n");
        for b in 0..books {
            fs::create_dir_all(dir.join(format!("b{b}"))).unwrap();
            write!(manifest, "[[books]]\ntitle = \"Book {b}\"\n\n").unwrap();
            for i in 0..per_book {
                let file = format!("b{b}/ch{i}.md");
                write!(
                    manifest,
                    "[[books.chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
                )
                .unwrap();
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
    #[ignore = "release-only perf guard; run via cargo perf"]
    fn open_is_fast_for_a_multi_book_series() {
        let (books, per_book) = (6, 250); // 1500 chapters across the series
        let dir = temp_dir("perf-open-multibook");
        fs::create_dir_all(&dir).unwrap();
        let manifest = write_series_project(&dir, books, per_book);
        fs::write(Project::manifest_path(&dir), manifest).unwrap();

        let t = Instant::now();
        let project = Project::open(&dir).unwrap();
        let dt = t.elapsed();

        println!(
            "open {books}-book series ({} chapters total) = {dt:?}",
            books * per_book
        );
        assert_eq!(project.book_count(), books);
        assert_eq!(
            project.manifest.chapters.len(),
            per_book,
            "active book is hoisted"
        );
        assert!(dt.as_millis() < 500, "open took {dt:?}");

        fs::remove_dir_all(&dir).ok();
    }

    /// Write a two-book series manifest (and its chapter files) to `dir`.
    pub(super) fn write_series(dir: &Path) {
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
        let titles: Vec<&str> = p
            .manifest
            .chapters
            .iter()
            .map(|c| c.title.as_str())
            .collect();
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
        assert!(
            toml.find("title =").unwrap() < toml.find("[[books]]").unwrap(),
            "{toml}"
        );
        assert!(
            toml.contains("Book Two") && toml.contains("b2/one.md"),
            "inactive book preserved:\n{toml}"
        );

        let p = Project::open(&dir).unwrap();
        assert_eq!(p.book_count(), 2);
        assert_eq!(p.book_title(0), Some("Book One"));
        assert_eq!(p.book_title(1), Some("Book Two"));
        assert_eq!(p.series_title(), Some("The Saga"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_standalone_novel_is_unchanged_by_the_series_model() {
        let dir = temp_dir("series-single");
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
        assert!(
            !toml.contains("[[books]]") && !toml.contains("\nbooks "),
            "no series table:\n{toml}"
        );
        assert!(Project::open(&dir).is_ok());

        fs::remove_dir_all(&dir).ok();
    }
}

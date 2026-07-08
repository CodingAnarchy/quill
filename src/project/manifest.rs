//! The on-disk manifest types and the pure structure helpers that read them:
//! a chapter's entry ([`ChapterEntry`], with its transient front-matter caches),
//! the per-project analysis overrides ([`Analysis`]), a chapter grouping
//! ([`Part`]), and the manifest itself ([`Manifest`]) with its metadata
//! accessors (epigraph, TODO marker, part label, run-on threshold, part-of).
//! The band/part mapping free functions ([`front_chapter_count`],
//! [`part_at_body_position`]) live here too, shared with the series tree so the
//! mapping can't drift. Extracted from `mod.rs` to keep that file under the
//! ~1,000-line convention (#894) — a pure move, no behavior change.

use serde::{Deserialize, Serialize};

use crate::frontmatter::{Band, Epigraph, FrontMatter, Role, Status};

use super::Goals;

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
    /// Cached count of the TODO marker (#867) in this chapter's prose, populated
    /// from the file at open (alongside `words`) and refreshed as the chapter is
    /// edited. Transient like `words` — never written to `book.toml` (`serde(skip)`);
    /// lets the save-path TODO roll-up sum cached counts instead of re-reading and
    /// re-scanning every chapter from disk on every save.
    #[serde(skip)]
    pub todo: usize,
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
    pub(super) const fn new(title: String, file: String) -> Self {
        Self {
            title,
            file,
            words: 0,
            todo: 0,
            status: None,
            summary: None,
            pov: None,
            goal: None,
            role: None,
        }
    }

    /// Refresh the cached front-matter metadata from `front` (none clears it).
    /// The chapter file is the source of truth; this populates the transient
    /// caches that power the pane, counts, and tree without re-reading.
    pub(super) fn cache_meta(&mut self, front: Option<&FrontMatter>) {
        self.status = front.and_then(FrontMatter::status);
        self.summary = front.and_then(FrontMatter::summary);
        self.pov = front.and_then(FrontMatter::pov);
        self.goal = front.and_then(FrontMatter::goal);
        self.role = front.and_then(FrontMatter::role);
    }
}

/// Per-project overrides for the analysis report's frequency word lists (#119),
/// from the `[analysis]` table in `book.toml`. Each list *adjusts* the built-in
/// defaults rather than replacing them — `*_add` extends, `*_remove` drops an
/// entry — so an author needn't restate the whole default list. Words are matched
/// lowercased. Absent (and empty lists) means the built-in defaults, unchanged.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Words to also exclude from the most-used-words list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stopwords_add: Vec<String>,
    /// Built-in stopwords to stop excluding (so they can surface in the list).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stopwords_remove: Vec<String>,
    /// Words to also count as filtering verbs in the style-flags section.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter_words_add: Vec<String>,
    /// Built-in filter words to stop counting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filter_words_remove: Vec<String>,
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
// Not `Eq`: the print config carries a float bleed allowance (#652).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
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
    /// Publishing metadata (#583): the ISBN (the EPUB `dc:identifier` when set),
    /// publisher, publication date (`YYYY` or `YYYY-MM-DD`), BCP-47 language tag
    /// (drives the EPUB `dc:language`/`xml:lang`, defaulting to `en`), and the
    /// rights/copyright line. Each rides into the EPUB OPF / PDF / DOCX and the
    /// generated copyright page; all optional and unset fields aren't written back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights: Option<String>,
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
    /// Opt-in designed-export styling (#468): the `[style]` table the EPUB (and,
    /// later, PDF) writers read for the scene-break ornament and the rest of the
    /// styling epic (#465). Absent → the plain default output. A table, so it must
    /// precede the `goals`/`parts`/`chapters` tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<crate::export::ExportStyle>,
    /// Opt-in print-interior settings (#464): the `[print]` table the print-ready
    /// PDF reads for its trim size. Absent → the default 6×9 trim. A table, so it
    /// must precede the `goals`/`parts`/`chapters` tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print: Option<crate::export::pdf::PrintConfig>,
    /// Per-project overrides for the analysis report's word lists (#119): add to
    /// or remove from the built-in stopword and filter-word lists. A table, so it
    /// must precede the `goals`/`parts`/`chapters` tables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<Analysis>,
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
pub fn front_chapter_count(chapters: &[ChapterEntry]) -> usize {
    chapters
        .iter()
        .filter(|c| Band::of(c.role) == Band::Front)
        .count()
}

/// The 0-based part containing 1-based **body** position `body_pos`, if any. Part
/// ranges are inclusive 1-based positions among the body chapters (#185). Shared
/// by [`Manifest::part_of`] and the series tree (#330) so the mapping can't drift.
pub fn part_at_body_position(parts: &[Part], body_pos: usize) -> Option<usize> {
    parts
        .iter()
        .position(|p| p.chapters[0] <= body_pos && body_pos <= p.chapters[1])
}

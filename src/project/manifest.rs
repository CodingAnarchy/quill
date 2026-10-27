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

use super::{Form, Goals};

/// The current on-disk `book.toml` format version (#1097). Bumped only for a
/// *breaking* format change — one that an older Quill couldn't read correctly, so
/// the migration hook ([`Manifest::migrate`]) must translate it forward on open.
/// A manifest with no `format_version` is treated as this version (every
/// pre-#1097 file omitted the key, and nothing about the shape changed when it was
/// introduced), so existing projects load unchanged.
///
/// The compatibility promise: a `book.toml` written by any 1.x opens in every
/// other 1.x. Additive fields don't bump this — unknown keys are ignored on load
/// (a newer version's field is simply not seen by an older one, never an error).
/// The version + migration hook is the escape valve for the rare change that
/// *can't* be additive.
pub const CURRENT_FORMAT_VERSION: u32 = 1;

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
/// chapter (see [`docs/progress-reports.md`]).
///
/// # Invariant
///
/// `chapters` is `[start, end]` where both endpoints are **1-based positions
/// among the *body* chapters** (front/back-matter sections belong to no part,
/// #185), the range is **inclusive**, and it is **well-formed**: both endpoints
/// are `>= 1` and `start <= end`. Nothing in `serde` enforces this — a
/// hand-edited `book.toml` can violate it — so [`normalize_parts`] repairs or
/// drops a violating part at load ([`Project::open`]) before any mapping code
/// (`part_at_body_position`, the progress report's `counts.rs`) reads it. An
/// inverted range (`[end, start]`) would otherwise silently map to no part
/// (`part_at_body_position` never matches `chapters[0] <= pos <= chapters[1]`).
///
/// `name` is optional; the report auto-labels an unnamed part `"{label} {n}"`
/// using [`Manifest::part_label`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Part {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Inclusive, well-formed 1-based body-chapter range `[start, end]`
    /// (`1 <= start <= end`). See the type's invariant.
    pub chapters: [usize; 2],
    /// Optional word-count goal for the part (#146), set in `book.toml`. When
    /// present, the progress report shows the part's words against it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<usize>,
}

/// Repair a hand-edited `parts` list to the [`Part`] invariant, in place, at
/// load: an inverted range (`[end, start]`, e.g. `[3, 1]`) is swapped back to
/// `[start, end]` rather than silently mapping to no part, and a part with a
/// zero endpoint (a `0` is invalid for a 1-based range — the manifest is
/// hand-broken) is dropped with a warning. Called by [`Project::open`] on the
/// active manifest and every series book's parts, so no downstream mapping
/// (`part_at_body_position`, the progress report's `counts.rs`) ever reads a
/// malformed range.
pub fn normalize_parts(parts: &mut Vec<Part>) {
    parts.retain_mut(|p| {
        if p.chapters[0] == 0 || p.chapters[1] == 0 {
            eprintln!(
                "quill: dropping malformed part range {:?} (a part range is 1-based; 0 is invalid)",
                p.chapters
            );
            return false;
        }
        if p.chapters[0] > p.chapters[1] {
            eprintln!(
                "quill: normalizing inverted part range [{}, {}] to [{}, {}]",
                p.chapters[0], p.chapters[1], p.chapters[1], p.chapters[0]
            );
            p.chapters.swap(0, 1);
        }
        true
    });
}

/// Adjust part ranges when a body chapter is **inserted** at 1-based body
/// position `body_pos` (the new chapter takes that position; every body chapter
/// formerly at position `q >= body_pos` is now at `q + 1`). Part ranges are
/// inclusive 1-based body positions ([`Part`]), and none of the structural edits
/// otherwise touch them (#316), so without this call a part would silently
/// regroup the wrong chapters after an insert (#… — parts follow structural edits).
///
/// Boundary convention: inserting **exactly at a part's start edge**
/// (`body_pos == start`) pushes the whole part down (`[start + 1, end + 1]`) —
/// the new chapter lands *before* the part's first chapter rather than joining
/// it. A chapter inserted strictly inside a part (`start < body_pos <= end`)
/// grows it (`[start, end + 1]`). A part entirely after the insert
/// (`body_pos <= start`) shifts up by one; a part entirely before it is untouched.
pub fn shift_parts_for_insert(parts: &mut [Part], body_pos: usize) {
    for p in parts.iter_mut() {
        let [start, end] = p.chapters;
        if body_pos <= start {
            // Insert at or before the part's first chapter: the whole part slides
            // down by one (start-edge inserts push the part, don't join it).
            p.chapters = [start + 1, end + 1];
        } else if body_pos <= end {
            // Insert strictly inside the part: it grows to cover the new chapter.
            p.chapters = [start, end + 1];
        }
        // body_pos > end: the insert is after the part — the part is untouched.
    }
}

/// Adjust part ranges when the body chapter at 1-based body position `body_pos`
/// is **deleted** (every body chapter formerly at `q > body_pos` is now at
/// `q - 1`). A part containing `body_pos` shrinks by one; a part entirely after
/// it shifts down by one; a part that shrinks to nothing (its only chapter was
/// deleted) is **dropped**. The counterpart to [`shift_parts_for_insert`] — the
/// two keep parts consistent with the chapter list across every structural edit.
pub fn shift_parts_for_delete(parts: &mut Vec<Part>, body_pos: usize) {
    parts.retain_mut(|p| {
        let [start, end] = p.chapters;
        if body_pos < start {
            // Delete before the part: the whole part slides down by one.
            p.chapters = [start - 1, end - 1];
        } else if body_pos <= end {
            // Delete inside (or at an edge of) the part: it shrinks by one. When
            // the part held a single chapter (start == end == body_pos) it now
            // spans nothing — drop it rather than leave a zero-length range.
            if start == end {
                return false;
            }
            p.chapters = [start, end - 1];
        }
        // body_pos > end: the delete is after the part — the part is untouched.
        true
    });
}

/// The on-disk manifest (`book.toml`), deserialized.
///
/// `title` is the book title; `subtitle`, `author`, and `series` are optional
/// book/series metadata. New optional fields default to absent so manifests
/// written before they existed still parse, and unset fields aren't written
/// back. Scalar keys are declared before the `goals` table and the `chapters`
/// array, so the TOML tables stay last (a TOML requirement).
///
/// Top-level keys this struct doesn't model — the `[quill]` per-project config
/// overrides (#710), or an additive key from a newer 1.x — aren't fields here and
/// so don't survive a plain re-serialization; [`Project::save_manifest`] carries
/// them over from the file on disk (see `extra_keys`). Unknown keys nested inside
/// a modeled table (`[goals]`, a `[[chapters]]` entry, …) are still dropped.
///
/// For a multi-book series (#328), this holds the *active* book's structure;
/// `title` is then the active book's title, `series` is the series title, and the
/// full book list lives on [`Project`].
// Not `Eq`: the print config carries a float bleed allowance (#652).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Manifest {
    /// On-disk format version (#1097): the schema-version escape valve for a future
    /// breaking format change. Absent on every pre-#1097 file, so it reads as
    /// [`CURRENT_FORMAT_VERSION`] and existing projects are unaffected. A scalar
    /// key, so it must precede the `goals`/`parts`/`chapters` tables. `migrate` runs
    /// it forward on open; `save_manifest` stamps the current version on write.
    ///
    /// Unknown *additive* fields never touch this — they're ignored on load, so a
    /// newer 1.x's extra key opens cleanly in an older 1.x (the compat guarantee).
    /// This is bumped only for a change an older reader would misread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_version: Option<u32>,
    /// What kind of work this is (#1239): a novel (the default, never written) or
    /// a short story. A scalar key, so it precedes the tables.
    #[serde(default, skip_serializing_if = "Form::is_novel")]
    pub form: Form,
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

    /// The on-disk format version this manifest was written at (#1097), defaulting
    /// to [`CURRENT_FORMAT_VERSION`] when the key is absent (every pre-#1097 file).
    pub fn format_version(&self) -> u32 {
        self.format_version.unwrap_or(CURRENT_FORMAT_VERSION)
    }

    /// Migrate an older on-disk format forward to [`CURRENT_FORMAT_VERSION`] on open
    /// (#1097). The escape valve for a genuinely-breaking future format change: each
    /// version bump adds one arm here that translates `v` → `v + 1` in place, then
    /// stamps the reached version. A manifest already at (or ahead of) the current
    /// version — which includes every unversioned pre-#1097 file, since an absent
    /// version reads as current — is left untouched, so an existing project that
    /// only loads and re-saves stays byte-stable (it doesn't sprout a version key it
    /// never had).
    ///
    /// Today there is exactly one version, so this is a no-op; the shape is the
    /// real, tested seam a future breaking change hangs its upgrade step on (rather
    /// than a hard break at load). Called by [`Project::open`].
    pub fn migrate(&mut self) {
        // Already at (or ahead of) the current version: nothing to translate, and
        // no re-stamp — an unversioned pre-#1097 file reads as the current version,
        // so it's left exactly as-is until the author's next save.
        if self.format_version() >= CURRENT_FORMAT_VERSION {
            return;
        }
        // Each future breaking bump inserts its translation here, walking the
        // version forward one step at a time, e.g.:
        //   if self.format_version() < 2 { /* v1 -> v2 rewrite */ }
        // then stamp the reached version:
        self.format_version = Some(CURRENT_FORMAT_VERSION);
    }

    /// How many leading chapters are front matter (#185) — the offset of the
    /// contiguous body block after the band reorder. The single source for the
    /// front-to-body mapping the parts logic relies on.
    pub fn front_chapter_count(&self) -> usize {
        front_chapter_count(&self.chapters)
    }

    /// Whether this project is a short story (#1239): the short-story form with
    /// exactly one body chapter. A short story an older Quill grew a second
    /// chapter onto reads as a novel, so nothing hides chapters it really has.
    pub fn is_short_story(&self) -> bool {
        self.form == Form::ShortStory && self.body_chapter_count() == 1
    }

    /// How many chapters are body chapters — neither front nor back matter.
    pub fn body_chapter_count(&self) -> usize {
        self.chapters
            .iter()
            .filter(|c| Band::of(c.role) == Band::Body)
            .count()
    }

    /// The index of a short story's one body chapter (#1239), or `None` for a novel.
    pub fn story_chapter(&self) -> Option<usize> {
        if !self.is_short_story() {
            return None;
        }
        self.chapters
            .iter()
            .position(|c| Band::of(c.role) == Band::Body)
    }

    /// Keep a short story's chapter title equal to the project title (#1239): the
    /// story has no separate chapter title to drift. A no-op for a novel.
    pub fn sync_story_title(&mut self) {
        if let Some(i) = self.story_chapter() {
            if self.chapters[i].title != self.title {
                self.chapters[i].title.clone_from(&self.title);
            }
        }
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

#[cfg(test)]
mod tests {
    use super::{
        normalize_parts, part_at_body_position, shift_parts_for_delete, shift_parts_for_insert,
        Manifest, Part, CURRENT_FORMAT_VERSION,
    };

    fn part(range: [usize; 2]) -> Part {
        Part {
            name: None,
            chapters: range,
            goal: None,
        }
    }

    fn ranges(parts: &[Part]) -> Vec<[usize; 2]> {
        parts.iter().map(|p| p.chapters).collect()
    }

    // Inserting strictly inside a part grows it; a part after the insert shifts up
    // by one so it keeps grouping the SAME chapters — not silently regrouping.
    #[test]
    fn insert_inside_grows_the_part_and_shifts_later_parts() {
        let mut parts = vec![part([1, 3]), part([4, 6])];
        shift_parts_for_insert(&mut parts, 2); // inside part 0
        assert_eq!(
            ranges(&parts),
            vec![[1, 4], [5, 7]],
            "part 0 grows to swallow the new chapter; part 1 shifts up intact"
        );
    }

    // Boundary convention: inserting exactly at a part's start edge pushes the
    // whole part down (the new chapter lands before it), rather than joining it.
    #[test]
    fn insert_at_start_edge_pushes_the_part_down() {
        let mut parts = vec![part([2, 4])];
        shift_parts_for_insert(&mut parts, 2); // == start
        assert_eq!(
            ranges(&parts),
            vec![[3, 5]],
            "start-edge insert pushes the part; it does not join it"
        );
    }

    // Inserting after a part leaves it untouched; inserting at the end edge grows
    // it (the end edge is inside the inclusive range).
    #[test]
    fn insert_after_is_a_noop_and_end_edge_grows() {
        let mut after = vec![part([1, 2])];
        shift_parts_for_insert(&mut after, 3); // strictly after end
        assert_eq!(
            ranges(&after),
            vec![[1, 2]],
            "insert after the part is a no-op"
        );

        let mut edge = vec![part([1, 3])];
        shift_parts_for_insert(&mut edge, 3); // == end, still inside
        assert_eq!(
            ranges(&edge),
            vec![[1, 4]],
            "end-edge insert grows the part"
        );
    }

    // Deleting inside a part shrinks it; a later part shifts down; a part whose
    // only chapter is deleted is dropped rather than left zero-length.
    #[test]
    fn delete_shrinks_shifts_and_drops_emptied_parts() {
        let mut parts = vec![part([1, 3]), part([4, 4]), part([5, 6])];
        shift_parts_for_delete(&mut parts, 4); // the sole chapter of part 1
        assert_eq!(
            ranges(&parts),
            vec![[1, 3], [4, 5]],
            "the single-chapter part drops; the later part shifts down intact"
        );

        shift_parts_for_delete(&mut parts, 2); // inside part 0
        assert_eq!(
            ranges(&parts),
            vec![[1, 2], [3, 4]],
            "part 0 shrinks; the later part shifts down"
        );
    }

    // Insert then delete at the same position is an identity on the ranges — the
    // two helpers are inverses at a shared boundary (guards against drift).
    #[test]
    fn insert_then_delete_is_identity() {
        let original = vec![part([1, 3]), part([4, 6])];
        let mut parts = original.clone();
        shift_parts_for_insert(&mut parts, 2);
        shift_parts_for_delete(&mut parts, 2);
        assert_eq!(
            ranges(&parts),
            ranges(&original),
            "insert+delete cancels out"
        );
    }

    // The mapping stays consistent: after growing a part, every original chapter
    // still resolves into it via `part_at_body_position`.
    #[test]
    fn shift_keeps_the_position_mapping_consistent() {
        let mut parts = vec![part([1, 2]), part([3, 4])];
        // Insert a new chapter at body position 1 (before part 0). Every original
        // chapter's position moves up by one; the parts must move with them.
        shift_parts_for_insert(&mut parts, 1);
        assert_eq!(ranges(&parts), vec![[2, 3], [4, 5]]);
        // Original chapter 1 (now at position 2) is still in part 0; original
        // chapter 3 (now at position 4) is still in part 1.
        assert_eq!(part_at_body_position(&parts, 2), Some(0));
        assert_eq!(part_at_body_position(&parts, 4), Some(1));
        // The freshly inserted chapter at position 1 belongs to no part.
        assert_eq!(part_at_body_position(&parts, 1), None);
    }

    // A hand-edited `book.toml` can invert a part range (`[3, 1]`). Left as-is it
    // silently maps to no part — `part_at_body_position` never matches a position
    // in the range. `normalize_parts` swaps it back so chapters 1–3 map correctly.
    #[test]
    fn inverted_part_range_is_normalized_on_load() {
        let mut parts = vec![part([3, 1])];

        // Before repair, the inverted range matches nothing in 1..=3.
        assert_eq!(
            part_at_body_position(&parts, 2),
            None,
            "an inverted range silently maps to no part"
        );

        normalize_parts(&mut parts);

        assert_eq!(parts.len(), 1, "the part is repaired, not dropped");
        assert_eq!(parts[0].chapters, [1, 3], "endpoints are swapped in order");
        for pos in 1..=3 {
            assert_eq!(
                part_at_body_position(&parts, pos),
                Some(0),
                "chapter {pos} now maps into the repaired part"
            );
        }
    }

    // A zero endpoint is invalid for a 1-based range (a hand-broken manifest); the
    // part is dropped rather than mismapping. Well-formed parts survive untouched.
    #[test]
    fn zero_based_part_range_is_dropped_and_valid_ones_kept() {
        let mut parts = vec![part([0, 2]), part([1, 4]), part([5, 0])];
        normalize_parts(&mut parts);
        assert_eq!(
            parts.iter().map(|p| p.chapters).collect::<Vec<_>>(),
            vec![[1, 4]],
            "both zero-endpoint parts drop; the well-formed one is kept as-is"
        );
    }

    // COMPAT (#1097): a `book.toml` written by a *newer* Quill — carrying keys this
    // version has never heard of, at the top level, inside `[goals]`, and inside a
    // `[[parts]]` entry — must open cleanly with no error and no loss of the fields
    // this version *does* understand. This is the core 1.x forward-compat promise:
    // an additive future field is ignored, never a hard break.
    #[test]
    fn a_manifest_with_unknown_future_keys_opens_cleanly() {
        let toml = "\
title = \"My Book\"
author = \"A. Writer\"
future_scalar = \"from a newer version\"
run_on_threshold = 30

[goals]
daily = 500
future_goal_field = 42

[[parts]]
name = \"Act I\"
chapters = [1, 3]
future_part_field = true

[[chapters]]
title = \"One\"
file = \"chapter-1.md\"
future_chapter_field = \"ignored\"
";
        let manifest: Manifest =
            toml::from_str(toml).expect("unknown future keys must not fail the load");
        // The known fields survived intact — no data loss.
        assert_eq!(manifest.title, "My Book");
        assert_eq!(manifest.author.as_deref(), Some("A. Writer"));
        assert_eq!(manifest.run_on_threshold(), 30);
        assert_eq!(manifest.chapters.len(), 1);
        assert_eq!(manifest.chapters[0].file, "chapter-1.md");
        let parts = manifest.parts.expect("parts parsed");
        assert_eq!(parts[0].chapters, [1, 3]);
        assert_eq!(parts[0].name.as_deref(), Some("Act I"));
        assert!(manifest.goals.is_some(), "the [goals] table still parsed");
    }

    // The typed struct alone does NOT round-trip a key it doesn't model: an
    // unknown top-level key is gone from its serialization. `save_manifest` is what
    // carries such keys (the `[quill]` overrides, a newer 1.x's additive field)
    // over from the file on disk (`extra_keys`); this pins that the struct itself
    // stays a plain typed view, so that merge step is what preserves them.
    #[test]
    fn the_typed_manifest_alone_drops_unknown_top_level_keys() {
        let toml = "title = \"Book\"\nfuture_scalar = \"newer version wrote this\"\n";
        let manifest: Manifest = toml::from_str(toml).unwrap();
        let reserialized = toml::to_string_pretty(&manifest).unwrap();
        assert!(
            !reserialized.contains("future_scalar"),
            "the typed struct doesn't model unknown keys: {reserialized}"
        );
        assert!(
            reserialized.contains("title = \"Book\""),
            "known fields are preserved: {reserialized}"
        );
    }

    // SCHEMA VERSION + MIGRATION (#1097). A pre-#1097 file has no `format_version`,
    // so it *reads* as the current version — and migrate leaves it untouched, so an
    // existing project that only loads and re-saves is byte-stable (its `book.toml`
    // doesn't sprout a new key it never had). The version is the escape valve for a
    // future breaking bump, not a churn on every open.
    #[test]
    fn a_versionless_manifest_reads_as_current_and_is_left_untouched() {
        let mut manifest: Manifest = toml::from_str("title = \"Old Book\"").unwrap();
        assert_eq!(
            manifest.format_version(),
            CURRENT_FORMAT_VERSION,
            "an absent version reads as the current version"
        );
        manifest.migrate();
        assert_eq!(
            manifest.format_version, None,
            "a versionless file is already current — migrate doesn't restamp it"
        );
        let out = toml::to_string_pretty(&manifest).unwrap();
        assert!(
            !out.contains("format_version"),
            "an untouched existing project stays byte-stable on re-save: {out}"
        );
    }

    // A manifest carrying an explicit version round-trips it, and migrating a
    // current-version manifest is a no-op (idempotent) — the hook only does work
    // for a genuinely older version, which is what a future breaking bump adds.
    #[test]
    fn format_version_round_trips_and_migration_is_idempotent() {
        let toml = "format_version = 1\ntitle = \"Book\"\n";
        let mut manifest: Manifest = toml::from_str(toml).unwrap();
        assert_eq!(manifest.format_version, Some(1));
        manifest.migrate();
        assert_eq!(
            manifest.format_version,
            Some(CURRENT_FORMAT_VERSION),
            "migrating a current-version manifest is a no-op"
        );
        let out = toml::to_string_pretty(&manifest).unwrap();
        assert!(
            out.contains("format_version = 1"),
            "an explicit version round-trips: {out}"
        );
    }
}

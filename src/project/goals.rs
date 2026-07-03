//! The author's writing goals (#550): the `[goals]` table of `book.toml`, the
//! [`GoalField`] enum that names each editable goal, and the in-app setter. The
//! goals power the progress report; the setter splices a single key into the
//! standalone manifest (preserving the rest byte-for-byte) or writes through the
//! per-book serializer for a series. Extracted from `mod.rs` to keep that file
//! under the ~1,000-line convention (#562) — a pure move, no behavior change.

use serde::{Deserialize, Serialize};

use crate::atomic;
use crate::config;

use super::{read_existing, Project};

/// The author's writing goals (see [`docs/progress-reports.md`]). Pages are
/// always derived from words via `words_per_page`, never tracked directly.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Goals {
    /// Target word count. Defaults to 0 (unset) so a partial `[goals]` table —
    /// e.g. one the in-app setter spliced a single key into (#550), or a
    /// hand-edited manifest with only a chapter target — still parses; the report
    /// treats 0 as "no word goal".
    #[serde(default)]
    pub words: usize,
    /// Target chapter count. Defaults to 0 (unset) for the same reason as `words`.
    #[serde(default)]
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

const fn default_words_per_page() -> usize {
    250
}

/// The default words per page (250) — a standard manuscript page. Shared by the
/// serde default and the in-app goal editor (#550), so a project that sets only
/// words/chapters from the app still gets the standard page size.
pub const DEFAULT_WORDS_PER_PAGE: usize = 250;

/// Which project goal an in-app edit targets (#550). The progress report's three
/// motivating goals plus the page size; each maps to a key in the `[goals]`
/// table of `book.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoalField {
    /// Target word count (`words`) — required for the report.
    Words,
    /// Target chapter count (`chapters`) — required for the report.
    Chapters,
    /// Optional daily word-count target (`daily`); clearable.
    Daily,
    /// Words per page (`words_per_page`), defaulting to 250.
    WordsPerPage,
}

impl GoalField {
    /// The `[goals]` table key this field is stored under in `book.toml`.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Words => "words",
            Self::Chapters => "chapters",
            Self::Daily => "daily",
            Self::WordsPerPage => "words_per_page",
        }
    }

    /// A human label for prompts and status messages.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Words => "target words",
            Self::Chapters => "target chapters",
            Self::Daily => "daily word target",
            Self::WordsPerPage => "words per page",
        }
    }

    /// Whether this goal is optional — cleared (removed from `book.toml`) on blank
    /// input rather than rejected. Only the daily target is optional.
    pub const fn is_optional(self) -> bool {
        matches!(self, Self::Daily)
    }
}

impl Project {
    /// The author's writing goals, if set (the `[goals]` table of `book.toml`).
    pub const fn goals(&self) -> Option<&Goals> {
        self.manifest.goals.as_ref()
    }

    /// Set or clear one project goal (#550) and persist it to the `[goals]` table
    /// of `book.toml`. `value` is the goal's count, or `None` to clear an optional
    /// goal (removing the key, not writing 0). Mirrors the chapter word-goal flow:
    /// the in-memory manifest is updated so the progress report reflects it
    /// immediately, then the change is persisted.
    ///
    /// For a standalone novel the write is a **targeted splice** into the existing
    /// `[goals]` table, so the rest of the manifest — other sections, comments,
    /// formatting — is left byte-for-byte. A multi-book series (#328) keeps the
    /// per-book serializer path, which preserves each book's goals.
    pub fn set_goal(&mut self, field: GoalField, value: Option<usize>) -> std::io::Result<()> {
        // Update the in-memory manifest first, so the report reflects it with no
        // restart even if the (rare) save fails. A required goal needs a `Goals`
        // table to live in; default the siblings the way serde would.
        let goals = self.manifest.goals.get_or_insert(Goals {
            words: 0,
            chapters: 0,
            words_per_page: DEFAULT_WORDS_PER_PAGE,
            daily: None,
        });
        match field {
            GoalField::Words => goals.words = value.unwrap_or(0),
            GoalField::Chapters => goals.chapters = value.unwrap_or(0),
            GoalField::WordsPerPage => {
                goals.words_per_page = value.unwrap_or(DEFAULT_WORDS_PER_PAGE);
            }
            GoalField::Daily => goals.daily = value,
        }

        // A series writes through the per-book serializer (it has no single
        // top-level `[goals]` table to splice). The standalone case splices the
        // one key in place, preserving the rest of the manifest.
        if !self.books.is_empty() {
            return self.save_manifest();
        }
        let path = Self::manifest_path(&self.root);
        // A failed read must abort: splicing the one `[goals]` key onto an empty
        // base would overwrite the entire manifest (title, author, the whole
        // chapter list) while reporting success (#565).
        let existing = read_existing(&path)?;
        let updated = match value {
            Some(n) => config::set_value(&existing, "goals", field.key(), &n.to_string()),
            None => config::strip_value(&existing, "goals", field.key()).0,
        };
        atomic::write(&path, updated.as_bytes())
    }
}

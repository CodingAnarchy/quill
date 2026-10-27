//! A project's form (#1239): a novel (the default, chapters and parts) or a short
//! story — one piece of prose whose only structure is its scenes. Written to
//! `book.toml` as a top-level `form = "short-story"`; absent means a novel, so
//! every existing project is unchanged. Also holds the short-fiction length
//! categories the status bar shows, so the thresholds live in one place.
//!
//! A short story is still stored as exactly one body chapter (whose title mirrors
//! the project title), so the buffer, save, recovery, annotation, and word-count
//! paths need no special case — the form only changes what the author is shown.

use serde::{Deserialize, Serialize};

/// What kind of work a project holds. `Novel` is the default and is never
/// written to `book.toml`, so a novel's manifest is byte-for-byte unchanged.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Form {
    /// Chapters (optionally grouped into parts) — the historic project shape.
    #[default]
    Novel,
    /// One continuous piece of prose shorter than a novella, structured by scenes.
    ShortStory,
}

impl Form {
    /// Whether this is the default form, so serde can leave the key out.
    pub fn is_novel(&self) -> bool {
        *self == Self::Novel
    }
}

/// Word count at which a short work becomes a novelette (SFWA's category).
const NOVELETTE_WORDS: usize = 7_500;

/// Word count at which a work reaches novella length (SFWA's category) — past it,
/// a short story has outgrown the form (#1242 offers to grow it into a novel).
pub const NOVELLA_WORDS: usize = 17_500;

/// Below this, a piece is flash fiction.
const FLASH_WORDS: usize = 1_000;

/// The short-fiction length category for `words`, as the status bar names it.
pub const fn length_category(words: usize) -> &'static str {
    if words < FLASH_WORDS {
        "flash fiction"
    } else if words < NOVELETTE_WORDS {
        "short story"
    } else if words < NOVELLA_WORDS {
        "novelette"
    } else {
        "novella length"
    }
}

#[cfg(test)]
mod tests {
    use super::{length_category, Form};

    #[test]
    fn length_categories_follow_the_sfwa_thresholds() {
        assert_eq!(length_category(0), "flash fiction");
        assert_eq!(length_category(999), "flash fiction");
        assert_eq!(length_category(1_000), "short story");
        assert_eq!(length_category(7_499), "short story");
        assert_eq!(length_category(7_500), "novelette");
        assert_eq!(length_category(17_499), "novelette");
        assert_eq!(length_category(17_500), "novella length");
    }

    #[test]
    fn the_form_round_trips_as_a_kebab_case_string() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrap {
            form: Form,
        }
        let text = toml::to_string(&Wrap {
            form: Form::ShortStory,
        })
        .unwrap();
        assert_eq!(text.trim(), r#"form = "short-story""#);
        let back: Wrap = toml::from_str(&text).unwrap();
        assert_eq!(back.form, Form::ShortStory);
    }
}

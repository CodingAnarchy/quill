//! The compact story-bible seed (#856): [`BibleSeed`] — the global index the loop
//! opens with and the source [`READ_BIBLE`](crate::ai::retrieval::READ_BIBLE)
//! serves — the retained [`Excerpt`], and [`seed_message`], the opening context
//! (bible overview + chapter list + question, never the whole manuscript).

use std::fmt::Write as _;

use serde_json::{json, Value};

use super::search::ManuscriptSearch;

/// The compact story-bible seed + a bible-section reader (#856): the global index the
/// loop opens with, and the source [`READ_BIBLE`](crate::ai::retrieval::READ_BIBLE)
/// serves. Sections are named (`characters`, `setting`, …) with their rendered text;
/// a short seed lists the section names + a preview so the model knows what it can
/// read. Assembled on the UI thread (it reads the bible files) and moved into the
/// worker.
#[derive(Debug, Clone, Default)]
pub struct BibleSeed {
    /// `(section name, full text)`, in a stable order. Empty when no bible exists.
    sections: Vec<(String, String)>,
}

impl BibleSeed {
    /// Build a seed from named sections (drops empty ones).
    pub fn new(sections: Vec<(String, String)>) -> Self {
        Self {
            sections: sections
                .into_iter()
                .filter(|(_, text)| !text.trim().is_empty())
                .collect(),
        }
    }

    /// Whether any bible section exists — when false the loop is search-only and the
    /// answer discloses the weaker coverage (#856 acceptance criteria).
    pub const fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The opening index block: the section names and a short preview of each, so the
    /// model knows what global structure it can pull in via
    /// [`READ_BIBLE`](crate::ai::retrieval::READ_BIBLE). A brief note when no bible
    /// exists.
    pub(super) fn overview(&self) -> String {
        if self.sections.is_empty() {
            return "No story bible has been built yet — answer from manuscript search \
                    and chapter reads alone, and note that coverage may be partial."
                .to_string();
        }
        let mut out = String::from("Story bible sections available via read:bible:\n");
        for (name, text) in &self.sections {
            let preview: String = text.chars().take(200).collect();
            let _ = writeln!(out, "- {name}: {}", preview.replace('\n', " ").trim());
        }
        out
    }

    /// The full text of a named bible section, or `None` — the
    /// [`READ_BIBLE`](crate::ai::retrieval::READ_BIBLE) backend.
    pub(super) fn section(&self, name: &str) -> Option<&str> {
        let name = name.trim().to_lowercase();
        self.sections
            .iter()
            .find(|(n, _)| n.to_lowercase() == name)
            .map(|(_, text)| text.as_str())
    }
}

/// One accumulated tool result, kept so it can be compacted later (#856).
pub(super) struct Excerpt {
    /// The tool that produced it (for the compaction note).
    pub(super) tool: String,
    /// The lookup input.
    pub(super) argument: String,
    /// The raw result text (chapter prose, search excerpts, bible section).
    pub(super) body: String,
    /// The chapters this excerpt drew from — the citations it contributes, kept even
    /// after the raw body is compacted away.
    pub(super) citations: Vec<String>,
}

/// The opening seed message (#856): the bible overview as the global index and the
/// chapter list as the map, then the question. This is the whole starting context —
/// no manuscript prose, so it fits the window.
pub(super) fn seed_message<S: ManuscriptSearch>(
    bible: &BibleSeed,
    search: &S,
    question: &str,
) -> Value {
    let chapters = search
        .chapters()
        .iter()
        .map(|c| format!("- {}", c.title))
        .collect::<Vec<_>>()
        .join("\n");
    let seed = format!(
        "{}\n\nChapters (read with read:chapter, search with search:manuscript):\n{}\n\n\
         Question: {}",
        bible.overview(),
        chapters,
        question.trim(),
    );
    json!({ "role": "user", "content": [ { "type": "text", "text": seed } ] })
}

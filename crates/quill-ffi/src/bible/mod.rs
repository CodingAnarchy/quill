//! The story bible and the analysis findings, read-only on the phone (#1236).
//! Both are already in the project (`bible/`, kept current by the desktop's
//! passes and synced through git), so reading them costs no AI call. Nothing
//! here writes under `bible/`: dismissing or resolving a finding stays a
//! desktop action, so the phone's work never conflicts with a bible update.

use std::fs;
use std::path::{Component, Path};

use quill_core::ai::Finding;
use quill_core::buffer::Buffer;
use quill_core::findings_store::FindingsStore;
use quill_core::frontmatter;
use quill_core::text::match_ranges;

use crate::{QuillError, QuillProject};

const BIBLE_DIR: &str = "bible";

/// One page of the story bible.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BiblePage {
    /// The page's path inside `bible/` ("setting.md", "characters/mara.md").
    pub path: String,
    pub title: String,
    /// "Story" for the top-level pages, "Characters" for the cast.
    pub section: String,
}

/// One open finding from the desktop's last analysis passes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FindingInfo {
    pub summary: String,
    /// The pass that raised it ("Continuity", "Prose & style").
    pub kind: String,
    /// Every chapter it names; the first is the one to open.
    pub chapters: Vec<String>,
    /// The first named chapter's index in the active book, when it's there.
    pub chapter_index: Option<u32>,
    /// "flagged by continuity + plot" when several passes agree.
    pub agreement: Option<String>,
}

/// A range of a chapter's prose, in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TextSpan {
    pub start: u32,
    pub length: u32,
}

#[uniffi::export]
impl QuillProject {
    /// The bible's pages: the story pages first, then the characters, each in
    /// name order.
    pub fn bible_pages(&self) -> Result<Vec<BiblePage>, QuillError> {
        let root = self.lock().get()?.root.join(BIBLE_DIR);
        let mut pages = pages_in(&root, "", "Story");
        pages.extend(pages_in(&root, "characters/", "Characters"));
        Ok(pages)
    }

    /// A bible page's text, without its front matter. `path` must name a page
    /// inside `bible/`.
    pub fn bible_page(&self, path: &str) -> Result<String, QuillError> {
        let rel = Path::new(path);
        let inside = rel.components().all(|c| matches!(c, Component::Normal(_)));
        if !inside || rel.extension().is_none_or(|e| e != "md") {
            return Err(QuillError::NotFound {
                message: format!("There's no bible page called {path}."),
            });
        }
        let file = self.lock().get()?.root.join(BIBLE_DIR).join(rel);
        let text = fs::read_to_string(file)?;
        Ok(frontmatter::split(&text).1.to_string())
    }

    /// The open findings for the active book, in manuscript order (findings
    /// naming no chapter of this book come last).
    pub fn findings(&self) -> Result<Vec<FindingInfo>, QuillError> {
        let guard = self.lock();
        let p = guard.get()?;
        let active = p.active_book();
        let series = p.book_count() > 1;
        let mut out: Vec<FindingInfo> = FindingsStore::load(&p.root)
            .open()
            .into_iter()
            .filter(|f| !series || f.book.unwrap_or(active) == active)
            .map(|f| finding_info(&f, |title| chapter_named(p, title)))
            .collect();
        out.sort_by_key(|f| f.chapter_index.unwrap_or(u32::MAX));
        Ok(out)
    }

    /// Where in chapter `index` a finding points: the first phrase it quotes
    /// that the chapter contains, as a UTF-16 range. `None` when it quotes
    /// nothing that's there; the chapter then opens at the top.
    pub fn finding_passage(
        &self,
        index: u32,
        summary: &str,
    ) -> Result<Option<TextSpan>, QuillError> {
        let body = self.chapter_text(index)?;
        let found = quoted_phrases(summary)
            .into_iter()
            .find_map(|phrase| match_ranges(&body, &phrase).into_iter().next());
        Ok(found.map(|(start, end)| {
            let b = Buffer::from_text(&body);
            let (s, e) = (b.utf16_of_byte(start), b.utf16_of_byte(end));
            TextSpan {
                start: u32::try_from(s).unwrap_or(u32::MAX),
                length: u32::try_from(e - s).unwrap_or(u32::MAX),
            }
        }))
    }
}

/// The `.md` pages directly in `bible/<sub>`, titled from their `name:` front
/// matter or their file name.
fn pages_in(bible: &Path, sub: &str, section: &str) -> Vec<BiblePage> {
    let Ok(entries) = fs::read_dir(bible.join(sub)) else {
        return Vec::new();
    };
    let mut pages: Vec<BiblePage> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".md")?.to_string();
            if stem.starts_with('.') {
                return None;
            }
            let text = fs::read_to_string(e.path()).unwrap_or_default();
            let title = quill_core::ai::bible::front_field(&text, "name")
                .map(|n| n.trim_matches(['"', '\'']).to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| title_case(&stem));
            Some(BiblePage {
                path: format!("{sub}{name}"),
                title,
                section: section.to_string(),
            })
        })
        .collect();
    pages.sort_by_key(|p| p.title.to_lowercase());
    pages
}

/// "research-notes" is "Research notes".
fn title_case(stem: &str) -> String {
    let words = stem.replace(['-', '_'], " ");
    let mut chars = words.chars();
    chars
        .next()
        .map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// The active book's chapter titled `title`: exactly, or else ignoring case,
/// the way an answer's citations resolve (#1145).
fn chapter_named(p: &quill_core::project::Project, title: &str) -> Option<usize> {
    let titles = || (0..p.manifest.chapters.len()).filter_map(|i| Some((i, p.chapter_title(i)?)));
    titles()
        .find(|(_, t)| *t == title)
        .or_else(|| titles().find(|(_, t)| t.eq_ignore_ascii_case(title.trim())))
        .map(|(i, _)| i)
}

fn finding_info(f: &Finding, chapter: impl Fn(&str) -> Option<usize>) -> FindingInfo {
    FindingInfo {
        summary: f.summary.clone(),
        kind: f.kind.label().to_string(),
        chapters: f.chapters.clone(),
        chapter_index: f
            .chapters
            .first()
            .and_then(|t| chapter(t))
            .and_then(|i| u32::try_from(i).ok()),
        agreement: f.agreement_signal(),
    }
}

/// The phrases of two or more words a summary quotes ("…", “…”).
fn quoted_phrases(summary: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    for (open, close) in [('"', '"'), ('\u{201c}', '\u{201d}')] {
        let mut rest = summary;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len_utf8()..];
            let Some(end) = after.find(close) else { break };
            let phrase = after[..end].trim();
            if phrase.split_whitespace().count() >= 2 {
                phrases.push(phrase.to_string());
            }
            rest = &after[end + close.len_utf8()..];
        }
    }
    phrases
}

#[cfg(test)]
mod tests;

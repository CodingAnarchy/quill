//! Import-preview mode state (#702/#886): the manuscript-import split preview and
//! the review-comment import preview, each capturing everything the confirm needs
//! so it never re-reads the source. Split out of `states.rs` (pure move).

use std::path::PathBuf;

/// State for the manuscript-import preview (#702): the split the parser produced,
/// shown to the author before anything is written so a mis-split (scene breaks
/// read as chapters, or one giant chapter) is caught up front rather than after
/// the project exists. Enter confirms and proceeds to the write+open; Esc cancels
/// and writes nothing.
///
/// Everything needed to write is captured here when the preview opens, so the
/// confirm never re-reads the source. For a Markdown / plain-text source the split
/// heading level can be toggled (`levels` lists the levels the manuscript uses)
/// and the preview re-parsed at the chosen level; other formats (`.docx`,
/// `.scriv`, a folder of chapters) don't split on a Markdown level, so `levels` is
/// empty and only confirm/cancel apply.
pub struct ImportPreview {
    /// The source manuscript path (read-only; never moved or altered).
    pub(crate) source: PathBuf,
    /// Where the new project will be created on confirm.
    pub(crate) dest: PathBuf,
    /// The Markdown source text, kept so a level toggle can re-split without
    /// re-reading; `None` for formats that don't re-split (`.docx`/`.scriv`/folder).
    pub(crate) markdown: Option<String>,
    /// The book title mined from the source, if any.
    pub(crate) title: Option<String>,
    /// The book author mined from the source, if any.
    pub(crate) author: Option<String>,
    /// The chapters the current split produced (title + body), in reading order.
    pub(crate) chapters: Vec<(String, String)>,
    /// The heading level the current Markdown split broke on (`None` for the
    /// non-Markdown formats, which don't split on a level).
    pub(crate) level: Option<usize>,
    /// The heading levels the Markdown source actually uses, offered for re-split
    /// (empty for the non-Markdown formats).
    pub(crate) levels: Vec<usize>,
    /// The dropped-element / unreadable-body warning suffix (#511/#646), already
    /// formatted (empty when the parse was clean).
    pub(crate) warning: String,
    /// The first visible chapter row, so a long chapter list scrolls.
    pub(crate) scroll: usize,
}

/// State for the review-comment import preview (#886): the reviewer's Word
/// comments recovered from a marked-up `.docx`, resolved against the active
/// chapter's prose. The matched comments will become live annotations; the
/// unmatched ones (whose commented phrase no longer appears — the prose drifted)
/// are reported, not silently dropped. Enter merges the matches into the chapter's
/// annotations; Esc cancels and writes nothing.
///
/// Everything needed to create the annotations is captured here when the preview
/// opens (the resolved byte spans against the chapter buffer), so the merge never
/// re-reads the source or re-searches the prose.
pub struct ReviewImport {
    /// The source `.docx` path (read-only; never moved or altered).
    pub(crate) source: PathBuf,
    /// The chapter file the comments will be anchored into — the active chapter.
    pub(crate) chapter: String,
    /// The matched comments: `(start, end, note, reviewer)` buffer byte spans
    /// against the active chapter, each becoming one annotation on merge. The
    /// reviewer (#933) is the named editor the comment came from, `None` when the
    /// source didn't attribute it.
    pub(crate) matched: Vec<(usize, usize, String, Option<String>)>,
    /// The unmatched comments: `(phrase, note)` whose commented phrase no longer
    /// appears in the prose — reported so the author knows what didn't carry.
    pub(crate) dropped: Vec<(String, String)>,
    /// The first visible row, so a long list scrolls.
    pub(crate) scroll: usize,
}

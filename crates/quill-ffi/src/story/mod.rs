//! Short stories on the phone (#1247). A story is one piece of prose whose
//! structure is its scenes (#1240): split by scene breaks or `##` headings,
//! each with an optional synopsis and status in the chapter's front matter.
//! The phone gets the scene list from the core, never re-deriving where a
//! scene starts in Swift, so the two devices can't disagree about it.

use quill_core::frontmatter::FrontMatter;
use quill_core::project::length_category;
use quill_core::scenes::{reconcile_scene_meta, scene_label, story_scenes};

use crate::ChapterSession;

/// One scene of the story being edited.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SceneInfo {
    pub index: u32,
    /// The scene's heading, or its first line when it has none.
    pub title: String,
    pub synopsis: Option<String>,
    /// "Stub", "Draft", or "Done".
    pub status: String,
    /// Where the scene starts in the prose, in UTF-16 code units.
    pub start: u32,
}

/// The short-fiction length category for `words` ("flash fiction", "short
/// story", "novelette", "novella length"), as the desktop status bar names it.
#[uniffi::export]
pub fn story_length(words: u64) -> String {
    length_category(usize::try_from(words).unwrap_or(usize::MAX)).to_string()
}

#[uniffi::export]
impl ChapterSession {
    /// The story's scenes as the prose stands now, with their synopses and
    /// statuses matched to them by anchor, as the desktop does.
    pub fn scenes(&self) -> Vec<SceneInfo> {
        self.with_buffer_mut(|b| {
            b.refresh_syntax();
            let text = b.text();
            let syntax = b.syntax();
            let scenes = story_scenes(&text, syntax.headings(), syntax.scene_break_lines());
            let labels: Vec<String> = scenes.iter().map(|s| scene_label(&text, s)).collect();
            let stored = b
                .front_matter()
                .map(FrontMatter::scenes)
                .unwrap_or_default();
            let meta = reconcile_scene_meta(&stored, &labels);
            scenes
                .iter()
                .zip(labels)
                .zip(meta)
                .enumerate()
                .map(|(i, ((scene, title), meta))| SceneInfo {
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    title,
                    synopsis: Some(meta.synopsis).filter(|s| !s.trim().is_empty()),
                    status: meta.status.label().to_string(),
                    start: u32::try_from(b.utf16_of_byte(scene.start)).unwrap_or(u32::MAX),
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests;

//! A short story's scenes in the editor (#1240). A story's structure is its
//! scenes, split by scene breaks or `##` headings ([`crate::scenes`]), so the
//! outline pane, the focused outline, quick switch, and the corkboard list scenes
//! for a story where they list headings for a novel. The scenes are presented to
//! those views as [`Heading`]-shaped entries (a label and a jump offset), so their
//! list, filter, and jump code is shared with the novel path.

use std::borrow::Cow;

use super::App;
use crate::frontmatter::SceneMeta;
use crate::scenes::{reconcile_scene_meta, scene_label, story_scenes, swap_scenes, StoryScene};
use crate::syntax::{Heading, Syntax};

impl App {
    /// Whether the focused buffer is a short story's prose (#1240): the story's
    /// one chapter, in the single-chapter view, with no reference pane focused.
    pub(crate) fn editing_story(&self) -> bool {
        self.manuscript.is_none()
            && self.focus_shows_active_chapter()
            && self.project.manifest.story_chapter() == Some(self.project.active_index())
    }

    /// The entries the outline lists (#1240): a story's scenes when editing one,
    /// else the active chapter's headings, borrowed as they are.
    pub(crate) fn outline_entries(&self) -> Cow<'_, [Heading]> {
        if self.editing_story() {
            let text = self.buffer.text();
            Cow::Owned(scene_entries(&text, self.buffer.syntax()))
        } else {
            Cow::Borrowed(self.buffer.syntax().headings())
        }
    }

    /// The focused story's scenes and the text they index (#1240).
    pub(super) fn story_scenes_now(&self) -> (String, Vec<StoryScene>) {
        let buffer = self.edit_buffer();
        let text = buffer.text();
        let syntax = buffer.syntax();
        let scenes = story_scenes(&text, syntax.headings(), syntax.scene_break_lines());
        (text, scenes)
    }

    /// Move story scene `index` one slot in `dir` (−1 up, +1 down) (#1240): the
    /// scene's prose (and its break) swap with its neighbour's, its `scenes:`
    /// metadata moving with it, in one rewrite that's saved at once. Returns the
    /// scene's new index, or `None` at the ends (nothing moves).
    pub(super) fn move_story_scene(&mut self, index: usize, dir: isize) -> Option<usize> {
        let (text, scenes) = self.story_scenes_now();
        let other = index
            .checked_add_signed(dir)
            .filter(|&o| o < scenes.len())?;
        if index >= scenes.len() {
            return None;
        }
        let body = swap_scenes(&text, &scenes, index, other);
        let mut meta = self.chapter_scenes(scenes.len());
        meta.swap(index, other);
        self.rewrite_chapter(&body, &meta);
        Some(other)
    }

    /// The story's stored scene metadata matched to its current scenes by anchor
    /// (#1240) — see [`reconcile_scene_meta`].
    pub(super) fn reconciled_story_meta(&self, stored: &[SceneMeta]) -> Vec<SceneMeta> {
        let (text, scenes) = self.story_scenes_now();
        let labels: Vec<String> = scenes.iter().map(|s| scene_label(&text, s)).collect();
        reconcile_scene_meta(stored, &labels)
    }
}

/// A story body's scenes as outline entries (#1240): one per scene, labelled by
/// its heading or first line, at the scene's start offset. Every entry is level 2
/// so the outline indents them evenly.
pub(super) fn scene_entries(text: &str, syntax: &Syntax) -> Vec<Heading> {
    story_scenes(text, syntax.headings(), syntax.scene_break_lines())
        .iter()
        .map(|s| Heading {
            level: 2,
            title: scene_label(text, s),
            start: s.start,
        })
        .collect()
}

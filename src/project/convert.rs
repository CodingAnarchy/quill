//! Converting a project between forms (#1243): a novel with a single body
//! chapter becomes a short story. The prose is untouched — a short story is
//! already stored as one chapter — so the conversion is a manifest change: the
//! form, no parts, and the chapter titled after the story.

use std::io;

use super::{Form, Project};

impl Project {
    /// Why this project can't become a short story, or `None` when it can: a
    /// standalone novel with exactly one body chapter and no parts. Front and back
    /// matter (a dedication, an afterword) don't count against it; they stay
    /// around the story.
    pub fn short_story_blocker(&self) -> Option<String> {
        if self.manifest.is_short_story() {
            return Some("it's already a short story".into());
        }
        if self.book_count() > 1 {
            return Some("a series can't become one short story".into());
        }
        let body = self.manifest.body_chapter_count();
        if body != 1 {
            return Some(format!(
                "a short story is one chapter, and this book has {body} — merge them first"
            ));
        }
        if self.manifest.parts.as_ref().is_some_and(|p| !p.is_empty()) {
            return Some(format!(
                "a short story has no {}s — remove them first",
                self.manifest.part_label()
            ));
        }
        None
    }

    /// Turn this one-chapter novel into a short story (#1243) and save the
    /// manifest. Errors (changing nothing) when [`short_story_blocker`] names a
    /// reason. The chapter keeps its file and prose; its title becomes the story
    /// title.
    ///
    /// [`short_story_blocker`]: Self::short_story_blocker
    pub fn convert_to_short_story(&mut self) -> io::Result<()> {
        if let Some(why) = self.short_story_blocker() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
        }
        let before = self.manifest.clone();
        self.manifest.form = Form::ShortStory;
        self.manifest.parts = None;
        self.manifest.sync_story_title();
        self.save_manifest().inspect_err(|_| {
            // Leave the in-memory project as it still is on disk.
            self.manifest = before;
        })
    }
}

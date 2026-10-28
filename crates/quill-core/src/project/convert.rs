//! Converting a project between forms. A novel with a single body chapter
//! becomes a short story (#1243): the prose is untouched — a short story is
//! already stored as one chapter — so that's a manifest change: the form, no
//! parts, and the chapter titled after the story. A short story grows into a
//! novel (#1242) by splitting its one chapter into one chapter per scene, carrying
//! each scene's notes and the annotations anchored in it to the new chapter.

use std::collections::HashSet;
use std::io;

use crate::annotation::Live;
use crate::atomic;
use crate::frontmatter::{self, FrontMatter, Status};

use super::{ChapterEntry, Form, Project};

/// One chapter a growing story becomes (#1242): its title and prose, the scene
/// notes it inherits, and where its prose began in the story body, so the
/// annotations anchored there can be moved onto it.
pub struct GrownChapter {
    pub title: String,
    pub body: String,
    pub summary: String,
    pub status: Option<Status>,
    /// Byte offset in the story body where this chapter's text begins.
    pub from: usize,
}

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

impl Project {
    /// Grow this short story into a novel (#1242): its one chapter becomes
    /// `chapters`, in order, and the form becomes a novel. The first chapter keeps
    /// the story's file (and its other front matter, such as a POV); the rest get
    /// new `chapter-N.md` files. Each annotation in the story moves to the chapter
    /// its start falls in, re-anchored to that chapter's text. Errors before
    /// changing the manifest when a file can't be written; the caller records a
    /// restore point first.
    pub fn grow_into_novel(&mut self, chapters: &[GrownChapter]) -> io::Result<()> {
        let Some(story) = self.manifest.story_chapter() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "this project isn't a short story",
            ));
        };
        if chapters.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the story has no scenes to turn into chapters",
            ));
        }
        let story_file = self.manifest.chapters[story].file.clone();
        let story_front = self.try_split_chapter(story)?.0.unwrap_or_default();

        // Name every chapter's file: the story's own for the first, then fresh
        // `chapter-N.md` names that collide with nothing already in the project.
        let mut taken: HashSet<String> = self
            .manifest
            .chapters
            .iter()
            .map(|c| c.file.clone())
            .collect();
        let mut files = vec![story_file.clone()];
        let mut n = 2;
        while files.len() < chapters.len() {
            let name = format!("chapter-{n}.md");
            n += 1;
            if taken.contains(&name) || self.root.join(&name).exists() {
                continue;
            }
            taken.insert(name.clone());
            files.push(name);
        }

        // Write the new chapters first and the story's own file last, so a failure
        // part-way leaves the story's text where it was.
        for (i, (chapter, file)) in chapters.iter().zip(&files).enumerate().rev() {
            let mut front = if i == 0 {
                story_front.clone()
            } else {
                FrontMatter::default()
            };
            front.set_scenes(&[]);
            if !chapter.summary.is_empty() {
                front.set_summary(&chapter.summary);
            }
            if let Some(status) = chapter.status {
                front.set_status(status);
            }
            let block = (!front.is_empty()).then(|| front.as_str());
            atomic::write(
                &self.root.join(file),
                frontmatter::join(block, &chapter.body).as_bytes(),
            )?;
        }

        // Move each of the story's annotations onto the chapter it falls in.
        let mut moved: Vec<Vec<Live>> = vec![Vec::new(); chapters.len()];
        for live in self.annotations_for(&story_file) {
            let i = chapters
                .iter()
                .rposition(|c| c.from <= live.start)
                .unwrap_or(0);
            let len = chapters[i].body.len();
            let start = live.start.saturating_sub(chapters[i].from).min(len);
            let end = live.end.saturating_sub(chapters[i].from).clamp(start, len);
            moved[i].push(Live {
                chapter: files[i].clone(),
                start,
                end,
                ..live
            });
        }
        self.set_annotations(
            files
                .iter()
                .map(String::as_str)
                .zip(moved.iter().map(Vec::as_slice)),
        )?;

        // Swap the story's entry for the chapters, then save as a novel.
        let entries = chapters
            .iter()
            .zip(&files)
            .map(|(c, f)| ChapterEntry::new(c.title.clone(), f.clone()));
        self.manifest.chapters.splice(story..=story, entries);
        self.manifest.form = Form::Novel;
        for i in story..story + chapters.len() {
            self.read_chapter_cache(i);
        }
        self.active.clone_from(&files[0]);
        self.save_manifest()
    }
}

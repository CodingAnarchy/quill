//! A collection of short stories (#1244): a project whose `book.toml` carries
//! `form = "collection"` and *links* existing short story projects by path rather
//! than copying them, so each story keeps one source of truth — an edit made in
//! the story shows up in the collection, and the reverse. The collection itself
//! holds only what belongs to it: the stories' order, a per-story credit line
//! ("First published in …"), and its own front and back matter (an introduction,
//! acknowledgements), which are its ordinary chapters.
//!
//! Linked stories are read once at open — each one's manifest and story file,
//! without opening (or locking) the story project — and cached on its link, so the
//! story list and its word totals never touch the disk per frame.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::buffer::count_words;
use crate::frontmatter::{self, Epigraph, FrontMatter, Status};

use super::{sanitize_dir_name, Manifest, Project, ProjectTemplate, MANIFEST_NAME};

/// Where a collection's newly written stories live, inside the collection.
pub const STORIES_DIR: &str = "stories";

/// One story linked into a collection: the story project's folder, relative to
/// the collection (so moving both together keeps the link), and the credit line
/// the collection prints under its title. The cached `info` is read at open.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct StoryLink {
    pub path: String,
    /// A collection-only note under the story's title, such as "First published
    /// in *Lightspeed*, 2027". The story project itself is never changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<String>,
    /// What the story project holds, cached at open; `None` when its folder is
    /// missing or isn't a Quill project.
    #[serde(skip)]
    pub info: Option<StoryInfo>,
}

/// A linked story's cached details: its title, the words in its story, and the
/// story chapter's revision status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoryInfo {
    pub title: String,
    pub words: usize,
    pub status: Option<Status>,
}

/// One linked story's text for a collection export (#1245): its title, its own
/// epigraph, the collection's credit line for it, and its prose (front matter
/// stripped), read from the story project at export time.
pub struct StoryText {
    pub title: String,
    pub epigraph: Option<Epigraph>,
    pub credit: Option<String>,
    pub body: String,
}

impl StoryLink {
    /// A link to the story at `path` (relative to the collection).
    pub const fn new(path: String) -> Self {
        Self {
            path,
            credit: None,
            info: None,
        }
    }
}

impl Project {
    /// Whether this project is a collection of short stories (#1244).
    pub fn is_collection(&self) -> bool {
        self.manifest.form == super::Form::Collection
    }

    /// The absolute folder of linked story `index`.
    pub fn story_dir(&self, index: usize) -> Option<PathBuf> {
        let link = self.manifest.stories.get(index)?;
        Some(self.root.join(&link.path))
    }

    /// Re-read every linked story's cached details (#1244) — at open, and after
    /// the author comes back from editing a story.
    pub fn refresh_stories(&mut self) {
        let root = self.root.clone();
        for link in &mut self.manifest.stories {
            link.info = peek_story(&root.join(&link.path)).ok();
        }
    }

    /// The words across every linked story that could be read.
    pub fn collection_words(&self) -> usize {
        self.manifest
            .stories
            .iter()
            .filter_map(|s| s.info.as_ref())
            .map(|i| i.words)
            .sum()
    }

    /// Link the short story project at `dir` into this collection (#1244),
    /// appended last. Errors when `dir` isn't a short story or is already linked.
    pub fn add_story(&mut self, dir: &Path) -> io::Result<usize> {
        let dir = absolute(&self.root, dir);
        let info = peek_story(&dir)?;
        if self.find_story(&dir).is_some() {
            return Err(invalid(format!(
                "“{}” is already in this collection",
                info.title
            )));
        }
        let mut link = StoryLink::new(relative_path(&self.root, &dir));
        link.info = Some(info);
        self.manifest.stories.push(link);
        self.save_manifest()?;
        Ok(self.manifest.stories.len() - 1)
    }

    /// Write a new short story inside the collection (#1244): a story project
    /// in `stories/<title>`, linked last. Returns its folder, so the caller can
    /// open it.
    pub fn create_story(&mut self, title: &str) -> io::Result<PathBuf> {
        let name = sanitize_dir_name(title);
        if name.is_empty() {
            return Err(invalid("a story needs a title".into()));
        }
        let dir = self.root.join(STORIES_DIR).join(&name);
        if dir.join(MANIFEST_NAME).exists() {
            return Err(invalid(format!(
                "a story called “{name}” already exists here"
            )));
        }
        {
            let mut story = Self::create_with_template(&dir, ProjectTemplate::short_story())?;
            story.manifest.title = title.trim().to_string();
            story.manifest.author.clone_from(&self.manifest.author);
            story.manifest.sync_story_title();
            story.save_manifest()?;
        } // dropping the story releases its lock before the author opens it
        self.add_story(&dir)?;
        Ok(dir)
    }

    /// Move linked story `index` one place earlier (`up`) or later. Returns its
    /// new index, or `None` at either end.
    pub fn move_story(&mut self, index: usize, up: bool) -> io::Result<Option<usize>> {
        let len = self.manifest.stories.len();
        let other = if up {
            index.checked_sub(1)
        } else {
            (index + 1 < len).then_some(index + 1)
        };
        let Some(other) = other.filter(|_| index < len) else {
            return Ok(None);
        };
        self.manifest.stories.swap(index, other);
        self.save_manifest()?;
        Ok(Some(other))
    }

    /// Unlink story `index` from the collection. The story project is never
    /// touched — only the link goes.
    pub fn remove_story(&mut self, index: usize) -> io::Result<()> {
        if index < self.manifest.stories.len() {
            self.manifest.stories.remove(index);
            self.save_manifest()?;
        }
        Ok(())
    }

    /// Set (or, with blank text, clear) story `index`'s credit line.
    pub fn set_story_credit(&mut self, index: usize, credit: &str) -> io::Result<()> {
        if let Some(link) = self.manifest.stories.get_mut(index) {
            let credit = credit.trim();
            link.credit = (!credit.is_empty()).then(|| credit.to_string());
            self.save_manifest()?;
        }
        Ok(())
    }

    /// Point story `index` at its new folder `dir` after it moved (#1244). Errors,
    /// keeping the old link, when `dir` isn't a short story project.
    pub fn relocate_story(&mut self, index: usize, dir: &Path) -> io::Result<()> {
        let dir = absolute(&self.root, dir);
        let info = peek_story(&dir)?;
        if let Some(link) = self.manifest.stories.get_mut(index) {
            link.path = relative_path(&self.root, &dir);
            link.info = Some(info);
            self.save_manifest()?;
        }
        Ok(())
    }

    /// Linked story `index`'s title, epigraph, credit, and prose, read from its
    /// project now so an export always has the latest text (#1245). Errors when
    /// the story can't be read (its folder is missing, say).
    pub fn story_text(&self, index: usize) -> io::Result<StoryText> {
        let link = self
            .manifest
            .stories
            .get(index)
            .ok_or_else(|| invalid("no such story".into()))?;
        let dir = self.root.join(&link.path);
        let (manifest, file) = story_manifest(&dir)?;
        let text = fs::read_to_string(dir.join(file))?;
        let (front, body) = frontmatter::split(&text);
        Ok(StoryText {
            title: manifest.title,
            epigraph: front.map(FrontMatter::from_raw).and_then(|f| f.epigraph()),
            credit: link.credit.clone(),
            body: body.to_string(),
        })
    }

    /// The index of the link already pointing at `dir`, if any.
    fn find_story(&self, dir: &Path) -> Option<usize> {
        let want = fs::canonicalize(dir).ok()?;
        self.manifest
            .stories
            .iter()
            .position(|s| fs::canonicalize(self.root.join(&s.path)).is_ok_and(|p| p == want))
    }
}

/// Read a short story project's details without opening (or locking) it: its
/// title, its story's word count, and its status. Errors when `dir` holds no
/// Quill project, or one that isn't a short story.
pub fn peek_story(dir: &Path) -> io::Result<StoryInfo> {
    let (manifest, file) = story_manifest(dir)?;
    let text = fs::read_to_string(dir.join(&file)).or_else(|e| match e.kind() {
        io::ErrorKind::NotFound => Ok(String::new()),
        _ => Err(e),
    })?;
    let (front, body) = frontmatter::split(&text);
    Ok(StoryInfo {
        title: manifest.title,
        words: count_words(body),
        status: front.map(FrontMatter::from_raw).and_then(|f| f.status()),
    })
}

/// A short story project's manifest and its story chapter's file.
fn story_manifest(dir: &Path) -> io::Result<(Manifest, String)> {
    let path = dir.join(MANIFEST_NAME);
    let text = fs::read_to_string(&path)
        .map_err(|e| io::Error::new(e.kind(), format!("no Quill project at {}", dir.display())))?;
    let value: toml::Value =
        toml::from_str(&text).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    let (mut manifest, _, _) = Manifest::from_toml_value(value)
        .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    // Roles live in each chapter's front matter; read them so the story chapter
    // is found among any dedication or afterword.
    for chapter in &mut manifest.chapters {
        let text = fs::read_to_string(dir.join(&chapter.file)).unwrap_or_default();
        let front = frontmatter::split(&text).0.map(FrontMatter::from_raw);
        chapter.role = front.as_ref().and_then(FrontMatter::role);
    }
    let Some(index) = manifest.story_chapter() else {
        return Err(invalid(format!("“{}” isn't a short story", manifest.title)));
    };
    let file = manifest.chapters[index].file.clone();
    Ok((manifest, file))
}

/// `path` made absolute against `base` when it's relative.
fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// `to` relative to the folder `from`, with `/` separators (`../other-story`),
/// so the link survives moving both folders together. Falls back to the
/// absolute path when the two share no root (another drive).
pub fn relative_path(from: &Path, to: &Path) -> String {
    let from = resolve(from);
    let to = resolve(to);
    let a: Vec<Component> = from.components().collect();
    let b: Vec<Component> = to.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return to.to_string_lossy().into_owned();
    }
    let mut parts: Vec<String> = vec!["..".into(); a.len() - common];
    parts.extend(
        b[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

/// `path` with symlinks resolved through its deepest existing ancestor, so a
/// folder not created yet compares like one that exists (`/var` and
/// `/private/var` are one place on macOS).
fn resolve(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut base = path;
    loop {
        if let Ok(real) = fs::canonicalize(base) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                base = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

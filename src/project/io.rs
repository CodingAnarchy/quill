//! Chapter-file and manifest I/O for a project: reading a chapter's text and
//! front matter from disk, the splice-then-write helpers that edit a chapter's
//! front matter or body in place, importing a manuscript into per-chapter files,
//! caching per-chapter metadata at open, and writing `book.toml` back. Every
//! write that derives its content from existing bytes reads through
//! [`read_existing`] first, so a failed read aborts rather than blanking the file
//! (#565). Extracted from `mod.rs` to keep that file under the ~1,000-line
//! convention (#562) — a pure move, no behavior change.

use std::fs;
use std::path::Path;

use crate::atomic;
use crate::buffer::count_words;
use crate::frontmatter::{self, FrontMatter};

use super::{series, ChapterEntry, Project};

/// Read a file that is expected to exist, ahead of a write derived from its
/// contents (#565). A missing file is the legitimate "starts empty" case and
/// yields `""`; any other read error (permissions, an I/O fault, the path being
/// a directory) is propagated so the caller aborts rather than splicing its new
/// content onto an empty base — which would durably destroy the real data while
/// reporting success (the shape of #511). "File is empty" (`Ok("")`) stays
/// distinct from "read failed" (`Err`).
///
/// Shared with the other splice-then-write manifest/chapter paths outside this
/// module (the font `[style]` writer, the AI marker inserter).
pub(crate) fn read_existing(path: &Path) -> std::io::Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Read `entry`'s file under `root` and refresh its cached word count and
/// front-matter metadata (#331). Shared by the active book's caching and the
/// other books of a series, so every book's counts/roles are populated at open
/// without re-reading later. One file read; never holds the body text.
pub(crate) fn cache_chapter(root: &Path, entry: &mut ChapterEntry) {
    let text = fs::read_to_string(root.join(&entry.file)).unwrap_or_default();
    let (front, body) = frontmatter::split(&text);
    entry.words = count_words(body);
    entry.cache_meta(front.map(FrontMatter::from_raw).as_ref());
}

impl Project {
    /// Replace this project's chapters with an imported manuscript (#298): write
    /// each chapter body to its own file, rebuild the manifest in reading order,
    /// and set the book title / author when the source supplied them. Existing
    /// chapter files (e.g. the starter from [`create`](Self::create)) are removed
    /// first. Writes go through the atomic-save path; the source is never touched.
    pub fn import_chapters(
        &mut self,
        title: Option<&str>,
        author: Option<&str>,
        chapters: &[(String, String)],
    ) -> std::io::Result<()> {
        for c in &self.manifest.chapters {
            let _ = fs::remove_file(self.root.join(&c.file)); // best effort
        }
        self.manifest.chapters.clear();
        for (i, (ctitle, body)) in chapters.iter().enumerate() {
            let file = format!("chapter-{}.md", i + 1);
            atomic::write(&self.root.join(&file), body.as_bytes())?;
            self.manifest
                .chapters
                .push(ChapterEntry::new(ctitle.clone(), file));
        }
        if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
            self.manifest.title = t.to_string();
        }
        if let Some(a) = author.map(str::trim).filter(|a| !a.is_empty()) {
            self.manifest.author = Some(a.to_string());
        }
        self.active = self
            .manifest
            .chapters
            .first()
            .map(|c| c.file.clone())
            .unwrap_or_default();
        self.save_manifest()
    }

    /// Read chapter `index` from disk once and refresh its cached word count and
    /// front-matter metadata (status / summary / POV). Used at open.
    pub(super) fn read_chapter_cache(&mut self, index: usize) {
        if index < self.manifest.chapters.len() {
            cache_chapter(&self.root, &mut self.manifest.chapters[index]);
        }
    }

    /// Read chapter `index`'s file from disk, or "" if it can't be read. The
    /// shared front of the off-the-per-frame-path chapter readers.
    fn read_chapter(&self, index: usize) -> String {
        fs::read_to_string(self.chapter_path(index)).unwrap_or_default()
    }

    /// The front matter of chapter `index` as written on disk (`None` if the
    /// file has no block). For reading a chapter's metadata that isn't cached —
    /// keep it off the per-frame path.
    pub fn chapter_front_matter(&self, index: usize) -> Option<FrontMatter> {
        let text = self.read_chapter(index);
        let (front, _) = frontmatter::split(&text);
        front.map(FrontMatter::from_raw)
    }

    /// The prose body of chapter `index` from disk (front matter stripped),
    /// empty if it can't be read. Off the per-frame path (used by the report).
    pub fn chapter_body(&self, index: usize) -> String {
        frontmatter::split(&self.read_chapter(index)).1.to_string()
    }

    /// Edit a (non-active) chapter's front matter on disk: read the file, apply
    /// `f` to its [`FrontMatter`], write it back leaving the prose body untouched,
    /// and refresh the cache. The body is preserved verbatim — only the front
    /// matter region changes.
    pub fn edit_chapter_front_matter(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut FrontMatter),
    ) -> std::io::Result<()> {
        let path = self.chapter_path(index);
        // A failed read must abort: splicing onto an empty body would blank the
        // chapter's prose while reporting success (#565).
        let text = read_existing(&path)?;
        let (front, body) = frontmatter::split(&text);
        let mut fm = front.map(FrontMatter::from_raw).unwrap_or_default();
        f(&mut fm);
        let front = (!fm.is_empty()).then(|| fm.as_str());
        atomic::write(&path, frontmatter::join(front, body).as_bytes())?;
        self.cache_chapter_meta(index, Some(&fm));
        Ok(())
    }

    /// Rewrite chapter `index`'s prose body on disk (#295), replacing each byte
    /// range in `ranges` with `replacement`, with front matter preserved and the
    /// cached word count refreshed. Ranges are byte offsets into the body and are
    /// applied right-to-left so earlier offsets stay valid. The inverse of
    /// [`edit_chapter_front_matter`](Self::edit_chapter_front_matter), which edits
    /// the front matter and preserves the body. Used by manuscript-wide replace
    /// for chapters other than the active one (whose live buffer is edited
    /// directly so its change stays undoable).
    pub(crate) fn replace_chapter_body(
        &mut self,
        index: usize,
        ranges: &[(usize, usize)],
        replacement: &str,
    ) -> std::io::Result<()> {
        let path = self.chapter_path(index);
        // A failed read must abort: splicing onto an empty body would wipe the
        // chapter's prose while the replace reported success (#565).
        let text = read_existing(&path)?;
        let (front, body) = frontmatter::split(&text);
        let mut body = body.to_string();
        let mut ranges = ranges.to_vec();
        ranges.sort_by_key(|&(s, _)| s);
        for &(start, end) in ranges.iter().rev() {
            if start <= end && end <= body.len() {
                body.replace_range(start..end, replacement);
            }
        }
        let front = front.map(FrontMatter::from_raw);
        let content = frontmatter::join(front.as_ref().map(FrontMatter::as_str), &body);
        atomic::write(&path, content.as_bytes())?;
        self.read_chapter_cache(index); // refresh the cached word count
        Ok(())
    }

    /// Write the manifest back to `book.toml`. A standalone novel serializes the
    /// flat manifest exactly as before; a multi-book series (#328) folds the live
    /// active book back into the series list and writes the `[[books]]` form.
    pub fn save_manifest(&self) -> std::io::Result<()> {
        let err = |e| std::io::Error::other(format!("could not serialize manifest: {e}"));
        let text = if self.books.is_empty() {
            toml::to_string_pretty(&self.manifest).map_err(err)?
        } else {
            series::series_to_toml(&self.manifest, &self.books, self.active_book).map_err(err)?
        };
        atomic::write(&Self::manifest_path(&self.root), text.as_bytes())
    }
}

//! Chapter-file and manifest I/O for a project: reading a chapter's text and
//! front matter from disk, the splice-then-write helpers that edit a chapter's
//! front matter or body in place, importing a manuscript into per-chapter files,
//! caching per-chapter metadata at open, and writing `book.toml` back. Every
//! write that derives its content from existing bytes reads through
//! [`read_existing`] first, so a failed read aborts rather than blanking the file
//! (#565). Extracted from `mod.rs` to keep that file under the ~1,000-line
//! convention (#562) — a pure move, no behavior change.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::atomic;
use crate::buffer::count_words;
use crate::config;
use crate::frontmatter::{self, FrontMatter};
use crate::text::count_matches;

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
pub fn read_existing(path: &Path) -> std::io::Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// A hidden staging path under `root` for a chapter file being imported (#600),
/// so the new set can be written durably before any original is touched. Hidden
/// (dot-prefixed) and process-keyed so a leftover never shows up as a stray
/// chapter and parallel test processes can't collide. It sits in `root` — the
/// same directory as the final file — so the swap into place is a same-filesystem
/// rename.
fn import_staging_path(root: &Path, file: &str) -> PathBuf {
    root.join(format!(".import.{file}.{}.tmp", std::process::id()))
}

/// What [`cache_chapter`] found on disk for a chapter (#934): whether its file
/// read, was missing, or failed to read. Recorded on the [`Project`] so the
/// on-open damage check reads it from memory instead of touching every file again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChapterRead {
    /// The file read cleanly (possibly empty).
    Read,
    /// No file at the path — cached as an empty chapter, but damage to the
    /// project when the manifest still lists it.
    Missing,
    /// A hard read failure (permissions, an I/O fault, invalid UTF-8, a directory
    /// in the file's place) — the cache is left as it was.
    Unreadable,
}

/// Read `entry`'s file under `root` and refresh its cached word count, TODO
/// count, and front-matter metadata (#331, #867). Shared by the active book's
/// caching and the other books of a series, so every book's counts/roles are
/// populated at open without re-reading later. One file read; never holds the
/// body text. `marker` is the series-wide TODO marker (#867), so the save-path
/// TODO roll-up sums cached counts instead of re-scanning each file.
///
/// A *hard* read failure (permissions, a transient I/O fault) leaves the cache
/// untouched rather than recording the chapter as 0 words with its metadata
/// dropped, which would corrupt the word/TODO rollups until the next good read
/// (#871). A missing file is the legitimate "starts empty" case and caches as
/// empty, exactly as before — the same line [`read_existing`] draws. Returns which
/// of the three it was, for the damage record ([`record_read`]).
pub fn cache_chapter(root: &Path, entry: &mut ChapterEntry, marker: &str) -> ChapterRead {
    let (text, outcome) = match fs::read_to_string(root.join(&entry.file)) {
        Ok(text) => (text, ChapterRead::Read),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), ChapterRead::Missing),
        // Hard read error — keep the last good cache, don't zero it out.
        Err(_) => return ChapterRead::Unreadable,
    };
    let (front, body) = frontmatter::split(&text);
    entry.words = count_words(body);
    entry.todo = count_matches(body, marker);
    entry.cache_meta(front.map(FrontMatter::from_raw).as_ref());
    outcome
}

/// Fold one [`cache_chapter`] outcome into a project's set of damaged chapter
/// files (#934): a missing or unreadable file is added, a clean read clears it.
/// Keyed by the chapter's root-relative file, so it survives reordering.
pub fn record_read(damaged: &mut HashSet<String>, file: &str, outcome: ChapterRead) {
    if outcome == ChapterRead::Read {
        damaged.remove(file);
    } else {
        damaged.insert(file.to_string());
    }
}

/// Pre-flight the swap renames of [`Project::write_chapters_atomic`] so the loop
/// is overwhelmingly unlikely to fail partway (#262). Each `dest` is the
/// already-symlink-resolved destination; rename fails if `dest` is a directory or
/// its parent isn't a writable directory, so both are checked here before any
/// rename lands. A pre-flight error aborts with nothing swapped.
fn preflight_swaps(plan: &[(usize, PathBuf, PathBuf, &String)]) -> std::io::Result<()> {
    for (_, _, dest, _) in plan {
        if dest.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("destination is a directory: {}", dest.display()),
            ));
        }
        let parent = dest.parent().filter(|p| !p.as_os_str().is_empty());
        if let Some(parent) = parent {
            let meta = fs::metadata(parent)?; // parent must exist and be readable
            if !meta.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    format!(
                        "destination parent is not a directory: {}",
                        parent.display()
                    ),
                ));
            }
            if meta.permissions().readonly() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("destination parent is not writable: {}", parent.display()),
                ));
            }
        }
    }
    Ok(())
}

/// Roll back the swap renames already applied by [`Project::write_chapters_atomic`]
/// when a later rename fails (#262), restoring each destination to the `original`
/// bytes captured before the swap so no chapter is left half-updated. A `None`
/// original was a file that didn't exist before the swap (a brand-new chapter), so
/// its rollback removes the just-written file instead of restoring bytes. Best
/// effort: a rollback write can itself fail (the same fault that broke the swap),
/// so failures are swallowed — the caller already returns the original error.
fn rollback_swaps(applied: &[(usize, PathBuf, PathBuf, &String)], originals: &[Option<Vec<u8>>]) {
    for ((_, _, dest, _), original) in applied.iter().zip(originals) {
        match original {
            Some(bytes) => {
                let _ = atomic::write(dest, bytes);
            }
            None => {
                let _ = fs::remove_file(dest);
            }
        }
    }
}

impl Project {
    /// Replace this project's chapters with an imported manuscript (#298): write
    /// each chapter body to its own file, rebuild the manifest in reading order,
    /// and set the book title / author when the source supplied them.
    ///
    /// Safe under a mid-operation failure (#600): the new files are staged to
    /// hidden temp paths first, and only once the whole new set is durably on
    /// disk are they swapped into place and any orphaned old files removed. A
    /// staging failure (disk full, permissions) leaves the originals and the
    /// manifest untouched — the same durability the atomic-write layer gives a
    /// single file, extended across the multi-file replacement. Each stage/swap
    /// goes through the atomic-save path; the source is never touched.
    pub fn import_chapters(
        &mut self,
        title: Option<&str>,
        author: Option<&str>,
        chapters: &[(String, String)],
    ) -> std::io::Result<()> {
        if self.is_read_only() {
            return Err(Self::read_only_error()); // #871: don't race another instance
        }
        // Stage every new file to a hidden temp path first. Nothing destructive
        // happens here, so a failure returns with the originals still on disk and
        // `self` unmodified — we clean up whatever we already staged.
        let staged: Vec<(String, PathBuf)> = (1..=chapters.len())
            .map(|n| {
                let file = format!("chapter-{n}.md");
                let path = import_staging_path(&self.root, &file);
                (file, path)
            })
            .collect();
        for (i, ((_, staged_path), (_, body))) in staged.iter().zip(chapters).enumerate() {
            if let Err(e) = atomic::write(staged_path, body.as_bytes()) {
                for (_, done) in &staged[..i] {
                    let _ = fs::remove_file(done); // best-effort cleanup
                }
                return Err(e);
            }
        }

        // The full new set is durable. From here the swap is the destructive
        // step, but every original it overwrites has an equivalent already
        // staged, so the manifest never points at a file that isn't on disk.
        let old_files: Vec<String> = self
            .manifest
            .chapters
            .iter()
            .map(|c| c.file.clone())
            .collect();
        self.manifest.chapters.clear();
        for ((file, staged_path), (ctitle, _)) in staged.iter().zip(chapters) {
            fs::rename(staged_path, self.root.join(file))?;
            self.manifest
                .chapters
                .push(ChapterEntry::new(ctitle.clone(), file.clone()));
        }
        // Remove old chapter files the new set didn't overwrite (a shorter
        // import, or differently named originals) — orphans only, after the swap.
        for old in &old_files {
            if !staged.iter().any(|(file, _)| file == old) {
                let _ = fs::remove_file(self.root.join(old)); // best effort
            }
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

    /// Read chapter `index` from disk once and refresh its cached word count, TODO
    /// count, and front-matter metadata (status / summary / POV). Used at open and
    /// after an off-buffer edit to a single chapter. Adjusts the running word total
    /// by the delta (#867), so caching every chapter at open stays O(n) rather than
    /// a full re-sum per chapter. Records whether the file was missing or
    /// unreadable for the on-open damage check (#934).
    pub(super) fn read_chapter_cache(&mut self, index: usize) {
        if index < self.manifest.chapters.len() {
            let marker = self.manifest.todo_marker().to_string();
            let before = self.manifest.chapters[index].words;
            let entry = &mut self.manifest.chapters[index];
            let outcome = cache_chapter(&self.root, entry, &marker);
            record_read(&mut self.damaged_chapters, &entry.file, outcome);
            let after = self.manifest.chapters[index].words;
            self.words_total = self.words_total + after - before;
        }
    }

    /// Read chapter `index`'s file from disk. A missing file is the legitimate
    /// "empty chapter" case and yields `""`; any *hard* failure (permissions, a
    /// transient I/O fault) is propagated so a caller can tell an empty chapter
    /// from an unreadable one, rather than a blanket "" that makes a read error
    /// look like real content (#871). Routes through [`read_existing`], the same
    /// line the splice-then-write paths draw. Off the per-frame path.
    fn try_read_chapter(&self, index: usize) -> std::io::Result<String> {
        read_existing(&self.chapter_path(index))
    }

    /// The front matter of chapter `index` as written on disk (`None` if the file
    /// has no block, empty, or can't be read). For reading a chapter's metadata
    /// that isn't cached — keep it off the per-frame path.
    pub fn chapter_front_matter(&self, index: usize) -> Option<FrontMatter> {
        let text = self.try_read_chapter(index).unwrap_or_default();
        let (front, _) = frontmatter::split(&text);
        front.map(FrontMatter::from_raw)
    }

    /// The prose body of chapter `index` from disk (front matter stripped),
    /// empty if it can't be read. Off the per-frame path (used by the report).
    /// Callers that must distinguish an unreadable chapter from an empty one —
    /// e.g. the recovery offer — use [`try_chapter_body`](Self::try_chapter_body).
    pub fn chapter_body(&self, index: usize) -> String {
        self.try_chapter_body(index).unwrap_or_default()
    }

    /// The prose body of chapter `index` from disk, distinguishing a hard read
    /// failure (`Err`) from a missing/empty file (`Ok("")`) (#871). The recovery
    /// offer relies on this: on a hard read error it must *not* treat the chapter
    /// as differing from disk (an empty body compared against captured prose looks
    /// "changed"), which would spuriously re-offer content that was never lost.
    pub fn try_chapter_body(&self, index: usize) -> std::io::Result<String> {
        Ok(frontmatter::split(&self.try_read_chapter(index)?)
            .1
            .to_string())
    }

    /// Split chapter `index`'s file into its front matter and prose body in a
    /// single read, distinguishing a hard read failure (`Err`) from a
    /// missing/empty file (`Ok`) (#565/#871). Unlike the swallowing
    /// [`chapter_front_matter`](Self::chapter_front_matter) /
    /// [`chapter_body`](Self::chapter_body) pair, a permissions/I-O fault surfaces
    /// as `Err` here rather than collapsing to default front matter + `""` body —
    /// which the cross-chapter move must know about, since composing a
    /// destination chapter's new content onto an empty base would durably wipe its
    /// real prose while reporting success. Off the per-frame path.
    pub fn try_split_chapter(
        &self,
        index: usize,
    ) -> std::io::Result<(Option<FrontMatter>, String)> {
        let text = self.try_read_chapter(index)?;
        let (front, body) = frontmatter::split(&text);
        Ok((front.map(FrontMatter::from_raw), body.to_string()))
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
        if self.is_read_only() {
            return Err(Self::read_only_error()); // #871: don't race another instance
        }
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
        if self.is_read_only() {
            return Err(Self::read_only_error()); // #871: don't race another instance
        }
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

    /// Atomically rewrite the full contents of several chapter files at once
    /// (#974): the cross-chapter scene move on the manuscript corkboard carries a
    /// scene's prose + `scenes:` metadata from a source chapter into a destination
    /// chapter, so *both* files must land together or neither. `contents` is the
    /// verbatim new bytes for each `(index, content)` (front matter already joined
    /// on by the caller).
    ///
    /// Multi-file durability mirrors [`import_chapters`](Self::import_chapters):
    /// every new file is staged to a hidden sibling temp path first — the step
    /// where a disk-full / permissions / (in tests) injected fault lands — and only
    /// once *all* are durably on disk are they renamed into place. A staging failure
    /// removes whatever temp was already written and returns with every original
    /// untouched, so a failed move never leaves one chapter rewritten and the other
    /// stale. Refreshes each chapter's cached word/TODO/metadata after.
    ///
    /// The final swap is guarded on two fronts (#262/#871):
    ///
    /// - **Symlink-following.** Each destination is resolved through
    ///   [`atomic::resolve_symlink`] before the rename, so a chapter symlinked into
    ///   a synced/shared folder has its *target* replaced and the link preserved,
    ///   rather than the bare rename replacing the link with a regular file and
    ///   orphaning the target as silently stale (#871) — the same guarantee
    ///   [`atomic::write`] gives a single file.
    /// - **All-or-nothing renames.** Before touching anything the loop pre-flights
    ///   every destination (parent writable, destination not a directory) so a
    ///   rename is overwhelmingly unlikely to fail partway; and it captures each
    ///   original's bytes so that if a later rename *does* fail with an earlier one
    ///   already applied, it rolls the applied ones back to their originals. On any
    ///   swap failure the caller sees `Err` with every destination either all-new
    ///   or all-restored — never a half-updated split state (#262).
    pub(crate) fn write_chapters_atomic(
        &mut self,
        contents: &[(usize, String)],
    ) -> std::io::Result<()> {
        if self.is_read_only() {
            return Err(Self::read_only_error()); // #871: don't race another instance
        }
        // Resolve each destination through any symlink now (#871), so both the
        // pre-flight and the swap act on the real target — a bare rename onto the
        // link would replace it with a regular file and orphan the target. The
        // resolved path is where the temp is renamed and where a rollback restores.
        let mut plan: Vec<(usize, PathBuf, PathBuf, &String)> = Vec::with_capacity(contents.len());
        for (index, content) in contents {
            let dest = atomic::resolve_symlink(&self.chapter_path(*index))?;
            let tmp = import_staging_path(&self.root, self.chapter_file(*index));
            plan.push((*index, tmp, dest, content));
        }

        // Stage every new file to a hidden temp path first; nothing destructive
        // happens until the renames below, so a failure here leaves every original
        // in place — clean up whatever was already staged.
        for (i, (_, tmp, _, content)) in plan.iter().enumerate() {
            if let Err(e) = atomic::write(tmp, content.as_bytes()) {
                for (_, done, _, _) in &plan[..i] {
                    let _ = fs::remove_file(done); // best-effort cleanup
                }
                return Err(e);
            }
        }

        // Pre-flight every destination so the swap loop is overwhelmingly unlikely
        // to fail partway: a destination that is a directory (rename would fail) or
        // whose parent isn't writable is caught here, before any rename lands — so
        // the common failure modes abort with nothing swapped and all temps cleaned
        // up, rather than mid-loop with one chapter already rewritten.
        if let Err(e) = preflight_swaps(&plan) {
            for (_, tmp, _, _) in &plan {
                let _ = fs::remove_file(tmp); // best-effort cleanup
            }
            return Err(e);
        }

        // Capture each original's bytes before the swap so a mid-loop failure can
        // restore the destinations already renamed — the all-or-nothing guarantee.
        // A missing original (a brand-new chapter file) captures as `None`, and a
        // rollback removes it rather than restoring bytes.
        let originals: Vec<Option<Vec<u8>>> = plan
            .iter()
            .map(|(_, _, dest, _)| fs::read(dest).ok())
            .collect();

        // Every file is durably staged and pre-flighted. Swap each temp into place;
        // on a failure, roll back the renames already applied so no destination is
        // left half-updated.
        for (i, (_, tmp, dest, _)) in plan.iter().enumerate() {
            let failed = {
                #[cfg(any(test, feature = "test-support"))]
                {
                    atomic::fail_point::should_fail_swap()
                }
                #[cfg(not(any(test, feature = "test-support")))]
                {
                    false
                }
            };
            let result = if failed {
                Err(std::io::Error::other("injected swap failure"))
            } else {
                fs::rename(tmp, dest)
            };
            if let Err(e) = result {
                rollback_swaps(&plan[..i], &originals[..i]);
                for (_, leftover, _, _) in &plan[i..] {
                    let _ = fs::remove_file(leftover); // best-effort cleanup
                }
                return Err(e);
            }
        }
        for (index, _, _, _) in &plan {
            self.read_chapter_cache(*index); // refresh cached word/TODO/metadata
        }
        Ok(())
    }

    /// Write the manifest back to `book.toml`. A standalone novel serializes the
    /// flat manifest exactly as before; a multi-book series (#328) folds the live
    /// active book back into the series list and writes the `[[books]]` form.
    ///
    /// Top-level keys the manifest doesn't model — the `[quill]` config-override
    /// table (#710), or an additive key from a newer 1.x (#1097) — are carried
    /// over from the file on disk rather than dropped. They're read at save time,
    /// not cached at open, so a hand edit made while Quill is running survives the
    /// next structural save too. A file that no longer parses aborts the save
    /// instead of being overwritten (its unknown keys can't be recovered).
    pub fn save_manifest(&self) -> std::io::Result<()> {
        if self.is_read_only() {
            return Err(Self::read_only_error()); // #871: don't race another instance
        }
        let path = Self::manifest_path(&self.root);
        // A failed read must abort (#565): writing without the file's unknown keys
        // would silently erase the author's `[quill]` overrides.
        let existing = read_existing(&path)?;
        let extra = series::unknown_keys(&existing).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} no longer parses, so it wasn't rewritten: {e}",
                    path.display()
                ),
            )
        })?;
        let text = series::manifest_to_toml(&self.manifest, &self.books, self.active_book, &extra)
            .map_err(|e| std::io::Error::other(format!("could not serialize manifest: {e}")))?;
        atomic::write(&path, text.as_bytes())?;
        // The manifest carries the `[quill.*]` config overrides (#710): an open
        // config load session must re-read it rather than serve the old parse.
        config::forget_parsed();
        Ok(())
    }
}

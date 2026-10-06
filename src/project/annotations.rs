//! The project's inline-annotations store (#302): the in-memory set loaded from
//! `annotations.toml` at open, the per-chapter read the editor anchors from, and
//! the batched write-back that persists the loaded chapters' notes.
//!
//! The write-back runs on hot, keystroke-adjacent paths — every chapter switch,
//! every save and autosave, every continuous-view page — and the side file goes
//! through [`atomic::write`], whose two `fsync`s cost several milliseconds each on
//! macOS (`F_FULLFSYNC`). So [`Project::set_annotations`] takes every loaded
//! chapter at once, writes the file at most once, and skips the write entirely
//! when nothing it was handed differs from what's already stored.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

use crate::annotation::{Annotation, AnnotationFile, Live};
use crate::atomic;

use super::{Project, ANNOTATIONS_NAME};

/// A borrowed view of [`AnnotationFile`] for serializing: the same single
/// `annotation` array (so the TOML is byte-identical), but over a slice, so a
/// write never clones the whole project's notes just to format them.
#[derive(Serialize)]
struct AnnotationFileRef<'a> {
    #[serde(rename = "annotation")]
    items: &'a [Annotation],
}

impl Project {
    /// The inline annotations (#302) anchored in chapter `file`, as live buffer
    /// ranges (chapter-local offsets == buffer offsets in the single-chapter
    /// view), in document order. Out-of-range offsets are the caller's to clamp.
    pub fn annotations_for(&self, file: &str) -> Vec<Live> {
        let mut live: Vec<_> = self
            .annotations
            .iter()
            .filter(|a| a.chapter == file)
            .map(|a| Live {
                chapter: file.to_string(),
                start: a.start,
                end: a.end,
                note: a.note.clone(),
                replies: a.replies.clone(),
                resolved: a.resolved,
                reviewer: a.reviewer.clone(),
            })
            .collect();
        live.sort_by_key(|a| (a.start, a.end));
        live
    }

    /// Replace chapter `file`'s annotations with `live` (byte ranges + notes) and
    /// persist the side file (#302) — [`set_annotations`](Self::set_annotations)
    /// for a single chapter.
    #[cfg(test)]
    pub fn set_chapter_annotations(&mut self, file: &str, live: &[Live]) -> io::Result<()> {
        self.set_annotations([(file, live)])
    }

    /// Replace each listed chapter's annotations with its new set (chapter-local
    /// byte ranges + notes) and write the side file **once** for the whole batch
    /// (#302/#370). Other chapters' annotations are kept. Removing the file when
    /// nothing is left anywhere keeps the project tidy.
    ///
    /// Each chapter's new set is compared with what's stored before anything is
    /// mutated; when every chapter is unchanged (and no earlier write is still
    /// owed) the call touches no file at all. That's the common case — a clean
    /// chapter switch, or a save that didn't move any note — and it's what keeps
    /// those paths off the side file's `fsync`s.
    ///
    /// The on-disk form is unchanged by the batching: the whole set is stably
    /// sorted by `(chapter, start, end)`, and a chapter's notes that tie on
    /// `(start, end)` keep the order they were handed in.
    pub fn set_annotations<'a, I>(&mut self, updates: I) -> io::Result<()>
    where
        I: IntoIterator<Item = (&'a str, &'a [Live])>,
    {
        let mut changed = false;
        for (file, live) in updates {
            let mut fresh: Vec<Annotation> = live
                .iter()
                .map(|a| Annotation {
                    chapter: file.to_string(),
                    start: a.start,
                    end: a.end,
                    note: a.note.clone(),
                    replies: a.replies.clone(),
                    resolved: a.resolved,
                    reviewer: a.reviewer.clone(),
                })
                .collect();
            // The order the whole-set sort below gives this chapter's notes (it's
            // stable, so ties keep their input order) — compared against the stored
            // order, which that same sort produced at the last write.
            fresh.sort_by_key(|a| (a.start, a.end));
            if self
                .annotations
                .iter()
                .filter(|a| a.chapter == file)
                .eq(fresh.iter())
            {
                continue;
            }
            self.annotations.retain(|a| a.chapter != file);
            self.annotations.extend(fresh);
            changed = true;
        }
        if !changed && !self.annotations_unsaved {
            return Ok(());
        }
        // Owed until a write lands: if this one fails, the next call retries even
        // though memory then already matches what it's handed.
        self.annotations_unsaved = true;
        let path = self.root.join(ANNOTATIONS_NAME);
        if self.annotations.is_empty() {
            // Nothing left anywhere — remove the side file rather than write `[]`.
            match fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        } else {
            self.annotations
                .sort_by(|a, b| (&a.chapter, a.start, a.end).cmp(&(&b.chapter, b.start, b.end)));
            let file = AnnotationFileRef {
                items: &self.annotations,
            };
            let text = toml::to_string_pretty(&file).map_err(io::Error::other)?;
            atomic::write(&path, text.as_bytes())?;
        }
        #[cfg(test)]
        write_count::bump();
        self.annotations_unsaved = false;
        Ok(())
    }
}

/// Read the inline annotations side file (#302). Best-effort: a missing or
/// malformed file yields no annotations rather than failing the open.
pub(super) fn load_annotations(root: &Path) -> Vec<Annotation> {
    fs::read_to_string(root.join(ANNOTATIONS_NAME))
        .ok()
        .and_then(|text| toml::from_str::<AnnotationFile>(&text).ok())
        .map(|f| f.items)
        .unwrap_or_default()
}

/// Test-only count of side-file writes (and removals) on this thread, so tests
/// can assert a batch writes once and an unchanged one writes nothing — content
/// alone can't tell "rewritten identically" from "not touched". Compiled out of
/// production builds.
#[cfg(test)]
pub mod write_count {
    use std::cell::Cell;

    thread_local! {
        static WRITES: Cell<usize> = const { Cell::new(0) };
    }

    /// Record one write (or removal) of `annotations.toml`.
    pub(super) fn bump() {
        WRITES.with(|w| w.set(w.get() + 1));
    }

    /// How many times this thread has written (or removed) `annotations.toml`.
    pub fn get() -> usize {
        WRITES.with(Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::Reply;
    use crate::testutil::scratch_dir;

    fn note(chapter: &str, start: usize, end: usize, text: &str) -> Annotation {
        Annotation {
            chapter: chapter.into(),
            start,
            end,
            note: text.into(),
            replies: vec![Reply {
                author: "you".into(),
                text: "reply".into(),
            }],
            resolved: start.is_multiple_of(2),
            reviewer: start.is_multiple_of(3).then(|| "Ann".into()),
        }
    }

    // The borrowed serializer must produce exactly what the owned
    // `AnnotationFile` did, so the batching change never rewrites a file's bytes.
    #[test]
    fn borrowed_view_serializes_byte_identically() {
        let items = vec![
            note("a.md", 0, 4, "first"),
            note("a.md", 3, 9, "second \"quoted\""),
            note("b.md", 6, 7, "third"),
        ];
        let owned = toml::to_string_pretty(&AnnotationFile {
            items: items.clone(),
        })
        .unwrap();
        let borrowed = toml::to_string_pretty(&AnnotationFileRef { items: &items }).unwrap();
        assert_eq!(owned, borrowed);
        // And it still round-trips through the loader's owned form.
        let back: AnnotationFile = toml::from_str(&borrowed).unwrap();
        assert_eq!(back.items, items);
    }

    fn live(chapter: &str, start: usize, end: usize, text: &str) -> Live {
        Live {
            chapter: chapter.into(),
            start,
            end,
            note: text.into(),
            replies: Vec::new(),
            resolved: false,
            reviewer: None,
        }
    }

    /// A created project at a fresh scratch dir (the store needs a real root).
    fn project(name: &str) -> Project {
        Project::create(scratch_dir("proj-anno", name)).unwrap()
    }

    fn side(p: &Project) -> Option<String> {
        fs::read_to_string(p.root.join(ANNOTATIONS_NAME)).ok()
    }

    #[test]
    fn a_batch_writes_the_side_file_once() {
        let mut p = project("batch");
        let a = [live("a.md", 0, 3, "one")];
        let b = [live("b.md", 4, 9, "two")];
        let c = [live("c.md", 1, 2, "three")];
        let before = write_count::get();
        p.set_annotations([("a.md", &a[..]), ("b.md", &b[..]), ("c.md", &c[..])])
            .unwrap();
        assert_eq!(
            write_count::get() - before,
            1,
            "one write for three chapters"
        );
        let text = side(&p).unwrap();
        for n in ["one", "two", "three"] {
            assert!(text.contains(n), "{text}");
        }
    }

    #[test]
    fn an_unchanged_batch_touches_no_file() {
        let mut p = project("unchanged");
        let a = [live("a.md", 5, 9, "late"), live("a.md", 0, 3, "early")];
        p.set_annotations([("a.md", &a[..])]).unwrap();
        // Mark the file: a rewrite would drop the comment.
        let marked = format!("# untouched\n{}", side(&p).unwrap());
        fs::write(p.root.join(ANNOTATIONS_NAME), &marked).unwrap();

        let before = write_count::get();
        // Same notes, handed in a different order: still the same stored set.
        let again = [live("a.md", 0, 3, "early"), live("a.md", 5, 9, "late")];
        p.set_annotations([("a.md", &again[..]), ("b.md", &[][..])])
            .unwrap();
        assert_eq!(
            write_count::get(),
            before,
            "nothing changed, nothing written"
        );
        assert_eq!(side(&p).unwrap(), marked);

        // A real change (one note moved) does write.
        let moved = [live("a.md", 1, 3, "early"), live("a.md", 5, 9, "late")];
        p.set_annotations([("a.md", &moved[..])]).unwrap();
        assert_eq!(write_count::get(), before + 1);
        assert!(!side(&p).unwrap().contains("# untouched"));
    }

    #[test]
    fn the_on_disk_order_matches_the_per_chapter_writes() {
        // The batched write must produce exactly the bytes the old one-chapter-at-
        // a-time write did: sorted by (chapter, start, end), ties in input order.
        let mut batched = project("order-batched");
        let mut single = project("order-single");
        let b = [
            live("b.md", 4, 6, "tie-first"),
            live("b.md", 0, 2, "b-early"),
            live("b.md", 4, 6, "tie-second"),
        ];
        let a = [live("a.md", 9, 12, "a-late"), live("a.md", 1, 2, "a-early")];
        batched
            .set_annotations([("b.md", &b[..]), ("a.md", &a[..])])
            .unwrap();
        single.set_chapter_annotations("b.md", &b).unwrap();
        single.set_chapter_annotations("a.md", &a).unwrap();
        let text = side(&batched).unwrap();
        assert_eq!(text, side(&single).unwrap());
        let order: Vec<_> = ["a-early", "a-late", "b-early", "tie-first", "tie-second"]
            .iter()
            .map(|n| text.find(n).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{text}");
    }

    #[test]
    fn removing_the_last_annotation_removes_the_side_file() {
        let mut p = project("remove-last");
        let a = [live("a.md", 0, 3, "one")];
        let b = [live("b.md", 0, 3, "two")];
        p.set_annotations([("a.md", &a[..]), ("b.md", &b[..])])
            .unwrap();
        // Clearing one chapter keeps the file (the other's note remains)...
        p.set_annotations([("a.md", &[][..])]).unwrap();
        let text = side(&p).expect("b.md's note keeps the file");
        assert!(!text.contains("one") && text.contains("two"), "{text}");
        // ...clearing the last one removes it.
        p.set_annotations([("b.md", &[][..])]).unwrap();
        assert!(side(&p).is_none(), "no notes left, no side file");
    }

    #[test]
    fn a_failed_write_is_retried_even_when_unchanged() {
        let mut p = project("retry");
        let a = [live("a.md", 0, 3, "one")];
        atomic::fail_point::arm_pre_rename_failure();
        assert!(p.set_annotations([("a.md", &a[..])]).is_err());
        assert!(side(&p).is_none(), "the failed write left no file");
        // Memory already holds the note, but the write is still owed.
        p.set_annotations([("a.md", &a[..])]).unwrap();
        assert!(side(&p).unwrap().contains("one"));
    }
}

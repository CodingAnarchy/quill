//! The review-copy export (#834): a `.docx` with numbered lines and generous
//! margins for markup, carrying the author's inline annotations (#302) as Word
//! comments anchored to their ranges — the collaboration/handoff artifact for a
//! beta reader or editor. Every other export strips annotations; this one carries
//! them.

use super::super::super::*;

use crate::ai::bible;
use crate::export;
use crate::frontmatter::Band;

impl App {
    /// Export a review copy (#834): a `.docx` with numbered lines and generous
    /// margins for markup, carrying the author's inline annotations (#302) as Word
    /// comments anchored to their ranges — the collaboration/handoff artifact for a
    /// beta reader or editor. Every other export strips annotations; this one alone
    /// carries them. Compiles the active book (the whole novel, never a sample) from
    /// the single-chapter view, like the other exports, so nothing is read stale.
    pub(crate) fn run_review_copy_export(&mut self) {
        if self.manuscript.is_some() {
            self.message =
                Some("Leave the continuous view (Ctrl-l) before exporting a review copy.".into());
            return;
        }
        let doc = self.export_document(None);
        let notes = self.review_chapter_notes();
        let name = format!("{}-review", bible::slug(&self.project.manifest.title));
        self.write_built(
            export::docx::to_docx_review(&doc, &notes),
            &name,
            "review.docx",
        );
    }

    /// The inline annotations (#302) for each chapter, in the order the review
    /// writer visits chapters — front, then the body (which the [`Document`]'s
    /// blocks group into parts, in chapter-index order), then back — matching how
    /// [`export_document`](Self::export_document) assembles the tree. Each note
    /// carries the exact prose it was anchored to (extracted from the chapter body)
    /// so the writer can re-find it without threading byte offsets through the
    /// body's export transforms.
    fn review_chapter_notes(&self) -> Vec<export::docx::ChapterNotes> {
        // One pass per band so the order matches `Document::blocks` (front → body →
        // back). Within a band, chapters stay in manifest-index order.
        let mut front = Vec::new();
        let mut body = Vec::new();
        let mut back = Vec::new();
        // Resolve the active index once — `active_index()` is O(n), so reading it
        // per chapter (via `chapter_text`) would make this O(chapters²) (#865, #867).
        let active = self.project.active_index();
        for i in 0..self.chapter_count() {
            let notes = self.chapter_review_notes(i, active);
            match self.project.chapter_band(i) {
                Band::Front => front.push(notes),
                Band::Body => body.push(notes),
                Band::Back => back.push(notes),
            }
        }
        front.extend(body);
        front.extend(back);
        front
    }

    /// One chapter's annotations as [`ReviewNote`]s (#834): each stored range
    /// resolved against the chapter's body text to the exact snippet it anchors, so
    /// the review writer can locate and comment it. A range that no longer falls
    /// within the body (a stale side-file entry) is skipped.
    fn chapter_review_notes(&self, i: usize, active: usize) -> export::docx::ChapterNotes {
        let text = self.chapter_text_with_active(i, active);
        let file = self.project.chapter_file(i).to_string();
        self.project
            .annotations_for(&file)
            .into_iter()
            .filter_map(|a| {
                let snippet = text.get(a.start..a.end)?;
                Some(export::docx::ReviewNote {
                    text: snippet.to_string(),
                    note: a.note,
                })
            })
            .collect()
    }
}

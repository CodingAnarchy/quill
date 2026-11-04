//! Document assembly (#190/#332): the editor's side of compiling the project
//! into the export [`Document`] tree. The compiling itself is the core's
//! ([`crate::export::assemble`]), shared with the phone (#1300) so a book exports
//! the same from either; this hands it the open chapter as the editor holds it,
//! unsaved edits and all.

use super::super::super::*;

use crate::export::assemble::{self, Assembly, Live};
use crate::export::{self, Document};
use crate::frontmatter::FrontMatter;

impl App {
    /// The open chapter as the editor holds it, for the export to read instead
    /// of the file: only while the buffer is that chapter (a focused reference
    /// pane holds some other file, and the chapter on disk is current then).
    fn live_chapter(&self, active: usize) -> Option<Live> {
        self.focus_shows_active_chapter().then(|| Live {
            index: active,
            text: self.buffer.text(),
            epigraph: self.buffer.front_matter().and_then(FrontMatter::epigraph),
            words: self.buffer.word_count(),
        })
    }

    /// Assemble the active book into the export tree (#190); see
    /// [`Assembly::document`]. `span` is `None` for the whole book, or a sample's
    /// chapter range (#584).
    pub(super) fn export_document(
        &self,
        span: Option<&std::ops::RangeInclusive<usize>>,
    ) -> Document {
        let style = self.export_style().0;
        let live = self.live_chapter(self.project.active_index());
        Assembly::new(&self.project, live, &style).document(span)
    }

    /// One [`Document`] per book of the series, in reading order (#332).
    pub(super) fn export_series_documents(&self) -> Vec<Document> {
        let style = self.export_style().0;
        let live = self.live_chapter(self.project.active_index());
        Assembly::new(&self.project, live, &style).series_documents()
    }

    /// How many of a collection's stories can't be read for an export (#1245).
    pub(super) fn missing_stories(&self) -> usize {
        assemble::missing_stories(&self.project)
    }

    /// The book-level publishing metadata (#583) for the export tree.
    pub(super) fn publishing_meta(&self) -> export::PubMeta {
        assemble::publishing_meta(&self.project)
    }
}

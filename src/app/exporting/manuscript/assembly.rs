//! Document assembly (#190/#332): compile the live project into the shared export
//! [`Document`] tree — book metadata, front matter, the body grouped into parts,
//! and back matter. The active chapter reads live from its buffer, the rest from
//! disk; a series compiles one [`Document`] per book (#332).

use super::super::super::*;

use crate::config::AuthorConfig;
use crate::export::{self, Chapter, Document, Part};
use crate::frontmatter::Band;
use crate::project::{front_chapter_count, part_at_body_position};

impl App {
    /// Assemble the project into the export tree (#190): book metadata, then the
    /// front matter, the body grouped into parts, and the back matter — each
    /// chapter with its epigraph and prose (active chapter live from its buffer,
    /// the rest from disk; front matter stripped; a repeated title heading dropped).
    ///
    /// `span` is `None` for a whole-book export (every chapter); a *sample* (#584)
    /// passes an inclusive manifest chapter-index range, which keeps only the body
    /// chapters in it — front and back matter are dropped from a sample, and the
    /// kept body chapters still group under their part headings.
    pub(super) fn export_document(
        &self,
        span: Option<&std::ops::RangeInclusive<usize>>,
    ) -> Document {
        let m = &self.project.manifest;
        let parts = self.part_ranges(); // (name, body-chapter index range), in order
        let mut front = Vec::new();
        let mut back = Vec::new();
        let mut body_parts: Vec<Part> = Vec::new();
        // Group consecutive body chapters by the part they're in, in reading order
        // — body chapters in no defined part form an unnamed (heading-less) group.
        // A fresh group opens whenever the part changes (`prev` starts unset so the
        // first body chapter always opens one).
        let mut prev: Option<Option<usize>> = None;
        // Resolve the active index once — `active_index()` is O(n), so reading it per
        // chapter (via `export_chapter` → `chapter_text`) would make this a
        // O(chapters²) compile on a large book (#865, #867).
        let active = self.project.active_index();
        for i in 0..self.chapter_count() {
            // A sample keeps only its chapters; the rest of the book is skipped.
            if span.is_some_and(|s| !s.contains(&i)) {
                continue;
            }
            let chapter = self.export_chapter(i, active);
            match self.project.chapter_band(i) {
                Band::Front => front.push(chapter),
                Band::Back => back.push(chapter),
                Band::Body => {
                    let part = parts.iter().position(|(_, range)| range.contains(&i));
                    if prev != Some(part) {
                        prev = Some(part);
                        body_parts.push(Part {
                            name: part.map(|p| parts[p].0.clone()),
                            chapters: Vec::new(),
                        });
                    }
                    // The first body chapter pushes a part (prev starts `None`), so
                    // `body_parts` is non-empty here; `if let` binds it without a panic.
                    if let Some(part) = body_parts.last_mut() {
                        part.chapters.push(chapter);
                    }
                }
            }
        }

        // The generated front-matter options (#820) are `[style]`-driven; resolve
        // the style once for both the half-title flag and the series-list gate.
        let style = self.export_style().0;
        Document {
            title: m.title.clone(),
            subtitle: m.subtitle.clone(),
            author: m.author.clone(),
            series: m.series.clone(),
            publishing: self.publishing_meta(),
            epigraph: m.epigraph(),
            // A half-title page is opt-in via the `[style]` `half_title` key (#820).
            // The "also in this series" list is generated when this active book is
            // part of a multi-book series (a standalone project lists nothing).
            half_title: style.half_title(),
            series_books: self.series_book_titles(style.series_list()),
            front,
            parts: body_parts,
            back,
            story: self.story_meta(active),
        }
    }

    /// A short story's submission details (#1241), or `None` for a novel: the
    /// `[author]` contact block (falling back to the byline) and the story's word
    /// count, its live buffer's when it's the open chapter.
    fn story_meta(&self, active: usize) -> Option<export::StoryMeta> {
        let story = self.project.manifest.story_chapter()?;
        Some(export::StoryMeta {
            contact: AuthorConfig::load().contact_lines(self.project.manifest.author.as_deref()),
            words: self.chapter_word_count_with_active(story, active),
        })
    }

    /// The series book list for the generated "also in this series" page (#820):
    /// every book of the project by title when it is a multi-book series and the
    /// caller's `series_list` gate (from `[style]`) is on, else empty. Empty for a
    /// standalone project (one book) — its own title is the only entry, which
    /// [`Document::series_list_lines`](crate::export::Document::series_list_lines)
    /// filters out, so the page is omitted. The omnibus series export renders each
    /// book in full, so it carries no such page; this drives the single-book export
    /// of a series title.
    fn series_book_titles(&self, enabled: bool) -> Vec<String> {
        if !enabled || self.project.book_count() <= 1 {
            return Vec::new();
        }
        (0..self.project.book_count())
            .map(|b| self.project.book_title(b).unwrap_or("Untitled").to_string())
            .collect()
    }

    /// The book-level publishing metadata (#583) read from the manifest, for the
    /// export tree — the OPF `dc:*`, the copyright page, and the PDF/DOCX. Shared
    /// by the active-book and per-book document builders so both carry it.
    pub(super) fn publishing_meta(&self) -> export::PubMeta {
        let m = &self.project.manifest;
        export::PubMeta {
            isbn: m.isbn.clone(),
            publisher: m.publisher.clone(),
            date: m.pub_date.clone(),
            lang: m.language.clone(),
            rights: m.rights.clone(),
        }
    }

    /// Book `b`'s own publishing metadata for its divider copyright in a series
    /// export (#810): each book carries its own ISBN/publisher/date/rights (a book
    /// with none falls back to the series/active-book metadata — see
    /// [`Project::book_publishing`]), so an omnibus stamps each book's copyright
    /// with that book's data rather than reusing the active book's for all of them.
    /// Language is a package-level concern kept once on the manifest, so it stays
    /// series-level here.
    fn book_publishing_meta(&self, b: usize) -> export::PubMeta {
        let pubmeta = self.project.book_publishing(b);
        export::PubMeta {
            isbn: pubmeta.isbn,
            publisher: pubmeta.publisher,
            date: pubmeta.pub_date,
            lang: self.project.manifest.language.clone(),
            rights: pubmeta.rights,
        }
    }

    /// Build one chapter for export: its title, epigraph, and prose (front matter
    /// stripped, a repeated title heading dropped). `active` is the pre-resolved
    /// active index so the caller's per-chapter loop stays O(chapters) (#865, #867).
    fn export_chapter(&self, i: usize, active: usize) -> Chapter {
        let title = self
            .project
            .chapter_title(i)
            .unwrap_or("Untitled")
            .to_string();
        let body = export::strip_repeated_title(&self.chapter_text_with_active(i, active), &title);
        Chapter {
            title,
            epigraph: self.chapter_epigraph(i),
            body,
            role: self.project.chapter_role(i),
        }
    }

    /// One [`Document`] per book of the series, in reading order (#332): the
    /// active book compiled live (its active chapter from the buffer), the others
    /// read from disk.
    pub(super) fn export_series_documents(&self) -> Vec<Document> {
        (0..self.project.book_count())
            .map(|b| {
                let mut doc = if b == self.project.active_book() {
                    self.export_document(None)
                } else {
                    self.export_book_document(b)
                };
                // The series title and author are rendered once by the series
                // writer, not repeated above every book. The omnibus renders every
                // book in full, so no book carries an "also in this series" page
                // (#820) — that page is for the single-book export of a series
                // title, driven by `export_document`.
                doc.series = None;
                doc.author = None;
                doc.series_books = Vec::new();
                doc
            })
            .collect()
    }

    /// Compile a non-active book into a [`Document`] (#332), reading its chapters'
    /// prose and front matter from disk and grouping them into front / parts /
    /// back the same way [`export_document`](Self::export_document) does for the
    /// active book.
    fn export_book_document(&self, b: usize) -> Document {
        let chapters = self.project.book_chapters(b);
        let parts = self.project.book_parts(b);
        // Band order (front → body → back), then group body chapters by part.
        let mut order: Vec<usize> = (0..chapters.len()).collect();
        order.sort_by_key(|&i| Band::of(chapters[i].role));
        let front_count = front_chapter_count(chapters);
        let (mut front, mut back, mut body_parts) = (Vec::new(), Vec::new(), Vec::<Part>::new());
        let mut prev: Option<Option<usize>> = None;
        for (pos, &ci) in order.iter().enumerate() {
            let ce = &chapters[ci];
            let (body, fm) = self.project.read_chapter_parts(&ce.file);
            let chapter = Chapter {
                title: ce.title.clone(),
                epigraph: fm.and_then(|f| f.epigraph()),
                body: export::strip_repeated_title(&body, &ce.title),
                role: ce.role,
            };
            match Band::of(ce.role) {
                Band::Front => front.push(chapter),
                Band::Back => back.push(chapter),
                Band::Body => {
                    let part = parts.and_then(|ps| {
                        part_at_body_position(ps, pos.saturating_sub(front_count) + 1)
                    });
                    if prev != Some(part) {
                        prev = Some(part);
                        let name = part.map(|pi| self.project.book_part_display_name(b, pi));
                        body_parts.push(Part {
                            name,
                            chapters: Vec::new(),
                        });
                    }
                    // A new part was just pushed on the first body chapter (prev starts
                    // `None`), so `body_parts` is non-empty here; `if let` binds it
                    // without a panic path.
                    if let Some(part) = body_parts.last_mut() {
                        part.chapters.push(chapter);
                    }
                }
            }
        }
        Document {
            title: self.project.book_title(b).unwrap_or("Untitled").to_string(),
            subtitle: self.project.book_subtitle(b).map(str::to_string),
            author: self.project.manifest.author.clone(),
            // The series title is carried once by the series writer, not per book.
            series: None,
            // Each book in a series carries its OWN publishing metadata (#810): its
            // ISBN/publisher/date/rights (falling back to the series/active-book
            // metadata when unset), so its divider copyright is stamped with its own
            // data rather than the active book's reused for every book.
            publishing: self.book_publishing_meta(b),
            epigraph: self.project.book_epigraph(b),
            // Opt-in half-title (#820). A non-active series book is only ever part
            // of the omnibus export, which carries no "also in this series" page.
            half_title: self.export_style().0.half_title(),
            series_books: Vec::new(),
            front,
            parts: body_parts,
            back,
            story: None,
        }
    }
}

//! Compiling a project into the export [`Document`] tree (#190, #332), shared by
//! every front end (#1300): the desktop editor, and the phone, so a book exported
//! on either comes out the same. Book metadata, then the front matter, the body
//! grouped into parts, and the back matter; a series compiles one [`Document`]
//! per book, and a collection's body is its linked stories.
//!
//! An editor holding a chapter's unsaved text passes it as [`Live`], so the
//! export reflects what's on screen rather than the last save. Everything else is
//! read from disk.
//!
//! The style the writers use ([`resolve_style`]) and the print geometry
//! ([`print_trim`], [`print_bleed`], [`print_caliper`]) are resolved here too, for
//! the same reason.

use std::ops::{Range, RangeInclusive};

use super::{pdf, strip_repeated_title, Chapter, Document, ExportStyle, Part, PubMeta, StoryMeta};
use crate::config::AuthorConfig;
use crate::frontmatter::{Band, Epigraph, FrontMatter};
use crate::project::{front_chapter_count, part_at_body_position, Project};

/// A chapter's text as an editor holds it, unsaved edits and all.
#[derive(Clone, Debug)]
pub struct Live {
    /// The chapter's index in the active book.
    pub index: usize,
    /// Its prose, front matter split off.
    pub text: String,
    /// Its epigraph, from the editor's copy of its front matter.
    pub epigraph: Option<Epigraph>,
    /// Its word count.
    pub words: usize,
}

/// Compiles a project into export documents.
pub struct Assembly<'a> {
    project: &'a Project,
    live: Option<Live>,
    style: &'a ExportStyle,
}

impl<'a> Assembly<'a> {
    /// Compile `project`, with `live` standing in for the chapter an editor
    /// holds (if any), under the resolved `style` (see [`resolve_style`]).
    pub const fn new(project: &'a Project, live: Option<Live>, style: &'a ExportStyle) -> Self {
        Self {
            project,
            live,
            style,
        }
    }

    /// The active book as a [`Document`]: book metadata, then the front
    /// matter, the body grouped into parts, and the back matter, each chapter
    /// with its epigraph and prose (front matter stripped, a repeated title
    /// heading dropped).
    ///
    /// `span` is `None` for the whole book; a *sample* (#584) passes an
    /// inclusive chapter-index range, which keeps only the body chapters in it.
    /// Front and back matter are dropped from a sample, and the kept chapters
    /// still group under their part headings.
    pub fn document(&self, span: Option<&RangeInclusive<usize>>) -> Document {
        let project = self.project;
        let m = &project.manifest;
        let parts = part_ranges(project);
        let mut front = Vec::new();
        let mut back = Vec::new();
        let mut body_parts: Vec<Part> = Vec::new();
        // Group consecutive body chapters by part, in reading order; chapters in
        // no part form an unnamed (heading-less) group. A group opens whenever the
        // part changes (`prev` starts unset so the first body chapter opens one).
        let mut prev: Option<Option<usize>> = None;
        for i in 0..m.chapters.len() {
            if span.is_some_and(|s| !s.contains(&i)) {
                continue;
            }
            let chapter = self.chapter(i);
            match project.chapter_band(i) {
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
                    // The first body chapter pushed a part, so there's a last one.
                    if let Some(part) = body_parts.last_mut() {
                        part.chapters.push(chapter);
                    }
                }
            }
        }

        // A collection's body is its linked stories, read fresh from each story
        // (#1245); its own chapters are the pages around them.
        if span.is_none() && project.is_collection() {
            for part in body_parts.drain(..) {
                front.extend(part.chapters);
            }
            body_parts.push(Part {
                name: None,
                chapters: self.collection_stories(),
            });
        }

        Document {
            title: m.title.clone(),
            subtitle: m.subtitle.clone(),
            author: m.author.clone(),
            series: m.series.clone(),
            publishing: publishing_meta(project),
            epigraph: m.epigraph(),
            // A half-title page is opt-in via `[style] half_title` (#820). The
            // "also in this series" list is generated for a book of a series.
            half_title: self.style.half_title(),
            series_books: self.series_book_titles(),
            front,
            parts: body_parts,
            back,
            story: self.story_meta(),
            novel: self.novel_meta(),
        }
    }

    /// One [`Document`] per book of the series, in reading order (#332): the
    /// active book compiled with the editor's live chapter, the others read from
    /// disk. The series title and author are rendered once by the series writer,
    /// and the omnibus renders every book, so no book carries an "also in this
    /// series" page (#820).
    pub fn series_documents(&self) -> Vec<Document> {
        (0..self.project.book_count())
            .map(|b| {
                let mut doc = if b == self.project.active_book() {
                    self.document(None)
                } else {
                    self.book_document(b)
                };
                doc.series = None;
                doc.author = None;
                doc.series_books = Vec::new();
                doc
            })
            .collect()
    }

    /// Chapter `i` of the active book, for export.
    fn chapter(&self, i: usize) -> Chapter {
        let project = self.project;
        let title = project.chapter_title(i).unwrap_or("Untitled").to_string();
        let (text, epigraph) = match &self.live {
            Some(live) if live.index == i => (live.text.clone(), live.epigraph.clone()),
            _ => (
                project.chapter_body(i),
                project.chapter_front_matter(i).and_then(|fm| fm.epigraph()),
            ),
        };
        Chapter {
            body: strip_repeated_title(&text, &title),
            title,
            epigraph,
            role: project.chapter_role(i),
            verse: project.chapter_is_verse(i),
        }
    }

    /// A short story's submission details (#1241), or `None` for a novel: the
    /// `[author]` contact block (falling back to the byline) and the story's
    /// word count.
    fn story_meta(&self) -> Option<StoryMeta> {
        let story = self.project.manifest.story_chapter()?;
        let words = match &self.live {
            Some(live) if live.index == story => live.words,
            _ => self.project.chapter_words(story),
        };
        Some(StoryMeta {
            contact: AuthorConfig::load().contact_lines(self.project.manifest.author.as_deref()),
            words,
        })
    }

    /// A novel's submission details for its Word manuscript's title page (#1302),
    /// or `None` for a short story or a collection: the `[author]` contact block
    /// (falling back to the byline) and the word count of the body chapters, the
    /// whole book's even when exporting a sample.
    fn novel_meta(&self) -> Option<StoryMeta> {
        let project = self.project;
        if project.manifest.story_chapter().is_some() || project.is_collection() {
            return None;
        }
        let words = (0..project.manifest.chapters.len())
            .filter(|&i| project.chapter_band(i) == Band::Body)
            .map(|i| match &self.live {
                Some(live) if live.index == i => live.words,
                _ => project.chapter_words(i),
            })
            .sum();
        Some(StoryMeta {
            contact: AuthorConfig::load().contact_lines(project.manifest.author.as_deref()),
            words,
        })
    }

    /// The series' book titles for the "also in this series" page (#820), when
    /// this is a multi-book series and `[style] series_list` is on.
    fn series_book_titles(&self) -> Vec<String> {
        let project = self.project;
        if !self.style.series_list() || project.book_count() <= 1 {
            return Vec::new();
        }
        (0..project.book_count())
            .map(|b| project.book_title(b).unwrap_or("Untitled").to_string())
            .collect()
    }

    /// A collection's stories as export chapters, in the collection's order
    /// (#1245): each story's title, its prose read from the story project now,
    /// and its credit line under the title (as an epigraph, or as the first line
    /// when the story has its own epigraph). A story that can't be read is left
    /// out; [`missing_stories`] counts them.
    fn collection_stories(&self) -> Vec<Chapter> {
        (0..self.project.manifest.stories.len())
            .filter_map(|i| self.project.story_text(i).ok())
            .map(|story| {
                let body = strip_repeated_title(&story.body, &story.title);
                let (epigraph, body) = match (story.epigraph, story.credit) {
                    (Some(e), Some(credit)) => (Some(e), format!("{credit}\n\n{body}")),
                    (None, Some(credit)) => (
                        Some(Epigraph {
                            text: credit,
                            attribution: None,
                        }),
                        body,
                    ),
                    (epigraph, None) => (epigraph, body),
                };
                Chapter {
                    title: story.title,
                    epigraph,
                    body,
                    role: None,
                    verse: story.verse,
                }
            })
            .collect()
    }

    /// A book of the series other than the active one, read from disk (#332),
    /// grouped into front, parts, and back the same way as [`Self::document`].
    fn book_document(&self, b: usize) -> Document {
        let project = self.project;
        let chapters = project.book_chapters(b);
        let parts = project.book_parts(b);
        let mut order: Vec<usize> = (0..chapters.len()).collect();
        order.sort_by_key(|&i| Band::of(chapters[i].role));
        let front_count = front_chapter_count(chapters);
        let (mut front, mut back, mut body_parts) = (Vec::new(), Vec::new(), Vec::<Part>::new());
        let mut prev: Option<Option<usize>> = None;
        for (pos, &ci) in order.iter().enumerate() {
            let ce = &chapters[ci];
            let (body, fm) = project.read_chapter_parts(&ce.file);
            let chapter = Chapter {
                title: ce.title.clone(),
                verse: fm.as_ref().is_some_and(FrontMatter::is_verse),
                epigraph: fm.and_then(|f| f.epigraph()),
                body: strip_repeated_title(&body, &ce.title),
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
                        let name = part.map(|pi| project.book_part_display_name(b, pi));
                        body_parts.push(Part {
                            name,
                            chapters: Vec::new(),
                        });
                    }
                    if let Some(part) = body_parts.last_mut() {
                        part.chapters.push(chapter);
                    }
                }
            }
        }
        Document {
            title: project.book_title(b).unwrap_or("Untitled").to_string(),
            subtitle: project.book_subtitle(b).map(str::to_string),
            author: project.manifest.author.clone(),
            series: None,
            // Each book carries its own publishing metadata (#810), so its divider
            // copyright is stamped with its own ISBN and rights.
            publishing: book_publishing_meta(project, b),
            epigraph: project.book_epigraph(b),
            half_title: self.style.half_title(),
            series_books: Vec::new(),
            front,
            parts: body_parts,
            back,
            story: None,
            novel: None,
        }
    }
}

/// The active book's parts as `(name, chapter-index range)`, in order (#185).
/// Part ranges are 1-based positions among the body chapters; the body block
/// is contiguous after the front matter, so a body position maps to
/// `front + position`.
pub fn part_ranges(project: &Project) -> Vec<(String, Range<usize>)> {
    let Some(parts) = project.manifest.parts.as_ref() else {
        return Vec::new();
    };
    let label = project.manifest.part_label();
    let front = project.manifest.front_chapter_count();
    let body = project.body_chapter_count();
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = p
                .name
                .clone()
                .unwrap_or_else(|| format!("{label} {}", i + 1));
            let start = p.chapters[0].saturating_sub(1).min(body);
            let end = p.chapters[1].min(body).max(start);
            (name, (front + start)..(front + end))
        })
        .collect()
}

/// How many of a collection's stories can't be read for an export (#1245).
pub fn missing_stories(project: &Project) -> usize {
    if !project.is_collection() {
        return 0;
    }
    (0..project.manifest.stories.len())
        .filter(|&i| project.story_text(i).is_err())
        .count()
}

/// The book-level publishing metadata (#583) from the manifest: the OPF
/// `dc:*`, the copyright page, and the PDF/DOCX metadata.
pub fn publishing_meta(project: &Project) -> PubMeta {
    let m = &project.manifest;
    PubMeta {
        isbn: m.isbn.clone(),
        publisher: m.publisher.clone(),
        date: m.pub_date.clone(),
        lang: m.language.clone(),
        rights: m.rights.clone(),
    }
}

/// Book `b`'s own publishing metadata for its divider copyright in a series
/// export (#810), falling back to the series' when unset. Language is a
/// package-level concern, kept once on the manifest.
fn book_publishing_meta(project: &Project, b: usize) -> PubMeta {
    let pubmeta = project.book_publishing(b);
    PubMeta {
        isbn: pubmeta.isbn,
        publisher: pubmeta.publisher,
        date: pubmeta.pub_date,
        lang: project.manifest.language.clone(),
        rights: pubmeta.rights,
    }
}

/// The project's `[style]` with its preset applied and its fonts and images
/// read from the project folder (#483), plus a warning for anything that
/// couldn't be resolved, so a misconfigured style doesn't fail silently.
pub fn resolve_style(project: &Project) -> (ExportStyle, Vec<String>) {
    let mut style = project.manifest.style.clone().unwrap_or_default();
    let root = &project.root;
    let mut warnings: Vec<String> = style.apply_preset().into_iter().collect();
    warnings.extend(style.resolve_fonts(|path| std::fs::read(root.join(path))));
    warnings.extend(style.resolve_images(|path| std::fs::read(root.join(path))));
    warnings.extend(style.style_warnings());
    (style, warnings)
}

/// The print trim in points from the manifest's `[print]` table, with a warning
/// when the configured size is unknown (the default is used instead).
pub fn print_trim(project: &Project) -> ((f32, f32), Option<String>) {
    let name = project
        .manifest
        .print
        .as_ref()
        .and_then(|p| p.trim.as_deref())
        .unwrap_or(pdf::DEFAULT_TRIM);
    pdf::trim_points(name).map_or_else(
        || {
            // `DEFAULT_TRIM` is a known size; 6×9 is the fallback if it weren't.
            let default = pdf::trim_points(pdf::DEFAULT_TRIM).unwrap_or((6.0 * 72.0, 9.0 * 72.0));
            (
                default,
                Some(format!(
                    "print: unknown trim '{name}' (use {}) — using {}",
                    pdf::TRIM_NAMES.join(", "),
                    pdf::DEFAULT_TRIM
                )),
            )
        },
        |dims| (dims, None),
    )
}

/// The print bleed in points from the manifest's `[print]` table (0 when unset).
pub fn print_bleed(project: &Project) -> f32 {
    project
        .manifest
        .print
        .as_ref()
        .map_or(0.0, pdf::PrintConfig::bleed_points)
}

/// The paper caliper in inches per page (#819) from `[print] caliper`, or the
/// default. Sizes the wrap cover's spine.
pub fn print_caliper(project: &Project) -> f32 {
    project.manifest.print.as_ref().map_or(
        pdf::DEFAULT_CALIPER_INCHES,
        pdf::PrintConfig::caliper_inches,
    )
}

/// A format a compiled book renders to, shared by the desktop and the phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Word, in standard manuscript format, for an agent or an editor.
    Manuscript,
    /// An EPUB ebook, styled by `[style]`.
    Epub,
    /// A PDF to read on screen, styled by `[style]`.
    Pdf,
    /// A print interior PDF at the `[print]` trim and bleed.
    PrintPdf,
}

impl Format {
    /// The file extension the format is written with.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Manuscript => "docx",
            Self::Epub => "epub",
            Self::Pdf => "pdf",
            Self::PrintPdf => "print.pdf",
        }
    }
}

/// Render the compiled `doc` of `project` in `format`: the file's bytes, and
/// any warnings about the styling (an unreadable font, a low-resolution image,
/// an unknown trim) to show beside the result.
pub fn render(
    project: &Project,
    doc: &Document,
    format: Format,
) -> (std::io::Result<Vec<u8>>, Vec<String>) {
    match format {
        Format::Manuscript => (
            super::docx::to_docx(doc, super::docx::Profile::Manuscript),
            Vec::new(),
        ),
        Format::Epub => {
            let (style, warnings) = resolve_style(project);
            (
                super::epub::to_epub(doc, &crate::date::utc_timestamp(), &style),
                warnings,
            )
        }
        Format::Pdf => {
            let (style, mut warnings) = resolve_style(project);
            warnings.extend(pdf::pdf_style_warnings(&style));
            warnings.extend(pdf::pdf_dpi_warnings(&style, pdf::SCREEN_MEDIA_W));
            (pdf::to_pdf(doc, &style), warnings)
        }
        Format::PrintPdf => {
            let (style, mut warnings) = resolve_style(project);
            let ((w, h), trim_warning) = print_trim(project);
            warnings.extend(trim_warning);
            warnings.extend(pdf::pdf_style_warnings(&style));
            let bleed = print_bleed(project);
            warnings.extend(pdf::pdf_dpi_warnings(&style, 2.0f32.mul_add(bleed, w)));
            (pdf::to_pdf_print(doc, w, h, bleed, &style), warnings)
        }
    }
}

/// The chapters of the active book whose files can't be read, as
/// `(title, file)`, so an export can say what to fix rather than leave a
/// chapter silently empty.
pub fn unreadable_chapters(project: &Project) -> Vec<(String, String)> {
    (0..project.manifest.chapters.len())
        .filter(|&i| project.try_chapter_body(i).is_err() || !project.chapter_path(i).exists())
        .map(|i| {
            (
                project.chapter_title(i).unwrap_or("Untitled").to_string(),
                project.chapter_file(i).to_string(),
            )
        })
        .collect()
}

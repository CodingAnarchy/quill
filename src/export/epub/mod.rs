//! EPUB export (#297). An `.epub` is a ZIP with a fixed structure — the
//! `mimetype` entry (stored, uncompressed, first), `META-INF/container.xml`, an
//! OPF package (metadata + manifest + spine), an EPUB 3 navigation document plus
//! an EPUB 2 NCX for older readers, a stylesheet, and one XHTML file per content
//! item. We reuse the shared [`Document`] (#190) and the Markdown→emphasis and
//! block-classification helpers the other writers use, emitting XHTML here. The
//! result opens in Apple Books / Kobo / Calibre and is structured to pass
//! epubcheck.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use self::opf::{build_nav, build_ncx, build_opf, OpfMeta};
use super::pdf::font::subset_for_web;
use super::{
    copyright_lines, inline_runs, prose_blocks, render_epigraph, xml_escape, Chapter, Document,
    EmbeddedImage, ExportStyle, FontRole, ImageRole, Prose, PubMeta, Run,
};
use crate::frontmatter::Epigraph;

mod opf;
#[cfg(test)]
mod tests;

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// Minimal reading stylesheet: centered titles, indented body paragraphs (the
/// first after a heading flush-left), set-apart epigraphs, and a centered scene
/// break. Readers layer their own typography on top.
const STYLE_CSS: &str = r#"body { margin: 5%; line-height: 1.5; }
h1, h2 { text-align: center; font-weight: bold; }
h1 { margin: 2em 0 1em; }
p { margin: 0; text-indent: 1.5em; text-align: justify; -webkit-hyphens: auto; hyphens: auto; orphans: 2; widows: 2; }
h1 + p, h2 + p, .epigraph p, .title-page p, p.scene-break { text-indent: 0; }
.title-page { text-align: center; margin-top: 20%; }
.title-page .title { font-size: 2em; font-weight: bold; margin: 0.5em 0; }
.title-page .subtitle { font-style: italic; }
.title-page .series { letter-spacing: 0.15em; text-transform: uppercase; }
.half-title-page { text-align: center; margin-top: 30%; }
.half-title-page .title { font-size: 1.6em; font-weight: bold; }
.series-list-page { text-align: center; margin-top: 20%; }
.series-list-page .heading { font-size: 1.3em; font-weight: bold; margin-bottom: 1em; }
.series-list-page .book { font-style: italic; margin: 0.4em 0; text-indent: 0; }
.copyright-page { text-align: center; margin-top: 30%; font-size: 0.9em; }
.copyright-page p { text-indent: 0; margin: 0.4em 0; }
.epigraph { margin: 2em 10%; font-style: italic; }
.epigraph p { text-align: initial; hyphens: none; }
.epigraph .attribution { text-align: right; }
.scene-break { text-align: center; margin: 1.5em 0; }
pre { font-family: monospace; white-space: pre-wrap; margin: 1em 0; text-indent: 0; }
"#;

/// Compile a [`Document`] to the bytes of an `.epub`. `modified` is the
/// `dcterms:modified` UTC timestamp (`YYYY-MM-DDThh:mm:ssZ`); callers pass
/// [`crate::date::utc_timestamp`].
pub fn to_epub(doc: &Document, modified: &str, style: &ExportStyle) -> std::io::Result<Vec<u8>> {
    let mut b = Builder::new(style, &doc.publishing);
    b.add_cover_page(&doc.title);
    // The half-title (#820) is the leaf ahead of the full title page, when enabled.
    if doc.half_title {
        b.add_half_title_page(&doc.title);
    }
    b.add_title_page(
        doc.series.as_deref(),
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.epigraph.as_ref(),
    );
    b.add_copyright_page();
    // The "also in this series" page (#820), when this is a series title's book.
    b.add_series_list_page(&doc.series_list_lines());
    let nav = b.book_sections(doc);
    b.finish(
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.series.as_deref(),
        modified,
        &nav,
    )
}

/// Compile a whole series (one book per [`Document`], in order) to one `.epub`
/// (#332): a series title page, then each book as a divider page with its
/// parts/chapters nested beneath it in the table of contents.
///
/// # Series publishing metadata (#711)
///
/// An omnibus is one publication, so the *package-level* metadata is
/// series-level: the OPF `dc:identifier` (one ISBN for the omnibus, if any),
/// `dc:language`, and the single OPF-level copyright page come from the first
/// book's [`PubMeta`]. That is deliberate — an omnibus legitimately carries one
/// ISBN and one package language, not one per contained book.
///
/// Per-book rights, publisher, publication date, and ISBN are *not* dropped: each
/// book renders its own copyright block on its divider page (via
/// [`Builder::add_book_divider`]) when its metadata warrants one, so a
/// three-book omnibus reprints all three copyright notices in reading order —
/// mirroring how print omnibuses reproduce each original edition's copyright.
pub fn to_epub_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    modified: &str,
    style: &ExportStyle,
) -> std::io::Result<Vec<u8>> {
    let mut b = Builder::new(
        style,
        // Package-level metadata (the OPF identifier/language and the
        // omnibus-level copyright page) is series-level — the first book's
        // (#711). Each book's own copyright rides its divider page below, so no
        // later book's rights/ISBN are silently dropped.
        &books
            .first()
            .map(|d| d.publishing.clone())
            .unwrap_or_default(),
    );
    let title = series.unwrap_or("Series");
    b.add_cover_page(title);
    // A half-title ahead of the omnibus title page, when the first book enables it
    // (#820) — the omnibus's own front matter, so it carries no series-list page.
    if books.first().is_some_and(|d| d.half_title) {
        b.add_half_title_page(title);
    }
    b.add_title_page(series, title, None, author, None);
    b.add_copyright_page();
    let mut nav = Vec::new();
    for book in books {
        let href = b.add_book_divider(book);
        let children = b.book_sections(book);
        nav.push(Nav {
            label: book.title.clone(),
            href,
            children,
        });
    }
    b.finish(title, None, author, series, modified, &nav)
}

/// One content document in the package — a manifest/spine entry and its XHTML.
struct Item {
    id: String,
    href: String,
    xhtml: String,
}

/// A table-of-contents entry: a label, the file it links to, and any nested
/// entries (chapters under a part, parts/chapters under a series book).
struct Nav {
    label: String,
    href: String,
    children: Vec<Self>,
}

/// Accumulates the package's content files (in spine order) as sections are
/// added, then [`Builder::finish`] renders the OPF/nav/NCX and zips it all.
struct Builder {
    seq: usize,
    items: Vec<Item>,
    /// The styling that shapes the rendered XHTML (scene-break ornament, …).
    style: ExportStyle,
    /// Publishing metadata (#583): drives the OPF `dc:*`, the per-document
    /// `xml:lang`/`lang`, and the copyright page.
    publishing: PubMeta,
    /// The landmark hrefs (#604), captured as the front matter and the first body
    /// section are added, so [`Builder::finish`] can build the `landmarks` nav.
    landmarks: Landmarks,
}

/// The `landmarks` nav targets (#604): the cover page, the title page, and the
/// first body-matter section ("Begin Reading"). Each is captured the first time
/// its section is added; the `toc` landmark points at the nav doc itself.
#[derive(Default)]
struct Landmarks {
    cover: Option<String>,
    titlepage: Option<String>,
    bodymatter: Option<String>,
    /// Role-specific reference points (#695): a front/back-matter section whose
    /// role carries a distinct `epub:type` (a foreword, dedication, glossary, …)
    /// becomes its own landmark so screen readers can jump straight to it. In
    /// reading order, first occurrence wins per type.
    sections: Vec<RoleLandmark>,
}

/// One role-specific landmark (#695): the section's `epub:type`, a human label,
/// and the href of its content document.
struct RoleLandmark {
    epub_type: &'static str,
    label: String,
    href: String,
}

impl Builder {
    fn new(style: &ExportStyle, publishing: &PubMeta) -> Self {
        Self {
            seq: 0,
            items: Vec::new(),
            style: style.clone(),
            publishing: publishing.clone(),
            landmarks: Landmarks::default(),
        }
    }

    /// Add a content document (XHTML `body`), returning its href. Files are added
    /// in reading order, so the items list doubles as the spine order. `epub_type`
    /// tags the `<body>` with EPUB 3 structural semantics (#604) — `cover`,
    /// `titlepage`, `frontmatter`, `bodymatter`, `backmatter`.
    fn add(&mut self, prefix: &str, head_title: &str, epub_type: &str, body: &str) -> String {
        self.seq += 1;
        let id = format!("{prefix}{:04}", self.seq);
        let href = format!("{id}.xhtml");
        let xhtml = xhtml_doc(head_title, epub_type, body, self.publishing.language());
        self.items.push(Item {
            id,
            href: href.clone(),
            xhtml,
        });
        href
    }

    /// The cover page (#582), when a cover image is configured: a single full-page
    /// `<img>` made the first spine entry so storefronts and readers show it on
    /// open. Not a TOC entry. No-op when no cover is set, leaving the package
    /// byte-for-byte unchanged.
    ///
    /// The cover's `alt` (#818) is the author-supplied description, falling back to
    /// the book `title` — a cover is content, not decoration, so it always carries
    /// a meaningful description rather than the old generic `alt="Cover"`.
    fn add_cover_page(&mut self, title: &str) {
        let Some(img) = self.style.image(ImageRole::Cover) else {
            return;
        };
        let alt = img.alt.as_deref().unwrap_or(title);
        let body = format!(
            "<div class=\"cover\"><img src=\"{}\" alt=\"{}\"/></div>\n",
            img.href,
            xml_escape(alt)
        );
        let href = self.add("cover", "Cover", "cover", &body);
        self.landmarks.cover = Some(href);
    }

    /// The title page (not itself a TOC entry): series, title, subtitle, author,
    /// and the book epigraph, centered.
    fn add_title_page(
        &mut self,
        series: Option<&str>,
        title: &str,
        subtitle: Option<&str>,
        author: Option<&str>,
        epigraph: Option<&Epigraph>,
    ) {
        let mut body = String::from("<div class=\"title-page\">\n");
        if let Some(img) = self.style.image(ImageRole::Title) {
            push_img(&mut body, "title-image", img);
        }
        if let Some(s) = series {
            push_p(&mut body, "series", s);
        }
        push_p(&mut body, "title", title);
        if let Some(s) = subtitle {
            push_p(&mut body, "subtitle", s);
        }
        if let Some(a) = author {
            push_p(&mut body, "author", &format!("by {a}"));
        }
        if let Some(o) = &self.style.title_ornament {
            push_p(&mut body, "ornament", o);
        }
        body.push_str("</div>\n");
        if let Some(e) = epigraph {
            body.push_str(&epigraph_xhtml(e));
        }
        let href = self.add("title", title, "titlepage", &body);
        self.landmarks.titlepage = Some(href);
    }

    /// The half-title page (#820): the book title alone, ahead of the full title
    /// page — the conventional first printed leaf of a trade book. Tagged with the
    /// EPUB `halftitlepage` structural term. Not a TOC entry.
    fn add_half_title_page(&mut self, title: &str) {
        let mut body = String::from("<div class=\"half-title-page\">\n");
        push_p(&mut body, "title", title);
        body.push_str("</div>\n");
        self.add("halftitle", title, "halftitlepage", &body);
    }

    /// The "also in this series" page (#820), when a series title's book supplies
    /// the list: the heading, then each *other* book of the series by title,
    /// centered. `lines` is [`Document::series_list_lines`] — the heading followed
    /// by the book titles — so an empty slice (a standalone book, or the page
    /// disabled) is a no-op that leaves the package unchanged. Slots into the front
    /// matter after the copyright page; not a TOC entry.
    fn add_series_list_page(&mut self, lines: &[String]) {
        let Some((heading, books)) = lines.split_first() else {
            return;
        };
        let mut body = String::from("<div class=\"series-list-page\">\n");
        push_p(&mut body, "heading", heading);
        for book in books {
            push_p(&mut body, "book", book);
        }
        body.push_str("</div>\n");
        self.add("series", heading, "frontmatter", &body);
    }

    /// The copyright page (#583), when publishing metadata warrants one: the
    /// rights line, publisher, publication date, and ISBN, each centered. Slots
    /// into the conventional front-matter order after the title page (cover →
    /// title → copyright → body). Not a TOC entry. No-op when no metadata is set,
    /// leaving the package unchanged.
    fn add_copyright_page(&mut self) {
        let lines = copyright_lines(&self.publishing);
        if lines.is_empty() {
            return;
        }
        let mut body = String::from("<div class=\"copyright-page\">\n");
        for line in lines {
            push_p(&mut body, "copyright", &line);
        }
        body.push_str("</div>\n");
        self.add("copyright", "Copyright", "copyright-page", &body);
    }

    /// A series book's divider page: its title, subtitle, epigraph, and — so a
    /// series export never silently drops a later book's rights/ISBN (#711) — the
    /// book's own copyright block when its [`PubMeta`] warrants one. The omnibus's
    /// package-level identifier/language stays series-level (the first book's; see
    /// [`to_epub_series`]); this reprints each contained book's copyright notice in
    /// reading order, the way a print omnibus does. Body matter (#604) — the first
    /// divider is the "Begin Reading" landmark.
    fn add_book_divider(&mut self, doc: &Document) -> String {
        let mut body = heading_with_ornament("book-title", &doc.title, None);
        if let Some(s) = &doc.subtitle {
            push_p(&mut body, "subtitle", s);
        }
        if let Some(e) = &doc.epigraph {
            body.push_str(&epigraph_xhtml(e));
        }
        let copyright = copyright_lines(&doc.publishing);
        if !copyright.is_empty() {
            body.push_str("<div class=\"copyright-page book-copyright\">\n");
            for line in copyright {
                push_p(&mut body, "copyright", &line);
            }
            body.push_str("</div>\n");
        }
        let href = self.add("book", &doc.title, "bodymatter", &body);
        self.landmarks
            .bodymatter
            .get_or_insert_with(|| href.clone());
        href
    }

    /// One chapter or section as its own XHTML document, returning its TOC entry.
    /// `band_type` is the generic structural role of the band (#604):
    /// `frontmatter` for front chapters, `bodymatter` for the body, `backmatter`
    /// for the back. A front/back-matter section whose [`Role`](crate::frontmatter::Role) carries a distinct
    /// `epub:type` (#695) — a foreword, dedication, glossary, … — is tagged with
    /// that precise term instead and becomes its own landmark; the first
    /// body-matter section becomes the "Begin Reading" landmark.
    fn add_chapter(&mut self, ch: &Chapter, band_type: &str) -> Nav {
        // The section's role gives the precise EPUB term; an untyped (body) section
        // — or a role with no distinct term — keeps the generic band semantic.
        let role_type = ch.role.and_then(crate::frontmatter::Role::epub_type);
        let epub_type = role_type.unwrap_or(band_type);
        let href = self.add("chap", &ch.title, epub_type, &chapter_body(ch, &self.style));
        if band_type == "bodymatter" {
            self.landmarks
                .bodymatter
                .get_or_insert_with(|| href.clone());
        }
        if let Some(role_type) = role_type {
            self.landmarks.sections.push(RoleLandmark {
                epub_type: role_type,
                label: ch.title.clone(),
                href: href.clone(),
            });
        }
        Nav {
            label: ch.title.clone(),
            href,
            children: Vec::new(),
        }
    }

    /// A book's sections as TOC entries in reading order: front matter, then each
    /// part (a named part becomes a divider page with its chapters nested; an
    /// unnamed part's chapters sit at the top level), then back matter.
    fn book_sections(&mut self, doc: &Document) -> Vec<Nav> {
        let mut nav = Vec::new();
        for ch in &doc.front {
            nav.push(self.add_chapter(ch, "frontmatter"));
        }
        for part in &doc.parts {
            match &part.name {
                Some(name) => {
                    let body = heading_with_ornament(
                        "part-title",
                        name,
                        self.style.part_ornament.as_deref(),
                    );
                    // The part divider begins the body matter (#604) — a candidate
                    // "Begin Reading" landmark ahead of its chapters.
                    let href = self.add("part", name, "bodymatter", &body);
                    self.landmarks
                        .bodymatter
                        .get_or_insert_with(|| href.clone());
                    let children = part
                        .chapters
                        .iter()
                        .map(|c| self.add_chapter(c, "bodymatter"))
                        .collect();
                    nav.push(Nav {
                        label: name.clone(),
                        href,
                        children,
                    });
                }
                None => {
                    for ch in &part.chapters {
                        nav.push(self.add_chapter(ch, "bodymatter"));
                    }
                }
            }
        }
        for ch in &doc.back {
            nav.push(self.add_chapter(ch, "backmatter"));
        }
        nav
    }

    /// Every character the book renders (#709), for subsetting the embedded fonts:
    /// the content XHTML, the TOC labels (recursively), and the title-block
    /// metadata, seeded with printable ASCII so markup, punctuation, and the
    /// XML-escaped `& < >` are always covered. The XHTML holds non-ASCII text
    /// literally (only `& < >` are escaped, and those are ASCII), so gathering its
    /// chars is an exact-or-superset of the glyphs the reader will shape.
    fn used_chars(
        &self,
        nav: &[Nav],
        title: &str,
        subtitle: Option<&str>,
        author: Option<&str>,
        series: Option<&str>,
    ) -> BTreeSet<char> {
        fn walk(nav: &[Nav], used: &mut BTreeSet<char>) {
            for entry in nav {
                used.extend(entry.label.chars());
                walk(&entry.children, used);
            }
        }
        let mut used: BTreeSet<char> = (' '..='~').collect();
        for item in &self.items {
            used.extend(item.xhtml.chars());
        }
        for meta in [Some(title), subtitle, author, series]
            .into_iter()
            .flatten()
        {
            used.extend(meta.chars());
        }
        walk(nav, &mut used);
        used
    }

    /// Render the OPF, nav doc, and NCX from the accumulated items + TOC, then zip
    /// the package (mimetype first and stored, the rest deflated).
    fn finish(
        self,
        title: &str,
        subtitle: Option<&str>,
        author: Option<&str>,
        series: Option<&str>,
        modified: &str,
        nav: &[Nav],
    ) -> std::io::Result<Vec<u8>> {
        // A real ISBN is the `dc:identifier` when set (#583); otherwise fall back
        // to the stable minted `urn:quill:` slug so re-exports keep their id.
        let id = match &self.publishing.isbn {
            Some(isbn) if !isbn.trim().is_empty() => isbn.trim().to_string(),
            _ => identifier(title, series),
        };
        let meta = OpfMeta {
            id: &id,
            title,
            subtitle,
            author,
            series,
            modified,
            publishing: &self.publishing,
        };
        // The glyphs the book actually renders (#709): every char across the content
        // XHTML, the TOC labels, and the title-block metadata, plus printable ASCII
        // as a floor so markup/punctuation and the entity-escaped `& < >` are always
        // covered. Non-ASCII text passes through XML escaping literally, so gathering
        // it here is exact. Used to subset the embedded fonts before they're zipped.
        let used = self.used_chars(nav, title, subtitle, author, series);

        let opf = build_opf(&meta, &self.items, &self.style.fonts, &self.style.images);
        let nav_doc = build_nav(nav, &self.landmarks, self.publishing.language());
        let ncx = build_ncx(&id, title, nav);

        let mut parts: Vec<(String, Vec<u8>)> = vec![
            (
                "META-INF/container.xml".into(),
                CONTAINER.as_bytes().to_vec(),
            ),
            ("OEBPS/content.opf".into(), opf.into_bytes()),
            ("OEBPS/nav.xhtml".into(), nav_doc.into_bytes()),
            ("OEBPS/toc.ncx".into(), ncx.into_bytes()),
            (
                "OEBPS/style.css".into(),
                stylesheet(&self.style).into_bytes(),
            ),
        ];
        // Embedded fonts (#469) and images (#472), referenced by the stylesheet /
        // the `<img>` tags. `finish` owns `self`, so the (potentially large) bytes
        // move, not copy. Each static font is subset to just the used glyphs (#709)
        // — a full serif family is megabytes verbatim; a variable font or a
        // `.woff`/`.woff2` (already compact/compressed) stays verbatim.
        for font in self.style.fonts {
            let bytes = subset_for_web(&font.bytes, &used).unwrap_or(font.bytes);
            parts.push((format!("OEBPS/{}", font.href), bytes));
        }
        for image in self.style.images {
            parts.push((format!("OEBPS/{}", image.href), image.bytes));
        }
        for item in self.items {
            parts.push((format!("OEBPS/{}", item.href), item.xhtml.into_bytes()));
        }

        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        // The mimetype must be the first entry and stored uncompressed (OCF).
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", stored)
            .map_err(std::io::Error::other)?;
        zip.write_all(b"application/epub+zip")?;
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (path, bytes) in parts {
            zip.start_file(path, deflated)
                .map_err(std::io::Error::other)?;
            zip.write_all(&bytes)?;
        }
        Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
    }
}

/// The package stylesheet: the base rules plus any embedded-font `@font-face`
/// declarations and the body/heading `font-family` overrides (#469). With no
/// fonts configured this is exactly [`STYLE_CSS`].
fn stylesheet(style: &ExportStyle) -> String {
    let mut css = String::from(STYLE_CSS);
    // One `@font-face` per embedded file (#638), carrying the `font-style` and a
    // `font-weight` range so the reader picks the upright file for regular/bold and
    // the true-italic file for italic/bold-italic — and a variable `[wght]` file
    // supplies both the normal and bold weights from its axis. `<em>`/`<strong>`
    // then resolve to the right face instead of the reader faux-styling one weight.
    for font in &style.fonts {
        // A `format(...)` hint (#709) lets a reader that can't handle the file's
        // format skip it rather than fetch an unusable face; omitted only for a
        // media type with no standard token.
        let format = font
            .css_format()
            .map(|f| format!(" format(\"{f}\")"))
            .unwrap_or_default();
        let _ = writeln!(
            css,
            "@font-face {{ font-family: \"{}\"; font-style: {}; font-weight: 400 700; src: url(\"{}\"){format}; }}",
            font.family,
            if font.italic { "italic" } else { "normal" },
            font.href
        );
    }
    if let Some(f) = style.font(FontRole::Body) {
        let _ = writeln!(css, "body {{ font-family: \"{}\", serif; }}", f.family);
    }
    if let Some(f) = style.font(FontRole::Heading) {
        let _ = writeln!(css, "h1, h2 {{ font-family: \"{}\", serif; }}", f.family);
    }
    if let Some(align) = style.heading_align() {
        let _ = writeln!(css, "h1, h2 {{ text-align: {align}; }}");
    }
    if style.drop_cap() {
        // The first paragraph of each chapter (marked `opening`) gets a drop cap;
        // its first line is flush so the initial sits in the margin.
        css.push_str("p.opening { text-indent: 0; }\n");
        css.push_str(".opening::first-letter { float: left; font-size: 3.2em; line-height: 0.72; padding-right: 0.06em; font-weight: bold; }\n");
    }
    if style.title_ornament.is_some() || style.part_ornament.is_some() {
        css.push_str(".ornament { text-align: center; text-indent: 0; margin: 1.5em 0; font-size: 1.5em; }\n");
    }
    if !style.images.is_empty() {
        // Centered, never wider than the page; the scene-break image reuses the
        // scene-break paragraph's centering.
        css.push_str(
            ".chapter-image, .title-image { text-align: center; text-indent: 0; margin: 1em 0; }\n",
        );
        css.push_str(".scene-break img, .chapter-image img, .title-image img { max-width: 100%; height: auto; }\n");
    }
    if style.image(ImageRole::Cover).is_some() {
        // The cover fills its page: no margins, the image scaled to fit the viewport
        // (#582).
        css.push_str(".cover { margin: 0; padding: 0; text-align: center; }\n");
        css.push_str(".cover img { max-width: 100%; max-height: 100%; height: auto; }\n");
    }
    css
}

/// Wrap an XHTML `body` in the document shell (XHTML + EPUB namespaces, the
/// shared stylesheet, an escaped `<title>`). `epub_type` tags the `<body>` with
/// the EPUB 3 structural semantics (#604) — `cover`, `titlepage`, `frontmatter`,
/// `bodymatter`, `backmatter` — so readers and the `landmarks` nav can classify
/// the section.
fn xhtml_doc(title: &str, epub_type: &str, body: &str, lang: &str) -> String {
    let lang = xml_escape(lang);
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE html>\n",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" ",
            "xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{lang}\" lang=\"{lang}\">",
            "<head><meta charset=\"utf-8\"/><title>{title}</title>",
            "<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head>",
            "<body epub:type=\"{epub_type}\">\n{body}</body></html>"
        ),
        lang = lang,
        title = xml_escape(title),
        epub_type = xml_escape(epub_type),
        body = body,
    )
}

/// A chapter's XHTML: an optional chapter-heading image (#472), its title as
/// `<h1>`, the epigraph set apart, then the prose blocks (scene breaks, inner
/// headings, and emphasis-bearing paragraphs).
fn chapter_body(ch: &Chapter, style: &ExportStyle) -> String {
    let mut out = String::new();
    if let Some(img) = style.image(ImageRole::ChapterHeading) {
        push_img(&mut out, "chapter-image", img);
    }
    let _ = writeln!(out, "<h1>{}</h1>", xml_escape(&ch.title));
    if let Some(e) = &ch.epigraph {
        out.push_str(&epigraph_xhtml(e));
    }
    // With drop caps on, the chapter's first prose paragraph is marked `opening`.
    let mut opened = false;
    for prose in prose_blocks(&ch.body) {
        let opening = style.drop_cap() && !opened && matches!(prose, Prose::Paragraph(_));
        opened |= opening;
        out.push_str(&prose_xhtml(&prose, style, opening));
        out.push('\n');
    }
    out
}

/// One classified body block → XHTML: a scene break becomes a centered divider
/// (the styled glyph, default `#`), a heading line an `<h2>`, prose a `<p>` with
/// `*`/`**` emphasis as `<em>`/`<strong>`. The chapter's opening paragraph carries
/// the `opening` class when drop caps are enabled.
fn prose_xhtml(prose: &Prose, style: &ExportStyle, opening: bool) -> String {
    match prose {
        Prose::SceneBreak => style.image(ImageRole::SceneBreak).map_or_else(
            || p_class("scene-break", style.scene_break()),
            |img| img_p("scene-break", img),
        ),
        Prose::Heading(h) => format!("<h2>{}</h2>", xml_escape(h)),
        Prose::Paragraph(text) => {
            let class = if opening { " class=\"opening\"" } else { "" };
            format!("<p{class}>{}</p>", inline_xhtml(text))
        }
        // Verbatim, monospaced, whitespace preserved — no emphasis parsing (#522).
        Prose::Preformatted(code) => format!("<pre>{}</pre>", xml_escape(code)),
    }
}

/// An epigraph as a set-apart block: each line a `<p>`, the attribution on its
/// own line. Text is taken literally (the whole block is italicized by the
/// stylesheet), matching the other writers' epigraph fidelity.
fn epigraph_xhtml(e: &Epigraph) -> String {
    let mut out = String::from("<div class=\"epigraph\">\n");
    out.push_str(&render_epigraph(
        e,
        // A class-less `<p>`, so not via `push_p`.
        |line| format!("<p>{}</p>\n", xml_escape(line)),
        |a| format!("{}\n", p_class("attribution", &format!("— {a}"))),
    ));
    out.push_str("</div>\n");
    out
}

/// A styled `<h1 class="…">` heading (text XML-escaped) followed, when one is
/// configured, by a centered ornament paragraph — the divider idiom the book and
/// part dividers share. Returns the `body` fragment (heading + trailing newline,
/// then any ornament via [`push_p`]).
fn heading_with_ornament(class: &str, text: &str, ornament: Option<&str>) -> String {
    let mut body = format!("<h1 class=\"{class}\">{}</h1>\n", xml_escape(text));
    if let Some(o) = ornament {
        push_p(&mut body, "ornament", o);
    }
    body
}

/// `<p class="…">…</p>` with the text XML-escaped — the styled-paragraph idiom the
/// designed pages share (title, ornaments, scene break, …).
fn p_class(class: &str, text: &str) -> String {
    format!("<p class=\"{class}\">{}</p>", xml_escape(text))
}

/// A placed image (#472): a centered `<p>` wrapping an `<img>`. The `alt`
/// attribute follows the author's intent (#818) via [`img_attrs`] — a real
/// description for a content image, or the empty-alt + `role="presentation"`
/// decorative convention when none is supplied. The href is package-internal, so
/// it isn't escaped.
fn img_p(class: &str, img: &EmbeddedImage) -> String {
    format!(
        "<p class=\"{class}\"><img src=\"{}\" {}/></p>",
        img.href,
        img_attrs(img.alt.as_deref())
    )
}

/// The accessibility attributes for an `<img>` given its author-supplied alt text
/// (#818). `Some(text)` → a described content image (`alt="…"`, XML-escaped).
/// `None` → the *decorative* convention: an empty `alt` paired with
/// `role="presentation"`, which declares the image ornamental (ACE/epubcheck-clean)
/// rather than leaving a bare empty `alt` on what a reader would treat as content.
fn img_attrs(alt: Option<&str>) -> String {
    alt.map_or_else(
        || String::from("alt=\"\" role=\"presentation\""),
        |text| format!("alt=\"{}\"", xml_escape(text)),
    )
}

/// Append an [`img_p`] paragraph and a trailing newline to `body` — the
/// title-page / chapter builders' convention (parallel to [`push_p`]).
fn push_img(body: &mut String, class: &str, img: &EmbeddedImage) {
    body.push_str(&img_p(class, img));
    body.push('\n');
}

/// Append a [`p_class`] paragraph and a trailing newline to `body` — the
/// title-page / divider builders' convention.
fn push_p(body: &mut String, class: &str, text: &str) {
    body.push_str(&p_class(class, text));
    body.push('\n');
}

/// Render `*`/`**` runs as `<em>`/`<strong>`, escaping the text of each run.
fn inline_xhtml(text: &str) -> String {
    let mut out = String::new();
    for Run { text, bold, italic } in inline_runs(text) {
        let esc = xml_escape(&text);
        match (bold, italic) {
            (true, true) => {
                let _ = write!(out, "<strong><em>{esc}</em></strong>");
            }
            (true, false) => {
                let _ = write!(out, "<strong>{esc}</strong>");
            }
            (false, true) => {
                let _ = write!(out, "<em>{esc}</em>");
            }
            (false, false) => out.push_str(&esc),
        }
    }
    out
}

/// A stable, reader-dedupable identifier derived from the book/series title, so
/// re-exporting the same book keeps the same id (not a random UUID per export). A
/// local ASCII slug — the parallel `ai::bible::slug` lives in a higher layer
/// `export` can't depend on; the `book` fallback keeps the id non-empty.
fn identifier(title: &str, series: Option<&str>) -> String {
    let basis = format!("{}{}", series.unwrap_or(""), title);
    let collapsed: String = basis
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = collapsed
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!(
        "urn:quill:{}",
        if slug.is_empty() {
            "book"
        } else {
            slug.as_str()
        }
    )
}

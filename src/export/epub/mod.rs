//! EPUB export (#297). An `.epub` is a ZIP with a fixed structure — the
//! `mimetype` entry (stored, uncompressed, first), `META-INF/container.xml`, an
//! OPF package (metadata + manifest + spine), an EPUB 3 navigation document plus
//! an EPUB 2 NCX for older readers, a stylesheet, and one XHTML file per content
//! item. We reuse the shared [`Document`] (#190) and the Markdown→emphasis and
//! block-classification helpers the other writers use, emitting XHTML here. The
//! result opens in Apple Books / Kobo / Calibre and is structured to pass
//! epubcheck.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use self::opf::{build_nav, build_ncx, build_opf, OpfMeta};
use super::{
    inline_runs, prose_blocks, render_epigraph, xml_escape, Chapter, Document, EmbeddedImage,
    ExportStyle, FontRole, ImageRole, Prose, Run,
};
use crate::frontmatter::Epigraph;

mod opf;

const CONTAINER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;

/// Minimal reading stylesheet: centered titles, indented body paragraphs (the
/// first after a heading flush-left), set-apart epigraphs, and a centered scene
/// break. Readers layer their own typography on top.
const STYLE_CSS: &str = r#"body { margin: 5%; line-height: 1.5; }
h1, h2 { text-align: center; font-weight: bold; }
h1 { margin: 2em 0 1em; }
p { margin: 0; text-indent: 1.5em; }
h1 + p, h2 + p, .epigraph p, .title-page p, p.scene-break { text-indent: 0; }
.title-page { text-align: center; margin-top: 20%; }
.title-page .title { font-size: 2em; font-weight: bold; margin: 0.5em 0; }
.title-page .subtitle { font-style: italic; }
.title-page .series { letter-spacing: 0.15em; text-transform: uppercase; }
.epigraph { margin: 2em 10%; font-style: italic; }
.epigraph .attribution { text-align: right; }
.scene-break { text-align: center; margin: 1.5em 0; }
pre { font-family: monospace; white-space: pre-wrap; margin: 1em 0; text-indent: 0; }
"#;

/// Compile a [`Document`] to the bytes of an `.epub`. `modified` is the
/// `dcterms:modified` UTC timestamp (`YYYY-MM-DDThh:mm:ssZ`); callers pass
/// [`crate::date::utc_timestamp`].
pub fn to_epub(doc: &Document, modified: &str, style: &ExportStyle) -> std::io::Result<Vec<u8>> {
    let mut b = Builder::new(style);
    b.add_title_page(
        doc.series.as_deref(),
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.epigraph.as_ref(),
    );
    let nav = b.book_sections(doc);
    b.finish(
        &doc.title,
        doc.subtitle.as_deref(),
        doc.author.as_deref(),
        doc.series.as_deref(),
        modified,
        nav,
    )
}

/// Compile a whole series (one book per [`Document`], in order) to one `.epub`
/// (#332): a series title page, then each book as a divider page with its
/// parts/chapters nested beneath it in the table of contents.
pub fn to_epub_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
    modified: &str,
    style: &ExportStyle,
) -> std::io::Result<Vec<u8>> {
    let mut b = Builder::new(style);
    let title = series.unwrap_or("Series");
    b.add_title_page(series, title, None, author, None);
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
    b.finish(title, None, author, series, modified, nav)
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
    children: Vec<Nav>,
}

/// Accumulates the package's content files (in spine order) as sections are
/// added, then [`Builder::finish`] renders the OPF/nav/NCX and zips it all.
struct Builder {
    seq: usize,
    items: Vec<Item>,
    /// The styling that shapes the rendered XHTML (scene-break ornament, …).
    style: ExportStyle,
}

impl Builder {
    fn new(style: &ExportStyle) -> Self {
        Builder {
            seq: 0,
            items: Vec::new(),
            style: style.clone(),
        }
    }

    /// Add a content document (XHTML `body`), returning its href. Files are added
    /// in reading order, so the items list doubles as the spine order.
    fn add(&mut self, prefix: &str, head_title: &str, body: String) -> String {
        self.seq += 1;
        let id = format!("{prefix}{:04}", self.seq);
        let href = format!("{id}.xhtml");
        let xhtml = xhtml_doc(head_title, &body);
        self.items.push(Item {
            id,
            href: href.clone(),
            xhtml,
        });
        href
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
        self.add("title", title, body);
    }

    /// A series book's divider page: its title, subtitle, and epigraph.
    fn add_book_divider(&mut self, doc: &Document) -> String {
        let mut body = heading_with_ornament("book-title", &doc.title, None);
        if let Some(s) = &doc.subtitle {
            push_p(&mut body, "subtitle", s);
        }
        if let Some(e) = &doc.epigraph {
            body.push_str(&epigraph_xhtml(e));
        }
        self.add("book", &doc.title, body)
    }

    /// One chapter or section as its own XHTML document, returning its TOC entry.
    fn add_chapter(&mut self, ch: &Chapter) -> Nav {
        let href = self.add("chap", &ch.title, chapter_body(ch, &self.style));
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
            nav.push(self.add_chapter(ch));
        }
        for part in &doc.parts {
            match &part.name {
                Some(name) => {
                    let body = heading_with_ornament(
                        "part-title",
                        name,
                        self.style.part_ornament.as_deref(),
                    );
                    let href = self.add("part", name, body);
                    let children = part.chapters.iter().map(|c| self.add_chapter(c)).collect();
                    nav.push(Nav {
                        label: name.clone(),
                        href,
                        children,
                    });
                }
                None => {
                    for ch in &part.chapters {
                        nav.push(self.add_chapter(ch));
                    }
                }
            }
        }
        for ch in &doc.back {
            nav.push(self.add_chapter(ch));
        }
        nav
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
        nav: Vec<Nav>,
    ) -> std::io::Result<Vec<u8>> {
        let id = identifier(title, series);
        let meta = OpfMeta {
            id: &id,
            title,
            subtitle,
            author,
            series,
            modified,
        };
        let opf = build_opf(&meta, &self.items, &self.style.fonts, &self.style.images);
        let nav_doc = build_nav(&nav);
        let ncx = build_ncx(&id, title, &nav);

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
        // move, not copy.
        for font in self.style.fonts {
            parts.push((format!("OEBPS/{}", font.href), font.bytes));
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
    for font in &style.fonts {
        css.push_str(&format!(
            "@font-face {{ font-family: \"{}\"; src: url(\"{}\"); }}\n",
            font.family, font.href
        ));
    }
    if let Some(f) = style.font(FontRole::Body) {
        css.push_str(&format!(
            "body {{ font-family: \"{}\", serif; }}\n",
            f.family
        ));
    }
    if let Some(f) = style.font(FontRole::Heading) {
        css.push_str(&format!(
            "h1, h2 {{ font-family: \"{}\", serif; }}\n",
            f.family
        ));
    }
    if let Some(align) = style.heading_align() {
        css.push_str(&format!("h1, h2 {{ text-align: {align}; }}\n"));
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
    css
}

/// Wrap an XHTML `body` in the document shell (XHTML namespace, the shared
/// stylesheet, an escaped `<title>`).
fn xhtml_doc(title: &str, body: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE html>\n",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\" lang=\"en\">",
            "<head><meta charset=\"utf-8\"/><title>{title}</title>",
            "<link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head>",
            "<body>\n{body}</body></html>"
        ),
        title = xml_escape(title),
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
    out.push_str(&format!("<h1>{}</h1>\n", xml_escape(&ch.title)));
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
        Prose::SceneBreak => match style.image(ImageRole::SceneBreak) {
            Some(img) => img_p("scene-break", img),
            None => p_class("scene-break", style.scene_break()),
        },
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

/// A placed ornament image (#472): a centered `<p>` wrapping a decorative `<img>`
/// (empty `alt`). The href is package-internal, so it isn't escaped.
fn img_p(class: &str, img: &EmbeddedImage) -> String {
    format!(
        "<p class=\"{class}\"><img src=\"{}\" alt=\"\"/></p>",
        img.href
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
        out.push_str(&match (bold, italic) {
            (true, true) => format!("<strong><em>{esc}</em></strong>"),
            (true, false) => format!("<strong>{esc}</strong>"),
            (false, true) => format!("<em>{esc}</em>"),
            (false, false) => esc,
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Chapter, Document, ExportStyle, Part};
    use std::io::Read;

    const MODIFIED: &str = "2026-06-27T00:00:00Z";

    fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
        Epigraph {
            text: text.to_string(),
            attribution: by.map(str::to_string),
        }
    }

    fn chapter(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
        Chapter {
            title: title.to_string(),
            epigraph,
            body: body.to_string(),
        }
    }

    fn doc() -> Document {
        Document {
            title: "Red <State> Earth".into(),
            subtitle: Some("a novel".into()),
            author: Some("A. Writer".into()),
            series: Some("The Cycle".into()),
            epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
            front: vec![chapter("Prologue", None, "Before it all.")],
            parts: vec![Part {
                name: Some("Part One".into()),
                chapters: vec![chapter(
                    "The Find",
                    Some(epigraph("A line.", None)),
                    "The locket *gleamed* and felt **heavy**.\n\n## A scene\n\nScene one.\n\n---\n\nScene two.",
                )],
            }],
            back: vec![chapter("Afterword", None, "After.")],
        }
    }

    /// Read one zip entry's text by name.
    fn part(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
        let mut file = zip.by_name(name).expect("entry present");
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        s
    }

    fn names(bytes: &[u8]) -> Vec<String> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
        zip.file_names().map(str::to_string).collect()
    }

    #[test]
    fn mimetype_is_first_and_stored() {
        let bytes = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
        assert_eq!(&bytes[..2], b"PK", "a zip container");
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let first = zip.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype", "mimetype is the first entry");
        assert_eq!(
            first.compression(),
            CompressionMethod::Stored,
            "stored uncompressed"
        );
        drop(first);
        // ...with exactly the OCF media type, no trailing newline.
        let mut s = String::new();
        zip.by_index(0).unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "application/epub+zip");
    }

    #[test]
    fn has_the_required_package_parts() {
        let bytes = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
        for required in [
            "mimetype",
            "META-INF/container.xml",
            "OEBPS/content.opf",
            "OEBPS/nav.xhtml",
            "OEBPS/toc.ncx",
            "OEBPS/style.css",
        ] {
            assert!(
                names(&bytes).iter().any(|n| n == required),
                "{required} present"
            );
        }
        // container.xml points at the OPF.
        assert!(part(&bytes, "META-INF/container.xml").contains("OEBPS/content.opf"));
    }

    #[test]
    fn emphasis_and_scene_breaks_render_as_markup_not_literals() {
        let bytes = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
        let chap = part(&bytes, "OEBPS/chap0004.xhtml");
        assert!(chap.contains("<em>gleamed</em>"), "italic: {chap}");
        assert!(chap.contains("<strong>heavy</strong>"), "bold");
        assert!(!chap.contains('*'), "no literal asterisks");
        assert!(chap.contains("<h2>A scene</h2>"), "inner heading");
        assert!(
            chap.contains("scene-break") && !chap.contains("---"),
            "scene break, no rule"
        );
        // Chapter epigraph rendered set-apart before the prose.
        assert!(chap.contains("class=\"epigraph\"") && chap.contains("A line."));
        assert!(
            chap.find("A line.").unwrap() < chap.find("gleamed").unwrap(),
            "epigraph first"
        );
    }

    #[test]
    fn title_page_shows_metadata_and_book_epigraph() {
        let title = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/title0001.xhtml",
        );
        assert!(title.contains("The Cycle") && title.contains("Red &lt;State&gt; Earth"));
        assert!(title.contains("a novel") && title.contains("by A. Writer"));
        assert!(title.contains("In the beginning.") && title.contains("— Anon"));
    }

    #[test]
    fn smart_typography_glyphs_pass_through() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter(
                    "C",
                    None,
                    "He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}",
                )],
            }],
            back: vec![],
        };
        let chap = part(
            &to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/chap0002.xhtml",
        );
        assert!(
            chap.contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
            "curly glyphs"
        );
    }

    #[test]
    fn a_fenced_block_renders_as_verbatim_preformatted_xhtml() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter(
                    "C",
                    None,
                    "Before.\n\n```\nLOG // 2053\n  indented\n```\n\nAfter.",
                )],
            }],
            back: vec![],
        };
        let chap = part(
            &to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/chap0002.xhtml",
        );
        // Whitespace and line breaks preserved; not wrapped in a <p>.
        assert!(
            chap.contains("<pre>LOG // 2053\n  indented</pre>"),
            "{chap}"
        );
        assert!(!chap.contains("<p>LOG"), "code is not a paragraph:\n{chap}");
    }

    #[test]
    fn scene_break_renders_the_styled_glyph_default_unchanged() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("C", None, "A.\n\n---\n\nB.")],
            }],
            back: vec![],
        };
        // Default style → the plain centered `#`, exactly as before.
        let plain = part(
            &to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/chap0002.xhtml",
        );
        assert!(
            plain.contains("<p class=\"scene-break\">#</p>"),
            "default glyph: {plain}"
        );
        // A configured glyph replaces it (and is XML-escaped).
        let style = ExportStyle {
            scene_break: Some("* * &".into()),
            ..Default::default()
        };
        let styled = part(
            &to_epub(&d, MODIFIED, &style).unwrap(),
            "OEBPS/chap0002.xhtml",
        );
        assert!(
            styled.contains("<p class=\"scene-break\">* * &amp;</p>"),
            "styled+escaped: {styled}"
        );
        assert!(!styled.contains(">#</p>"), "default glyph replaced");
    }

    #[test]
    fn fonts_are_embedded_and_applied_via_the_stylesheet() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("C", None, "Body.")],
            }],
            back: vec![],
        };
        let mut style = ExportStyle {
            body_font: Some("b.otf".into()),
            heading_font: Some("h.ttf".into()),
            ..Default::default()
        };
        style.resolve_fonts(|p| Ok(format!("font:{p}").into_bytes()));
        let bytes = to_epub(&d, MODIFIED, &style).unwrap();

        // The font files are embedded under OEBPS/fonts/, named by role.
        assert!(
            names(&bytes).iter().any(|n| n == "OEBPS/fonts/body.otf"),
            "body font file"
        );
        assert!(
            names(&bytes).iter().any(|n| n == "OEBPS/fonts/heading.ttf"),
            "heading font file"
        );
        // The OPF manifest declares them with their media types.
        let opf = part(&bytes, "OEBPS/content.opf");
        assert!(
            opf.contains("href=\"fonts/body.otf\" media-type=\"font/otf\""),
            "{opf}"
        );
        assert!(
            opf.contains("href=\"fonts/heading.ttf\" media-type=\"font/ttf\""),
            "{opf}"
        );
        // The stylesheet declares @font-face and applies the families.
        let css = part(&bytes, "OEBPS/style.css");
        assert!(
            css.contains("@font-face") && css.contains("url(\"fonts/body.otf\")"),
            "{css}"
        );
        assert!(
            css.contains("body { font-family: \"b\""),
            "body family: {css}"
        );
        assert!(
            css.contains("h1, h2 { font-family: \"h\""),
            "heading family: {css}"
        );

        // Default style → no fonts embedded, no @font-face.
        let plain = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
        assert!(
            !names(&plain).iter().any(|n| n.starts_with("OEBPS/fonts/")),
            "no fonts by default"
        );
        assert!(!part(&plain, "OEBPS/style.css").contains("@font-face"));
    }

    #[test]
    fn heading_alignment_and_drop_cap_render_via_css() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("One", None, "First para.\n\nSecond para.")],
            }],
            back: vec![],
        };
        let style = ExportStyle {
            heading_align: Some("left".into()),
            drop_cap: Some(true),
            ..Default::default()
        };
        let bytes = to_epub(&d, MODIFIED, &style).unwrap();
        let css = part(&bytes, "OEBPS/style.css");
        assert!(
            css.contains("h1, h2 { text-align: left; }"),
            "alignment: {css}"
        );
        assert!(
            css.contains(".opening::first-letter {"),
            "drop-cap rule: {css}"
        );
        // Only the chapter's first paragraph carries the `opening` class.
        let chap = part(&bytes, "OEBPS/chap0002.xhtml");
        assert_eq!(chap.matches("class=\"opening\"").count(), 1, "{chap}");
        assert!(
            chap.contains("<p class=\"opening\">First para.</p>"),
            "{chap}"
        );
        assert!(chap.contains("<p>Second para.</p>"), "{chap}");

        // Default style → no alignment override, no drop cap, no opening class.
        let plain = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
        assert!(!part(&plain, "OEBPS/style.css").contains("text-align: left"));
        assert!(!part(&plain, "OEBPS/style.css").contains("first-letter"));
        assert!(!part(&plain, "OEBPS/chap0002.xhtml").contains("opening"));
    }

    #[test]
    fn title_and_part_ornaments_render_when_configured() {
        // `doc()` has a title page (with author) and a named part ("Part One").
        let style = ExportStyle {
            title_ornament: Some("❧".into()),
            part_ornament: Some("* * &".into()),
            ..Default::default()
        };
        let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();
        let css = part(&bytes, "OEBPS/style.css");
        assert!(css.contains(".ornament {"), "ornament rule: {css}");
        // Title-page ornament (inside the centered title block).
        let title = part(&bytes, "OEBPS/title0001.xhtml");
        assert!(title.contains("<p class=\"ornament\">❧</p>"), "{title}");
        // Part-divider ornament, XML-escaped.
        let part_doc = part(&bytes, "OEBPS/part0003.xhtml");
        assert!(
            part_doc.contains("<p class=\"ornament\">* * &amp;</p>"),
            "{part_doc}"
        );

        // Default style → no ornaments, no rule.
        let plain = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
        assert!(!part(&plain, "OEBPS/style.css").contains(".ornament"));
        assert!(!part(&plain, "OEBPS/title0001.xhtml").contains("ornament"));
    }

    #[test]
    fn a_preset_styles_the_export_and_explicit_keys_still_override_it() {
        // `classic` bundles a scene break, drop cap, and ornaments; one explicit
        // key overrides just that setting and the rest of the preset stands.
        let mut style = ExportStyle {
            preset: Some("classic".into()),
            scene_break: Some("~ ~ ~".into()),
            ..Default::default()
        };
        assert!(style.apply_preset().is_none());
        let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();

        let css = part(&bytes, "OEBPS/style.css");
        assert!(css.contains(".opening::first-letter {"), "drop cap: {css}");
        assert!(css.contains(".ornament {"), "ornaments: {css}");

        // The preset's title ornament renders; the overridden scene break wins.
        assert!(part(&bytes, "OEBPS/title0001.xhtml").contains("<p class=\"ornament\">❦</p>"));
        let chap = part(&bytes, "OEBPS/chap0004.xhtml");
        assert!(
            chap.contains("<p class=\"scene-break\">~ ~ ~</p>"),
            "{chap}"
        );
        assert!(
            chap.contains("class=\"opening\""),
            "drop cap applied: {chap}"
        );

        // `modern` is the contrasting bundle: left headings, a bullet break, no
        // drop cap.
        let mut modern = ExportStyle {
            preset: Some("modern".into()),
            ..Default::default()
        };
        assert!(modern.apply_preset().is_none());
        let mbytes = to_epub(&doc(), MODIFIED, &modern).unwrap();
        let mcss = part(&mbytes, "OEBPS/style.css");
        assert!(mcss.contains("text-align: left;"), "modern align: {mcss}");
        assert!(
            !mcss.contains("first-letter"),
            "modern has no drop cap: {mcss}"
        );
        assert!(part(&mbytes, "OEBPS/chap0004.xhtml").contains("<p class=\"scene-break\">•</p>"));

        // A genre preset from the library (#482) renders its own bundle too:
        // `fantasy` is a diamond scene break with fleur-de-lis ornaments.
        let mut fantasy = ExportStyle {
            preset: Some("fantasy".into()),
            ..Default::default()
        };
        assert!(fantasy.apply_preset().is_none());
        let fbytes = to_epub(&doc(), MODIFIED, &fantasy).unwrap();
        assert!(part(&fbytes, "OEBPS/title0001.xhtml").contains("<p class=\"ornament\">⚜</p>"));
        assert!(part(&fbytes, "OEBPS/chap0004.xhtml").contains("<p class=\"scene-break\">❖</p>"));
    }

    #[test]
    fn images_are_embedded_and_placed_and_override_the_scene_glyph() {
        // doc(): title page (with author), a named part, a chapter ("The Find")
        // whose body has a `---` scene break.
        let mut style = ExportStyle {
            scene_break_image: Some("art/break.png".into()),
            chapter_heading_image: Some("art/banner.jpg".into()),
            title_image: Some("art/title.svg".into()),
            ..Default::default()
        };
        style.resolve_images(|_| Ok(vec![1, 2, 3]));
        let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();

        // Files embedded under OEBPS/images/, named by role.
        for f in [
            "OEBPS/images/scene-break.png",
            "OEBPS/images/chapter-heading.jpg",
            "OEBPS/images/title.svg",
        ] {
            assert!(names(&bytes).iter().any(|n| n == f), "{f} embedded");
        }
        // OPF manifest declares them with media types.
        let opf = part(&bytes, "OEBPS/content.opf");
        assert!(
            opf.contains("href=\"images/scene-break.png\" media-type=\"image/png\""),
            "{opf}"
        );
        assert!(
            opf.contains("href=\"images/title.svg\" media-type=\"image/svg+xml\""),
            "{opf}"
        );
        // CSS for the image placements.
        assert!(
            part(&bytes, "OEBPS/style.css").contains(".chapter-image"),
            "image css"
        );
        // Title image on the title page.
        assert!(part(&bytes, "OEBPS/title0001.xhtml")
            .contains("<img src=\"images/title.svg\" alt=\"\"/>"));
        // Chapter-heading image above the title; scene break renders the image, not `#`.
        let chap = part(&bytes, "OEBPS/chap0004.xhtml");
        assert!(
            chap.contains("<p class=\"chapter-image\"><img src=\"images/chapter-heading.jpg\""),
            "{chap}"
        );
        assert!(
            chap.contains("<p class=\"scene-break\"><img src=\"images/scene-break.png\""),
            "{chap}"
        );
        assert!(!chap.contains("scene-break\">#"), "glyph replaced by image");

        // Default style → no image files, no <img>.
        let plain = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
        assert!(!names(&plain).iter().any(|n| n.starts_with("OEBPS/images/")));
        assert!(!part(&plain, "OEBPS/chap0004.xhtml").contains("<img"));
    }

    #[test]
    fn a_stable_identifier_is_derived_from_the_title() {
        let opf = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/content.opf",
        );
        // Same book → same id across exports (readers dedupe on it).
        assert!(
            opf.contains("urn:quill:the-cycle"),
            "series+title slug: {opf}"
        );
    }
}

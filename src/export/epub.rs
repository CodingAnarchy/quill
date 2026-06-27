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

use super::{
    classify_block, inline_runs, xml_escape, Chapter, Document, EmbeddedFont, ExportStyle,
    FontRole, Prose,
};
use crate::frontmatter::Epigraph;

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
        let mut body = format!("<h1 class=\"book-title\">{}</h1>\n", xml_escape(&doc.title));
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
                    let mut body = format!("<h1 class=\"part-title\">{}</h1>\n", xml_escape(name));
                    if let Some(o) = &self.style.part_ornament {
                        push_p(&mut body, "ornament", o);
                    }
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
        let opf = build_opf(&meta, &self.items, &self.style.fonts);
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
        // Embedded fonts (#469), referenced by the stylesheet's `@font-face` rules.
        // `finish` owns `self`, so the (potentially large) font bytes move, not copy.
        for font in self.style.fonts {
            parts.push((format!("OEBPS/{}", font.href), font.bytes));
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
    if style.drop_cap {
        // The first paragraph of each chapter (marked `opening`) gets a drop cap;
        // its first line is flush so the initial sits in the margin.
        css.push_str("p.opening { text-indent: 0; }\n");
        css.push_str(".opening::first-letter { float: left; font-size: 3.2em; line-height: 0.72; padding-right: 0.06em; font-weight: bold; }\n");
    }
    if style.title_ornament.is_some() || style.part_ornament.is_some() {
        css.push_str(".ornament { text-align: center; text-indent: 0; margin: 1.5em 0; font-size: 1.5em; }\n");
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

/// A chapter's XHTML: its title as `<h1>`, the epigraph set apart, then the prose
/// blocks (scene breaks, inner headings, and emphasis-bearing paragraphs).
fn chapter_body(ch: &Chapter, style: &ExportStyle) -> String {
    let mut out = format!("<h1>{}</h1>\n", xml_escape(&ch.title));
    if let Some(e) = &ch.epigraph {
        out.push_str(&epigraph_xhtml(e));
    }
    // With drop caps on, the chapter's first prose paragraph is marked `opening`.
    let mut opened = false;
    for block in ch.body.split("\n\n") {
        let Some(prose) = classify_block(block) else {
            continue;
        };
        let opening = style.drop_cap && !opened && matches!(prose, Prose::Paragraph(_));
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
        Prose::SceneBreak => p_class("scene-break", style.scene_break()),
        Prose::Heading(h) => format!("<h2>{}</h2>", xml_escape(h)),
        Prose::Paragraph(text) => {
            let class = if opening { " class=\"opening\"" } else { "" };
            format!("<p{class}>{}</p>", inline_xhtml(text))
        }
    }
}

/// An epigraph as a set-apart block: each line a `<p>`, the attribution on its
/// own line. Text is taken literally (the whole block is italicized by the
/// stylesheet), matching the other writers' epigraph fidelity.
fn epigraph_xhtml(e: &Epigraph) -> String {
    let mut out = String::from("<div class=\"epigraph\">\n");
    for line in e.text.lines() {
        // A class-less `<p>`, so not via `push_p`.
        out.push_str(&format!("<p>{}</p>\n", xml_escape(line)));
    }
    if let Some(a) = &e.attribution {
        push_p(&mut out, "attribution", &format!("— {a}"));
    }
    out.push_str("</div>\n");
    out
}

/// `<p class="…">…</p>` with the text XML-escaped — the styled-paragraph idiom the
/// designed pages share (title, ornaments, scene break, …).
fn p_class(class: &str, text: &str) -> String {
    format!("<p class=\"{class}\">{}</p>", xml_escape(text))
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
    for (run, bold, italic) in inline_runs(text) {
        let esc = xml_escape(&run);
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

/// The OPF package: metadata (identifier, title + optional subtitle, language,
/// author, series collection, modified time), the manifest (nav, NCX, CSS, and
/// every content doc), and the spine in reading order.
/// The OPF metadata for one package — grouped so [`build_opf`] stays within a
/// sane argument count.
struct OpfMeta<'a> {
    id: &'a str,
    title: &'a str,
    subtitle: Option<&'a str>,
    author: Option<&'a str>,
    series: Option<&'a str>,
    modified: &'a str,
}

fn build_opf(book: &OpfMeta, items: &[Item], fonts: &[EmbeddedFont]) -> String {
    let mut meta = String::new();
    meta.push_str(&format!(
        "<dc:identifier id=\"bookid\">{}</dc:identifier>",
        xml_escape(book.id)
    ));
    meta.push_str(&format!(
        "<dc:title id=\"title\">{}</dc:title>",
        xml_escape(book.title)
    ));
    meta.push_str("<meta refines=\"#title\" property=\"title-type\">main</meta>");
    if let Some(s) = book.subtitle {
        meta.push_str(&format!(
            "<dc:title id=\"subtitle\">{}</dc:title>",
            xml_escape(s)
        ));
        meta.push_str("<meta refines=\"#subtitle\" property=\"title-type\">subtitle</meta>");
    }
    meta.push_str("<dc:language>en</dc:language>");
    if let Some(a) = book.author {
        meta.push_str(&format!("<dc:creator>{}</dc:creator>", xml_escape(a)));
    }
    if let Some(s) = book.series {
        meta.push_str(&format!(
            "<meta property=\"belongs-to-collection\" id=\"series\">{}</meta>",
            xml_escape(s)
        ));
        meta.push_str("<meta refines=\"#series\" property=\"collection-type\">series</meta>");
    }
    meta.push_str(&format!(
        "<meta property=\"dcterms:modified\">{}</meta>",
        book.modified
    ));

    let mut manifest = String::from(concat!(
        "<item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>",
        "<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>",
        "<item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>"
    ));
    for (i, font) in fonts.iter().enumerate() {
        manifest.push_str(&format!(
            "<item id=\"font{i}\" href=\"{}\" media-type=\"{}\"/>",
            font.href, font.media_type
        ));
    }
    let mut spine = String::new();
    for it in items {
        manifest.push_str(&format!(
            "<item id=\"{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>",
            it.id, it.href
        ));
        spine.push_str(&format!("<itemref idref=\"{}\"/>", it.id));
    }

    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" ",
            "unique-identifier=\"bookid\" xml:lang=\"en\">",
            "<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">{meta}</metadata>",
            "<manifest>{manifest}</manifest>",
            "<spine toc=\"ncx\">{spine}</spine></package>"
        ),
        meta = meta,
        manifest = manifest,
        spine = spine,
    )
}

/// The EPUB 3 navigation document — a nested `<ol>` table of contents.
fn build_nav(entries: &[Nav]) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE html>\n",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" ",
            "xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"en\" lang=\"en\">",
            "<head><meta charset=\"utf-8\"/><title>Contents</title></head>",
            "<body><nav epub:type=\"toc\" id=\"toc\"><h1>Contents</h1>\n{ol}</nav></body></html>"
        ),
        ol = nav_ol(entries),
    )
}

fn nav_ol(entries: &[Nav]) -> String {
    let mut s = String::from("<ol>\n");
    for e in entries {
        s.push_str(&format!(
            "<li><a href=\"{}\">{}</a>",
            e.href,
            xml_escape(&e.label)
        ));
        if !e.children.is_empty() {
            s.push('\n');
            s.push_str(&nav_ol(&e.children));
        }
        s.push_str("</li>\n");
    }
    s.push_str("</ol>\n");
    s
}

/// The EPUB 2 NCX, for readers that don't read the EPUB 3 nav doc.
fn build_ncx(id: &str, title: &str, entries: &[Nav]) -> String {
    let mut order = 0;
    let points = ncx_points(entries, &mut order);
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">",
            "<head><meta name=\"dtb:uid\" content=\"{uid}\"/>",
            "<meta name=\"dtb:depth\" content=\"{depth}\"/>",
            "<meta name=\"dtb:totalPageCount\" content=\"0\"/>",
            "<meta name=\"dtb:maxPageNumber\" content=\"0\"/></head>",
            "<docTitle><text>{title}</text></docTitle><navMap>{points}</navMap></ncx>"
        ),
        uid = xml_escape(id),
        depth = nav_depth(entries),
        title = xml_escape(title),
        points = points,
    )
}

fn ncx_points(entries: &[Nav], order: &mut usize) -> String {
    let mut s = String::new();
    for e in entries {
        *order += 1;
        s.push_str(&format!(
            "<navPoint id=\"np{order}\" playOrder=\"{order}\"><navLabel><text>{}</text></navLabel><content src=\"{}\"/>",
            xml_escape(&e.label),
            e.href
        ));
        s.push_str(&ncx_points(&e.children, order));
        s.push_str("</navPoint>");
    }
    s
}

/// The deepest nesting level of the TOC, for the NCX `dtb:depth`.
fn nav_depth(entries: &[Nav]) -> usize {
    entries
        .iter()
        .map(|e| 1 + nav_depth(&e.children))
        .max()
        .unwrap_or(0)
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
    fn opf_carries_book_metadata() {
        let opf = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/content.opf",
        );
        assert!(
            opf.contains("Red &lt;State&gt; Earth"),
            "title escaped: {opf}"
        );
        assert!(
            opf.contains("a novel") && opf.contains("title-type\">subtitle"),
            "subtitle"
        );
        assert!(opf.contains("<dc:creator>A. Writer</dc:creator>"), "author");
        assert!(
            opf.contains("belongs-to-collection") && opf.contains("The Cycle"),
            "series"
        );
        assert!(
            opf.contains("dcterms:modified\">2026-06-27T00:00:00Z"),
            "modified"
        );
        assert!(opf.contains("unique-identifier=\"bookid\""));
        // Every spine itemref resolves to a manifest item id.
        let manifest = &opf[opf.find("<manifest>").unwrap()..opf.find("</manifest>").unwrap()];
        let spine = &opf[opf.find("<spine").unwrap()..];
        for chunk in spine.split("idref=\"").skip(1) {
            let id = &chunk[..chunk.find('"').unwrap()];
            assert!(
                manifest.contains(&format!("id=\"{id}\"")),
                "spine {id} in manifest"
            );
        }
    }

    #[test]
    fn chapters_appear_in_reading_order_in_the_spine() {
        let opf = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/content.opf",
        );
        let spine = &opf[opf.find("<spine").unwrap()..];
        // title page, prologue (front), part divider, the chapter, afterword (back) —
        // front/body/back chapters all use the `chap` prefix, in reading order.
        let order = ["title0001", "chap0002", "part0003", "chap0004", "chap0005"];
        let mut last = 0;
        for id in order {
            let at = spine
                .find(id)
                .unwrap_or_else(|| panic!("{id} in spine: {spine}"));
            assert!(at >= last, "{id} out of order");
            last = at;
        }
    }

    #[test]
    fn nav_nests_chapters_under_their_part() {
        let nav = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/nav.xhtml",
        );
        // The part is a TOC entry with a nested list holding its chapter.
        let part_at = nav.find("Part One").expect("part in toc");
        let nested = nav[part_at..].find("<ol>").expect("nested list under part");
        let chapter_at = nav[part_at..].find("The Find").expect("chapter in toc");
        assert!(nested < chapter_at, "chapter nested under the part: {nav}");
        // Front/back matter are top-level entries.
        assert!(nav.contains("Prologue") && nav.contains("Afterword"));
        // The NCX mirrors the structure for older readers.
        let ncx = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/toc.ncx",
        );
        assert!(ncx.contains("Part One") && ncx.contains("The Find"));
        assert!(
            ncx.contains("dtb:depth\" content=\"2\""),
            "two-level depth: {ncx}"
        );
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
            drop_cap: true,
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

    #[test]
    fn series_epub_nests_each_book_with_its_parts() {
        let book = |title: &str, body: &str| Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("Ch", None, body)],
            }],
            back: vec![],
        };
        let bytes = to_epub_series(
            &[book("Book One", "alpha"), book("Book Two", "beta")],
            Some("The Saga"),
            Some("A. Writer"),
            MODIFIED,
            &ExportStyle::default(),
        )
        .unwrap();
        let nav = part(&bytes, "OEBPS/nav.xhtml");
        let one = nav.find("Book One").expect("book one");
        let two = nav.find("Book Two").expect("book two");
        assert!(one < two, "books in order");
        let opf = part(&bytes, "OEBPS/content.opf");
        assert!(
            opf.contains("The Saga") && opf.contains("A. Writer"),
            "series metadata"
        );
        // Each book's prose is in its own chapter file.
        assert!(part(&bytes, "OEBPS/chap0003.xhtml").contains("alpha"));
    }
}

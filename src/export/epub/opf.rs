//! The EPUB package serializers — a distinct layer from the [`super::Builder`]
//! section-assembly logic. From the accumulated content [`Item`]s and the TOC
//! [`Nav`] tree, these render the OPF package document (#297), the EPUB 3
//! navigation document, and the EPUB 2 NCX for older readers. Re-exported into
//! [`super`] so `finish` calls them unqualified.

use std::fmt::Write;

use super::{Item, Landmarks, Nav};
use crate::export::{xml_escape, EmbeddedFont, EmbeddedImage, ImageRole, PubMeta};

/// The manifest id of the cover-image item (#582) — referenced by the EPUB 2
/// `<meta name="cover">` so older readers find the cover.
const COVER_IMAGE_ID: &str = "cover-image";

/// The OPF package: metadata (identifier, title + optional subtitle, language,
/// author, series collection, modified time), the manifest (nav, NCX, CSS, and
/// every content doc), and the spine in reading order.
/// The OPF metadata for one package — grouped so [`build_opf`] stays within a
/// sane argument count.
pub(super) struct OpfMeta<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub subtitle: Option<&'a str>,
    pub author: Option<&'a str>,
    pub series: Option<&'a str>,
    pub modified: &'a str,
    /// Publishing metadata (#583): drives `dc:publisher`/`dc:date`/`dc:rights` and
    /// the `dc:language`/`xml:lang` (its `en` fallback when unset).
    pub publishing: &'a PubMeta,
}

pub(super) fn build_opf(
    book: &OpfMeta,
    items: &[Item],
    fonts: &[EmbeddedFont],
    images: &[EmbeddedImage],
) -> String {
    let mut meta = String::new();
    let _ = write!(
        meta,
        "<dc:identifier id=\"bookid\">{}</dc:identifier>",
        xml_escape(book.id)
    );
    let _ = write!(
        meta,
        "<dc:title id=\"title\">{}</dc:title>",
        xml_escape(book.title)
    );
    meta.push_str("<meta refines=\"#title\" property=\"title-type\">main</meta>");
    if let Some(s) = book.subtitle {
        let _ = write!(
            meta,
            "<dc:title id=\"subtitle\">{}</dc:title>",
            xml_escape(s)
        );
        meta.push_str("<meta refines=\"#subtitle\" property=\"title-type\">subtitle</meta>");
    }
    // The language drives `dc:language` (and the package `xml:lang` below), the
    // `en` fallback now living in `PubMeta::language` rather than hardcoded (#583).
    let _ = write!(
        meta,
        "<dc:language>{}</dc:language>",
        xml_escape(book.publishing.language())
    );
    if let Some(a) = book.author {
        let _ = write!(meta, "<dc:creator>{}</dc:creator>", xml_escape(a));
    }
    // Publishing metadata (#583): publisher, publication date, and the rights line.
    if let Some(p) = &book.publishing.publisher {
        let _ = write!(meta, "<dc:publisher>{}</dc:publisher>", xml_escape(p));
    }
    if let Some(d) = &book.publishing.date {
        let _ = write!(meta, "<dc:date>{}</dc:date>", xml_escape(d));
    }
    if let Some(r) = &book.publishing.rights {
        let _ = write!(meta, "<dc:rights>{}</dc:rights>", xml_escape(r));
    }
    if let Some(s) = book.series {
        let _ = write!(
            meta,
            "<meta property=\"belongs-to-collection\" id=\"series\">{}</meta>",
            xml_escape(s)
        );
        meta.push_str("<meta refines=\"#series\" property=\"collection-type\">series</meta>");
    }
    let _ = write!(
        meta,
        "<meta property=\"dcterms:modified\">{}</meta>",
        book.modified
    );
    // Accessibility metadata (#642): storefronts (Apple Books, Kobo) and the EU
    // Accessibility Act expect the `schema:*` a11y properties. A Quill EPUB is a
    // reflowable, text-only novel — everything is textual, so `textual` alone is
    // a sufficient access mode. The feature list reflects what the package
    // actually provides: a nav TOC + landmarks (#604) and a heading structure.
    meta.push_str("<meta property=\"schema:accessMode\">textual</meta>");
    meta.push_str("<meta property=\"schema:accessModeSufficient\">textual</meta>");
    meta.push_str("<meta property=\"schema:accessibilityFeature\">tableOfContents</meta>");
    meta.push_str("<meta property=\"schema:accessibilityFeature\">structuralNavigation</meta>");
    meta.push_str("<meta property=\"schema:accessibilityFeature\">readingOrder</meta>");
    // When any embedded image carries author-supplied alt text (#818), the package
    // genuinely provides text alternatives — advertise `alternativeText` so the
    // metadata stays truthful. The cover always describes itself (it falls back to
    // the title), so a cover alone earns the feature.
    if images
        .iter()
        .any(|i| i.role == ImageRole::Cover || i.alt.is_some())
    {
        meta.push_str("<meta property=\"schema:accessibilityFeature\">alternativeText</meta>");
    }
    meta.push_str("<meta property=\"schema:accessibilityHazard\">none</meta>");
    meta.push_str(
        "<meta property=\"schema:accessibilitySummary\">This publication is a \
         reflowable, text-only book. All content is presented as text, with a \
         navigable table of contents, landmarks, and a logical heading \
         structure. It contains no audio, video, or other content that would \
         pose a known accessibility hazard.</meta>",
    );
    // The EPUB 2 cover hint (#582): older readers (and many storefronts) find the
    // cover via this legacy `<meta name="cover">` pointing at the manifest item id.
    if images.iter().any(|i| i.role == ImageRole::Cover) {
        let _ = write!(meta, "<meta name=\"cover\" content=\"{COVER_IMAGE_ID}\"/>");
    }

    let mut manifest = String::new();
    manifest.push_str(&manifest_item(
        "nav",
        "nav.xhtml",
        "application/xhtml+xml",
        Some("nav"),
    ));
    manifest.push_str(&manifest_item(
        "ncx",
        "toc.ncx",
        "application/x-dtbncx+xml",
        None,
    ));
    manifest.push_str(&manifest_item("css", "style.css", "text/css", None));
    for (i, font) in fonts.iter().enumerate() {
        manifest.push_str(&manifest_item(
            &format!("font{i}"),
            &font.href,
            font.media_type,
            None,
        ));
    }
    for (i, image) in images.iter().enumerate() {
        // The cover image carries the EPUB 3 `cover-image` property and a stable id
        // the EPUB 2 `<meta name="cover">` above references (#582); the decorative
        // images keep their generic ids.
        let (id, properties) = if image.role == ImageRole::Cover {
            (COVER_IMAGE_ID.to_string(), Some("cover-image"))
        } else {
            (format!("img{i}"), None)
        };
        manifest.push_str(&manifest_item(
            &id,
            &image.href,
            image.media_type,
            properties,
        ));
    }
    // The nav doc rides the spine as a non-linear entry (#604) so the `landmarks`
    // "Table of Contents" reference resolves to a spine item (an epubcheck
    // requirement) without inserting the TOC into the linear reading order.
    let mut spine = String::from("<itemref idref=\"nav\" linear=\"no\"/>");
    for it in items {
        manifest.push_str(&manifest_item(
            &it.id,
            &it.href,
            "application/xhtml+xml",
            None,
        ));
        let _ = write!(spine, "<itemref idref=\"{}\"/>", it.id);
    }

    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" ",
            "unique-identifier=\"bookid\" xml:lang=\"{lang}\">",
            "<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">{meta}</metadata>",
            "<manifest>{manifest}</manifest>",
            "<spine toc=\"ncx\">{spine}</spine></package>"
        ),
        lang = xml_escape(book.publishing.language()),
        meta = meta,
        manifest = manifest,
        spine = spine,
    )
}

/// One OPF manifest `<item/>`: id, href, media type, and optional `properties`
/// (e.g. `nav` for the navigation document). Every manifest entry routes through
/// here so attribute order and escaping stay uniform — `id`/`href` are
/// XML-escaped even though current package-internal values have no special
/// characters.
fn manifest_item(id: &str, href: &str, media_type: &str, properties: Option<&str>) -> String {
    let props = properties.map_or_else(String::new, |p| {
        format!(" properties=\"{}\"", xml_escape(p))
    });
    format!(
        "<item id=\"{}\" href=\"{}\" media-type=\"{}\"{props}/>",
        xml_escape(id),
        xml_escape(href),
        xml_escape(media_type),
    )
}

/// The EPUB 3 navigation document — a nested `<ol>` table of contents plus a
/// `landmarks` nav (#604) pointing at the cover, title page, table of contents,
/// and the start of the body matter ("Begin Reading"), the reference points
/// Apple Books / Kobo use to jump around the book. `lang` is the book language
/// (#583), driving the `xml:lang`/`lang` (default `en`).
pub(super) fn build_nav(entries: &[Nav], landmarks: &Landmarks, lang: &str) -> String {
    let lang = xml_escape(lang);
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE html>\n",
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" ",
            "xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{lang}\" lang=\"{lang}\">",
            "<head><meta charset=\"utf-8\"/><title>Contents</title></head>",
            "<body><nav epub:type=\"toc\" id=\"toc\"><h1>Contents</h1>\n{ol}</nav>\n{landmarks}</body></html>"
        ),
        lang = lang,
        ol = nav_ol(entries),
        landmarks = nav_landmarks(landmarks),
    )
}

/// The hidden `landmarks` nav (#604): one `<li>` per reference point, each an
/// anchor carrying its own `epub:type`. The `toc` landmark points back at this
/// document's `toc` nav; the cover, title page, and body-matter start point at
/// their content docs when present.
fn nav_landmarks(landmarks: &Landmarks) -> String {
    let mut lis = String::new();
    let mut li = |kind: &str, href: &str, label: &str| {
        let _ = writeln!(
            lis,
            "<li><a epub:type=\"{kind}\" href=\"{}\">{}</a></li>",
            href,
            xml_escape(label),
        );
    };
    if let Some(href) = &landmarks.cover {
        li("cover", href, "Cover");
    }
    if let Some(href) = &landmarks.titlepage {
        li("titlepage", href, "Title Page");
    }
    li("toc", "nav.xhtml#toc", "Table of Contents");
    if let Some(href) = &landmarks.bodymatter {
        li("bodymatter", href, "Begin Reading");
    }
    // Role-specific reference points (#695): a foreword, dedication, glossary, …
    // in reading order, first occurrence per `epub:type` (a book with two
    // appendices lands one appendix landmark, at the first).
    let mut seen: Vec<&str> = Vec::new();
    for section in &landmarks.sections {
        if seen.contains(&section.epub_type) {
            continue;
        }
        seen.push(section.epub_type);
        li(section.epub_type, &section.href, &section.label);
    }
    format!("<nav epub:type=\"landmarks\" hidden=\"hidden\"><h2>Landmarks</h2>\n<ol>\n{lis}</ol>\n</nav>")
}

fn nav_ol(entries: &[Nav]) -> String {
    let mut s = String::from("<ol>\n");
    for e in entries {
        let _ = write!(s, "<li><a href=\"{}\">{}</a>", e.href, xml_escape(&e.label));
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
pub(super) fn build_ncx(id: &str, title: &str, entries: &[Nav]) -> String {
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
        let _ = write!(
            s,
            "<navPoint id=\"np{order}\" playOrder=\"{order}\"><navLabel><text>{}</text></navLabel><content src=\"{}\"/>",
            xml_escape(&e.label),
            e.href
        );
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
    use std::io::{Cursor, Read};
    use std::path::Path;

    use crate::export::epub::{to_epub, to_epub_series};
    use crate::export::{Chapter, Document, ExportStyle, Part, PubMeta};
    use crate::frontmatter::Epigraph;

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
            role: None,
        }
    }

    fn doc() -> Document {
        Document {
            title: "Red <State> Earth".into(),
            subtitle: Some("a novel".into()),
            author: Some("A. Writer".into()),
            series: Some("The Cycle".into()),
            publishing: PubMeta::default(),
            epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
            half_title: false,
            series_books: Vec::new(),
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
    fn opf_declares_accessibility_metadata() {
        // The OPF carries the `schema:*` accessibility metadata (#642) for a
        // reflowable, text-only novel: textual access mode (sufficient on its
        // own), the structural-navigation features the package provides (nav TOC
        // + landmarks + heading structure), no known hazards, and a
        // human-readable summary.
        let opf = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/content.opf",
        );
        assert!(
            opf.contains("<meta property=\"schema:accessMode\">textual</meta>"),
            "accessMode textual: {opf}"
        );
        assert!(
            opf.contains("<meta property=\"schema:accessModeSufficient\">textual</meta>"),
            "accessModeSufficient textual: {opf}"
        );
        assert!(
            opf.contains("<meta property=\"schema:accessibilityFeature\">tableOfContents</meta>"),
            "tableOfContents feature: {opf}"
        );
        assert!(
            opf.contains(
                "<meta property=\"schema:accessibilityFeature\">structuralNavigation</meta>"
            ),
            "structuralNavigation feature: {opf}"
        );
        assert!(
            opf.contains("<meta property=\"schema:accessibilityFeature\">readingOrder</meta>"),
            "readingOrder feature: {opf}"
        );
        assert!(
            opf.contains("<meta property=\"schema:accessibilityHazard\">none</meta>"),
            "accessibilityHazard none: {opf}"
        );
        assert!(
            opf.contains("<meta property=\"schema:accessibilitySummary\">")
                && opf.contains("reflowable, text-only"),
            "human-readable accessibilitySummary: {opf}"
        );
    }

    #[test]
    fn opf_emits_publishing_metadata_and_drives_language() {
        // With publishing metadata set (#583): the ISBN is the identifier, the
        // language drives `dc:language` + `xml:lang`, and publisher/date/rights
        // ride into the OPF.
        let mut d = doc();
        d.publishing = PubMeta {
            isbn: Some("978-0-00-000000-0".into()),
            publisher: Some("Quill & Co.".into()),
            date: Some("2026-06-30".into()),
            lang: Some("fr".into()),
            rights: Some("© 2026 A. Writer".into()),
        };
        let opf = part(
            &to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/content.opf",
        );
        assert!(
            opf.contains("<dc:identifier id=\"bookid\">978-0-00-000000-0</dc:identifier>"),
            "ISBN is the identifier: {opf}"
        );
        assert!(
            !opf.contains("urn:quill:"),
            "minted id not used when ISBN set"
        );
        assert!(opf.contains("<dc:language>fr</dc:language>"), "dc:language");
        assert!(opf.contains("xml:lang=\"fr\""), "package xml:lang");
        assert!(
            opf.contains("<dc:publisher>Quill &amp; Co.</dc:publisher>"),
            "publisher escaped: {opf}"
        );
        assert!(opf.contains("<dc:date>2026-06-30</dc:date>"), "date");
        assert!(
            opf.contains("<dc:rights>© 2026 A. Writer</dc:rights>"),
            "rights"
        );

        // Default (no publishing metadata) → the minted id and the `en` fallback,
        // exactly as before (#583 fallback).
        let plain = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/content.opf",
        );
        assert!(
            plain.contains("urn:quill:"),
            "minted id by default: {plain}"
        );
        assert!(
            plain.contains("<dc:language>en</dc:language>"),
            "en default"
        );
        assert!(plain.contains("xml:lang=\"en\""), "en xml:lang default");
        assert!(!plain.contains("<dc:publisher>"), "no publisher by default");
        assert!(!plain.contains("<dc:rights>"), "no rights by default");
    }

    #[test]
    fn a_copyright_page_follows_the_title_page_in_the_spine() {
        // A book with publishing metadata gets a copyright page after the title
        // page (cover → title → copyright → body), carrying the rights line.
        let mut d = doc();
        d.publishing = PubMeta {
            publisher: Some("Quill & Co.".into()),
            rights: Some("© 2026 A. Writer".into()),
            ..Default::default()
        };
        let bytes = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
        let opf = part(&bytes, "OEBPS/content.opf");
        let spine = &opf[opf.find("<spine").unwrap()..];
        let title_at = spine.find("idref=\"title0001\"").expect("title in spine");
        let copy_at = spine
            .find("idref=\"copyright0002\"")
            .expect("copyright in spine");
        assert!(title_at < copy_at, "copyright follows title: {spine}");
        // The copyright XHTML carries the rights line and publisher.
        let page = part(&bytes, "OEBPS/copyright0002.xhtml");
        assert!(page.contains("© 2026 A. Writer"), "rights line: {page}");
        assert!(page.contains("Published by Quill &amp; Co."), "publisher");

        // No publishing metadata → no copyright page (default output unchanged).
        let plain = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
        let names: Vec<String> = zip::ZipArchive::new(Cursor::new(plain))
            .unwrap()
            .file_names()
            .map(str::to_string)
            .collect();
        assert!(
            !names.iter().any(|n| n.contains("copyright")),
            "no copyright page by default"
        );
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
    fn a_cover_image_is_the_first_spine_item_with_the_cover_metadata() {
        // A configured cover image (#582) embeds with the EPUB 3 cover-image property
        // and the EPUB 2 `<meta name="cover">`, and its page leads the spine.
        let mut style = ExportStyle {
            cover_image: Some("art/cover.png".into()),
            ..Default::default()
        };
        style.resolve_images(|_| Ok(vec![0x89, b'P', b'N', b'G']));
        let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();
        let opf = part(&bytes, "OEBPS/content.opf");

        // The image file is embedded and declared with the cover-image property.
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(
            zip.file_names().any(|n| n == "OEBPS/images/cover.png"),
            "cover image embedded"
        );
        drop(zip);
        assert!(
            opf.contains(
                "id=\"cover-image\" href=\"images/cover.png\" media-type=\"image/png\" \
                 properties=\"cover-image\""
            ),
            "cover-image manifest item: {opf}"
        );
        // The EPUB 2 legacy cover hint points at that item.
        assert!(
            opf.contains("<meta name=\"cover\" content=\"cover-image\"/>"),
            "EPUB 2 cover meta: {opf}"
        );
        // The cover page is the very first spine entry, ahead of the title page.
        let spine = &opf[opf.find("<spine").unwrap()..];
        let cover_at = spine.find("idref=\"cover0001\"").expect("cover in spine");
        let title_at = spine.find("idref=\"title0002\"").expect("title in spine");
        assert!(cover_at < title_at, "cover leads the spine: {spine}");

        // Default style → no cover item, no cover meta.
        let plain = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/content.opf",
        );
        assert!(
            !plain.contains("cover-image"),
            "no cover by default: {plain}"
        );
        assert!(
            !plain.contains("name=\"cover\""),
            "no cover meta by default"
        );
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
    fn nav_emits_a_landmarks_nav_pointing_at_the_reference_points() {
        // With a cover image and publishing metadata, the front matter is cover →
        // title → copyright, and the landmarks nav (#604) points at the cover, the
        // title page, the toc, and the first body-matter section ("Begin Reading").
        let mut style = ExportStyle {
            cover_image: Some("art/cover.png".into()),
            ..Default::default()
        };
        style.resolve_images(|_| Ok(vec![0x89, b'P', b'N', b'G']));
        let mut d = doc();
        d.publishing = PubMeta {
            rights: Some("© 2026 A. Writer".into()),
            ..Default::default()
        };
        let bytes = to_epub(&d, MODIFIED, &style).unwrap();
        let nav = part(&bytes, "OEBPS/nav.xhtml");

        let lm = &nav[nav
            .find("epub:type=\"landmarks\"")
            .expect("landmarks nav present")..];
        assert!(lm.contains("hidden"), "landmarks nav is hidden: {lm}");
        // cover0001 → title0002 → (copyright0003) → the prologue is the first
        // body... actually the prologue is front matter, so the first bodymatter
        // section is the part divider. The Prologue is frontmatter.
        assert!(
            lm.contains("epub:type=\"cover\" href=\"cover0001.xhtml\""),
            "cover landmark: {lm}"
        );
        assert!(
            lm.contains("epub:type=\"titlepage\" href=\"title0002.xhtml\""),
            "titlepage landmark: {lm}"
        );
        assert!(
            lm.contains("epub:type=\"toc\" href=\"nav.xhtml#toc\""),
            "toc landmark: {lm}"
        );
        // The Prologue (front) is frontmatter; the part divider is the first
        // body-matter section, so it is the "Begin Reading" target.
        assert!(
            lm.contains("epub:type=\"bodymatter\" href=\"part0005.xhtml\""),
            "bodymatter landmark points at the first body section: {lm}"
        );

        // Without a cover or copyright page, the landmarks nav drops those entries
        // but keeps toc + the body-matter start (here the Prologue leads, and it is
        // front matter, so the part divider is still the body start).
        let plain = part(
            &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
            "OEBPS/nav.xhtml",
        );
        let plm = &plain[plain.find("epub:type=\"landmarks\"").expect("landmarks")..];
        assert!(
            !plm.contains("epub:type=\"cover\""),
            "no cover landmark: {plm}"
        );
        assert!(plm.contains("epub:type=\"toc\""), "toc landmark present");
        assert!(
            plm.contains("epub:type=\"titlepage\""),
            "titlepage landmark present"
        );
        assert!(
            plm.contains("epub:type=\"bodymatter\""),
            "bodymatter landmark present"
        );
    }

    #[test]
    fn series_epub_nests_each_book_with_its_parts() {
        let book = |title: &str, body: &str| Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            half_title: false,
            series_books: Vec::new(),
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

    #[test]
    fn series_epub_reprints_each_books_copyright_not_just_the_first() {
        // #711: a series omnibus must not silently drop a later book's rights/ISBN.
        // The package-level OPF identifier/language and the single OPF-level
        // copyright page are series-level (the first book's), but each book's own
        // copyright is reprinted on its divider page — so both books' distinct
        // rights lines and ISBNs reach the omnibus.
        let book = |title: &str, publishing: PubMeta| Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            publishing,
            epigraph: None,
            half_title: false,
            series_books: Vec::new(),
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("Ch", None, "prose")],
            }],
            back: vec![],
        };
        let first = PubMeta {
            isbn: Some("978-0-00-000000-0".into()),
            rights: Some("© 2024 A. Writer".into()),
            ..Default::default()
        };
        let second = PubMeta {
            isbn: Some("978-1-11-111111-1".into()),
            rights: Some("© 2026 B. Author".into()),
            ..Default::default()
        };
        let bytes = to_epub_series(
            &[book("Book One", first), book("Book Two", second)],
            Some("The Saga"),
            Some("A. Writer"),
            MODIFIED,
            &ExportStyle::default(),
        )
        .unwrap();

        // The omnibus package identifier is the first book's ISBN (one per package);
        // the second book's ISBN is not the package id.
        let opf = part(&bytes, "OEBPS/content.opf");
        assert!(
            opf.contains("<dc:identifier id=\"bookid\">978-0-00-000000-0</dc:identifier>"),
            "package identifier is the first book's ISBN: {opf}"
        );

        // The divider files (the `book*` prefix) reprint each book's own copyright,
        // so no later book's rights/ISBN is dropped. Concatenate them and assert
        // both books' distinct notices are present.
        let dividers: String = {
            let zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
            let names: Vec<String> = zip
                .file_names()
                .filter(|n| {
                    n.contains("/book")
                        && Path::new(n)
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("xhtml"))
                })
                .map(str::to_string)
                .collect();
            names.iter().map(|n| part(&bytes, n)).collect()
        };
        assert!(
            dividers.contains("© 2024 A. Writer") && dividers.contains("ISBN: 978-0-00-000000-0"),
            "first book's copyright on its divider: {dividers}"
        );
        assert!(
            dividers.contains("© 2026 B. Author") && dividers.contains("ISBN: 978-1-11-111111-1"),
            "second book's copyright reprinted, not dropped: {dividers}"
        );
        assert!(
            dividers.matches("book-copyright").count() == 2,
            "each book's divider carries its own copyright block: {dividers}"
        );

        // A book with no publishing metadata gets no copyright block on its divider,
        // leaving the default output unchanged.
        let plain = to_epub_series(
            &[
                book("Book One", PubMeta::default()),
                book("Book Two", PubMeta::default()),
            ],
            Some("The Saga"),
            None,
            MODIFIED,
            &ExportStyle::default(),
        )
        .unwrap();
        let plain_dividers: String = {
            let zip = zip::ZipArchive::new(Cursor::new(plain.clone())).unwrap();
            let names: Vec<String> = zip
                .file_names()
                .filter(|n| {
                    n.contains("/book")
                        && Path::new(n)
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("xhtml"))
                })
                .map(str::to_string)
                .collect();
            names.iter().map(|n| part(&plain, n)).collect()
        };
        assert!(
            !plain_dividers.contains("book-copyright"),
            "no per-book copyright block without metadata: {plain_dividers}"
        );
    }
}

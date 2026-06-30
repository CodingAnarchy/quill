//! The EPUB package serializers — a distinct layer from the [`super::Builder`]
//! section-assembly logic. From the accumulated content [`Item`]s and the TOC
//! [`Nav`] tree, these render the OPF package document (#297), the EPUB 3
//! navigation document, and the EPUB 2 NCX for older readers. Re-exported into
//! [`super`] so `finish` calls them unqualified.

use super::{Item, Nav};
use crate::export::{xml_escape, EmbeddedFont, EmbeddedImage, ImageRole};

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
}

pub(super) fn build_opf(
    book: &OpfMeta,
    items: &[Item],
    fonts: &[EmbeddedFont],
    images: &[EmbeddedImage],
) -> String {
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
    // The EPUB 2 cover hint (#582): older readers (and many storefronts) find the
    // cover via this legacy `<meta name="cover">` pointing at the manifest item id.
    if images.iter().any(|i| i.role == ImageRole::Cover) {
        meta.push_str(&format!(
            "<meta name=\"cover\" content=\"{COVER_IMAGE_ID}\"/>"
        ));
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
    let mut spine = String::new();
    for it in items {
        manifest.push_str(&manifest_item(
            &it.id,
            &it.href,
            "application/xhtml+xml",
            None,
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

/// One OPF manifest `<item/>`: id, href, media type, and optional `properties`
/// (e.g. `nav` for the navigation document). Every manifest entry routes through
/// here so attribute order and escaping stay uniform — `id`/`href` are
/// XML-escaped even though current package-internal values have no special
/// characters.
fn manifest_item(id: &str, href: &str, media_type: &str, properties: Option<&str>) -> String {
    let props = match properties {
        Some(p) => format!(" properties=\"{}\"", xml_escape(p)),
        None => String::new(),
    };
    format!(
        "<item id=\"{}\" href=\"{}\" media-type=\"{}\"{props}/>",
        xml_escape(id),
        xml_escape(href),
        xml_escape(media_type),
    )
}

/// The EPUB 3 navigation document — a nested `<ol>` table of contents.
pub(super) fn build_nav(entries: &[Nav]) -> String {
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
    use std::io::{Cursor, Read};

    use crate::export::epub::{to_epub, to_epub_series};
    use crate::export::{Chapter, Document, ExportStyle, Part};
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
        let zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
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

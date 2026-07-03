//! Tests for the `.docx` writer, exercising both profiles end to end: the
//! produced zip's required parts, manuscript title/copyright/contents pages,
//! running header + TOC fields, emphasis runs, and the series omnibus.

use std::io::{Cursor, Read};

use super::*;
use crate::frontmatter::Epigraph;

fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
    Epigraph {
        text: text.to_string(),
        attribution: by.map(str::to_string),
    }
}

fn doc() -> Document {
    Document {
        title: "Red <State> Earth".into(),
        subtitle: None,
        author: Some("A. Writer".into()),
        series: None,
        publishing: Default::default(),
        epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
        front: vec![],
        parts: vec![crate::export::Part {
            name: Some("Part One".into()),
            chapters: vec![crate::export::Chapter {
                title: "The Find".into(),
                epigraph: None,
                body: "The locket *gleamed* and felt **heavy**.\n\n## A scene\n\nMore.".into(),
                role: None,
            }],
        }],
        back: vec![],
    }
}

/// Read one part's text out of the produced .docx zip.
fn part(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
    let mut file = zip.by_name(name).expect("part present");
    let mut s = String::new();
    file.read_to_string(&mut s).unwrap();
    s
}

#[test]
fn smart_typography_glyphs_pass_through_to_docx() {
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![crate::export::Part {
            name: None,
            chapters: vec![crate::export::Chapter {
                title: "C".into(),
                epigraph: None,
                body: "He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}".into(),
                role: None,
            }],
        }],
        back: vec![],
    };
    let bytes = to_docx(&d, Profile::Manuscript).unwrap();
    let xml = part(&bytes, "word/document.xml");
    assert!(
        xml.contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
        "curly glyphs reach the docx body"
    );
}

#[test]
fn a_fenced_block_renders_as_a_monospace_paragraph_with_breaks() {
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![crate::export::Part {
            name: None,
            chapters: vec![crate::export::Chapter {
                title: "C".into(),
                epigraph: None,
                body: "Before.\n\n```\nLOG one\nLOG two\n```\n\nAfter.".into(),
                role: None,
            }],
        }],
        back: vec![],
    };
    let xml = part(
        &to_docx(&d, Profile::Manuscript).unwrap(),
        "word/document.xml",
    );
    // Monospace font, both lines present, joined by a run-level break.
    assert!(xml.contains("Courier New"), "monospace font:\n{xml}");
    assert!(
        xml.contains("LOG one</w:t><w:br/><w:t xml:space=\"preserve\">LOG two"),
        "lines kept with a break:\n{xml}"
    );
}

#[test]
fn series_docx_carries_every_book_in_order() {
    let book = |title: &str| crate::export::Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![crate::export::Part {
            name: None,
            chapters: vec![crate::export::Chapter {
                title: "Ch".into(),
                epigraph: None,
                body: "prose".into(),
                role: None,
            }],
        }],
        back: vec![],
    };
    let bytes = to_docx_series(
        &[book("Book One"), book("Book Two")],
        Some("The Saga"),
        Some("A. Writer"),
        Profile::Manuscript,
    )
    .unwrap();
    let xml = part(&bytes, "word/document.xml");
    assert!(
        xml.contains("The Saga") && xml.contains("by A. Writer"),
        "series title page"
    );
    let one = xml.find("Book One").expect("book one");
    let two = xml.find("Book Two").expect("book two");
    assert!(one < two, "books in order");
}

#[test]
fn series_docx_reprints_each_books_copyright_not_just_the_first() {
    // #711: a series omnibus must not silently drop a later book's rights/ISBN.
    // The front-matter copyright page is series-level (the first book's), but
    // each book's own copyright is reprinted under its title — so both books'
    // distinct rights lines and ISBNs reach the document.
    let book = |title: &str, publishing: PubMeta| crate::export::Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
        publishing,
        epigraph: None,
        front: vec![],
        parts: vec![crate::export::Part {
            name: None,
            chapters: vec![crate::export::Chapter {
                title: "Ch".into(),
                epigraph: None,
                body: "prose".into(),
                role: None,
            }],
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
    let xml = part(
        &to_docx_series(
            &[book("Book One", first), book("Book Two", second)],
            Some("The Saga"),
            Some("A. Writer"),
            Profile::Manuscript,
        )
        .unwrap(),
        "word/document.xml",
    );
    // Both books' distinct rights lines and ISBNs are present, in reading order.
    for needle in [
        "© 2024 A. Writer",
        "ISBN: 978-0-00-000000-0",
        "© 2026 B. Author",
        "ISBN: 978-1-11-111111-1",
    ] {
        assert!(xml.contains(needle), "{needle} present: {xml}");
    }
    // The second book's copyright follows the second book's title (reprinted
    // under it, not dropped).
    let book_two = xml.find("Book Two").expect("second book title");
    let second_rights = xml.find("© 2026 B. Author").expect("second rights");
    assert!(
        book_two < second_rights,
        "second book's copyright is under its title: {xml}"
    );

    // The Vellum profile applies its own front matter — no generated copyright.
    let vellum = part(
        &to_docx_series(
            &[book("Book One", PubMeta::default())],
            Some("The Saga"),
            None,
            Profile::Vellum,
        )
        .unwrap(),
        "word/document.xml",
    );
    assert!(
        !vellum.contains("ISBN:") && !vellum.contains("Published by"),
        "no per-book copyright in the Vellum profile: {vellum}"
    );
}

#[test]
fn produces_a_zip_with_the_required_parts() {
    let bytes = to_docx(&doc(), Profile::Manuscript).unwrap();
    assert_eq!(&bytes[..2], b"PK", "a zip container");
    for name in [
        "[Content_Types].xml",
        "_rels/.rels",
        "word/styles.xml",
        "word/document.xml",
    ] {
        assert!(!part(&bytes, name).is_empty(), "{name} present");
    }
    // styles.xml is closed once, whichever profile.
    assert_eq!(
        part(&bytes, "word/styles.xml")
            .matches("</w:styles>")
            .count(),
        1
    );
}

#[test]
fn document_carries_structure_and_emphasis() {
    let xml = part(
        &to_docx(&doc(), Profile::Manuscript).unwrap(),
        "word/document.xml",
    );
    assert!(xml.contains("by A. Writer"));
    assert!(
        xml.contains("Part One") && xml.contains("The Find"),
        "headings present"
    );
    // Each chapter starts on a new page.
    assert!(xml.contains("<w:pageBreakBefore/>"), "chapter page break");
    // Markdown emphasis became real runs, not literal markup.
    assert!(
        xml.contains("<w:b/>") && xml.contains("<w:i/>"),
        "bold + italic runs"
    );
    assert!(!xml.contains('*'), "no literal asterisks: {xml}");
    // XML special characters in the title are escaped.
    assert!(xml.contains("Red &lt;State&gt; Earth"), "title escaped");
}

#[test]
fn body_paragraphs_carry_widow_orphan_control() {
    // Widow/orphan control (#650): each body paragraph sets `widowControl` so no
    // single line strands at a page top/bottom. The body stays ragged-right (no
    // justified `jc`) — manuscript format and Vellum import both expect that.
    for profile in [Profile::Manuscript, Profile::Vellum] {
        let xml = part(&to_docx(&doc(), profile).unwrap(), "word/document.xml");
        assert!(
            xml.contains("<w:widowControl/>"),
            "widowControl on body paragraphs"
        );
        // The body paragraph pairs widowControl with the first-line indent.
        assert!(
            xml.contains(r#"<w:widowControl/><w:ind w:firstLine="720"/>"#),
            "widowControl on the indented body paragraph: {xml}"
        );
        // Body is not justified (manuscript/Vellum convention).
        assert!(
            !xml.contains(r#"<w:jc w:val="both"/>"#),
            "body stays ragged-right, not justified"
        );
    }
}

#[test]
fn a_scene_break_becomes_a_centered_divider_not_literal_dashes() {
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![crate::export::Part {
            name: None,
            chapters: vec![crate::export::Chapter {
                title: "One".into(),
                epigraph: None,
                body: "Scene one.\n\n---\n\nScene two.".into(),
                role: None,
            }],
        }],
        back: vec![],
    };
    let xml = part(
        &to_docx(&d, Profile::Manuscript).unwrap(),
        "word/document.xml",
    );
    assert!(!xml.contains("---"), "no literal rule: {xml}");
    // A centered `#` divider sits between the two scenes.
    let one = xml.find("Scene one.").unwrap();
    let hash = xml
        .find(r#"<w:t xml:space="preserve">#</w:t>"#)
        .expect("# divider");
    let two = xml.find("Scene two.").unwrap();
    assert!(one < hash && hash < two, "divider between scenes: {xml}");
}

#[test]
fn a_copyright_page_renders_after_the_title_page() {
    // Publishing metadata (#583) renders a copyright page after the title page,
    // in reading order, carrying the rights line and publisher.
    let mut d = doc();
    d.publishing = PubMeta {
        publisher: Some("Quill & Co.".into()),
        date: Some("2026".into()),
        rights: Some("© 2026 A. Writer".into()),
        ..Default::default()
    };
    let xml = part(
        &to_docx(&d, Profile::Manuscript).unwrap(),
        "word/document.xml",
    );
    let author_at = xml.find("by A. Writer").expect("title page");
    let rights_at = xml.find("© 2026 A. Writer").expect("rights line");
    let body_at = xml.find("The Find").expect("first chapter");
    assert!(
        author_at < rights_at && rights_at < body_at,
        "copyright between title page and body: {xml}"
    );
    assert!(
        xml.contains("Published by Quill &amp; Co."),
        "publisher escaped: {xml}"
    );
    assert!(xml.contains("First published 2026"), "date line");

    // No publishing metadata → no copyright page (default output unchanged).
    let plain = part(
        &to_docx(&doc(), Profile::Manuscript).unwrap(),
        "word/document.xml",
    );
    assert!(!plain.contains("Published by"), "no copyright by default");

    // Vellum applies its own front matter → no generated copyright page.
    let vellum = part(&to_docx(&d, Profile::Vellum).unwrap(), "word/document.xml");
    assert!(
        !vellum.contains("Published by"),
        "no copyright page in the Vellum profile"
    );
}

#[test]
fn vellum_profile_uses_heading1_and_no_title_page() {
    let bytes = to_docx(&doc(), Profile::Vellum).unwrap();
    let styles = part(&bytes, "word/styles.xml");
    // The Heading 1 style Vellum splits chapters on is defined.
    assert!(
        styles.contains(r#"w:styleId="Heading1""#),
        "Heading1 style: {styles}"
    );
    assert!(
        styles.contains(r#"w:val="heading 1""#),
        "built-in heading 1 name"
    );
    let xml = part(&bytes, "word/document.xml");
    // Chapter and part titles are tagged Heading 1, in reading order.
    assert!(
        xml.contains(r#"<w:pStyle w:val="Heading1"/>"#),
        "headings styled: {xml}"
    );
    let part_at = xml.find("Part One").unwrap();
    let chapter_at = xml.find("The Find").unwrap();
    assert!(part_at < chapter_at, "reading order");
    // No manuscript title page (the author sets title/author in Vellum) and no
    // page-break-driven layout (Vellum reflows).
    assert!(!xml.contains("by A. Writer"), "no title page: {xml}");
    assert!(!xml.contains("<w:pageBreakBefore/>"), "no page breaks");
    // Emphasis still carries through as real runs.
    assert!(
        xml.contains("<w:b/>") && xml.contains("<w:i/>"),
        "bold + italic runs"
    );
    assert!(!xml.contains('*'), "no literal asterisks");
}

/// Whether the produced .docx zip contains a part by name.
fn has_part(bytes: &[u8], name: &str) -> bool {
    let zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    names.iter().any(|n| n == name)
}

#[test]
fn manuscript_profile_emits_a_running_header_with_a_page_field() {
    // #705: the manuscript profile carries a running header (author / title /
    // page) wired through the package — a header part, its content-type
    // override, a relationship, and a section reference.
    let bytes = to_docx(&doc(), Profile::Manuscript).unwrap();
    assert!(
        has_part(&bytes, "word/header1.xml"),
        "header part is written"
    );
    let header = part(&bytes, "word/header1.xml");
    // Running header carries author and title, right-aligned.
    assert!(header.contains("<w:hdr"), "header root element");
    assert!(
        header.contains("A. Writer /") && header.contains("Red &lt;State&gt; Earth /"),
        "author and (escaped) title in the header: {header}"
    );
    assert!(
        header.contains(r#"<w:jc w:val="right"/>"#),
        "header right-aligned"
    );
    // A live PAGE field, not a literal number.
    assert!(
        header.contains("<w:instrText xml:space=\"preserve\"> PAGE </w:instrText>"),
        "PAGE field instruction: {header}"
    );
    assert!(
        header.contains(r#"<w:fldChar w:fldCharType="begin"/>"#)
            && header.contains(r#"<w:fldChar w:fldCharType="end"/>"#),
        "field delimiters"
    );
    // The content-type override and relationship are declared, and the section
    // references the header.
    let ctypes = part(&bytes, "[Content_Types].xml");
    assert!(
        ctypes.contains(r#"PartName="/word/header1.xml""#),
        "header content-type override: {ctypes}"
    );
    let rels = part(&bytes, "word/_rels/document.xml.rels");
    assert!(
        rels.contains(r#"Target="header1.xml""#) && rels.contains(r#"Id="rId2""#),
        "header relationship: {rels}"
    );
    let xml = part(&bytes, "word/document.xml");
    assert!(
        xml.contains(r#"<w:headerReference w:type="default" r:id="rId2"/>"#),
        "section references the header: {xml}"
    );
    assert!(
        xml.contains(
            r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#
        ),
        "the r namespace is declared for the header id"
    );
}

#[test]
fn manuscript_profile_emits_a_toc_field_for_the_chapters() {
    // #705: a TOC field collects the chapter/part titles into a clickable,
    // updatable contents. The chapter headings carry outline level 1 so the
    // field picks them up.
    let bytes = to_docx(&doc(), Profile::Manuscript).unwrap();
    let xml = part(&bytes, "word/document.xml");
    assert!(
        xml.contains(r#"<w:instrText xml:space="preserve"> TOC \o "1-1" \h </w:instrText>"#),
        "TOC field instruction: {xml}"
    );
    // The Contents heading precedes the chapter body.
    let toc_at = xml.find("Contents").expect("contents heading");
    let chapter_at = xml.find("The Find").expect("first chapter");
    assert!(toc_at < chapter_at, "TOC before the body: {xml}");
    // Chapter/part headings are outline level 1 so the TOC can collect them.
    assert!(
        xml.contains(r#"<w:outlineLvl w:val="0"/>"#),
        "headings carry an outline level: {xml}"
    );
}

#[test]
fn vellum_profile_has_no_header_or_toc() {
    // Vellum applies its own front matter and running header — the generated
    // package must not add one, or it would conflict on import.
    let bytes = to_docx(&doc(), Profile::Vellum).unwrap();
    assert!(
        !has_part(&bytes, "word/header1.xml"),
        "no header part in the Vellum profile"
    );
    let ctypes = part(&bytes, "[Content_Types].xml");
    assert!(
        !ctypes.contains("header1.xml"),
        "no header content-type override"
    );
    let rels = part(&bytes, "word/_rels/document.xml.rels");
    assert!(!rels.contains("header1.xml"), "no header relationship");
    let xml = part(&bytes, "word/document.xml");
    assert!(
        !xml.contains("<w:headerReference"),
        "no header reference in the section"
    );
    assert!(!xml.contains(" TOC "), "no TOC field: {xml}");
    assert!(!xml.contains("Contents"), "no contents page");
}

#[test]
fn series_docx_carries_a_header_and_toc() {
    // #705: the series document (manuscript) also gets a running header
    // (series / author) and a TOC over the book/chapter titles.
    let book = |title: &str| crate::export::Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![crate::export::Part {
            name: None,
            chapters: vec![crate::export::Chapter {
                title: "Ch".into(),
                epigraph: None,
                body: "prose".into(),
                role: None,
            }],
        }],
        back: vec![],
    };
    let bytes = to_docx_series(
        &[book("Book One"), book("Book Two")],
        Some("The Saga"),
        Some("A. Writer"),
        Profile::Manuscript,
    )
    .unwrap();
    let header = part(&bytes, "word/header1.xml");
    assert!(
        header.contains("The Saga /") && header.contains("A. Writer /"),
        "series and author in the header: {header}"
    );
    let xml = part(&bytes, "word/document.xml");
    assert!(xml.contains(" TOC "), "series TOC field: {xml}");
    assert!(
        xml.contains(r#"<w:headerReference w:type="default" r:id="rId2"/>"#),
        "series header reference"
    );
}

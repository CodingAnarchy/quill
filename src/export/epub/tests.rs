//! Tests for the EPUB writer (#297): package structure, XHTML rendering,
//! `[style]` parity, embedded/subset fonts, images, `epub:type` semantics, and
//! landmarks. Split from `mod.rs` to keep both files under the size convention
//! (#716).

use super::*;
use crate::export::{Chapter, Document, ExportStyle, Part};
use crate::frontmatter::Role;
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
        role: None,
    }
}

/// A front/back-matter section carrying a specific [`Role`] (#695), so a test
/// can assert its `epub:type` reaches the output.
fn section(title: &str, role: Role, body: &str) -> Chapter {
    Chapter {
        title: title.to_string(),
        epigraph: None,
        body: body.to_string(),
        role: Some(role),
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
        publishing: PubMeta::default(),
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
        publishing: PubMeta::default(),
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
        publishing: PubMeta::default(),
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
        publishing: PubMeta::default(),
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
fn a_true_italic_sibling_embeds_and_gets_an_italic_font_face() {
    // A body font with a `-italic` sibling on disk (#638): both files embed, and
    // the stylesheet declares an upright and an italic `@font-face` for the same
    // family with a `400 700` weight range — so <em>/<strong> resolve to the
    // right face instead of the reader faux-styling one weight.
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![chapter("C", None, "Body *emph* and **bold**.")],
        }],
        back: vec![],
    };
    let mut style = ExportStyle {
        body_font: Some("fonts/eb-garamond.ttf".into()),
        ..Default::default()
    };
    style.resolve_fonts(|path| match path {
        "fonts/eb-garamond.ttf" => Ok(b"upright".to_vec()),
        "fonts/eb-garamond-italic.ttf" => Ok(b"italic".to_vec()),
        other => Err(std::io::Error::other(format!("no {other}"))),
    });
    let bytes = to_epub(&d, MODIFIED, &style).unwrap();

    // Both files embed under OEBPS/fonts/.
    assert!(
        names(&bytes).iter().any(|n| n == "OEBPS/fonts/body.ttf"),
        "upright body font"
    );
    assert!(
        names(&bytes)
            .iter()
            .any(|n| n == "OEBPS/fonts/body-italic.ttf"),
        "true-italic body font"
    );
    // The OPF declares both.
    let opf = part(&bytes, "OEBPS/content.opf");
    assert!(opf.contains("href=\"fonts/body.ttf\""), "{opf}");
    assert!(opf.contains("href=\"fonts/body-italic.ttf\""), "{opf}");
    // The stylesheet has an upright and an italic @font-face, each spanning the
    // regular→bold weight range.
    let css = part(&bytes, "OEBPS/style.css");
    assert!(
        css.contains("font-style: normal; font-weight: 400 700; src: url(\"fonts/body.ttf\")"),
        "upright face: {css}"
    );
    assert!(
        css.contains(
            "font-style: italic; font-weight: 400 700; src: url(\"fonts/body-italic.ttf\")"
        ),
        "italic face: {css}"
    );
}

/// Read one package entry as raw bytes (fonts and images aren't UTF-8).
fn part_bytes(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid zip");
    let mut file = zip.by_name(name).expect("entry present");
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    out
}

#[test]
fn embedded_fonts_are_subset_smaller_than_the_source() {
    // A real serif family is embedded; the package copy must be subset to the
    // book's glyphs (#709), so far smaller than the ~380 KB source, yet still
    // render every character the text uses — punctuation and the smart-typography
    // em-dash/curly quotes included.
    let source = include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf");
    let d = Document {
        title: "The Book".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![chapter(
                "One",
                None,
                "She paused — then, \u{201c}quietly,\u{201d} left. \u{2018}Café\u{2019}",
            )],
        }],
        back: vec![],
    };
    let mut style = ExportStyle {
        body_font: Some("fonts/body.ttf".into()),
        ..Default::default()
    };
    style.resolve_fonts(|_| Ok(source.to_vec()));
    let bytes = to_epub(&d, MODIFIED, &style).unwrap();

    let embedded = part_bytes(&bytes, "OEBPS/fonts/body.ttf");
    assert!(
        embedded.len() < source.len() / 4,
        "subset {} should be far smaller than source {}",
        embedded.len(),
        source.len()
    );
    // The subset still resolves every glyph actually used, via a rebuilt cmap.
    let face = ttf_parser::Face::parse(&embedded, 0).expect("subset parses");
    assert!(face.tables().cmap.is_some(), "subset keeps a cmap");
    for c in [
        'S', 'h', 'e', ',', '.', '\u{2014}', '\u{201c}', '\u{201d}', 'é',
    ] {
        assert!(face.glyph_index(c).is_some(), "subset covers {c:?}");
    }
}

#[test]
fn font_face_declarations_carry_a_format_hint() {
    // Each `@font-face src:` gets a `format(...)` hint matching the media type
    // (#709), so a reader that can't handle the format has a fallback signal.
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
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
    let css = part(&bytes, "OEBPS/style.css");
    assert!(
        css.contains("src: url(\"fonts/body.otf\") format(\"opentype\");"),
        "otf hint: {css}"
    );
    assert!(
        css.contains("src: url(\"fonts/heading.ttf\") format(\"truetype\");"),
        "ttf hint: {css}"
    );
}

#[test]
fn heading_alignment_and_drop_cap_render_via_css() {
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
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
    assert!(
        part(&bytes, "OEBPS/title0001.xhtml").contains("<img src=\"images/title.svg\" alt=\"\"/>")
    );
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
fn content_docs_carry_epub_type_structural_semantics() {
    // With a cover image and publishing metadata: cover → title → copyright →
    // front (Prologue) → part divider → chapter → back (Afterword). Each body
    // carries its EPUB 3 structural role (#604) and declares the epub namespace.
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

    let carries = |file: &str, kind: &str| {
        let x = part(&bytes, file);
        assert!(
            x.contains("xmlns:epub=\"http://www.idpf.org/2007/ops\""),
            "{file} declares the epub namespace: {x}"
        );
        assert!(
            x.contains(&format!("<body epub:type=\"{kind}\">")),
            "{file} is {kind}: {x}"
        );
    };
    carries("OEBPS/cover0001.xhtml", "cover");
    carries("OEBPS/title0002.xhtml", "titlepage");
    carries("OEBPS/copyright0003.xhtml", "copyright-page");
    carries("OEBPS/chap0004.xhtml", "frontmatter"); // Prologue
    carries("OEBPS/part0005.xhtml", "bodymatter"); // Part One divider
    carries("OEBPS/chap0006.xhtml", "bodymatter"); // The Find
    carries("OEBPS/chap0007.xhtml", "backmatter"); // Afterword
}

#[test]
fn role_sections_carry_their_specific_epub_type() {
    // A book with a dedication and a foreword up front and a glossary in back
    // (#695): each section's `<body>` carries its precise EPUB Structural
    // Semantics term — not the generic front/back-matter band — while an
    // untyped body chapter stays `bodymatter`.
    let d = Document {
        front: vec![
            section("For E.", Role::Dedication, "For everyone."),
            section("Foreword", Role::Foreword, "A word before."),
        ],
        back: vec![section("Glossary", Role::Glossary, "arcana — old magic.")],
        ..doc()
    };
    let bytes = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();

    let body_type = |file: &str| {
        let x = part(&bytes, file);
        let at = x.find("<body epub:type=\"").expect("a typed body") + 17;
        x[at..x[at..].find('"').map(|i| at + i).unwrap()].to_string()
    };
    // No cover / copyright page here: title → dedication → foreword → part →
    // chapter → glossary.
    assert_eq!(body_type("OEBPS/chap0002.xhtml"), "dedication");
    assert_eq!(body_type("OEBPS/chap0003.xhtml"), "foreword");
    assert_eq!(body_type("OEBPS/chap0006.xhtml"), "glossary");
    // An untyped body chapter keeps the generic body-matter semantic.
    assert_eq!(body_type("OEBPS/chap0005.xhtml"), "bodymatter");

    // The role sections also become landmarks pointing at their own docs, so a
    // reader can jump straight to the dedication / foreword / glossary.
    let nav = part(&bytes, "OEBPS/nav.xhtml");
    let lm = &nav[nav.find("epub:type=\"landmarks\"").expect("landmarks nav")..];
    assert!(
        lm.contains("epub:type=\"dedication\" href=\"chap0002.xhtml\""),
        "dedication landmark: {lm}"
    );
    assert!(
        lm.contains("epub:type=\"foreword\" href=\"chap0003.xhtml\""),
        "foreword landmark: {lm}"
    );
    assert!(
        lm.contains("epub:type=\"glossary\" href=\"chap0006.xhtml\""),
        "glossary landmark: {lm}"
    );
}

#[test]
fn a_role_without_a_distinct_epub_type_keeps_the_band_fallback() {
    // "Also by the author" and "about the author" have no distinct EPUB term
    // (#695): they stay generic `backmatter` and raise no role landmark, so an
    // untyped section is unchanged from before roles reached the writer.
    let d = Document {
        front: vec![],
        back: vec![section("Also by", Role::AlsoBy, "Other books.")],
        ..doc()
    };
    let bytes = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
    // title → part → chapter → also-by (backmatter).
    let also = part(&bytes, "OEBPS/chap0004.xhtml");
    assert!(
        also.contains("<body epub:type=\"backmatter\">"),
        "no distinct term falls back to the band: {also}"
    );
    let nav = part(&bytes, "OEBPS/nav.xhtml");
    let lm = &nav[nav.find("epub:type=\"landmarks\"").expect("landmarks nav")..];
    assert!(
        !lm.contains("also-by") && !lm.contains("\"also"),
        "no role landmark for a term-less section: {lm}"
    );
}

#[test]
fn body_css_justifies_hyphenates_and_controls_widows_and_orphans() {
    // Print/ebook readiness (#650): the body paragraph rule carries justification,
    // automatic hyphenation (with the -webkit- prefix for older WebKit readers), and
    // widow/orphan protection; inset epigraph blocks stay ragged, not justified.
    let css = part(
        &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
        "OEBPS/style.css",
    );
    let p_rule = css
        .lines()
        .find(|l| l.starts_with("p {"))
        .expect("the body paragraph rule");
    assert!(
        p_rule.contains("text-align: justify"),
        "justified: {p_rule}"
    );
    assert!(p_rule.contains("hyphens: auto"), "hyphenated: {p_rule}");
    assert!(
        p_rule.contains("-webkit-hyphens: auto"),
        "webkit hyphens: {p_rule}"
    );
    assert!(p_rule.contains("orphans: 2"), "orphans: {p_rule}");
    assert!(p_rule.contains("widows: 2"), "widows: {p_rule}");
    // Inset epigraph paragraphs are not justified (a set-apart block).
    assert!(
        css.contains(".epigraph p { text-align: initial"),
        "epigraphs stay ragged: {css}"
    );
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

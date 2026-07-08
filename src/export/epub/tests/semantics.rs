//! EPUB 3 structural semantics and landmarks (#604, #695): the `epub:type` on
//! each content doc's `<body>`, the precise role terms, and the band fallback
//! for role-less sections.

use super::*;

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
        half_title: false,
        series_books: Vec::new(),
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
        half_title: false,
        series_books: Vec::new(),
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

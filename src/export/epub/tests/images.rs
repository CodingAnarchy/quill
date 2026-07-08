//! Embedded images and their alt-text semantics (#818): placement/embedding,
//! author-supplied descriptions, the decorative `role="presentation"`
//! convention, and the cover's title fallback.

use super::*;

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
    // Title image on the title page — no author description, so it's declared
    // decorative (empty alt + role, #818) rather than a bare empty alt.
    assert!(part(&bytes, "OEBPS/title0001.xhtml")
        .contains("<img src=\"images/title.svg\" alt=\"\" role=\"presentation\"/>"));
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
fn author_supplied_alt_text_is_emitted_on_content_images() {
    // An author who describes an image (#818): the description rides through
    // `resolve_images` and lands on the `<img>` as `alt="…"`, XML-escaped — not
    // the decorative empty-alt convention.
    let mut style = ExportStyle {
        chapter_heading_image: Some("art/banner.jpg".into()),
        chapter_heading_image_alt: Some("A raven perched on a <broken> gate".into()),
        cover_image: Some("art/cover.png".into()),
        cover_image_alt: Some("A lone lighthouse under a storm".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(vec![1, 2, 3]));
    let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();

    // The chapter-heading image (on every chapter) carries the author's (escaped)
    // description, and does *not* fall back to the bare empty-alt content
    // convention. The "The Find" chapter opens with it.
    let chap = names(&bytes)
        .into_iter()
        .filter(|n| n.starts_with("OEBPS/chap"))
        .map(|n| part(&bytes, &n))
        .find(|x| x.contains("chapter-heading.jpg"))
        .expect("a chapter carries the heading image");
    assert!(
        chap.contains(
            "<img src=\"images/chapter-heading.jpg\" \
             alt=\"A raven perched on a &lt;broken&gt; gate\"/>"
        ),
        "described chapter image: {chap}"
    );
    assert!(
        !chap.contains("chapter-heading.jpg\" alt=\"\""),
        "a described image is not emitted as decorative: {chap}"
    );

    // The cover uses the author's description rather than the old generic
    // `alt="Cover"`.
    let cover = part(&bytes, "OEBPS/cover0001.xhtml");
    assert!(
        cover.contains("alt=\"A lone lighthouse under a storm\""),
        "cover carries author description: {cover}"
    );
    assert!(!cover.contains("alt=\"Cover\""), "generic cover alt gone");

    // The advertised a11y metadata now truthfully includes text alternatives.
    assert!(
        part(&bytes, "OEBPS/content.opf")
            .contains("<meta property=\"schema:accessibilityFeature\">alternativeText</meta>"),
        "alternativeText advertised"
    );
}

#[test]
fn a_decorative_image_uses_the_presentation_convention_not_a_bare_empty_alt() {
    // With no author description (#818), an ornamental image is *declared*
    // decorative: empty `alt` paired with `role="presentation"`, so it reads as
    // ornamental rather than an undescribed content image.
    let mut style = ExportStyle {
        scene_break_image: Some("art/break.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(vec![1, 2, 3]));
    let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();

    let chap = part(&bytes, "OEBPS/chap0004.xhtml");
    assert!(
        chap.contains("<img src=\"images/scene-break.png\" alt=\"\" role=\"presentation\"/>"),
        "decorative scene break: {chap}"
    );

    // No cover / described image → the alternativeText feature is not advertised.
    assert!(
        !part(&bytes, "OEBPS/content.opf")
            .contains("<meta property=\"schema:accessibilityFeature\">alternativeText</meta>"),
        "alternativeText not advertised for a purely decorative image"
    );
}

#[test]
fn a_cover_without_a_description_falls_back_to_the_book_title() {
    // A cover is content, not decoration: with no `cover_image_alt`, its `alt`
    // falls back to the book title (#818) — never a bare empty alt or the old
    // generic "Cover".
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(vec![0x89, b'P', b'N', b'G']));
    let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();

    let cover = part(&bytes, "OEBPS/cover0001.xhtml");
    // doc()'s title is "Red <State> Earth" — the fallback is XML-escaped.
    assert!(
        cover.contains("alt=\"Red &lt;State&gt; Earth\""),
        "cover falls back to the escaped title: {cover}"
    );
    assert!(!cover.contains("alt=\"Cover\""), "generic cover alt gone");
    assert!(
        !cover.contains("alt=\"\""),
        "cover never emits a bare empty alt: {cover}"
    );
}

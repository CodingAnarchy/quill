//! Bookmark-outline, copyright-page, Contents-page, and folio tests for the PDF
//! writer (#583/#603/#651/#711).
use super::*;

#[test]
fn builds_a_nested_bookmark_outline() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        has(&bytes, "/PageMode /UseOutlines"),
        "opens with the outline"
    );
    for title in [
        "/Title (Prologue)",
        "/Title (Part One)",
        "/Title (The Find)",
        "/Title (Afterword)",
    ] {
        assert!(has(&bytes, title), "{title} bookmarked");
    }
    let at = s.find("/Title (Part One)").unwrap();
    let item = &s[at..at + 200.min(s.len() - at)];
    assert!(
        item.contains("/First ") && item.contains("/Count 1"),
        "part nests its chapter: {item}"
    );
}
#[test]
fn a_series_bookmarks_every_book_in_order() {
    let book = |title: &str| Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch("Ch", None, "Prose.")],
        }],
        back: vec![],
    };
    let bytes = to_pdf_series(
        &[book("Book One"), book("Book Two")],
        Some("The Saga"),
        Some("A. Writer"),
        &plain(),
    )
    .unwrap();
    let s = String::from_utf8_lossy(&bytes);
    let one = s.find("/Title (Book One)").expect("book one bookmarked");
    let two = s.find("/Title (Book Two)").expect("book two bookmarked");
    assert!(one < two, "books in reading order");
}
#[test]
fn a_series_reprints_each_books_copyright_not_just_the_first() {
    // #711: a series omnibus must not silently drop a later book's rights/ISBN. The
    // omnibus's front-matter copyright page is series-level (the first book's), but
    // each book's own copyright is reprinted on its divider page. So a series whose
    // *second* book carries copyright metadata draws strictly more page-content text
    // (Identity-H glyph runs shown with `Tj`) than the same series with no per-book
    // metadata — proof the later book's copyright reached the layout.
    let book = |title: &str, publishing: PubMeta| Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
        publishing,
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch("Ch", None, "Prose.")],
        }],
        back: vec![],
    };
    let bare = || book("Book One", PubMeta::default());
    // Only the *second* book carries metadata, so a first-book-only reader would
    // drop it — the regression this guards.
    let second_meta = PubMeta {
        isbn: Some("978-1-11-111111-1".into()),
        rights: Some("© 2026 B. Author. All rights reserved.".into()),
        ..Default::default()
    };

    let with_meta = to_pdf_series(
        &[bare(), book("Book Two", second_meta)],
        Some("The Saga"),
        Some("A. Writer"),
        &plain(),
    )
    .unwrap();
    let without = to_pdf_series(
        &[bare(), book("Book Two", PubMeta::default())],
        Some("The Saga"),
        Some("A. Writer"),
        &plain(),
    )
    .unwrap();

    let shown = |bytes: &[u8]| count(content(bytes).as_bytes(), " Tj\n");
    assert!(
        shown(&with_meta) > shown(&without),
        "the second book's copyright is reprinted, not dropped: {} vs {}",
        shown(&with_meta),
        shown(&without),
    );
}
#[test]
fn a_copyright_page_adds_a_front_matter_page() {
    // Publishing metadata (#583) renders a copyright page after the title page:
    // one extra page over the no-metadata baseline, in a still-valid PDF.
    let mut d = doc();
    d.publishing = PubMeta {
        publisher: Some("Quill & Co.".into()),
        date: Some("2026".into()),
        rights: Some("© 2026 A. Writer".into()),
        ..Default::default()
    };
    let bytes = to_pdf(&d, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
    let baseline = count(&to_pdf(&doc(), &plain()).unwrap(), "/Type /Page /Parent");
    let with_copyright = count(&bytes, "/Type /Page /Parent");
    assert_eq!(
        with_copyright,
        baseline + 1,
        "copyright adds a front-matter page"
    );

    // No publishing metadata → no extra page (default output unchanged).
    assert_eq!(
        count(&to_pdf(&doc(), &plain()).unwrap(), "/Type /Page /Parent"),
        baseline,
        "no copyright page by default"
    );
}
#[test]
fn a_contents_page_lists_titles_with_folios() {
    // The front matter carries a printed "Contents" page (#603): the heading, every
    // section title, and — for chapters/front/back, not the part heading — a folio (a
    // page label = page index + 1). Content is Identity-H glyph ids, so assert on the
    // page count and that the outline folios the Contents shows are the real body pages.
    let d = doc();
    let bytes = to_pdf(&d, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));

    // The Contents page is a real extra front-matter page: title + prologue + part +
    // chapter + afterword = 5 section pages, plus the one Contents page = 6.
    assert_eq!(
        count(&bytes, "/Type /Page /Parent"),
        6,
        "a Contents page joins the front matter"
    );

    // The Contents folios come from the same outline the bookmarks use; reserving the
    // TOC first makes the body land after it. Rebuild the outline the way render_book
    // does to read the folios the Contents page shows.
    let outline = {
        let mut pdf = Pdf::new(
            Layout::screen(&plain()),
            StyleBits::from_style(&plain()),
            vec![],
        );
        pdf.title_page(None, "T", None, None, None);
        pdf.reserve_contents(&doc_toc_titles(&d));
        book_outline(&mut pdf, &d)
    };
    // Four entries total (prologue, part, chapter, afterword); the part carries no
    // folio, so three folios are shown.
    let all = toc_entries(&outline);
    assert_eq!(all.len(), 4, "prologue, part, chapter, afterword");
    let folios: Vec<usize> = all.into_iter().flatten().collect();
    assert_eq!(folios.len(), 3, "the part has no folio");
    assert!(
        folios.iter().all(|&p| p >= 2),
        "the body follows the reserved front matter, got {folios:?}"
    );
}
#[test]
fn the_contents_page_reserves_body_pagination() {
    // Reserving the Contents page before the body means the body's folios already
    // account for it — the first body section starts after the reserved page.
    let d = doc();
    let mut pdf = Pdf::new(
        Layout::screen(&plain()),
        StyleBits::from_style(&plain()),
        vec![],
    );
    pdf.title_page(None, "T", None, None, None);
    let toc_start = pdf.reserve_contents(&doc_toc_titles(&d)).unwrap().start;
    let prologue = pdf.chapter(&d.front[0]);
    assert!(
        prologue > toc_start,
        "the body follows the reserved Contents page ({prologue} > {toc_start})"
    );
}
#[test]
fn a_long_contents_flows_onto_a_second_page() {
    // More chapters than fit on one Contents page flow onto a second reserved page —
    // the plan and the reservation agree on the page count, one folio line per entry.
    let many: Vec<Chapter> = (0..60)
        .map(|i| ch(&format!("Chapter Number {i}"), None, "Body."))
        .collect();
    let d = Document {
        title: "Big".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: many,
        }],
        back: vec![],
    };
    let (w, h) = trim_points("6x9").unwrap();
    let mut pdf = Pdf::new(
        Layout::print(w, h, 0.0, &plain()),
        StyleBits::from_style(&plain()),
        vec![],
    );
    pdf.title_page(None, "Big", None, None, None);
    let titles = doc_toc_titles(&d);
    let plan = pdf.reserve_contents(&titles).unwrap();
    let pages = plan.lines.iter().map(|l| l.page).max().unwrap() + 1;
    assert!(
        pages >= 2,
        "60 chapters overflow one Contents page, got {pages}"
    );
    assert_eq!(
        plan.lines
            .iter()
            .filter(|l| l.folio_entry.is_some())
            .count(),
        titles.len(),
        "one folio line per entry across the pages"
    );
    // And the whole book still serializes to a well-formed PDF.
    let bytes = to_pdf_print(&d, w, h, 0.0, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
}
#[test]
fn roman_numerals_render_the_conventional_forms() {
    // The front-matter folio converter (#651): the compact subtractive forms.
    for (n, s) in [
        (1, "i"),
        (2, "ii"),
        (3, "iii"),
        (4, "iv"),
        (5, "v"),
        (9, "ix"),
        (10, "x"),
        (14, "xiv"),
        (40, "xl"),
        (49, "xlix"),
        (90, "xc"),
        (2026, "mmxxvi"),
    ] {
        assert_eq!(roman(n), s, "{n} → {s}");
    }
    assert_eq!(roman(0), "", "zero has no numeral");
}
/// Lay out `d` the way `render_book` does — title, copyright, reserved Contents, then
/// the body — so `folio_label` reflects the real front/body split (#651). Returns the
/// laid-out `Pdf` and the per-entry folio page indices the Contents shows.
fn laid_out(d: &Document) -> (Pdf, Vec<Option<usize>>) {
    let (w, h) = trim_points("6x9").unwrap();
    let mut pdf = Pdf::new(
        Layout::print(w, h, 0.0, &plain()),
        StyleBits::from_style(&plain()),
        vec![],
    );
    pdf.title_page(
        d.series.as_deref(),
        &d.title,
        d.subtitle.as_deref(),
        d.author.as_deref(),
        d.epigraph.as_ref(),
    );
    pdf.copyright_page(&d.publishing);
    pdf.reserve_contents(&doc_toc_titles(d));
    pdf.begin_body();
    pdf.outline = book_outline(&mut pdf, d);
    let folios = toc_entries(&pdf.outline);
    (pdf, folios)
}
#[test]
fn front_matter_folios_are_roman_and_the_body_restarts_at_arabic_one() {
    // The #651 scheme: the title page is unnumbered, the following front-matter pages
    // (copyright, Contents) carry roman folios, and the first body page is Arabic "1".
    let mut d = doc();
    d.publishing = PubMeta {
        rights: Some("© 2026 A. Writer".into()),
        ..Default::default()
    };
    let (pdf, _) = laid_out(&d);
    // Page 0 is the title page (opener, non-blank): counted but unnumbered.
    assert_eq!(pdf.folio_label(0), None, "title page unnumbered");
    // The pages before the body are all roman (or unnumbered), never Arabic.
    for i in 0..pdf.body_start {
        if let Some(label) = pdf.folio_label(i) {
            assert!(
                label.chars().all(|c| "ivxlcdm".contains(c)),
                "front-matter folio {label:?} at page {i} is roman"
            );
        }
    }
    // At least one front-matter page actually shows a roman folio (copyright/Contents).
    assert!(
        (0..pdf.body_start).any(|i| pdf.folio_label(i).as_deref() == Some("ii")),
        "a front-matter page is folio ii"
    );
    // The first numbered body page (a recto start may insert a blank verso first) is
    // Arabic 1.
    let first_body = (pdf.body_start..pdf.meta.len())
        .find(|&i| !pdf.meta[i].blank)
        .expect("a body page");
    assert_eq!(
        pdf.folio_label(first_body),
        Some("1".to_string()),
        "the body starts at Arabic 1"
    );
    // No body page carries a roman folio.
    for i in pdf.body_start..pdf.meta.len() {
        if let Some(label) = pdf.folio_label(i) {
            assert!(
                label.chars().all(|c| c.is_ascii_digit()),
                "body folio {label:?} at page {i} is Arabic"
            );
        }
    }
}
#[test]
fn blank_and_cover_pages_carry_no_folio() {
    // Recto starts insert blank versos, and a cover leads page 0 (#651/#582): none of
    // those pages take a folio, and skipping them keeps the count contiguous.
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(png_bytes(600, 900, 255)));
    let (w, h) = trim_points("6x9").unwrap();
    let mut pdf = Pdf::new(
        Layout::print(w, h, 0.0, &style),
        StyleBits::from_style(&style),
        decode_images(&style),
    );
    let d = doc();
    pdf.cover_page();
    pdf.title_page(None, &d.title, None, d.author.as_deref(), None);
    pdf.copyright_page(&d.publishing);
    pdf.reserve_contents(&doc_toc_titles(&d));
    pdf.begin_body();
    pdf.outline = book_outline(&mut pdf, &d);
    // Every blank page (cover + inserted blank versos) is unnumbered.
    for (i, m) in pdf.meta.iter().enumerate() {
        if m.blank {
            assert_eq!(pdf.folio_label(i), None, "blank page {i} unnumbered");
        }
    }
    // The Arabic body folios are contiguous (1, 2, 3, …) despite the blank versos.
    let body: Vec<String> = (pdf.body_start..pdf.meta.len())
        .filter_map(|i| pdf.folio_label(i))
        .collect();
    let expected: Vec<String> = (1..=body.len()).map(|n| n.to_string()).collect();
    assert_eq!(body, expected, "body folios run 1,2,3,… skipping blanks");
}
#[test]
fn contents_folios_use_the_body_scheme() {
    // The printed Contents lists each entry's folio under the same scheme as the page
    // decoration (#651): body chapters land on Arabic labels, not physical indices.
    let (pdf, folios) = laid_out(&doc());
    let labels: Vec<String> = folios
        .iter()
        .filter_map(|f| f.and_then(|i| pdf.folio_label(i)))
        .collect();
    // Prologue, chapter, afterword each get a folio (the part heading doesn't).
    assert_eq!(labels.len(), 3, "three entries carry a folio");
    // Every Contents folio is Arabic (the body), matching the decoration scheme.
    for label in &labels {
        assert!(
            label.chars().all(|c| c.is_ascii_digit()),
            "Contents folio {label:?} is Arabic"
        );
    }
    // The prologue (first body section) is folio 1, and the folios ascend.
    assert_eq!(labels[0], "1", "the first body entry is folio 1");
    let nums: Vec<usize> = labels.iter().map(|l| l.parse().unwrap()).collect();
    assert!(
        nums.windows(2).all(|w| w[0] < w[1]),
        "Contents folios ascend, got {nums:?}"
    );
    // The label matches what the page itself is numbered.
    for (entry, label) in folios.iter().flatten().zip(&labels) {
        assert_eq!(
            pdf.folio_label(*entry).as_deref(),
            Some(label.as_str()),
            "Contents folio matches the page's own folio"
        );
    }
}

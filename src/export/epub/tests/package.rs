//! Package/container structure and OPF-level metadata (#297): the OCF
//! `mimetype` entry, the required package parts, and the stable identifier.

use super::*;

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

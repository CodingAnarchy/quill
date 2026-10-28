//! The `.docx` package plumbing: the content-types manifest, the package and
//! document relationships, the styles part, and the zip assembly that wraps a
//! built `word/document.xml` (plus, for the manuscript profile, a running-header
//! part) into the final container.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use super::{Header, Profile};

/// The content-types manifest, minus its closing tag. The manuscript profile
/// appends the header override (see [`content_types`]); this head declares the
/// always-present parts.
const CONTENT_TYPES_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>"#;

/// The header part's content-type override, added only for the manuscript profile.
const CONTENT_TYPES_HEADER: &str = r#"<Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/>"#;

const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

/// The document's relationships, minus its closing tag (always the styles part).
/// The manuscript profile appends the header relationship (see [`doc_rels`]).
const DOC_RELS_HEAD: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#;

/// The header relationship, added only for the manuscript profile. `rId2` is
/// referenced by the `<w:headerReference>` in the section properties.
const DOC_RELS_HEADER: &str = r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>"#;

/// Document defaults: Times New Roman 12pt, double-spaced — the manuscript-format
/// baseline that body paragraphs inherit and headings/epigraphs override.
const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:line="480" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>"#;

/// A `Normal` base plus the canonical built-in "heading 1" paragraph style.
/// Vellum starts a new chapter at each Heading 1, so the [`Profile::Vellum`]
/// writer tags chapter/part titles with it; the full canonical form (the
/// `Heading1` id, the "heading 1" name, `basedOn`/`qFormat`/`outlineLvl`) is what
/// makes word processors recognize it as *the* Heading 1 rather than a lookalike.
const VELLUM_STYLES: &str = r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:uiPriority w:val="9"/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>"#;

/// Zip the `.docx` package around a built `word/document.xml`. The manuscript
/// profile adds a `word/header1.xml` part (with its content-type + relationship);
/// Vellum omits it.
pub(super) fn build_docx(
    document_xml: String,
    profile: Profile,
    header: Header,
) -> std::io::Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    // Vellum needs a Heading 1 style to recognize chapter breaks; the manuscript
    // profile adds nothing to the base styles.
    let extra = match profile {
        Profile::Manuscript => "",
        Profile::Vellum => VELLUM_STYLES,
    };
    let styles = format!("{STYLES}{extra}</w:styles>");
    let mut parts = vec![
        ("[Content_Types].xml".to_string(), content_types(&header)),
        ("_rels/.rels".to_string(), RELS.to_string()),
        (
            "word/_rels/document.xml.rels".to_string(),
            doc_rels(&header),
        ),
        ("word/styles.xml".to_string(), styles),
        ("word/document.xml".to_string(), document_xml),
    ];
    if let Some(header) = header {
        parts.push(("word/header1.xml".to_string(), header));
    }
    for (name, content) in parts {
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        zip.write_all(content.as_bytes())?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

/// The `[Content_Types].xml`, with the header override when a header is present.
fn content_types(header: &Header) -> String {
    let extra = if header.is_some() {
        CONTENT_TYPES_HEADER
    } else {
        ""
    };
    format!("{CONTENT_TYPES_HEAD}{extra}</Types>")
}

/// The document relationships, with the header relationship when present.
fn doc_rels(header: &Header) -> String {
    let extra = if header.is_some() {
        DOC_RELS_HEADER
    } else {
        ""
    };
    format!("{DOC_RELS_HEAD}{extra}</Relationships>")
}

/// The `word/header1.xml` part wrapping one header paragraph. Its own `<w:hdr>`
/// root with the wordprocessing namespace, as a header part requires.
pub(super) fn header_xml(paragraph: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
            "<w:hdr xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "{paragraph}</w:hdr>"
        ),
        paragraph = paragraph,
    )
}

//! The bundled grayscale ICC profile and the PDF/X output intent it feeds (#817).
//!
//! A print-ready novel interior is grayscale (black text, gray-flattened ornaments,
//! no transparency, fonts already subset + embedded), which makes it an ideal PDF/X
//! candidate. To reach conformance we add an `/OutputIntent` of subtype `/GTS_PDFX`
//! that names a *destination profile* — an embedded ICC characterizing the color the
//! file is prepared for. For a grayscale interior that profile is a calibrated
//! grayscale space, so the writer bundles [`GRAY_ICC`] and streams it as the output
//! intent's `/DestOutputProfile`.
//!
//! Conformance level: **PDF/X-4** (#1303), which IngramSpark and KDP both accept
//! (IngramSpark's file guide lists PDF/X-1a and PDF/X-4, not the X-3 the writer
//! used to declare, #817). PDF/X-4 permits calibrated (ICC-based) grayscale, which
//! is exactly this interior: DeviceGray samples plus a grayscale output intent, no
//! uncalibrated RGB, no transparency, all fonts embedded. Beyond the output intent,
//! [`add_pdfx`] gives the file what PDF/X-4 requires of every conforming file: an
//! XMP metadata stream naming the version (`pdfxid:GTS_PDFXVersion`), matching
//! `/Info` entries, `/Trapped`, creation and modification dates, and (in
//! [`super::write::serialize`]) a PDF 1.6 header and a trailer `/ID`.
//!
//! [`GRAY_ICC`] is a minimal, public-domain grayscale display profile (gamma 2.2, D50
//! white point) generated for Quill — no third-party licence rides along in the export.

/// A minimal public-domain grayscale ICC profile (gamma 2.2, D50 white point),
/// embedded as the PDF/X output intent's destination profile (#817). Generated for
/// Quill; carries a "Public Domain" copyright tag so the exported PDF stays freely
/// distributable.
pub(super) const GRAY_ICC: &[u8] = include_bytes!("../../../../../assets/icc/QuillGray.icc");

/// The PDF/X version recorded in `/Info` and the XMP metadata (`GTS_PDFXVersion`),
/// which a PDF/X reader keys off to select validation (#817, #1303).
pub(super) const PDFX_VERSION: &str = "PDF/X-4";

/// The human-readable name of the output (print) condition the interior is prepared
/// for (#817), recorded on the output intent as `/OutputCondition` and, absent a
/// registered ICC characterization name, as `/OutputConditionIdentifier`.
pub(super) const OUTPUT_CONDITION: &str = "Quill Grayscale (gamma 2.2)";

use std::fmt::Write as _;

use time::OffsetDateTime;

use super::font::{pdf_text_string, take, ObjMap};
use super::write::flate_stream;

/// What [`add_pdfx`] added, for the caller to put in the catalog and `/Info`.
pub(super) struct PdfX {
    /// Entries for the catalog: the output intent and the XMP metadata.
    pub(super) catalog: String,
    /// Entries for `/Info`: the PDF/X version, `/Trapped`, and the dates.
    pub(super) info: String,
}

/// Add the objects a PDF/X-4 file needs beyond its pages (#817, #1303): the
/// embedded grayscale ICC profile, the `/GTS_PDFX` output intent naming it, and
/// the XMP metadata stream. `title` and `author` must be the ones in `/Info`,
/// which PDF/X requires the XMP to match. Returns the entries the catalog and
/// `/Info` need to point at them.
pub(super) fn add_pdfx(objects: &mut ObjMap, next: &mut u32, title: &str, author: &str) -> PdfX {
    // The ICC profile stream carries `/N 1` (a 1-component, grayscale space) so a
    // reader knows the destination colour space without parsing the profile.
    let icc_id = take(next);
    objects.insert(icc_id, flate_stream(GRAY_ICC, " /N 1"));
    let intent_id = take(next);
    objects.insert(
        intent_id,
        format!(
            "<< /Type /OutputIntent /S /GTS_PDFX \
             /OutputConditionIdentifier {ident} /OutputCondition {cond} \
             /Info {ident} /DestOutputProfile {icc_id} 0 R >>",
            ident = pdf_text_string(OUTPUT_CONDITION),
            cond = pdf_text_string(OUTPUT_CONDITION),
        )
        .into_bytes(),
    );
    let now = OffsetDateTime::now_utc();
    let xmp = xmp_packet(title, author, now);
    // The metadata stream stays uncompressed, so print tools that scan for the
    // packet find it, as PDF/X expects.
    let meta_id = take(next);
    let mut stream = format!(
        "<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n",
        xmp.len()
    )
    .into_bytes();
    stream.extend_from_slice(xmp.as_bytes());
    stream.extend_from_slice(b"\nendstream");
    objects.insert(meta_id, stream);

    let date = pdf_text_string(&pdf_date(now));
    let mut info = String::new();
    let _ = write!(
        info,
        " /GTS_PDFXVersion {} /Trapped /False /CreationDate {date} /ModDate {date}",
        pdf_text_string(PDFX_VERSION)
    );
    PdfX {
        catalog: format!(" /OutputIntents [{intent_id} 0 R] /Metadata {meta_id} 0 R"),
        info,
    }
}

/// A PDF date string, `D:YYYYMMDDHHmmSS+00'00'`, in UTC.
fn pdf_date(at: OffsetDateTime) -> String {
    format!(
        "D:{:04}{:02}{:02}{:02}{:02}{:02}+00'00'",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// The XMP metadata packet for a PDF/X-4 file: the version, the same title,
/// author, producer, and dates as `/Info`, `Trapped` false, and the document
/// identifiers PDF/X requires.
fn xmp_packet(title: &str, author: &str, at: OffsetDateTime) -> String {
    let date = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    let id = document_uuid(title, at);
    let mut dc = String::new();
    if !title.is_empty() {
        let _ = write!(
            dc,
            "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>",
            xml_escape(title)
        );
    }
    if !author.is_empty() {
        let _ = write!(
            dc,
            "<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>",
            xml_escape(author)
        );
    }
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" \
         xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" \
         xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\" \
         xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\">\
         <dc:format>application/pdf</dc:format>{dc}\
         <xmp:CreateDate>{date}</xmp:CreateDate>\
         <xmp:ModifyDate>{date}</xmp:ModifyDate>\
         <xmp:MetadataDate>{date}</xmp:MetadataDate>\
         <xmp:CreatorTool>Quill</xmp:CreatorTool>\
         <pdf:Producer>Quill</pdf:Producer>\
         <pdf:Trapped>False</pdf:Trapped>\
         <xmpMM:DocumentID>uuid:{id}</xmpMM:DocumentID>\
         <xmpMM:InstanceID>uuid:{id}</xmpMM:InstanceID>\
         <xmpMM:VersionID>1</xmpMM:VersionID>\
         <xmpMM:RenditionClass>default</xmpMM:RenditionClass>\
         <pdfxid:GTS_PDFXVersion>{PDFX_VERSION}</pdfxid:GTS_PDFXVersion>\
         </rdf:Description></rdf:RDF></x:xmpmeta>\n\
         <?xpacket end=\"w\"?>"
    )
}

/// A UUID-shaped identifier for the document, from its title and the time.
fn document_uuid(title: &str, at: OffsetDateTime) -> String {
    use std::hash::{Hash, Hasher};
    let half = |salt: u8| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (salt, title, at.unix_timestamp_nanos()).hash(&mut h);
        h.finish()
    };
    let (a, b) = (half(1), half(2));
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        b >> 52,
        b & 0xffff_ffff_ffff
    )
}

/// Escape text for an XML element.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

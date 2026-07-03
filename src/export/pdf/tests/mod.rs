//! Tests for the PDF writer: the screen proof (#463), the print interior (#464),
//! `[style]` parity (#489), image ornaments (#490), and full-Unicode Type0 fonts
//! (#492). Content text is emitted as Identity-H glyph ids, so these assert on PDF
//! structure (raw byte markers) and the font-routing logic rather than grepping the
//! rendered words; visual/qpdf validation is covered in the PR.

use std::io::Read;

use flate2::read::ZlibDecoder;

use super::*;
use crate::export::{Chapter, ExportStyle, Part, PubMeta};

mod font;
mod images;
mod layout;
mod toc;

/// The default (no `[style]`) baseline.
fn plain() -> ExportStyle {
    ExportStyle::default()
}
fn ep(text: &str, by: Option<&str>) -> Epigraph {
    Epigraph {
        text: text.into(),
        attribution: by.map(str::to_string),
    }
}
fn ch(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
    Chapter {
        title: title.into(),
        epigraph,
        body: body.into(),
        role: None,
    }
}
/// A book exercising every section kind: front matter, a named part with one chapter
/// (epigraph + italic + bold + a scene break + a sub-heading), and back matter.
fn doc() -> Document {
    Document {
        title: "Red State Earth".into(),
        subtitle: Some("a novel".into()),
        author: Some("A. Writer".into()),
        series: Some("The Cycle".into()),
        publishing: PubMeta::default(),
        epigraph: Some(ep("In the beginning.", Some("Anon"))),
        front: vec![ch("Prologue", None, "Before it all.")],
        parts: vec![Part {
            name: Some("Part One".into()),
            chapters: vec![ch(
                "The Find",
                Some(ep("A single line.", None)),
                "The locket *gleamed* and felt **heavy**.\n\n## A turn\n\nScene one.\n\n---\n\nScene two.",
            )],
        }],
        back: vec![ch("Afterword", None, "After it all.")],
    }
}
/// Whether the raw PDF bytes contain `needle` (avoids lossy-UTF-8 offset issues from
/// the embedded binary font streams).
fn has(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
}
fn count(bytes: &[u8], needle: &str) -> usize {
    bytes
        .windows(needle.len())
        .filter(|w| *w == needle.as_bytes())
        .count()
}
/// The concatenated, inflated page content streams as text. Content streams ship
/// FlateDecode-compressed (#662), so tests that assert on drawing operators (`Tj`,
/// `Tm`, `cm`, `Do`, `re W n`) inflate them first rather than grepping raw bytes.
fn content(bytes: &[u8]) -> String {
    let mut s = String::new();
    for (dict, body) in flate_streams(bytes) {
        // Content streams have a bare `<< /Filter … /Length … >>`; the font program
        // carries `/Length1`. Skip the font so the text is real page content.
        if !dict.contains("/Length1") {
            s.push_str(&String::from_utf8_lossy(&body));
        }
    }
    s
}
/// Inflate every FlateDecode stream in `bytes` and return each `(dict, inflated)`
/// pair, where `dict` is the stream's `<< … >>` dictionary text. Walks the raw PDF
/// object by object: each `stream\n … \nendstream` payload preceded by a dict that
/// names `/Filter /FlateDecode` is decompressed with zlib inflate.
fn flate_streams(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let open = b"stream\n";
    let close = b"\nendstream";
    let mut i = 0;
    while let Some(rel) = bytes[i..].windows(open.len()).position(|w| w == open) {
        let dict_end = i + rel;
        let payload = dict_end + open.len();
        let end_rel = bytes[payload..]
            .windows(close.len())
            .position(|w| w == close)
            .expect("every stream is terminated");
        let dict_start = bytes[..dict_end]
            .windows(2)
            .rposition(|w| w == b"<<")
            .expect("a stream has a preceding dict");
        let dict = String::from_utf8_lossy(&bytes[dict_start..dict_end]).into_owned();
        if dict.contains("/Filter /FlateDecode") {
            let mut inflated = Vec::new();
            ZlibDecoder::new(&bytes[payload..payload + end_rel])
                .read_to_end(&mut inflated)
                .expect("FlateDecode stream inflates");
            out.push((dict, inflated));
        }
        i = payload + end_rel + close.len();
    }
    out
}

/// Encode a tiny `w`×`h` solid-red PNG with the given alpha — a real raster the PDF
/// writer can decode and embed.
fn png_bytes(w: u32, h: u32, alpha: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, alpha]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

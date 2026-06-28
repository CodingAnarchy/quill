//! Fonts and metrics for the PDF writer: WinAnsi encoding, the standard-14 Times
//! metrics (the screen profile, #463), and TrueType font embedding (the print
//! profile, #464 — POD services require embedded fonts). Reading glyph metrics from
//! the bundled fonts is delegated to `ttf-parser`; everything emitted into the PDF
//! is still hand-written here.

use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::export::{ExportStyle, FontRole};

/// PDF object bodies keyed by id — shared with the layout module. Bodies are bytes
/// because embedded font programs (`/FontFile2`) are binary.
pub(super) type ObjMap = BTreeMap<u32, Vec<u8>>;

// The four bundled Liberation Serif faces (metric-compatible with Times, SIL OFL),
// embedded whole into print PDFs.
const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Bold.ttf");
const ITALIC: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Italic.ttf");
const BOLD_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-BoldItalic.ttf");

/// The face slot for an emphasis combination — the order both font plans lay their
/// faces out in (regular, bold, italic, bold-italic), and the `/F1`…`/F4` index.
fn face_index(bold: bool, italic: bool) -> usize {
    match (bold, italic) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    }
}

/// The `/F1`…`/F4` resource name for an emphasis combination.
pub(super) fn font_name(bold: bool, italic: bool) -> &'static str {
    ["F1", "F2", "F3", "F4"][face_index(bold, italic)]
}

/// How a render supplies its fonts: the standard-14 Times faces (no embedding —
/// the screen PDF) or the bundled faces embedded whole (the print PDF).
pub(super) enum FontPlan {
    Standard,
    Embedded(Vec<EmbeddedFace>),
}

impl FontPlan {
    /// The screen profile's fonts (#463/#489): the standard-14 Times faces when no
    /// font is configured (the unchanged default), otherwise the embedded plan so a
    /// `[style]` body/heading font can be applied.
    pub(super) fn for_screen(style: &ExportStyle) -> FontPlan {
        if style.font(FontRole::Body).is_some() || style.font(FontRole::Heading).is_some() {
            FontPlan::embedded(style)
        } else {
            FontPlan::Standard
        }
    }

    /// The print profile's fonts (#464): always embedded (POD requires it).
    pub(super) fn for_print(style: &ExportStyle) -> FontPlan {
        FontPlan::embedded(style)
    }

    /// The four embedded faces (#464/#489): bundled Liberation Serif by default, with
    /// a configured `body_font` substituted for the regular face and `heading_font`
    /// for the bold face. Body emphasis (italic/bold-italic) and any unset slot keep
    /// the bundled face — PDF can't faux-style a single configured file, so a chosen
    /// body font pairs with the bundled weights for emphasis (a documented gap).
    fn embedded(style: &ExportStyle) -> FontPlan {
        let face =
            |default: &'static [u8], role: Option<FontRole>, base: &str, bold, italic| match role
                .and_then(|r| style.font(r))
            {
                Some(f) => EmbeddedFace::load(
                    Cow::Owned(f.bytes.clone()),
                    &pdf_name(&f.family),
                    bold,
                    italic,
                ),
                None => EmbeddedFace::load(Cow::Borrowed(default), base, bold, italic),
            };
        FontPlan::Embedded(vec![
            face(
                REGULAR,
                Some(FontRole::Body),
                "LiberationSerif",
                false,
                false,
            ),
            face(
                BOLD,
                Some(FontRole::Heading),
                "LiberationSerif-Bold",
                true,
                false,
            ),
            face(ITALIC, None, "LiberationSerif-Italic", false, true),
            face(BOLD_ITALIC, None, "LiberationSerif-BoldItalic", true, true),
        ])
    }

    /// One character's advance for the chosen face, in 1000-em units — Times
    /// metrics for the standard plan, the embedded face's own metrics otherwise.
    pub(super) fn em_width(&self, c: char, bold: bool, italic: bool) -> f32 {
        match self {
            FontPlan::Standard => times_em(c),
            FontPlan::Embedded(faces) => {
                let face = &faces[face_index(bold, italic)];
                let b = winansi(c).unwrap_or(b'?') as usize;
                let w = face.em[b];
                if w > 0.0 {
                    w
                } else {
                    face.em[b'?' as usize]
                }
            }
        }
    }

    /// Emit the font objects and return the object ids backing `/F1`…`/F4`.
    pub(super) fn objects(&self, next: &mut u32, out: &mut ObjMap) -> [u32; 4] {
        let mut ids = [0u32; 4];
        match self {
            FontPlan::Standard => {
                const BASES: [&str; 4] = [
                    "Times-Roman",
                    "Times-Bold",
                    "Times-Italic",
                    "Times-BoldItalic",
                ];
                for (i, base) in BASES.iter().enumerate() {
                    let id = take(next);
                    out.insert(id, format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>").into_bytes());
                    ids[i] = id;
                }
            }
            FontPlan::Embedded(faces) => {
                for (i, face) in faces.iter().enumerate() {
                    let file = take(next);
                    out.insert(file, font_file_stream(&face.program));
                    let desc = take(next);
                    out.insert(desc, face.descriptor(file).into_bytes());
                    let dict = take(next);
                    out.insert(dict, face.font_dict(desc).into_bytes());
                    ids[i] = dict;
                }
            }
        }
        ids
    }
}

/// A bundled font parsed for embedding: its program bytes plus the descriptor
/// metrics and per-WinAnsi-byte advances the PDF font objects need.
pub(super) struct EmbeddedFace {
    base: String,
    program: Cow<'static, [u8]>,
    flags: i32,
    bbox: [i32; 4],
    italic_angle: f32,
    ascent: i32,
    descent: i32,
    cap_height: i32,
    stem_v: i32,
    /// Advance per WinAnsi byte (0x20–0xFF) in 1000-em units; 0 for unmapped.
    em: [f32; 256],
}

impl EmbeddedFace {
    fn load(program: Cow<'static, [u8]>, base: &str, bold: bool, italic: bool) -> EmbeddedFace {
        let face = ttf_parser::Face::parse(&program, 0).expect("font parses");
        let scale = 1000.0 / face.units_per_em() as f32;
        let mut em = [0.0f32; 256];
        for b in 0x20u16..=0xFF {
            if let Some(ch) = winansi_to_char(b as u8) {
                if let Some(gid) = face.glyph_index(ch) {
                    if let Some(adv) = face.glyph_hor_advance(gid) {
                        em[b as usize] = adv as f32 * scale;
                    }
                }
            }
        }
        let bb = face.global_bounding_box();
        let s = |v: i16| (v as f32 * scale).round() as i32;
        // Serif + nonsymbolic (uses a standard encoding), italic when slanted.
        let flags = 0x02 | 0x20 | if italic { 0x40 } else { 0 };
        let ascent = s(face.ascender());
        let descent = s(face.descender());
        let italic_angle = face.italic_angle();
        let bbox = [s(bb.x_min), s(bb.y_min), s(bb.x_max), s(bb.y_max)];
        let cap_height = face
            .capital_height()
            .map(s)
            .unwrap_or((ascent as f32 * 0.7) as i32);
        // All face-derived values are read into locals above, so `face`'s borrow of
        // `program` ends here — before `program` moves into the struct below.
        EmbeddedFace {
            base: base.to_string(),
            program,
            flags,
            bbox,
            italic_angle,
            ascent,
            descent,
            cap_height,
            stem_v: if bold { 120 } else { 80 },
            em,
        }
    }

    fn descriptor(&self, font_file: u32) -> String {
        format!(
            "<< /Type /FontDescriptor /FontName /{} /Flags {} /FontBBox [{} {} {} {}] \
             /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV {} /FontFile2 {} 0 R >>",
            self.base,
            self.flags,
            self.bbox[0],
            self.bbox[1],
            self.bbox[2],
            self.bbox[3],
            trim_float(self.italic_angle),
            self.ascent,
            self.descent,
            self.cap_height,
            self.stem_v,
            font_file,
        )
    }

    fn font_dict(&self, descriptor: u32) -> String {
        let widths: String = (0x20u16..=0xFF)
            .map(|b| format!("{} ", self.em[b as usize].round() as i32))
            .collect();
        format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /{} /FirstChar 32 /LastChar 255 \
             /Widths [ {widths}] /FontDescriptor {descriptor} 0 R /Encoding /WinAnsiEncoding >>",
            self.base,
        )
    }
}

/// A PDF name token for a font family: drop spaces and anything outside
/// `[A-Za-z0-9-]` so the configured family is a valid `/BaseFont` name.
fn pdf_name(family: &str) -> String {
    let cleaned: String = family
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if cleaned.is_empty() {
        "EmbeddedFont".to_string()
    } else {
        cleaned
    }
}

/// A `/FontFile2` stream: the raw (uncompressed) TrueType program, with `/Length1`
/// giving its length as required for embedded TrueType fonts.
fn font_file_stream(program: &[u8]) -> Vec<u8> {
    let mut v = format!(
        "<< /Length {} /Length1 {} >>\nstream\n",
        program.len(),
        program.len()
    )
    .into_bytes();
    v.extend_from_slice(program);
    v.extend_from_slice(b"\nendstream");
    v
}

fn take(next: &mut u32) -> u32 {
    let id = *next;
    *next += 1;
    id
}

fn trim_float(x: f32) -> String {
    if (x - x.round()).abs() < 0.005 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    }
}

/// Map a char to its WinAnsi byte, or `None` if the standard fonts can't show it.
/// ASCII and Latin-1 map directly; the common typographic punctuation maps into the
/// `0x80–0x9F` WinAnsi slots.
pub(super) fn winansi(c: char) -> Option<u8> {
    let cp = c as u32;
    Some(match cp {
        0x20..=0x7E | 0xA0..=0xFF => cp as u8,
        0x2018 => 0x91,
        0x2019 => 0x92,
        0x201A => 0x82,
        0x201C => 0x93,
        0x201D => 0x94,
        0x201E => 0x84,
        0x2013 => 0x96,
        0x2014 => 0x97,
        0x2026 => 0x85,
        0x2022 => 0x95,
        0x2020 => 0x86,
        0x2021 => 0x87,
        0x2039 => 0x8B,
        0x203A => 0x9B,
        0x0152 => 0x8C,
        0x0153 => 0x9C,
        0x0160 => 0x8A,
        0x0161 => 0x9A,
        0x0178 => 0x9F,
        0x017D => 0x8E,
        0x017E => 0x9E,
        0x0192 => 0x83,
        0x02C6 => 0x88,
        0x02DC => 0x98,
        0x2122 => 0x99,
        0x2030 => 0x89,
        _ => return None,
    })
}

/// The char a WinAnsi byte encodes (inverse of [`winansi`]) — used to read the
/// embedded font's advance for each encoded byte.
fn winansi_to_char(b: u8) -> Option<char> {
    Some(match b {
        0x20..=0x7E | 0xA0..=0xFF => b as char,
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x82 => '\u{201A}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x84 => '\u{201E}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x85 => '\u{2026}',
        0x95 => '\u{2022}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x8B => '\u{2039}',
        0x9B => '\u{203A}',
        0x8C => '\u{0152}',
        0x9C => '\u{0153}',
        0x8A => '\u{0160}',
        0x9A => '\u{0161}',
        0x9F => '\u{0178}',
        0x8E => '\u{017D}',
        0x9E => '\u{017E}',
        0x83 => '\u{0192}',
        0x88 => '\u{02C6}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x89 => '\u{2030}',
        _ => return None,
    })
}

/// Encode text as a PDF literal string body (between the parens): WinAnsi bytes,
/// with `(`, `)`, `\` escaped and non-ASCII bytes written as `\ddd` octal so the
/// content stream stays ASCII. Unsupported characters become `?`.
pub(super) fn pdf_string(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match winansi(c) {
            None => out.push('?'),
            Some(b'(') => out.push_str("\\("),
            Some(b')') => out.push_str("\\)"),
            Some(b'\\') => out.push_str("\\\\"),
            Some(b @ 0x20..=0x7E) => out.push(b as char),
            Some(b) => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out
}

/// One character's Times-Roman advance in 1000-em units (the screen profile's
/// metric for every face). The body is Roman, so wrapping is exact there; bold and
/// italic spans measure a hair narrow, well within the margin.
fn times_em(c: char) -> f32 {
    let b = winansi(c).unwrap_or(b'?');
    let w = if (0x20..=0x7E).contains(&b) {
        TIMES_ASCII[(b - 0x20) as usize]
    } else {
        match b {
            0x97 => 1000, // em dash
            0x96 => 500,  // en dash
            0x85 => 1000, // ellipsis
            0x91 | 0x92 => 333,
            0x93 | 0x94 => 444,
            0x95 => 350, // bullet
            0x82 => 333,
            0x84 => 444,
            0x86 | 0x87 => 500,
            _ => 500, // accented Latin & misc — close enough for wrapping
        }
    };
    w as f32
}

/// Times-Roman advance widths (1000-em units) for ASCII 0x20–0x7E, from the Adobe
/// Core-14 metrics.
const TIMES_ASCII: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250,
    278, // 0x20–0x2F
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564,
    444, // 0x30–0x3F
    921, 722, 667, 667, 722, 611, 556, 722, 722, 333, 389, 722, 611, 889, 722,
    722, // 0x40–0x4F
    556, 722, 667, 556, 611, 722, 722, 944, 722, 722, 611, 333, 278, 333, 469,
    500, // 0x50–0x5F
    333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500, 278, 778, 500,
    500, // 0x60–0x6F
    500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541, // 0x70–0x7E
];

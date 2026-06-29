//! Fonts for the PDF writer (#492): every face is embedded as a Type0 /Identity-H
//! composite font over a TrueType program, so the PDF renders the full Unicode
//! range its fonts cover — body text, ornaments, and non-Latin scripts alike, not
//! just WinAnsi. Glyph metrics and cmaps are read with `ttf-parser`; the PDF font
//! objects (Type0 + CIDFontType2 + descriptor + `/FontFile2` + `/W` + `/ToUnicode`)
//! are hand-written here.
//!
//! Bundled defaults: Liberation Serif (the body, four weights, SIL OFL) and DejaVu
//! Sans (a broad fallback for glyphs the body lacks — ornaments, symbols, scripts;
//! Bitstream Vera license). A `[style]` body/heading font overrides the serif. CJK
//! needs an author-supplied CJK font (the fallback has no CJK). Fonts embed whole;
//! glyph subsetting to shrink files is tracked in #495.

use std::cell::RefCell;
use std::collections::BTreeMap;

use ttf_parser::{Face, GlyphId};

use crate::export::{ExportStyle, FontRole};

/// PDF object bodies keyed by id — shared with the layout module. Bodies are bytes
/// because embedded font programs (`/FontFile2`) are binary.
pub(super) type ObjMap = BTreeMap<u32, Vec<u8>>;

// Bundled font programs.
const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Bold.ttf");
const ITALIC: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Italic.ttf");
const BOLD_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-BoldItalic.ttf");
const FALLBACK: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");

/// The `/F1`…`/F4` resource names for the four body faces, plus `/Ffb` for the
/// fallback — the order [`Fonts`] lays them out in.
const FACE_NAMES: [&str; 4] = ["F1", "F2", "F3", "F4"];
const FALLBACK_NAME: &str = "Ffb";

fn face_index(bold: bool, italic: bool) -> usize {
    match (bold, italic) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    }
}

/// The render's fonts: four body faces (regular/bold/italic/bold-italic) and one
/// broad fallback, each embeddable as a Type0 composite.
pub(super) struct Fonts {
    faces: [CidFont; 4],
    fallback: CidFont,
}

impl Fonts {
    /// Build the font set: the bundled Liberation Serif faces (or a `[style]`
    /// body/heading font in the regular/bold slots) plus the DejaVu Sans fallback.
    pub(super) fn new(style: &ExportStyle) -> Fonts {
        let body = |role: FontRole, default: &'static [u8], base: &str| match style.font(role) {
            Some(f) => CidFont::load(leak(&f.bytes), &pdf_name(&f.family), true),
            None => CidFont::load(default, base, true),
        };
        Fonts {
            faces: [
                body(FontRole::Body, REGULAR, "LiberationSerif"),
                body(FontRole::Heading, BOLD, "LiberationSerif-Bold"),
                CidFont::load(ITALIC, "LiberationSerif-Italic", true),
                CidFont::load(BOLD_ITALIC, "LiberationSerif-BoldItalic", true),
            ],
            fallback: CidFont::load(FALLBACK, "DejaVuSans", false),
        }
    }

    /// Resolve `c` for the `(bold, italic)` face to a `(resource name, glyph id)`,
    /// recording the glyph for embedding. Falls back to DejaVu for glyphs the body
    /// face lacks; `None` if no font has the glyph.
    pub(super) fn resolve(&self, c: char, bold: bool, italic: bool) -> Option<(&'static str, u16)> {
        let idx = face_index(bold, italic);
        if let Some(gid) = self.faces[idx].record(c) {
            return Some((FACE_NAMES[idx], gid));
        }
        self.fallback.record(c).map(|gid| (FALLBACK_NAME, gid))
    }

    /// One character's advance for the `(bold, italic)` face (em units / 1000),
    /// using the same body→fallback resolution as drawing. 0 if unrenderable.
    pub(super) fn advance_em(&self, c: char, bold: bool, italic: bool) -> f32 {
        let idx = face_index(bold, italic);
        if let Some(gid) = self.faces[idx].gid(c) {
            return self.faces[idx].advance_em(gid);
        }
        self.fallback
            .gid(c)
            .map(|gid| self.fallback.advance_em(gid))
            .unwrap_or(0.0)
    }

    /// Whether any font can render `c`.
    pub(super) fn covers(&self, c: char) -> bool {
        self.faces.iter().any(|f| f.gid(c).is_some()) || self.fallback.gid(c).is_some()
    }

    /// Emit the objects for every face that was used, returning the
    /// (resource name, Type0 object id) pairs for the page `/Resources /Font` dict.
    pub(super) fn objects(&self, next: &mut u32, out: &mut ObjMap) -> Vec<(&'static str, u32)> {
        let mut map = Vec::new();
        for (i, face) in self.faces.iter().enumerate() {
            if face.is_used() {
                map.push((FACE_NAMES[i], face.emit(next, out)));
            }
        }
        if self.fallback.is_used() {
            map.push((FALLBACK_NAME, self.fallback.emit(next, out)));
        }
        map
    }
}

/// One font embedded as a Type0/Identity-H composite over a TrueType program: the
/// parsed face (for glyph ids + advances) and the set of glyphs actually used (for
/// `/W` and `/ToUnicode`).
struct CidFont {
    base: String,
    program: &'static [u8],
    face: Face<'static>,
    scale: f32, // 1000 / units-per-em
    serif: bool,
    used: RefCell<BTreeMap<u16, char>>,
}

impl CidFont {
    fn load(program: &'static [u8], base: &str, serif: bool) -> CidFont {
        let face = Face::parse(program, 0).expect("font parses");
        let scale = 1000.0 / face.units_per_em() as f32;
        CidFont {
            base: base.to_string(),
            program,
            face,
            scale,
            serif,
            used: RefCell::new(BTreeMap::new()),
        }
    }

    fn gid(&self, c: char) -> Option<u16> {
        self.face.glyph_index(c).map(|g| g.0)
    }

    fn advance_em(&self, gid: u16) -> f32 {
        self.face.glyph_hor_advance(GlyphId(gid)).unwrap_or(0) as f32 * self.scale
    }

    /// Note `c` as used and return its glyph id, or `None` if this font lacks it.
    fn record(&self, c: char) -> Option<u16> {
        let gid = self.gid(c)?;
        self.used.borrow_mut().insert(gid, c);
        Some(gid)
    }

    fn is_used(&self) -> bool {
        !self.used.borrow().is_empty()
    }

    /// Emit `/FontFile2`, `/FontDescriptor`, the descendant `CIDFontType2`, a
    /// `/ToUnicode` CMap, and the Type0 dict; return the Type0 object id.
    fn emit(&self, next: &mut u32, out: &mut ObjMap) -> u32 {
        let used = self.used.borrow();
        let file = take(next);
        out.insert(file, font_file_stream(self.program));

        let bb = self.face.global_bounding_box();
        let s = |v: i16| (v as f32 * self.scale).round() as i32;
        let ascent = s(self.face.ascender());
        let cap = self
            .face
            .capital_height()
            .map(s)
            .unwrap_or((ascent as f32 * 0.7) as i32);
        // Nonsymbolic (uses a standard charset), serif bit for the body faces.
        let flags = 0x20 | if self.serif { 0x02 } else { 0 };
        let angle = self.face.italic_angle();
        let descriptor = take(next);
        out.insert(
            descriptor,
            format!(
                "<< /Type /FontDescriptor /FontName /{} /Flags {flags} /FontBBox [{} {} {} {}] \
                 /ItalicAngle {angle} /Ascent {ascent} /Descent {} /CapHeight {cap} /StemV 80 \
                 /FontFile2 {file} 0 R >>",
                self.base,
                s(bb.x_min),
                s(bb.y_min),
                s(bb.x_max),
                s(bb.y_max),
                s(self.face.descender()),
            )
            .into_bytes(),
        );

        let widths: String = used
            .keys()
            .map(|&gid| format!("{gid} [{}] ", self.advance_em(gid).round() as i32))
            .collect();
        let cid = take(next);
        out.insert(
            cid,
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{} \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                 /FontDescriptor {descriptor} 0 R /CIDToGIDMap /Identity /DW 1000 /W [ {widths}] >>",
                self.base
            )
            .into_bytes(),
        );

        let to_unicode = take(next);
        out.insert(to_unicode, to_unicode_cmap(&used));

        let type0 = take(next);
        out.insert(
            type0,
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{} /Encoding /Identity-H \
                 /DescendantFonts [{cid} 0 R] /ToUnicode {to_unicode} 0 R >>",
                self.base
            )
            .into_bytes(),
        );
        type0
    }
}

/// A `/ToUnicode` CMap mapping each used glyph id (the 2-byte CID under Identity-H)
/// to its Unicode value, so the rendered text stays selectable and searchable.
fn to_unicode_cmap(used: &BTreeMap<u16, char>) -> Vec<u8> {
    let mut body = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    // bfchar blocks are capped at 100 entries.
    let entries: Vec<(u16, char)> = used.iter().map(|(&g, &c)| (g, c)).collect();
    for chunk in entries.chunks(100) {
        body.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (gid, ch) in chunk {
            body.push_str(&format!("<{gid:04X}> <{}>\n", utf16be_hex(*ch)));
        }
        body.push_str("endbfchar\n");
    }
    body.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend");
    let mut v = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
    v.extend_from_slice(body.as_bytes());
    v.extend_from_slice(b"\nendstream");
    v
}

/// A char as a UTF-16BE hex string (a surrogate pair for non-BMP).
fn utf16be_hex(c: char) -> String {
    let mut units = [0u16; 2];
    c.encode_utf16(&mut units)
        .iter()
        .map(|u| format!("{u:04X}"))
        .collect()
}

/// Glyph ids for `text` in the given face (body→fallback resolved), as a content
/// hex string (`<48656C…>`) and its total advance in em/1000-scaled width at
/// `size`. Unrenderable characters are dropped.
pub(super) fn glyph_run(
    fonts: &Fonts,
    text: &str,
    bold: bool,
    italic: bool,
) -> Vec<(&'static str, String)> {
    let mut runs: Vec<(&'static str, String)> = Vec::new();
    for c in text.chars() {
        let Some((name, gid)) = fonts.resolve(c, bold, italic) else {
            continue;
        };
        match runs.last_mut() {
            Some((n, hex)) if *n == name => hex.push_str(&format!("{gid:04X}")),
            _ => runs.push((name, format!("{gid:04X}"))),
        }
    }
    runs
}

/// A `/FontFile2` stream: the raw (uncompressed) TrueType program, with `/Length1`.
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

/// A PDF name token for a font family: keep `[A-Za-z0-9-]` so it's a valid name.
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

/// Leak runtime font bytes to `'static` so the parsed [`Face`] can be stored. Only
/// configured (`[style]`) fonts hit this — a few KB–MB per export, rare; the
/// bundled fonts are already `'static`.
fn leak(bytes: &[u8]) -> &'static [u8] {
    Box::leak(bytes.to_vec().into_boxed_slice())
}

/// Allocate the next object id (shared with the layout module).
pub(super) fn take(next: &mut u32) -> u32 {
    let id = *next;
    *next += 1;
    id
}

/// Encode `text` as a PDF text-string for a dictionary value (bookmark titles): an
/// ASCII literal, or a UTF-16BE hex string (with BOM) when it has non-ASCII.
pub(super) fn pdf_text_string(text: &str) -> String {
    if text.is_ascii() {
        let escaped: String = text
            .chars()
            .flat_map(|c| match c {
                '(' | ')' | '\\' => vec!['\\', c],
                _ => vec![c],
            })
            .collect();
        format!("({escaped})")
    } else {
        let hex: String = std::iter::once("FEFF".to_string())
            .chain(text.chars().map(utf16be_hex))
            .collect();
        format!("<{hex}>")
    }
}

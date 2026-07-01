//! Fonts for the PDF writer (#492): every face is embedded as a Type0 /Identity-H
//! composite font over a TrueType program, so the PDF renders the full Unicode
//! range its fonts cover — body text, ornaments, and non-Latin scripts alike, not
//! just WinAnsi. Glyph metrics and cmaps are read with `ttf-parser`; the PDF font
//! objects (Type0 + CIDFontType2 + descriptor + `/FontFile2` + `/W` + `/ToUnicode`)
//! are hand-written here.
//!
//! Bundled defaults: Liberation Serif (the body, four weights, SIL OFL), DejaVu
//! Sans (a broad fallback for glyphs the body lacks — ornaments, symbols, scripts;
//! Bitstream Vera license), and DejaVu Sans Mono (the fixed-width face for fenced
//! preformatted blocks, #524; same Bitstream Vera license). A `[style]` body/heading
//! font overrides the serif. CJK needs an author-supplied CJK font (the fallback has
//! no CJK). Each face is subset
//! to the glyphs the document uses (#495), so embedding stays small; subsetting
//! renumbers glyphs, so the content streams are remapped through [`GidMap`].

use std::cell::RefCell;
use std::collections::BTreeMap;

use subsetter::{GlyphRemapper, Tag};
use ttf_parser::{Face, GlyphId};

use crate::export::{ExportStyle, FontRole, FontStyle};

/// The bold weight the PDF pins for the bold slots (#638). A variable-`[wght]`
/// body/heading font is instanced at this weight for bold text, so bold body prose
/// is a genuinely heavier face — not the default weight reused, and not the heading
/// font leaking into the bold slot.
const BOLD_WGHT: f32 = 700.0;

/// The `wght` (weight) variation axis tag.
fn wght_tag() -> Tag {
    Tag::new(b"wght")
}

/// The synthetic-oblique shear (a slant of ~12°, `tan ≈ 0.21`) applied only as a
/// last resort — when an italic style is wanted but the family shipped no true
/// italic file (#638). Curated families always ship a real italic, so this is a
/// fallback for a hand-specified single upright file. Kept in step with the
/// `-12` `/ItalicAngle` the descriptor reports for a synthesized face.
pub(super) const OBLIQUE_SHEAR: f32 = 0.21;

/// PDF object bodies keyed by id — shared with the layout module. Bodies are bytes
/// because embedded font programs (`/FontFile2`) are binary.
pub(super) type ObjMap = BTreeMap<u32, Vec<u8>>;

/// A glyph-id remapping from the original font to its embedded subset (old → new).
/// Subsetting renumbers glyphs, so content streams keyed by glyph id are remapped
/// through this at serialization.
pub(super) type GidMap = BTreeMap<u16, u16>;

// Bundled font programs.
const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Bold.ttf");
const ITALIC: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Italic.ttf");
const BOLD_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-BoldItalic.ttf");
const FALLBACK: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSansMono.ttf");

/// The `/F1`…`/F4` resource names for the four body faces (regular/bold/italic/
/// bold-italic), `/Fh1`/`/Fh2` for the heading faces (regular/bold), `/Ffb` for the
/// broad fallback, and `/Fmo` for the monospace face — the order [`Fonts`] lays
/// them out in.
const BODY_NAMES: [&str; 4] = ["F1", "F2", "F3", "F4"];
const HEADING_NAMES: [&str; 2] = ["Fh1", "Fh2"];
const FALLBACK_NAME: &str = "Ffb";
const MONO_NAME: &str = "Fmo";

/// The body-face slot index for a `(bold, italic)` style: regular, bold, italic,
/// bold-italic.
fn body_index(style: FontStyle) -> usize {
    match (style.bold, style.italic) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    }
}

/// The render's fonts: four body faces (regular/bold/italic/bold-italic), two
/// heading faces (regular/bold) resolved separately so the heading font never
/// leaks into body-bold (#638), one broad fallback, and the monospace face — each
/// embeddable as a Type0 composite.
pub(super) struct Fonts {
    body: [CidFont; 4],
    heading: [CidFont; 2],
    fallback: CidFont,
    /// The fixed-width face for fenced preformatted blocks (#524).
    mono: CidFont,
}

impl Fonts {
    /// Build the font set (#638): the four body faces and two heading faces, each
    /// from the configured `[style]` font when present (its true-italic file for
    /// the italic slots, `wght`-instanced for the bold slots) or the bundled
    /// Liberation Serif faces otherwise, plus the DejaVu Sans fallback and mono.
    pub(super) fn new(style: &ExportStyle) -> Fonts {
        let body = |s: FontStyle, default: &'static [u8], base: &str| {
            body_face(style, FontRole::Body, s, default, base)
        };
        Fonts {
            body: [
                body(FontStyle::REGULAR, REGULAR, "LiberationSerif"),
                body(FontStyle::BOLD, BOLD, "LiberationSerif-Bold"),
                body(FontStyle::ITALIC, ITALIC, "LiberationSerif-Italic"),
                body(
                    FontStyle::BOLD_ITALIC,
                    BOLD_ITALIC,
                    "LiberationSerif-BoldItalic",
                ),
            ],
            heading: [
                // Headings are upright; the writer renders them bold, but keep a
                // regular slot for completeness. Both come from the heading font
                // (its own family), never the body font.
                body_face(
                    style,
                    FontRole::Heading,
                    FontStyle::REGULAR,
                    REGULAR,
                    "LiberationSerif",
                ),
                body_face(
                    style,
                    FontRole::Heading,
                    FontStyle::BOLD,
                    BOLD,
                    "LiberationSerif-Bold",
                ),
            ],
            fallback: CidFont::load(FALLBACK, "DejaVuSans", false),
            mono: CidFont::load(MONO, "DejaVuSansMono", false),
        }
    }

    /// The `(face, resource name)` for a body or heading style — the single place
    /// the two families' slotting is decided.
    fn face_of(&self, style: FontStyle, heading: bool) -> (&CidFont, &'static str) {
        if heading {
            let idx = style.bold as usize;
            (&self.heading[idx], HEADING_NAMES[idx])
        } else {
            let idx = body_index(style);
            (&self.body[idx], BODY_NAMES[idx])
        }
    }

    /// Resolve `c` for the `style` face (body or `heading`) to a `(resource name,
    /// glyph id)`, recording the glyph for embedding. Falls back to DejaVu for
    /// glyphs the face lacks; `None` if no font has the glyph.
    pub(super) fn resolve(
        &self,
        c: char,
        style: FontStyle,
        heading: bool,
    ) -> Option<(&'static str, u16)> {
        let (face, name) = self.face_of(style, heading);
        if let Some(gid) = face.record(c) {
            return Some((name, gid));
        }
        self.fallback.record(c).map(|gid| (FALLBACK_NAME, gid))
    }

    /// Whether the `style` face (body or `heading`) is synthetically obliqued —
    /// i.e. an italic style with no true-italic file, drawn sheared (#638). The
    /// draw layer skews the text matrix for such runs.
    pub(super) fn is_synthetic_oblique(&self, style: FontStyle, heading: bool) -> bool {
        self.face_of(style, heading).0.synthetic_oblique
    }

    /// One character's advance for the `style` face (em units / 1000), using the
    /// same face→fallback resolution as drawing. 0 if unrenderable.
    pub(super) fn advance_em(&self, c: char, style: FontStyle, heading: bool) -> f32 {
        let (face, _) = self.face_of(style, heading);
        if let Some(gid) = face.gid(c) {
            return face.advance_em(gid);
        }
        self.fallback
            .gid(c)
            .map(|gid| self.fallback.advance_em(gid))
            .unwrap_or(0.0)
    }

    /// Resolve `c` for the monospace face (fenced preformatted blocks, #524) to a
    /// `(resource name, glyph id)`, recording the glyph for embedding. Falls back to
    /// DejaVu Sans for glyphs the mono face lacks; `None` if no font has the glyph.
    pub(super) fn resolve_mono(&self, c: char) -> Option<(&'static str, u16)> {
        if let Some(gid) = self.mono.record(c) {
            return Some((MONO_NAME, gid));
        }
        self.fallback.record(c).map(|gid| (FALLBACK_NAME, gid))
    }

    /// One character's advance in the monospace face (em units / 1000), using the same
    /// mono→fallback resolution as drawing. 0 if unrenderable.
    pub(super) fn advance_mono(&self, c: char) -> f32 {
        if let Some(gid) = self.mono.gid(c) {
            return self.mono.advance_em(gid);
        }
        self.fallback
            .gid(c)
            .map(|gid| self.fallback.advance_em(gid))
            .unwrap_or(0.0)
    }

    /// Whether any font can render `c`.
    pub(super) fn covers(&self, c: char) -> bool {
        self.body.iter().any(|f| f.gid(c).is_some())
            || self.heading.iter().any(|f| f.gid(c).is_some())
            || self.fallback.gid(c).is_some()
    }

    /// Emit the objects for every face that was used, returning per face its resource
    /// name, Type0 object id, and the glyph-id remap from subsetting (for the page
    /// `/Resources /Font` dict and remapping content-stream glyph ids).
    pub(super) fn objects(
        &self,
        next: &mut u32,
        out: &mut ObjMap,
    ) -> Vec<(&'static str, u32, GidMap)> {
        let mut map = Vec::new();
        for (i, face) in self.body.iter().enumerate() {
            if face.is_used() {
                let (id, gids) = face.emit(next, out);
                map.push((BODY_NAMES[i], id, gids));
            }
        }
        for (i, face) in self.heading.iter().enumerate() {
            if face.is_used() {
                let (id, gids) = face.emit(next, out);
                map.push((HEADING_NAMES[i], id, gids));
            }
        }
        if self.fallback.is_used() {
            let (id, gids) = self.fallback.emit(next, out);
            map.push((FALLBACK_NAME, id, gids));
        }
        if self.mono.is_used() {
            let (id, gids) = self.mono.emit(next, out);
            map.push((MONO_NAME, id, gids));
        }
        map
    }
}

/// Build one body/heading face for `role` at `style` (#638): use the configured
/// font's true-italic file for italic slots when present (else the upright file,
/// synthetically obliqued), pin the `wght` axis to bold for bold slots when the
/// font is variable, and fall back to the bundled `default` serif when no font is
/// configured for the role.
fn body_face(
    style: &ExportStyle,
    role: FontRole,
    want: FontStyle,
    default: &'static [u8],
    base: &str,
) -> CidFont {
    match style.font_face(role, want) {
        Some(f) => {
            let program = leak(&f.bytes);
            // Only pin the weight when the file actually has a `wght` axis; a static
            // per-weight file (or the bundled defaults) already carries its weight.
            let bold_weight = want.bold && has_wght_axis(program);
            // Synthesize the slope only when italic is wanted but this file isn't a
            // true italic (no `-italic` sibling shipped).
            let synthetic_oblique = want.italic && !f.italic;
            CidFont::new(
                program,
                &pdf_name(&f.family),
                true,
                bold_weight,
                synthetic_oblique,
            )
        }
        None => CidFont::new(default, base, true, false, false),
    }
}

/// Whether `program` is a variable font carrying a `wght` (weight) axis, so it can
/// be instanced to a genuine bold (#638).
fn has_wght_axis(program: &[u8]) -> bool {
    Face::parse(program, 0)
        .map(|f| {
            f.variation_axes()
                .into_iter()
                .any(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght"))
        })
        .unwrap_or(false)
}

/// One font embedded as a Type0/Identity-H composite over a TrueType program: the
/// parsed face (for glyph ids + advances) and the set of glyphs actually used (for
/// `/W` and `/ToUnicode`). A variable font is instanced to `bold_weight` at embed
/// time; a face wanting italic with no true-italic file is drawn `synthetic_oblique`
/// (sheared by the draw layer) (#638).
struct CidFont {
    base: String,
    program: &'static [u8],
    face: Face<'static>,
    scale: f32, // 1000 / units-per-em
    serif: bool,
    /// Pin the `wght` axis to [`BOLD_WGHT`] when embedding (variable fonts only).
    bold_weight: bool,
    /// Draw this face sheared to fake italics — the last-resort slope (#638).
    synthetic_oblique: bool,
    used: RefCell<BTreeMap<u16, char>>,
}

impl CidFont {
    /// Load a face with no instancing or synthesis (the fallback and mono faces).
    fn load(program: &'static [u8], base: &str, serif: bool) -> CidFont {
        CidFont::new(program, base, serif, false, false)
    }

    fn new(
        program: &'static [u8],
        base: &str,
        serif: bool,
        bold_weight: bool,
        synthetic_oblique: bool,
    ) -> CidFont {
        let mut face = Face::parse(program, 0).expect("font parses");
        // Match the instancing done at embed time (#638): pin the parsed face to the
        // bold weight too, so measured advances/metrics reflect the heavier outline
        // the subset carries — keeping layout and rendering consistent.
        if bold_weight {
            face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), BOLD_WGHT);
        }
        let scale = 1000.0 / face.units_per_em() as f32;
        CidFont {
            base: base.to_string(),
            program,
            face,
            scale,
            serif,
            bold_weight,
            synthetic_oblique,
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

    /// Emit `/FontFile2` (a glyph subset), `/FontDescriptor`, the descendant
    /// `CIDFontType2`, a `/ToUnicode` CMap, and the Type0 dict; return the Type0
    /// object id and the old→new glyph-id remap the subset introduced.
    fn emit(&self, next: &mut u32, out: &mut ObjMap) -> (u32, GidMap) {
        let used = self.used.borrow();
        // Subset to just the used glyphs (renumbered); fall back to the whole font
        // with identity ids if subsetting fails (shouldn't for our valid fonts).
        let old_gids: Vec<u16> = used.keys().copied().collect();
        let remapper = GlyphRemapper::new_from_glyphs(&old_gids);
        // A variable font is instanced to the bold weight while subsetting, so the
        // embedded program carries a genuinely heavier outline — not the default
        // instance reused for every weight (#638).
        let subset = if self.bold_weight {
            subsetter::subset_with_variations(
                self.program,
                0,
                &[(wght_tag(), BOLD_WGHT)],
                &remapper,
            )
        } else {
            subsetter::subset(self.program, 0, &remapper)
        };
        let (program, gids): (Vec<u8>, GidMap) = match subset {
            Ok(sub) => (
                sub,
                old_gids
                    .iter()
                    .map(|&o| (o, remapper.get(o).unwrap_or(o)))
                    .collect(),
            ),
            Err(_) => (
                self.program.to_vec(),
                old_gids.iter().map(|&o| (o, o)).collect(),
            ),
        };
        let file = take(next);
        out.insert(file, font_file_stream(&program));

        let bb = self.face.global_bounding_box();
        let s = |v: i16| (v as f32 * self.scale).round() as i32;
        let ascent = s(self.face.ascender());
        let cap = self
            .face
            .capital_height()
            .map(s)
            .unwrap_or((ascent as f32 * 0.7) as i32);
        // Nonsymbolic (uses a standard charset), serif bit for the body faces, and
        // the italic bit (0x40) when the face is slanted — a true italic or a
        // synthetically obliqued one (#638).
        let italic =
            self.synthetic_oblique || self.face.is_italic() || self.face.italic_angle() != 0.0;
        let flags = 0x20 | if self.serif { 0x02 } else { 0 } | if italic { 0x40 } else { 0 };
        // Report the synthetic slant so a viewer knows the run is italic; else the
        // face's own angle.
        let angle = if self.synthetic_oblique {
            -12.0
        } else {
            self.face.italic_angle()
        };
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

        // Widths keyed by the new (subset) glyph id, measured from the original.
        let widths: String = used
            .keys()
            .map(|&old| format!("{} [{}] ", gids[&old], self.advance_em(old).round() as i32))
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

        // ToUnicode keyed by the new glyph id.
        let to_unicode = take(next);
        let unicode: BTreeMap<u16, char> = used.iter().map(|(&old, &c)| (gids[&old], c)).collect();
        out.insert(to_unicode, to_unicode_cmap(&unicode));

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
        (type0, gids)
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

/// Glyph ids for `text` in the `style` face (body or `heading`), face→fallback
/// resolved, as content hex runs grouped by resource name (`<48656C…>`).
/// Unrenderable characters are dropped (#638).
pub(super) fn glyph_run(
    fonts: &Fonts,
    text: &str,
    style: FontStyle,
    heading: bool,
) -> Vec<(&'static str, String)> {
    let mut runs: Vec<(&'static str, String)> = Vec::new();
    for c in text.chars() {
        let Some((name, gid)) = fonts.resolve(c, style, heading) else {
            continue;
        };
        match runs.last_mut() {
            Some((n, hex)) if *n == name => hex.push_str(&format!("{gid:04X}")),
            _ => runs.push((name, format!("{gid:04X}"))),
        }
    }
    runs
}

/// Glyph ids for `text` in the monospace face (mono→fallback resolved), as content
/// hex runs grouped by resource name (#524). Unrenderable characters are dropped.
pub(super) fn glyph_run_mono(fonts: &Fonts, text: &str) -> Vec<(&'static str, String)> {
    let mut runs: Vec<(&'static str, String)> = Vec::new();
    for c in text.chars() {
        let Some((name, gid)) = fonts.resolve_mono(c) else {
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

//! Embedded fonts for the designed exports (#469, #638): the [`FontRole`] family,
//! the four synthesizable [`FontStyle`]s, the resolved [`EmbeddedFont`] file, and
//! the path helpers ([`font_media_type`], [`italic_sibling`], [`font_family`]) the
//! resolver in [`super`] uses to turn a `[style]` font path into embedded bytes.

/// Which family an [`EmbeddedFont`] belongs to (#638): the body text or the
/// headings. Each family resolves to up to two physical files — an upright and a
/// true-italic face — and each file covers both a regular and a bold weight (via
/// the `[wght]` axis for a variable font, pinned at embed time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontRole {
    Body,
    Heading,
}

impl FontRole {
    /// The in-package file-name stem for this role (`fonts/<slug>.<ext>`); the
    /// italic face appends `-italic` (see [`super::ExportStyle::resolve_fonts`]).
    pub(super) const fn slug(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Heading => "heading",
        }
    }
}

/// A designed export's four synthesizable text styles (#638): the combinations of
/// weight (regular/bold) and slope (upright/italic) prose can take. Emphasis is
/// pervasive in fiction, so all four must render in the chosen family — not just
/// regular and a heading-derived bold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontStyle {
    pub bold: bool,
    pub italic: bool,
}

impl FontStyle {
    pub const REGULAR: Self = Self {
        bold: false,
        italic: false,
    };
    pub const BOLD: Self = Self {
        bold: true,
        italic: false,
    };
    pub const ITALIC: Self = Self {
        bold: false,
        italic: true,
    };
    pub const BOLD_ITALIC: Self = Self {
        bold: true,
        italic: true,
    };
}

/// A font file embedded into a designed export (#469, #638): its CSS family name,
/// the family [`FontRole`] it fills, whether it is the true-italic face, the
/// in-package href, the OPF manifest media type, and the bytes. Resolved by the
/// app from the `[style]` font paths; not part of the persisted config.
///
/// One configured path resolves to up to two `EmbeddedFont`s per role: the upright
/// file and — when a sibling `<stem>-italic.<ext>` exists — the true-italic file.
/// Each file covers both a regular and a bold weight: for a variable `[wght]` font
/// the PDF writer pins the weight at embed time and the EPUB `@font-face` rules
/// carry the matching `font-weight`, so bold body text is a real heavier weight,
/// never the heading font leaking into the bold slot (#638).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedFont {
    pub role: FontRole,
    pub family: String,
    /// The true-italic face for its family (a `<stem>-italic` sibling file); the
    /// upright file when `false`. Body text resolves italic/bold-italic to the
    /// italic file and regular/bold to the upright file.
    pub italic: bool,
    pub href: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

impl EmbeddedFont {
    /// The CSS `@font-face` `format(...)` token for this file's media type (#709),
    /// so a reader that can't handle the format has a fallback signal instead of
    /// downloading an unusable face. `None` for a type with no standard token.
    pub(crate) fn css_format(&self) -> Option<&'static str> {
        Some(match self.media_type {
            "font/ttf" => "truetype",
            "font/otf" => "opentype",
            "font/woff" => "woff",
            "font/woff2" => "woff2",
            _ => return None,
        })
    }
}

/// The OPF media type and canonical extension for a font path, or `None` for an
/// unsupported type.
pub(super) fn font_media_type(path: &str) -> Option<(&'static str, &'static str)> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "ttf" => ("font/ttf", "ttf"),
        "otf" => ("font/otf", "otf"),
        "woff" => ("font/woff", "woff"),
        "woff2" => ("font/woff2", "woff2"),
        _ => return None,
    })
}

/// The conventional true-italic sibling of a configured upright font path (#638):
/// `fonts/eb-garamond.ttf` → `fonts/eb-garamond-italic.ttf`. The curated
/// downloader writes the italic face at exactly this name, and a hand-specified
/// font can drop one beside it to gain real italics. `None` when the path has no
/// recognizable extension. A path that already ends in `-italic` isn't doubled.
pub(super) fn italic_sibling(path: &str) -> Option<String> {
    let p = std::path::Path::new(path);
    let stem = p.file_stem()?.to_str()?;
    if stem.to_ascii_lowercase().ends_with("-italic") {
        return None;
    }
    let ext = p.extension()?.to_str()?;
    let parent = p.parent().and_then(|d| d.to_str()).unwrap_or("");
    let file = format!("{stem}-italic.{ext}");
    Some(if parent.is_empty() {
        file
    } else {
        format!("{parent}/{file}")
    })
}

/// A CSS-safe family name derived from a font file's stem (`Fonts/Georgia.ttf` →
/// `Georgia`). Restricted to letters, digits, spaces, hyphens, and underscores so
/// it can't break out of the quoted `font-family` / `@font-face` string it's
/// interpolated into (#470); falls back to a generic name.
pub(super) fn font_family(path: &str) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let safe = safe.split_whitespace().collect::<Vec<_>>().join(" ");
    if safe.is_empty() {
        "Embedded".to_string()
    } else {
        safe
    }
}

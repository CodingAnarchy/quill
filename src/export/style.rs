//! Opt-in styling for the *designed* exports (#465–#482). The [`ExportStyle`]
//! struct and its supporting types — the asset roles ([`FontRole`]/[`ImageRole`]),
//! the resolved [`EmbeddedFont`]/[`EmbeddedImage`], the preset library, and the
//! media-type/family helpers — form a self-contained subsystem the format writers
//! consume. Re-exported from [`super`] so callers see it on `crate::export`.

use serde::{Deserialize, Serialize};

use super::SCENE_BREAK;

/// Opt-in styling for the *designed* exports — EPUB now, PDF when it lands
/// (#465). Loaded from the project manifest's `[style]` table; the default
/// reproduces today's output exactly, and the plain writers (Markdown, Word,
/// Scrivener, Vellum) ignore it entirely.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ExportStyle {
    /// A named preset bundling the knobs below into a coherent look (#473) — its
    /// values are the base, and any explicit key here overrides it. `None` → no
    /// preset. See [`Self::apply_preset`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// The scene-break divider glyph (#468). `None` → the default centered
    /// [`SCENE_BREAK`] (`#`), so an unstyled export is byte-for-byte unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_break: Option<String>,
    /// Path (relative to the project) to a font file embedded for the body text
    /// (#469). `None` → the reader's default font.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_font: Option<String>,
    /// Path (relative to the project) to a font file embedded for headings (#469).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading_font: Option<String>,
    /// The fonts resolved from the paths above at export time (#469) — read from
    /// disk by the app, not persisted in the manifest.
    #[serde(skip)]
    pub fonts: Vec<EmbeddedFont>,
    /// Chapter-heading alignment (#470): `left` / `center` / `right` / `justify`.
    /// `None` → the default centered heading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading_align: Option<String>,
    /// Whether each chapter's first paragraph opens with a drop cap (#470).
    /// `None` → off (and lets a preset's value show through, #473).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_cap: Option<bool>,
    /// A decorative glyph centered on the title page (#471). `None` → none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_ornament: Option<String>,
    /// A decorative glyph centered on each part-divider page (#471). `None` → none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_ornament: Option<String>,
    /// Path (relative to the project) to an image embedded as the scene-break
    /// ornament (#472), overriding [`Self::scene_break`]. `None` → the glyph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_break_image: Option<String>,
    /// Path to an image embedded above each chapter's title (#472). `None` → none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter_heading_image: Option<String>,
    /// Path to an image embedded atop the title page (#472). `None` → none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_image: Option<String>,
    /// Path to the book cover image (#582). `None` → no cover. Embedded in the EPUB
    /// as the `cover-image` item (the first spine page) and rendered as a full-page
    /// page 0 in the print/PDF.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_image: Option<String>,
    /// The images resolved from the paths above at export time (#472) — read from
    /// disk by the app, not persisted in the manifest.
    #[serde(skip)]
    pub images: Vec<EmbeddedImage>,
}

/// The CSS `text-align` values a heading may use (#470) — an allowlist, since the
/// value is interpolated into the stylesheet. Public so the in-app style setter
/// (#569) offers exactly the values the writers honor.
pub const HEADING_ALIGNS: [&str; 4] = ["left", "center", "right", "justify"];

/// The built-in preset names (#473, #482), in the order shown to authors. The
/// single source of truth: [`preset_base`] resolves each one and the unknown-preset
/// warning lists them, so the library can't drift between code and the message (a
/// test asserts every name here resolves).
pub(crate) const PRESET_NAMES: [&str; 7] = [
    "classic",
    "modern",
    "literary",
    "romance",
    "thriller",
    "fantasy",
    "minimalist",
];

/// The base [`ExportStyle`] for a built-in preset (#473, #482), or `None` for an
/// unknown name. Presets set only the asset-free knobs (scene break, ornaments,
/// heading alignment, drop cap) so any author can use one without bundling licensed
/// fonts or artwork — those are layered on top via explicit `[style]` keys. Each is
/// a distinct, coherent look; keep the arms in sync with [`PRESET_NAMES`].
fn preset_base(name: &str) -> Option<ExportStyle> {
    Some(match name {
        // Traditional: centered headings, drop caps, floral scene break + dividers.
        "classic" => ExportStyle {
            scene_break: Some("* * *".into()),
            drop_cap: Some(true),
            title_ornament: Some("❦".into()),
            part_ornament: Some("❦".into()),
            ..Default::default()
        },
        // Spare and contemporary: left-aligned headings, a minimal scene mark.
        "modern" => ExportStyle {
            heading_align: Some("left".into()),
            scene_break: Some("•".into()),
            ..Default::default()
        },
        // Literary fiction: understated — an asterism between scenes and a drop cap,
        // no divider ornaments.
        "literary" => ExportStyle {
            scene_break: Some("⁂".into()),
            drop_cap: Some(true),
            ..Default::default()
        },
        // Romance: warm and decorative — a flower scene break with floral dividers.
        "romance" => ExportStyle {
            scene_break: Some("❀".into()),
            drop_cap: Some(true),
            title_ornament: Some("❦".into()),
            part_ornament: Some("❦".into()),
            ..Default::default()
        },
        // Thriller: stark and punchy — left-aligned headings, an em-dash scene
        // break, no drop cap or ornaments.
        "thriller" => ExportStyle {
            heading_align: Some("left".into()),
            scene_break: Some("—".into()),
            ..Default::default()
        },
        // Fantasy: ornate — a diamond scene break with fleur-de-lis dividers and a
        // drop cap.
        "fantasy" => ExportStyle {
            scene_break: Some("❖".into()),
            drop_cap: Some(true),
            title_ornament: Some("⚜".into()),
            part_ornament: Some("⚜".into()),
            ..Default::default()
        },
        // Minimalist: the barest mark — left-aligned headings, a single centered
        // mid-dot, nothing else.
        "minimalist" => ExportStyle {
            heading_align: Some("left".into()),
            scene_break: Some("·".into()),
            ..Default::default()
        },
        _ => return None,
    })
}

/// A one-line description of a built-in preset's look (#569), for the in-app
/// preset picker — the same library [`PRESET_NAMES`] enumerates. Returns an empty
/// string for an unknown name (which the picker never passes).
pub(crate) fn preset_blurb(name: &str) -> &'static str {
    match name {
        "classic" => "* * * scene breaks, drop caps, ❦ title/part ornaments",
        "modern" => "left-aligned headings, • scene breaks",
        "literary" => "⁂ asterism scene breaks and drop caps, no ornaments",
        "romance" => "❀ scene breaks, drop caps, ❦ ornaments",
        "thriller" => "left-aligned headings, em-dash (—) scene breaks, spare",
        "fantasy" => "❖ scene breaks, drop caps, ⚜ fleur-de-lis ornaments",
        "minimalist" => "left-aligned headings, a single centered mid-dot (·)",
        _ => "",
    }
}

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
    /// italic face appends `-italic` (see [`ExportStyle::resolve_fonts`]).
    fn slug(self) -> &'static str {
        match self {
            FontRole::Body => "body",
            FontRole::Heading => "heading",
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
    pub const REGULAR: FontStyle = FontStyle {
        bold: false,
        italic: false,
    };
    pub const BOLD: FontStyle = FontStyle {
        bold: true,
        italic: false,
    };
    pub const ITALIC: FontStyle = FontStyle {
        bold: false,
        italic: true,
    };
    pub const BOLD_ITALIC: FontStyle = FontStyle {
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

/// Where an [`EmbeddedImage`] is placed in a designed export (#472).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRole {
    SceneBreak,
    ChapterHeading,
    Title,
    /// The book cover (#582) — the first spine page in the EPUB, a full page 0 in
    /// the PDF.
    Cover,
}

impl ImageRole {
    /// The in-package file slug for this role (`images/<slug>.<ext>`).
    fn slug(self) -> &'static str {
        match self {
            ImageRole::SceneBreak => "scene-break",
            ImageRole::ChapterHeading => "chapter-heading",
            ImageRole::Title => "title",
            ImageRole::Cover => "cover",
        }
    }
}

/// An image file embedded into a designed export (#472): its in-package href, the
/// OPF manifest media type, and the bytes (embedded verbatim — no decode/re-encode,
/// so no image-codec dependency). Resolved by the app from the `[style]` paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedImage {
    pub role: ImageRole,
    pub href: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Why a styling asset (font or image) couldn't be loaded.
enum AssetError {
    Unsupported,
    Read(std::io::Error),
}

/// Load one styling-asset file: look up its (media type, extension) via `media`,
/// then read its bytes via `load`. Shared by font (#469) and image (#472)
/// resolution; the caller maps the error to a domain-specific warning.
fn load_asset(
    path: &str,
    media: impl Fn(&str) -> Option<(&'static str, &'static str)>,
    load: &mut impl FnMut(&str) -> std::io::Result<Vec<u8>>,
) -> Result<(&'static str, &'static str, Vec<u8>), AssetError> {
    let (media_type, ext) = media(path).ok_or(AssetError::Unsupported)?;
    let bytes = load(path).map_err(AssetError::Read)?;
    Ok((media_type, ext, bytes))
}

impl ExportStyle {
    /// The scene-break glyph to render: the configured one, or the default `#`.
    pub fn scene_break(&self) -> &str {
        self.scene_break.as_deref().unwrap_or(SCENE_BREAK)
    }

    /// The resolved upright file for `role`, if one was configured and loaded —
    /// the face that carries the family name and CSS `font-family` (#469).
    pub fn font(&self, role: FontRole) -> Option<&EmbeddedFont> {
        self.fonts.iter().find(|f| f.role == role && !f.italic)
    }

    /// The resolved font face `role` uses for `style` (#638): the true-italic file
    /// for an italic style when one was loaded, else the upright file (whose slope
    /// the writer synthesizes as a last resort). `None` when no font is configured
    /// for the role. Weight (regular/bold) rides the same file — pinned at embed
    /// time — so it isn't part of the file selection.
    pub fn font_face(&self, role: FontRole, style: FontStyle) -> Option<&EmbeddedFont> {
        let want_italic = style.italic && self.fonts.iter().any(|f| f.role == role && f.italic);
        self.fonts
            .iter()
            .find(|f| f.role == role && f.italic == want_italic)
    }

    /// The resolved image for `role`, if one was configured and loaded.
    pub fn image(&self, role: ImageRole) -> Option<&EmbeddedImage> {
        self.images.iter().find(|i| i.role == role)
    }

    /// The validated heading alignment (#470), or `None` for the default (centered)
    /// — an out-of-allowlist value is ignored here and reported by
    /// [`Self::style_warnings`].
    pub fn heading_align(&self) -> Option<&str> {
        self.heading_align
            .as_deref()
            .filter(|v| HEADING_ALIGNS.contains(v))
    }

    /// Whether chapter openings get a drop cap (#470) — off unless set.
    pub fn drop_cap(&self) -> bool {
        self.drop_cap.unwrap_or(false)
    }

    /// Apply the named `preset` (#473): start from its bundled values and overlay
    /// the explicitly-set keys on top, so an author can pick a look and still tweak
    /// individual knobs. No-op when no preset is set; returns a warning (and leaves
    /// the style untouched) for an unknown preset name. Run before font/image
    /// resolution.
    pub fn apply_preset(&mut self) -> Option<String> {
        let name = self.preset.clone()?;
        match preset_base(&name) {
            Some(base) => {
                let overrides = std::mem::take(self);
                *self = base.merged_with(overrides);
                None
            }
            None => Some(format!(
                "style: unknown preset '{name}' (presets: {})",
                PRESET_NAMES.join(", ")
            )),
        }
    }

    /// Overlay `over`'s explicitly-set (`Some`/named) config fields onto `self`,
    /// so `over` wins where set — the preset-override merge (#473). Destructured
    /// (no `..`) on purpose: a field added to `ExportStyle` won't compile until it
    /// gets a merge rule here, so a new style knob can't silently skip presets.
    /// `preset` is the input, not a result; `fonts`/`images` are resolved later.
    fn merged_with(mut self, over: ExportStyle) -> ExportStyle {
        let ExportStyle {
            preset: _,
            scene_break,
            body_font,
            heading_font,
            heading_align,
            drop_cap,
            title_ornament,
            part_ornament,
            scene_break_image,
            chapter_heading_image,
            title_image,
            cover_image,
            fonts: _,
            images: _,
        } = over;
        self.scene_break = scene_break.or(self.scene_break);
        self.body_font = body_font.or(self.body_font);
        self.heading_font = heading_font.or(self.heading_font);
        self.heading_align = heading_align.or(self.heading_align);
        self.drop_cap = drop_cap.or(self.drop_cap);
        self.title_ornament = title_ornament.or(self.title_ornament);
        self.part_ornament = part_ornament.or(self.part_ornament);
        self.scene_break_image = scene_break_image.or(self.scene_break_image);
        self.chapter_heading_image = chapter_heading_image.or(self.chapter_heading_image);
        self.title_image = title_image.or(self.title_image);
        self.cover_image = cover_image.or(self.cover_image);
        self
    }

    /// Warnings for non-fatal `[style]` config problems (#470) — e.g. an unknown
    /// heading alignment. (Font-resolution warnings come from [`Self::resolve_fonts`].)
    pub fn style_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        // A configured alignment that didn't survive `heading_align`'s allowlist.
        if let Some(v) = &self.heading_align {
            if self.heading_align().is_none() {
                warnings.push(format!(
                    "style: unknown heading_align '{v}' (use left/center/right/justify)"
                ));
            }
        }
        warnings
    }

    /// Resolve the configured font paths into [`EmbeddedFont`]s using `load`
    /// (a relative path → its bytes), filling [`Self::fonts`]. Returns a warning
    /// for each font that can't be read or has an unsupported type. The caller
    /// supplies `load`, so this stays off the filesystem and is testable.
    /// Resolve the configured font paths into [`EmbeddedFont`]s using `load`
    /// (a relative path → its bytes), filling [`Self::fonts`]. Returns a warning
    /// for each configured font that can't be read or has an unsupported type. The
    /// caller supplies `load`, so this stays off the filesystem and is testable.
    ///
    /// Each configured role resolves to its upright file plus, when a sibling
    /// `<stem>-italic.<ext>` loads, a true-italic file (#638) — so italic and
    /// bold-italic body text render in the chosen family rather than the bundled
    /// fallback. A missing italic sibling is silent (the writer synthesizes the
    /// slope); only a *configured* path that can't be read or is an unsupported
    /// type warns.
    pub fn resolve_fonts(
        &mut self,
        mut load: impl FnMut(&str) -> std::io::Result<Vec<u8>>,
    ) -> Vec<String> {
        let mut fonts = Vec::new();
        let mut warnings = Vec::new();
        let targets = [
            (FontRole::Body, self.body_font.clone()),
            (FontRole::Heading, self.heading_font.clone()),
        ];
        for (role, path) in targets {
            let Some(path) = path else { continue };
            // The upright file — the configured path itself. A read/type failure
            // here is the one the author must hear about.
            match load_asset(&path, font_media_type, &mut load) {
                Ok((media_type, ext, bytes)) => fonts.push(EmbeddedFont {
                    role,
                    family: font_family(&path),
                    italic: false,
                    href: format!("fonts/{}.{ext}", role.slug()),
                    media_type,
                    bytes,
                }),
                Err(AssetError::Unsupported) => warnings.push(format!(
                    "style: unsupported font '{path}' (use .ttf/.otf/.woff/.woff2)"
                )),
                Err(AssetError::Read(e)) => {
                    warnings.push(format!("style: couldn't read font '{path}': {e}"))
                }
            }
            // The optional true-italic sibling — best-effort, no warning if absent.
            if let Some(italic_path) = italic_sibling(&path) {
                if let Ok((media_type, ext, bytes)) =
                    load_asset(&italic_path, font_media_type, &mut load)
                {
                    fonts.push(EmbeddedFont {
                        role,
                        family: font_family(&path),
                        italic: true,
                        href: format!("fonts/{}-italic.{ext}", role.slug()),
                        media_type,
                        bytes,
                    });
                }
            }
        }
        self.fonts = fonts;
        warnings
    }

    /// Resolve the configured image paths into [`EmbeddedImage`]s using `load`,
    /// filling [`Self::images`] (#472). Images are embedded verbatim (bytes only,
    /// media type from the extension — no decode). Returns a warning per image that
    /// can't be read or has an unsupported type. Mirrors [`Self::resolve_fonts`].
    pub fn resolve_images(
        &mut self,
        load: impl FnMut(&str) -> std::io::Result<Vec<u8>>,
    ) -> Vec<String> {
        let targets = [
            (ImageRole::SceneBreak, self.scene_break_image.clone()),
            (
                ImageRole::ChapterHeading,
                self.chapter_heading_image.clone(),
            ),
            (ImageRole::Title, self.title_image.clone()),
            (ImageRole::Cover, self.cover_image.clone()),
        ];
        let (images, warnings) = resolve_assets(
            targets,
            image_media_type,
            "image",
            ".png/.jpg/.jpeg/.gif/.svg/.webp",
            load,
            |role, _path, media_type, ext, bytes| EmbeddedImage {
                role,
                href: format!("images/{}.{ext}", role.slug()),
                media_type,
                bytes,
            },
        );
        self.images = images;
        warnings
    }
}

/// Resolve a set of configured `(role, Option<path>)` asset targets into embedded
/// structs, returning them alongside one warning per asset that couldn't be read
/// or had an unsupported type. The shared body of [`ExportStyle::resolve_fonts`]
/// and [`ExportStyle::resolve_images`] (#504): `media` looks up the (media type,
/// extension); `kind`/`exts` shape the warning text (`unsupported {kind}` lists
/// `{exts}`); `load` reads the bytes; `embed` builds the role-specific struct.
fn resolve_assets<Role, Asset, const N: usize>(
    targets: [(Role, Option<String>); N],
    media: impl Fn(&str) -> Option<(&'static str, &'static str)>,
    kind: &str,
    exts: &str,
    mut load: impl FnMut(&str) -> std::io::Result<Vec<u8>>,
    mut embed: impl FnMut(Role, &str, &'static str, &'static str, Vec<u8>) -> Asset,
) -> (Vec<Asset>, Vec<String>) {
    let mut assets = Vec::new();
    let mut warnings = Vec::new();
    for (role, path) in targets {
        let Some(path) = path else { continue };
        match load_asset(&path, &media, &mut load) {
            Ok((media_type, ext, bytes)) => assets.push(embed(role, &path, media_type, ext, bytes)),
            Err(AssetError::Unsupported) => {
                warnings.push(format!("style: unsupported {kind} '{path}' (use {exts})"))
            }
            Err(AssetError::Read(e)) => {
                warnings.push(format!("style: couldn't read {kind} '{path}': {e}"))
            }
        }
    }
    (assets, warnings)
}

/// The OPF media type and canonical extension for a font path, or `None` for an
/// unsupported type.
fn font_media_type(path: &str) -> Option<(&'static str, &'static str)> {
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

/// The OPF media type and extension for an image path, or `None` for an
/// unsupported type. The EPUB-core raster/vector image formats.
fn image_media_type(path: &str) -> Option<(&'static str, &'static str)> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => ("image/png", "png"),
        "jpg" => ("image/jpeg", "jpg"),
        "jpeg" => ("image/jpeg", "jpeg"),
        "gif" => ("image/gif", "gif"),
        "svg" => ("image/svg+xml", "svg"),
        "webp" => ("image/webp", "webp"),
        _ => return None,
    })
}

/// The conventional true-italic sibling of a configured upright font path (#638):
/// `fonts/eb-garamond.ttf` → `fonts/eb-garamond-italic.ttf`. The curated
/// downloader writes the italic face at exactly this name, and a hand-specified
/// font can drop one beside it to gain real italics. `None` when the path has no
/// recognizable extension. A path that already ends in `-italic` isn't doubled.
fn italic_sibling(path: &str) -> Option<String> {
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
fn font_family(path: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_style_scene_break_defaults_to_the_plain_glyph() {
        assert_eq!(ExportStyle::default().scene_break(), SCENE_BREAK);
        assert_eq!(
            ExportStyle {
                scene_break: Some("❧".into()),
                ..Default::default()
            }
            .scene_break(),
            "❧"
        );
    }

    #[test]
    fn resolve_fonts_loads_supported_files_and_warns_on_the_rest() {
        let mut style = ExportStyle {
            body_font: Some("fonts/Georgia.otf".into()),
            heading_font: Some("fonts/Display.bad".into()),
            ..Default::default()
        };
        let warnings = style.resolve_fonts(|path| match path {
            "fonts/Georgia.otf" => Ok(b"OTTO".to_vec()),
            other => Err(std::io::Error::other(format!("no {other}"))),
        });
        // The body font resolves: family from the stem, role-named href, media type.
        let body = style.font(FontRole::Body).expect("body font resolved");
        assert_eq!(body.family, "Georgia");
        assert_eq!(body.href, "fonts/body.otf");
        assert_eq!(body.media_type, "font/otf");
        assert_eq!(body.bytes, b"OTTO");
        // The unsupported-extension heading font is skipped with a warning.
        assert!(style.font(FontRole::Heading).is_none());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unsupported"), "{warnings:?}");
    }

    #[test]
    fn resolve_fonts_picks_up_a_true_italic_sibling_for_all_four_styles() {
        // A body font with a `-italic` sibling on disk (#638): the resolver loads
        // both files and `font_face` routes upright styles to the upright file and
        // italic styles to the true-italic file — all in the same family.
        let mut style = ExportStyle {
            body_font: Some("fonts/eb-garamond.ttf".into()),
            ..Default::default()
        };
        let warnings = style.resolve_fonts(|path| match path {
            "fonts/eb-garamond.ttf" => Ok(b"upright".to_vec()),
            "fonts/eb-garamond-italic.ttf" => Ok(b"italic".to_vec()),
            other => Err(std::io::Error::other(format!("no {other}"))),
        });
        assert!(warnings.is_empty(), "both files load: {warnings:?}");

        // Two embedded files, distinct hrefs, one shared family.
        assert_eq!(style.fonts.len(), 2, "upright + italic");
        let upright = style.font(FontRole::Body).expect("upright");
        assert_eq!(upright.href, "fonts/body.ttf");
        assert!(!upright.italic);
        let italic = style
            .fonts
            .iter()
            .find(|f| f.role == FontRole::Body && f.italic)
            .expect("italic");
        assert_eq!(italic.href, "fonts/body-italic.ttf");
        assert_eq!(italic.family, upright.family, "same CSS family");
        assert_eq!(italic.bytes, b"italic");

        // The four styles resolve to the right file — italics to the italic face,
        // regular/bold (weight rides the same upright file) to the upright.
        let face = |s| style.font_face(FontRole::Body, s).unwrap();
        assert!(!face(FontStyle::REGULAR).italic);
        assert!(!face(FontStyle::BOLD).italic);
        assert!(face(FontStyle::ITALIC).italic, "italic → true-italic file");
        assert!(
            face(FontStyle::BOLD_ITALIC).italic,
            "bold-italic → true-italic file"
        );
    }

    #[test]
    fn resolve_fonts_without_an_italic_sibling_falls_back_to_the_upright_file() {
        // A single upright file, no `-italic` sibling (a hand-specified font, #638):
        // no warning, and italic styles fall back to the upright file (the writer
        // synthesizes the slope) — never a different family.
        let mut style = ExportStyle {
            body_font: Some("fonts/only.ttf".into()),
            ..Default::default()
        };
        let warnings = style.resolve_fonts(|path| match path {
            "fonts/only.ttf" => Ok(b"upright".to_vec()),
            other => Err(std::io::Error::other(format!("no {other}"))),
        });
        assert!(
            warnings.is_empty(),
            "a missing italic sibling is silent: {warnings:?}"
        );
        assert_eq!(style.fonts.len(), 1, "just the upright file");
        // Every style resolves to the one upright file.
        for s in [
            FontStyle::REGULAR,
            FontStyle::BOLD,
            FontStyle::ITALIC,
            FontStyle::BOLD_ITALIC,
        ] {
            let f = style.font_face(FontRole::Body, s).expect("resolves");
            assert!(!f.italic, "{s:?} → the upright file");
            assert_eq!(f.href, "fonts/body.ttf");
        }
    }

    #[test]
    fn italic_sibling_derivation_is_conventional() {
        assert_eq!(
            italic_sibling("fonts/eb-garamond.ttf").as_deref(),
            Some("fonts/eb-garamond-italic.ttf")
        );
        assert_eq!(
            italic_sibling("Georgia.otf").as_deref(),
            Some("Georgia-italic.otf")
        );
        // An already-italic path isn't doubled; an extension-less path has no sibling.
        assert_eq!(italic_sibling("fonts/body-italic.ttf"), None);
        assert_eq!(italic_sibling("fonts/noext"), None);
    }

    #[test]
    fn resolve_fonts_warns_on_an_unreadable_file() {
        let mut style = ExportStyle {
            body_font: Some("missing.ttf".into()),
            ..Default::default()
        };
        let warnings = style.resolve_fonts(|_| Err(std::io::Error::other("nope")));
        assert!(style.fonts.is_empty());
        assert!(warnings[0].contains("couldn't read"), "{warnings:?}");
    }

    #[test]
    fn resolve_fonts_sanitizes_the_css_family_name() {
        // A stem with CSS-breaking characters can't escape the quoted family string.
        let mut style = ExportStyle {
            body_font: Some("fonts/Ev\"il; }.otf".into()),
            ..Default::default()
        };
        style.resolve_fonts(|_| Ok(b"OTTO".to_vec()));
        let family = &style.font(FontRole::Body).unwrap().family;
        assert!(
            !family.contains(['"', ';', '{', '}', '\\']),
            "css-safe family: {family:?}"
        );
    }

    #[test]
    fn resolve_images_loads_supported_files_and_warns_on_the_rest() {
        let mut style = ExportStyle {
            scene_break_image: Some("art/break.png".into()),
            chapter_heading_image: Some("art/banner.jpg".into()),
            title_image: Some("art/title.bmp".into()),
            ..Default::default()
        };
        let warnings = style.resolve_images(|path| match path {
            "art/break.png" => Ok(vec![0x89, b'P', b'N', b'G']),
            "art/banner.jpg" => Ok(vec![0xff, 0xd8]),
            other => Err(std::io::Error::other(format!("no {other}"))),
        });
        // Supported images resolve: role-named href + media type from the extension.
        let brk = style
            .image(ImageRole::SceneBreak)
            .expect("scene-break image");
        assert_eq!(brk.href, "images/scene-break.png");
        assert_eq!(brk.media_type, "image/png");
        let banner = style
            .image(ImageRole::ChapterHeading)
            .expect("chapter image");
        assert_eq!(banner.href, "images/chapter-heading.jpg");
        assert_eq!(banner.media_type, "image/jpeg");
        // The unsupported `.bmp` title image is skipped with a warning.
        assert!(style.image(ImageRole::Title).is_none());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unsupported image"), "{warnings:?}");
    }

    #[test]
    fn a_preset_seeds_the_style_and_explicit_keys_override_it() {
        // `classic` bundles a scene break, drop cap, and ornaments.
        let mut style = ExportStyle {
            preset: Some("classic".into()),
            ..Default::default()
        };
        assert!(style.apply_preset().is_none());
        assert!(style.drop_cap());
        assert_eq!(style.scene_break(), "* * *");
        assert_eq!(style.title_ornament.as_deref(), Some("❦"));

        // An explicit key wins; unset keys keep the preset's value.
        let mut tweaked = ExportStyle {
            preset: Some("classic".into()),
            scene_break: Some("@@@".into()),
            ..Default::default()
        };
        assert!(tweaked.apply_preset().is_none());
        assert_eq!(
            tweaked.scene_break(),
            "@@@",
            "explicit key overrides preset"
        );
        assert!(tweaked.drop_cap(), "unset key keeps the preset");

        // `modern` is the contrasting look.
        let mut modern = ExportStyle {
            preset: Some("modern".into()),
            ..Default::default()
        };
        assert!(modern.apply_preset().is_none());
        assert_eq!(modern.heading_align(), Some("left"));
        assert!(!modern.drop_cap());

        // An unknown preset warns (listing the library) and leaves the style
        // untouched.
        let mut bad = ExportStyle {
            preset: Some("retro".into()),
            ..Default::default()
        };
        let warning = bad.apply_preset().expect("unknown preset warns");
        assert!(warning.contains("unknown preset"), "{warning}");
        assert!(warning.contains("fantasy"), "lists the library: {warning}");
        assert_eq!(bad.scene_break(), "#", "default scene break unchanged");

        // No preset → no-op, no warning.
        assert!(ExportStyle::default().apply_preset().is_none());
    }

    #[test]
    fn every_named_preset_resolves_to_a_distinct_styled_look() {
        // Each name in the library resolves without warning and actually changes
        // something from the plain default — this guards against a name being added
        // to PRESET_NAMES (or the warning) without a matching `preset_base` arm.
        for name in PRESET_NAMES {
            let mut style = ExportStyle {
                preset: Some(name.to_string()),
                ..Default::default()
            };
            assert!(
                style.apply_preset().is_none(),
                "preset '{name}' should resolve"
            );
            assert_ne!(
                style,
                ExportStyle::default(),
                "preset '{name}' should style something"
            );
        }
    }

    #[test]
    fn every_preset_has_a_picker_blurb() {
        // The in-app picker (#569) shows a blurb per preset; guard against a name
        // being added to PRESET_NAMES without a matching `preset_blurb` arm.
        for name in PRESET_NAMES {
            assert!(
                !preset_blurb(name).is_empty(),
                "preset '{name}' should have a picker blurb"
            );
        }
        assert!(preset_blurb("nope").is_empty());
    }

    #[test]
    fn heading_align_is_allowlisted_and_warns_on_an_unknown_value() {
        let ok = ExportStyle {
            heading_align: Some("left".into()),
            ..Default::default()
        };
        assert_eq!(ok.heading_align(), Some("left"));
        assert!(ok.style_warnings().is_empty());

        // A bad value is ignored (default centered) and reported.
        let bad = ExportStyle {
            heading_align: Some("centre".into()),
            ..Default::default()
        };
        assert_eq!(
            bad.heading_align(),
            None,
            "unknown alignment falls back to default"
        );
        assert!(
            bad.style_warnings()[0].contains("heading_align"),
            "{:?}",
            bad.style_warnings()
        );

        // No alignment → no warning, default centered.
        assert!(ExportStyle::default().heading_align().is_none());
        assert!(ExportStyle::default().style_warnings().is_empty());
    }
}

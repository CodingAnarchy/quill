//! Manuscript export (#7). A [`Document`] is the project compiled into a single,
//! ordered structure — book metadata, then parts and chapters with their
//! epigraphs and prose, in reading order. This module is the shared layer the
//! format writers build on: [`to_markdown`] renders one to a single Markdown file
//! (#190); the Word / Scrivener / Vellum writers (#191–#193) consume the same
//! `Document`.

use serde::{Deserialize, Serialize};

use crate::frontmatter::Epigraph;

pub mod docx;
pub mod epub;
pub mod scriv;

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
    /// The images resolved from the paths above at export time (#472) — read from
    /// disk by the app, not persisted in the manifest.
    #[serde(skip)]
    pub images: Vec<EmbeddedImage>,
}

/// The CSS `text-align` values a heading may use (#470) — an allowlist, since the
/// value is interpolated into the stylesheet.
const HEADING_ALIGNS: [&str; 4] = ["left", "center", "right", "justify"];

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

/// Which text a [`EmbeddedFont`] styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontRole {
    Body,
    Heading,
}

impl FontRole {
    /// The in-package file slug for this role (`fonts/<slug>.<ext>`).
    fn slug(self) -> &'static str {
        match self {
            FontRole::Body => "body",
            FontRole::Heading => "heading",
        }
    }
}

/// A font file embedded into a designed export (#469): its CSS family name, the
/// in-package href, the OPF manifest media type, and the bytes. Resolved by the
/// app from the `[style]` font paths; not part of the persisted config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedFont {
    pub role: FontRole,
    pub family: String,
    pub href: String,
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Where an [`EmbeddedImage`] is placed in a designed export (#472).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRole {
    SceneBreak,
    ChapterHeading,
    Title,
}

impl ImageRole {
    /// The in-package file slug for this role (`images/<slug>.<ext>`).
    fn slug(self) -> &'static str {
        match self {
            ImageRole::SceneBreak => "scene-break",
            ImageRole::ChapterHeading => "chapter-heading",
            ImageRole::Title => "title",
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

    /// The resolved font for `role`, if one was configured and loaded.
    pub fn font(&self, role: FontRole) -> Option<&EmbeddedFont> {
        self.fonts.iter().find(|f| f.role == role)
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
    pub fn resolve_fonts(
        &mut self,
        mut load: impl FnMut(&str) -> std::io::Result<Vec<u8>>,
    ) -> Vec<String> {
        self.fonts.clear();
        let mut warnings = Vec::new();
        let targets = [
            (FontRole::Body, self.body_font.clone()),
            (FontRole::Heading, self.heading_font.clone()),
        ];
        for (role, path) in targets {
            let Some(path) = path else { continue };
            match load_asset(&path, font_media_type, &mut load) {
                Ok((media_type, ext, bytes)) => self.fonts.push(EmbeddedFont {
                    role,
                    family: font_family(&path),
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
        }
        warnings
    }

    /// Resolve the configured image paths into [`EmbeddedImage`]s using `load`,
    /// filling [`Self::images`] (#472). Images are embedded verbatim (bytes only,
    /// media type from the extension — no decode). Returns a warning per image that
    /// can't be read or has an unsupported type. Mirrors [`Self::resolve_fonts`].
    pub fn resolve_images(
        &mut self,
        mut load: impl FnMut(&str) -> std::io::Result<Vec<u8>>,
    ) -> Vec<String> {
        self.images.clear();
        let mut warnings = Vec::new();
        let targets = [
            (ImageRole::SceneBreak, self.scene_break_image.clone()),
            (
                ImageRole::ChapterHeading,
                self.chapter_heading_image.clone(),
            ),
            (ImageRole::Title, self.title_image.clone()),
        ];
        for (role, path) in targets {
            let Some(path) = path else { continue };
            match load_asset(&path, image_media_type, &mut load) {
                Ok((media_type, ext, bytes)) => self.images.push(EmbeddedImage {
                    role,
                    href: format!("images/{}.{ext}", role.slug()),
                    media_type,
                    bytes,
                }),
                Err(AssetError::Unsupported) => warnings.push(format!(
                    "style: unsupported image '{path}' (use .png/.jpg/.jpeg/.gif/.svg/.webp)"
                )),
                Err(AssetError::Read(e)) => {
                    warnings.push(format!("style: couldn't read image '{path}': {e}"))
                }
            }
        }
        warnings
    }
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

/// A project compiled for export: the book's metadata and its contents as a tree
/// — front matter, the body (grouped into parts), and back matter — so a binder
/// format (Scrivener, #192) can nest it and a linear format (Markdown/Word) can
/// flatten it via [`Document::blocks`].
pub struct Document {
    pub title: String,
    pub subtitle: Option<String>,
    pub author: Option<String>,
    pub series: Option<String>,
    /// The book epigraph, rendered at the start.
    pub epigraph: Option<Epigraph>,
    /// Front-matter sections (foreword, prologue, …), in order.
    pub front: Vec<Chapter>,
    /// The body, grouped into parts. A book with no defined parts has a single
    /// unnamed part holding all body chapters.
    pub parts: Vec<Part>,
    /// Back-matter sections (epilogue, appendix, …), in order.
    pub back: Vec<Chapter>,
}

/// A named (or unnamed) grouping of body chapters.
pub struct Part {
    /// The part's title, or `None` for the catch-all group of ungrouped body
    /// chapters (rendered without a part heading / folder).
    pub name: Option<String>,
    pub chapters: Vec<Chapter>,
}

/// A chapter or front/back-matter section: its title, optional epigraph, and prose
/// body (front matter already stripped).
pub struct Chapter {
    pub title: String,
    pub epigraph: Option<Epigraph>,
    pub body: String,
}

/// The book flattened to a linear sequence — part headings interleaved with
/// chapters — for the Markdown and Word writers.
pub enum Block<'a> {
    Part(&'a str),
    Chapter(&'a Chapter),
}

impl Document {
    /// Front matter, then each part (a heading when named) and its chapters, then
    /// back matter — in reading order.
    pub fn blocks(&self) -> Vec<Block<'_>> {
        let mut out = Vec::new();
        out.extend(self.front.iter().map(Block::Chapter));
        for part in &self.parts {
            if let Some(name) = &part.name {
                out.push(Block::Part(name));
            }
            out.extend(part.chapters.iter().map(Block::Chapter));
        }
        out.extend(self.back.iter().map(Block::Chapter));
        out
    }
}

/// Render a [`Document`] as a single Markdown document (#190): a title block,
/// then each part/chapter under a heading with its epigraph set apart as a
/// block quote. Clean, pandoc-friendly Markdown — the manuscript's native form.
pub fn to_markdown(doc: &Document) -> String {
    let mut out = String::new();
    if let Some(series) = &doc.series {
        out.push_str(series);
        out.push_str("\n\n");
    }
    out.push_str(&format!("# {}\n\n", doc.title));
    if let Some(subtitle) = &doc.subtitle {
        out.push_str(&format!("*{subtitle}*\n\n"));
    }
    if let Some(author) = &doc.author {
        out.push_str(&format!("by {author}\n\n"));
    }
    if let Some(epigraph) = &doc.epigraph {
        out.push_str(&epigraph_block(epigraph));
    }

    for block in doc.blocks() {
        match block {
            Block::Part(name) => out.push_str(&format!("## {name}\n\n")),
            Block::Chapter(chapter) => {
                out.push_str(&format!("## {}\n\n", chapter.title));
                if let Some(epigraph) = &chapter.epigraph {
                    out.push_str(&epigraph_block(epigraph));
                }
                // The body is already Markdown, so it's emitted verbatim — no
                // block classification needed (a scene-break rule like `---` is
                // already a Markdown thematic break; #199 only translates it for
                // the Word/Scrivener writers, which can't carry raw Markdown).
                let body = chapter.body.trim();
                if !body.is_empty() {
                    out.push_str(body);
                    out.push_str("\n\n");
                }
            }
        }
    }
    out
}

/// Render a whole series as one Markdown document (#332): the series title and
/// author once, then each book in order (each via [`to_markdown`], so its title
/// leads a top-level section and its parts/chapters nest beneath). The books
/// carry no `series`/`author` line of their own — the header above stands for the
/// whole series.
pub fn to_markdown_series(
    books: &[Document],
    series: Option<&str>,
    author: Option<&str>,
) -> String {
    let mut out = String::new();
    if let Some(series) = series {
        out.push_str(series);
        out.push_str("\n\n");
    }
    if let Some(author) = author {
        out.push_str(&format!("by {author}\n\n"));
    }
    for book in books {
        out.push_str(&to_markdown(book));
    }
    out
}

/// Drop a chapter body's leading `# {title}` line when it just repeats the
/// chapter title (the compiled heading already carries it), so the export doesn't
/// show the title twice. A body whose first heading differs from the title is
/// left untouched — only a true duplicate is removed. Mirrors the outline pane's
/// `redundant_title_heading`.
pub(crate) fn strip_repeated_title(body: &str, title: &str) -> String {
    let trimmed = body.trim_start_matches('\n');
    let Some(rest) = trimmed.strip_prefix("# ") else {
        return body.to_string();
    };
    let (heading, after) = rest.split_once('\n').unwrap_or((rest, ""));
    if heading.trim() == title.trim() {
        after.trim_start_matches('\n').to_string()
    } else {
        body.to_string()
    }
}

/// Split a line into runs by `*` (italic) and `**` (bold) markers — Quill's
/// emphasis convention. Other text (including `_`) is taken literally. Shared by
/// the Word and Scrivener writers, which carry emphasis through to their own
/// run markup.
fn inline_runs(text: &str) -> Vec<(String, bool, bool)> {
    let mut runs = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut italic) = (false, false);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' {
            if chars.get(i + 1) == Some(&'*') {
                push_run(&mut runs, &mut cur, bold, italic);
                bold = !bold;
                i += 2;
                continue;
            }
            push_run(&mut runs, &mut cur, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    push_run(&mut runs, &mut cur, bold, italic);
    // Unbalanced markers mean the `*`s weren't emphasis (a stray `*` in prose,
    // e.g. "5 * 3"). Don't drop them and over-italicize — emit the line verbatim.
    if bold || italic {
        return vec![(text.to_string(), false, false)];
    }
    runs
}

fn push_run(runs: &mut Vec<(String, bool, bool)>, cur: &mut String, bold: bool, italic: bool) {
    if !cur.is_empty() {
        runs.push((std::mem::take(cur), bold, italic));
    }
}

/// Escape `& < >` for embedding text in XML (the `.docx` and `.scrivx` markup).
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The scene-break glyph the writers emit for a thematic break: a centered `#`,
/// the standard manuscript / Scrivener scene divider, which Vellum also
/// recognizes as a scene break on import (#199). The `.scriv` importer reads it
/// back to a thematic break, so it's shared rather than re-declared.
pub(crate) const SCENE_BREAK: &str = "#";

/// One block of a chapter body (the body split on blank lines), classified so
/// each format writer renders it in its own markup. Shared by the Word and
/// Scrivener writers so scene breaks, headings, and prose are recognized
/// identically.
enum Prose {
    /// A scene break — a thematic-break line (`---`, `* * *`, …).
    SceneBreak,
    /// A heading line (`#`…), markers stripped.
    Heading(String),
    /// A normal paragraph, soft-wrapped lines joined into one.
    Paragraph(String),
}

/// Classify one body block (already split on blank lines), or `None` when it's
/// empty.
fn classify_block(block: &str) -> Option<Prose> {
    let text = block.trim();
    if text.is_empty() {
        return None;
    }
    if is_thematic_break(text) {
        return Some(Prose::SceneBreak);
    }
    if let Some(heading) = text.strip_prefix('#') {
        return Some(Prose::Heading(
            heading.trim_start_matches('#').trim().to_string(),
        ));
    }
    // Soft-wrapped lines join into one paragraph.
    let joined = text
        .split('\n')
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    Some(Prose::Paragraph(joined))
}

/// A CommonMark thematic break: a single line of three or more matching `-`,
/// `_`, or `*`, with optional spaces between and nothing else — Quill's
/// scene-break marker. A line with any other character (an em-dash, "5 * 3") is
/// not one, so prose isn't mistaken for a break.
fn is_thematic_break(block: &str) -> bool {
    if block.contains('\n') {
        return false;
    }
    let marks: Vec<char> = block.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3 && matches!(marks[0], '-' | '_' | '*') && marks.iter().all(|&c| c == marks[0])
}

/// An epigraph as a Markdown block quote — each line prefixed `> `, the
/// attribution on its own dimmed line — set apart from the surrounding prose.
fn epigraph_block(epigraph: &Epigraph) -> String {
    let mut out = String::new();
    for line in epigraph.text.lines() {
        if line.is_empty() {
            out.push_str(">\n");
        } else {
            out.push_str(&format!("> {line}\n"));
        }
    }
    if let Some(attribution) = &epigraph.attribution {
        out.push_str(&format!(">\n> — {attribution}\n"));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
        Epigraph {
            text: text.to_string(),
            attribution: by.map(str::to_string),
        }
    }

    fn chapter(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
        Chapter {
            title: title.to_string(),
            epigraph,
            body: body.to_string(),
        }
    }

    fn doc() -> Document {
        Document {
            title: "Red State Earth".into(),
            subtitle: Some("a novel".into()),
            author: Some("A. Writer".into()),
            series: Some("The Cycle".into()),
            epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
            front: vec![chapter("Prologue", None, "Before it all.")],
            parts: vec![Part {
                name: Some("Part One".into()),
                chapters: vec![chapter(
                    "The Find",
                    Some(epigraph("A line.", None)),
                    "The locket *gleamed*.",
                )],
            }],
            back: vec![],
        }
    }

    #[test]
    fn series_markdown_lists_books_in_order_under_the_series_title() {
        let book = |title: &str, body: &str| Document {
            title: title.into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("Ch", None, body)],
            }],
            back: vec![],
        };
        let md = to_markdown_series(
            &[book("Book One", "alpha"), book("Book Two", "beta")],
            Some("The Saga"),
            Some("A. Writer"),
        );
        assert!(md.starts_with("The Saga\n\n"), "{md}");
        let one = md.find("# Book One").expect("book one heading");
        let two = md.find("# Book Two").expect("book two heading");
        assert!(one < two, "books in order");
        assert!(md.contains("alpha") && md.contains("beta"));
    }

    #[test]
    fn smart_typography_glyphs_pass_through_to_markdown() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter(
                    "C",
                    None,
                    "He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}",
                )],
            }],
            back: vec![],
        };
        assert!(
            to_markdown(&d).contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
            "curly glyphs reach the Markdown/Vellum output"
        );
    }

    #[test]
    fn renders_the_book_header_and_epigraph() {
        let md = to_markdown(&doc());
        assert!(md.starts_with("The Cycle\n\n# Red State Earth\n\n*a novel*\n\nby A. Writer\n\n"));
        assert!(md.contains("> In the beginning.\n>\n> — Anon\n"), "{md}");
    }

    #[test]
    fn renders_items_in_order_with_headings() {
        let md = to_markdown(&doc());
        let prologue = md.find("## Prologue").unwrap();
        let part = md.find("## Part One").unwrap();
        let chapter = md.find("## The Find").unwrap();
        assert!(
            prologue < part && part < chapter,
            "reading order preserved:\n{md}"
        );
        // The chapter epigraph renders before its body; prose is verbatim.
        let find = &md[chapter..];
        assert!(
            find.find("> A line.").unwrap() < find.find("The locket *gleamed*.").unwrap(),
            "{md}"
        );
    }

    #[test]
    fn strips_a_body_heading_only_when_it_repeats_the_title() {
        // A leading `# Title` that matches is dropped (the compiled heading has it).
        assert_eq!(
            strip_repeated_title("# The Find\n\nProse.\n", "The Find"),
            "Prose.\n"
        );
        // A differing heading is kept (don't lose the author's text).
        assert_eq!(
            strip_repeated_title("# Prologue\n\nBefore.\n", "A Prologue"),
            "# Prologue\n\nBefore.\n"
        );
        // No leading heading → untouched.
        assert_eq!(
            strip_repeated_title("Just prose.\n", "Title"),
            "Just prose.\n"
        );
    }

    #[test]
    fn parses_nested_and_plain_emphasis() {
        assert_eq!(inline_runs("plain"), vec![("plain".into(), false, false)]);
        assert_eq!(
            inline_runs("a *b* c"),
            vec![
                ("a ".into(), false, false),
                ("b".into(), false, true),
                (" c".into(), false, false)
            ]
        );
        assert_eq!(inline_runs("**bold**"), vec![("bold".into(), true, false)]);
        // Underscores are literal (not emphasis), so snake_case survives.
        assert_eq!(
            inline_runs("snake_case"),
            vec![("snake_case".into(), false, false)]
        );
        // A stray/unbalanced `*` is kept literal, not turned into runaway italics.
        assert_eq!(
            inline_runs("5 * 3 = 15"),
            vec![("5 * 3 = 15".into(), false, false)]
        );
    }

    #[test]
    fn xml_escape_escapes_the_markup_chars() {
        assert_eq!(xml_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }

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

    #[test]
    fn recognizes_thematic_breaks_but_not_prose() {
        // The scene-break forms an author might type.
        for br in ["---", "***", "___", "* * *", "- - -", "----"] {
            assert!(is_thematic_break(br), "{br:?} is a scene break");
            assert!(
                matches!(classify_block(br), Some(Prose::SceneBreak)),
                "{br:?}"
            );
        }
        // Prose that merely contains the same characters is not a break.
        for prose in [
            "a — dash",
            "5 * 3 = 15",
            "--",
            "well - maybe",
            "* one bullet",
        ] {
            assert!(!is_thematic_break(prose), "{prose:?} is prose, not a break");
        }
        // Headings and paragraphs still classify as before.
        assert!(matches!(classify_block("## A scene"), Some(Prose::Heading(h)) if h == "A scene"));
        assert!(classify_block("  ").is_none());
    }

    #[test]
    fn markdown_keeps_a_scene_break_as_a_thematic_break() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![chapter("One", None, "Scene one.\n\n---\n\nScene two.")],
            }],
            back: vec![],
        };
        assert!(
            to_markdown(&d).contains("Scene one.\n\n---\n\nScene two."),
            "{}",
            to_markdown(&d)
        );
    }

    #[test]
    fn a_multiline_epigraph_quotes_every_line() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: Some(epigraph("Roses are red\n\nViolets are blue", None)),
            front: vec![],
            parts: vec![],
            back: vec![],
        };
        let md = to_markdown(&d);
        assert!(
            md.contains("> Roses are red\n>\n> Violets are blue\n"),
            "{md}"
        );
    }
}

//! The built-in preset library (#473, #482) and the heading-alignment allowlist
//! (#470): the named looks an author can pick, their one-line picker blurbs, and
//! the base [`ExportStyle`] each resolves to. Presets set only the asset-free
//! knobs so any author can use one without bundling licensed fonts or artwork.

use super::ExportStyle;

/// The CSS `text-align` values a heading may use (#470) — an allowlist, since the
/// value is interpolated into the stylesheet. Public so the in-app style setter
/// (#569) offers exactly the values the writers honor.
pub const HEADING_ALIGNS: [&str; 4] = ["left", "center", "right", "justify"];

/// The built-in preset names (#473, #482), in the order shown to authors. The
/// single source of truth: [`preset_base`] resolves each one and the unknown-preset
/// warning lists them, so the library can't drift between code and the message (a
/// test asserts every name here resolves).
pub const PRESET_NAMES: [&str; 7] = [
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
pub(super) fn preset_base(name: &str) -> Option<ExportStyle> {
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
pub fn preset_blurb(name: &str) -> &'static str {
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

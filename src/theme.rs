//! Color themes (#300). The editor's colors are a named set of
//! [`ratatui::style::Color`]s in one [`Theme`] struct, threaded through the render
//! path instead of inline literals, so the palette is data, not code. Two
//! built-ins ship — `dark` (the historical default, so an unset theme looks
//! exactly as before) and `light` — and any field can be overridden from the
//! `[theme]` config table. A bad theme name or color value is reported and falls
//! back to the default rather than rendering garbage.

use std::cell::Cell;
use std::collections::HashMap;
use std::str::FromStr;

use ratatui::style::Color;

thread_local! {
    /// The theme the render path reads (#300). Set from `App::theme` at startup
    /// and at the top of every `ui::draw`, so styling helpers can read the active
    /// palette without threading a `&Theme` through the whole (and hot) render
    /// tree. Per-thread, and overwritten on each draw, so parallel tests (each
    /// rendering its own `App`) don't interfere; the lazy default is the dark
    /// theme, which is Quill's historical palette.
    static CURRENT: Cell<Theme> = Cell::new(Theme::dark());
}

/// Make `theme` the active theme for subsequent rendering on this thread.
pub fn set_current(theme: Theme) {
    CURRENT.with(|c| c.set(theme));
}

/// The active theme — the palette the render path styles with (#300). `ui::draw`
/// sets it from the app being drawn at the top of every frame, so render code
/// always reads the right palette. A test that asserts colors on content built
/// *outside* a draw (a panel builder called directly) should `set_current` first;
/// otherwise it reads the per-thread default (the dark = historical palette).
pub fn current() -> Theme {
    CURRENT.with(Cell::get)
}

/// Declare the themable colors once (#434): generate the [`Theme`] struct field,
/// the `set_field` arm, the `COLOR_FIELDS` override-key list, and the
/// describe-to-config role blurb from a single `name => "role"` (+ doc) per
/// color, so adding one is a single line, not four coordinated edits. The
/// per-theme values live in [`Theme::dark`] / [`Theme::light`], which assign
/// these fields by name.
macro_rules! theme_colors {
    ($($(#[$attr:meta])* $name:ident => $role:literal),+ $(,)?) => {
        /// The editor's color palette — one named [`Color`] per semantic role.
        /// Emphasis (bold / italic) is a text modifier, not a color, so it isn't
        /// themed; prose uses the terminal's default foreground, so it isn't here.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Theme {
            $($(#[$attr])* pub $name: Color,)+
        }

        impl Theme {
            /// Set the field named `key` (the `[theme]` config key) to `color`;
            /// returns `false` for an unknown field.
            fn set_field(&mut self, key: &str, color: Color) -> bool {
                match key {
                    $(stringify!($name) => self.$name = color,)+
                    _ => return false,
                }
                true
            }
        }

        /// The overridable color-field keys (the `[theme]` keys other than
        /// `name`), for the describe-to-config vocabulary (#303) and any other
        /// schema-driven caller.
        pub(crate) const COLOR_FIELDS: &[&str] = &[$(stringify!($name)),+];

        /// A short human description of what a theme color field controls, for
        /// the describe-to-config vocabulary (#303). Falls back to a generic
        /// phrase for an unknown key.
        pub(crate) fn color_role(field: &str) -> &'static str {
            match field {
                $(stringify!($name) => $role,)+
                _ => "a theme element",
            }
        }
    };
}

theme_colors! {
    /// Headings, and the accent used for active rows and banner backgrounds.
    heading => "headings and accents",
    /// Foreground for text on an accent / match / mode-chip background.
    on_accent => "text on an accent/match background",
    /// Secondary text and chrome: hints, dividers, attributions, diff context.
    dim => "secondary text and chrome (hints, dividers)",
    /// Muted prose set off from the body — epigraph text, placeholders.
    muted => "muted prose (epigraphs)",
    /// Background of the current search match (the bright one).
    match_current => "the current search match background",
    /// Background of the other (non-current) search matches.
    match_other => "other search matches' background",
    /// The status bar's background.
    status_bg => "the status bar background",
    /// Additions: diff `+`, a positive day's net, the `Final` chapter status.
    added => "additions (diff +, gains)",
    /// Removals: diff `-`, a negative day's net.
    removed => "removals (diff -, losses)",
    /// The `Revised` chapter status.
    revised => "the 'revised' chapter status",
    /// The Insert-mode status indicator.
    mode_insert => "the Insert-mode indicator",
    /// The Normal-mode status indicator.
    mode_normal => "the Normal-mode indicator",
    /// The Visual-mode status indicator.
    mode_visual => "the Visual-mode indicator",
    /// Background of an inline author annotation's anchored span (#302).
    annotation => "an inline author annotation's highlight",
}

impl Default for Theme {
    /// The historical palette, so a project with no `[theme]` set looks exactly
    /// as Quill always has.
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The built-in dark theme — Quill's original hard-coded colors.
    pub fn dark() -> Self {
        Self {
            heading: Color::Cyan,
            on_accent: Color::Black,
            dim: Color::DarkGray,
            muted: Color::Gray,
            match_current: Color::Yellow,
            match_other: Color::DarkGray,
            status_bg: Color::Rgb(30, 30, 36),
            added: Color::Green,
            removed: Color::Red,
            revised: Color::Yellow,
            mode_insert: Color::Green,
            mode_normal: Color::Blue,
            mode_visual: Color::Magenta,
            annotation: Color::Rgb(80, 70, 30),
        }
    }

    /// A built-in light theme: darker grays and a light status bar for legibility
    /// on a light terminal background.
    pub fn light() -> Self {
        Self {
            heading: Color::Blue,
            on_accent: Color::Black,
            dim: Color::Rgb(90, 90, 96),
            muted: Color::Rgb(110, 110, 116),
            match_current: Color::Yellow,
            match_other: Color::Rgb(205, 205, 205),
            status_bg: Color::Rgb(225, 225, 230),
            added: Color::Rgb(0, 135, 0),
            removed: Color::Rgb(175, 0, 0),
            revised: Color::Rgb(160, 110, 0),
            mode_insert: Color::Rgb(0, 135, 0),
            mode_normal: Color::Blue,
            mode_visual: Color::Magenta,
            annotation: Color::Rgb(250, 240, 180),
        }
    }

    /// The built-in theme named `name` (case-insensitive), or `None` if unknown.
    fn builtin(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            _ => None,
        }
    }

    /// Build a theme from the `[theme]` config: start from the named built-in
    /// (default `dark`), then apply per-field color overrides. Returns the theme
    /// plus any warnings (an unknown theme name, an unknown field, or an
    /// unparseable color) so the caller can surface them — a bad value falls back
    /// to the default for that field rather than failing the whole load.
    pub fn from_config(
        name: Option<&str>,
        overrides: &HashMap<String, String>,
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut theme = match name {
            Some(n) => Self::builtin(n).unwrap_or_else(|| {
                warnings.push(format!("unknown theme '{n}' — using 'dark'"));
                Self::dark()
            }),
            None => Self::dark(),
        };
        // Apply overrides in a stable order so warnings read consistently.
        let mut keys: Vec<&String> = overrides.keys().collect();
        keys.sort();
        for key in keys {
            let raw = &overrides[key];
            let Some(color) = parse_color(raw) else {
                warnings.push(format!("theme: invalid color '{raw}' for '{key}'"));
                continue;
            };
            if !theme.set_field(key, color) {
                warnings.push(format!("theme: unknown color field '{key}'"));
            }
        }
        (theme, warnings)
    }
}

/// Whether `s` parses as a color value (for validating a proposed override).
pub(crate) fn is_color(s: &str) -> bool {
    parse_color(s).is_some()
}

/// Parse a color from config: a named ANSI color (`cyan`, `darkgray`, …), a hex
/// string (`#rrggbb` or `#rgb`), an `r,g,b` triple, or a 0–255 palette index.
/// `None` on anything unrecognized.
///
/// Names, `#rrggbb`, and the palette index are handled by ratatui's own
/// [`Color::from_str`] (which also accepts `grey`/`bright-*`/`silver` spellings);
/// this only adds the `#rgb` shorthand and the `r,g,b` triple, which it doesn't.
fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    // `#rgb` shorthand — ratatui's parser only accepts the 6-digit `#rrggbb`.
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 3 {
            return parse_hex_short(hex);
        }
    }
    // `r,g,b` triple — ratatui has no comma form.
    if s.contains(',') {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if let [r, g, b] = parts[..] {
            return Some(Color::Rgb(
                r.parse().ok()?,
                g.parse().ok()?,
                b.parse().ok()?,
            ));
        }
        return None;
    }
    Color::from_str(s).ok()
}

/// `#rgb` shorthand: each hex digit doubled (`#abc` → `#aabbcc`).
fn parse_hex_short(hex: &str) -> Option<Color> {
    let d = |c: &str| u8::from_str_radix(c, 16).ok().map(|v| v * 17);
    Some(Color::Rgb(d(&hex[0..1])?, d(&hex[1..2])?, d(&hex[2..3])?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_theme_set_is_the_dark_default() {
        let (theme, warnings) = Theme::from_config(None, &HashMap::new());
        assert_eq!(theme, Theme::dark());
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_named_builtin_loads() {
        let (theme, warnings) = Theme::from_config(Some("light"), &HashMap::new());
        assert_eq!(theme, Theme::light());
        assert!(warnings.is_empty());
    }

    #[test]
    fn an_unknown_theme_name_warns_and_falls_back_to_dark() {
        let (theme, warnings) = Theme::from_config(Some("solarized"), &HashMap::new());
        assert_eq!(theme, Theme::dark());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("solarized"), "{warnings:?}");
    }

    #[test]
    fn field_overrides_apply_over_the_base() {
        let overrides = HashMap::from([
            ("heading".to_string(), "#ff8800".to_string()),
            ("dim".to_string(), "240".to_string()),
            ("status_bg".to_string(), "10,20,30".to_string()),
        ]);
        let (theme, warnings) = Theme::from_config(Some("dark"), &overrides);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(theme.heading, Color::Rgb(0xff, 0x88, 0x00));
        assert_eq!(theme.dim, Color::Indexed(240));
        assert_eq!(theme.status_bg, Color::Rgb(10, 20, 30));
        // Untouched fields keep the base theme's values.
        assert_eq!(theme.added, Theme::dark().added);
    }

    #[test]
    fn a_bad_color_value_warns_and_keeps_the_default_for_that_field() {
        let overrides = HashMap::from([("heading".to_string(), "not-a-color".to_string())]);
        let (theme, warnings) = Theme::from_config(None, &overrides);
        assert_eq!(theme.heading, Theme::dark().heading, "kept the default");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("heading"), "{warnings:?}");
    }

    #[test]
    fn an_unknown_field_warns() {
        let overrides = HashMap::from([("backgruond".to_string(), "blue".to_string())]);
        let (_, warnings) = Theme::from_config(None, &overrides);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("backgruond"), "{warnings:?}");
    }

    #[test]
    fn every_color_field_is_settable() {
        // COLOR_FIELDS (the override vocabulary) and the fields `set_field`
        // accepts are now both generated from the one `theme_colors!` list, so
        // they can't drift; this still exercises that every key is settable.
        let mut t = Theme::dark();
        for &field in COLOR_FIELDS {
            assert!(
                t.set_field(field, Color::Red),
                "{field} should be a settable color field"
            );
        }
    }

    #[test]
    fn parse_color_accepts_names_hex_rgb_and_index() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("DarkGray"), Some(Color::DarkGray));
        assert_eq!(parse_color("#1e1e24"), Some(Color::Rgb(30, 30, 36)));
        assert_eq!(parse_color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(parse_color("30, 30, 36"), Some(Color::Rgb(30, 30, 36)));
        assert_eq!(parse_color("200"), Some(Color::Indexed(200)));
        assert_eq!(parse_color("chartreuse"), None);
        assert_eq!(parse_color("#12"), None);
        // Out-of-range index and rgb components fall through to `None`.
        assert_eq!(parse_color("256"), None);
        assert_eq!(parse_color("300,0,0"), None);
        // ratatui's spellings come for free via its `FromStr`.
        assert_eq!(parse_color("grey"), Some(Color::Gray));
    }
}

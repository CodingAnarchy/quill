//! The first-run quick-start card (#837): a dismissible overlay that introduces
//! the command palette, the core keybindings, and the enable-AI on-ramp to a
//! brand-new user. It's shown once — on the first launch, tracked by the
//! `[general] onboarding_shown` config flag — and never again after it's
//! dismissed, but stays reachable on demand from the run-action menu / help.
//!
//! The card's text is built here, pure, so a test can assert it covers the palette
//! key, the core shortcuts, and the AI on-ramp without standing up an editor. The
//! flag read/write funnels through [`already_shown`] / [`mark_shown`], the same
//! hermetic config machinery the rest of the settings use (#528/#531), so the
//! first-run logic is unit-testable against a scratch config dir.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::config::{self, GeneralConfig};

/// Whether the quick-start card has already been shown and dismissed (#837), read
/// from the global `[general] onboarding_shown` flag. `true` suppresses the
/// first-run card; a fresh install (no config, or the flag unset) reads `false`,
/// so the card shows once.
pub fn already_shown() -> bool {
    GeneralConfig::load().onboarding_shown
}

/// Record that the quick-start card has been shown (#837): splice
/// `[general] onboarding_shown = true` into the home config, so it never appears
/// again on launch. Best-effort — a failed write leaves the flag unset (the card
/// may reappear next launch) rather than propagating; the caller reports nothing,
/// since a re-shown card is a mild annoyance, not a failure the author must act on.
pub fn mark_shown() {
    let changes = [(
        "general".to_string(),
        "onboarding_shown".to_string(),
        "true".to_string(),
    )];
    let _ = config::set_values(&changes);
}

/// The read-only banner lines prepended to the quick-start card when the project
/// opened read-only because it's already open in another live Quill instance
/// (#871/#885). Without this, the open-time read-only warning is momentary and
/// the first-run card overlays it, so a brand-new author never learns why their
/// saves are refused. Bold and leading, so it reads before the tour; the same
/// state also persists in the status bar after the card is dismissed. `holder`
/// names the instance that already holds the project.
pub fn read_only_banner_lines(holder: &str) -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    vec![
        Line::from(Span::styled(
            "Read-only: this project is already open in another Quill instance.".to_string(),
            bold,
        )),
        Line::from(Span::styled(
            format!("Saves are disabled while {holder} holds it."),
            bold,
        )),
        Line::raw(""),
    ]
}

/// The quick-start card's rendered lines (#837): a short, bold-titled tour of the
/// three things a new author most needs — the command palette, the core
/// keybindings, and turning on the (opt-in, off-by-default) AI assistance. Pure
/// (no I/O), so the overlay content is built the same in production and in tests.
pub fn card_lines() -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);

    // A `key — description` row, the key emphasized so the shortcuts scan at a
    // glance down the card.
    let key = |k: &'static str, desc: &'static str| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{k:<10}"), bold),
            Span::raw(desc.to_string()),
        ])
    };
    let heading = |text: &'static str| Line::from(Span::styled(text.to_string(), bold));

    vec![
        Line::from(Span::styled(
            "Welcome to Quill — a keyboard-first editor for novels.".to_string(),
            bold,
        )),
        Line::raw(""),
        Line::raw("Here are the essentials to get you writing. This card shows once;"),
        Line::from(vec![
            Span::raw("reopen it any time from the command palette ("),
            Span::styled("Ctrl-K".to_string(), bold),
            Span::raw(") → \"Quick-start\"."),
        ]),
        Line::raw(""),
        heading("The command palette"),
        key(
            "Ctrl-K",
            "open the command palette — every action, searchable by name.",
        ),
        Line::from(Span::styled(
            "  Start here: it's the one shortcut that reaches all the others.".to_string(),
            dim,
        )),
        Line::raw(""),
        heading("Core keys"),
        key("Ctrl-S", "save the current chapter"),
        key("Ctrl-O", "toggle the heading outline"),
        key(
            "Ctrl-L",
            "continuous view — read/edit adjacent chapters as one",
        ),
        key("Ctrl-N", "next chapter"),
        key("Ctrl-P", "previous chapter"),
        key("Ctrl-F", "find (and replace) in this chapter"),
        key("Ctrl-Q", "quit"),
        key("F1", "open the full user's guide"),
        Line::raw(""),
        heading("AI assistance (optional, off by default)"),
        Line::raw("  Quill's AI tracks and observes — a story bible, continuity flags,"),
        Line::raw("  outlines — but never writes your prose. It's opt-in and needs a key."),
        key(
            "Ctrl-K",
            "then \"Enable AI assistance\" to turn it on when you're ready.",
        ),
        Line::raw(""),
        Line::from(Span::styled("Press any key to dismiss.".to_string(), dim)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{config_path, set_config_dir};

    /// The card must introduce the three things the ticket calls out — the palette
    /// (Ctrl-K), the core keys, and the enable-AI on-ramp — so a new author lands on
    /// all of them from the one card. Flattened to plain text so the assertion
    /// isn't coupled to how the spans are styled.
    #[test]
    fn card_covers_palette_core_keys_and_the_ai_on_ramp() {
        let text: String = card_lines()
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Ctrl-K"), "names the palette key: {text}");
        assert!(text.contains("Ctrl-S"), "names a core key: {text}");
        assert!(
            text.contains("Enable AI"),
            "names the enable-AI on-ramp: {text}"
        );
    }

    /// The first-run flag round-trips through the config: unset on a fresh install
    /// (the card shows), set to `true` after [`mark_shown`] (it never shows again).
    /// Hermetic (#528/#531) — a scratch config dir, never the developer's real one.
    #[test]
    fn mark_shown_flips_the_first_run_flag_in_config() {
        let dir = crate::testutil::scratch_dir("onboarding", "first-run-flag");
        set_config_dir(Some(dir.clone()));

        // Fresh install: no config file → the flag reads false → the card shows.
        assert!(
            !already_shown(),
            "onboarding not yet shown on a fresh install"
        );

        // Dismissing it records the flag; a later launch reads true and suppresses.
        mark_shown();
        assert!(already_shown(), "the flag is set after mark_shown");
        // Persisted to the [general] section of the home config, not a project file.
        let written = std::fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(
            written.contains("[general]") && written.contains("onboarding_shown = true"),
            "flag spliced into [general]: {written}"
        );

        set_config_dir(None);
        std::fs::remove_dir_all(&dir).ok();
    }
}

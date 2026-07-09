//! The fuzzy chapter/scene quick-switcher overlay (#976): a `Ctrl-P`-style list of
//! every chapter and scene heading, narrowed live by the typed query. A scene row
//! is indented and shows its chapter dimmed as context; a chapter row stands on its
//! own. Renders through the shared [`draw_list_overlay`], like the other choosers.

use ratatui::{layout::Rect, Frame};

use super::{draw_list_overlay, list_row};
use crate::mode::QuickSwitch;

pub(in crate::ui) fn draw_quick_switch(frame: &mut Frame, area: Rect, switch: &QuickSwitch) {
    let matches = switch.matches();
    let rows = matches
        .iter()
        .enumerate()
        .map(|(pos, (_, t))| {
            let selected = pos == switch.selected.min(matches.len().saturating_sub(1));
            if t.is_scene {
                // A scene heading, indented under a "scene" tag, with its chapter
                // shown dimmed as context.
                list_row(
                    selected,
                    &format!("  ¶ {}", t.title),
                    Some(t.chapter_title.clone()),
                )
            } else {
                list_row(selected, &t.title, None)
            }
        })
        .collect();
    let help = if switch.query.is_empty() {
        "Quick switch — type to filter chapters & scenes · ↑↓ move · enter jump · esc cancel"
            .to_string()
    } else {
        format!(
            "Quick switch “{}_” — {} match{} · ↑↓ move · enter jump · esc cancel",
            switch.query,
            matches.len(),
            if matches.len() == 1 { "" } else { "es" }
        )
    };
    let selected = (!matches.is_empty()).then_some(switch.selected.min(matches.len() - 1));
    draw_list_overlay(frame, area, " Go to ", &help, rows, selected);
}

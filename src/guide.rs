//! The built-in user's guide (#281): a readable help document shipped *in* the
//! binary (via `include_str!`) so it's available offline and can never drift
//! from the build. Rendered as a scrollable overlay; opened with F1 or the
//! run-action menu.

/// The guide's text, bundled at build time.
pub const TEXT: &str = include_str!("../docs/guide.md");

/// The standalone configuration reference (#807), bundled at build time so the
/// drift guard below can assert every home-config section is documented in it
/// without reading a file at test time. Test-only: unlike the guide it isn't
/// rendered in-app, so it's compiled in solely for the drift guard.
#[cfg(test)]
pub const CONFIG_REFERENCE: &str = include_str!("../docs/configuration.md");

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `Ctrl-<letter>` keybinding documented in the README's "Keys" table
    /// must also appear in the guide, so the in-tool help can't silently fall
    /// behind the README as shortcuts are added or changed.
    #[test]
    fn guide_covers_every_readme_key() {
        const README: &str = include_str!("../README.md");

        // The Ctrl-<LETTER> tokens listed in the README "## Keys" table — bounded
        // to the table itself (up to the first "###" subsection) and to the
        // canonical uppercase form the table uses.
        let keys_section = README
            .split("## Keys")
            .nth(1)
            .and_then(|after| after.split("\n###").next())
            .expect("README has a Keys section");
        let mut want: Vec<String> = Vec::new();
        let bytes = keys_section.as_bytes();
        for (i, _) in keys_section.match_indices("Ctrl-") {
            // The character right after "Ctrl-"; collect only letter shortcuts.
            if let Some(&c) = bytes.get(i + 5) {
                if c.is_ascii_uppercase() {
                    let token = format!("Ctrl-{}", c as char);
                    if !want.contains(&token) {
                        want.push(token);
                    }
                }
            }
        }
        assert!(
            want.len() >= 10,
            "sanity: found the Keys table ({} keys)",
            want.len()
        );

        let missing: Vec<&String> = want.iter().filter(|k| !TEXT.contains(k.as_str())).collect();
        assert!(
            missing.is_empty(),
            "guide is missing README keys: {missing:?}"
        );
    }

    #[test]
    fn guide_mentions_f1_to_reopen_itself() {
        assert!(
            TEXT.contains("F1"),
            "the guide should tell the reader how to reopen it"
        );
    }

    /// Every home-config section loaded via `config::load_section("…")` must
    /// appear as a documented `[section]` header in the configuration reference
    /// (#807), so a newly added section can't ship undocumented. Keyed on the
    /// section-name set — the stable contract — rather than where a Rust struct
    /// happens to live, so it's robust to the `[ai]`/`[image]` type move (#809).
    #[test]
    fn config_reference_documents_every_home_config_section() {
        // The home-config sections each loaded through `config::load_section`
        // (see `ai::Settings::load`, `ImageSettings::load`, `EditorConfig::load`,
        // `load_theme`, `ProjectsConfig::load`, `load_snippets`, `workflow::load`).
        const SECTIONS: &[&str] = &[
            "ai",
            "image",
            "editor",
            "theme",
            "projects",
            "snippets",
            "workflows",
            "triggers",
        ];
        let missing: Vec<&str> = SECTIONS
            .iter()
            .copied()
            .filter(|s| !CONFIG_REFERENCE.contains(&format!("`[{s}]`")))
            .collect();
        assert!(
            missing.is_empty(),
            "docs/configuration.md is missing config sections: {missing:?}"
        );
    }

    /// Every field of the `[editor]` and `[projects]` sections must be documented
    /// by name in the configuration reference (#807). The names are listed here
    /// (mirroring `EditorConfig` / `ProjectsConfig`) rather than derived, so the
    /// guard needn't touch `config/types.rs` — adding a field means updating this
    /// list, which fails until the doc gains the key.
    #[test]
    fn config_reference_documents_every_editor_and_projects_field() {
        // Mirror of `EditorConfig` (incl. the `[editor.keys]` remap table) and
        // `ProjectsConfig` field names in `src/config/types.rs`.
        const FIELDS: &[&str] = &[
            "vim",
            "keys",
            "smart_typography",
            "column_width",
            "typewriter",
            "focus_mode",
            "paragraph_indent",
            "autosave_interval_secs",
            "home",
            "setup_wizard",
            "open_last",
        ];
        let missing: Vec<&str> = FIELDS
            .iter()
            .copied()
            .filter(|f| !CONFIG_REFERENCE.contains(&format!("`{f}`")))
            .collect();
        assert!(
            missing.is_empty(),
            "docs/configuration.md is missing editor/projects fields: {missing:?}"
        );
    }

    /// The reference must state the three ways to change a setting (#807): the
    /// config file, the in-app Settings menu, and `:set` / describe-by-description.
    #[test]
    fn config_reference_states_the_three_ways_to_change_a_setting() {
        assert!(CONFIG_REFERENCE.contains("config.toml"));
        assert!(CONFIG_REFERENCE.contains("Settings menu"));
        assert!(CONFIG_REFERENCE.contains(":set"));
    }
}

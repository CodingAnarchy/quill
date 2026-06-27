//! The built-in user's guide (#281): a readable help document shipped *in* the
//! binary (via `include_str!`) so it's available offline and can never drift
//! from the build. Rendered as a scrollable overlay; opened with F1 or the
//! run-action menu.

/// The guide's text, bundled at build time.
pub const TEXT: &str = include_str!("../docs/guide.md");

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
}

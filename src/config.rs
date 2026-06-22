//! User configuration shared across features, loaded from the user's home
//! config file (`~/.config/quill/config.toml`) — never the project directory, so
//! an unpublished draft and the user's settings stay separate. Each feature owns
//! its own section (`[ai]`, `[editor]`, …); this module owns the file location
//! and the `[editor]` section.

use std::path::PathBuf;

use serde::Deserialize;

/// `$XDG_CONFIG_HOME/quill/config.toml`, else `$HOME/.config/quill/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("quill").join("config.toml"))
}

/// Editor behavior from the `[editor]` section. The default keeps Quill's
/// modeless editing unchanged.
#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct EditorConfig {
    /// Opt-in Vim-style modal editing (#27). Off by default — with it off the
    /// editor behaves exactly as it always has.
    pub vim: bool,
}

impl EditorConfig {
    /// Load the `[editor]` section from the home config file. Best-effort: a
    /// missing or unreadable file yields the defaults (Vim off).
    pub fn load() -> Self {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct File {
            editor: EditorConfig,
        }
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str::<File>(&t).ok())
            .map(|f| f.editor)
            .unwrap_or_default()
    }
}

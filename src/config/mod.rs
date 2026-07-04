//! User configuration shared across features. The base layer is the user's home
//! config file (`~/.config/quill/config.toml`) — the global settings shared
//! across every project. On top of it sits an optional per-project layer inside
//! the project's `book.toml` manifest, under a `[quill]` table (#710): a project
//! may override any of the settings sections (`[quill.editor]`, `[quill.theme]`,
//! `[quill.snippets]`, `[quill.workflows]`, `[quill.projects]`, …) for that
//! project alone.
//!
//! The per-project layer lives in `book.toml` — the file that already carries the
//! project's committed configuration (`[goals]`, `[analysis]`) — rather than the
//! project's `.quill/` directory, which holds transient machine-local state
//! (recovery, history, cursor position) and is git-ignored. Putting the override
//! in `book.toml` means a project's settings travel with the book: they're
//! committed and shared, so a screenplay's wider column or its own workflow set is
//! part of the project, not a local artifact. Nesting under `[quill]` keeps the
//! override sections clear of the manifest's own tables.
//!
//! **Precedence is per field.** For each `[section]`, a key set in the project's
//! `[quill.section]` wins; any key it leaves unset falls back to the global value;
//! and a key neither layer sets falls back to the built-in default. With no
//! project override present, behavior is exactly the global-only behavior it always
//! was. The home config stays the base so the user's private settings stay out of
//! a shared project directory — the AI keys, for instance, live only in the global
//! file, never in a committed `book.toml`.
//!
//! Each feature owns its own section; this module owns both file locations and
//! the `[editor]` section.
//!
//! The module is split into cohesive submodules, re-exported here so every caller
//! keeps addressing them through `config::` unchanged: [`load`] holds the section
//! load/merge, [`splice`] the in-place TOML writers, and [`types`] the typed
//! config structs. Dir resolution and the per-thread test/project overrides
//! (#528/#531/#710) stay in this file, since both the loaders and the writers key
//! off them.

mod load;
mod splice;
mod types;

use std::ffi::OsString;
use std::path::PathBuf;

pub use load::load_section;
pub use splice::{add_workflow, remove_value, set_values};
pub use splice::{set_value, strip_value};
pub use types::theme_name;
pub use types::{load_snippets, load_theme, EditorConfig, GeneralConfig, ProjectsConfig, Snippet};

/// Quill's config directory: `$XDG_CONFIG_HOME/quill`, else `$HOME/.config/quill`.
/// Home of the config file and other global state shared across projects (e.g.
/// the recent-projects list, #270).
///
/// In test builds this is hermetic (#528): it returns a per-thread override
/// (default `None`) instead of the developer's real `~/.config`, so no test reads
/// the machine's config — and the AI backend, finding no key, never makes a live
/// request. A test that exercises config points it at a temp dir via
/// [`set_config_dir`].
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_CONFIG_DIR.with(|c| c.borrow().clone())
    }
    #[cfg(not(test))]
    {
        resolve_config_dir(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }
}

/// The config dir from the raw `$XDG_CONFIG_HOME` / `$HOME` values: XDG wins,
/// else `$HOME/.config`, else nothing. Pure, so the production resolution stays
/// unit-tested even though [`config_dir`] is hermetically stubbed in test builds
/// (#528).
fn resolve_config_dir(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("quill"))
}

#[cfg(test)]
thread_local! {
    /// Per-test override for [`config_dir`] (#528) — see its docs. Per-thread, so
    /// parallel tests don't race the way a process-global env var would.
    static TEST_CONFIG_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

thread_local! {
    /// The currently-open project's root directory, or `None` when no project is
    /// open (#710). Set by [`set_project_dir`] when a project opens; consulted by
    /// [`load_section`] to layer the project's `book.toml` `[quill]` overrides over
    /// the global config. Per-thread — like [`config_dir`]'s test override — so it
    /// stays hermetic in the parallel test suite (#531) and never leaks a project's
    /// overrides into an unrelated thread.
    static PROJECT_CONFIG_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Point config resolution at `dir` (or clear it with `None`) for the current
/// test thread (#528). Test-only; production resolves from `$XDG_CONFIG_HOME` /
/// `$HOME`.
#[cfg(test)]
pub fn set_config_dir(dir: Option<PathBuf>) {
    TEST_CONFIG_DIR.with(|c| *c.borrow_mut() = dir);
}

/// Register the open project's root so [`load_section`] layers that project's
/// `book.toml` `[quill]` overrides over the global config (#710). Called when a
/// project opens (and re-called on a switch); pass `None` to clear it. Per-thread,
/// so the override is scoped to the running editor and stays hermetic in tests
/// (#531).
pub fn set_project_dir(dir: Option<PathBuf>) {
    PROJECT_CONFIG_DIR.with(|c| *c.borrow_mut() = dir);
}

/// The open project's manifest, `<project>/book.toml` (#710), or `None` when no
/// project is registered. Its `[quill]` table is the per-project override layer:
/// any settings section under `[quill]` wins, field by field, over the global
/// config; keys it omits fall back to the global value.
fn project_manifest_path() -> Option<PathBuf> {
    PROJECT_CONFIG_DIR.with(|c| c.borrow().as_ref().map(|root| root.join("book.toml")))
}

/// `$XDG_CONFIG_HOME/quill/config.toml`, else `$HOME/.config/quill/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The suite must never read the developer's real config (#528): with no
    /// override, config resolution yields nothing, so every loader defaults — and
    /// the AI backend, finding no key, can't authenticate or reach the API.
    #[test]
    fn config_resolution_is_hermetic_by_default() {
        assert_eq!(config_dir(), None, "no real ~/.config is read in tests");
        let ai = crate::ai::Settings::load();
        assert!(!ai.enabled, "AI defaults off");
        assert!(ai.resolved_key().is_none(), "no API key leaks into tests");
    }

    #[test]
    fn production_resolution_prefers_xdg_then_home_config() {
        // `config_dir` is stubbed in tests, so cover the real resolver directly.
        assert_eq!(
            resolve_config_dir(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x/quill"))
        );
        assert_eq!(
            resolve_config_dir(None, Some("/h".into())),
            Some(PathBuf::from("/h/.config/quill"))
        );
        assert_eq!(resolve_config_dir(None, None), None);
    }

    #[test]
    fn set_config_dir_points_resolution_at_a_temp_dir_then_clears() {
        let dir = std::env::temp_dir().join("quill-cfg-hermetic-test");
        set_config_dir(Some(dir.clone()));
        assert_eq!(config_dir(), Some(dir.clone()));
        assert_eq!(config_path(), Some(dir.join("config.toml")));
        set_config_dir(None);
        assert_eq!(config_dir(), None, "cleared back to hermetic default");
    }
}

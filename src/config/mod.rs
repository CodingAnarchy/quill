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
//! This module owns the file locations and the typed home for every config
//! section, so "what is Quill's config shape?" is answerable by reading one place
//! (#809). Each feature still owns its section's *behavior*, but the section's
//! *data* — the serde-`Deserialize` fields — lives here, including the `[ai]` and
//! `[image]` sections ([`AiConfig`], [`ImageConfig`]) whose key-resolution and
//! backend logic stays in the `ai` module as inherent impls on these types.
//!
//! The module is split into cohesive submodules, re-exported here so every caller
//! keeps addressing them through `config::` unchanged: [`load`] holds the section
//! load/merge, [`splice`] the in-place TOML writers, [`types`] the `[editor]`,
//! `[projects]`, `[snippets]`, and `[theme]` types, and [`ai`] the `[ai]`/`[image]`
//! config data. Dir resolution and the per-thread test/project overrides
//! (#528/#531/#710) stay in this file, since both the loaders and the writers key
//! off them.

mod ai;
mod load;
mod splice;
mod types;

use std::ffi::OsString;
use std::path::PathBuf;

pub use ai::{AiConfig, BackendKind, ImageBackendKind, ImageConfig, Policy};
pub use load::{forget_parsed, global_config_warning, load_section, LoadSession};
pub use splice::{add_workflow, remove_value, set_values};
pub use splice::{set_value, strip_value};
pub use types::{
    load_snippets, load_theme, BackupConfig, EditorConfig, EditorFlags, GeneralConfig,
    ProjectsConfig, Snippet,
};
pub use types::{theme_color_overrides, theme_name};

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

/// Quill's data directory: `$XDG_DATA_HOME/quill`, else `$HOME/.local/share/quill`
/// (#888). Distinct from [`config_dir`] — this holds generated, machine-local data
/// (the scheduled local archive backups) rather than the user's settings, matching
/// the XDG split of config vs. data.
///
/// In test builds this is hermetic like [`config_dir`] (#528): it returns a
/// per-thread override (default `None`) instead of the developer's real
/// `~/.local/share`, so no test writes to the machine's data dir. A test that
/// exercises backups points it at a temp dir via [`set_data_dir`].
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        TEST_DATA_DIR.with(|c| c.borrow().clone())
    }
    #[cfg(not(test))]
    {
        resolve_data_dir(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
    }
}

/// The data dir from the raw `$XDG_DATA_HOME` / `$HOME` values: XDG wins, else
/// `$HOME/.local/share`, else nothing. Pure, so the production resolution stays
/// unit-tested even though [`data_dir`] is hermetically stubbed in tests (#888).
#[cfg_attr(test, allow(dead_code))]
fn resolve_data_dir(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg
        .map(PathBuf::from)
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local").join("share")))?;
    Some(base.join("quill"))
}

/// The directory holding a project's scheduled local archive backups (#888):
/// `<data_dir>/backups/<project-folder-name>`. `None` when no data dir resolves
/// (no `$HOME`, or hermetically stubbed off in tests). The project's own folder
/// name keys the subdirectory so each book's archives stay separate; an oddly
/// named root falls back to `project`.
pub fn backups_dir(project_root: &std::path::Path) -> Option<PathBuf> {
    let name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "project".into());
    Some(data_dir()?.join("backups").join(name))
}

#[cfg(test)]
thread_local! {
    /// Per-test override for [`data_dir`] (#888) — mirrors [`TEST_CONFIG_DIR`], so
    /// backup tests write to a scratch dir and never the developer's real
    /// `~/.local/share`. Per-thread for the parallel suite.
    static TEST_DATA_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Point [`data_dir`] resolution at `dir` (or clear it with `None`) for the current
/// test thread (#888). Test-only; production resolves from `$XDG_DATA_HOME` /
/// `$HOME`.
#[cfg(test)]
pub fn set_data_dir(dir: Option<PathBuf>) {
    TEST_DATA_DIR.with(|c| *c.borrow_mut() = dir);
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
        let ai = AiConfig::load();
        assert!(!ai.enabled, "AI defaults off");
        // `resolved_key` is defined in the `ai` module as an inherent impl on the
        // config-owned `AiConfig` (#809) — data here, behavior there.
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

    #[test]
    fn production_data_dir_prefers_xdg_then_home_local_share() {
        // `data_dir` is stubbed in tests, so cover the real resolver directly (#888).
        assert_eq!(
            resolve_data_dir(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x/quill"))
        );
        assert_eq!(
            resolve_data_dir(None, Some("/h".into())),
            Some(PathBuf::from("/h/.local/share/quill"))
        );
        assert_eq!(resolve_data_dir(None, None), None);
    }

    #[test]
    fn data_dir_is_hermetic_by_default_and_backups_dir_keys_by_project() {
        assert_eq!(data_dir(), None, "no real ~/.local/share is read in tests");
        let dir = std::env::temp_dir().join("quill-data-hermetic-test");
        set_data_dir(Some(dir.clone()));
        assert_eq!(data_dir(), Some(dir.clone()));
        // A project's backups dir keys off the project folder name.
        assert_eq!(
            backups_dir(std::path::Path::new("/tmp/My Novel")),
            Some(dir.join("backups").join("My Novel"))
        );
        set_data_dir(None);
        assert_eq!(data_dir(), None, "cleared back to hermetic default");
        assert_eq!(backups_dir(std::path::Path::new("/tmp/x")), None);
    }
}

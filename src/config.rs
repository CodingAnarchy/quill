//! User configuration shared across features, loaded from the user's home
//! config file (`~/.config/quill/config.toml`) — never the project directory, so
//! an unpublished draft and the user's settings stay separate. Each feature owns
//! its own section (`[ai]`, `[editor]`, …); this module owns the file location
//! and the `[editor]` section.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;

use crate::theme::Theme;

/// Quill's config directory: `$XDG_CONFIG_HOME/quill`, else `$HOME/.config/quill`.
/// Home of the config file and other global state shared across projects (e.g.
/// the recent-projects list, #270).
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("quill"))
}

/// `$XDG_CONFIG_HOME/quill/config.toml`, else `$HOME/.config/quill/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

/// Add a `[workflows]` entry to the home config file (#232), creating the file /
/// directory if needed. Inserts only the one line, preserving everything else in
/// the file (other sections, comments, formatting) — so it's safe to write a
/// config the author hand-maintains. Returns the path written.
pub fn add_workflow(name: &str, steps: &[String]) -> io::Result<PathBuf> {
    let path = config_path().ok_or_else(|| io::Error::other("no config directory"))?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = insert_workflow(&existing, name, steps);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, updated)?;
    Ok(path)
}

/// Splice a workflow entry into a config file's text: under an existing
/// `[workflows]` header if there is one, else in a new `[workflows]` section
/// appended at the end. Everything already in the file is left untouched.
fn insert_workflow(existing: &str, name: &str, steps: &[String]) -> String {
    let entry = workflow_entry(name, steps);

    if let Some(header) = existing.lines().position(|l| l.trim() == "[workflows]") {
        let mut lines: Vec<String> = existing.lines().map(String::from).collect();
        lines.insert(header + 1, entry);
        let mut out = lines.join("\n");
        if existing.ends_with('\n') {
            out.push('\n');
        }
        out
    } else {
        let mut out = existing.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("[workflows]\n");
        out.push_str(&entry);
        out.push('\n');
        out
    }
}

/// Render one `name = ["step", …]` entry, delegating key quoting and string
/// escaping to the `toml` serializer (a one-key table serializes to exactly this
/// line) so they always match the format the loader parses.
fn workflow_entry(name: &str, steps: &[String]) -> String {
    let array = steps.iter().cloned().map(toml::Value::String).collect();
    let mut table = toml::Table::new();
    table.insert(name.to_string(), toml::Value::Array(array));
    toml::to_string(&table).unwrap_or_default().trim_end().to_string()
}

/// Editor behavior from the `[editor]` section. The default keeps Quill's
/// modeless editing unchanged (but with smart typography on — see below).
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// Opt-in Vim-style modal editing (#27). Off by default — with it off the
    /// editor behaves exactly as it always has.
    pub vim: bool,
    /// Vim key remaps (#208) from the `[editor.keys]` table: action name → the
    /// key that should trigger it (e.g. `down = "n"`). Validated when the editor
    /// builds its keymap; unknown actions are reported, not silently dropped.
    pub keys: HashMap<String, String>,
    /// Smart typography (#296): as you type, straight quotes become curly
    /// (`“” ‘’`), `--` becomes an em-dash (`—`), and `...` becomes an ellipsis
    /// (`…`). **On by default** — set `smart_typography = false` for plain ASCII
    /// punctuation (and an exact byte-for-byte round-trip of what you type).
    pub smart_typography: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self { vim: false, keys: HashMap::new(), smart_typography: true }
    }
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

/// Load the color theme from the `[theme]` section (#300): an optional `name`
/// (`dark` / `light`) plus any per-field color overrides, all string values.
/// Returns the theme and any warnings (bad name / field / color) for the caller
/// to surface. A missing section (or a malformed one) yields the default theme.
pub fn load_theme() -> (Theme, Vec<String>) {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct File {
        // The whole `[theme]` table as string keys; `name` is pulled out, the
        // rest are color-field overrides. Keeping it untyped means an unknown key
        // is a reported warning, not a parse failure.
        theme: HashMap<String, String>,
    }
    let mut table = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<File>(&t).ok())
        .map(|f| f.theme)
        .unwrap_or_default();
    let name = table.remove("name");
    Theme::from_config(name.as_deref(), &table)
}

/// The `[projects]` section (#272): where the project browser looks for books
/// beyond the recently-opened list.
#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct ProjectsConfig {
    /// A folder holding your projects (e.g. `~/Writing`); the project browser
    /// lists every project under it. `~` expands to the home directory. Absent
    /// when unset — the browser then shows only recent projects.
    pub home: Option<String>,
    /// Whether a newly-created project offers the setup wizard (#282). Defaults
    /// to on; set `setup_wizard = false` to never be prompted.
    pub setup_wizard: Option<bool>,
}

impl ProjectsConfig {
    /// Load the `[projects]` section from the home config file. Best-effort: a
    /// missing or unreadable file yields the defaults (no projects home).
    pub fn load() -> Self {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct File {
            projects: ProjectsConfig,
        }
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str::<File>(&t).ok())
            .map(|f| f.projects)
            .unwrap_or_default()
    }

    /// The configured projects-home as an absolute path with `~` expanded, if set.
    pub fn home_dir(&self) -> Option<PathBuf> {
        self.home.as_deref().map(expand_tilde)
    }

    /// Whether a new project should offer the setup wizard (#282); on by default.
    pub fn wizard_enabled(&self) -> bool {
        self.setup_wizard.unwrap_or(true)
    }
}

/// Expand a leading `~` / `~/` to the home directory; leave any other path as-is.
fn expand_tilde(path: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if path == "~" {
        if let Some(h) = home() {
            return h;
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(h) = home() {
            return h.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn projects_home_parses_and_expands_tilde() {
        let cfg: ProjectsConfig = toml::from_str("home = \"~/Writing\"").unwrap();
        let dir = cfg.home_dir().expect("home set");
        assert!(dir.is_absolute(), "~ expanded to an absolute path: {dir:?}");
        assert!(dir.ends_with("Writing"));
    }

    #[test]
    fn projects_home_passes_an_absolute_path_through() {
        let cfg = ProjectsConfig { home: Some("/books".into()), ..Default::default() };
        assert_eq!(cfg.home_dir(), Some(PathBuf::from("/books")));
    }

    #[test]
    fn the_setup_wizard_is_on_by_default_and_opt_out() {
        assert!(ProjectsConfig::default().wizard_enabled(), "on by default");
        let off: ProjectsConfig = toml::from_str("setup_wizard = false").unwrap();
        assert!(!off.wizard_enabled(), "opt out with setup_wizard = false");
    }

    #[test]
    fn no_projects_home_is_none() {
        assert_eq!(ProjectsConfig::default().home_dir(), None);
    }

    #[test]
    fn inserts_under_an_existing_workflows_section() {
        let existing = "[editor]\nvim = true\n\n[workflows]\nfinish = [\"save\"]\n";
        let out = insert_workflow(existing, "revise", &steps(&["save", "ai:continuity"]));
        // The new entry is under the header, and the existing content is intact.
        assert!(out.contains("[workflows]\nrevise = [\"save\", \"ai:continuity\"]\n"));
        assert!(out.contains("finish = [\"save\"]"));
        assert!(out.contains("[editor]\nvim = true"));
    }

    #[test]
    fn appends_a_new_section_when_none_exists() {
        let existing = "[editor]\nvim = true\n";
        let out = insert_workflow(existing, "revise", &steps(&["save"]));
        assert!(out.contains("[editor]\nvim = true"), "existing kept: {out}");
        assert!(out.ends_with("[workflows]\nrevise = [\"save\"]\n"), "appended: {out}");
    }

    #[test]
    fn writes_a_fresh_file_when_empty() {
        let out = insert_workflow("", "x", &steps(&["save"]));
        assert_eq!(out, "[workflows]\nx = [\"save\"]\n");
    }

    #[test]
    fn a_name_needing_quotes_is_quoted() {
        let out = insert_workflow("", "my pass", &steps(&["save"]));
        assert!(out.contains("\"my pass\" = [\"save\"]"), "{out}");
    }
}

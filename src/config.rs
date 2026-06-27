//! User configuration shared across features, loaded from the user's home
//! config file (`~/.config/quill/config.toml`) — never the project directory, so
//! an unpublished draft and the user's settings stay separate. Each feature owns
//! its own section (`[ai]`, `[editor]`, …); this module owns the file location
//! and the `[editor]` section.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
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

/// Write a batch of `key = value` settings into the config file (#303), each
/// under its `[section]`, preserving everything else in the file (other keys,
/// sections, and comments). `value` is a ready-to-write TOML scalar literal
/// (e.g. `true`, `64`, `"#1e1e24"`). Creates the file/section if absent.
pub fn set_values(changes: &[(String, String, String)]) -> io::Result<PathBuf> {
    let path = config_path().ok_or_else(|| io::Error::other("no config directory"))?;
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for (section, key, value) in changes {
        text = set_value(&text, section, key, value);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Set `key = value` under `[section]` in `existing`, replacing the key in place
/// if present, inserting it under the header if the section exists, or appending
/// a new section otherwise. Everything else is left byte-for-byte.
fn set_value(existing: &str, section: &str, key: &str, value: &str) -> String {
    let entry = format!("{key} = {value}");
    let header = format!("[{section}]");
    let mut lines: Vec<String> = existing.lines().map(String::from).collect();
    let trailing_nl = existing.is_empty() || existing.ends_with('\n');

    if let Some(h) = lines.iter().position(|l| l.trim() == header) {
        // The section body runs to the next `[…]` header (or end of file).
        let end = lines[h + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map_or(lines.len(), |i| h + 1 + i);
        let existing_key = (h + 1..end).find(|&i| {
            lines[i].split_once('=').is_some_and(|(lhs, _)| lhs.trim() == key)
        });
        match existing_key {
            Some(k) => lines[k] = entry,
            None => lines.insert(h + 1, entry),
        }
    } else {
        if lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(String::new()); // a blank line before the new section
        }
        lines.push(header);
        lines.push(entry);
    }

    let mut out = lines.join("\n");
    if trailing_nl {
        out.push('\n');
    }
    out
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
    /// The centered text column width in characters (#303). Defaults to 72; the
    /// runtime `+`/`-` adjustment still applies on top, this just sets the start.
    pub column_width: u16,
    /// Typewriter scrolling (#301): keep the cursor's line pinned at the vertical
    /// centre of the page, the text scrolling under it, instead of letting the
    /// cursor drift to the bottom as a chapter grows. Off by default.
    pub typewriter: bool,
    /// Focus mode (#301): dim every paragraph but the one under the cursor, so the
    /// passage you're shaping stands out. Off by default.
    pub focus_mode: bool,
    /// Autosave interval in seconds (#409): after this many seconds with no input,
    /// the active chapter is saved automatically through the same atomic,
    /// front-matter-preserving path as Ctrl-S (but without firing on-save
    /// workflows). `0` (the default) turns autosave off — behavior unchanged.
    pub autosave_interval_secs: u64,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim: false,
            keys: HashMap::new(),
            smart_typography: true,
            column_width: 72,
            typewriter: false,
            focus_mode: false,
            autosave_interval_secs: 0,
        }
    }
}

/// Read one `[section]` table from the home config file, deserialized into `T`
/// (#430). Best-effort: a missing/unreadable/malformed file, or an absent or
/// malformed section, yields `T::default()`. Collapses the per-section
/// wrapper-struct idiom (`struct File { <section>: T }`) every loader used to
/// re-derive into one call. The config file is small, so re-reading it per
/// section at startup is negligible.
pub(crate) fn load_section<T: DeserializeOwned + Default>(section: &str) -> T {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .and_then(|mut table| table.remove(section))
        .and_then(|value| value.try_into().ok())
        .unwrap_or_default()
}

impl EditorConfig {
    /// Load the `[editor]` section from the home config file. Best-effort: a
    /// missing or unreadable file yields the defaults (Vim off).
    pub fn load() -> Self {
        load_section("editor")
    }
}

/// Load the color theme from the `[theme]` section (#300): an optional `name`
/// (`dark` / `light`) plus any per-field color overrides, all string values.
/// Returns the theme and any warnings (bad name / field / color) for the caller
/// to surface. A missing section (or a malformed one) yields the default theme.
pub fn load_theme() -> (Theme, Vec<String>) {
    // The whole `[theme]` table as string keys; `name` is pulled out, the rest
    // are color-field overrides. Untyped (string values) so an unknown key is a
    // reported warning, not a parse failure.
    let mut table: HashMap<String, String> = load_section("theme");
    let name = table.remove("name");
    Theme::from_config(name.as_deref(), &table)
}

/// An author-defined text snippet (#411): a `name` and the literal `text` it
/// expands to. Configured in the `[snippets]` table and inserted verbatim from
/// the snippet picker — no transformation, no templating.
#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub text: String,
}

/// Load the `[snippets]` section (#411): a table mapping each snippet's name to
/// its literal expansion (a string). Returns the snippets, sorted by name for a
/// stable menu, plus a warning for any non-string entry (reported and skipped,
/// like the other config sections). A missing or empty table yields nothing.
pub fn load_snippets() -> (Vec<Snippet>, Vec<String>) {
    // Untyped values (`toml::Value`) so a malformed entry (e.g. an array) is a
    // reported, skipped warning rather than failing the whole section's parse.
    let table: HashMap<String, toml::Value> = load_section("snippets");
    snippets_from_table(table)
}

/// Turn a parsed `[snippets]` table into sorted snippets plus a warning per
/// non-string entry. Separate from [`load_snippets`] so the rule is testable
/// without a config file on disk (like `validate_triggers` for workflows).
fn snippets_from_table(table: HashMap<String, toml::Value>) -> (Vec<Snippet>, Vec<String>) {
    let mut snippets = Vec::new();
    let mut errors = Vec::new();
    for (name, value) in table {
        match value.as_str() {
            Some(text) => snippets.push(Snippet { name, text: text.to_string() }),
            None => errors.push(format!("snippet '{name}' must be a string")),
        }
    }
    snippets.sort_by(|a, b| a.name.cmp(&b.name));
    (snippets, errors)
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
        load_section("projects")
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
    fn snippets_parse_sorts_strings_and_reports_non_strings() {
        let table = toml::from_str("zeta = \"the end\"\nalpha = \"* * *\"\nbad = 42\n").unwrap();
        let (snippets, errors) = snippets_from_table(table);
        // Kept entries are the string values, sorted by name for a stable menu.
        assert_eq!(
            snippets,
            vec![
                Snippet { name: "alpha".into(), text: "* * *".into() },
                Snippet { name: "zeta".into(), text: "the end".into() },
            ],
        );
        // The non-string entry is reported (and skipped), not a parse failure.
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("bad"), "{errors:?}");
    }

    #[test]
    fn an_empty_snippets_table_yields_nothing() {
        let (snippets, errors) = snippets_from_table(HashMap::new());
        assert!(snippets.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn set_value_replaces_an_existing_key_in_place() {
        let existing = "[editor]\nvim = false\nsmart_typography = true\n";
        let out = set_value(existing, "editor", "vim", "true");
        assert!(out.contains("vim = true"), "{out}");
        assert!(!out.contains("vim = false"), "{out}");
        assert!(out.contains("smart_typography = true"), "other keys kept: {out}");
    }

    #[test]
    fn set_value_inserts_under_an_existing_section() {
        let existing = "[editor]\nvim = true\n";
        let out = set_value(existing, "editor", "column_width", "64");
        assert!(out.contains("[editor]"), "{out}");
        assert!(out.contains("vim = true") && out.contains("column_width = 64"), "{out}");
    }

    #[test]
    fn set_value_appends_a_new_section_preserving_the_rest() {
        let existing = "[editor]\nvim = true\n";
        let out = set_value(existing, "theme", "name", "\"light\"");
        assert!(out.contains("[editor]\nvim = true"), "existing section kept: {out}");
        assert!(out.contains("[theme]") && out.contains("name = \"light\""), "{out}");
    }

    #[test]
    fn set_value_writes_a_fresh_file_when_empty() {
        let out = set_value("", "theme", "heading", "\"cyan\"");
        assert_eq!(out, "[theme]\nheading = \"cyan\"\n");
    }

    #[test]
    fn set_value_leaves_comments_on_other_lines() {
        let existing = "# my settings\n[editor]\n# turn on vim\nvim = false\n";
        let out = set_value(existing, "editor", "vim", "true");
        assert!(out.contains("# my settings") && out.contains("# turn on vim"), "comments kept: {out}");
        assert!(out.contains("vim = true"), "{out}");
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

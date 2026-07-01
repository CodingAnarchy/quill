//! User configuration shared across features, loaded from the user's home
//! config file (`~/.config/quill/config.toml`) — never the project directory, so
//! an unpublished draft and the user's settings stay separate. Each feature owns
//! its own section (`[ai]`, `[editor]`, …); this module owns the file location
//! and the `[editor]` section.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::theme::Theme;

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

/// Point config resolution at `dir` (or clear it with `None`) for the current
/// test thread (#528). Test-only; production resolves from `$XDG_CONFIG_HOME` /
/// `$HOME`.
#[cfg(test)]
pub(crate) fn set_config_dir(dir: Option<PathBuf>) {
    TEST_CONFIG_DIR.with(|c| *c.borrow_mut() = dir);
}

/// `$XDG_CONFIG_HOME/quill/config.toml`, else `$HOME/.config/quill/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.toml"))
}

/// Read the config file ahead of an in-place splice + rewrite. A missing file is
/// the legitimate first-run case (`""`, the splice creates it); any other read
/// error is propagated so the rewrite aborts rather than overwriting the user's
/// hand-maintained config with just the spliced entry (#565). "Empty" stays
/// distinct from "read failed".
fn read_existing_config(path: &std::path::Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Add a `[workflows]` entry to the home config file (#232), creating the file /
/// directory if needed. Inserts only the one line, preserving everything else in
/// the file (other sections, comments, formatting) — so it's safe to write a
/// config the author hand-maintains. Returns the path written.
pub fn add_workflow(name: &str, steps: &[String]) -> io::Result<PathBuf> {
    let path = config_path().ok_or_else(|| io::Error::other("no config directory"))?;
    let existing = read_existing_config(&path)?;
    let updated = upsert_workflow(&existing, name, steps);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, updated)?;
    Ok(path)
}

/// Splice a `name = ["step", …]` workflow entry into config text under
/// `[workflows]`, replacing an entry of the same name in place rather than
/// duplicating it. The section-finding and newline handling is the shared
/// `set_value` splicer (#436); only the workflow-specific key/array encoding
/// lives here, via the `toml` serializer so it matches what the loader parses.
fn upsert_workflow(existing: &str, name: &str, steps: &[String]) -> String {
    let array: Vec<toml::Value> = steps.iter().cloned().map(toml::Value::String).collect();
    let value = toml::Value::Array(array).to_string();
    set_value(existing, "workflows", &toml_key(name), &value)
}

/// The TOML-encoded form of a key — bare for an identifier-like name, otherwise
/// a quoted, escaped basic string — matching how the `toml` serializer would
/// write it, so a spliced workflow name round-trips through the loader.
fn toml_key(name: &str) -> String {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        name.to_string()
    } else {
        toml::Value::String(name.to_string()).to_string()
    }
}

/// Write a batch of `key = value` settings into the config file (#303), each
/// under its `[section]`, preserving everything else in the file (other keys,
/// sections, and comments). `value` is a ready-to-write TOML scalar literal
/// (e.g. `true`, `64`, `"#1e1e24"`). Creates the file/section if absent.
pub fn set_values(changes: &[(String, String, String)]) -> io::Result<PathBuf> {
    let path = config_path().ok_or_else(|| io::Error::other("no config directory"))?;
    let mut text = read_existing_config(&path)?;
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
///
/// The line is spliced verbatim, so both sides must already be TOML-encoded:
/// `value` a ready-to-write literal (e.g. `true`, `"#1e1e24"`), and `key` either
/// a constant bare key or, for a user-supplied name, run through [`toml_key`].
///
/// Pure (no I/O), so the same splice serves both the home config file and a
/// project's `book.toml` (#550): the manifest's `[goals]` table is updated in
/// place this way, leaving the rest of the manifest byte-for-byte.
pub(crate) fn set_value(existing: &str, section: &str, key: &str, value: &str) -> String {
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
            lines[i]
                .split_once('=')
                .is_some_and(|(lhs, _)| lhs.trim() == key)
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

/// Remove `key` from `[section]` in the config file (#530), preserving everything
/// else (other keys, sections, comments). Used to strip a now-redundant plaintext
/// `api_key` once the key is stored in the OS keychain. Returns whether a line was
/// removed (so the caller can report "nothing to remove" rather than rewriting an
/// unchanged file). A missing file / section / key is a no-op.
pub fn remove_value(section: &str, key: &str) -> io::Result<bool> {
    let Some(path) = config_path() else {
        return Ok(false);
    };
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let (updated, removed) = strip_value(&existing, section, key);
    if removed {
        std::fs::write(&path, updated)?;
    }
    Ok(removed)
}

/// Drop the `key = …` line under `[section]` from `existing`, returning the new
/// text and whether a line was removed. Pure (no I/O), so the splice is testable
/// without a file on disk, like [`set_value`]. Everything else is left
/// byte-for-byte; the section header stays even if it ends up empty.
///
/// Pure (no I/O), so it also serves project `book.toml` edits (#550): clearing an
/// optional goal (e.g. the daily target) removes its key from `[goals]` rather
/// than writing a zero.
pub(crate) fn strip_value(existing: &str, section: &str, key: &str) -> (String, bool) {
    let header = format!("[{section}]");
    let mut lines: Vec<String> = existing.lines().map(String::from).collect();
    let trailing_nl = existing.is_empty() || existing.ends_with('\n');

    let Some(h) = lines.iter().position(|l| l.trim() == header) else {
        return (existing.to_string(), false);
    };
    // The section body runs to the next `[…]` header (or end of file).
    let end = lines[h + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |i| h + 1 + i);
    let found = (h + 1..end).find(|&i| {
        lines[i]
            .split_once('=')
            .is_some_and(|(lhs, _)| lhs.trim() == key)
    });
    let Some(k) = found else {
        return (existing.to_string(), false);
    };
    lines.remove(k);

    let mut out = lines.join("\n");
    if trailing_nl && !out.is_empty() {
        out.push('\n');
    }
    (out, true)
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

/// The configured `[theme] name`, or `None` when unset (the default `dark` shows
/// through). Read by the plain Settings menu (#592) to mark the active theme in
/// its picker without re-parsing the whole `[theme]` table for its colors.
pub(crate) fn theme_name() -> Option<String> {
    let table: HashMap<String, String> = load_section("theme");
    table.get("name").cloned()
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
            Some(text) => snippets.push(Snippet {
                name,
                text: text.to_string(),
            }),
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
    /// Whether a bare `quill` launch (no path, not inside a project) reopens the
    /// most-recently-opened project directly instead of showing the create/open
    /// chooser (#613). Off by default (absent) — the chooser still appears. Set
    /// `open_last = true` to skip straight into the last project.
    pub open_last: Option<bool>,
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

    /// Whether a bare launch should reopen the most-recent project directly (#613);
    /// off by default (opt in with `open_last = true`).
    pub fn open_last_enabled(&self) -> bool {
        self.open_last.unwrap_or(false)
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
                Snippet {
                    name: "alpha".into(),
                    text: "* * *".into()
                },
                Snippet {
                    name: "zeta".into(),
                    text: "the end".into()
                },
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
        assert!(
            out.contains("smart_typography = true"),
            "other keys kept: {out}"
        );
    }

    #[test]
    fn set_value_inserts_under_an_existing_section() {
        let existing = "[editor]\nvim = true\n";
        let out = set_value(existing, "editor", "column_width", "64");
        assert!(out.contains("[editor]"), "{out}");
        assert!(
            out.contains("vim = true") && out.contains("column_width = 64"),
            "{out}"
        );
    }

    #[test]
    fn set_value_appends_a_new_section_preserving_the_rest() {
        let existing = "[editor]\nvim = true\n";
        let out = set_value(existing, "theme", "name", "\"light\"");
        assert!(
            out.contains("[editor]\nvim = true"),
            "existing section kept: {out}"
        );
        assert!(
            out.contains("[theme]") && out.contains("name = \"light\""),
            "{out}"
        );
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
        assert!(
            out.contains("# my settings") && out.contains("# turn on vim"),
            "comments kept: {out}"
        );
        assert!(out.contains("vim = true"), "{out}");
    }

    #[test]
    fn strip_value_removes_the_key_and_keeps_the_rest() {
        let existing = "[ai]\nenabled = true\napi_key = \"sk-secret\"\nmodel = \"x\"\n";
        let (out, removed) = strip_value(existing, "ai", "api_key");
        assert!(removed);
        assert!(!out.contains("sk-secret"), "plaintext key gone: {out}");
        assert!(
            out.contains("enabled = true") && out.contains("model = \"x\""),
            "other keys kept: {out}"
        );
        assert!(out.ends_with('\n'), "trailing newline preserved: {out:?}");
    }

    #[test]
    fn strip_value_is_a_no_op_when_absent() {
        let existing = "[ai]\nenabled = true\n";
        let (out, removed) = strip_value(existing, "ai", "api_key");
        assert!(!removed);
        assert_eq!(out, existing, "unchanged when the key isn't there");
        // A missing section is likewise a no-op.
        let (out, removed) = strip_value(existing, "image", "api_key");
        assert!(!removed);
        assert_eq!(out, existing);
    }

    #[test]
    fn strip_value_only_touches_the_named_section() {
        let existing = "[ai]\napi_key = \"a\"\n\n[image]\napi_key = \"b\"\n";
        let (out, removed) = strip_value(existing, "image", "api_key");
        assert!(removed);
        assert!(
            out.contains("[ai]\napi_key = \"a\""),
            "[ai] key kept: {out}"
        );
        assert!(!out.contains("\"b\""), "[image] key removed: {out}");
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
        let cfg = ProjectsConfig {
            home: Some("/books".into()),
            ..Default::default()
        };
        assert_eq!(cfg.home_dir(), Some(PathBuf::from("/books")));
    }

    #[test]
    fn the_setup_wizard_is_on_by_default_and_opt_out() {
        assert!(ProjectsConfig::default().wizard_enabled(), "on by default");
        let off: ProjectsConfig = toml::from_str("setup_wizard = false").unwrap();
        assert!(!off.wizard_enabled(), "opt out with setup_wizard = false");
    }

    #[test]
    fn open_last_is_off_by_default_and_opt_in() {
        assert!(
            !ProjectsConfig::default().open_last_enabled(),
            "off by default"
        );
        let on: ProjectsConfig = toml::from_str("open_last = true").unwrap();
        assert!(on.open_last_enabled(), "opt in with open_last = true");
    }

    #[test]
    fn no_projects_home_is_none() {
        assert_eq!(ProjectsConfig::default().home_dir(), None);
    }

    #[test]
    fn inserts_under_an_existing_workflows_section() {
        let existing = "[editor]\nvim = true\n\n[workflows]\nfinish = [\"save\"]\n";
        let out = upsert_workflow(existing, "revise", &steps(&["save", "ai:continuity"]));
        // The new entry is under the header, and the existing content is intact.
        assert!(
            out.contains("[workflows]\nrevise = [\"save\", \"ai:continuity\"]\n"),
            "{out}"
        );
        assert!(out.contains("finish = [\"save\"]"));
        assert!(out.contains("[editor]\nvim = true"));
    }

    #[test]
    fn appends_a_new_section_when_none_exists() {
        let existing = "[editor]\nvim = true\n";
        let out = upsert_workflow(existing, "revise", &steps(&["save"]));
        assert!(out.contains("[editor]\nvim = true"), "existing kept: {out}");
        assert!(
            out.ends_with("[workflows]\nrevise = [\"save\"]\n"),
            "appended: {out}"
        );
    }

    #[test]
    fn writes_a_fresh_file_when_empty() {
        let out = upsert_workflow("", "x", &steps(&["save"]));
        assert_eq!(out, "[workflows]\nx = [\"save\"]\n");
    }

    #[test]
    fn a_name_needing_quotes_is_quoted() {
        let out = upsert_workflow("", "my pass", &steps(&["save"]));
        assert!(out.contains("\"my pass\" = [\"save\"]"), "{out}");
    }

    #[test]
    fn re_adding_a_workflow_replaces_it_in_place() {
        let existing = "[workflows]\nrevise = [\"save\"]\n";
        let out = upsert_workflow(existing, "revise", &steps(&["save", "ai:continuity"]));
        assert!(
            out.contains("revise = [\"save\", \"ai:continuity\"]"),
            "replaced: {out}"
        );
        assert_eq!(
            out.matches("revise =").count(),
            1,
            "no duplicate entry: {out}"
        );
    }
}

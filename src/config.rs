//! User configuration shared across features, loaded from the user's home
//! config file (`~/.config/quill/config.toml`) — never the project directory, so
//! an unpublished draft and the user's settings stay separate. Each feature owns
//! its own section (`[ai]`, `[editor]`, …); this module owns the file location
//! and the `[editor]` section.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde::Deserialize;

/// `$XDG_CONFIG_HOME/quill/config.toml`, else `$HOME/.config/quill/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("quill").join("config.toml"))
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
    let values: Vec<String> = steps.iter().map(|s| toml_string(s)).collect();
    let entry = format!("{} = [{}]", toml_key(name), values.join(", "));

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

/// A TOML bare key when it's a simple identifier, else a quoted key.
fn toml_key(name: &str) -> String {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        name.to_string()
    } else {
        toml_string(name)
    }
}

/// A double-quoted TOML basic string, escaping the two characters that matter for
/// the tokens/names we write (quote and backslash).
fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Editor behavior from the `[editor]` section. The default keeps Quill's
/// modeless editing unchanged.
#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct EditorConfig {
    /// Opt-in Vim-style modal editing (#27). Off by default — with it off the
    /// editor behaves exactly as it always has.
    pub vim: bool,
    /// Vim key remaps (#208) from the `[editor.keys]` table: action name → the
    /// key that should trigger it (e.g. `down = "n"`). Validated when the editor
    /// builds its keymap; unknown actions are reported, not silently dropped.
    pub keys: HashMap<String, String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_string()).collect()
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

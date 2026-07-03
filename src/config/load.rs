//! Section loading with the per-project override layered over the home config
//! (#710). Each typed loader ([`EditorConfig::load`], [`load_theme`], …) routes
//! through [`load_section`]: read the global `[section]`, read the project's
//! `[quill.section]` override, merge them field by field, then deserialize. The
//! merge is untyped ([`toml::Value`]) so the two layers overlay key by key before
//! either becomes a concrete type.
//!
//! [`EditorConfig`]: super::EditorConfig
//! [`load_theme`]: super::load_theme

use serde::de::DeserializeOwned;

use super::{config_path, project_manifest_path};

/// Read one `[section]` table, deserialized into `T` (#430), layering the open
/// project's override over the global config (#710). The global home config is
/// the base; if the project registered via [`set_project_dir`] declares this
/// section under `[quill]` in its `book.toml` (e.g. `[quill.editor]`), its keys
/// are merged over the global section field by field (project wins, global fills
/// the gaps). With no project override the result is exactly the global section,
/// unchanged.
///
/// Best-effort throughout: a missing/unreadable/malformed file, or an absent or
/// malformed section, contributes nothing (rather than failing), so an absent
/// section on both layers yields `T::default()`. Collapses the per-section
/// wrapper-struct idiom (`struct File { <section>: T }`) every loader used to
/// re-derive into one call. The config files are small, so re-reading them per
/// section at startup is negligible.
///
/// [`set_project_dir`]: super::set_project_dir
pub fn load_section<T: DeserializeOwned + Default>(section: &str) -> T {
    let global = read_section(config_path().as_deref(), section);
    let project = read_project_section(section);
    merge_sections(global, project)
        .map_or_else(T::default, |value| value.try_into().unwrap_or_default())
}

/// The `[section]` table of the TOML file at `path`, as a raw [`toml::Value`], or
/// `None` when the path is absent, unreadable, unparseable, or lacks the section.
/// Untyped so [`merge_sections`] can overlay two layers key by key before either
/// is deserialized into the section's concrete type.
fn read_section(path: Option<&std::path::Path>, section: &str) -> Option<toml::Value> {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .and_then(|mut table| table.remove(section))
}

/// The project's override for `section`: the `[quill.<section>]` table in the open
/// project's `book.toml` (#710), or `None` when no project is registered or the
/// manifest has no such override. Best-effort like [`read_section`].
fn read_project_section(section: &str) -> Option<toml::Value> {
    project_manifest_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .and_then(|mut table| table.remove("quill"))
        .and_then(|quill| match quill {
            toml::Value::Table(mut t) => t.remove(section),
            _ => None,
        })
}

/// Overlay a project section over the global section, key by key (#710): a key
/// present in `project` wins; a key only `global` has is kept; nested tables
/// (e.g. `[editor.keys]`) merge recursively so a project can override one nested
/// key without dropping the rest. When either side is absent the other is used
/// as-is; when both are, and both are tables, they merge — otherwise the project
/// value replaces the global one wholesale.
fn merge_sections(
    global: Option<toml::Value>,
    project: Option<toml::Value>,
) -> Option<toml::Value> {
    match (global, project) {
        (Some(toml::Value::Table(mut base)), Some(toml::Value::Table(over))) => {
            for (key, value) in over {
                let merged = merge_sections(base.remove(&key), Some(value));
                if let Some(v) = merged {
                    base.insert(key, v);
                }
            }
            Some(toml::Value::Table(base))
        }
        (base, None) => base,
        (_, over) => over,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::{set_config_dir, set_project_dir, EditorConfig};
    use super::*;

    /// Point the global config at a scratch dir and register a scratch project,
    /// both hermetic (#528/#531). `global` is the home `config.toml`; `project`, if
    /// given, is the `book.toml` manifest text (its overrides live under `[quill]`).
    /// Returns a guard that clears both overrides and removes the dirs on drop, so
    /// the layering tests stay isolated even on an assertion panic.
    struct LayerFixture {
        cfg_dir: PathBuf,
        project_dir: PathBuf,
    }

    impl LayerFixture {
        fn new(name: &str, global: &str, project: Option<&str>) -> Self {
            let cfg_dir = crate::testutil::scratch_dir("config", &format!("{name}-cfg"));
            let project_dir = crate::testutil::scratch_dir("config", &format!("{name}-proj"));
            std::fs::create_dir_all(&cfg_dir).unwrap();
            std::fs::create_dir_all(&project_dir).unwrap();
            std::fs::write(cfg_dir.join("config.toml"), global).unwrap();
            set_config_dir(Some(cfg_dir.clone()));
            if let Some(manifest) = project {
                std::fs::write(project_dir.join("book.toml"), manifest).unwrap();
            }
            set_project_dir(Some(project_dir.clone()));
            Self {
                cfg_dir,
                project_dir,
            }
        }
    }

    impl Drop for LayerFixture {
        fn drop(&mut self) {
            set_project_dir(None);
            set_config_dir(None);
            std::fs::remove_dir_all(&self.cfg_dir).ok();
            std::fs::remove_dir_all(&self.project_dir).ok();
        }
    }

    #[test]
    fn a_project_override_wins_over_the_global_setting() {
        // Global says width 72; this project's book.toml overrides it to 100 (#710).
        let _fx = LayerFixture::new(
            "override-wins",
            "[editor]\ncolumn_width = 72\n",
            Some("title = \"Book\"\n\n[quill.editor]\ncolumn_width = 100\n"),
        );
        assert_eq!(EditorConfig::load().column_width, 100, "project value wins");
    }

    #[test]
    fn an_unset_project_key_falls_back_to_the_global_value() {
        // The project overrides only `vim`; `column_width` falls back to global,
        // and a key neither layer sets falls back to the built-in default (#710).
        let _fx = LayerFixture::new(
            "field-fallback",
            "[editor]\ncolumn_width = 80\n",
            Some("title = \"Book\"\n\n[quill.editor]\nvim = true\n"),
        );
        let editor = EditorConfig::load();
        assert!(editor.vim, "project's key applies");
        assert_eq!(editor.column_width, 80, "global fills the unset key");
        assert!(
            editor.smart_typography,
            "a key neither layer sets keeps its default"
        );
    }

    #[test]
    fn with_no_project_override_the_global_value_is_used() {
        // A registered project whose book.toml has no `[quill]` table behaves
        // exactly as the global-only loader always did (#710).
        let _fx = LayerFixture::new(
            "no-override",
            "[editor]\ncolumn_width = 88\n",
            Some("title = \"Book\"\n"),
        );
        assert_eq!(
            EditorConfig::load().column_width,
            88,
            "global value used when the project has no override"
        );
    }

    #[test]
    fn a_registered_project_without_a_manifest_uses_the_global_value() {
        // No book.toml at all (e.g. before one is written): still global-only (#710).
        let _fx = LayerFixture::new("no-manifest", "[editor]\ncolumn_width = 66\n", None);
        assert_eq!(EditorConfig::load().column_width, 66);
    }

    #[test]
    fn a_project_override_leaves_the_global_config_unchanged() {
        // The override is a read-only overlay: the global file on disk isn't
        // rewritten when a project layers its own value on top (#710).
        let fx = LayerFixture::new(
            "global-untouched",
            "[editor]\ncolumn_width = 72\n",
            Some("title = \"Book\"\n\n[quill.editor]\ncolumn_width = 100\n"),
        );
        assert_eq!(EditorConfig::load().column_width, 100);
        let global = std::fs::read_to_string(fx.cfg_dir.join("config.toml")).unwrap();
        assert_eq!(
            global, "[editor]\ncolumn_width = 72\n",
            "the global config on disk is untouched"
        );
    }

    #[test]
    fn merge_overlays_nested_tables_key_by_key() {
        // A project can override one key of a nested table (e.g. [editor.keys])
        // without dropping the sibling keys the global sets (#710). Both sides are
        // the `[editor]` section value, each carrying a nested `keys` table.
        let global = toml::from_str::<toml::Table>("[editor.keys]\ndown = \"n\"\nup = \"e\"\n")
            .unwrap()
            .remove("editor");
        let project = toml::from_str::<toml::Table>("[editor.keys]\ndown = \"j\"\n")
            .unwrap()
            .remove("editor");
        let merged = merge_sections(global, project).unwrap();
        let keys = merged.get("keys").unwrap().as_table().unwrap();
        assert_eq!(keys["down"].as_str(), Some("j"), "project key wins");
        assert_eq!(keys["up"].as_str(), Some("e"), "global sibling kept");
    }

    #[test]
    fn merge_uses_whichever_layer_is_present_when_only_one_is() {
        let only_global = toml::Value::Integer(1);
        assert_eq!(
            merge_sections(Some(only_global.clone()), None),
            Some(only_global)
        );
        let only_project = toml::Value::Integer(2);
        assert_eq!(
            merge_sections(None, Some(only_project.clone())),
            Some(only_project)
        );
        assert_eq!(merge_sections(None, None), None);
    }
}

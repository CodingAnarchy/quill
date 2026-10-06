//! Section loading with the per-project override layered over the home config
//! (#710). Each typed loader ([`EditorConfig::load`], [`load_theme`], …) routes
//! through [`load_section`]: read the global `[section]`, read the project's
//! `[quill.section]` override, merge them field by field, then deserialize. The
//! merge is untyped ([`toml::Value`]) so the two layers overlay key by key before
//! either becomes a concrete type.
//!
//!
//! A batch of loads — startup, a project switch, a live config reload — runs
//! inside a [`LoadSession`], which parses each file at most once for the batch
//! and serves every section from that parse. Outside a session every load reads
//! the files fresh, exactly as before.
//!
//! [`EditorConfig`]: super::EditorConfig
//! [`load_theme`]: super::load_theme

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use super::{config_path, project_manifest_path};

/// A [`LoadSession`]'s parsed files, keyed by (is-project-manifest, path): the
/// global config's full table, or the manifest's narrowed `[quill]` table.
type ParsedFiles = HashMap<(bool, PathBuf), Option<toml::Table>>;

thread_local! {
    /// The parsed files of the active [`LoadSession`] on this thread, keyed by
    /// (is-project-manifest, path), or `None` with no session open. Per-thread
    /// like the config/project dir overrides, so the parallel test suite stays
    /// hermetic.
    static SESSION: RefCell<Option<ParsedFiles>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// How many config files this thread has read and parsed (test-only), so a
    /// test can prove a session parses each file once.
    static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Parse each config file at most once across a batch of section loads.
///
/// Startup loads a dozen sections (editor, theme, workflows, triggers, snippets,
/// AI, image, backup, general, …), and each used to read and TOML-parse both the
/// global `config.toml` and the project's `book.toml` — which for a series-sized
/// project is ~100KB+ of chapter list — to pull out one table. Inside a session
/// the first load of each file parses it and every later load reuses that parse.
///
/// The cache lives only as long as the guard, so it can't serve a stale file to a
/// later batch: open one around a burst of loads that does no writing (App
/// construction, a live config reload), never across a write. As a backstop,
/// Quill's own config writers and the manifest save call [`forget_parsed`], so
/// even a write made mid-session is seen by the next load. Sessions nest: only
/// the outermost guard clears the cache when it drops.
pub struct LoadSession {
    outermost: bool,
}

impl LoadSession {
    /// Open a load session on this thread (or join the one already open).
    #[must_use = "the session ends when the guard drops"]
    pub fn begin() -> Self {
        let outermost = SESSION.with(|s| {
            let mut s = s.borrow_mut();
            let fresh = s.is_none();
            if fresh {
                *s = Some(HashMap::new());
            }
            fresh
        });
        Self { outermost }
    }
}

impl Drop for LoadSession {
    fn drop(&mut self) {
        if self.outermost {
            SESSION.with(|s| *s.borrow_mut() = None);
        }
    }
}

/// Drop every parse the open [`LoadSession`] holds (a no-op with none open), so
/// the next load re-reads the files. Called after any write to `config.toml` or
/// `book.toml` that Quill makes, so a session can never serve a file as it was
/// before the write.
pub fn forget_parsed() {
    SESSION.with(|s| {
        if let Some(cache) = s.borrow_mut().as_mut() {
            cache.clear();
        }
    });
}

/// The parsed table of the TOML file at `path`, narrowed by `extract`, or `None`
/// when the file is absent, unreadable, unparseable, or `extract` finds nothing.
/// Within a [`LoadSession`] the result is computed once per file and cloned from
/// the cache after; `project` distinguishes the manifest's narrowed `[quill]`
/// table from a full config table at the same path.
fn parsed(
    path: &Path,
    project: bool,
    extract: fn(toml::Table) -> Option<toml::Table>,
) -> Option<toml::Table> {
    let load = || {
        #[cfg(test)]
        PARSES.with(|n| n.set(n.get() + 1));
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
            .and_then(extract)
    };
    SESSION.with(|s| {
        s.borrow_mut().as_mut().map_or_else(load, |cache| {
            cache
                .entry((project, path.to_path_buf()))
                .or_insert_with(load)
                .clone()
        })
    })
}

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
/// re-derive into one call. Each call reads both files afresh unless a
/// [`LoadSession`] is open, which parses each once for the whole batch.
///
/// [`set_project_dir`]: super::set_project_dir
pub fn load_section<T: DeserializeOwned + Default>(section: &str) -> T {
    let global = read_section(config_path().as_deref(), section);
    let project = read_project_section(section);
    merge_sections(global, project)
        .map_or_else(T::default, |value| value.try_into().unwrap_or_default())
}

/// A warning if the global `config.toml` is present but malformed (#1100), naming
/// the file and the TOML parse error so the author can fix it — rather than the
/// per-section loaders silently swallowing the failure and falling back to
/// defaults with no message (as [`read_section`]'s `.ok()` does). `None` when the
/// file is absent (a fresh install has none) or parses cleanly; the defaults still
/// apply either way, so the editor stays usable. The project manifest already
/// surfaces its own parse error by name; this brings the global config to parity.
///
/// Read once at startup by the caller, which threads the warning into the same
/// non-fatal startup notice as the theme / snippet / keymap warnings.
pub fn global_config_warning() -> Option<String> {
    let path = config_path()?;
    // Only a genuine parse failure of a file that *is* there warrants a warning; a
    // missing or unreadable file is the ordinary defaults-only case the loaders
    // already handle silently.
    let text = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<toml::Table>(&text) {
        Ok(_) => None,
        // The full `Display` (like the manifest's `invalid <file>: {e}`) carries the
        // line/column span, so the author can find the offending line — richer than
        // the bare `.message()`. Framed as "using defaults" so it reads as a
        // non-fatal notice, not the manifest's hard open failure.
        Err(err) => Some(format!(
            "Malformed config, using defaults: {} — {err}",
            path.display()
        )),
    }
}

/// The `[section]` table of the TOML file at `path`, as a raw [`toml::Value`], or
/// `None` when the path is absent, unreadable, unparseable, or lacks the section.
/// Untyped so [`merge_sections`] can overlay two layers key by key before either
/// is deserialized into the section's concrete type. A malformed file is swallowed
/// here (defaults still load); [`global_config_warning`] surfaces that case to the
/// author so the silent reset (#1100) at least announces itself.
fn read_section(path: Option<&Path>, section: &str) -> Option<toml::Value> {
    parsed(path?, false, Some)?.remove(section)
}

/// The project's override for `section`: the `[quill.<section>]` table in the open
/// project's `book.toml` (#710), or `None` when no project is registered or the
/// manifest has no such override. Best-effort like [`read_section`].
fn read_project_section(section: &str) -> Option<toml::Value> {
    // Keep only the `[quill]` table: the rest of the manifest (the chapter list)
    // is never needed here, so a session caches and clones just the overrides.
    let quill = |mut table: toml::Table| match table.remove("quill") {
        Some(toml::Value::Table(t)) => Some(t),
        _ => None,
    };
    parsed(&project_manifest_path()?, true, quill)?.remove(section)
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

    use super::super::{
        load_snippets, load_theme, set_config_dir, set_project_dir, set_values, EditorConfig,
    };
    use super::*;
    use crate::project::Project;

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
        assert!(editor.flags.vim, "project's key applies");
        assert_eq!(editor.column_width, 80, "global fills the unset key");
        assert!(
            editor.flags.smart_typography,
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
    fn a_malformed_global_config_warns_and_still_loads_defaults() {
        // A broken global config.toml used to be swallowed silently, section by
        // section, with defaults quietly substituted (#1100). Now it surfaces a
        // warning naming the file and the parse error — while the defaults still
        // apply, so the editor stays usable.
        let _fx = LayerFixture::new(
            "malformed-global",
            "[editor]\ncolumn_width = ", // truncated value: not valid TOML
            None,
        );
        let warning = global_config_warning().expect("a malformed config warns");
        assert!(
            warning.contains("config.toml"),
            "the warning names the file: {warning}"
        );
        assert!(
            warning.to_lowercase().contains("malformed")
                || warning.to_lowercase().contains("default"),
            "the warning explains defaults are used: {warning}"
        );
        // Defaults still load: the built-in column width, not a panic or reset error.
        assert_eq!(
            EditorConfig::load().column_width,
            EditorConfig::default().column_width,
            "defaults still apply so the editor stays usable"
        );
    }

    #[test]
    fn a_well_formed_global_config_does_not_warn() {
        // The happy path stays quiet: a valid config yields no warning (and no false
        // positive on a fresh install, whose config is simply absent).
        let _fx = LayerFixture::new("valid-global", "[editor]\ncolumn_width = 72\n", None);
        assert_eq!(global_config_warning(), None, "a valid config is silent");
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

    /// How many config files this thread has parsed so far.
    fn parses() -> usize {
        PARSES.with(std::cell::Cell::get)
    }

    #[test]
    fn a_load_session_parses_each_file_once() {
        // Startup loads many sections; inside a session the global config and the
        // project manifest are each read and parsed once, not once per section.
        let _fx = LayerFixture::new(
            "session-once",
            "[editor]\ncolumn_width = 72\n[theme]\nname = \"light\"\n",
            Some("title = \"Book\"\n\n[quill.editor]\ncolumn_width = 100\n"),
        );
        let before = parses();
        {
            let _session = LoadSession::begin();
            assert_eq!(EditorConfig::load().column_width, 100);
            let _ = load_theme();
            let _ = load_snippets();
            let _nested = LoadSession::begin(); // joins the open session
            let _ = EditorConfig::load();
        }
        assert_eq!(parses() - before, 2, "one parse per file for the batch");

        // Outside a session every load reads afresh, as before.
        let before = parses();
        let _ = EditorConfig::load();
        let _ = EditorConfig::load();
        assert_eq!(parses() - before, 4);
    }

    #[test]
    fn a_settings_write_is_seen_by_the_next_load() {
        // The in-app settings writer splices config.toml; a load after it — even
        // one inside the same session — sees the new value, never the old parse.
        let _fx = LayerFixture::new("session-write", "[editor]\ncolumn_width = 72\n", None);
        let _session = LoadSession::begin();
        assert_eq!(EditorConfig::load().column_width, 72);
        set_values(&[(
            "editor".to_string(),
            "column_width".to_string(),
            "90".to_string(),
        )])
        .unwrap();
        assert_eq!(EditorConfig::load().column_width, 90, "the write is seen");
    }

    #[test]
    fn an_outside_edit_is_seen_by_the_next_session() {
        // An external edit (or any write that doesn't go through Quill) is seen by
        // the next batch of loads: a session's cache dies with its guard.
        let fx = LayerFixture::new("session-external", "[editor]\ncolumn_width = 72\n", None);
        {
            let _session = LoadSession::begin();
            assert_eq!(EditorConfig::load().column_width, 72);
        }
        std::fs::write(
            fx.cfg_dir.join("config.toml"),
            "[editor]\ncolumn_width = 66\n",
        )
        .unwrap();
        let _session = LoadSession::begin();
        assert_eq!(EditorConfig::load().column_width, 66);
    }

    #[test]
    fn a_manifest_save_drops_the_sessions_parse() {
        // `book.toml` carries the `[quill.*]` overrides; saving the manifest drops
        // the session's parse so the next load re-reads the saved file.
        let fx = LayerFixture::new(
            "session-manifest",
            "",
            Some("title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n"),
        );
        let project = Project::open(&fx.project_dir).unwrap();
        let _session = LoadSession::begin();
        let _ = EditorConfig::load();
        let before = parses();
        let _ = EditorConfig::load();
        assert_eq!(parses(), before, "served from the session");
        project.save_manifest().unwrap();
        let _ = EditorConfig::load();
        assert_eq!(parses() - before, 2, "both files re-read after the save");
    }
}

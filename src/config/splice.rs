//! In-place TOML writers for the config files. Each writer reads the existing
//! file, splices the one change in (preserving every other line — keys, sections,
//! comments, formatting), and rewrites it through the atomic layer (#644). The
//! splices ([`set_value`], [`strip_value`]) are pure, so they serve both the home
//! `config.toml` and a project's `book.toml` (#550) and stay unit-testable without
//! a file on disk.

use std::io;

use crate::atomic;

use super::config_path;

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
pub fn add_workflow(name: &str, steps: &[String]) -> io::Result<std::path::PathBuf> {
    let path = config_path().ok_or_else(|| io::Error::other("no config directory"))?;
    let existing = read_existing_config(&path)?;
    let updated = upsert_workflow(&existing, name, steps);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic::write(&path, updated.as_bytes())?;
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
pub fn set_values(changes: &[(String, String, String)]) -> io::Result<std::path::PathBuf> {
    let path = config_path().ok_or_else(|| io::Error::other("no config directory"))?;
    let mut text = read_existing_config(&path)?;
    for (section, key, value) in changes {
        text = set_value(&text, section, key, value);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    atomic::write(&path, text.as_bytes())?;
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
///
/// The line splice only recognises a section written as a literal `[header]`
/// line. A hand-maintained config may instead write the same section as an
/// inline table (`editor = { vim = true }`) or with dotted keys (`keys.down =
/// "j"` under `[editor]`); the splice can't see those, so it would append a
/// duplicate `[section]` table and corrupt the file into invalid TOML — which the
/// loader's best-effort parse then swallows, silently resetting every setting
/// (#1140). So the spliced output is parse-validated before it's returned; if it
/// no longer parses (the duplicate-table case), we fall back to
/// [`rewrite_value`], which re-emits the whole document through the `toml`
/// serializer with the one change applied. The fallback loses comment/formatting
/// fidelity for that write, but it keeps every other value and stays valid TOML —
/// the invariant that matters — rather than writing a document that reloads to
/// defaults.
pub fn set_value(existing: &str, section: &str, key: &str, value: &str) -> String {
    let spliced = splice_line(existing, section, key, value);
    // The common case: the splice produced valid TOML (a `[header]` section, or a
    // fresh file). Return it verbatim so comments and formatting are preserved.
    if toml::from_str::<toml::Table>(&spliced).is_ok() {
        return spliced;
    }
    // The splice corrupted the file (an inline-table or dotted-key section form it
    // couldn't match, now duplicated). Rewrite structurally instead, but only if
    // the *original* was itself valid TOML — otherwise there's nothing better to
    // fall back to and the spliced text is as good as any.
    rewrite_value(existing, section, key, value).unwrap_or(spliced)
}

/// The original line splice: set `key = value` under a literal `[section]` header,
/// replacing the key in place, inserting under the header, or appending a new
/// section. Preserves every other line byte-for-byte. Split out from [`set_value`]
/// so the fast path stays a pure text edit while [`set_value`] validates its
/// result and falls back when the section is written in a form this can't see.
fn splice_line(existing: &str, section: &str, key: &str, value: &str) -> String {
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

/// The structured fallback for [`set_value`]: parse `existing` as a whole TOML
/// document, set `key = value` in the `[section]` table (creating the nested path
/// — `section` may be dotted, e.g. `editor.keys`), and re-serialize. Returns
/// `None` if the original doesn't parse or the value literal can't be decoded, so
/// [`set_value`] keeps its spliced text in that case.
///
/// This is what keeps an in-app change from clobbering a config whose sections are
/// written as inline tables or dotted keys (#1140): the round-trip normalises
/// those forms to standard `[header]` tables while preserving every value, so the
/// result is valid TOML that reloads with the change applied and nothing lost.
fn rewrite_value(existing: &str, section: &str, key: &str, value: &str) -> Option<String> {
    let mut root: toml::Table = toml::from_str(existing).ok()?;
    // Decode the pre-encoded scalar/array literal back into a value by parsing it
    // as a one-key mini-document, so whatever `set_value`'s callers encoded (a
    // bool, an int, a quoted string, an array) round-trips unchanged.
    let decoded = toml::from_str::<toml::Table>(&format!("v = {value}"))
        .ok()?
        .remove("v")?;

    // Walk (and create) the dotted section path, e.g. `editor.keys` → the `keys`
    // table nested in `editor`. A path segment that exists but isn't a table would
    // make the insert ambiguous, so bail to the spliced text instead.
    let mut table = &mut root;
    for segment in section.split('.') {
        let next = table
            .entry(segment.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        table = match next {
            toml::Value::Table(t) => t,
            _ => return None,
        };
    }
    table.insert(key.to_string(), decoded);

    let mut out = toml::to_string(&root).ok()?;
    // Match the newline convention the splice uses: a trailing newline unless the
    // original had none and wasn't empty. `toml::to_string` already ends in one.
    if !(existing.is_empty() || existing.ends_with('\n')) {
        while out.ends_with('\n') {
            out.pop();
        }
    }
    Some(out)
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
        atomic::write(&path, updated.as_bytes())?;
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
pub fn strip_value(existing: &str, section: &str, key: &str) -> (String, bool) {
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

#[cfg(test)]
mod tests {
    use super::super::{config_path, set_config_dir};
    use super::*;

    fn steps(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_string()).collect()
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

    /// Parse `text` and read the (possibly dotted) `section.key` back as a raw
    /// value, asserting the whole document is valid TOML along the way. The core
    /// #1140 invariant: after a splice the file still parses and the change is
    /// there. Returns `None` if the key is absent (but a parse failure panics).
    fn reload(text: &str, section: &str, key: &str) -> Option<toml::Value> {
        let mut table: toml::Table = toml::from_str(text)
            .unwrap_or_else(|e| panic!("result must be valid TOML: {e}\n{text}"));
        for segment in section.split('.') {
            table = match table.remove(segment) {
                Some(toml::Value::Table(t)) => t,
                _ => return None,
            };
        }
        table.remove(key)
    }

    #[test]
    fn set_value_edits_an_inline_table_section_without_corrupting_the_file() {
        // A hand-maintained config may write a whole section as an inline table.
        // The line splice can't see it and used to append a duplicate `[editor]`
        // table → invalid TOML → every setting silently reset (#1140). The result
        // must stay valid TOML, apply the change, and keep the sibling value.
        let existing = "editor = { vim = true, column_width = 96 }\n[theme]\nname = \"dark\"\n";
        let out = set_value(existing, "editor", "vim", "false");
        assert_eq!(
            reload(&out, "editor", "vim"),
            Some(toml::Value::Boolean(false)),
            "the change is applied and reloads: {out}"
        );
        assert_eq!(
            reload(&out, "editor", "column_width"),
            Some(toml::Value::Integer(96)),
            "the sibling inline-table value is preserved: {out}"
        );
        assert_eq!(
            reload(&out, "theme", "name"),
            Some(toml::Value::String("dark".into())),
            "an unrelated section is preserved: {out}"
        );
        assert_eq!(
            out.matches("column_width").count(),
            1,
            "no duplicate editor table: {out}"
        );
    }

    #[test]
    fn key_remap_over_a_dotted_key_binding_stays_valid_toml() {
        // A key binding written with dotted keys under `[editor]` (`keys.down =
        // "j"`) is the dotted-key section form. An in-app remap targets section
        // `editor.keys`; the splice can't match it and used to append a duplicate
        // `[editor.keys]` table → invalid TOML → reset (#1140).
        let existing = "[editor]\nvim = true\nkeys.down = \"j\"\nkeys.up = \"k\"\n";
        let out = set_value(existing, "editor.keys", "down", "\"n\"");
        assert_eq!(
            reload(&out, "editor.keys", "down"),
            Some(toml::Value::String("n".into())),
            "the remap is applied and reloads: {out}"
        );
        assert_eq!(
            reload(&out, "editor.keys", "up"),
            Some(toml::Value::String("k".into())),
            "the sibling binding is preserved: {out}"
        );
        assert_eq!(
            reload(&out, "editor", "vim"),
            Some(toml::Value::Boolean(true)),
            "the sibling [editor] value is preserved: {out}"
        );
    }

    #[test]
    fn set_value_appends_a_new_binding_to_an_inline_editor_table() {
        // Adding a brand-new key (a first key remap) to an inline-table `editor`
        // section must also stay valid TOML and keep the existing values.
        let existing = "editor = { vim = true }\n";
        let out = set_value(existing, "editor.keys", "down", "\"n\"");
        assert_eq!(
            reload(&out, "editor.keys", "down"),
            Some(toml::Value::String("n".into())),
            "the new binding reloads: {out}"
        );
        assert_eq!(
            reload(&out, "editor", "vim"),
            Some(toml::Value::Boolean(true)),
            "the inline-table value is preserved: {out}"
        );
    }

    #[test]
    fn set_value_leaves_the_valid_header_form_untouched() {
        // The fast path is unchanged for the ordinary `[header]` form: the output
        // is still the verbatim line splice (comments/formatting preserved), not a
        // structural rewrite. A rewrite would drop the comment and reflow the file.
        let existing = "# hand-maintained\n[editor]\nvim = false\ncolumn_width = 80\n";
        let out = set_value(existing, "editor", "vim", "true");
        assert_eq!(
            out, "# hand-maintained\n[editor]\nvim = true\ncolumn_width = 80\n",
            "the header form is a verbatim splice, no fallback rewrite: {out}"
        );
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

    // The config writers route through the atomic layer (#644), so an interrupted
    // rewrite leaves the user's hand-maintained config byte-for-byte intact rather
    // than truncating it. We drive the same pre-rename fault seam `atomic` uses for
    // its own crash test (#265), hermetically (#528/#531) against a scratch config
    // dir. Each writer (splice/batch/key-removal) is a distinct on-disk path.
    #[test]
    fn a_failed_config_write_preserves_the_previous_file() {
        let dir = crate::testutil::scratch_dir("config", "atomic-preserve");
        set_config_dir(Some(dir.clone()));
        let path = config_path().unwrap();

        // Seed a good config the writers must not clobber on a failed rewrite.
        let good = "[editor]\nvim = true\ncolumn_width = 80\n";
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, good).unwrap();

        // set_values (batch splice): armed to fail before the rename.
        atomic::fail_point::arm_pre_rename_failure();
        let changes = [("theme".into(), "name".into(), "\"light\"".into())];
        assert!(
            set_values(&changes).is_err(),
            "the doomed write reports err"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            good,
            "the previous config survives a failed set_values"
        );

        // add_workflow (single splice): same guarantee.
        atomic::fail_point::arm_pre_rename_failure();
        assert!(add_workflow("revise", &[String::from("save")]).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good);

        // remove_value (key removal): the removed line must not be lost when the
        // rewrite is interrupted.
        atomic::fail_point::arm_pre_rename_failure();
        assert!(remove_value("editor", "vim").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), good);

        set_config_dir(None);
        std::fs::remove_dir_all(&dir).ok();
    }

    // The happy path is unchanged by routing through the atomic layer: each writer
    // still produces exactly the spliced/removed result on disk (#644).
    #[test]
    fn config_writers_still_produce_the_expected_file_on_success() {
        let dir = crate::testutil::scratch_dir("config", "atomic-success");
        set_config_dir(Some(dir.clone()));
        let path = config_path().unwrap();

        set_values(&[("editor".into(), "vim".into(), "true".into())]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[editor]\nvim = true\n"
        );

        let removed = remove_value("editor", "vim").unwrap();
        assert!(removed, "the key was there to remove");
        assert!(
            !std::fs::read_to_string(&path).unwrap().contains("vim"),
            "the key is gone after remove_value"
        );

        set_config_dir(None);
        std::fs::remove_dir_all(&dir).ok();
    }
}

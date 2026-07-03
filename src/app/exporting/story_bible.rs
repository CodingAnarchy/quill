//! Story-bible export (#708): assemble the scattered `bible/` files into one clean,
//! shareable Markdown document — the machine scaffolding (YAML front matter and the
//! generated/notes block markers) stripped. Pure local concatenation of files
//! already on disk; no AI request.

use std::fmt::Write as _;

use super::super::*;

use crate::ai::bible;

/// One section of the assembled story bible (#708): a heading and the `bible/`
/// source it draws from — either a single file (`Single`) or every `.md` in a
/// directory (`Dir`, e.g. the per-character profiles). The order of
/// [`BIBLE_SECTIONS`] is the reading order of the exported document.
enum BibleSource {
    /// A single `bible/<file>` (the file name relative to `bible/`).
    Single(&'static str),
    /// Every `.md` under `bible/<dir>/`, concatenated in filename order.
    Dir(&'static str),
}

/// The story-bible sections in export order (#708): characters, relationships,
/// setting, timeline, plot, outline. Each pairs the `##` heading with the `bible/`
/// source it reads; a missing file or directory is simply skipped.
const BIBLE_SECTIONS: &[(&str, BibleSource)] = &[
    ("Characters", BibleSource::Dir("characters")),
    ("Relationships", BibleSource::Single("relationships.md")),
    ("Setting", BibleSource::Single("setting.md")),
    ("Timeline", BibleSource::Single("timeline.md")),
    ("Plot", BibleSource::Single("plot.md")),
    ("Outline", BibleSource::Single("outline.md")),
];

impl App {
    /// Export the story bible (#708) as one clean, shareable Markdown document to
    /// `export/story-bible.md`: the scattered `bible/` files assembled in a sensible
    /// order with the machine scaffolding — YAML front matter and the
    /// generated/notes block markers — stripped. Pure local concatenation of files
    /// already on disk; no AI request. A project with no bible yet still writes a
    /// document (its sections skipped), so the author sees the empty result rather
    /// than a silent no-op.
    pub(crate) fn export_story_bible(&mut self) {
        let doc = assemble_story_bible(&self.project.root, &self.project.manifest.title);
        self.write_export("story-bible", "md", doc.as_bytes());
    }
}

/// Assemble the project's story bible (#708) into one clean Markdown document: a
/// title, then each present section ([`BIBLE_SECTIONS`], in order) under a `##`
/// heading, its source file(s) reduced to human-readable content by
/// [`bible::clean`] (front matter and generated/notes markers stripped). Missing
/// bible files are skipped, so a partial (or empty) bible still assembles. Pure
/// over the filesystem — reads `bible/`, writes nothing — so it's the hermetic
/// core the export action ([`App::export_story_bible`]) writes out.
fn assemble_story_bible(root: &std::path::Path, title: &str) -> String {
    let bible = root.join("bible");
    let mut out = format!("# {title} — Story Bible\n");
    for (heading, source) in BIBLE_SECTIONS {
        let content = match source {
            BibleSource::Single(file) => clean_bible_file(&bible.join(file)),
            BibleSource::Dir(dir) => clean_bible_dir(&bible.join(dir)),
        };
        if let Some(content) = content {
            write!(out, "\n## {heading}\n\n{content}\n").unwrap();
        }
    }
    out
}

/// One `bible/` file cleaned for export (#708) — [`bible::clean`] of its text, or
/// `None` when the file is absent or has no content once the scaffolding is
/// stripped (so an empty section is skipped rather than left as a bare heading).
fn clean_bible_file(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let clean = bible::clean(&text);
    (!clean.trim().is_empty()).then_some(clean)
}

/// Every `.md` under a `bible/` subdirectory cleaned and concatenated for export
/// (#708), in filename order (deterministic), separated by a blank line — the
/// per-character profiles are the one directory-backed section. `None` when the
/// directory is absent or yields no content.
fn clean_bible_dir(dir: &std::path::Path) -> Option<String> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    files.sort();
    let parts: Vec<String> = files.iter().filter_map(|p| clean_bible_file(p)).collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

#[cfg(test)]
mod bible_export_tests {
    use super::assemble_story_bible;
    use crate::ai::bible::render;
    use std::fs;

    /// A unique temp project root for a hermetic test, cleaned by the caller.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("quill-bible-export-{}-{tag}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(dir.join("bible").join("characters")).unwrap();
        dir
    }

    #[test]
    fn assembles_clean_sections_in_order_with_markers_stripped() {
        let root = scratch("full");
        let bible = root.join("bible");
        // A character profile, a setting, a timeline, a plot file, an outline —
        // each rendered the way the trackers write them (front matter + markers).
        fs::write(
            bible.join("characters").join("mara.md"),
            render(
                "name: Mara\n",
                "## Physical\n- green eyes\n",
                "## Notes\n\nMara is the lead.\n",
                "2026-06-19",
            ),
        )
        .unwrap();
        fs::write(
            bible.join("setting.md"),
            render(
                "title: Setting\n",
                "## Gazetteer\n- Aldermoor\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();
        fs::write(
            bible.join("timeline.md"),
            render(
                "title: Timeline\n",
                "## Events\n- Day one\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();
        fs::write(
            bible.join("plot.md"),
            render(
                "title: Plot\n",
                "## Threads\n- The locket\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();
        fs::write(
            bible.join("outline.md"),
            render(
                "title: Outline\n",
                "## Beats\n- Inciting incident\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();
        fs::write(
            bible.join("relationships.md"),
            render(
                "title: Relationships\n",
                "## Mara\n- Jon — ally\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();

        let doc = assemble_story_bible(&root, "My Novel");

        // Title and every section heading present.
        assert!(doc.starts_with("# My Novel — Story Bible\n"), "{doc}");
        for h in [
            "## Characters",
            "## Relationships",
            "## Setting",
            "## Timeline",
            "## Plot",
            "## Outline",
        ] {
            assert!(doc.contains(h), "missing {h} in:\n{doc}");
        }
        // Clean content is present, machine scaffolding is gone.
        assert!(doc.contains("green eyes"));
        assert!(doc.contains("Mara is the lead."));
        assert!(doc.contains("Aldermoor"));
        assert!(!doc.contains("generated:start"), "markers stripped:\n{doc}");
        assert!(!doc.contains("digest:"), "front matter stripped:\n{doc}");
        assert!(!doc.contains("---\n"), "no YAML fences:\n{doc}");

        // Section order: characters → relationships → setting → timeline → plot → outline.
        let pos = |h: &str| doc.find(h).unwrap();
        assert!(pos("## Characters") < pos("## Relationships"));
        assert!(pos("## Relationships") < pos("## Setting"));
        assert!(pos("## Setting") < pos("## Timeline"));
        assert!(pos("## Timeline") < pos("## Plot"));
        assert!(pos("## Plot") < pos("## Outline"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn skips_missing_bible_files_gracefully() {
        let root = scratch("partial");
        // Only a setting file exists; every other section is absent.
        fs::write(
            root.join("bible").join("setting.md"),
            render(
                "title: Setting\n",
                "## Gazetteer\n- Aldermoor\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();

        let doc = assemble_story_bible(&root, "Sparse");
        assert!(doc.contains("## Setting"));
        assert!(doc.contains("Aldermoor"));
        // The absent sections are skipped, not left as empty headings.
        assert!(!doc.contains("## Characters"), "{doc}");
        assert!(!doc.contains("## Timeline"), "{doc}");
        assert!(!doc.contains("## Outline"), "{doc}");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_bible_still_writes_a_titled_document() {
        let root = scratch("empty");
        let doc = assemble_story_bible(&root, "Nothing Yet");
        assert_eq!(doc, "# Nothing Yet — Story Bible\n");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn multiple_character_profiles_concatenate_in_filename_order() {
        let root = scratch("cast");
        let chars = root.join("bible").join("characters");
        fs::write(
            chars.join("aaron.md"),
            render(
                "name: Aaron\n",
                "## Physical\n- tall\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();
        fs::write(
            chars.join("mara.md"),
            render(
                "name: Mara\n",
                "## Physical\n- green eyes\n",
                "## Notes\n",
                "2026-06-19",
            ),
        )
        .unwrap();

        let doc = assemble_story_bible(&root, "Cast");
        let aaron = doc.find("tall").expect("aaron kept");
        let mara = doc.find("green eyes").expect("mara kept");
        assert!(
            aaron < mara,
            "profiles concatenate in filename order:\n{doc}"
        );

        fs::remove_dir_all(&root).ok();
    }
}

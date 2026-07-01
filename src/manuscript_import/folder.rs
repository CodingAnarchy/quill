//! Folder-of-Markdown manuscript import (#593): each `.md` file directly inside
//! the folder is one chapter, in filename order (Quill's own on-disk layout, and
//! the natural shape of many drafts). The chapter title is the file's leading `#`
//! heading when it has one, else the file stem. Unlike the single-file Markdown
//! parser, files are *not* split internally — one file, one chapter. See the
//! crate module doc for the shared output contract.

use std::path::{Path, PathBuf};

use crate::frontmatter;

use super::{fallback_chapter, finalize_titles, Imported, ImportedChapter};

/// Parse a folder of per-chapter `.md` files (#593): one [`ImportedChapter`] per
/// `.md` file directly inside `folder`, in filename order. Reads only; the folder
/// is never altered. Errors if the directory can't be listed. A file that can't
/// be read is counted in `unreadable_bodies` (the shared #511 policy), never
/// silently dropped.
pub fn parse_markdown_folder(folder: &Path) -> std::io::Result<Imported> {
    let files = markdown_files(folder)?;

    let mut chapters: Vec<ImportedChapter> = Vec::new();
    let mut unreadable_bodies = 0usize;
    for file in &files {
        let stem_title = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        match std::fs::read_to_string(file) {
            Ok(text) => {
                // Split off any leading `---` front matter (Quill's chapter files
                // may carry one) so the `# Heading` right after it is still the
                // title; we keep only the body prose.
                let (_front, body) = frontmatter::split(&text);
                let (title, body) = split_leading_heading(body, &stem_title);
                chapters.push(ImportedChapter { title, body });
            }
            Err(_) => {
                // Listed but unreadable: keep the chapter (from its filename) with
                // an empty body and count the drop, rather than lose the slot.
                unreadable_bodies += 1;
                chapters.push(ImportedChapter {
                    title: stem_title,
                    body: String::new(),
                });
            }
        }
    }

    // An empty folder still yields one (empty) chapter, per the shared fallback.
    fallback_chapter(&mut chapters, &None, String::new());
    finalize_titles(&mut chapters);

    Ok(Imported {
        title: None,
        author: None,
        chapters,
        unreadable_bodies,
    })
}

/// The `.md` files directly inside `folder`, in sorted (filename) order. Mirrors
/// notes-import's `collect_import_files` folder branch.
fn markdown_files(folder: &Path) -> std::io::Result<Vec<PathBuf>> {
    let is_md = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_md(p))
        .collect();
    files.sort();
    Ok(files)
}

/// Split a chapter file's leading ATX `#` heading off as the title, returning
/// `(title, body)`. When the file has no leading heading, the title is `fallback`
/// (the file stem) and the whole text is the body. The body is trimmed to a
/// single trailing newline, matching the other Markdown paths.
fn split_leading_heading(text: &str, fallback: &str) -> (String, String) {
    let mut lines = text.lines();
    // Skip leading blank lines to find the first content line.
    let mut leading_blanks = 0usize;
    let first = loop {
        match lines.next() {
            Some(l) if l.trim().is_empty() => leading_blanks += 1,
            other => break other,
        }
    };
    if let Some(line) = first {
        if let Some(title) = heading_text(line) {
            // The body is everything after that heading line.
            let rest: Vec<&str> = text.lines().skip(leading_blanks + 1).collect();
            return (title, clean(&rest));
        }
    }
    let title = if fallback.trim().is_empty() {
        String::new()
    } else {
        fallback.to_string()
    };
    (title, clean(&text.lines().collect::<Vec<_>>()))
}

/// The text of a leading ATX heading line (`#`…`######` + space), trimmed; `None`
/// if the line isn't a heading.
fn heading_text(line: &str) -> Option<String> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
        Some(t[hashes..].trim().to_string())
    } else {
        None
    }
}

/// Join body lines, trimming surrounding blank lines; a non-empty body ends in a
/// single newline (the chapter-file convention).
fn clean(lines: &[&str]) -> String {
    let trimmed = lines.join("\n").trim().to_string();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hermetic temp folder (#531) unique to this test process + tag.
    fn temp_folder(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-mdfolder-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn one_chapter_per_file_in_filename_order() {
        let dir = temp_folder("order");
        std::fs::write(dir.join("02-two.md"), "# Two\n\nSecond.\n").unwrap();
        std::fs::write(dir.join("01-one.md"), "# One\n\nFirst.\n").unwrap();
        let out = parse_markdown_folder(&dir).unwrap();
        assert_eq!(out.chapters.len(), 2);
        assert_eq!(out.chapters[0].title, "One", "sorted by filename");
        assert_eq!(out.chapters[0].body, "First.\n");
        assert_eq!(out.chapters[1].title, "Two");
        assert_eq!(out.chapters[1].body, "Second.\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn falls_back_to_the_filename_when_there_is_no_heading() {
        let dir = temp_folder("nohead");
        std::fs::write(dir.join("prologue.md"), "It began at sea.\n").unwrap();
        let out = parse_markdown_folder(&dir).unwrap();
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "prologue", "stem is the title");
        assert_eq!(out.chapters[0].body, "It began at sea.\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn leading_heading_is_dropped_from_the_body() {
        let dir = temp_folder("head");
        std::fs::write(dir.join("ch.md"), "# The Storm\n\nRain fell.\n").unwrap();
        let out = parse_markdown_folder(&dir).unwrap();
        assert_eq!(out.chapters[0].title, "The Storm");
        assert_eq!(
            out.chapters[0].body, "Rain fell.\n",
            "the title heading isn't part of the prose"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn front_matter_before_the_heading_is_handled() {
        let dir = temp_folder("front");
        std::fs::write(dir.join("ch.md"), "---\nfoo: bar\n---\n# Titled\n\nBody.\n").unwrap();
        let out = parse_markdown_folder(&dir).unwrap();
        assert_eq!(out.chapters[0].title, "Titled");
        assert_eq!(out.chapters[0].body, "Body.\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_markdown_files_are_ignored() {
        let dir = temp_folder("mixed");
        std::fs::write(dir.join("one.md"), "# One\n\nA.\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a chapter\n").unwrap();
        let out = parse_markdown_folder(&dir).unwrap();
        assert_eq!(out.chapters.len(), 1, "only the .md file becomes a chapter");
        assert_eq!(out.chapters[0].title, "One");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_folder_yields_one_fallback_chapter() {
        let dir = temp_folder("empty");
        let out = parse_markdown_folder(&dir).unwrap();
        assert_eq!(out.chapters.len(), 1);
        assert_eq!(out.chapters[0].title, "Manuscript");
        std::fs::remove_dir_all(&dir).ok();
    }
}

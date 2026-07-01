//! Offline project archive (#594): write the whole project — manifest, chapters,
//! story bible, annotations, dictionary — to a single timestamped `.zip`, and
//! restore one into a fresh project folder. No git or remote required; this is
//! the portable, no-network backup the sole prior story (`vcs.rs` push/pull)
//! couldn't be.
//!
//! The zip machinery is the one the `.docx`/`.epub`/`.scriv` writers already use
//! ([`zip`]), so this adds no dependency. Archiving mirrors what git ignores
//! (see [`is_ignored`]) — the editor's transient state (`.quill/`), retired
//! chapters (`unused/`), the AI scoping cache, and OS cruft stay out — plus the
//! generated `export/` directory, so a backup is the manuscript and its bible,
//! not scratch. Restore refuses to clobber an existing project, so a stray path
//! can't overwrite one.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::project::MANIFEST_NAME;

/// The default file name for a project's archive: `<project>-<stamp>.zip`, where
/// `stamp` is the local `YYYY-MM-DD-HHMMSS` so successive backups sort and never
/// collide. `root`'s folder name seeds the project part (falling back to
/// `project` for an oddly named root).
pub fn default_name(root: &Path) -> String {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "project".into());
    format!("{name}-{}.zip", crate::date::file_stamp())
}

/// Whether the project-relative path `rel` is one an archive skips, mirroring the
/// default `.gitignore` (`vcs.rs`) plus the generated `export/` tree: the editor
/// state, retired chapters, the AI scoping cache, and OS/editor cruft carry no
/// manuscript, so a backup leaves them out. `rel` is matched by its components,
/// so a nested `foo/.DS_Store` is caught as readily as a top-level one.
fn is_ignored(rel: &Path) -> bool {
    let parts: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    // Top-level directories that hold only transient / generated content.
    if matches!(
        parts.first(),
        Some(&".git" | &".quill" | &"unused" | &"export")
    ) {
        return true;
    }
    // The AI scoping cache lives under bible/ (#90).
    if parts == ["bible", ".state.toml"] {
        return true;
    }
    // OS / editor cruft, anywhere in the tree.
    parts
        .iter()
        .any(|p| *p == ".DS_Store" || p.ends_with(".swp") || p.ends_with(".tmp"))
}

/// Collect the project files to archive: every file under `root` whose relative
/// path isn't [`is_ignored`], sorted so the archive is deterministic. Ignored
/// directories are pruned so their contents are never walked.
fn archived_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Recursively gather the non-ignored files under `dir`, recording each as a path
/// relative to `root`. A directory whose relative path is ignored is skipped
/// wholesale.
fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if is_ignored(&rel) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect(root, &path, out)?;
        } else {
            out.push(rel);
        }
    }
    Ok(())
}

/// Archive the project at `root` into `dest` (a `.zip`): every non-ignored file,
/// stored under its project-relative path so restore reproduces the layout. The
/// destination's parent directory is created if needed. Returns the number of
/// files written.
pub fn write_archive(root: &Path, dest: &Path) -> std::io::Result<usize> {
    let files = archived_files(root)?;
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let bytes = build_zip(root, &files)?;
    fs::write(dest, bytes)?;
    Ok(files.len())
}

/// Build the archive's bytes: one deflate-compressed entry per file, keyed by its
/// forward-slash relative path (the portable zip separator).
fn build_zip(root: &Path, files: &[PathBuf]) -> std::io::Result<Vec<u8>> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    for rel in files {
        let name = zip_name(rel);
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        let data = fs::read(root.join(rel))?;
        zip.write_all(&data)?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

/// A relative path as a forward-slash zip entry name, so archives read the same
/// on every platform regardless of the host separator.
fn zip_name(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Restore the archive at `src` into a new project folder `dest`, unpacking every
/// entry under it. Refuses to clobber an existing project (a `book.toml` already
/// at `dest`) or to write outside `dest` (a zip-slip guard), and requires the
/// archive to actually be a project (its manifest present). Returns the number of
/// files unpacked.
pub fn restore_archive(src: &Path, dest: &Path) -> std::io::Result<usize> {
    if crate::project::Project::manifest_path(dest).exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("a project already exists at {}", dest.display()),
        ));
    }
    let bytes = fs::read(src)?;
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(std::io::Error::other)?;
    let mut has_manifest = false;
    let mut written = 0;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(std::io::Error::other)?;
        let Some(rel) = file.enclosed_name() else {
            // A path that escapes the archive root (zip slip) — skip it rather
            // than write outside the destination.
            continue;
        };
        if file.is_dir() {
            continue;
        }
        if rel == Path::new(MANIFEST_NAME) {
            has_manifest = true;
        }
        let target = dest.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf)?;
        fs::write(&target, buf)?;
        written += 1;
    }
    if !has_manifest {
        // Not a Quill project archive — don't leave a half-written folder behind.
        let _ = fs::remove_dir_all(dest);
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} is not a Quill project archive (no {MANIFEST_NAME})",
                src.display()
            ),
        ));
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use crate::testutil::scratch_dir;

    /// A minimal project on disk: manifest, one chapter, a bible file, a
    /// dictionary, and annotations — the artifacts an archive must carry.
    fn seed_project(root: &Path) {
        Project::create(root).expect("create project");
        fs::write(root.join("dictionary.txt"), "flumox\n").unwrap();
        fs::write(root.join("annotations.toml"), "# notes\n").unwrap();
        fs::create_dir_all(root.join("bible").join("characters")).unwrap();
        fs::write(
            root.join("bible").join("characters").join("ada.md"),
            "# Ada\n",
        )
        .unwrap();
    }

    #[test]
    fn archive_holds_the_manifest_chapters_and_bible() {
        let dir = scratch_dir("archive", "contents");
        let root = dir.join("book");
        seed_project(&root);
        let zip = dir.join("out.zip");

        let n = write_archive(&root, &zip).unwrap();
        assert!(n >= 4, "manifest + chapter + bible + side files, got {n}");

        let bytes = fs::read(&zip).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"book.toml".to_string()));
        assert!(names.contains(&"chapter-1.md".to_string()));
        assert!(names.contains(&"dictionary.txt".to_string()));
        assert!(names.contains(&"annotations.toml".to_string()));
        assert!(names.contains(&"bible/characters/ada.md".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_excludes_ignored_paths() {
        let dir = scratch_dir("archive", "ignored");
        let root = dir.join("book");
        seed_project(&root);
        // Transient / generated content the archive must skip.
        fs::create_dir_all(root.join(".quill")).unwrap();
        fs::write(root.join(".quill").join("recovery.json"), "{}").unwrap();
        fs::create_dir_all(root.join("unused")).unwrap();
        fs::write(root.join("unused").join("old.md"), "gone\n").unwrap();
        fs::create_dir_all(root.join("export")).unwrap();
        fs::write(root.join("export").join("book.docx"), "zip").unwrap();
        fs::write(root.join("bible").join(".state.toml"), "cache\n").unwrap();
        fs::write(root.join(".DS_Store"), "junk").unwrap();
        let zip = dir.join("out.zip");

        write_archive(&root, &zip).unwrap();
        let bytes = fs::read(&zip).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(!names.iter().any(|n| n.starts_with(".quill/")));
        assert!(!names.iter().any(|n| n.starts_with("unused/")));
        assert!(!names.iter().any(|n| n.starts_with("export/")));
        assert!(!names.contains(&"bible/.state.toml".to_string()));
        assert!(!names.contains(&".DS_Store".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_reproduces_the_project() {
        let dir = scratch_dir("archive", "roundtrip");
        let root = dir.join("book");
        seed_project(&root);
        // A distinct chapter body so we can prove the bytes came through.
        fs::write(root.join("chapter-1.md"), "The night was long.\n").unwrap();
        let zip = dir.join("out.zip");
        write_archive(&root, &zip).unwrap();

        let restored = dir.join("restored");
        let n = restore_archive(&zip, &restored).unwrap();
        assert!(n >= 4);

        assert!(Project::manifest_path(&restored).exists());
        assert_eq!(
            fs::read_to_string(restored.join("chapter-1.md")).unwrap(),
            "The night was long.\n"
        );
        assert_eq!(
            fs::read_to_string(restored.join("bible").join("characters").join("ada.md")).unwrap(),
            "# Ada\n"
        );
        // The restored folder opens as a project.
        Project::open(&restored).expect("restored project opens");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_refuses_to_clobber_an_existing_project() {
        let dir = scratch_dir("archive", "clobber");
        let root = dir.join("book");
        seed_project(&root);
        let zip = dir.join("out.zip");
        write_archive(&root, &zip).unwrap();

        let occupied = dir.join("occupied");
        seed_project(&occupied);
        let err = restore_archive(&zip, &occupied).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_name_carries_the_project_and_a_stamp() {
        let name = default_name(Path::new("/tmp/My Novel"));
        assert!(name.starts_with("My Novel-"));
        assert!(name.ends_with(".zip"));
        // The stamp is YYYY-MM-DD-HHMMSS, so the name is well past the prefix.
        assert!(name.len() > "My Novel-.zip".len() + 10);
    }
}

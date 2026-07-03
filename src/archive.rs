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

use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::atomic;
use crate::project::MANIFEST_NAME;

/// The most total uncompressed bytes a restore will unpack from one archive
/// before giving up. A project backup is a manuscript and its bible — text, not
/// media — so even a large novel is a few tens of megabytes; 2 GiB is orders of
/// magnitude over any real project yet refuses a decompression bomb that would
/// otherwise expand without bound and exhaust memory or disk.
const MAX_TOTAL_UNCOMPRESSED: u64 = 2 * 1024 * 1024 * 1024;

/// The largest compression ratio (uncompressed ÷ compressed) any single entry is
/// allowed. Ordinary text deflates well but not absurdly (a few-fold, tens at
/// most for very repetitive prose); a classic zip bomb is a tiny entry that
/// balloons thousands-fold. 1000 leaves honest text ample room while catching
/// the bomb. A floor on the compressed size keeps the ratio from tripping on
/// tiny-but-innocent entries whose few bytes divide badly.
const MAX_ENTRY_RATIO: u64 = 1000;

/// Below this compressed size an entry's ratio isn't checked: a handful of
/// compressed bytes can legitimately expand past the ratio (a short, repetitive
/// header) without being a bomb, and the cumulative cap still bounds the total.
const RATIO_CHECK_FLOOR: u64 = 4 * 1024;

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
///
/// The zip streams straight to a sibling temp file, which is `sync_all`ed and
/// renamed over `dest` ([`atomic::write_with`]), so the whole compressed project
/// never sits in memory and a failed or colliding write (e.g. two same-second
/// backups) leaves any prior archive at `dest` intact rather than truncated
/// (#658).
pub fn write_archive(root: &Path, dest: &Path) -> std::io::Result<usize> {
    let files = archived_files(root)?;
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    atomic::write_with(dest, |file| stream_zip(root, &files, file))?;
    Ok(files.len())
}

/// Stream the archive into `sink`: one deflate-compressed entry per file, keyed
/// by its forward-slash relative path (the portable zip separator). Each file's
/// bytes are copied through a small buffer rather than read whole, so a large
/// chapter never lands in memory in full.
fn stream_zip(root: &Path, files: &[PathBuf], sink: &mut File) -> std::io::Result<()> {
    let mut zip = ZipWriter::new(sink);
    let opts = SimpleFileOptions::default();
    for rel in files {
        let name = zip_name(rel);
        zip.start_file(name, opts).map_err(std::io::Error::other)?;
        let mut src = File::open(root.join(rel))?;
        std::io::copy(&mut src, &mut zip)?;
    }
    zip.finish().map_err(std::io::Error::other)?;
    Ok(())
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
///
/// Each entry streams through a bounded, atomic copy — never the whole zip nor a
/// header-sized buffer in memory — with per-entry ratio and cumulative-size caps
/// that reject a decompression bomb (#699). Any failure (bad zip, I/O error,
/// rejected bomb, missing manifest) removes `dest`, so a rejected archive never
/// leaves a half-restored project behind.
pub fn restore_archive(src: &Path, dest: &Path) -> std::io::Result<usize> {
    if crate::project::Project::manifest_path(dest).exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("a project already exists at {}", dest.display()),
        ));
    }
    // Restore is the recovery path and must be at least as robust as archive,
    // which streams to keep the project out of memory (#658). Open the zip over a
    // buffered file handle rather than slurping the whole archive into RAM, and
    // unpack each entry through a bounded, atomic copy so a large or hostile
    // archive can't OOM (#699). Any failure removes the partial folder so a
    // rejected archive never leaves a half-restored project behind.
    match unpack(src, dest) {
        Ok(written) => Ok(written),
        Err(e) => {
            let _ = fs::remove_dir_all(dest);
            Err(e)
        }
    }
}

/// Unpack every entry of the archive at `src` under `dest`, returning the number
/// of files written. Split out from [`restore_archive`] so its single caller can
/// clean up a partial `dest` on *any* error, whether a bad zip, an I/O failure,
/// a rejected zip bomb, or a missing manifest.
fn unpack(src: &Path, dest: &Path) -> io::Result<usize> {
    let file = File::open(src)?;
    let mut zip = ZipArchive::new(BufReader::new(file)).map_err(io::Error::other)?;
    let mut has_manifest = false;
    let mut written = 0;
    let mut total_uncompressed: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(io::Error::other)?;
        let Some(rel) = entry.enclosed_name() else {
            // A path that escapes the archive root (zip slip) — skip it rather
            // than write outside the destination.
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        if rel == Path::new(MANIFEST_NAME) {
            has_manifest = true;
        }
        // Guard the per-entry compression ratio before decompressing a byte: a
        // classic bomb is a tiny compressed entry that balloons thousandsfold.
        // The declared uncompressed size is untrusted, so this is a cheap first
        // filter; the streamed cap below is the real ceiling.
        let compressed = entry.compressed_size();
        if compressed >= RATIO_CHECK_FLOOR && entry.size() / compressed.max(1) > MAX_ENTRY_RATIO {
            return Err(bomb_error());
        }
        let target = dest.join(&rel);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // Stream the entry through a bounded copy into an atomically-swapped temp
        // file. `budget` is what remains of the whole-archive cap; the copy stops
        // and errors the moment an entry (whose real size we don't trust from the
        // header) would push the running total past it, so no single entry — nor
        // the archive as a whole — can expand without bound.
        atomic::write_with(&target, |out| {
            let budget = MAX_TOTAL_UNCOMPRESSED - total_uncompressed;
            let n = bounded_copy(&mut entry, out, budget)?;
            total_uncompressed += n;
            Ok(())
        })?;
        written += 1;
    }
    if !has_manifest {
        // Not a Quill project archive.
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is not a Quill project archive (no {MANIFEST_NAME})",
                src.display()
            ),
        ));
    }
    Ok(written)
}

/// Copy at most `limit` bytes from `reader` to `writer`, returning the number
/// copied. Errors [`io::ErrorKind::InvalidData`] the instant the source would
/// exceed `limit` — the source's real length is unknown until it's read, so the
/// only safe bound is to stop mid-stream rather than trust a declared size.
/// A fixed buffer keeps memory flat regardless of the entry's size.
fn bounded_copy<R: Read, W: Write>(reader: &mut R, writer: &mut W, limit: u64) -> io::Result<u64> {
    let mut buf = [0u8; 64 * 1024];
    let mut copied: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(copied);
        }
        // Reading past the budget means the archive is larger than any real
        // project — refuse it rather than keep expanding a possible bomb.
        if copied + n as u64 > limit {
            return Err(bomb_error());
        }
        writer.write_all(&buf[..n])?;
        copied += n as u64;
    }
}

/// The error a restore fails with when an archive would expand past the size or
/// ratio caps — a likely decompression bomb.
fn bomb_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "archive rejected: an entry expands too large (possible zip bomb)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
    fn a_failed_write_leaves_the_prior_archive_intact() {
        let dir = scratch_dir("archive", "atomic");
        let root = dir.join("book");
        seed_project(&root);
        let zip = dir.join("out.zip");

        // A first, good archive at the destination.
        write_archive(&root, &zip).unwrap();
        let before = fs::read(&zip).unwrap();
        assert!(ZipArchive::new(Cursor::new(before.clone())).is_ok());

        // A second write to the same name that fails after the temp is written
        // but before the rename — the collision case a same-second backup hits.
        crate::atomic::fail_point::arm_pre_rename_failure();
        let err = write_archive(&root, &zip).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);

        // The prior archive is byte-for-byte intact — not truncated or replaced
        // by a half-written zip — and still opens.
        let after = fs::read(&zip).unwrap();
        assert_eq!(
            after, before,
            "the prior archive must survive a failed write"
        );
        assert!(ZipArchive::new(Cursor::new(after)).is_ok());

        // No temp file left behind.
        let strays: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "temp file left behind: {strays:?}");

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

    /// Write a zip at `path` from `entries` (name, uncompressed bytes), deflating
    /// each so a highly repetitive payload lands with a bomb-like ratio.
    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn bounded_copy_stops_at_the_limit() {
        // The core defense in isolation: a source larger than the budget errors
        // mid-stream rather than allocating or copying past it.
        let src = vec![0u8; 4096];
        let mut out = Vec::new();
        let err = bounded_copy(&mut Cursor::new(&src), &mut out, 1024).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(out.len() <= 1024, "must not write past the budget");

        // A source within budget copies whole and reports its length.
        let mut out = Vec::new();
        let n = bounded_copy(&mut Cursor::new(&src), &mut out, 8192).unwrap();
        assert_eq!(n, 4096);
        assert_eq!(out, src);
    }

    #[test]
    fn restore_rejects_a_zip_bomb_entry() {
        let dir = scratch_dir("archive", "bomb");
        // A well-formed manifest plus one entry of many megabytes of zeros, which
        // deflates thousandsfold — a decompression bomb. The ratio guard must
        // refuse it, and no half-restored project may survive.
        let zip = dir.join("bomb.zip");
        let bomb = vec![0u8; 16 * 1024 * 1024];
        write_zip(
            &zip,
            &[(MANIFEST_NAME, b"title = \"x\"\n"), ("chapter-1.md", &bomb)],
        );

        let restored = dir.join("restored");
        let err = restore_archive(&zip, &restored).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            !restored.exists(),
            "a rejected archive must leave no project"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_blocks_zip_slip() {
        let dir = scratch_dir("archive", "slip");
        // A traversal entry (`../escape.md`) alongside a real project. The slip
        // entry must never be written outside `dest`; the rest still restores.
        let zip = dir.join("slip.zip");
        write_zip(
            &zip,
            &[
                (MANIFEST_NAME, b"title = \"x\"\n"),
                ("../escape.md", b"pwned\n"),
                ("chapter-1.md", b"safe\n"),
            ],
        );

        let restored = dir.join("restored");
        restore_archive(&zip, &restored).unwrap();
        // The traversal target — a sibling of `restored` — must not exist.
        assert!(
            !dir.join("escape.md").exists(),
            "zip slip wrote outside the destination"
        );
        assert!(restored.join("chapter-1.md").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}

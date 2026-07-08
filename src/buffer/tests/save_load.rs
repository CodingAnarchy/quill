//! Data-path resilience (#264) and file round-trips: reads that must error
//! rather than panic, saves that keep the buffer recoverable, byte-for-byte
//! round-trips, front-matter re-attachment, trailing-newline normalization, and
//! path/extension handling.

use super::*;

// --- Data-path resilience (#264) ---------------------------------------

#[test]
fn from_path_surfaces_a_read_error_rather_than_panicking() {
    // A path that exists but is a directory: `read_to_string` fails with a
    // non-NotFound error, which must surface as `Err`, not a panic and not a
    // silently-empty buffer (which a later save would write over the dir).
    // The name carries an extension so `with_default_extension` leaves it be
    // and the resolved path really is the directory.
    let dir = temp_path("from-path-is-a-dir.md");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let result = Buffer::from_path(&dir);
    assert!(
        result.is_err(),
        "opening a directory as a file should error"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_save_keeps_the_buffer_dirty_and_re_savable() {
    // Point the buffer at an existing directory so the write fails; the
    // unsaved content must remain (dirty stays true), so a later save to a
    // good path still works.
    let dir = temp_path("save-onto-a-dir");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let mut b = Buffer::from_text("precious unsaved words");
    b.path = Some(dir.clone());
    b.dirty = true;
    assert!(b.save().is_err(), "writing onto a directory should error");
    assert!(b.dirty, "a failed save must leave the buffer dirty");

    // Re-savable to a real file, with the content intact.
    let good = temp_path("save-onto-a-dir-good.md");
    b.path = Some(good.clone());
    b.save()
        .expect("the buffer should still save to a valid path");
    assert_eq!(read(&good), "precious unsaved words\n");

    fs::remove_dir_all(&dir).ok();
    fs::remove_file(&good).ok();
}

#[test]
fn round_trip_is_byte_identical_with_trailing_newline() {
    let path = temp_path("roundtrip.md");
    let original = "# Chapter One\n\nThe night was *dark* and **stormy**.\n";
    write(&path, original);

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), original);
    fs::remove_file(&path).ok();
}

#[test]
fn front_matter_is_kept_off_the_prose_and_re_attached_on_save() {
    let path = temp_path("frontmatter.md");
    let original = "---\nstatus: draft\npov: Mara\n---\n# Chapter One\n\nThe body.\n";
    write(&path, original);

    let mut buf = Buffer::from_path(&path).unwrap();
    // The prose area (rope) holds only the body — no raw YAML.
    assert_eq!(buf.line(0), "# Chapter One");
    assert!(buf.front_matter().is_some());
    // Word count covers the body, not the front matter keys.
    assert_eq!(
        buf.word_count(),
        count_words("# Chapter One\n\nThe body.\n")
    );

    // Saving re-attaches the front matter untouched: byte-for-byte round-trip.
    buf.save().unwrap();
    assert_eq!(read(&path), original);
    fs::remove_file(&path).ok();
}

#[test]
fn save_adds_missing_trailing_newline() {
    let path = temp_path("no-newline.md");
    write(&path, "No newline at end");

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), "No newline at end\n");
    fs::remove_file(&path).ok();
}

#[test]
fn save_collapses_multiple_trailing_newlines_to_one() {
    let path = temp_path("many-newlines.md");
    write(&path, "Body\n\n\n\n");

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), "Body\n");
    fs::remove_file(&path).ok();
}

#[test]
fn empty_document_saves_empty_file() {
    let path = temp_path("empty.md");
    write(&path, "");

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), "");
    fs::remove_file(&path).ok();
}

#[test]
fn non_ascii_round_trips_exactly() {
    let path = temp_path("unicode.md");
    let original = "Café — résumé, naïve façade. 日本語のテキスト. 🚀✍️🌙\n";
    write(&path, original);

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.save().unwrap();

    assert_eq!(read(&path), original);
    fs::remove_file(&path).ok();
}

#[test]
fn extensionless_path_defaults_to_md() {
    let path = temp_path("manuscript"); // no extension
    let buf = Buffer::from_path(&path).unwrap();

    let expected = path.with_extension("md");
    assert_eq!(buf.path.as_deref(), Some(expected.as_path()));
}

#[test]
fn explicit_extension_is_respected() {
    let path = temp_path("notes.txt");
    let buf = Buffer::from_path(&path).unwrap();

    assert_eq!(buf.path.as_deref(), Some(path.as_path()));
}

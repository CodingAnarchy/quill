//! The data-path test suite (#265): the regression net behind epic #15's
//! "never lose an author's words" promise. Three kinds of test, all runnable
//! under plain `cargo test` (no nightly, no separate fuzz crate):
//!
//! 1. **Round-trip / property tests** over randomized edit sequences and
//!    arbitrary content — load → edit → save → reload preserves the author's
//!    text and never mangles it across a save/reload boundary.
//! 2. **Adversarial-input ("fuzz") tests** — the loaders/parsers (manifest TOML,
//!    front matter, the Markdown buffer) survive arbitrary bytes, unusual
//!    Unicode, and very large input without panicking or corrupting content.
//! 3. **Failure injection** — a simulated write failure is handled gracefully
//!    and the previous good file survives, exercising atomic save (#262).
//!
//! These complement the example-based tests living with each module: this suite
//! is systematic (proptest-driven) and cross-cutting (buffer + project +
//! front matter together).

use proptest::prelude::*;
use std::fs;
use std::path::PathBuf;

use crate::buffer::{count_words, normalize_trailing_newline, Buffer};
use crate::frontmatter::split;
use crate::project::{Manifest, Project};

/// A scratch path under a per-process, per-tag directory. Reused across a
/// property's cases (each case overwrites it); distinct tags keep parallel test
/// functions from colliding.
fn scratch(tag: &str, file: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-datapath-{}-{tag}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir.join(file)
}

/// A fresh, empty per-process directory for a failure-injection test.
fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("quill-datapath-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// One editing action, mapped onto the buffer's real edit/navigation API.
#[derive(Clone, Debug)]
enum Op {
    Insert(char),
    Backspace,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        any::<char>().prop_map(Op::Insert),
        Just(Op::Backspace),
        Just(Op::Left),
        Just(Op::Right),
        Just(Op::Up),
        Just(Op::Down),
        Just(Op::Home),
        Just(Op::End),
    ]
}

fn apply(b: &mut Buffer, op: &Op) {
    match op {
        Op::Insert(c) => b.insert_char(*c),
        Op::Backspace => b.backspace(),
        Op::Left => b.move_left(),
        Op::Right => b.move_right(),
        Op::Up => b.move_up(80),
        Op::Down => b.move_down(80),
        Op::Home => b.move_line_start(),
        Op::End => b.move_line_end(),
    }
}

// --- Round-trip / property tests ------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// A random sequence of edits, then save → reload, preserves the author's
    /// prose: the reloaded body equals the in-memory text (modulo the one
    /// documented trailing-newline normalization), and a re-save is a fixed
    /// point. Scoped to prose — content that itself begins with a `---` fence is
    /// reserved front-matter syntax, exercised by the front-matter test below.
    #[test]
    fn edits_then_save_reload_preserve_content(ops in prop::collection::vec(op_strategy(), 0..150)) {
        let mut b = Buffer::from_text("");
        for op in &ops {
            apply(&mut b, op);
        }

        let p1 = scratch("edit", "a.md");
        b.path = Some(p1.clone());
        b.dirty = true;
        b.save().unwrap(); // must not panic on any edited content
        let c1 = fs::read_to_string(&p1).unwrap();

        let mut reloaded = Buffer::from_path(&p1).unwrap();
        if !c1.starts_with("---\n") {
            prop_assert_eq!(reloaded.text(), normalize_trailing_newline(&b.text()));
            // And re-saving is a fixed point — the bytes don't drift on reload.
            let p2 = scratch("edit", "b.md");
            reloaded.path = Some(p2.clone());
            reloaded.dirty = true;
            reloaded.save().unwrap();
            prop_assert_eq!(fs::read_to_string(&p2).unwrap(), c1);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Arbitrary prose (that isn't itself a front-matter block) round-trips
    /// through save and reload, modulo the one documented normalization (a
    /// single trailing newline).
    #[test]
    fn arbitrary_prose_round_trips(text in any::<String>()) {
        prop_assume!(!text.starts_with("---\n"));
        let mut b = Buffer::from_text(&text);
        let path = scratch("prose", "p.md");
        b.path = Some(path.clone());
        b.dirty = true;
        b.save().unwrap();
        let reloaded = Buffer::from_path(&path).unwrap();
        prop_assert_eq!(reloaded.text(), normalize_trailing_newline(&text));
    }

    /// A chapter file with front matter round-trips byte-for-byte: the verbatim
    /// block is split off on load and re-attached unchanged on save.
    #[test]
    fn front_matter_round_trips_byte_for_byte(
        keys in prop::collection::vec("[a-z]{1,8}: [^\n]{0,20}", 1..5),
        body in "[^\n]{0,40}(\n[^\n]{0,40}){0,8}",
    ) {
        let inner: String = keys.iter().map(|k| format!("{k}\n")).collect();
        let original = format!("---\n{inner}---\n{}", normalize_trailing_newline(&body));

        let src = scratch("fm", "c.md");
        fs::write(&src, &original).unwrap();
        let mut b = Buffer::from_path(&src).unwrap();

        let out = scratch("fm", "c2.md");
        b.path = Some(out.clone());
        b.dirty = true;
        b.save().unwrap();

        prop_assert_eq!(fs::read_to_string(&out).unwrap(), original);
    }
}

// --- Adversarial-input ("fuzz") tests -------------------------------------

proptest! {
    /// `frontmatter::split` is total (never panics) and never invents or drops
    /// bytes: with no front-matter block the whole input is the body; with one,
    /// both parts are verbatim slices of the input and the body is a suffix.
    #[test]
    fn frontmatter_split_is_total_and_preserves_bytes(s in any::<String>()) {
        let (front, body) = split(&s);
        match front {
            None => prop_assert_eq!(body, s.as_str(), "no block ⇒ the input is the body"),
            Some(inner) => {
                prop_assert!(s.starts_with("---\n"), "a block is only recognized after an opening fence");
                prop_assert!(s.ends_with(body), "the body must be a suffix slice of the input");
                prop_assert!(s.contains(inner), "the front matter is a verbatim slice of the input");
            }
        }
    }

    /// Parsing an arbitrary string as the manifest never panics — it's an
    /// `Ok`/`Err`, never a crash.
    #[test]
    fn manifest_parse_never_panics(s in any::<String>()) {
        let _: Result<Manifest, _> = toml::from_str(&s);
    }

    /// Loading a chapter from arbitrary *bytes* (including invalid UTF-8) never
    /// panics; a read failure surfaces as `Err`, valid text as `Ok`.
    #[test]
    fn loading_arbitrary_bytes_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..3000)) {
        let path = scratch("bytes", "b.md");
        fs::write(&path, &bytes).unwrap();
        let _ = Buffer::from_path(&path);
    }

    /// The word counter is total over arbitrary input.
    #[test]
    fn count_words_never_panics(s in any::<String>()) {
        let _ = count_words(&s);
    }
}

/// Unusual but valid Unicode — stacked combining marks, ZWJ emoji, RTL
/// overrides, zero-width spaces, a BOM, control characters — survives editing
/// near grapheme boundaries and a save/reload round-trip.
#[test]
fn adversarial_unicode_survives_editing_and_round_trip() {
    let samples = [
        "e\u{0301}\u{0301}\u{0301}",        // a letter with three stacked accents
        "\u{200B}\u{200B}zero-width",       // zero-width spaces
        "\u{1F469}\u{200D}\u{1F467}",       // ZWJ family emoji (one grapheme)
        "\u{202E}reversed\u{202C}",         // right-to-left override
        "\u{FEFF}bom at the start",         // byte-order mark
        "tab\tand\rcarriage",               // embedded control characters
        "ünïcödé ßß 漢字 かな 🜂",
    ];
    for s in samples {
        let mut b = Buffer::from_text(s);
        // Edit near the front: a grapheme-aware buffer must not split a cluster.
        b.move_right();
        b.insert_char('!');
        b.backspace();
        let path = scratch("uni", "u.md");
        b.path = Some(path.clone());
        b.dirty = true;
        b.save().unwrap();
        let reloaded = Buffer::from_path(&path).unwrap();
        assert_eq!(reloaded.text(), normalize_trailing_newline(&b.text()), "round-trip of {s:?}");
    }
}

/// A very large chapter (~1 MB) saves and reloads with its content intact, in a
/// blink — the data path is linear, not pathological, on big input.
#[test]
fn a_large_chapter_round_trips() {
    let big = "The night was dark and the road was long.\n".repeat(25_000);
    let mut b = Buffer::from_text(&big);
    let path = scratch("large", "L.md");
    b.path = Some(path.clone());
    b.dirty = true;
    b.save().unwrap();
    let reloaded = Buffer::from_path(&path).unwrap();
    assert_eq!(reloaded.text(), normalize_trailing_newline(&big));
    let _ = fs::remove_file(&path);
}

// --- Failure injection -----------------------------------------------------
//
// Simulate a crash mid-save — after the temp file is written but before the
// rename — via atomic's test-only fail point. This is the case that actually
// distinguishes an atomic save (target untouched, previous version survives)
// from a truncate-then-write (target already destroyed): a non-atomic
// implementation fails these tests. Portable (no permissions / root games), and
// the real save methods (`Buffer::save`, `Project::save_manifest`) are the path
// under test.

/// A chapter save interrupted before its rename leaves the previously saved
/// version byte-for-byte intact, and the buffer stays dirty (re-savable).
#[test]
fn an_interrupted_chapter_save_preserves_the_previous_version() {
    let dir = scratch_dir("fail-chapter");
    let path = dir.join("one.md");
    let mut b = Buffer::from_text("the good version");
    b.path = Some(path.clone());
    b.dirty = true;
    b.save().unwrap();
    let good = fs::read_to_string(&path).unwrap();

    b.move_line_end();
    b.insert_char('!'); // an unsaved edit, then a save that "crashes" before the rename
    crate::atomic::fail_point::arm_pre_rename_failure();
    let result = b.save();

    assert!(result.is_err(), "the interrupted save must report failure");
    assert!(b.dirty, "a failed save must leave the buffer dirty and re-savable");
    assert_eq!(fs::read_to_string(&path).unwrap(), good, "the previous version must survive");

    // And the buffer really is re-savable once the fault clears.
    b.save().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "the good version!\n");
    fs::remove_dir_all(&dir).ok();
}

/// A manifest save interrupted before its rename leaves the previous `book.toml`
/// intact.
#[test]
fn an_interrupted_manifest_save_preserves_the_previous_version() {
    let dir = scratch_dir("fail-manifest");
    let mut project = Project::create(&dir).unwrap();
    let manifest_path = Project::manifest_path(&dir);
    let before = fs::read_to_string(&manifest_path).unwrap();

    project.manifest.title = "a title that should never reach disk".into();
    crate::atomic::fail_point::arm_pre_rename_failure();
    let result = project.save_manifest();

    assert!(result.is_err(), "the interrupted manifest save must report failure");
    assert_eq!(fs::read_to_string(&manifest_path).unwrap(), before, "the previous manifest must survive");
    fs::remove_dir_all(&dir).ok();
}

// --- Malformed-input edge cases (#408) -------------------------------------

/// A chapter whose `---` block holds invalid YAML: the prose body loads intact,
/// the block is preserved verbatim across a save, and the good keys still read
/// while the broken lines are simply ignored — never a panic.
#[test]
fn malformed_front_matter_preserves_body_and_good_keys() {
    use crate::frontmatter::Status;
    let path = scratch("bad-fm", "ch.md");
    let original = "---\nstatus: draft\nsummary: : not : valid : yaml\n[ not yaml at all\n---\nThe prose survives.\n";
    fs::write(&path, original).unwrap();

    let mut buf = Buffer::from_path(&path).expect("loads despite broken YAML");
    assert_eq!(buf.text(), "The prose survives.\n", "body intact, front matter split off");
    let fm = buf.front_matter().expect("the block is preserved, not dropped");
    assert_eq!(fm.status(), Some(Status::Draft), "a valid key amid the broken lines still reads");
    // Chosen behavior is permissive line-scan: a malformed value is *preserved*
    // verbatim as a literal, not parsed-and-dropped — and never a panic.
    assert_eq!(fm.summary().as_deref(), Some(": not : valid : yaml"), "the odd value is kept as-is");

    buf.save().expect("saves");
    assert_eq!(fs::read_to_string(&path).unwrap(), original, "the block round-trips verbatim");
}

/// An empty `chapters = []` or a missing chapters key opens with a starter
/// chapter — the documented safe default — rather than erroring or crashing.
#[test]
fn a_manifest_with_no_chapters_opens_with_a_starter() {
    for manifest in ["title = \"Book\"\nchapters = []\n", "title = \"Book\"\n"] {
        let dir = scratch_dir("no-chapters");
        fs::write(Project::manifest_path(&dir), manifest).unwrap();
        let project = Project::open(&dir).expect("opens with a safe default");
        assert_eq!(project.manifest.chapters.len(), 1, "a starter chapter is added: {manifest:?}");
    }
}

/// An unparseable manifest is a reported error naming the file, not a crash.
#[test]
fn an_unparseable_manifest_is_reported() {
    let dir = scratch_dir("bad-manifest");
    fs::write(Project::manifest_path(&dir), "title = \"X\"\n[[chapters\nfile = \"a.md\"\n").unwrap();
    let err = Project::open(&dir).err().expect("malformed TOML must error");
    assert!(err.to_string().contains("book.toml"), "the error names the manifest: {err}");
}

/// A chapter entry with an empty `file` path is reported on open (#408), not
/// silently opened as an empty buffer at a stray path.
#[test]
fn a_chapter_entry_with_an_empty_file_path_is_reported() {
    let dir = scratch_dir("empty-file");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"X\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"\"\n",
    )
    .unwrap();
    let err = Project::open(&dir).err().expect("an empty file path must be reported");
    assert!(err.to_string().contains("empty file path"), "{err}");
}

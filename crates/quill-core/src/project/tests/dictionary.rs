//! Tests for the per-project spelling dictionary: accepting a word (added
//! case-insensitively, deduped, existing entries preserved), the
//! `is_accepted` lookup, and picking up an edit made to the file outside the
//! editor.

use super::*;

#[test]
fn accepts_words_into_the_project_dictionary() {
    let dir = temp_dir("dict");
    let mut p = Project::create(&dir).unwrap();

    assert!(!p.is_accepted("Vashti"));
    assert!(p.accept_word("Vashti").unwrap(), "newly added");
    assert!(
        p.is_accepted("Vashti") && p.is_accepted("vashti"),
        "matched case-insensitively"
    );
    assert!(
        !p.accept_word("vashti").unwrap(),
        "deduped case-insensitively"
    );

    // Persisted to dictionary.txt (original case) and respected after reload.
    let file = fs::read_to_string(dir.join(DICTIONARY_NAME)).unwrap();
    assert!(file.contains("Vashti"), "{file}");
    assert!(
        Project::open(&dir).unwrap().is_accepted("VASHTI"),
        "loaded on reopen"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn is_accepted_short_circuits_and_matches_case_insensitively() {
    // The empty-dictionary case (no custom words) accepts nothing — the common
    // idle-pass path, guarded at zero cost (#780).
    let dir = temp_dir("dict-empty");
    let mut p = Project::create(&dir).unwrap();
    assert!(p.dictionary.is_empty());
    assert!(!p.is_accepted("anything"));
    assert!(!p.is_accepted("MiXeD"));

    // With entries, matching is case-insensitive for both lowercase (borrow-lookup)
    // and uppercase-bearing (lowercased) words, unchanged from before.
    p.accept_word("Vashti").unwrap();
    assert!(p.is_accepted("vashti"), "already-lowercase, no alloc path");
    assert!(p.is_accepted("VASHTI"), "uppercase, lowercased path");
    assert!(p.is_accepted("VaShTi"), "mixed case");
    assert!(!p.is_accepted("other"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn accepting_a_word_preserves_existing_dictionary_entries() {
    let dir = temp_dir("dict-append");
    let mut p = Project::create(&dir).unwrap();
    fs::write(dir.join(DICTIONARY_NAME), "Aerendil\n").unwrap();
    p.dictionary = load_dictionary(&dir); // re-read what we just wrote
    p.accept_word("Vashti").unwrap();
    let file = fs::read_to_string(dir.join(DICTIONARY_NAME)).unwrap();
    assert!(
        file.contains("Aerendil") && file.contains("Vashti"),
        "both kept: {file}"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reload_dictionary_picks_up_an_external_edit() {
    // The in-app view/edit path (#935) writes `dictionary.txt` directly, then asks
    // the project to re-read it. `reload_dictionary` reflects an added word and
    // drops a removed one, without reopening the project.
    let dir = temp_dir("dict-reload");
    let mut p = Project::create(&dir).unwrap();
    assert!(p.dictionary.is_empty());

    // A hand-edit adds two words at the dictionary path the app exposes.
    fs::write(p.dictionary_path(), "Vashti\nAerendil\n").unwrap();
    assert!(!p.is_accepted("Vashti"), "not yet loaded");
    p.reload_dictionary();
    assert!(
        p.is_accepted("Vashti") && p.is_accepted("Aerendil"),
        "the external edit loaded"
    );

    // Removing a word from the file and reloading drops it from the set.
    fs::write(p.dictionary_path(), "Vashti\n").unwrap();
    p.reload_dictionary();
    assert!(p.is_accepted("Vashti"));
    assert!(!p.is_accepted("Aerendil"), "the removed word is gone");
    fs::remove_dir_all(&dir).ok();
}

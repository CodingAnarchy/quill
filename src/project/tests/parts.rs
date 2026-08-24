//! Tests for parts (the manifest's chapter groupings): their round trip and the
//! body-position → raw-index mapping, plus how every structural edit moves the
//! ranges with the chapters — insert inside / before a part, delete (including
//! the last chapter of a part), split, merge, a move across a boundary, and a
//! front-matter insert leaving the body parts untouched.

use super::*;

#[test]
fn parts_round_trip_and_map_chapters_to_ranges() {
    let dir = temp_dir("parts");
    let mut project = Project::create(&dir).unwrap(); // 1 starter chapter
    assert!(project.manifest.parts.is_none());
    assert_eq!(project.manifest.part_label(), "part"); // default
    assert_eq!(project.manifest.part_of(0), None); // no parts → no mapping

    // Four body chapters for the two parts to cover.
    project.add_chapter(1, "Two").unwrap();
    project.add_chapter(2, "Three").unwrap();
    project.add_chapter(3, "Four").unwrap();
    project.manifest.part_label = Some("act".into());
    project.manifest.parts = Some(vec![
        Part {
            name: Some("The Gathering".into()),
            chapters: [1, 2],
            goal: Some(20_000),
        },
        Part {
            name: None,
            chapters: [3, 4],
            goal: None,
        },
    ]);
    project.save_manifest().unwrap();

    // `[[parts]]` serializes after `part_label` (a scalar) and before
    // `[[chapters]]` (valid TOML: scalars precede arrays-of-tables).
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        toml.find("part_label").unwrap() < toml.find("[[parts]]").unwrap(),
        "part_label must precede [[parts]]:\n{toml}"
    );
    assert!(
        toml.find("[[parts]]").unwrap() < toml.find("[[chapters]]").unwrap(),
        "parts must precede chapters:\n{toml}"
    );

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.part_label(), "act");
    let parts = reopened.manifest.parts.as_ref().unwrap();
    assert_eq!(parts[0].name.as_deref(), Some("The Gathering"));
    assert_eq!(parts[0].chapters, [1, 2]);
    assert_eq!(parts[0].goal, Some(20_000), "part goal round-trips (#146)");
    assert_eq!(parts[1].goal, None);
    // Chapter index 0 (chapter 1) is in part 0; index 2 (chapter 3) in part 1.
    assert_eq!(reopened.manifest.part_of(0), Some(0));
    assert_eq!(reopened.manifest.part_of(2), Some(1));
    // A chapter outside every range maps to no part.
    assert_eq!(reopened.manifest.part_of(9), None);

    fs::remove_dir_all(&dir).ok();
}

// --- Parts follow structural edits (#316) -------------------------------------
//
// A `[[parts]]` range is an inclusive 1-based BODY-position span; the structural
// edits (add/split/delete/merge/move) used to leave those ranges stale, so a part
// silently regrouped the wrong chapters (and the report mis-attributed per-part
// words). These tests scaffold a project with parts, run each edit, and assert the
// parts still group their ORIGINAL chapters — by title, the content-level check —
// and that the per-part word attribution the report computes stays correct.

/// A project with four body chapters, each holding a body whose word count equals
/// its 1-based position (chapter 1 → 1 word, …, chapter 4 → 4 words), and two
/// parts `[1,2]` / `[3,4]`. Titles are `"C1".."C4"` so a part's membership can be
/// checked by content, not position.
fn parts_fixture(name: &str) -> (PathBuf, Project) {
    let dir = temp_dir(name);
    let mut project = Project::create(&dir).unwrap(); // 1 starter chapter at index 0
                                                      // Start from a known 4-chapter body: chapter i holds i words, titled "Ci".
    project.manifest.chapters.clear();
    for i in 1..=4 {
        let body = vec!["word"; i].join(" ");
        project
            .insert_chapter_with_body(i - 1, &format!("C{i}"), &body)
            .unwrap();
    }
    project.manifest.parts = Some(vec![
        Part {
            name: Some("One".into()),
            chapters: [1, 2],
            goal: None,
        },
        Part {
            name: Some("Two".into()),
            chapters: [3, 4],
            goal: None,
        },
    ]);
    project.save_manifest().unwrap();
    (dir, project)
}

/// The chapter titles a part `[a, b]` groups, resolved through the same
/// body-position → raw-index mapping the tree/report use (`front + a-1 .. front + b`).
fn part_titles(project: &Project, part: usize) -> Vec<String> {
    let front = project.manifest.front_chapter_count();
    let [a, b] = project.manifest.parts.as_ref().unwrap()[part].chapters;
    ((front + a - 1)..(front + b))
        .map(|i| project.manifest.chapters[i].title.clone())
        .collect()
}

/// The words a part `[a, b]` attributes, summed over the raw indices it covers —
/// the report's `part_word_counts` computation, at the project level.
fn part_words(project: &Project, part: usize) -> usize {
    let front = project.manifest.front_chapter_count();
    let [a, b] = project.manifest.parts.as_ref().unwrap()[part].chapters;
    ((front + a - 1)..(front + b))
        .map(|i| project.manifest.chapters[i].words)
        .sum()
}

#[test]
fn insert_grows_the_part_it_lands_in_keeping_the_grouping() {
    let (dir, mut project) = parts_fixture("parts-insert");
    // Insert a new chapter inside part 0 (between C1 and C2 → raw index 1).
    project.add_chapter(1, "NEW").unwrap();
    // Before the fix the range stayed [1,2] and silently dropped C2 out of part 0
    // (C2 moved to body position 3, now grouped under part 1). It must grow.
    assert_eq!(project.manifest.parts.as_ref().unwrap()[0].chapters, [1, 3]);
    assert_eq!(project.manifest.parts.as_ref().unwrap()[1].chapters, [4, 5]);
    assert_eq!(
        part_titles(&project, 0),
        ["C1", "NEW", "C2"],
        "part 0 still groups C1 and C2 (now with the inserted chapter)"
    );
    assert_eq!(
        part_titles(&project, 1),
        ["C3", "C4"],
        "part 1 still groups exactly C3 and C4 — not silently shifted"
    );
    // Per-part word attribution follows: part 1 still sums C3+C4 = 3+4 = 7.
    assert_eq!(part_words(&project, 1), 7);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn insert_before_a_part_shifts_it_intact() {
    let (dir, mut project) = parts_fixture("parts-insert-before");
    // Insert before every part (raw index 0 → body position 1, the start edge of
    // part 0). Convention: a start-edge insert pushes the part down, not into it.
    project.add_chapter(0, "NEW").unwrap();
    assert_eq!(project.manifest.parts.as_ref().unwrap()[0].chapters, [2, 3]);
    assert_eq!(project.manifest.parts.as_ref().unwrap()[1].chapters, [4, 5]);
    assert_eq!(part_titles(&project, 0), ["C1", "C2"], "part 0 keeps C1,C2");
    assert_eq!(part_titles(&project, 1), ["C3", "C4"], "part 1 keeps C3,C4");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn delete_inside_a_part_shrinks_it() {
    let (dir, mut project) = parts_fixture("parts-delete-inside");
    // Delete C1 (raw index 0, body position 1) — inside part 0.
    project.remove_chapter(0).unwrap();
    assert_eq!(
        project.manifest.parts.as_ref().unwrap()[0].chapters,
        [1, 1],
        "part 0 shrinks to its one surviving chapter"
    );
    assert_eq!(
        project.manifest.parts.as_ref().unwrap()[1].chapters,
        [2, 3],
        "part 1 shifts down by one so it keeps C3,C4"
    );
    assert_eq!(part_titles(&project, 0), ["C2"]);
    assert_eq!(part_titles(&project, 1), ["C3", "C4"]);
    assert_eq!(part_words(&project, 1), 7, "part 1 still sums C3+C4");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn deleting_the_only_chapter_of_a_part_drops_it() {
    let dir = temp_dir("parts-delete-drops");
    let mut project = Project::create(&dir).unwrap();
    project.manifest.chapters.clear();
    for i in 1..=3 {
        project
            .insert_chapter_with_body(i - 1, &format!("C{i}"), "word")
            .unwrap();
    }
    // Part 1 is a single chapter (C2); parts 0 and 2 flank it.
    project.manifest.parts = Some(vec![
        Part {
            name: Some("A".into()),
            chapters: [1, 1],
            goal: None,
        },
        Part {
            name: Some("B".into()),
            chapters: [2, 2],
            goal: None,
        },
        Part {
            name: Some("C".into()),
            chapters: [3, 3],
            goal: None,
        },
    ]);
    project.save_manifest().unwrap();
    // Delete C2 (raw index 1) — part 1's only chapter.
    project.remove_chapter(1).unwrap();
    let parts = project.manifest.parts.as_ref().unwrap();
    assert_eq!(
        parts.len(),
        2,
        "the emptied part is dropped, not left zero-length"
    );
    assert_eq!(
        parts[0].chapters,
        [1, 1],
        "part A unchanged (before the delete)"
    );
    assert_eq!(parts[1].chapters, [2, 2], "part C shifts down to keep C3");
    assert_eq!(part_titles(&project, 1), ["C3"]);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn split_inserts_a_chapter_and_grows_the_part() {
    // Split is an insert of the tail chapter right after the split point (#585);
    // exercise the underlying insert the app's split_chapter drives. Splitting a
    // chapter *inside* a part keeps the tail in that part — it grows.
    let (dir, mut project) = parts_fixture("parts-split");
    // Split C1 (raw index 0): its tail becomes a new chapter at raw index 1, which
    // is body position 2 — strictly inside part 0's [1,2] span.
    project
        .insert_chapter_with_body(1, "C1-tail", "tail")
        .unwrap();
    assert_eq!(
        project.manifest.parts.as_ref().unwrap()[0].chapters,
        [1, 3],
        "part 0 grows to include the split-off tail"
    );
    assert_eq!(project.manifest.parts.as_ref().unwrap()[1].chapters, [4, 5]);
    assert_eq!(part_titles(&project, 0), ["C1", "C1-tail", "C2"]);
    assert_eq!(part_titles(&project, 1), ["C3", "C4"]);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merge_removes_the_absorbed_chapter_and_shrinks_the_part() {
    // Merge appends a chapter's prose to the previous one and removes the absorbed
    // chapter (#585) — the part-facing half is exactly the remove. Exercise it.
    let (dir, mut project) = parts_fixture("parts-merge");
    // Merge C2 into C1: append then remove C2 (raw index 1, inside part 0).
    let body = project.try_chapter_body(1).unwrap();
    project.append_to_chapter(0, &body).unwrap();
    project.remove_chapter(1).unwrap();
    assert_eq!(
        project.manifest.parts.as_ref().unwrap()[0].chapters,
        [1, 1],
        "part 0 shrinks by the absorbed chapter"
    );
    assert_eq!(project.manifest.parts.as_ref().unwrap()[1].chapters, [2, 3]);
    assert_eq!(part_titles(&project, 0), ["C1"]);
    assert_eq!(part_titles(&project, 1), ["C3", "C4"]);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn move_across_a_part_boundary_is_reflected() {
    // A swap keeps every body position occupied and the body-chapter count fixed,
    // so the positional part ranges need no adjustment: a chapter moved across a
    // boundary lands in the neighbor's territory and is regrouped there. Assert the
    // grouping tracks the content after the move.
    let (dir, mut project) = parts_fixture("parts-move");
    // Move C2 (raw index 1, last of part 0) down past the boundary into C3's slot.
    project.swap_chapters(1, 2).unwrap();
    // Ranges are unchanged (the swap preserves the position set) but membership
    // follows the moved content: part 0 now groups C1,C3 and part 1 groups C2,C4.
    assert_eq!(project.manifest.parts.as_ref().unwrap()[0].chapters, [1, 2]);
    assert_eq!(project.manifest.parts.as_ref().unwrap()[1].chapters, [3, 4]);
    assert_eq!(
        part_titles(&project, 0),
        ["C1", "C3"],
        "C2 moved out of part 0 across the boundary"
    );
    assert_eq!(
        part_titles(&project, 1),
        ["C2", "C4"],
        "C2 moved into part 1 and is regrouped there"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn front_matter_insert_leaves_body_parts_untouched() {
    // Only body chapters carry part positions (#185); inserting a front-matter
    // chapter changes raw indices but not body positions, so the parts must not
    // shift. `insert_chapter_with_body` re-reads the file's cache, so a body that
    // carries a `role:` front-matter block registers as Front-band — the shift's
    // guard then leaves the parts alone.
    let (dir, mut project) = parts_fixture("parts-front-insert");
    let before = project.manifest.parts.clone();
    project
        .insert_chapter_with_body(0, "Foreword", "---\nrole: foreword\n---\n\nfront\n")
        .unwrap();
    assert_eq!(
        project.chapter_band(0),
        crate::frontmatter::Band::Front,
        "the inserted chapter is front matter"
    );
    assert_eq!(
        project.manifest.parts, before,
        "a front-matter insert must not disturb body-position part ranges"
    );
    // And the parts still group C1..C4 by content, through the new front offset.
    assert_eq!(part_titles(&project, 0), ["C1", "C2"]);
    assert_eq!(part_titles(&project, 1), ["C3", "C4"]);
    fs::remove_dir_all(&dir).ok();
}

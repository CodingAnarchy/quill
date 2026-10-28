//! Tests for the writing goals on the manifest: the `[goals]` table's parse and
//! round trip, and the splice-in-place setters — `set_goal` preserving comments
//! and clearing the optional keys, `set_deadline` writing an ISO date and
//! clearing to remove the key.

use super::*;

#[test]
fn goals_round_trip_through_the_manifest() {
    let dir = temp_dir("goals");
    let mut project = Project::create(&dir).unwrap();
    assert!(project.manifest.goals.is_none());
    assert_eq!(project.manifest.todo_marker(), "xxx"); // default

    project.manifest.start_date = Some("2026-01-11".into());
    project.manifest.goals = Some(Goals {
        words: 150_000,
        chapters: 60,
        words_per_page: 250,
        daily: None,
        deadline: None,
    });
    project.save_manifest().unwrap();

    // The `[goals]` table serializes after the scalars and before
    // `[[chapters]]` (valid TOML).
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        toml.find("[goals]").unwrap() < toml.find("[[chapters]]").unwrap(),
        "goals must precede chapters:\n{toml}"
    );

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.start_date.as_deref(), Some("2026-01-11"));
    assert_eq!(reopened.manifest.goals.unwrap().words, 150_000);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn set_goal_splices_into_goals_preserving_comments_and_clears_optional_keys() {
    // A hand-maintained manifest with a comment and another section.
    let dir = temp_dir("set-goal-splice");
    let mut project = Project::create(&dir).unwrap();
    let path = Project::manifest_path(&dir);
    fs::write(
        &path,
        "# my book\ntitle = \"Book\"\nauthor = \"Ada\"\n\n\
         [analysis]\nstopwords_add = [\"foo\"]\n\n\
         [[chapters]]\ntitle = \"Chapter 1\"\nfile = \"chapter-1.md\"\n",
    )
    .unwrap();

    // Set each goal; every write is a targeted splice that leaves the rest intact.
    project.set_goal(GoalField::Words, Some(80_000)).unwrap();
    project.set_goal(GoalField::Chapters, Some(30)).unwrap();
    project.set_goal(GoalField::Daily, Some(1_000)).unwrap();

    let toml = fs::read_to_string(&path).unwrap();
    assert!(toml.contains("# my book"), "comment preserved:\n{toml}");
    assert!(
        toml.contains("author = \"Ada\""),
        "scalar preserved:\n{toml}"
    );
    assert!(
        toml.contains("[analysis]") && toml.contains("stopwords_add = [\"foo\"]"),
        "other section preserved:\n{toml}"
    );
    assert!(
        toml.contains("words = 80000")
            && toml.contains("chapters = 30")
            && toml.contains("daily = 1000"),
        "goals spliced:\n{toml}"
    );

    // Clearing the optional daily goal removes the key, not writes a zero.
    project.set_goal(GoalField::Daily, None).unwrap();
    let toml = fs::read_to_string(&path).unwrap();
    assert!(!toml.contains("daily"), "daily key removed:\n{toml}");
    assert_eq!(project.goals().unwrap().daily, None);

    // Everything still round-trips on a fresh open.
    let reopened = Project::open(&dir).unwrap();
    let g = reopened.goals().unwrap();
    assert_eq!((g.words, g.chapters, g.daily), (80_000, 30, None));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn set_deadline_splices_an_iso_string_and_clears_to_remove_the_key() {
    // A hand-maintained manifest with a comment and another section (#814).
    let dir = temp_dir("set-deadline-splice");
    let mut project = Project::create(&dir).unwrap();
    let path = Project::manifest_path(&dir);
    fs::write(
        &path,
        "# my book\ntitle = \"Book\"\nauthor = \"Ada\"\n\n\
         [[chapters]]\ntitle = \"Chapter 1\"\nfile = \"chapter-1.md\"\n",
    )
    .unwrap();

    let deadline = crate::date::parse("2026-12-31").unwrap();
    project.set_deadline(Some(deadline)).unwrap();
    let toml = fs::read_to_string(&path).unwrap();
    // Persisted as a quoted ISO string (like `start_date`), the rest intact.
    assert!(
        toml.contains("deadline = \"2026-12-31\""),
        "deadline spliced as a quoted string:\n{toml}"
    );
    assert!(toml.contains("# my book") && toml.contains("author = \"Ada\""));

    // It round-trips: a fresh open parses the same date back.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.goals().unwrap().deadline_date(), Some(deadline));

    // Clearing removes the key rather than writing an empty string.
    project.set_deadline(None).unwrap();
    let toml = fs::read_to_string(&path).unwrap();
    assert!(!toml.contains("deadline"), "deadline key removed:\n{toml}");
    assert_eq!(project.goals().unwrap().deadline, None);

    fs::remove_dir_all(&dir).ok();
}

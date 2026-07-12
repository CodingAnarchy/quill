//! Starting a project directly in the multi-book series shape (#1135). Every
//! other creation path scaffolds a *standalone* novel; this is the "start a new
//! series, first book titled Y" front door reached from the launch chooser and
//! `quill --series`. It reuses the tested primitives — [`Project::create`]
//! scaffolds the standalone, then [`Project::convert_to_series`] hoists it into
//! `[[books]]` — so the on-disk result is byte-identical to a series built by
//! converting a standalone in-app, only reached at creation rather than after.

use std::path::Path;

use super::super::Project;

impl Project {
    /// Initialize a new project in `dir` already in the multi-book series shape
    /// (#1135): a series titled `series_title` holding one book titled
    /// `first_book_title`, with a single empty starter chapter ready to edit and
    /// that first book active.
    ///
    /// It scaffolds the ordinary standalone ([`Project::create`]), renames its
    /// lone book to `first_book_title`, then converts it to a series with
    /// `convert_to_series` — so the on-disk manifest is the identical `[[books]]`
    /// shape a normally-built (or in-app-converted) series produces, never a
    /// single-book manifest still needing conversion. The series navigator and
    /// switch-book are available immediately because `books` is non-empty.
    pub fn create_series(
        dir: impl AsRef<Path>,
        series_title: &str,
        first_book_title: &str,
    ) -> std::io::Result<Self> {
        // Scaffold the ordinary standalone first (manifest + one starter chapter),
        // then name its book. `convert_to_series` folds the live manifest into
        // book one, so the book's title must be set before the conversion.
        let mut project = Self::create(dir)?;
        project.manifest.title = first_book_title.trim().to_string();
        // Hoist the (now-named) single book into `[[books]]` and stamp the series
        // title; this persists the `[[books]]` manifest, so the on-disk shape is
        // identical to any other series with one book.
        project.convert_to_series(series_title)?;
        Ok(project)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::super::tests::temp_dir;
    use super::Project;

    /// `create_series` lands directly in the `[[books]]` series shape (#1135): the
    /// series title on the manifest, one book of the given first-book title active,
    /// and it round-trips through `Project::open` as a series (books non-empty).
    #[test]
    fn create_series_scaffolds_directly_into_series_shape() {
        let dir = temp_dir("create-series");
        let project = Project::create_series(&dir, "The Chronicles", "The First Book").unwrap();

        // A genuine series from creation: the series title on the manifest, one
        // book of the first-book title, and that book active.
        assert_eq!(project.series_title(), Some("The Chronicles"));
        assert_eq!(project.book_count(), 1);
        assert_eq!(project.active_book(), 0);
        assert_eq!(project.book_title(0), Some("The First Book"));
        // A starter chapter is ready to edit.
        assert_eq!(project.manifest.chapters.len(), 1);
        assert!(
            dir.join(&project.manifest.chapters[0].file).exists(),
            "starter chapter file created"
        );

        // The on-disk manifest is the `[[books]]` shape and reopens as a series.
        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(toml.contains("[[books]]"), "series shape:\n{toml}");
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.series_title(), Some("The Chronicles"));
        assert!(!reopened.books.is_empty(), "opens as a series");
        assert_eq!(reopened.book_title(0), Some("The First Book"));

        fs::remove_dir_all(&dir).ok();
    }

    /// The existing single-book create flow is unchanged (#1135): `Project::create`
    /// still scaffolds a *standalone* novel — no `[[books]]`, no series title —
    /// distinct from the new series front door. A guard so wiring the series
    /// creation path never accidentally makes plain "create new" a series.
    #[test]
    fn plain_create_still_yields_a_standalone() {
        let dir = temp_dir("create-standalone-guard");
        let project = Project::create(&dir).unwrap();
        assert!(project.books.is_empty(), "standalone: no series book list");
        assert_eq!(project.series_title(), None, "standalone: no series title");
        assert_eq!(project.book_count(), 1);

        let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
        assert!(
            !toml.contains("[[books]]"),
            "single-book manifest, not a series:\n{toml}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// The on-disk result is identical to converting a standalone in-app (#1135):
    /// `create_series(root, S, B)` must produce the same manifest as scaffolding a
    /// standalone titled `B` and calling `convert_to_series(S)`. The front door is
    /// pure wiring — it introduces no new series mechanics or shape.
    #[test]
    fn create_series_matches_a_converted_standalone() {
        // The front-door path: straight into series shape.
        let front = temp_dir("create-series-front");
        let front_project = Project::create_series(&front, "Saga", "Origins").unwrap();
        let front_chapter = front_project.manifest.chapters[0].file.clone();
        let front_toml = fs::read_to_string(Project::manifest_path(&front)).unwrap();

        // The equivalent in-app path: a standalone whose book is titled "Origins",
        // then converted to the "Saga" series.
        let converted = temp_dir("create-series-converted");
        let mut standalone = Project::create(&converted).unwrap();
        standalone.manifest.title = "Origins".to_string();
        standalone.convert_to_series("Saga").unwrap();
        let converted_chapter = standalone.manifest.chapters[0].file.clone();
        let converted_toml = fs::read_to_string(Project::manifest_path(&converted)).unwrap();

        // The manifests differ only in the starter chapter's file name (each project
        // derives its own from the dir name), so normalize that away: everything
        // else — series title, book title, and the `[[books]]` structure — is
        // byte-for-byte identical.
        let front_norm = front_toml.replace(front_chapter.as_str(), "chapter.md");
        let converted_norm = converted_toml.replace(converted_chapter.as_str(), "chapter.md");
        assert_eq!(
            front_norm, converted_norm,
            "the front door produces the same on-disk series shape as an in-app conversion"
        );

        fs::remove_dir_all(&front).ok();
        fs::remove_dir_all(&converted).ok();
    }
}

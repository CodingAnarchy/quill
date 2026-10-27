//! Converting a project between forms: a one-chapter novel becomes a short story
//! (#1243), and a short story grows into a novel, one chapter per scene (#1242).
//! Every conversion is confirmed first and records a restore point before it
//! changes anything — a commit plus a milestone when the project is under version
//! control, otherwise a zip in the project's backups folder — so the author can
//! always get the pre-conversion version back.

use std::fs;

use crate::archive;
use crate::config;
use crate::date;
use crate::frontmatter::{Band, SceneStatus, Status};
use crate::mode::Pending;
use crate::project::{GrownChapter, NOVELLA_WORDS};
use crate::vcs;

/// The marker that records a story was offered the novel conversion (#1242), so
/// the offer is made once per project.
const NOVELLA_OFFERED: &str = ".quill/novella-offered";

use super::App;

impl App {
    /// Start converting this novel into a short story (#1243): say why it can't,
    /// or ask for confirmation.
    pub(super) fn start_convert_to_short_story(&mut self) {
        if let Some(why) = self.project.short_story_blocker() {
            self.message = Some(format!("Can't convert to a short story: {why}."));
            return;
        }
        let title = self.project.manifest.title.clone();
        self.open_prompt(
            Pending::ConfirmToShortStory,
            format!(
                "Convert '{title}' to a short story? Its chapter becomes the story{}. (y/N)",
                self.sections_note()
            ),
            "",
            None,
        );
    }

    /// Convert to a short story after the author confirmed (#1243): flush,
    /// record a restore point, then change the form.
    pub(super) fn convert_to_short_story_confirmed(&mut self) {
        let restore =
            match self.record_restore_point("short-story", "Before converting to a short story") {
                Ok(restore) => restore,
                Err(e) => {
                    self.message = Some(format!("Not converted: {e}"));
                    return;
                }
            };
        match self.project.convert_to_short_story() {
            Ok(()) => {
                self.message = Some(format!(
                    "Converted to a short story{}. Restore point: {restore}.",
                    self.sections_note()
                ));
            }
            Err(e) => self.message = Some(format!("Not converted: {e}")),
        }
    }

    /// How front and back matter will appear in the story, for the confirm and
    /// the result message — empty when there is none.
    fn sections_note(&self) -> &'static str {
        let sections = (0..self.project.manifest.chapters.len())
            .filter(|&i| self.project.chapter_band(i) != Band::Body)
            .count();
        if sections == 0 {
            ""
        } else {
            "; its front and back matter stay before and after the story"
        }
    }

    /// Record a restore point before a conversion (#1243) and say where it is.
    /// The active chapter is saved first. Under version control, everything is
    /// committed and tagged as milestone `pre-<label>-<stamp>`; otherwise the
    /// project is archived to `pre-<label>-<stamp>.zip` in its backups folder.
    /// Errors (and the caller converts nothing) when neither can be written.
    pub(super) fn record_restore_point(
        &mut self,
        label: &str,
        note: &str,
    ) -> Result<String, String> {
        if self.flush_active().is_err() {
            return Err("the open chapter couldn't be saved".into());
        }
        let root = self.project.root.clone();
        let name = format!("pre-{label}-{}", date::file_stamp());
        if vcs::is_repo(&root) {
            vcs::commit(&root, note)?;
            vcs::create_tag(&root, &name, note)?;
            return Ok(format!("milestone {name}"));
        }
        let dir = config::backups_dir(&root)
            .ok_or_else(|| "there's no backups folder to save a restore point in".to_string())?;
        let dest = dir.join(format!("{name}.zip"));
        archive::write_archive(&root, &dest)
            .map_err(|e| format!("couldn't write a restore point: {e}"))?;
        Ok(format!("backup {}", dest.display()))
    }

    /// Start growing this short story into a novel (#1242): focus the story,
    /// then ask, naming how many chapters its scenes become.
    pub(super) fn start_grow_into_novel(&mut self) {
        let Some(story) = self.project.manifest.story_chapter() else {
            self.message = Some("Only a short story grows into a novel.".into());
            return;
        };
        if self.project.active_index() != story {
            self.activate_chapter(story);
        }
        // A reference pane in focus would be split instead of the story.
        if !self.editing_story() {
            self.message = Some("Focus the story (Ctrl-w w) to grow it into a novel.".into());
            return;
        }
        let scenes = self.story_scenes_now().1.len();
        if scenes == 0 {
            self.message = Some("The story has no prose yet to turn into chapters.".into());
            return;
        }
        let title = self.project.manifest.title.clone();
        let chapters = if scenes == 1 {
            "1 chapter".to_string()
        } else {
            format!("{scenes} chapters")
        };
        self.open_prompt(
            Pending::ConfirmToNovel,
            format!("Grow '{title}' into a novel? Its scenes become {chapters}. (y/N)"),
            "",
            None,
        );
    }

    /// Grow the story into a novel after the author confirmed (#1242): record a
    /// restore point, split the story at its scenes, and reopen the project.
    pub(super) fn grow_into_novel_confirmed(&mut self) {
        let restore = match self.record_restore_point("novel", "Before growing into a novel") {
            Ok(restore) => restore,
            Err(e) => {
                self.message = Some(format!("Not converted: {e}"));
                return;
            }
        };
        let chapters = self.grown_chapters();
        let count = chapters.len();
        if let Err(e) = self.project.grow_into_novel(&chapters) {
            self.message = Some(format!("Not converted: {e}. Restore point: {restore}."));
            return;
        }
        if let Err(e) = self.reload_project_after_vcs() {
            self.message = Some(format!("Converted, but reopening failed: {e}"));
            return;
        }
        self.message = Some(format!(
            "Grew into a novel of {count} chapter{}. Restore point: {restore}.",
            if count == 1 { "" } else { "s" }
        ));
    }

    /// The chapters the focused story's scenes become (#1242). Each runs from its
    /// scene's start (the first from the top, keeping a title line) through its
    /// prose; the breaks between scenes are dropped, since a chapter boundary
    /// replaces them. A headed scene is titled by its heading, an untitled one
    /// "Chapter N"; its synopsis becomes the chapter summary and its status the
    /// chapter status (done → final, draft → draft, stub → none).
    fn grown_chapters(&self) -> Vec<GrownChapter> {
        let (text, scenes) = self.story_scenes_now();
        let meta = self.chapter_scenes(scenes.len());
        scenes
            .iter()
            .zip(meta)
            .enumerate()
            .map(|(i, (scene, meta))| {
                let from = if i == 0 { 0 } else { scene.start };
                let mut body = text[from..scene.end].trim_end().to_string();
                body.push('\n');
                GrownChapter {
                    title: scene
                        .title
                        .clone()
                        .unwrap_or_else(|| format!("Chapter {}", i + 1)),
                    body,
                    summary: meta.synopsis,
                    status: match meta.status {
                        SceneStatus::Done => Some(Status::Final),
                        SceneStatus::Draft => Some(Status::Draft),
                        SceneStatus::Stub => None,
                    },
                    from,
                }
            })
            .collect()
    }

    /// Offer once, on the idle tick, to grow a short story that has passed novella
    /// length into a novel (#1242). A status-line note rather than a prompt, so it
    /// never interrupts typing; a marker in `.quill/` keeps it from repeating.
    pub(super) fn offer_novel_growth(&mut self) {
        if self.novella_checked || !self.editing_story() {
            return;
        }
        if self.buffer.word_count() < NOVELLA_WORDS {
            return;
        }
        self.novella_checked = true;
        let marker = self.project.root.join(NOVELLA_OFFERED);
        if marker.exists() {
            return;
        }
        if let Some(dir) = marker.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&marker, date::iso(date::today()));
        self.message = Some(
            "This story has reached novella length. If it wants to be a novel, run \
             \"Grow into a novel\" (Ctrl-K)."
                .into(),
        );
    }
}

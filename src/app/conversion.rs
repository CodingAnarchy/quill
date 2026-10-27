//! Converting a project between forms (#1243): a one-chapter novel becomes a
//! short story. Every conversion is confirmed first and records a restore point
//! before it changes anything — a commit plus a milestone when the project is
//! under version control, otherwise a zip in the project's backups folder — so
//! the author can always get the pre-conversion version back.

use crate::archive;
use crate::config;
use crate::date;
use crate::frontmatter::Band;
use crate::mode::Pending;
use crate::vcs;

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
}

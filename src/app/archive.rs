//! Offline project archive flow (#594): the run-actions that write a timestamped
//! `.zip` backup of the whole project and restore one into a fresh folder. The
//! zip/unzip work lives in [`crate::archive`]; this wires it to the prompt state
//! machine, mirroring the two-step new-project pattern (source, then destination).

use std::path::PathBuf;

use crate::archive;
use crate::mode::Pending;

use super::App;

impl App {
    // --- Archive the open project (#594) ----------------------------------

    /// Prompt for where to write the project's archive, prefilled with the
    /// projects-home (or the project's parent) and a default timestamped name so
    /// the author usually just presses Enter.
    pub(crate) fn start_archive_project(&mut self) {
        let base = self.new_project_base();
        let prefill = base.join(archive::default_name(&self.project.root));
        self.open_prompt(
            Pending::ArchiveProjectDest,
            "Archive project — .zip path:",
            &prefill.to_string_lossy(),
            None,
        );
    }

    /// Write the archive to `dest`, flushing unsaved work first so the backup
    /// reflects what's on screen. A `.zip` suffix is added if the author left it
    /// off. Restores the editing mode and reports the result either way.
    pub(crate) fn finish_archive_project(&mut self, dest: &str) {
        let dest = dest.trim();
        if dest.is_empty() {
            self.mode = self.editing_mode();
            self.message = Some("Archive cancelled — no path.".into());
            return;
        }
        // The archive reads files from disk, so save the active chapter first.
        if self.flush_active().is_err() {
            return; // save failed — message already set
        }
        let mut path = PathBuf::from(dest);
        if path.extension().is_none() {
            path.set_extension("zip");
        }
        let root = self.project.root.clone();
        self.mode = self.editing_mode();
        match archive::write_archive(&root, &path) {
            Ok(n) => {
                self.message = Some(format!("Archived {n} files to {}", path.display()));
            }
            Err(e) => self.message = Some(format!("Archive failed: {e}")),
        }
    }

    // --- Restore a project from an archive (#594) -------------------------

    /// Prompt for the `.zip` archive to restore a project from.
    pub(crate) fn start_restore_archive(&mut self) {
        self.open_prompt(
            Pending::RestoreArchiveSource,
            "Restore from archive — .zip path:",
            "",
            None,
        );
    }

    /// Validate `src` is a readable archive file, then ask where to unpack it.
    pub(crate) fn begin_restore_archive(&mut self, src: &str) {
        let path = PathBuf::from(src.trim());
        if !path.is_file() {
            self.mode = self.editing_mode();
            self.message = Some(format!("Not an archive file: {}", path.display()));
            return;
        }
        // Prefill the destination with the projects-home and the archive's stem,
        // so the restored folder defaults to the backup's name.
        let stem = path
            .file_stem()
            .map_or_else(|| "restored".into(), |s| s.to_string_lossy().into_owned());
        let prefill = format!("{}/{stem}", self.new_project_base().display());
        self.pending_archive_source = Some(path);
        self.open_prompt(
            Pending::RestoreArchiveDest,
            "Restore into — new project folder:",
            &prefill,
            None,
        );
    }

    /// Unpack the archive chosen earlier into `dest` and open it. Refuses to
    /// clobber an existing project (checked in [`archive::restore_archive`]) and
    /// reports on failure.
    pub(crate) fn finish_restore_archive(&mut self, dest: &str) {
        let Some(src) = self.pending_archive_source.take() else {
            self.mode = self.editing_mode();
            return;
        };
        let dest = PathBuf::from(dest.trim());
        if dest.as_os_str().is_empty() {
            self.mode = self.editing_mode();
            self.message = Some("Restore cancelled — no folder.".into());
            return;
        }
        match archive::restore_archive(&src, &dest) {
            Ok(n) => {
                // Open the restored project fresh from disk, like a switch; the
                // "Opened …" message it sets is replaced with the restore count
                // once the switch lands.
                self.switch_to(dest.clone());
                if self.project.root == dest {
                    self.message = Some(format!("Restored {n} files into {}", dest.display()));
                }
            }
            Err(e) => {
                self.mode = self.editing_mode();
                self.message = Some(format!("Restore failed: {e}"));
            }
        }
    }
}

//! Project lifecycle: creating, opening, and switching projects, plus renaming
//! the open one in place. Extracted from `mod.rs` (#314) and, once it outgrew a
//! single file (#760), split into cohesive submodules — [`browser`] (the project
//! and directory browsers, the type-to-filter sub-mode, and the browser's
//! rename / delete) and [`series`] (the book switcher and in-app series
//! management). The create / open / switch lifecycle stays here; every method is
//! `impl App`, and the submodules are children of this module, so callers
//! elsewhere in `app` are unaffected by the split.

use std::path::{Path, PathBuf};

use crate::buffer::Buffer;
use crate::config::ProjectsConfig;
use crate::mode::{Command, PaletteItem, Pending};
use crate::project::{Project, ProjectTemplate, TEMPLATES};
use crate::recent;

use super::App;

mod browser;
mod series;

impl App {
    /// The default location for a new project: the configured projects-home
    /// (#272), else beside the current project.
    pub(crate) fn new_project_base(&self) -> PathBuf {
        ProjectsConfig::load()
            .home_dir()
            .filter(|p| p.is_dir())
            .or_else(|| {
                self.project
                    .root
                    .canonicalize()
                    .ok()
                    .and_then(|r| r.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Prompt for a new project's path, prefilled with `base` and a trailing
    /// separator so the author just types a name (and can edit the location).
    #[allow(clippy::needless_pass_by_value)] // callers (browser.rs, tests) hand over an owned base path
    pub(crate) fn prompt_new_project(&mut self, base: PathBuf) {
        let prefill = format!("{}/", base.display());
        self.open_prompt(Pending::NewProject, "New project — path: ", &prefill, None);
    }

    /// Offer a structure template for the new project at `target` (#611), then
    /// create it. Refuses to clobber an existing project up front (so a bad path
    /// reports before the picker), stashes the target, and opens a small picker of
    /// the curated [`TEMPLATES`]; the selection routes to
    /// [`Self::create_project_with_template`].
    pub(crate) fn create_project_at(&mut self, target: PathBuf) {
        if Project::manifest_path(&target).exists() {
            self.message = Some(format!("A project already exists at {}", target.display()));
            self.mode = self.editing_mode();
            return;
        }
        self.pending_manage = Some(target);
        let items = TEMPLATES
            .iter()
            .enumerate()
            .map(|(i, t)| PaletteItem::new(t.name, t.desc, Command::NewProjectTemplate(i)))
            .collect();
        self.open_palette("New project — structure", items);
    }

    /// Create the stashed new project (#611) with the template at `index`, then
    /// open it in place. Offers the setup wizard only when the project was
    /// actually created and opened (#282), and the wizard isn't disabled.
    pub(crate) fn create_project_with_template(&mut self, index: usize) {
        let Some(target) = self.pending_manage.take() else {
            self.mode = self.editing_mode();
            return;
        };
        let template = ProjectTemplate::at(index);
        if self.create_and_open_with_template(&target, template)
            && ProjectsConfig::load().wizard_enabled()
        {
            self.start_wizard();
        }
    }

    /// Create a new empty project at `target` and switch to it in place. Returns
    /// whether the switch landed (false if a project already exists there,
    /// creation fails, or a save aborts the switch), with a clear message set on
    /// failure. Shared by "new project from notes" (#286) and manuscript import
    /// (#298), which scaffold their own content and so want the empty default.
    pub(crate) fn create_and_open(&mut self, target: &Path) -> bool {
        self.create_and_open_with_template(target, ProjectTemplate::at(0))
    }

    /// Create a new project at `target` scaffolded from `template` (#611) and
    /// switch to it in place. Returns whether the switch landed (false if a
    /// project already exists there, creation fails, or a save aborts the switch),
    /// with a clear message set on failure.
    fn create_and_open_with_template(&mut self, target: &Path, template: &ProjectTemplate) -> bool {
        if Project::manifest_path(target).exists() {
            self.message = Some(format!("A project already exists at {}", target.display()));
            self.mode = self.editing_mode();
            return false;
        }
        match Project::create_with_template(target, template) {
            // switch_to saves the current project, opens the new one, and records
            // it in recents — the same path a normal switch takes.
            Ok(_) => {
                self.switch_to(target.to_path_buf());
                self.project.root == *target
            }
            Err(e) => {
                self.message = Some(format!("Couldn't create project: {e}"));
                self.mode = self.editing_mode();
                false
            }
        }
    }

    /// Switch to `target`, guarding unsaved work first: if the buffer is dirty,
    /// confirm a save-and-switch; if `target` is already open, say so; otherwise
    /// switch straight away.
    pub(crate) fn request_switch(&mut self, target: PathBuf) {
        let target = target.canonicalize().unwrap_or(target);
        let current = self
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.project.root.clone());
        if target == current {
            self.message = Some("That project is already open.".into());
            self.mode = self.editing_mode();
            return;
        }
        if self.edit_dirty() {
            self.pending_switch = Some(target);
            self.open_prompt(
                Pending::ConfirmSwitch,
                "Unsaved changes — save and switch project? (y/N)",
                "",
                None,
            );
        } else {
            self.switch_to(target);
        }
    }

    /// Save and finalize the current project, then load `target` in place — the
    /// running editor adopts the new project wholesale, with no state carried
    /// over. A save failure aborts the switch (the current project stays put);
    /// an open failure leaves the current project open with a message.
    #[allow(clippy::needless_pass_by_value)] // callers (prompts.rs, imports.rs, archive.rs, tests) hand over an owned path
    pub(crate) fn switch_to(&mut self, target: PathBuf) {
        if self.finalize_current_project().is_err() {
            return; // save failed — message set; don't risk the unsaved work
        }
        self.open_in_place(&target);
    }

    /// Save and finalize the current project before adopting another in its place:
    /// flush the continuous window back to its chapters and the active chapter to
    /// disk, then drop the recovery sidecar. Returns `Err` (with a message set) if
    /// the save fails, so the caller aborts rather than risk unsaved work. Shared
    /// by switching projects (#270) and renaming the open one (#424).
    fn finalize_current_project(&mut self) -> Result<(), ()> {
        self.leave_continuous();
        self.flush_active()?;
        self.clear_recovery();
        Ok(())
    }

    /// Adopt the project at `target` wholesale — a fresh [`App`] with no state
    /// carried over — assuming the current project's work is already saved and
    /// finalized. Shared by switching projects (#270) and renaming the open one
    /// (#424), which both save first and then repoint here.
    fn open_in_place(&mut self, target: &Path) {
        let opened = Project::open(target).and_then(|project| {
            let buffer = Buffer::from_path(project.active_path())?;
            Ok((project, buffer))
        });
        match opened {
            Ok((project, buffer)) => {
                let root = project.root.clone();
                let title = project.manifest.title.clone();
                // A fresh App resets every per-project field (buffer, manuscript,
                // splits, tally baselines, caches, VC status) in one move.
                *self = Self::new(buffer, project);
                self.load_history();
                recent::record(&root, &title);
                // Preserve the read-only warning `Self::new` set (#871) rather than
                // overwriting it with "Opened …" — the author needs to know saves
                // are disabled. A normally-owned project shows the open confirmation.
                if !self.project.is_read_only() {
                    self.message = Some(format!("Opened \u{201c}{title}\u{201d}"));
                }
                self.offer_recovery(); // honor the incoming project's unsaved work, if any
            }
            Err(e) => {
                self.message = Some(format!("Couldn't open project: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Prompt for the open project's new name (#424), prefilled with the book
    /// title. Renaming moves the folder and updates the book title together
    /// (#690), so the folder and the title stay in sync.
    pub(crate) fn prompt_rename_project(&mut self) {
        let prefill = self.project.manifest.title.clone();
        self.open_prompt(
            Pending::RenameProject,
            "Rename project (folder and book title): ",
            &prefill,
            None,
        );
    }

    /// Rename the open project to `new_name`, in place: save the current work,
    /// move the folder, sync the book title to match (#690), and reopen the project
    /// at the new path so every save from here lands there (#424). Refuses to
    /// clobber an existing folder, and leaves the project untouched on any failure
    /// (no half-move).
    pub(crate) fn rename_project(&mut self, new_name: &str) {
        let name = crate::project::sanitize_dir_name(new_name);
        if name.is_empty() {
            self.message = Some("Enter a folder name.".into());
            self.mode = self.editing_mode();
            return;
        }
        // Resolve against the real location so the parent is correct even when the
        // project was opened by a relative path.
        let old_root = self
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.project.root.clone());
        let Some(new_root) = old_root.parent().map(|p| p.join(&name)) else {
            self.message = Some("Can't rename the filesystem root.".into());
            self.mode = self.editing_mode();
            return;
        };
        if new_root == old_root {
            self.message = Some(format!("Project is already named \u{201c}{name}\u{201d}."));
            self.mode = self.editing_mode();
            return;
        }
        if new_root.exists() {
            self.message = Some(format!(
                "A folder named \u{201c}{name}\u{201d} already exists here."
            ));
            self.mode = self.editing_mode();
            return;
        }
        // Save and finalize the current project at the OLD location before moving,
        // so nothing writes back to a path that's about to disappear.
        if self.finalize_current_project().is_err() {
            return; // save failed — message set; don't risk the move
        }
        if let Err(e) = std::fs::rename(&old_root, &new_root) {
            // The project is still intact at the old location; just report.
            self.message = Some(format!("Couldn't rename project: {e}"));
            self.mode = self.editing_mode();
            return;
        }
        // The folder moved; now bring the book title into sync so the folder and
        // the title (report, exports, recents) don't diverge (#690). Write it onto
        // the manifest at the new location before reopening, since open_in_place
        // adopts whatever the manifest on disk says.
        if let Err(e) = set_manifest_title(&new_root, &name) {
            // The folder is at the new path but the title didn't update — report,
            // then still reopen at the new path so the editor follows the folder.
            self.message = Some(format!(
                "Renamed folder, but couldn't update the title: {e}"
            ));
            self.open_in_place(&new_root);
            return;
        }
        // Reopen at the new path (the old recents entry self-prunes — its folder is
        // gone). open_in_place sets an "Opened …" message; replace it with the
        // rename confirmation.
        self.open_in_place(&new_root);
        if self.project.root == new_root {
            self.message = Some(format!("Renamed project to \u{201c}{name}\u{201d}."));
        }
    }
}

/// Set the book title in the manifest of the project at `root` to `title` and
/// write it back (#690), so renaming a project keeps its folder and its title in
/// sync. Loads the manifest first, so a multi-book series (`[[books]]`) round-trips
/// through the series form rather than being flattened. `title` is the sanitized
/// folder name, keeping the two identical. Shared by the in-place rename and the
/// browser's rename (in [`browser`]).
fn set_manifest_title(root: &Path, title: &str) -> std::io::Result<()> {
    let mut project = Project::open(root)?;
    project.manifest.title = title.to_string();
    project.save_manifest()
}

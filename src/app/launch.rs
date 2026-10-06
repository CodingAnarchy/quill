//! The launch sequence: open (or create) the project, load the chapter the
//! author was last in, build the [`App`], and run the on-open offers (crash
//! recovery, damage restore, pass-queue resume). Factored out of `main` so the
//! real sequence is testable and timed by the `startup_is_fast_for_a_series`
//! perf guard.
//!
//! Every step reads what it needs once. The remembered position (#588) is read
//! *before* the first buffer is built, so the launch loads that chapter directly
//! rather than loading chapter 0 and then switching away from it; the damage
//! check (#934) reads the record the open's cache pass left, rather than opening
//! the project a second time; and the config sections share one parse of each
//! file ([`LoadSession`]).

use std::io;
use std::path::Path;

use crate::buffer::Buffer;
use crate::config::LoadSession;
use crate::lastpos::{self, Position};
use crate::project::Project;

use super::App;

/// A project opened for launch, with its first buffer already loaded.
pub struct Loaded {
    pub project: Project,
    /// The active chapter's buffer — the remembered chapter when it loaded.
    pub buffer: Buffer,
    /// The remembered last position (#588), if a usable record was on disk.
    pub position: Option<Position>,
    /// Why the remembered chapter couldn't be loaded, when the launch had to fall
    /// back to the first chapter instead (shown as the launch message).
    pub fallback: Option<String>,
}

/// Open the project at `dir` — opening an existing one (a directory with a
/// manifest) or initializing a new one in an empty directory — and load the
/// chapter the author was last in (#588), falling back to the first chapter when
/// there's no record, the remembered chapter is gone (deleted, renamed, or in
/// another book of a series), or it won't read. Every foreseeable failure (a
/// malformed manifest, an unreadable first chapter, a directory that can't be
/// created or written) comes back as an `Err` carrying a clear message, never a
/// panic (#264).
pub fn load_project(dir: &Path) -> io::Result<Loaded> {
    // Recover from a crash mid backup-restore swap (#1179) before deciding
    // open-vs-create: if a previous restore was interrupted between its two renames,
    // the real project is stranded in a hidden sibling and `dir` is gone — move it
    // back, or the missing `dir` below would scaffold a fresh empty project over the
    // author's book. A cheap no-op on every normal launch.
    App::reconcile_interrupted_restore(dir);
    let mut project = if Project::manifest_path(dir).exists() {
        Project::open(dir)?
    } else {
        Project::create(dir)?
    };
    let position = lastpos::read(&project.root);
    let first = project.active_index();
    let mut fallback = None;
    let remembered = position
        .as_ref()
        .and_then(|pos| project.index_of_file(&pos.chapter_file));
    if let Some(index) = remembered.filter(|&i| i != first) {
        project.set_active(index);
        match Buffer::from_path(project.active_path()) {
            Ok(buffer) => {
                return Ok(Loaded {
                    project,
                    buffer,
                    position,
                    fallback,
                });
            }
            // Unreadable: start on the first chapter, as a failed switch would.
            Err(e) => {
                fallback = Some(format!("Open failed: {e}"));
                project.set_active(first);
            }
        }
    }
    let buffer = Buffer::from_path(project.active_path())?;
    Ok(Loaded {
        project,
        buffer,
        position,
        fallback,
    })
}

impl App {
    /// Run the launch sequence for the project at `dir` and hand back the ready
    /// editor: open it ([`load_project`]), build the [`App`], resume today's
    /// counters and the last cursor position, then make the on-open offers in
    /// precedence order — unsaved work from a crash first, then a restore when the
    /// project looks damaged, then an interrupted pass queue. Fails only when the
    /// project can't be opened at all.
    pub fn launch(dir: &Path) -> io::Result<Self> {
        // Opening the project and building the editor load a dozen config
        // sections; parse the config and manifest once for all of them.
        let _config_session = LoadSession::begin();
        let Loaded {
            project,
            buffer,
            position,
            fallback,
        } = load_project(dir)?;
        let mut app = Self::new(buffer, project);
        // Resume today's word-delta counters from the history log.
        app.load_history();
        // Resume where the last clean session left off — active chapter (already
        // loaded above) + cursor (#588). Before offering recovery, so a crash
        // sidecar still targets its own chapter and isn't overridden by the resume.
        app.resume_cursor(position);
        if let Some(note) = fallback {
            // Don't bury a startup warning (read-only, bad config) under it.
            app.message.get_or_insert(note);
        }
        // If a previous session left unsaved work behind (a crash), offer it back.
        app.offer_recovery();
        // If the project itself looks damaged (missing / unreadable chapter files),
        // offer to restore from the latest local backup (#934). After recovery,
        // which takes precedence — it's a no-op while that prompt is up.
        app.offer_backup_restore_on_damage();
        // If a durable pass queue was interrupted (a crash mid-run), offer to resume
        // it (#923) — after the recovery/backup offers, which take precedence.
        app.offer_pass_queue_resume();
        Ok(app)
    }
}

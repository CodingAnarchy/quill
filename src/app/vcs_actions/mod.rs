//! Version-control commands (#54): the Git palette dispatch and the
//! init / commit / history / diff / restore lifecycle plus remote push & pull.
//! Split out of `app/mod.rs` (#150); once it outgrew a single file (#925), split
//! by concern into cohesive submodules, each `impl App` and reached through
//! `use super::super::*`:
//!
//! - [`status`] — the cached uncommitted-changes count and its #779 idle
//!   debounce (with the #869 backoff), plus the `#[cfg(test)]` settle helpers.
//! - [`commit`] — `init` and the commit lifecycle: the pre-commit review (#704)
//!   and the commit itself.
//! - [`remote`] — the remote push / pull worker (#134), its `poll_vc` landing
//!   path (#870), and the remote URL prompt.
//! - [`sync`] — the two-way sync flow (#962): fetch + reconcile the remote's
//!   chapter edits with local, off-thread on the shared `vc_task`, exposing the
//!   per-chapter conflicts it detects for the resolution UI (#963).
//! - [`drafts`] — branch-as-draft (#965): the drafts overlay and its create /
//!   switch / list / merge flow, a merge with conflicts routing through the same
//!   resolution UI (#963).
//! - [`history`] — the navigable commit-history view (#587) and its
//!   type-to-filter sub-mode (#822).
//! - [`restore`] — milestones (#833), the working-tree / project diffs, and the
//!   chapter / whole-project restores.
//!
//! Shared here at the module root: the [`VcMsg`] worker result, the palette
//! dispatch, and the [`diff_lines`] colorizer / [`reload_active_chapter`] helper
//! the submodules reuse. The public surface is re-exported so callers elsewhere
//! in `app` are unaffected by the split.

use super::*;

mod commit;
mod drafts;
mod history;
mod remote;
mod restore;
mod status;
mod sync;
mod word_diff;

pub use status::VC_STATUS_INTERVAL;

/// The result of a remote push / pull / sync (#134, #962), sent back from the
/// worker thread to the UI thread. `reload` asks the UI to re-read the active
/// chapter, since a pull or a clean sync can change files on disk. `conflicts`
/// carries the per-chapter conflicts a two-way sync detected (#962) — empty for a
/// push, pull, or clean sync — so [`poll_vc`](App::poll_vc) can stash them for the
/// resolution UI (#963) without either side being clobbered.
pub struct VcMsg {
    text: String,
    reload: bool,
    conflicts: Vec<crate::vcs::ChapterConflict>,
}

impl VcMsg {
    /// A push/pull result with no sync conflicts — the common shape for the ops that
    /// predate two-way sync, so their call sites stay a two-field literal.
    const fn plain(text: String, reload: bool) -> Self {
        Self {
            text,
            reload,
            conflicts: Vec::new(),
        }
    }
}

impl App {
    /// Open the version-control palette (#54) — the single entry point for Git
    /// actions, shown with context-appropriate items.
    pub(crate) fn open_git_palette(&mut self) {
        let git = |label: &str, desc: &str, a| PaletteItem::new(label, desc, Command::Git(a));
        let items = if self.under_vc {
            let has_remote = crate::vcs::remote_url(&self.project.root).is_some();
            let mut items = vec![git(
                "Commit",
                "Snapshot the project with a message",
                GitAction::Commit,
            )];
            // Sync actions are only meaningful once a remote is configured.
            if has_remote {
                items.push(git(
                    "Push",
                    "Back up your commits to the remote",
                    GitAction::Push,
                ));
                items.push(git(
                    "Pull",
                    "Bring down the remote's latest changes",
                    GitAction::Pull,
                ));
                items.push(git(
                    "Sync",
                    "Two-way sync: merge the remote's chapter edits with yours",
                    GitAction::Sync,
                ));
            }
            items.push(git(
                "Drafts",
                "Alternate versions (alt ending, POV split) as branches",
                GitAction::Drafts,
            ));
            items.push(git(
                "Milestone",
                "Mark a named manuscript version (e.g. “first draft”)",
                GitAction::Milestone,
            ));
            items.push(git(
                "History",
                "Browse past commits and milestones",
                GitAction::History,
            ));
            items.push(git(
                "Changes",
                "Diff the working tree against the last commit",
                GitAction::Diff,
            ));
            items.push(git(
                "Project changes",
                "Diff the whole manuscript against the last commit",
                GitAction::DiffProject,
            ));
            items.push(git(
                "Restore chapter",
                "Revert the current chapter to its last commit",
                GitAction::RestoreChapter,
            ));
            items.push(git(
                "Restore project",
                "Revert the whole manuscript to its last commit",
                GitAction::RestoreProject,
            ));
            items.push(if has_remote {
                git(
                    "Change remote",
                    "Point backups at a different remote",
                    GitAction::SetRemote,
                )
            } else {
                git(
                    "Set remote",
                    "Back the project up to a hosted remote",
                    GitAction::SetRemote,
                )
            });
            items
        } else {
            vec![git(
                "Initialize",
                "Put this project under version control",
                GitAction::Initialize,
            )]
        };
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Version control",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Dispatch a version-control action chosen from the palette.
    pub(crate) fn start_git_action(&mut self, action: GitAction) {
        match action {
            GitAction::Initialize => self.init_version_control(),
            GitAction::Commit => self.start_commit(),
            GitAction::History => self.open_history(),
            GitAction::Diff => self.show_diff(),
            GitAction::DiffProject => self.show_project_diff(),
            GitAction::RestoreChapter => self.start_restore(),
            GitAction::RestoreProject => self.start_restore_project(None),
            GitAction::Milestone => self.start_milestone(),
            GitAction::SetRemote => self.start_set_remote(),
            GitAction::Push => self.start_push(),
            GitAction::Pull => self.start_pull(),
            GitAction::Sync => self.start_sync(),
            GitAction::Drafts => self.open_drafts(),
        }
    }

    /// Put the project under version control, or report that it already is (#130).
    /// Opt-in and non-destructive: it never clobbers an existing repo or
    /// `.gitignore`.
    fn init_version_control(&mut self) {
        if self.under_vc {
            self.message = Some("Already under version control.".into());
            return;
        }
        match crate::vcs::init(&self.project.root) {
            Ok(()) => {
                self.under_vc = true;
                self.refresh_vc_status();
                self.message = Some("Initialized version control (git).".into());
            }
            Err(e) => self.message = Some(format!("Version control: {e}")),
        }
    }

    /// Re-read the active chapter from disk into the buffer, after something (a
    /// pull, a restore) rewrote the file underneath the editor. Returns the read
    /// error so callers can report it; on failure the current buffer is left in
    /// place.
    fn reload_active_chapter(&mut self) -> Result<(), String> {
        let path = self.project.active_path();
        let buffer = Buffer::from_path(&path).map_err(|e| e.to_string())?;
        self.buffer = buffer;
        self.cache_focus_words();
        Ok(())
    }

    /// Reload the whole project from disk after a VCS *landing* op that can rewrite
    /// the tracked `book.toml` — a pull, a two-way sync, a draft switch/merge, or a
    /// restore. Unlike [`reload_active_chapter`](App::reload_active_chapter), which
    /// only re-reads a single buffer from the *current* `active_path`, this re-opens
    /// the project so the in-memory manifest, the per-chapter caches (word / TODO
    /// counts, front-matter metadata), and the book/series structure all reflect the
    /// new on-disk state: the op may have added, removed, renamed, or reordered
    /// chapters, and after a force-checkout the disk is authoritative.
    ///
    /// The previously-active chapter is kept active when it still exists in the new
    /// manifest; otherwise [`Project::open`] has already selected the first chapter,
    /// so the reloaded buffer is always a real chapter — never an orphan pointing at
    /// a now-deleted file. Returns the error so callers can report it; on failure the
    /// current project and buffer are left in place, exactly as before.
    fn reload_project_after_vcs(&mut self) -> Result<(), String> {
        let prev_active = self.project.active_file().to_string();
        let mut project = Project::open(&self.project.root).map_err(|e| e.to_string())?;
        // Stay on the same chapter when the op left it in place; otherwise `open`
        // already made the first chapter active, so the active buffer stays valid.
        if let Some(index) = project.index_of_file(&prev_active) {
            project.set_active(index);
        }
        self.project = project;
        self.reload_active_chapter()
    }

    /// The active chapter's path relative to the repo root — the file name, since
    /// chapters live at the project root. The handle the per-file version-control
    /// operations (restore, diff/restore-from-a-commit) address.
    fn active_chapter_rel(&self) -> String {
        self.project
            .active_path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// Color a unified diff for the changes overlay (#135): additions green, removals
/// red, file/hunk headers and context dimmed. Mirrors the bible-review diff.
///
/// Within a hunk, a run of removed lines followed by a run of added ones is a
/// replacement; each removed line is paired positionally with the added line that
/// replaced it and word-diffed (#1013), so the shared words read as dim context
/// and only the changed words carry the removed / added color. This is purely
/// additive — a line with no counterpart, or a full rewrite with nothing in
/// common, still renders as a plain whole-line removal / addition.
fn diff_lines(patch: &str) -> Vec<Line<'static>> {
    let theme = crate::theme::current();
    let plain = |l: &str| {
        let color = match l.chars().next() {
            Some('+') => theme.added,
            Some('-') => theme.removed,
            _ => theme.dim,
        };
        Line::from(Span::styled(l.to_string(), Style::default().fg(color)))
    };

    let raw: Vec<&str> = patch.lines().collect();
    let mut out: Vec<Line<'static>> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        // A `-`/`+` body pair (never the `---`/`+++` file headers) is a candidate
        // replacement block. Collect the removed run and the added run that follows
        // it, then pair them line-for-line for the intra-line word diff.
        if is_removed(raw[i]) {
            let del_start = i;
            while i < raw.len() && is_removed(raw[i]) {
                i += 1;
            }
            let dels = &raw[del_start..i];
            let add_start = i;
            while i < raw.len() && is_added(raw[i]) {
                i += 1;
            }
            let adds = &raw[add_start..i];
            emit_replacement(&mut out, dels, adds, &plain, theme);
            continue;
        }
        out.push(plain(raw[i]));
        i += 1;
    }
    out
}

/// A removed content line (`-…`) but not the `---` old-file header.
fn is_removed(line: &str) -> bool {
    line.starts_with('-') && !line.starts_with("---")
}

/// An added content line (`+…`) but not the `+++` new-file header.
fn is_added(line: &str) -> bool {
    line.starts_with('+') && !line.starts_with("+++")
}

/// Render a removed run and the added run that replaced it. Rows that pair up
/// (same index in each run) and share at least one word are word-diffed (#1013);
/// unmatched rows, or pairs with nothing in common, fall back to `plain` whole-
/// line coloring. Emits all removed lines first, then all added lines, preserving
/// the unified-diff order.
fn emit_replacement(
    out: &mut Vec<Line<'static>>,
    dels: &[&str],
    adds: &[&str],
    plain: &impl Fn(&str) -> Line<'static>,
    theme: crate::theme::Theme,
) {
    // Word-diff each paired row (same index in both runs) exactly once, and reuse
    // the result for the removed- and added-side rendering below — the diff is
    // symmetric, so computing it per side would double the quadratic LCS cost.
    // `segments_if_short` also caps the line length, falling back to plain
    // coloring for a pathological pair rather than allocating a huge table.
    let paired: Vec<Option<(Vec<word_diff::Seg>, Vec<word_diff::Seg>)>> = dels
        .iter()
        .enumerate()
        .map(|(row, del)| {
            adds.get(row)
                .and_then(|add| word_diff::segments_if_short(&del[1..], &add[1..]))
                .filter(|(del_segs, add_segs)| word_diff::is_useful(del_segs, add_segs))
        })
        .collect();

    for (row, del) in dels.iter().enumerate() {
        match paired.get(row).and_then(Option::as_ref) {
            Some((del_segs, _)) => out.push(Line::from(word_diff::spans(
                '-',
                del_segs,
                theme.removed,
                theme.dim,
            ))),
            None => out.push(plain(del)),
        }
    }
    for (row, add) in adds.iter().enumerate() {
        match paired.get(row).and_then(Option::as_ref) {
            Some((_, add_segs)) => out.push(Line::from(word_diff::spans(
                '+',
                add_segs,
                theme.added,
                theme.dim,
            ))),
            None => out.push(plain(add)),
        }
    }
}

#[cfg(test)]
mod diff_render_tests {
    use super::diff_lines;
    use ratatui::style::Modifier;
    use ratatui::text::Line;
    use std::time::Instant;

    /// Reassemble a rendered [`Line`]'s span contents back into its text.
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// A line was word-diffed iff at least one span carries the BOLD modifier the
    /// [`word_diff::spans`] renderer puts on a *changed* word — a plain whole-line
    /// `-`/`+` fallback is a single unbolded span.
    fn is_word_diffed(line: &Line<'_>) -> bool {
        line.spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
    }

    #[test]
    fn short_pair_gets_word_level_highlighting() {
        // A one-word revision on a normal-length line still word-diffs: the shared
        // words render as (dim, unbolded) context and only "bright" / "dark" is
        // bolded on each side (unchanged pre-fix behavior).
        let patch = "-the night was dark\n+the night was bright\n";
        let lines = diff_lines(patch);
        assert_eq!(lines.len(), 2);
        assert!(is_word_diffed(&lines[0]), "removed side word-diffed");
        assert!(is_word_diffed(&lines[1]), "added side word-diffed");
        assert_eq!(line_text(&lines[0]), "-the night was dark");
        assert_eq!(line_text(&lines[1]), "+the night was bright");
    }

    #[test]
    fn long_pair_falls_back_to_plain_and_is_fast() {
        // Two ~50k-token lines sharing a common prefix: the old code would build a
        // ~50k×50k `usize` LCS table (tens of GB) twice. The length guard must fall
        // back to plain whole-line coloring — a single unbolded span per side — and
        // return promptly, never allocating the table.
        let long = "word ".repeat(50_000);
        let old = format!("{long}alpha");
        let new = format!("{long}omega");
        let patch = format!("-{old}\n+{new}\n");

        let start = Instant::now();
        let lines = diff_lines(&patch);
        let elapsed = start.elapsed();

        assert_eq!(lines.len(), 2);
        // Plain fallback: no bolded changed-word spans on either side.
        assert!(!is_word_diffed(&lines[0]), "removed side is plain");
        assert!(!is_word_diffed(&lines[1]), "added side is plain");
        assert_eq!(line_text(&lines[0]), format!("-{old}"));
        assert_eq!(line_text(&lines[1]), format!("+{new}"));
        // Generous ceiling: building the table would take seconds / OOM; the guarded
        // path is well under this even on a slow debug build.
        assert!(
            elapsed.as_secs() < 2,
            "guarded diff should be prompt, took {elapsed:?}",
        );
    }

    #[test]
    fn pair_marks_the_same_words_on_both_sides() {
        // The word diff is computed once per pair and reused for the `-` and `+`
        // side, so the removed and added lines must agree on which word changed:
        // "dark" bolded on the `-` line, "bright" on the `+` line, and the shared
        // "the night was" unbolded on both.
        let patch = "-the night was dark\n+the night was bright\n";
        let lines = diff_lines(patch);

        let bold_text = |line: &Line<'_>| -> String {
            line.spans
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert_eq!(bold_text(&lines[0]), "dark");
        assert_eq!(bold_text(&lines[1]), "bright");
    }
}

//! The no-path launch flow (#526). Run bare, `quill` used to jump straight to a
//! stdin new-project-name prompt; now — when you're not already inside a project
//! and you have projects to open — it offers a choice first: create a new book or
//! open an existing one from your library.
//!
//! The *decision* (open this path in place, offer the chooser, or go straight to
//! the new-project prompt) is factored into the pure [`decide`] so its branches —
//! including the graceful degradation to new-project when there's nothing to open
//! — are unit-testable without a TTY. The interactive chooser/picker
//! ([`choose`]) drives the real terminal and is exercised by hand.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::recent::RecentProject;
use crate::ui;

/// What a no-path launch should do, decided before any terminal setup (#526).
/// Pure so the branches are testable; the caller acts on it.
#[derive(Debug, PartialEq, Eq)]
pub enum StartupAction {
    /// Open the project at this path in place — the current directory is already a
    /// project. No chooser.
    OpenInPlace(PathBuf),
    /// Offer the create-new / open-existing chooser, given the openable projects.
    Choose,
    /// Skip straight to the new-project prompt: there's nothing to open, so an
    /// empty picker would be a dead end (#526). Degrades gracefully.
    NewProject,
}

/// The launch situation the [`decide`] flow reads (#1083) — the two opaque
/// booleans (`in_project`, `open_last`) collapsed into their meaningful named
/// cases, so a caller states *which* situation it is in rather than passing two
/// bare flags. The reopen-last preference only matters off-project, so it lives on
/// the [`OffProject`](StartupContext::OffProject) variant, not as a free-floating
/// flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupContext {
    /// The current directory is already a project — open it in place, no chooser.
    InProject,
    /// Not inside a project: the launch falls to the chooser / reopen-last /
    /// new-project degrade, depending on the reopen-last preference and what is
    /// openable.
    OffProject {
        /// The opt-in reopen-last preference (#613): `[projects] open_last = true`.
        /// When set and a most-recent project exists, that project is reopened
        /// directly instead of showing the chooser.
        open_last: bool,
    },
}

/// Decide what a no-path launch does (#526), from the launch `context` (whether the
/// current directory is already a project, and — off-project — the opt-in
/// reopen-last preference #613), how many openable projects exist (`openable_count`
/// — recents + library), and the `most_recent` project's path, if any. Pure: no
/// I/O, no terminal.
///
/// - Inside a project → open it in place, no chooser.
/// - Off-project, `open_last` set, a most-recent project → open that project
///   directly, skipping the chooser (#613).
/// - Off-project, something to open → the chooser.
/// - Off-project, nothing to open → straight to the new-project prompt, rather
///   than an empty picker.
pub fn decide(
    cwd: &Path,
    context: StartupContext,
    openable_count: usize,
    most_recent: Option<&Path>,
) -> StartupAction {
    let open_last = match context {
        StartupContext::InProject => return StartupAction::OpenInPlace(cwd.to_path_buf()),
        StartupContext::OffProject { open_last } => open_last,
    };
    // The opt-in shortcut (#613): reopen the most-recent project straight away
    // rather than showing the chooser. Only when there *is* one — with no recents
    // this falls through to the usual chooser/degrade below, so the flag never
    // turns into an empty picker or a dead end.
    match most_recent.filter(|_| open_last) {
        Some(recent) => StartupAction::OpenInPlace(recent.to_path_buf()),
        None if openable_count > 0 => StartupAction::Choose,
        None => StartupAction::NewProject,
    }
}

/// The author's choice at the no-path chooser (#526).
pub enum Choice {
    /// Open the existing project at this path.
    Open(PathBuf),
    /// Create a new project (fall through to the name prompt).
    New,
    /// Quit without doing either (Esc / Ctrl-C at the top level).
    Quit,
}

/// Which screen the chooser is showing.
enum Screen {
    /// The two-row create-new / open-existing menu.
    Menu(usize),
    /// The project picker over `projects`, with the selected row.
    Browser(usize),
}

/// Run the no-path chooser on `terminal` (#526): a create-new / open-existing
/// menu, with "open existing" leading to a picker over `projects` (recents +
/// library). Returns the author's [`Choice`]. The terminal is already in raw /
/// alternate-screen mode (the caller's `Tui`).
pub fn choose<B: Backend>(
    terminal: &mut Terminal<B>,
    projects: &[RecentProject],
) -> io::Result<Choice>
where
    io::Error: From<B::Error>,
{
    let mut screen = Screen::Menu(0);
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            match &screen {
                Screen::Menu(sel) => ui::draw_startup_chooser(frame, area, *sel),
                Screen::Browser(sel) => ui::draw_startup_browser(frame, area, projects, *sel),
            }
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        // Ignore key *releases* under the enhanced keyboard protocol (#348); act
        // only on presses, as the editor does.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if let Some(choice) = step(&mut screen, key, projects) {
            return Ok(choice);
        }
    }
}

/// The new selection for a `len`-row list given an arrow / `j`/`k` key, or `None`
/// if the key doesn't move. Mirrors the editor's list navigation (`assist::list_move`),
/// kept local so the startup screen needn't reach into the editor's internals.
fn list_move(selected: usize, len: usize, code: KeyCode) -> Option<usize> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => Some(selected.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') => Some((selected + 1).min(len.saturating_sub(1))),
        _ => None,
    }
}

/// Advance the chooser one keypress: mutate `screen`, or return a final [`Choice`]
/// when the author commits or backs all the way out. Split from the event loop so
/// the loop stays a thin read-draw-dispatch shell.
fn step(screen: &mut Screen, key: KeyEvent, projects: &[RecentProject]) -> Option<Choice> {
    match screen {
        Screen::Menu(sel) => {
            if let Some(s) = list_move(*sel, 2, key.code) {
                *sel = s;
                return None;
            }
            match key.code {
                KeyCode::Enter if *sel == 0 => Some(Choice::New),
                // Open existing → the picker (the projects list is never empty here:
                // the chooser is only shown when there's something to open).
                KeyCode::Enter => {
                    *screen = Screen::Browser(0);
                    None
                }
                KeyCode::Esc => Some(Choice::Quit),
                _ => None,
            }
        }
        Screen::Browser(sel) => {
            if let Some(s) = list_move(*sel, projects.len(), key.code) {
                *sel = s;
                return None;
            }
            match key.code {
                KeyCode::Enter => projects.get(*sel).map(|p| Choice::Open(p.path.clone())),
                // Esc backs out to the menu rather than quitting, so a misfire is
                // recoverable.
                KeyCode::Esc => {
                    *screen = Screen::Menu(0);
                    None
                }
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rp(path: &str) -> RecentProject {
        RecentProject {
            path: PathBuf::from(path),
            title: "T".into(),
        }
    }

    #[test]
    fn inside_a_project_opens_in_place_with_no_chooser() {
        // Already in a project: open it, regardless of how many others exist —
        // and regardless of the reopen-last preference (#613).
        let cwd = Path::new("/books/current");
        assert_eq!(
            decide(cwd, StartupContext::InProject, 5, None),
            StartupAction::OpenInPlace(cwd.to_path_buf())
        );
        assert_eq!(
            decide(cwd, StartupContext::InProject, 0, None),
            StartupAction::OpenInPlace(cwd.to_path_buf())
        );
        // Even with `open_last` set and a recent, an in-project launch opens the
        // *current* directory, not the recent — in-project is unaffected. (The
        // in-project context carries no reopen-last flag: it never reaches it.)
        assert_eq!(
            decide(
                cwd,
                StartupContext::InProject,
                5,
                Some(Path::new("/books/other"))
            ),
            StartupAction::OpenInPlace(cwd.to_path_buf())
        );
    }

    #[test]
    fn not_in_a_project_with_openable_projects_offers_the_chooser() {
        // Default (unset) behavior: the chooser, regardless of a most-recent
        // project being available.
        assert_eq!(
            decide(
                Path::new("/tmp"),
                StartupContext::OffProject { open_last: false },
                1,
                None
            ),
            StartupAction::Choose
        );
        assert_eq!(
            decide(
                Path::new("/tmp"),
                StartupContext::OffProject { open_last: false },
                20,
                Some(Path::new("/books/last"))
            ),
            StartupAction::Choose
        );
    }

    #[test]
    fn not_in_a_project_with_nothing_to_open_degrades_to_new_project() {
        // The graceful-degradation case (#526): no recents, no library — straight
        // to the new-project prompt, never an empty picker.
        assert_eq!(
            decide(
                Path::new("/tmp"),
                StartupContext::OffProject { open_last: false },
                0,
                None
            ),
            StartupAction::NewProject
        );
    }

    #[test]
    fn open_last_reopens_the_most_recent_project_directly() {
        // Opt-in (#613): `open_last` set + a most-recent project → open it in
        // place, skipping the chooser even though there's plenty to choose from.
        let last = Path::new("/books/last");
        assert_eq!(
            decide(
                Path::new("/tmp"),
                StartupContext::OffProject { open_last: true },
                5,
                Some(last)
            ),
            StartupAction::OpenInPlace(last.to_path_buf())
        );
    }

    #[test]
    fn open_last_with_no_recents_is_unchanged() {
        // The flag never becomes a dead end (#613): with no most-recent project it
        // falls through to the usual behavior — the chooser when there's a library
        // to browse, the new-project degrade when there's nothing at all.
        assert_eq!(
            decide(
                Path::new("/tmp"),
                StartupContext::OffProject { open_last: true },
                3,
                None
            ),
            StartupAction::Choose
        );
        assert_eq!(
            decide(
                Path::new("/tmp"),
                StartupContext::OffProject { open_last: true },
                0,
                None
            ),
            StartupAction::NewProject
        );
    }

    #[test]
    fn menu_arrows_move_and_enter_picks_new_or_browser() {
        let projects = vec![rp("/a"), rp("/b")];
        // Down to "open existing", Enter → the picker (not a final choice yet).
        let mut screen = Screen::Menu(0);
        assert!(step(&mut screen, KeyEvent::from(KeyCode::Down), &projects).is_none());
        assert!(matches!(screen, Screen::Menu(1)));
        assert!(step(&mut screen, KeyEvent::from(KeyCode::Enter), &projects).is_none());
        assert!(matches!(screen, Screen::Browser(0)));

        // Top row Enter → create new.
        let mut screen = Screen::Menu(0);
        assert!(matches!(
            step(&mut screen, KeyEvent::from(KeyCode::Enter), &projects),
            Some(Choice::New)
        ));
    }

    #[test]
    fn menu_esc_quits() {
        let mut screen = Screen::Menu(0);
        assert!(matches!(
            step(&mut screen, KeyEvent::from(KeyCode::Esc), &[]),
            Some(Choice::Quit)
        ));
    }

    #[test]
    fn browser_enter_opens_the_selected_project() {
        let projects = vec![rp("/a"), rp("/b")];
        let mut screen = Screen::Browser(0);
        step(&mut screen, KeyEvent::from(KeyCode::Down), &projects);
        let chosen = step(&mut screen, KeyEvent::from(KeyCode::Enter), &projects);
        match chosen {
            Some(Choice::Open(p)) => assert_eq!(p, PathBuf::from("/b")),
            _ => panic!("Enter should open the selected project"),
        }
    }

    #[test]
    fn browser_esc_returns_to_the_menu() {
        let projects = vec![rp("/a")];
        let mut screen = Screen::Browser(0);
        assert!(step(&mut screen, KeyEvent::from(KeyCode::Esc), &projects).is_none());
        assert!(matches!(screen, Screen::Menu(0)));
    }
}

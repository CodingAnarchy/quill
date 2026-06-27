//! Quill — a terminal editor optimized for writing novels.
//!
//! Run `quill [DIR]` to open a project (or create one in an empty directory).
//! Defaults to the current directory.

mod ai;
mod analysis;
mod annotation;
mod app;
mod atomic;
mod buffer;
mod config;
#[cfg(test)]
mod datapath_tests;
mod date;
mod export;
mod frontmatter;
mod guide;
mod history;
mod manuscript;
mod manuscript_import;
mod mode;
mod project;
mod recent;
mod recovery;
mod report;
mod spell;
mod syntax;
#[cfg(test)]
mod testutil;
mod text;
mod theme;
mod typography;
mod ui;
mod vcs;
mod workflow;
mod wrap;

use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::{
    cursor::Show,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::app::App;
use crate::buffer::Buffer;
use crate::project::Project;

/// Whether the enhanced keyboard protocol was pushed at startup (#348), so
/// teardown pops it only when we pushed it. Shared by the `Tui` drop guard and
/// the panic hook (which has no per-instance state) via this process-global.
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

fn main() -> io::Result<()> {
    // A project lives in a directory. An explicit path opens (or creates) it
    // there; a bare launch resolves to an existing project or a freshly named
    // one in the projects home — never a stray project in the current folder
    // (#423).
    let dir = match std::env::args().nth(1) {
        Some(path) => PathBuf::from(path),
        None => match startup_dir() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("quill: {e}");
                std::process::exit(1);
            }
        },
    };

    // A foreseeable data-path failure at startup — a malformed manifest, an
    // unreadable chapter, a directory we can't write — exits with a clear,
    // single-line message instead of a panic or Rust's `Error: Custom {…}`
    // debug dump (#264). The terminal isn't in raw mode yet, so stderr is clean.
    let (project, buffer) = match load_project(&dir) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("quill: cannot open project at {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    // Remember this project so the in-app project browser (#270) can offer it.
    let (root, title) = (project.root.clone(), project.manifest.title.clone());
    let mut app = App::new(buffer, project);
    // Resume today's word-delta counters from the history log.
    app.load_history();
    // If a previous session left unsaved work behind (a crash), offer it back.
    app.offer_recovery();
    recent::record(&root, &title);

    // Drop restores the terminal on a normal or `?`-error exit, but the release
    // `panic = "abort"` profile never unwinds — so install a panic hook too.
    install_panic_hook();

    // `Tui` restores the terminal when dropped, so any return from `run` —
    // including a `?` error — leaves the terminal usable.
    let mut tui = Tui::new()?;
    let result = run(&mut tui, &mut app);
    // Record today's momentum so it survives the next reopen.
    app.persist_snapshot();
    result
}

/// Open the project at `dir` — opening an existing one (a directory with a
/// manifest) or initializing a new one in an empty directory — and load its
/// active chapter. Every foreseeable failure (a malformed manifest, an
/// unreadable chapter, a directory that can't be created or written) comes back
/// as an `Err` carrying a clear message, never a panic (#264).
fn load_project(dir: &Path) -> io::Result<(Project, Buffer)> {
    let project = if Project::manifest_path(dir).exists() {
        Project::open(dir)?
    } else {
        Project::create(dir)?
    };
    let buffer = Buffer::from_path(project.active_path())?;
    Ok((project, buffer))
}

/// The project directory for a no-path launch (#423). If the current directory is
/// already a Quill project, open it in place (the long-standing behavior).
/// Otherwise — rather than scattering a new project into whatever folder the
/// shell happens to be in — ask the author what to call the new book and place it
/// in their projects home. The freshly created project skips the in-app setup
/// wizard (#282), matching `quill <path>`; the wizard is offered only for
/// projects created from within the running editor (Ctrl-B → new).
fn startup_dir() -> io::Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if Project::manifest_path(&cwd).exists() {
        return Ok(cwd); // already inside a project — open it
    }
    let (base, fallback) = new_project_base(
        config::ProjectsConfig::load().home_dir(),
        std::env::var_os("HOME").map(PathBuf::from),
        &cwd,
    );
    // Don't fall back to the current directory silently (#423): when there's no
    // projects home, say where the book is going and how to choose.
    if fallback {
        eprintln!(
            "quill: no projects home configured — set [projects] home in your config \
             to choose where new books live. Creating in {} for now.",
            base.display()
        );
    }
    let name = prompt_project_name()?;
    Ok(base.join(name))
}

/// Where a new project goes for a no-path launch (#423): the configured projects
/// home if set, else a `Quill` folder under the OS home, else the current
/// directory. The second return is whether this is the unconfigured fallback, so
/// the caller can surface the choice instead of placing the book silently.
fn new_project_base(
    projects_home: Option<PathBuf>,
    os_home: Option<PathBuf>,
    cwd: &Path,
) -> (PathBuf, bool) {
    match projects_home {
        Some(home) => (home, false),
        None => (
            os_home
                .map(|h| h.join("Quill"))
                .unwrap_or_else(|| cwd.to_path_buf()),
            true,
        ),
    }
}

/// Ask the author to name the new book (#423), on the plain terminal before the
/// alternate screen opens (like the startup error path). The folder is derived
/// from the name; `Project::create` takes the book title from that folder.
fn prompt_project_name() -> io::Result<String> {
    print!("New project name: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let name = crate::project::sanitize_dir_name(&input);
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no project name entered",
        ));
    }
    Ok(name)
}

/// Undo the terminal setup from [`Tui::new`]: leave raw mode and the alternate
/// screen, and re-show the cursor. Shared by the [`Tui`] drop guard and the
/// panic hook so the two teardown paths can't drift. Best-effort — errors are
/// ignored because there's nothing useful to do with them mid-teardown.
///
/// Hand-rolled rather than `ratatui::restore()`: that helper only disables raw
/// mode and leaves the alternate screen, so it would skip the mouse-capture and
/// cursor teardown that [`Tui::new`]'s `EnableMouseCapture` requires.
fn restore_terminal(w: &mut impl io::Write) {
    // Pop the enhanced keyboard protocol first, but only if we pushed it (#348),
    // so a terminal that never had it enabled gets no stray escape sequence. The
    // `swap` makes this idempotent: whichever teardown path runs first (the drop
    // guard or the panic hook) pops, the other is a no-op.
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(w, PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(w, LeaveAlternateScreen, DisableMouseCapture, Show);
}

/// Install a panic hook that restores the terminal before the default hook
/// prints the panic message, so a crash doesn't leave the shell in raw mode /
/// the alternate screen. Chains to the previous hook so the message still shows.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(&mut io::stdout());
        prev(info);
    }));
}

/// Owns the terminal in raw / alternate-screen mode and restores it on drop.
struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        // Turn on the terminal's enhanced ("Kitty") keyboard protocol when it's
        // supported (#348), so combinations the legacy encoding can't tell apart
        // — Tab vs Ctrl-I, Enter vs Ctrl-M, and various Ctrl/Shift combos — arrive
        // as distinct key events. `DISAMBIGUATE_ESCAPE_CODES` is the high-value,
        // low-risk flag: it doesn't add key-release/repeat events, so the existing
        // press-only handling is unchanged. Terminals without support are left
        // exactly as before; the query and push are best-effort.
        if supports_keyboard_enhancement().unwrap_or(false)
            && execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )
            .is_ok()
        {
            KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
        }
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(stdout))?,
        })
    }

    fn draw(&mut self, app: &mut App) -> io::Result<()> {
        self.terminal.draw(|frame| {
            // Record the terminal height so the key handler can clamp overlay
            // scrolling to the content that actually exists.
            app.viewport_height = frame.area().height;
            ui::draw(frame, app);
        })?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        restore_terminal(self.terminal.backend_mut());
    }
}

fn run(tui: &mut Tui, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        // Surface a finished AI request (runs on a worker thread) before drawing,
        // so its answer appears within the poll window even with no keypress.
        app.poll_ai();
        app.tick_sprint(); // end a writing sprint when its time is up (#299)
        app.tick_autosave(); // idle-timer autosave, when enabled (#409)
        app.poll_assistant(); // advance the assistant pane's tool-use loop (#307)
        app.poll_vc(); // surface a finished push/pull (worker thread) before drawing
        app.poll_model_download(); // surface a finished local-model download (#382)
        tui.draw(app)?;

        // Poll so we can stay responsive without busy-spinning.
        if event::poll(Duration::from_millis(100))? {
            app.handle_event(event::read()?);
            // Drain the rest of the input burst before doing any heavier work,
            // so a fast typist (or a key-repeat) is handled as one batch.
            while event::poll(Duration::ZERO)? {
                app.handle_event(event::read()?);
            }
            // Input has gone idle: reparse for highlighting and tally the day's
            // word delta. Kept off the per-keystroke path so typing stays instant.
            app.on_idle();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::scratch_dir("main", name)
    }

    #[test]
    fn load_project_reports_a_malformed_manifest_instead_of_panicking() {
        let dir = tmpdir("bad-manifest");
        // A manifest that exists but is broken TOML: opened, not re-created.
        std::fs::write(Project::manifest_path(&dir), "title = \"X\"\n[[chapters]\n").unwrap();
        let err = load_project(&dir)
            .err()
            .expect("a malformed manifest should be an error");
        assert!(
            err.to_string().contains(crate::project::MANIFEST_NAME),
            "the message should name the manifest: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn new_project_base_prefers_the_configured_projects_home() {
        // A configured projects home is used as-is, no fallback note (#423).
        let home = PathBuf::from("/books");
        let (base, fallback) = new_project_base(
            Some(home.clone()),
            Some(PathBuf::from("/home/me")),
            Path::new("/tmp/cwd"),
        );
        assert_eq!(base, home);
        assert!(!fallback);
    }

    #[test]
    fn new_project_base_falls_back_to_a_quill_folder_in_home() {
        // With no projects home, a `Quill` folder under the OS home — never the
        // current directory — and the fallback is flagged so it isn't silent.
        let (base, fallback) =
            new_project_base(None, Some(PathBuf::from("/home/me")), Path::new("/tmp/cwd"));
        assert_eq!(base, PathBuf::from("/home/me/Quill"));
        assert!(fallback);
    }

    #[test]
    fn new_project_base_last_resorts_to_the_current_directory() {
        // Only when there's no home at all does it land beside the shell.
        let (base, fallback) = new_project_base(None, None, Path::new("/tmp/cwd"));
        assert_eq!(base, PathBuf::from("/tmp/cwd"));
        assert!(fallback);
    }

    #[test]
    fn load_project_initializes_an_empty_directory() {
        let dir = tmpdir("fresh");
        let (project, _buffer) = load_project(&dir).expect("an empty dir becomes a new project");
        assert!(Project::manifest_path(&project.root).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_terminal_teardown_sequences() {
        // Both phases share the process-global `KEYBOARD_ENHANCED`, so they're
        // one sequential test (no cross-test race on the flag).

        // restore_terminal writes the teardown escapes to the given sink;
        // disable_raw_mode touches the (absent) tty and is harmlessly ignored.
        KEYBOARD_ENHANCED.store(false, Ordering::SeqCst); // not pushed this session
        let mut out: Vec<u8> = Vec::new();
        restore_terminal(&mut out);
        let seq = String::from_utf8(out).unwrap();
        // The user-visible halves of teardown: back to the main screen, cursor
        // shown. If these regress, a crash leaves the shell unusable.
        assert!(
            seq.contains("\x1b[?1049l"),
            "should leave alt screen: {seq:?}"
        );
        assert!(seq.contains("\x1b[?25h"), "should show the cursor: {seq:?}");
        // With the enhanced keyboard protocol never pushed, teardown must not emit
        // its pop sequence — no stray escape on a terminal that didn't enable it.
        assert!(
            !seq.contains("\x1b[<1u"),
            "no keyboard-enhancement pop when unpushed: {seq:?}"
        );

        // A session that enabled the enhanced protocol (#348) pops it on teardown.
        KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
        let mut out: Vec<u8> = Vec::new();
        restore_terminal(&mut out);
        assert!(
            String::from_utf8(out).unwrap().contains("\x1b[<1u"),
            "should pop the enhancement"
        );
        // The flag is cleared, so the second teardown path (panic hook after the
        // drop guard) is a no-op — no double-pop.
        assert!(
            !KEYBOARD_ENHANCED.load(Ordering::SeqCst),
            "flag cleared after pop"
        );
        let mut again: Vec<u8> = Vec::new();
        restore_terminal(&mut again);
        assert!(
            !String::from_utf8(again).unwrap().contains("\x1b[<1u"),
            "no double-pop"
        );
    }
}

//! Quill — a terminal editor optimized for writing novels.
//!
//! Run `quill [PATH]` to open a project (or create one in an empty directory).
//! A bare `quill` resolves a project via the startup chooser; `quill --help`
//! and `quill --version` print usage / the version and exit.

mod ai;
mod analysis;
mod annotation;
mod app;
mod archive;
mod atomic;
mod buffer;
mod clipboard;
mod config;
#[cfg(test)]
mod datapath_tests;
mod date;
mod export;
mod fonts;
mod frontmatter;
mod guide;
mod history;
mod history_view;
mod lastpos;
mod manuscript;
mod manuscript_import;
mod mode;
mod onboarding;
mod project;
mod recent;
mod recovery;
mod report;
mod spell;
mod startup;
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
use std::time::{Duration, Instant};

use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::app::App;
use crate::project::Project;

/// Whether the enhanced keyboard protocol was pushed at startup (#348), so
/// teardown pops it only when we pushed it. Shared by the `Tui` drop guard and
/// the panic hook (which has no per-instance state) via this process-global.
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// The `quill <version>` banner printed for `--version`/`-V` (#516). The version
/// is the crate version from `Cargo.toml`, baked in at compile time.
const VERSION_LINE: &str = concat!("quill ", env!("CARGO_PKG_VERSION"));

/// The event loop's tick: the longest it waits for input before re-running the
/// `tick_*`/`poll_*` calls, so AI results, sprints, and autosave surface with no
/// keypress.
const TICK: Duration = Duration::from_millis(100);

/// How long input must stay quiet before the debounced idle pass
/// ([`App::on_quiet`]) runs. Long enough to span the gaps between keys in a
/// typing burst (typically ~100–200ms), so a burst stays on the cheap per-batch
/// path; short enough that spell flags, the revision passes, and the word tally
/// catch up as soon as the author pauses. It is also the undo grouping: keys
/// typed with no gap this long undo together.
const IDLE_QUIET: Duration = Duration::from_millis(200);

/// What the first command-line argument asks Quill to do. Decided by [`parse_arg`]
/// before any filesystem work so a flag-style argument (`--help`, `--version`) is
/// never mistaken for a project directory and scaffolded into a junk folder
/// (#517/#516).
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    /// `quill --help` / `quill -h`: print usage and exit (#517).
    Help,
    /// `quill --version` / `quill -V`: print the version and exit (#516).
    Version,
    /// `quill --series`: start a new multi-book series at launch (#1135) —
    /// prompt for a series title and a first-book title, scaffold directly into
    /// the `[[books]]` shape, and open it.
    Series,
    /// `quill <path>`: open or create a project at this directory.
    Path(PathBuf),
    /// A bare `quill`: resolve a project via the startup chooser.
    NoArg,
}

/// Classify the first command-line argument into an [`Invocation`]. Pure (no
/// filesystem or environment access) so the flag-vs-path decision is unit-testable
/// and so a help/version query does zero filesystem work (#517/#516): the flag
/// checks come first, then anything else is taken as a project path.
fn parse_arg(arg: Option<&str>) -> Invocation {
    match arg {
        Some("--help" | "-h") => Invocation::Help,
        Some("--version" | "-V") => Invocation::Version,
        Some("--series") => Invocation::Series,
        Some(path) => Invocation::Path(PathBuf::from(path)),
        None => Invocation::NoArg,
    }
}

/// The usage summary printed for `quill --help`/`-h` (#517). Kept short and
/// accurate to current behavior: a bare launch resolves to a project in the
/// projects home (#423) via the startup chooser (#526), and an explicit path opens
/// or creates a project there. A pure function so the text is unit-testable.
fn usage_text() -> String {
    format!(
        "{VERSION_LINE}\n\
         A terminal editor for writing novels.\n\
         \n\
         Usage: quill [OPTIONS] [PATH]\n\
         \n\
         Arguments:\n  \
         [PATH]  Open or create a project at this directory\n\
         \n\
         Options:\n  \
         --series       Start a new multi-book series (prompts for the series\n                 \
         and first-book titles) and open it\n  \
         -h, --help     Print this help and exit\n  \
         -V, --version  Print version and exit\n\
         \n\
         With no PATH, Quill opens the current directory if it is a project,\n\
         otherwise it offers to open a recent project, start a new series, or\n\
         create a new standalone book in your projects home."
    )
}

fn main() -> io::Result<()> {
    // Classify the first argument before any filesystem work (#517/#516), so a
    // flag-style argument prints and exits rather than being treated as a project
    // directory and scaffolding a junk folder named after the flag.
    let first_arg = std::env::args().nth(1);
    let dir = match parse_arg(first_arg.as_deref()) {
        Invocation::Help => {
            println!("{}", usage_text());
            return Ok(());
        }
        Invocation::Version => {
            println!("{VERSION_LINE}");
            return Ok(());
        }
        // Start a new series at launch (#1135): prompt for the series and
        // first-book titles on the plain terminal, scaffold directly into the
        // `[[books]]` shape, and open it. A scaffold failure exits cleanly, like
        // the no-path new-project path.
        Invocation::Series => match start_new_series() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("quill: {e}");
                std::process::exit(1);
            }
        },
        // An explicit path opens (or creates) a project there.
        Invocation::Path(path) => path,
        // A bare launch resolves to an existing project, one chosen from the
        // library, or a freshly named one in the projects home — never a stray
        // project in the current folder (#423/#526).
        Invocation::NoArg => match startup_dir() {
            Ok(Some(dir)) => dir,
            // The author quit at the startup chooser without picking anything (#526).
            Ok(None) => return Ok(()),
            Err(e) => {
                eprintln!("quill: {e}");
                std::process::exit(1);
            }
        },
    };

    // Startup loads a dozen config sections (the editor's, then onboarding's);
    // parse the config and manifest once for all of them. Dropped before the
    // event loop, so later loads read the files fresh.
    let config_session = config::LoadSession::begin();
    // A foreseeable data-path failure at startup — a malformed manifest, an
    // unreadable chapter, a directory we can't write — exits with a clear,
    // single-line message instead of a panic or Rust's `Error: Custom {…}`
    // debug dump (#264). The terminal isn't in raw mode yet, so stderr is clean.
    // `launch` opens the project on the chapter the author was last in, then
    // makes the on-open offers (crash recovery, damage restore, pass-queue resume).
    let mut app = match App::launch(&dir) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("quill: cannot open project at {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    // Remember this project so the in-app project browser (#270) can offer it.
    let (root, title) = (app.project.root.clone(), app.project.manifest.title.clone());
    // Reconcile any background bible batches submitted in a previous session (#950):
    // a batch can take hours, so its results are polled and applied at launch (and on
    // idle) rather than in-session. Non-blocking — it just kicks off the poll worker.
    app.reconcile_batch_update();
    // On the very first launch, show the dismissible quick-start card (#837).
    // After recovery, so a crash prompt takes precedence and the card waits for a
    // clean slate (it's a no-op when a prompt/overlay is already up).
    app.maybe_offer_onboarding();
    drop(config_session);
    recent::record(&root, &title);

    // Drop restores the terminal on a normal or `?`-error exit, but the release
    // `panic = "abort"` profile never unwinds — so install a panic hook too.
    install_panic_hook();

    // `Tui` restores the terminal when dropped, so any return from `run` —
    // including a `?` error — leaves the terminal usable.
    let mut tui = Tui::new()?;
    let result = run(&mut tui, &mut app);
    // Let the background recovery writer finish (bounded): a quit that discarded
    // unsaved work queued a sidecar clear, and a clean quit may still have a
    // write or clear in flight. Not a capture — a deliberate discard must not
    // re-record what it just threw away.
    app.flush_recovery();
    // The quiet on-quit recap (#1012): this session's net words and the day's net,
    // read from the daily tally already maintained (no new counting). Captured
    // before `record_session` advances the baseline, then printed *after* the TUI
    // drops so it lands on the restored plain terminal rather than the alt screen.
    let recap = app.session_recap();
    // Record today's momentum so it survives the next reopen.
    app.persist_snapshot();
    // Record this session's output to the per-session log (#835) — the calendar
    // heatmap's per-day breakdown reads it.
    app.record_session();
    // Remember where the author left off, to resume here next open (#588).
    app.persist_position();
    // Drop the TUI (leaving the alternate screen, restoring the terminal) before
    // printing the recap, so it appears on the normal screen the author returns to.
    drop(tui);
    if let Some(recap) = recap {
        println!("{recap}");
    }
    result
}

/// The project directory for a no-path launch (#423/#526), or `None` if the
/// author quit the startup chooser without picking anything.
///
/// If the current directory is already a Quill project, open it in place (the
/// long-standing behavior). Otherwise, when there are projects to open — recents
/// or a configured library — offer a chooser: create a new book or open an
/// existing one (#526). With nothing to open, the chooser would be a dead end, so
/// it degrades straight to the new-project prompt. Creating a new book asks the
/// author what to call it and places it in their projects home, never scattering a
/// project into whatever folder the shell happens to be in.
///
/// The freshly created project skips the in-app setup wizard (#282), matching
/// `quill <path>`; the wizard is offered only for projects created from within the
/// running editor (Ctrl-B → new).
fn startup_dir() -> io::Result<Option<PathBuf>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let in_project = Project::manifest_path(&cwd).exists();
    let cfg = config::ProjectsConfig::load();
    let projects = recent::openable(cfg.home_dir().as_deref());
    // The reopen-last shortcut (#613) targets the most-recent project specifically —
    // the head of the recents list, not a library-only project — so it never
    // "reopens" a book the author has yet to open.
    let most_recent = recent::list().into_iter().next().map(|p| p.path);
    let context = if in_project {
        startup::StartupContext::InProject
    } else {
        startup::StartupContext::OffProject {
            open_last: cfg.open_last_enabled(),
        }
    };
    match startup::decide(&cwd, context, projects.len(), most_recent.as_deref()) {
        startup::StartupAction::OpenInPlace(dir) => Ok(Some(dir)),
        startup::StartupAction::NewProject => Ok(Some(new_project_dir()?)),
        startup::StartupAction::Choose => choose_startup_project(&projects),
    }
}

/// Run the no-path chooser (#526) and turn the author's choice into a project
/// directory: an existing project's path, a freshly named new project, or `None`
/// when they quit. The chooser is a brief alternate-screen session (its own
/// [`Tui`], restored on drop) so it matches the in-editor browser; "create new"
/// then falls through to the plain-terminal name prompt, like before.
fn choose_startup_project(projects: &[recent::RecentProject]) -> io::Result<Option<PathBuf>> {
    let choice = {
        let mut tui = Tui::new()?;
        startup::choose(&mut tui.terminal, projects)?
        // `tui` drops here, restoring the plain terminal before any stdin prompt.
    };
    match choice {
        startup::Choice::Open(path) => Ok(Some(path)),
        startup::Choice::New => Ok(Some(new_project_dir()?)),
        // Start a new series (#1135): scaffold directly into the `[[books]]` shape,
        // like `quill --series`, then open it.
        startup::Choice::NewSeries => Ok(Some(start_new_series()?)),
        startup::Choice::Quit => Ok(None),
    }
}

/// The directory for a brand-new project on a no-path launch (#423): the projects
/// home (with the `~/Quill` fallback, flagged so it isn't silent) joined with the
/// name the author types at the plain-terminal prompt. Shared by the
/// degrade-to-new path and the chooser's "create new".
fn new_project_dir() -> io::Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
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
    projects_home.map_or_else(
        || {
            (
                os_home.map_or_else(|| cwd.to_path_buf(), |h| h.join("Quill")),
                true,
            )
        },
        |home| (home, false),
    )
}

/// Start a new multi-book series at launch (#1135), shared by `quill --series`
/// and the chooser's "New series…". Prompts on the plain terminal (like
/// [`prompt_project_name`]) for a series title and a first-book title, scaffolds a
/// project *directly* in the `[[books]]` shape via [`Project::create_series`], and
/// returns its directory so the usual open path loads it as a genuine series.
///
/// The project folder is derived from the series title (mirroring how the
/// single-book path derives its folder from the book name), placed in the projects
/// home with the same `~/Quill` fallback the single-book path uses.
fn start_new_series() -> io::Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (base, fallback) = new_project_base(
        config::ProjectsConfig::load().home_dir(),
        std::env::var_os("HOME").map(PathBuf::from),
        &cwd,
    );
    if fallback {
        eprintln!(
            "quill: no projects home configured — set [projects] home in your config \
             to choose where new books live. Creating in {} for now.",
            base.display()
        );
    }
    let (series_title, first_book_title) = prompt_series_titles()?;
    let folder = crate::project::sanitize_dir_name(&series_title);
    let root = base.join(folder);
    // Scaffold directly into the `[[books]]` series shape — the on-disk result is
    // identical to a series built by converting a standalone in-app.
    Project::create_series(&root, &series_title, &first_book_title)?;
    Ok(root)
}

/// Prompt for a new series' title and its first book's title (#1135), on the plain
/// terminal before the alternate screen opens — mirroring [`prompt_project_name`].
/// Both are required; an empty entry for either is an error rather than a blank
/// series or an untitled book.
///
/// The series title becomes the on-disk folder, so it's sanitized *before* the
/// empty check (like [`prompt_project_name`]): a punctuation-only title such as
/// `.` or `...` sanitizes to `""` and must be rejected, or it would scaffold into
/// the projects-home root and could overwrite an existing `book.toml` there
/// (#1143). The first book's title is a book title, not a folder, so it keeps its
/// own raw-trimmed validation.
fn prompt_series_titles() -> io::Result<(String, String)> {
    let read = |label: &str, sanitize: bool| -> io::Result<String> {
        print!("{label}: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let value = if sanitize {
            crate::project::sanitize_dir_name(&input)
        } else {
            input.trim().to_string()
        };
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no {label} entered"),
            ));
        }
        Ok(value)
    };
    let series_title = read("Series title", true)?;
    let first_book_title = read("First book title", false)?;
    Ok((series_title, first_book_title))
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
    let _ = execute!(
        w,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show
    );
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
        // Bracketed paste (#811) so a terminal paste (Ctrl-Shift-V / Cmd-V /
        // middle-click) arrives as one `Event::Paste` with the OS clipboard's
        // text, inserted as a single undoable edit rather than replayed key by key.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
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
    // When the last input batch was handled, while an idle pass is still owed.
    let mut idle_owed_since: Option<Instant> = None;
    while !app.should_quit {
        // Surface a finished AI request (runs on a worker thread) before drawing,
        // so its answer appears within the poll window even with no keypress.
        app.poll_ai();
        app.tick_sprint(); // end a writing sprint when its time is up (#299)
        app.tick_session_target(); // signal reaching the session word target (#1011)
        app.tick_autosave(); // idle-timer autosave, when enabled (#409)
        app.tick_backup(); // automatic off-site backup on its cadence, when enabled (#887)
        app.tick_local_backup(); // scheduled local archive on its cadence, when enabled (#888)
        app.poll_assistant(); // advance the assistant pane's tool-use loop (#307)
        app.poll_backup(); // surface a finished automatic backup (worker thread) (#887)
        app.poll_local_backup(); // surface a finished local archive (worker thread) (#888)
        app.poll_vc(); // surface a finished push/pull (worker thread) before drawing
        app.poll_model_download(); // surface a finished local-model download (#382)
        app.poll_batch_update(); // apply/report a finished background bible batch (#950)
        tui.draw(app)?;
        // Crash-recovery capture, after the frame so a typed key has already
        // echoed: a no-op unless the text changed since the last capture, and at
        // most one write per `RECOVERY_MIN_INTERVAL` — so sustained typing that
        // never pauses long enough for the idle pass is still captured.
        app.tick_recovery();

        // Wait for input, but no longer than the tick (so AI results, sprints, and
        // autosave surface without a keypress) — or, while an idle pass is owed,
        // than the rest of the quiet window.
        let timeout =
            idle_owed_since.map_or(TICK, |t| IDLE_QUIET.saturating_sub(t.elapsed()).min(TICK));
        if event::poll(timeout)? {
            app.handle_event(event::read()?);
            // Drain the rest of the input burst before doing any heavier work,
            // so a fast typist (or a key-repeat) is handled as one batch.
            while event::poll(Duration::ZERO)? {
                app.handle_event(event::read()?);
            }
            // Per batch, only the cheap pass that keeps the renderer's byte-offset
            // caches (syntax, folds, annotations, spell flags) aligned with the
            // edited text; the loop then draws at once, so the key echoes before
            // any heavy idle work.
            app.on_input_settled();
            idle_owed_since = Some(Instant::now());
        } else if idle_owed_since.is_some_and(|t| t.elapsed() >= IDLE_QUIET) {
            // Input has been quiet for `IDLE_QUIET`: the debounced idle pass —
            // re-judge spelling and the revision passes, tally the day's words,
            // seal the undo group (so a burst undoes as one step), and capture
            // recovery. Kept off the per-batch path so typing stays instant.
            app.on_quiet();
            idle_owed_since = None;
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
    fn launch_reports_a_malformed_manifest_instead_of_panicking() {
        let dir = tmpdir("bad-manifest");
        // A manifest that exists but is broken TOML: opened, not re-created.
        std::fs::write(Project::manifest_path(&dir), "title = \"X\"\n[[chapters]\n").unwrap();
        let err = App::launch(&dir)
            .err()
            .expect("a malformed manifest should be an error");
        assert!(
            err.to_string().contains(crate::project::MANIFEST_NAME),
            "the message should name the manifest: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn punctuation_only_series_title_would_scaffold_into_the_projects_home_root() {
        // #1143 regression: a punctuation-only series title (`.`, `...`, or only
        // whitespace/punctuation) sanitizes to `""`, and `base.join("")` is the
        // projects-home root itself — so scaffolding there would overwrite an
        // existing `book.toml`. `prompt_series_titles` now sanitizes the series
        // title *before* the empty check and rejects it, mirroring the single-book
        // `prompt_project_name`. This pins the trap the guard exists to prevent.
        let base = PathBuf::from("/tmp/projects-home");
        for title in ["", ".", "...", "   ", " .. ", "\t...\n", ".....  "] {
            let folder = crate::project::sanitize_dir_name(title);
            assert!(
                folder.is_empty(),
                "punctuation/whitespace-only {title:?} should sanitize to empty, got {folder:?}"
            );
            // The empty folder collapses `base.join(folder)` onto the root: the
            // exact overwrite the pre-empty-check sanitize now prevents.
            assert_eq!(
                base.join(&folder),
                base,
                "an empty folder scaffolds into the projects-home root"
            );
        }
        // A valid title still yields a correctly-named folder under the home.
        let folder = crate::project::sanitize_dir_name("The Chronicles");
        assert_eq!(folder, "The Chronicles");
        assert_eq!(base.join(&folder), base.join("The Chronicles"));
    }

    #[test]
    fn parse_arg_classifies_flags_paths_and_no_arg() {
        // The help and version flags are recognized in both spellings (#517/#516)
        // and never treated as a path, so neither scaffolds a junk folder.
        assert_eq!(parse_arg(Some("--help")), Invocation::Help);
        assert_eq!(parse_arg(Some("-h")), Invocation::Help);
        assert_eq!(parse_arg(Some("--version")), Invocation::Version);
        assert_eq!(parse_arg(Some("-V")), Invocation::Version);
        // The new-series flag (#1135) is recognized and never treated as a path,
        // so it starts the series flow rather than scaffolding a `--series` folder.
        assert_eq!(parse_arg(Some("--series")), Invocation::Series);
        // Anything else is a project path; a bare launch is the no-arg case.
        assert_eq!(
            parse_arg(Some("/tmp/book")),
            Invocation::Path(PathBuf::from("/tmp/book"))
        );
        assert_eq!(parse_arg(None), Invocation::NoArg);
        // Near-misses are paths, not flags — `-v` and a bare `help` aren't the flag.
        assert_eq!(parse_arg(Some("-v")), Invocation::Path(PathBuf::from("-v")));
        assert_eq!(
            parse_arg(Some("help")),
            Invocation::Path(PathBuf::from("help"))
        );
    }

    #[test]
    fn usage_text_describes_the_real_invocations() {
        // The help text leads with the version banner and covers the actual
        // invocations: a bare launch, an explicit path, and both flags (#517).
        let usage = usage_text();
        assert!(
            usage.starts_with(VERSION_LINE),
            "leads with the banner: {usage}"
        );
        assert!(usage.contains("Usage: quill [OPTIONS] [PATH]"));
        assert!(usage.contains("[PATH]"));
        assert!(usage.contains("--help"));
        assert!(usage.contains("--version"));
        // The new-series flag is documented (#1135).
        assert!(usage.contains("--series"), "documents --series: {usage}");
        assert!(
            usage.contains("projects home"),
            "explains the no-path behavior: {usage}"
        );
    }

    #[test]
    fn version_line_matches_the_crate_version() {
        // The banner is `quill <Cargo.toml version>`, baked in at compile time.
        assert_eq!(VERSION_LINE, format!("quill {}", env!("CARGO_PKG_VERSION")));
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
    fn launch_initializes_an_empty_directory() {
        let dir = tmpdir("fresh");
        let app = App::launch(&dir).expect("an empty dir becomes a new project");
        assert!(Project::manifest_path(&app.project.root).exists());
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

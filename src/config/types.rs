//! The typed config structs and their loaders: the `[editor]` section
//! ([`EditorConfig`]), the `[projects]` section ([`ProjectsConfig`]), the
//! `[snippets]` table ([`Snippet`]), and the `[theme]` section. Each loader
//! routes through [`load_section`] so it picks up the per-project override (#710)
//! for free.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::theme::Theme;

use super::load_section;

/// Editor behavior from the `[editor]` section. The default keeps Quill's
/// modeless editing unchanged (but with smart typography on — see below).
///
/// The on/off knobs live in [`EditorFlags`], flattened in so the file layout is
/// unchanged (`[editor] typewriter = true`); this struct holds only the valued
/// settings.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    /// The boolean `[editor]` knobs, read from the same table (flattened).
    #[serde(flatten)]
    pub flags: EditorFlags,
    /// Vim key remaps (#208) from the `[editor.keys]` table: action name → the
    /// key that should trigger it (e.g. `down = "n"`). Validated when the editor
    /// builds its keymap; unknown actions are reported, not silently dropped.
    pub keys: HashMap<String, String>,
    /// The centered text column width in characters (#303). Defaults to 72; the
    /// runtime `+`/`-` adjustment still applies on top, this just sets the start.
    pub column_width: u16,
    /// Autosave interval in seconds (#409): after this many seconds with no input,
    /// the active chapter is saved automatically through the same atomic,
    /// front-matter-preserving path as Ctrl-S (but without firing on-save
    /// workflows). `0` (the default) turns autosave off — behavior unchanged.
    pub autosave_interval_secs: u64,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            flags: EditorFlags::default(),
            keys: HashMap::new(),
            column_width: 72,
            autosave_interval_secs: 0,
        }
    }
}

/// The boolean `[editor]` knobs, grouped so the editor copies them as one value
/// (at startup and on a live config reload) and reads them as `app.flags.<knob>`.
///
/// Adding a knob: a field here (plus its default below), one
/// [`SettingBool::SPECS`](crate::mode::SettingBool::SPECS) row (config key, label,
/// AI-vocabulary description, Settings-menu placement, optional palette toggle),
/// and its line in `docs/configuration.md`. The Settings menu, the `:set` /
/// describe-config vocabulary and validator, the palette toggle, and the docs
/// guard all derive from that row; a field with no row fails the SPECS
/// completeness test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct EditorFlags {
    /// Opt-in Vim-style modal editing (#27). Off by default — with it off the
    /// editor behaves exactly as it always has. When on, the editor's resting
    /// state is Normal mode and the status bar shows the mode.
    pub vim: bool,
    /// Smart typography (#296): as you type, straight quotes become curly
    /// (`“” ‘’`), `--` becomes an em-dash (`—`), and `...` becomes an ellipsis
    /// (`…`). **On by default** — set `smart_typography = false` for plain ASCII
    /// punctuation (and an exact byte-for-byte round-trip of what you type).
    pub smart_typography: bool,
    /// Auto-pair quotes and brackets (#1007): typing an opener (`"`, `(`, `[`, or
    /// an opening curly quote) inserts its matching closer and drops the cursor
    /// between the pair; typing the closer when it's already at the cursor skips
    /// past it instead of duplicating. **Off by default** — opt in with
    /// `auto_pair = true`. Composes with `smart_typography`: with both on, a `"`
    /// becomes a correctly-oriented open/close curly-quote pair.
    pub auto_pair: bool,
    /// Typewriter scrolling (#301): keep the cursor's line pinned at the vertical
    /// centre of the page, the text scrolling under it, instead of letting the
    /// cursor drift to the bottom as a chapter grows. Off by default.
    pub typewriter: bool,
    /// Focus mode (#301): dim every paragraph but the one under the cursor, so the
    /// passage you're shaping stands out. Off by default.
    pub focus_mode: bool,
    /// First-line paragraph indent (#722): visually indent the first line of each
    /// prose paragraph in the editor, matching what the exports already do
    /// (soft-wrapped continuation rows, blank lines, headings, dividers, epigraphs,
    /// fenced code, and the first paragraph after a heading are never indented). It's
    /// display-only — no spaces ever enter the buffer or the saved file. Off by
    /// default.
    pub paragraph_indent: bool,
    /// The annotation margin: show each inline note in the gutter to the right of
    /// the prose column, level with the text it's anchored to, so the author sees
    /// their notes while they write. Display-only. On by default; it only appears
    /// when the chapter has annotations and the terminal has room beside the column.
    pub annotation_margin: bool,
    /// The ambient below-target cue (#1012): when a `[goals] daily` target is set
    /// and today's net is under it, a quiet, dismissible status-bar tag shows how
    /// far short the day is. **On by default** — set `daily_nudge = false` to turn
    /// it off entirely for authors who don't want it. It never appears without a
    /// daily target, so a project with no target is unaffected either way. Reads
    /// only the cached daily tally, so it costs nothing per draw.
    pub daily_nudge: bool,
}

impl Default for EditorFlags {
    fn default() -> Self {
        Self {
            vim: false,
            smart_typography: true,
            auto_pair: false,
            typewriter: false,
            focus_mode: false,
            paragraph_indent: false,
            annotation_margin: true,
            daily_nudge: true,
        }
    }
}

impl EditorConfig {
    /// Load the `[editor]` section from the home config file. Best-effort: a
    /// missing or unreadable file yields the defaults (Vim off).
    pub fn load() -> Self {
        load_section("editor")
    }
}

/// The default number of local archives to keep per project (#888) once local
/// backups are on: the most-recent few, enough to roll back a mistake without a
/// backups folder growing without bound. Older archives past this count are
/// pruned after each new one lands.
const DEFAULT_LOCAL_KEEP: usize = 5;

/// The default stale-backup warning window in hours (#888): open a project whose
/// newest local archive is older than this and the browser/status warns the copy
/// is stale. A day is long enough not to nag between sessions, short enough that
/// a real gap is caught.
const DEFAULT_STALE_WARN_HOURS: u64 = 24;

/// The `[backup]` section (#887/#888): opt-in automatic backup, per project. Two
/// independent stories share the one section — an off-site push to the configured
/// Git remote (#887) and a local timestamped `.zip` archive cadence (#888) — so a
/// book can have either, both, or neither. Off by default; like every settings
/// section it layers per project through `[quill.backup]` in the project's
/// `book.toml`, so one book can back up automatically while the rest don't (#710).
///
/// The remote push has two independent triggers, either or both of which may be
/// on once `enabled` is set:
/// * `on_save` — push after a save lands (the buffer reached disk), so the remote
///   tracks the author's saves.
/// * `interval_mins` — push at least this often while the editor is open, catching
///   a long session that saves rarely. `0` (the default) leaves the timed trigger
///   off, so `on_save` alone drives backups.
///
/// The local archive cadence (#888) is gated behind its own `local_enabled` flag,
/// independent of `enabled`: it needs no remote (or even a git repo), just a
/// backups directory, so it protects prose off-network. `local_interval_hours`
/// sets how often it archives; `local_keep` how many recent archives to keep; and
/// `stale_warn_hours` the window past which an author is warned on open that their
/// last local backup is old.
///
/// With everything off (the default) nothing here has any effect; with the remote
/// push on but no remote configured, that hook is a no-op too — a push needs
/// somewhere to go.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct BackupConfig {
    /// Whether automatic remote backup is on for this project (#887). Off by
    /// default — with it off the remote is only ever updated by an explicit
    /// push, exactly as before.
    pub enabled: bool,
    /// Push after a save reaches disk (#887). On by default *once `enabled` is
    /// set*, so the common "back up whenever I save" case needs only
    /// `enabled = true`; set `on_save = false` to rely on the timed interval
    /// alone.
    pub on_save: bool,
    /// Push at least this often, in minutes, while the editor is open (#887) —
    /// the cadence that catches a long session with few saves. `0` (the default)
    /// leaves the timed trigger off; `on_save` still applies.
    pub interval_mins: u64,
    /// Whether scheduled *local* archive backups are on for this project (#888).
    /// Off by default and independent of `enabled` (the remote push): a local
    /// snapshot needs no network, so it's opted into on its own.
    pub local_enabled: bool,
    /// Archive the project to the backups directory at least this often, in hours,
    /// while the editor is open (#888). `0` (the default) leaves the local cadence
    /// off even when `local_enabled` is set — a belt-and-braces guard against a
    /// zero-interval hammering the disk.
    pub local_interval_hours: u64,
    /// How many recent local archives to keep per project (#888); older ones are
    /// pruned after each new archive lands. Defaults to [`DEFAULT_LOCAL_KEEP`]; a
    /// `0` disables pruning (keep everything) rather than deleting all backups.
    pub local_keep: usize,
    /// Warn on open when the newest local archive is older than this many hours
    /// (#888). Defaults to [`DEFAULT_STALE_WARN_HOURS`]; `0` turns the staleness
    /// warning off.
    pub stale_warn_hours: u64,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            on_save: true,
            interval_mins: 0,
            local_enabled: false,
            local_interval_hours: 0,
            local_keep: DEFAULT_LOCAL_KEEP,
            stale_warn_hours: DEFAULT_STALE_WARN_HOURS,
        }
    }
}

impl BackupConfig {
    /// Load the `[backup]` section, layering the open project's `[quill.backup]`
    /// override over the global config (#887/#710). Best-effort: a missing or
    /// unreadable file yields the defaults (backup off).
    pub fn load() -> Self {
        load_section("backup")
    }

    /// Whether a save should trigger a backup: only when backup is enabled and
    /// the on-save trigger is on (#887).
    pub const fn backs_up_on_save(&self) -> bool {
        self.enabled && self.on_save
    }

    /// The timed backup cadence, if enabled and a positive interval is set (#887).
    /// `None` when backup is off or `interval_mins` is `0` (the timed trigger off).
    pub fn interval(&self) -> Option<std::time::Duration> {
        (self.enabled && self.interval_mins > 0)
            .then(|| std::time::Duration::from_secs(self.interval_mins * 60))
    }

    /// The scheduled local-archive cadence, if local backups are on and a positive
    /// hourly interval is set (#888). `None` when `local_enabled` is off or
    /// `local_interval_hours` is `0` — either leaves the local cadence off.
    pub fn local_interval(&self) -> Option<std::time::Duration> {
        (self.local_enabled && self.local_interval_hours > 0)
            .then(|| std::time::Duration::from_secs(self.local_interval_hours * 3600))
    }

    /// How many local archives to keep per project (#888): `None` when pruning is
    /// off (`local_keep == 0`, keep everything), else the retention count.
    pub fn local_retention(&self) -> Option<usize> {
        (self.local_keep > 0).then_some(self.local_keep)
    }

    /// The staleness window past which a local backup is considered old (#888):
    /// `None` when the warning is off (`stale_warn_hours == 0`), else the window as
    /// a duration. Used on open to warn the author their last backup is old.
    pub fn stale_after(&self) -> Option<std::time::Duration> {
        (self.stale_warn_hours > 0)
            .then(|| std::time::Duration::from_secs(self.stale_warn_hours * 3600))
    }
}

/// The `[general]` section (#837): app-level flags that aren't editor behavior,
/// a theme, or a project setting. Today it carries only the first-run marker.
#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct GeneralConfig {
    /// Whether the first-run quick-start card (#837) has been shown and dismissed.
    /// Absent / `false` on a fresh install, so the card shows once on first launch;
    /// set to `true` the moment it's dismissed, so it never reappears. Lives in the
    /// global home config (not a project's `book.toml`), so it's per-install: a new
    /// project doesn't re-trigger onboarding.
    pub onboarding_shown: bool,
}

impl GeneralConfig {
    /// Load the `[general]` section from the home config file. Best-effort: a
    /// missing or unreadable file yields the defaults (onboarding not yet shown).
    pub fn load() -> Self {
        load_section("general")
    }
}

/// Load the color theme from the `[theme]` section (#300): an optional `name`
/// (`dark` / `light`) plus any per-field color overrides, all string values.
/// Returns the theme and any warnings (bad name / field / color) for the caller
/// to surface. A missing section (or a malformed one) yields the default theme.
pub fn load_theme() -> (Theme, Vec<String>) {
    // The whole `[theme]` table as string keys; `name` is pulled out, the rest
    // are color-field overrides. Untyped (string values) so an unknown key is a
    // reported warning, not a parse failure.
    let mut table: HashMap<String, String> = load_section("theme");
    let name = table.remove("name");
    Theme::from_config(name.as_deref(), &table)
}

/// The configured `[theme] name`, or `None` when unset (the default `dark` shows
/// through). Read by the plain Settings menu (#592) to mark the active theme in
/// its picker without re-parsing the whole `[theme]` table for its colors.
pub fn theme_name() -> Option<String> {
    let table: HashMap<String, String> = load_section("theme");
    table.get("name").cloned()
}

/// The configured `[theme]` color overrides — the raw string value set for each
/// color field, excluding `name` (#1109). Read by the plain Settings menu's
/// theme-color editor to show which fields the author has overridden (and to
/// what) alongside the base theme's colors. Empty when nothing is overridden.
pub fn theme_color_overrides() -> HashMap<String, String> {
    let mut table: HashMap<String, String> = load_section("theme");
    table.remove("name");
    table
}

/// An author-defined text snippet (#411): a `name` and the literal `text` it
/// expands to. Configured in the `[snippets]` table and inserted verbatim from
/// the snippet picker — no transformation, no templating, with one exception:
///
/// The text may contain a single caret marker `$0` (#823). On insertion the
/// marker is consumed (never inserted) and the cursor lands where it was, so a
/// template can position the cursor mid-snippet (e.g. `"“$0”"` drops the cursor
/// between the quotes). Only the first `$0` is a caret; any later ones insert
/// literally. To insert a literal `$0`, escape it as `$$0` — that yields the two
/// characters `$0` with no caret. A snippet with no marker inserts exactly as
/// before, cursor at the end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub name: String,
    pub text: String,
}

/// Load the `[snippets]` section (#411): a table mapping each snippet's name to
/// its literal expansion (a string). Returns the snippets, sorted by name for a
/// stable menu, plus a warning for any non-string entry (reported and skipped,
/// like the other config sections). A missing or empty table yields nothing.
pub fn load_snippets() -> (Vec<Snippet>, Vec<String>) {
    // Untyped values (`toml::Value`) so a malformed entry (e.g. an array) is a
    // reported, skipped warning rather than failing the whole section's parse.
    let table: HashMap<String, toml::Value> = load_section("snippets");
    snippets_from_table(table)
}

/// Turn a parsed `[snippets]` table into sorted snippets plus a warning per
/// non-string entry. Separate from [`load_snippets`] so the rule is testable
/// without a config file on disk (like `validate_triggers` for workflows).
fn snippets_from_table(table: HashMap<String, toml::Value>) -> (Vec<Snippet>, Vec<String>) {
    let mut snippets = Vec::new();
    let mut errors = Vec::new();
    for (name, value) in table {
        match value.as_str() {
            Some(text) => snippets.push(Snippet {
                name,
                text: text.to_string(),
            }),
            None => errors.push(format!("snippet '{name}' must be a string")),
        }
    }
    snippets.sort_by(|a, b| a.name.cmp(&b.name));
    (snippets, errors)
}

/// The `[projects]` section (#272): where the project browser looks for books
/// beyond the recently-opened list.
#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct ProjectsConfig {
    /// A folder holding your projects (e.g. `~/Writing`); the project browser
    /// lists every project under it. `~` expands to the home directory. Absent
    /// when unset — the browser then shows only recent projects.
    pub home: Option<String>,
    /// Whether a newly-created project offers the setup wizard (#282). Defaults
    /// to on; set `setup_wizard = false` to never be prompted.
    pub setup_wizard: Option<bool>,
    /// Whether a bare `quill` launch (no path, not inside a project) reopens the
    /// most-recently-opened project directly instead of showing the create/open
    /// chooser (#613). Off by default (absent) — the chooser still appears. Set
    /// `open_last = true` to skip straight into the last project.
    pub open_last: Option<bool>,
}

/// The `[author]` section (#1241): the author's contact details, written once
/// in the home config and set at the top of every short story submission
/// manuscript. Every field is optional; with none set the manuscript still
/// exports, carrying just the book's author name.
#[derive(Clone, Deserialize, Default)]
#[serde(default)]
pub struct AuthorConfig {
    /// The author's legal name for the contact block, when it differs from the
    /// byline (`author` in `book.toml`).
    pub name: Option<String>,
    /// The postal address, one line per entry.
    pub address: Vec<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
}

impl AuthorConfig {
    /// Load the `[author]` section from the home config file (best-effort).
    pub fn load() -> Self {
        load_section("author")
    }

    /// The contact block's lines, in order: the name (falling back to `byline`),
    /// the address lines, then email and phone. Empty when nothing is known.
    pub fn contact_lines(&self, byline: Option<&str>) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(name) = self
            .name
            .as_deref()
            .or(byline)
            .filter(|n| !n.trim().is_empty())
        {
            lines.push(name.trim().to_string());
        }
        lines.extend(
            self.address
                .iter()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
        );
        lines.extend(
            [&self.email, &self.phone]
                .into_iter()
                .flatten()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        );
        lines
    }
}

impl ProjectsConfig {
    /// Load the `[projects]` section from the home config file. Best-effort: a
    /// missing or unreadable file yields the defaults (no projects home).
    pub fn load() -> Self {
        load_section("projects")
    }

    /// The configured projects-home as an absolute path with `~` expanded, if set.
    pub fn home_dir(&self) -> Option<PathBuf> {
        self.home.as_deref().map(expand_tilde)
    }

    /// Whether a new project should offer the setup wizard (#282); on by default.
    pub fn wizard_enabled(&self) -> bool {
        self.setup_wizard.unwrap_or(true)
    }

    /// Whether a bare launch should reopen the most-recent project directly (#613);
    /// off by default (opt in with `open_last = true`).
    pub fn open_last_enabled(&self) -> bool {
        self.open_last.unwrap_or(false)
    }
}

/// Expand a leading `~` / `~/` to the home directory; leave any other path as-is.
fn expand_tilde(path: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if path == "~" {
        if let Some(h) = home() {
            return h;
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Some(h) = home() {
            return h.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_pair_is_off_by_default_and_opt_in() {
        // Auto-pair ships off (#1007) so authors who type both marks are unaffected;
        // opt in with `auto_pair = true` under `[editor]`.
        assert!(
            !EditorConfig::default().flags.auto_pair,
            "auto_pair is off by default"
        );
        let on: EditorConfig = toml::from_str("auto_pair = true").unwrap();
        assert!(on.flags.auto_pair, "opt in with auto_pair = true");
        // Turning it on doesn't disturb the other opt-ins (smart typography stays
        // at its own default).
        assert!(
            on.flags.smart_typography,
            "unrelated defaults are untouched"
        );
    }

    #[test]
    fn the_daily_nudge_is_on_by_default_and_opt_out() {
        // The below-target cue ships on (#1012) so the pace feedback is ambient;
        // opt out with `daily_nudge = false` under `[editor]`.
        assert!(
            EditorConfig::default().flags.daily_nudge,
            "daily_nudge is on by default"
        );
        let off: EditorConfig = toml::from_str("daily_nudge = false").unwrap();
        assert!(!off.flags.daily_nudge, "opt out with daily_nudge = false");
        // Turning it off doesn't disturb the other defaults.
        assert!(
            off.flags.smart_typography,
            "unrelated defaults are untouched"
        );
    }

    #[test]
    fn a_config_section_with_unknown_future_keys_loads_cleanly() {
        // COMPAT (#1097): a config section written by a newer Quill carries keys
        // this version has never heard of. Every config struct is `#[serde(default)]`
        // and none uses `deny_unknown_fields`, so an unknown key is ignored, not a
        // parse failure — an older Quill opens a newer config with no error and no
        // loss of the fields it understands. This test pins that promise so a future
        // `deny_unknown_fields` can't silently break every existing config.
        let editor: EditorConfig =
            toml::from_str("vim = true\ncolumn_width = 90\nfuture_editor_feature = \"ignored\"\n")
                .expect("unknown config keys must not fail the load");
        assert!(editor.flags.vim, "known key applied");
        assert_eq!(editor.column_width, 90, "known key applied");
        assert!(
            editor.flags.smart_typography,
            "unset known key keeps its default"
        );

        // The behavior on re-save is DROP (config structs are typed and only
        // deserialized here — Quill never rewrites the home config wholesale), and
        // it holds across the other sections too.
        let backup: BackupConfig =
            toml::from_str("enabled = true\nfuture_backup_field = 7\n").unwrap();
        assert!(
            backup.enabled,
            "unknown key alongside a known one is ignored"
        );
        let projects: ProjectsConfig =
            toml::from_str("open_last = true\nfuture = \"x\"\n").unwrap();
        assert!(projects.open_last_enabled());
    }

    #[test]
    fn snippets_parse_sorts_strings_and_reports_non_strings() {
        let table = toml::from_str("zeta = \"the end\"\nalpha = \"* * *\"\nbad = 42\n").unwrap();
        let (snippets, errors) = snippets_from_table(table);
        // Kept entries are the string values, sorted by name for a stable menu.
        assert_eq!(
            snippets,
            vec![
                Snippet {
                    name: "alpha".into(),
                    text: "* * *".into()
                },
                Snippet {
                    name: "zeta".into(),
                    text: "the end".into()
                },
            ],
        );
        // The non-string entry is reported (and skipped), not a parse failure.
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("bad"), "{errors:?}");
    }

    #[test]
    fn an_empty_snippets_table_yields_nothing() {
        let (snippets, errors) = snippets_from_table(HashMap::new());
        assert!(snippets.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn projects_home_parses_and_expands_tilde() {
        let cfg: ProjectsConfig = toml::from_str("home = \"~/Writing\"").unwrap();
        let dir = cfg.home_dir().expect("home set");
        assert!(dir.is_absolute(), "~ expanded to an absolute path: {dir:?}");
        assert!(dir.ends_with("Writing"));
    }

    #[test]
    fn projects_home_passes_an_absolute_path_through() {
        let cfg = ProjectsConfig {
            home: Some("/books".into()),
            ..Default::default()
        };
        assert_eq!(cfg.home_dir(), Some(PathBuf::from("/books")));
    }

    #[test]
    fn the_setup_wizard_is_on_by_default_and_opt_out() {
        assert!(ProjectsConfig::default().wizard_enabled(), "on by default");
        let off: ProjectsConfig = toml::from_str("setup_wizard = false").unwrap();
        assert!(!off.wizard_enabled(), "opt out with setup_wizard = false");
    }

    #[test]
    fn open_last_is_off_by_default_and_opt_in() {
        assert!(
            !ProjectsConfig::default().open_last_enabled(),
            "off by default"
        );
        let on: ProjectsConfig = toml::from_str("open_last = true").unwrap();
        assert!(on.open_last_enabled(), "opt in with open_last = true");
    }

    #[test]
    fn no_projects_home_is_none() {
        assert_eq!(ProjectsConfig::default().home_dir(), None);
    }

    #[test]
    fn backup_is_off_by_default() {
        let cfg = BackupConfig::default();
        assert!(!cfg.enabled, "backup is off by default (#887)");
        assert!(
            !cfg.backs_up_on_save(),
            "no on-save backup while disabled, even though on_save defaults true"
        );
        assert_eq!(cfg.interval(), None, "no timed cadence while disabled");
    }

    #[test]
    fn enabling_backup_opts_into_on_save_by_default() {
        // `enabled = true` alone gives the common "back up whenever I save" case:
        // on_save defaults on, the timed interval stays off (#887).
        let cfg: BackupConfig = toml::from_str("enabled = true").unwrap();
        assert!(
            cfg.backs_up_on_save(),
            "on_save is on by default once enabled"
        );
        assert_eq!(
            cfg.interval(),
            None,
            "timed cadence off until interval_mins is set"
        );
    }

    #[test]
    fn on_save_can_be_opted_out_while_a_timed_interval_drives_backups() {
        let cfg: BackupConfig =
            toml::from_str("enabled = true\non_save = false\ninterval_mins = 15").unwrap();
        assert!(!cfg.backs_up_on_save(), "on_save explicitly off");
        assert_eq!(
            cfg.interval(),
            Some(std::time::Duration::from_secs(15 * 60)),
            "the timed interval is minutes → a duration"
        );
    }

    #[test]
    fn a_zero_interval_leaves_the_timed_trigger_off() {
        let cfg: BackupConfig = toml::from_str("enabled = true\ninterval_mins = 0").unwrap();
        assert_eq!(
            cfg.interval(),
            None,
            "interval_mins = 0 is the timed trigger off"
        );
    }

    #[test]
    fn a_disabled_project_gates_off_a_set_interval() {
        // Even a positive interval yields no cadence while backup is disabled — the
        // whole feature is opt-in behind `enabled` (#887).
        let cfg = BackupConfig {
            enabled: false,
            interval_mins: 30,
            ..Default::default()
        };
        assert_eq!(cfg.interval(), None);
        assert!(!cfg.backs_up_on_save());
    }

    #[test]
    fn local_backups_are_off_by_default() {
        let cfg = BackupConfig::default();
        assert!(!cfg.local_enabled, "local backups off by default (#888)");
        assert_eq!(cfg.local_interval(), None, "no local cadence while off");
        // The remote push and the local cadence are independent: enabling the
        // remote must not turn on the local archive.
        let remote_only: BackupConfig = toml::from_str("enabled = true").unwrap();
        assert!(
            !remote_only.local_enabled,
            "enabling the remote push leaves local backups off"
        );
        assert_eq!(remote_only.local_interval(), None);
    }

    #[test]
    fn enabling_local_backups_needs_a_positive_interval() {
        // `local_enabled` alone isn't a cadence — a positive `local_interval_hours`
        // is what schedules archives (#888).
        let no_interval: BackupConfig = toml::from_str("local_enabled = true").unwrap();
        assert_eq!(
            no_interval.local_interval(),
            None,
            "local_enabled with no interval is still off"
        );
        let cfg: BackupConfig =
            toml::from_str("local_enabled = true\nlocal_interval_hours = 6").unwrap();
        assert_eq!(
            cfg.local_interval(),
            Some(std::time::Duration::from_secs(6 * 3600)),
            "hours → a duration"
        );
    }

    #[test]
    fn local_retention_defaults_and_zero_keeps_everything() {
        assert_eq!(
            BackupConfig::default().local_retention(),
            Some(DEFAULT_LOCAL_KEEP),
            "a sensible default retention"
        );
        let keep_all: BackupConfig = toml::from_str("local_keep = 0").unwrap();
        assert_eq!(
            keep_all.local_retention(),
            None,
            "local_keep = 0 keeps everything rather than deleting all"
        );
        let keep_three: BackupConfig = toml::from_str("local_keep = 3").unwrap();
        assert_eq!(keep_three.local_retention(), Some(3));
    }

    #[test]
    fn stale_window_defaults_and_can_be_disabled() {
        assert_eq!(
            BackupConfig::default().stale_after(),
            Some(std::time::Duration::from_secs(
                DEFAULT_STALE_WARN_HOURS * 3600
            )),
            "a default staleness window"
        );
        let off: BackupConfig = toml::from_str("stale_warn_hours = 0").unwrap();
        assert_eq!(off.stale_after(), None, "0 turns the warning off");
        let custom: BackupConfig = toml::from_str("stale_warn_hours = 48").unwrap();
        assert_eq!(
            custom.stale_after(),
            Some(std::time::Duration::from_secs(48 * 3600))
        );
    }
}

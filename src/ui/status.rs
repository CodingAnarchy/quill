//! Status-bar tags and the per-mode status-line text (#409/#54/#132/#299/#1011/…).
//! The bottom line's content: the editor bar (chapter name, save state, and the
//! persistent AI / version-control / sprint / goal tags), the browse bar, the
//! find/replace prompt line, and the Vim mode chip. Split out of `ui/mod.rs` to
//! keep the render glue under the ~1,000-line convention; `StatusBar::render`
//! (in `mod.rs`) assembles these into the drawn line.

use std::ptr;

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::buffer::count_words;
use crate::mode::{Mode, PromptState, Search, SearchStage};
use crate::project::length_category;
use crate::theme::Theme;
use crate::wrap::{row_starts_first_indent, visual_pos};

/// The most rows a wrapped prompt grows to. A long annotation note wraps upward
/// over the dimmed editor instead of running off the right edge; past this cap it
/// scrolls to keep the caret's row in view rather than swallowing the screen.
const MAX_PROMPT_ROWS: usize = 8;

/// The prompt's soft-wrapped layout at `width` cells: the byte offsets into
/// `input` where each visual row starts (row 0 follows the label, so it's
/// narrower by the label's width), and the caret's `(row, column)` with the
/// label counted on row 0. A caret sitting just past a full final row gets a
/// fresh empty row of its own, so the block cursor never falls off the edge.
fn prompt_layout(prompt: &PromptState, width: u16) -> (Vec<usize>, (usize, usize)) {
    let width = usize::from(width);
    let label_w = prompt.label.width();
    let mut starts = row_starts_first_indent(&prompt.input, width, label_w);
    let (mut row, mut col) = visual_pos(&prompt.input, &starts, prompt.caret);
    if row == 0 {
        col += label_w;
    }
    if width > 0 && col >= width {
        row += 1;
        col = 0;
        if row == starts.len() {
            starts.push(prompt.input.len());
        }
    }
    (starts, (row, col))
}

/// How many rows the prompt occupies on a `screen`, counting up from its bottom
/// line: the wrapped row count, capped at [`MAX_PROMPT_ROWS`] and the screen.
pub(super) fn prompt_height(prompt: &PromptState, screen: Rect) -> u16 {
    let (starts, _) = prompt_layout(prompt, screen.width);
    let rows = starts.len().min(MAX_PROMPT_ROWS);
    u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .min(screen.height)
        .max(1)
}

/// Draw the open prompt, word-wrapped: the label then the typed text, growing
/// upward from the status line `status` (whose bottom is the screen's) as the
/// input wraps, with the block cursor at the caret (#706). y/N confirmations
/// carry no input, so they stay one row with the caret right after the label.
pub(super) fn draw_prompt(
    frame: &mut Frame,
    status: Rect,
    screen: Rect,
    prompt: &PromptState,
    theme: &Theme,
) {
    let style = Style::default().fg(theme.on_accent).bg(theme.match_current);
    let (starts, (caret_row, caret_col)) = prompt_layout(prompt, status.width);
    let height = prompt_height(prompt, screen);
    let shown = usize::from(height);
    // Scroll so the caret's row stays visible once the note outgrows the cap.
    let first = (caret_row + 1).saturating_sub(shown);
    let area = Rect {
        x: status.x,
        y: status.bottom().saturating_sub(height),
        width: status.width,
        height,
    };

    let input = prompt.input.as_str();
    let lines: Vec<Line> = (first..(first + shown).min(starts.len()))
        .map(|i| {
            let end = starts.get(i + 1).copied().unwrap_or(input.len());
            let text = Span::styled(&input[starts[i]..end], style);
            if i == 0 {
                Line::from(vec![Span::styled(prompt.label.as_str(), style), text])
            } else {
                Line::from(text)
            }
        })
        .collect();
    // Wipe the editor rows the prompt grows over, so no prose shows beside a short row.
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines), area);

    let col = area
        .x
        .saturating_add(u16::try_from(caret_col).unwrap_or(u16::MAX))
        .min(area.right().saturating_sub(1));
    let row = area
        .y
        .saturating_add(u16::try_from(caret_row - first).unwrap_or(0));
    frame.set_cursor_position((col, row));
}

/// The status-bar line for the find/replace mode (#144): the query, a match
/// count, and the keys available at the current step.
pub(super) fn search_status(s: &Search) -> String {
    let count = if s.matches.is_empty() {
        if s.query.is_empty() {
            String::new()
        } else {
            "  no matches".to_string()
        }
    } else {
        format!("  {}/{}", s.current + 1, s.matches.len())
    };
    // The active match modes (#570): a short tag after the query, e.g. "[Aa w]".
    let modes = match s.opts.label() {
        ref m if m.is_empty() => String::new(),
        m => format!(" {m}"),
    };
    match s.stage {
        // Vim search (#206) shows its `/` / `?` prompt; the Ctrl-F find labels itself.
        SearchStage::Query if s.from_vim => {
            format!(" {}{}{count}", if s.backward { '?' } else { '/' }, s.query)
        }
        SearchStage::Query => {
            format!(
                " Find: {}{modes}{count}  · ^C case · ^W word · ^R regex",
                s.query
            )
        }
        SearchStage::Navigate => {
            format!(
                " Find: {}{modes}{count}  · n next · N prev · r replace · ^C case · ^W word · ^R regex · Esc done",
                s.query
            )
        }
        SearchStage::Replacement => {
            format!(" Replace '{}'{modes} with: {}", s.query, s.replacement)
        }
        SearchStage::Step => format!(
            " Replace '{}' → '{}'?{count}  · y · n skip · a all · Esc done ({} done)",
            s.query, s.replacement, s.replaced
        ),
    }
}

/// Save-state indicator (#409): ● when there are unsaved changes, ✓ when the
/// buffer is on disk — so persistence is legible at a glance.
const fn dirty_marker(dirtied: bool) -> &'static str {
    if dirtied {
        " ●"
    } else {
        " ✓"
    }
}

/// Persistent AI indicator (independent of the transient message, which clears
/// on the next keystroke): in-flight spinner, else an enabled tag. While a
/// request is in flight, name the backend it's actually running on (#372) —
/// `⟳ Claude` or `⟳ local` — so a routed pass is visible.
fn ai_tag(app: &App) -> String {
    if app.ai_busy() {
        format!(" · ⟳ {}", app.ai_in_flight_label())
    } else if app.ai.enabled {
        " · AI".to_string()
    } else {
        String::new()
    }
}

/// Version-control indicator (#54/#132): a clean repo shows "git", an
/// uncommitted one shows the changed-file count. When automatic backup is on and
/// has succeeded this session, a "backed up N ago" note (#887) tells the author
/// how current the off-site copy is.
fn vc_tag(app: &App) -> String {
    let base = match (app.under_vc, app.vc_changes) {
        (false, _) => return String::new(),
        (true, 0) => " · git".to_string(),
        (true, n) => format!(" · git ●{n}"),
    };
    match app.last_backup_age() {
        Some(age) => format!("{base} · backed up {}", backup_age_label(age)),
        None => base,
    }
}

/// A compact "how long ago the last backup succeeded" label (#887): seconds under
/// a minute, then minutes, then hours — enough for the author to see the off-site
/// copy is current without a precise timestamp cluttering the status bar.
fn backup_age_label(age: std::time::Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// Scheduled local-backup indicator (#888): once a local archive has landed this
/// session, a "saved N ago" note tells the author how current the on-disk copy is.
/// Independent of the git/remote `vc_tag` — a local archive needs no repo, so it's
/// shown on its own. Empty until the first local backup succeeds (or when local
/// backups are off).
fn local_backup_tag(app: &App) -> String {
    app.last_local_backup_age().map_or_else(String::new, |age| {
        format!(" · saved {}", backup_age_label(age))
    })
}

/// Persistent read-only indicator (#885): while the project is open read-only —
/// held by a live other Quill instance (#871) — a "read-only" chip stays in the
/// status bar regardless of the transient message. The open-time explanation
/// (and the first-run onboarding card that can overlay it) is momentary; this
/// tag keeps the author aware that saves are refused for as long as the state
/// lasts, and disappears the moment a stale lock is reclaimed (#890).
fn read_only_tag(app: &App) -> String {
    if app.project.is_read_only() {
        " · read-only".to_string()
    } else {
        String::new()
    }
}

/// A running writing sprint (#299): live countdown + net words written, always
/// visible (independent of the transient message on the right).
fn sprint_tag(app: &App, active: usize) -> String {
    app.sprint.as_ref().map_or_else(String::new, |s| {
        let words = s.words(app.project_word_count_with_active(active));
        format!(
            " · {}",
            crate::app::sprint::status_segment(s.remaining(), words)
        )
    })
}

/// The active chapter's stored word goal (#1011): a live fill bar toward the
/// front-matter `goal` as the author writes, shown only when the chapter has a
/// goal set. The goal is the value cached on the chapter entry at open — never a
/// per-draw file read — and the count is the live active-chapter count (the
/// buffer, so it moves as you type), so this stays O(1) per frame.
fn chapter_goal_tag(app: &App, active: usize) -> String {
    app.project
        .chapter_goal(active)
        .map_or_else(String::new, |goal| {
            // `active` is resolved once by the caller (`active_index` is an O(n)
            // scan), so the goal and the count share the one lookup.
            let words = app.chapter_word_count_with_active(active, active);
            format!(
                " · {}",
                crate::app::sprint::chapter_goal_segment(words, goal)
            )
        })
}

/// A per-session word target (#1011): a live fill bar toward the words-this-session
/// goal, shown only when the author has set one. The words-written figure diffs
/// the maintained running project total against the session baseline (O(1)), so it
/// fills as words are added without any per-draw file work.
fn session_target_tag(app: &App, active: usize) -> String {
    app.session_target.as_ref().map_or_else(String::new, |t| {
        let written = t.written(app.project_word_count_with_active(active));
        format!(
            " · {}",
            crate::app::sprint::session_target_segment(written, t.target())
        )
    })
}

/// The ambient below-target cue (#1012): when the daily nudge is on, a daily
/// target is set, and today's net is under it, a quiet tag shows how far short
/// the day is (e.g. "◈ 180 to daily goal"). Hidden or restored with the "Daily
/// goal cue" toggle, never by a stray Esc.
/// Reads only the cached daily tally + goal (`daily_target_shortfall`), so it's
/// O(1) per frame — no manuscript walk. Empty (nothing rendered) whenever the cue
/// shouldn't show, so the bar is unchanged for authors who never set a target or
/// who opted out.
fn daily_nudge_tag(app: &App) -> String {
    if !app.nudge_showing() {
        return String::new();
    }
    app.daily_target_shortfall()
        .map_or_else(String::new, |short| format!(" · ◈ {short} to daily goal"))
}

/// While browsing, the bar describes the *selected* chapter: its title, and
/// (unless a transient message is up) its summary / POV / status.
pub(super) fn nav_status_bar(
    app: &App,
    selected: usize,
    filter: &str,
    typing: bool,
) -> (String, String) {
    let title = app.project.chapter_title(selected).unwrap_or("[untitled]");
    // A quick-jump filter (#410) takes over the bar: the live query and a
    // match count on the left, the highlighted chapter and the keys on the
    // right. `/` enters this; Esc keeps the filter (commands now act on
    // the narrowed list), a second Esc clears it.
    if typing || !filter.is_empty() {
        let n = app.nav_match_count();
        let count = match n {
            0 => "  no matches".to_string(),
            1 => "  1 match".to_string(),
            n => format!("  {n} matches"),
        };
        let hint = if typing {
            "  · Enter open · Esc keep filter"
        } else {
            "  · Esc clears · / refine"
        };
        (format!(" /{filter}{count}"), format!("{title}{hint}  "))
    } else {
        let right = app.message.as_ref().map_or_else(
            || {
                let mut parts = Vec::new();
                if let Some(s) = app.project.chapter_summary(selected) {
                    parts.push(s.to_string());
                }
                if let Some(p) = app.project.chapter_pov(selected) {
                    parts.push(format!("POV: {p}"));
                }
                if let Some(st) = app.project.chapter_status(selected) {
                    parts.push(st.label().to_string());
                }
                if parts.is_empty() {
                    String::new()
                } else {
                    format!("{}  ", parts.join("  ·  "))
                }
            },
            |m| format!("{m}  "),
        );
        (format!(" {title}{}", read_only_tag(app)), right)
    }
}

/// The editor bar: the active chapter's name and save state, the persistent
/// AI / version-control / sprint tags, and a word/line count (or a transient
/// message) on the right.
pub(super) fn editor_status_bar(app: &App, active: usize) -> (String, String) {
    let buffer = app.edit_buffer();
    // The chapter the cursor is in: in continuous view that's the segment
    // under the cursor (with a marker); otherwise the single active chapter.
    let (name, dirtied) = app.manuscript.as_ref().map_or_else(
        // The focused buffer's own title — usually the active chapter, but
        // a reference pane swapped into focus (#165) shows its file name.
        || (app.pane_title_for_with_active(buffer, active), buffer.dirty),
        |m| (format!("≡ {}", m.current_title()), m.dirty()),
    );
    let dirty = dirty_marker(dirtied);
    let right = app.message.as_ref().map_or_else(
        || {
            // In Visual mode, tally the selection itself (#824) — "how long is
            // this passage I'm about to cut?" — scoped to the selected bytes
            // rather than the whole document.
            if let Mode::Visual { anchor, linewise } = app.mode {
                let (lo, hi) = app.selection_bytes(anchor, linewise);
                let selected = buffer.byte_slice(lo, hi);
                let words = count_words(&selected);
                let chars = selected.chars().count();
                return format!(
                    "selected {words} words · {chars} chars · ln {}/{}  ",
                    buffer.cursor.line + 1,
                    buffer.line_count()
                );
            }
            let words = buffer.word_count();
            // A short story names its length category beside the count (#1239) —
            // only for the story itself, not a reference file swapped into focus.
            let category = if app.manuscript.is_none()
                && ptr::eq(buffer, &app.buffer)
                && app.project.manifest.story_chapter() == Some(active)
            {
                format!(" · {}", length_category(words))
            } else {
                String::new()
            };
            format!(
                "{words} words{category} · ln {}/{}  ",
                buffer.cursor.line + 1,
                buffer.line_count()
            )
        },
        |m| format!("{m}  "),
    );
    let left = format!(
        " {name}{dirty}{}{}{}{}{}{}{}{}",
        read_only_tag(app),
        vc_tag(app),
        local_backup_tag(app),
        ai_tag(app),
        sprint_tag(app, active),
        chapter_goal_tag(app, active),
        session_target_tag(app, active),
        daily_nudge_tag(app)
    );
    (left, right)
}

/// Vim mode (#27): a chip at the far left naming the editing mode. Only while
/// actually in the editor (not browsing or editing a metadata field).
pub(super) fn mode_chip(app: &App) -> Option<Span<'static>> {
    let theme = app.theme;
    match app.mode {
        _ if !app.flags.vim => None, // Vim mode off: no chip
        Mode::Insert => Some(("INSERT", theme.mode_insert)),
        Mode::Visual {
            linewise: false, ..
        } => Some(("VISUAL", theme.mode_visual)),
        Mode::Visual { linewise: true, .. } => Some(("VISUAL LINE", theme.mode_visual)),
        Mode::Normal => Some(("NORMAL", theme.mode_normal)),
        _ => None, // not in the editor (browsing, prompt, …)
    }
    .map(|(label, bg)| {
        Span::styled(
            format!(" {label} "),
            Style::default()
                .fg(theme.on_accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )
    })
}

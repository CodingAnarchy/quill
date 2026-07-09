//! The local-backup browser (#934): the project's timestamped `.zip` archives,
//! newest first, each a "how long ago" label + its date over its human size. A
//! click-selectable list, drawn through [`draw_list_overlay_recording`].

use std::cell::RefCell;

use ratatui::{layout::Rect, Frame};

use super::{draw_list_overlay_recording, list_row};
use crate::app::ListLayout;
use crate::mode::BackupBrowser;

/// Render the local-backup browser (#934): the project's timestamped `.zip`
/// archives, newest first, each a "how long ago" label + its date over its human
/// size. Navigable; Enter restores the selected backup over the open project
/// (behind a confirm), Esc closes. `recorder` captures the row geometry for
/// click-to-select.
pub(in crate::ui) fn draw_backup_browser(
    frame: &mut Frame,
    area: Rect,
    browser: &BackupBrowser,
    recorder: &RefCell<ListLayout>,
) {
    let title = format!(" Backups ({}) ", browser.entries.len());
    let now = std::time::SystemTime::now();
    let rows = browser
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let when = backup_when(e.modified, now);
            list_row(
                i == browser.selected,
                &when,
                Some(format!("{}  ·  {}", human_size(e.size), e.name)),
            )
        })
        .collect();
    let help = "Backups — ↑↓ move · enter restore · esc close";
    draw_list_overlay_recording(
        frame,
        area,
        &title,
        help,
        rows,
        Some(browser.selected),
        Some(recorder),
    );
}

/// A backup's "when" label for the browser (#934): its local date-time, with a
/// coarse "how long ago" suffix so recency reads at a glance. Falls back to just
/// the age (or "unknown time" for a clock skew that puts the archive in the future)
/// when the timestamp can't be formatted.
fn backup_when(modified: std::time::SystemTime, now: std::time::SystemTime) -> String {
    let ago = now
        .duration_since(modified)
        .map(|d| format!(" ({} ago)", backup_age(d)))
        .unwrap_or_default();
    let offset = time::OffsetDateTime::now_local()
        .map(time::OffsetDateTime::offset)
        .unwrap_or(time::UtcOffset::UTC);
    let local = time::OffsetDateTime::from(modified).to_offset(offset);
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
    match local.format(&fmt) {
        Ok(s) => format!("{s}{ago}"),
        Err(_) if ago.is_empty() => "unknown time".to_string(),
        Err(_) => ago.trim().trim_matches(['(', ')']).to_string(),
    }
}

/// A compact "how long ago" label for a backup's age (#934): minutes under an
/// hour, hours under a day, then days. Coarse on purpose — the browser wants
/// "recent vs. old" at a glance, with the exact date already alongside it.
fn backup_age(age: std::time::Duration) -> String {
    let mins = age.as_secs() / 60;
    if mins < 60 {
        format!("{mins}m")
    } else if mins < 60 * 24 {
        format!("{}h", mins / 60)
    } else {
        format!("{}d", mins / (60 * 24))
    }
}

/// A backup's size as a compact human-readable string (#934): bytes, then KB / MB
/// as the size grows, one decimal place past a kilobyte. A backup is text, so this
/// tops out in the low megabytes for even a large novel.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::human_size;

    #[test]
    fn human_size_reads_bytes_then_kb_then_mb() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}

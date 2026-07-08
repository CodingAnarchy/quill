//! Browse & restore local backups (#934): the run-action that lists the project's
//! timestamped `.zip` archives (dates + sizes, newest first) and restores a chosen
//! one over the open project, plus the on-open damage offer.
//!
//! Hermetic (#531): the backups dir is a per-test scratch dir pointed at by
//! `config::set_data_dir`, seeded with real archives written by `archive::write_archive`
//! (so a restore actually validates and unpacks a manifest), and the clock is only
//! ever the real one through backdated mtimes — no network, no developer data dir.

use super::*;
use crate::mode::{Action, Mode, Pending};
use crate::{archive, config};
use std::time::{Duration, SystemTime};

/// Point the data dir at a fresh scratch dir for this test and return the project's
/// backups dir (created), so seeded archives land where `list_local_backups` reads.
fn with_backups_dir(app: &App, tag: &str) -> (PathBuf, PathBuf) {
    let data = app_scratch_dir(tag);
    config::set_data_dir(Some(data.clone()));
    let bdir = config::backups_dir(&app.project.root).unwrap();
    fs::create_dir_all(&bdir).unwrap();
    (data, bdir)
}

/// Write a real archive of `app`'s project into the backups dir under `name`, then
/// backdate its mtime to `age_secs` ago so the newest-first ordering is deterministic
/// without waiting real time.
fn seed_backup(app: &App, bdir: &Path, name: &str, age_secs: u64) -> PathBuf {
    let path = bdir.join(name);
    archive::write_archive(&app.project.root, &path).unwrap();
    let when = SystemTime::now() - Duration::from_secs(age_secs);
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(when)
        .unwrap();
    path
}

#[test]
fn the_browser_lists_seeded_backups_newest_first() {
    let (mut app, _dir) = two_chapter_app("bb-list");
    let (data, bdir) = with_backups_dir(&app, "bb-list-data");
    // Three archives at known ages — the browser must order them newest first.
    seed_backup(&app, &bdir, "Book-2000-01-01-000000.zip", 3 * 24 * 3600); // 3 days
    seed_backup(&app, &bdir, "Book-2000-01-02-000000.zip", 24 * 3600); // 1 day
    seed_backup(&app, &bdir, "Book-2000-01-03-000000.zip", 3600); // 1 hour (newest)

    app.run_action(Action::BrowseBackups, false);
    let Mode::BackupBrowser(browser) = &app.mode else {
        panic!("the backup browser opened");
    };
    let names: Vec<&str> = browser.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "Book-2000-01-03-000000.zip",
            "Book-2000-01-02-000000.zip",
            "Book-2000-01-01-000000.zip",
        ],
        "newest first"
    );
    // Sizes are populated from the real archives on disk.
    assert!(browser.entries.iter().all(|e| e.size > 0), "sizes read");

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn no_backups_is_a_clean_no_op() {
    // The backups dir exists but is empty (local backups never ran): the browser
    // opens nothing and reports, staying in the editor (#934).
    let (mut app, _dir) = two_chapter_app("bb-empty");
    let (data, _bdir) = with_backups_dir(&app, "bb-empty-data");

    app.run_action(Action::BrowseBackups, false);
    assert!(
        matches!(app.mode, Mode::Insert),
        "no overlay opens with no backups"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("No local backups"),
        "reported the empty case: {:?}",
        app.message
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn no_data_dir_is_a_clean_no_op() {
    // With no data dir resolvable (the hermetic default), there's no backups dir at
    // all — the browser is still a clean no-op rather than an error.
    let (mut app, _dir) = two_chapter_app("bb-nodir");
    config::set_data_dir(None);
    app.run_action(Action::BrowseBackups, false);
    assert!(matches!(app.mode, Mode::Insert), "no overlay, no panic");
}

#[test]
fn restoring_a_backup_replaces_the_project_from_the_chosen_zip() {
    let (mut app, _dir) = two_chapter_app("bb-restore");
    let (data, bdir) = with_backups_dir(&app, "bb-restore-data");
    // Archive the pristine project, then mutate a chapter on disk so restore has
    // something to undo.
    let backup = seed_backup(&app, &bdir, "Book-2000-01-01-000000.zip", 3600);
    let chapter = app.project.root.join("one.md");
    assert_eq!(fs::read_to_string(&chapter).unwrap(), "first chapter\n");
    fs::write(&chapter, "CLOBBERED\n").unwrap();

    // Restore the backup over the project.
    let ok = app.restore_backup_for_test(backup);
    assert!(ok, "the project reopened at its original root");
    assert_eq!(
        fs::read_to_string(&chapter).unwrap(),
        "first chapter\n",
        "the chapter's pre-clobber content is back"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("Restored"),
        "reported the restore: {:?}",
        app.message
    );
    // Non-destructive to the working state: a safety archive of the clobbered state
    // was written before the swap, so the backups dir grew.
    let after = config::backups_dir(&app.project.root).unwrap();
    let count = fs::read_dir(&after)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "zip"))
        .count();
    assert!(count >= 2, "a pre-restore safety archive was written too");

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn enter_opens_a_confirm_before_restoring() {
    // Enter on a selected backup asks first — it doesn't restore on a stray press.
    let (mut app, _dir) = two_chapter_app("bb-confirm");
    let (data, bdir) = with_backups_dir(&app, "bb-confirm-data");
    seed_backup(&app, &bdir, "Book-2000-01-01-000000.zip", 3600);

    app.run_action(Action::BrowseBackups, false);
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ConfirmRestoreBackup)),
        "Enter opens the restore confirm"
    );
    // Declining leaves the project untouched.
    app.on_key(key(KeyCode::Char('n')));
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("cancelled"),
        "a declined restore is cancelled: {:?}",
        app.message
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn a_damaged_project_offers_restore_from_the_latest_backup_on_open() {
    let (mut app, _dir) = two_chapter_app("bb-damaged");
    let (data, bdir) = with_backups_dir(&app, "bb-damaged-data");
    seed_backup(&app, &bdir, "Book-2000-01-01-000000.zip", 3600);

    // Damage the project: delete a chapter file the manifest still references.
    fs::remove_file(app.project.root.join("two.md")).unwrap();

    app.offer_backup_restore_on_damage();
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ConfirmRestoreBackup)),
        "a damaged project offers a restore"
    );
    assert!(
        app.message.is_none(),
        "the offer is a prompt, not a passive message"
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn an_intact_project_offers_nothing_on_open() {
    // The healthy path: every chapter reads and the manifest parses, so the on-open
    // check is silent — no spurious restore prompt.
    let (mut app, _dir) = two_chapter_app("bb-intact");
    let (data, bdir) = with_backups_dir(&app, "bb-intact-data");
    seed_backup(&app, &bdir, "Book-2000-01-01-000000.zip", 3600);

    app.offer_backup_restore_on_damage();
    assert!(
        matches!(app.mode, Mode::Insert),
        "an intact project prompts nothing"
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn a_damaged_project_with_no_backup_reports_rather_than_offering() {
    // Damage but no backup to restore from: the author is told, not dropped into a
    // dead confirm.
    let (mut app, _dir) = two_chapter_app("bb-damaged-nobak");
    let (data, _bdir) = with_backups_dir(&app, "bb-damaged-nobak-data");
    fs::remove_file(app.project.root.join("two.md")).unwrap();

    app.offer_backup_restore_on_damage();
    assert!(
        matches!(app.mode, Mode::Insert),
        "no confirm without a backup"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("no local backup"),
        "reported the missing backup: {:?}",
        app.message
    );

    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

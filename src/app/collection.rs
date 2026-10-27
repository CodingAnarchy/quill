//! A collection's story list (#1244): the palette Ctrl-G opens in a collection,
//! listing its linked short stories in order with their word counts and status
//! (and a total), where the author opens, reorders, credits, unlinks, or relocates
//! a story, links an existing one, or writes a new one inside the collection.
//! Opening a story switches to its own project, so every edit lands in the story
//! itself and the collection shows it the next time it's opened.

use std::path::{Path, PathBuf};

use crate::mode::{Command, Mode, PaletteItem, Pending, StoryCommand};
use crate::project::{StoryLink, STORIES_DIR};

use super::App;

impl App {
    /// Open the story list, highlighting story `selected` when given.
    pub(crate) fn open_stories(&mut self, selected: Option<usize>) {
        if !self.project.is_collection() {
            self.message = Some("Only a collection has stories.".into());
            return;
        }
        let stories = &self.project.manifest.stories;
        let count = stories.len();
        let mut items = Vec::new();
        for (i, link) in stories.iter().enumerate() {
            items.push(story_row(i, link));
            let title = link
                .info
                .as_ref()
                .map_or(link.path.as_str(), |i| i.title.as_str())
                .to_string();
            if i > 0 {
                items.push(PaletteItem::new(
                    format!("  ↑ move “{title}” up"),
                    "earlier in the collection".to_string(),
                    Command::Story(StoryCommand::Move { index: i, up: true }),
                ));
            }
            if i + 1 < count {
                items.push(PaletteItem::new(
                    format!("  ↓ move “{title}” down"),
                    "later in the collection".to_string(),
                    Command::Story(StoryCommand::Move {
                        index: i,
                        up: false,
                    }),
                ));
            }
            items.push(PaletteItem::new(
                format!("  ✎ credit line for “{title}”"),
                "e.g. First published in Lightspeed, 2027 — the story isn't changed".to_string(),
                Command::Story(StoryCommand::Credit(i)),
            ));
            items.push(PaletteItem::new(
                format!("  ✕ remove “{title}”"),
                "unlink it from the collection; the story itself is kept".to_string(),
                Command::Story(StoryCommand::Remove(i)),
            ));
        }
        items.push(PaletteItem::new(
            "Add an existing story…".to_string(),
            "link a short story project by its folder".to_string(),
            Command::Story(StoryCommand::AddExisting),
        ));
        items.push(PaletteItem::new(
            "Write a new story…".to_string(),
            format!("a new short story in {STORIES_DIR}/, linked and opened"),
            Command::Story(StoryCommand::New),
        ));
        let words = self.project.collection_words();
        let noun = if count == 1 { "story" } else { "stories" };
        self.open_palette("Stories", items);
        // The palette title is fixed, so the totals ride the status line.
        self.message = Some(format!("{count} {noun} · {words} words in all"));
        if let (Some(index), Mode::Palette(p)) = (selected, &mut self.mode) {
            if let Some(row) = p.items.iter().position(|item| {
                matches!(
                    item.command,
                    Command::Story(StoryCommand::Open(i) | StoryCommand::Locate(i)) if i == index
                )
            }) {
                p.selected = row;
            }
        }
    }

    /// Run a story-list row (#1244).
    pub(super) fn run_story_command(&mut self, command: StoryCommand) {
        match command {
            StoryCommand::Open(i) => self.open_story(i),
            StoryCommand::Move { index, up } => match self.project.move_story(index, up) {
                Ok(to) => self.open_stories(to.or(Some(index))),
                Err(e) => self.message = Some(format!("Not moved: {e}")),
            },
            StoryCommand::Credit(index) => {
                let current = self
                    .project
                    .manifest
                    .stories
                    .get(index)
                    .and_then(|s| s.credit.clone())
                    .unwrap_or_default();
                self.open_prompt(
                    Pending::StoryCredit { index },
                    "Credit line: ",
                    &current,
                    None,
                );
            }
            StoryCommand::Remove(index) => {
                let title = self.story_title(index);
                self.open_prompt(
                    Pending::ConfirmRemoveStory { index },
                    format!(
                        "Remove “{title}” from the collection? The story itself is kept. (y/N)"
                    ),
                    "",
                    None,
                );
            }
            StoryCommand::Locate(index) => {
                let current = self
                    .project
                    .manifest
                    .stories
                    .get(index)
                    .map(|s| s.path.clone())
                    .unwrap_or_default();
                self.open_prompt(
                    Pending::LocateStory { index },
                    "Story folder: ",
                    &current,
                    None,
                );
            }
            StoryCommand::AddExisting => {
                let start = self.project.root.parent().map(prefill).unwrap_or_default();
                self.open_prompt(Pending::AddStoryPath, "Short story folder: ", &start, None);
            }
            StoryCommand::New => {
                self.open_prompt(Pending::NewStoryTitle, "New story title: ", "", None);
            }
        }
    }

    /// Open linked story `index`'s own project in place of the collection.
    fn open_story(&mut self, index: usize) {
        match self.project.story_dir(index) {
            Some(dir) if dir.exists() => {
                self.switch_to(dir);
                if self.project.manifest.is_short_story() {
                    self.message =
                        Some("Editing the story itself; Ctrl-B returns to the collection.".into());
                }
            }
            _ => self.message = Some("That story's folder is missing — locate it first.".into()),
        }
    }

    /// Link the short story project at `text` (#1244).
    pub(super) fn add_story(&mut self, text: &str) {
        if text.is_empty() {
            return self.open_stories(None);
        }
        match self.project.add_story(&expand(text)) {
            Ok(index) => {
                self.open_stories(Some(index));
                self.message = Some(format!("Added “{}”.", self.story_title(index)));
            }
            Err(e) => {
                self.open_stories(None);
                self.message = Some(format!("Not added: {e}."));
            }
        }
    }

    /// Write a new story titled `text` inside the collection and open it.
    pub(super) fn write_new_story(&mut self, text: &str) {
        if text.is_empty() {
            return self.open_stories(None);
        }
        match self.project.create_story(text) {
            Ok(dir) => {
                self.switch_to(dir);
                self.message =
                    Some("A new story in the collection; Ctrl-B returns to the collection.".into());
            }
            Err(e) => {
                self.open_stories(None);
                self.message = Some(format!("Not created: {e}."));
            }
        }
    }

    /// Set story `index`'s credit line (blank clears it).
    pub(super) fn set_story_credit(&mut self, index: usize, text: &str) {
        if let Err(e) = self.project.set_story_credit(index, text) {
            self.message = Some(format!("Not saved: {e}"));
        }
        self.open_stories(Some(index));
    }

    /// Point missing story `index` at the folder `text`.
    pub(super) fn locate_story(&mut self, index: usize, text: &str) {
        if !text.is_empty() {
            if let Err(e) = self.project.relocate_story(index, &expand(text)) {
                self.message = Some(format!("Not found: {e}."));
            }
        }
        self.open_stories(Some(index));
    }

    /// Unlink story `index` after the author confirmed; the story is untouched.
    pub(super) fn remove_story(&mut self, index: usize) {
        let title = self.story_title(index);
        match self.project.remove_story(index) {
            Ok(()) => self.message = Some(format!("Removed “{title}” from the collection.")),
            Err(e) => self.message = Some(format!("Not removed: {e}")),
        }
        self.open_stories(None);
    }

    /// Story `index`'s title, or its folder when it can't be read.
    fn story_title(&self, index: usize) -> String {
        self.project
            .manifest
            .stories
            .get(index)
            .map_or_else(String::new, |s| {
                s.info
                    .as_ref()
                    .map_or_else(|| s.path.clone(), |i| i.title.clone())
            })
    }
}

/// Story `index`'s main row: its title, words, status, and credit, opening it —
/// or, when its folder is gone, its path, offering to locate it.
fn story_row(index: usize, link: &StoryLink) -> PaletteItem {
    let n = index + 1;
    link.info.as_ref().map_or_else(
        || {
            PaletteItem::new(
                format!("{n}. (missing) {}", link.path),
                "its folder is gone — choose to locate it",
                Command::Story(StoryCommand::Locate(index)),
            )
        },
        |info| {
            let status = info
                .status
                .map_or(String::new(), |s| format!(" · {}", s.label()));
            let credit = link
                .credit
                .as_deref()
                .map_or(String::new(), |c| format!(" · {c}"));
            PaletteItem::new(
                format!("{n}. {}", info.title),
                format!("{} words{status}{credit}", info.words),
                Command::Story(StoryCommand::Open(index)),
            )
        },
    )
}

/// `dir` as a prompt prefill: its path with a trailing separator, ready for a
/// folder name to be typed after it.
fn prefill(dir: &Path) -> String {
    let mut s = dir.to_string_lossy().into_owned();
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// A typed path with a leading `~` expanded to the home directory.
fn expand(text: &str) -> PathBuf {
    match (text.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(text),
    }
}

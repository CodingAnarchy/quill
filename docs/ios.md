# Quill on iPhone

The iPhone app is a SwiftUI front end over the same Rust core the terminal
editor uses (`crates/quill-core`), reached through UniFFI bindings
(`crates/quill-ffi`). Chapters, front matter, word counts, saves, and git
sync all come from the core, so the phone and the desktop can't disagree
about a project.

```
crates/quill-core        the editing core (no terminal UI)
crates/quill-ffi         its Swift-facing API, exported with UniFFI
ios/build-core.sh        builds the core for iOS and generates the bindings
ios/test-kit.sh          runs QuillKit's Swift tests on this Mac
ios/QuillKit             Swift package: the bindings plus the app's model layer
ios/App                  the app itself (an XcodeGen spec and SwiftUI views)
```

## What you need

- **Xcode** (from the App Store), selected with
  `sudo xcode-select -s /Applications/Xcode.app`. The Command Line Tools
  alone can build and test the Mac slice (see below), but not the iOS app.
- **XcodeGen**: `brew install xcodegen`. The Xcode project is generated from
  `ios/App/project.yml` rather than checked in, so it never drifts or
  conflicts.
- **Rust**: nothing extra. `rust-toolchain.toml` lists the iOS targets, so
  rustup installs them with the pinned toolchain on the first build.

## Build the core

One command builds everything the app links against:

```sh
ios/build-core.sh
```

It compiles `quill-ffi` for the device (`aarch64-apple-ios`), the Apple-silicon
simulator (`aarch64-apple-ios-sim`), and this Mac (`aarch64-apple-darwin`, for
the Swift tests), using the `ios` Cargo profile. Then it does two things:

- generates the Swift bindings into
  `ios/QuillKit/Sources/QuillCore/QuillCore.swift`;
- packs the three static libraries, with the generated C header and module
  map, into `ios/build/QuillFFI.xcframework`.

Both are build products and git-ignored. Rerun the script after any change to
the Rust crates.

The script writes the XCFramework by hand rather than calling
`xcodebuild -create-xcframework`. That way `ios/build-core.sh --host`, which
builds only the Mac slice, works on a machine with just the Command Line Tools.

The phone build leaves out the offline `local` model: `quill-ffi` depends on
`quill-core` with default features, and `local` is off by default.

## Test

```sh
ios/test-kit.sh            # build the Mac slice, then `swift test` QuillKit
ios/test-kit.sh --no-build # reuse the last build
cargo test -p quill-ffi    # the Rust side of the bindings
```

`ios/typecheck-app.sh` typechecks the whole app (views included) without Xcode.
The macOS SDK carries UIKit for Mac Catalyst, so the app, QuillKit, and the
bindings typecheck together as one Catalyst module. It catches Swift errors
locally; CI still builds the real iOS app.

With Xcode, the UI tests (`ios/App/QuillUITests`) drive the real app in a
simulator: they open the sample, check the chapter list, type into a chapter,
background and kill the app, and find the words still there after a relaunch.

`ios/test-iphones.sh` runs them on the newest base iPhone, e, Pro, Pro Max,
and Air simulator the installed Xcode has, building once (#1289).
`--only` runs some of them (`--only pro-max`, `--only base,e,pro`), and
`--no-build` tests the last build. `PhoneLayoutUITests` checks each phone upright and sideways:

| | Upright | Sideways |
|---|---|---|
| Base iPhone, e, Pro | one column: the list, then the chapter, with a way back | one column |
| Pro Max, Air | one column | the list beside the chapter |

That follows iOS: only the widest phones get a regular width, and only
sideways. The Air's split was measured in the simulator, not assumed. A new
model in the 17 or 18 line shares one of these screens (the 18 Pro has the
17 Pro's, the 18 Pro Max the 17 Pro Max's), and the script picks the newest of
each kind. The tests tell the phone apart by its simulator's name. On these
phones, a layout check that skips fails the run. The type-then-kill check runs
upright on every phone, and sideways on the base iPhone and the Pro Max: it
checks saving, not layout, and it's the slowest check (#1310).

The iPhone Duo simulator needs Xcode 27.1 or later. Xcode 27 replaces the
Simulator app with DeviceHub (Xcode → Open Developer Tool → DeviceHub), and
the fold buttons appear when you hover over the device in DeviceHub's window.
Neither `simctl` nor XCTest can fold it, so the Duo's checks run only under
`ios/test-duo.sh`, which tells them the pose.

`ios/test-duo.sh` runs the suite in each pose: folded, unfolded (where the
unfolded check must run, not skip), half open and upright (where the book check
must run), and half open and sideways (where the laptop check must run). For
the laptop check it turns off DeviceHub's Device → Keyboard → Simulate
Hardware Keyboard, which otherwise keeps the on-screen keyboard away, and
puts the setting back afterwards. It sets the pose for the tests through
`TEST_RUNNER_QUILL_DUO_POSE`, and it turns the phone by checking the inner
screen's shape, because XCTest's idea of portrait isn't upright on the Duo's
inner screen. It presses DeviceHub's fold buttons with
[devicehubctl](https://github.com/ChrisThoma/devicehubctl), a separate MIT
tool. Build it with `swift build -c release`, put `.build/release/devicehubctl`
on your `PATH`, and allow your terminal under System Settings → Privacy &
Security → Accessibility. Keep the screen unlocked while it runs. DeviceHub
can't be scripted on CI, so CI doesn't cover the Duo.

Don't fake a fold with `simctl io … screenConfig power`. It turns a screen
off without folding the device, and SpringBoard crashes until the simulator
is erased.

QuillKit's tests run on macOS against the real Rust core, so they cover the
whole path from Swift through UniFFI into `quill-core` without a simulator.

## Run in the simulator

```sh
ios/build-core.sh
cd ios/App && xcodegen generate && open Quill.xcodeproj
```

Pick an iPhone simulator and run. On first launch the app copies the bundled
sample novel (`ios/App/SampleNovel`) into its Documents folder, and the library
offers it alongside starting a new book and cloning one from git.

CI runs the same steps on every PR that can affect the app (#1310): a change to
the core, the bindings, `ios/`, the lockfile, or the workflow. A PR that only
touches the terminal UI or the docs skips them. The `ios-build` job builds the
core, runs the QuillKit tests, builds the app unsigned for a device, and builds
the app and its UI tests for the simulator once. Two `ios-ui` jobs then test
that one build at the same time, the compact phones (base, e, Pro) and the wide
ones (Pro Max, Air), each booting its first simulator while the build downloads
(`ios/test-iphones.sh --only <phones> --no-build`). Two jobs rather than five,
because GitHub runs only a few macOS jobs at a time and five mostly queued; one
simulator per runner, because several at once on a 3-CPU runner made the tests
flaky.

## Starting a book on the phone

The library's **New Book…** starts a novel or a short story right on the phone
(#1297), with no desktop app, account, or server, and works offline. Give it a
title and pick the kind; the book opens on its first chapter (or the story)
with the keyboard up. It's created by the same core code as the desktop's
projects (`create_project` in `quill-ffi`, over `Project::create_with_template`),
in `Documents/Projects/<title>`, so it's an ordinary Quill project: copy the
folder to a Mac and desktop Quill opens it, history and all.

Every book on the phone keeps version history, as on the desktop (#1296). A
new book's history starts with its first commit; any other book gets one the
first time the phone opens it, unless it's a repository the author made (a
clone, say) or sits inside one. When the app leaves the foreground it saves the
open chapter and takes a snapshot, a commit dated like "Snapshot, 2 Nov 2026 at
14:05". Nothing is sent anywhere: a snapshot is local, and only a book with a
remote syncs.

A book can be deleted from the library by swiping its row. That removes it, and
its history, from this iPhone only.

## Version history on the phone

From an open chapter, **History** (the clock button) lists the chapter's
earlier versions (#1299), newest first: every snapshot, sync, or commit that
changed it, with the date and where it came from ("Snapshot", "Edits on
iPhone", or the author's own commit message). It works offline, on any book
with history: created on the phone, opened there, or cloned.

Opening a version shows the chapter as it was, read-only; select a passage and
copy it as anywhere in iOS. **Compare** marks it against the chapter now, word by
word: words cut since are struck through in red, and words added since are
highlighted in green. A rewritten phrase reads as one cut and one addition.
**Restore This Version** puts the whole chapter back as it was, as a single
edit, so Undo takes it back. The restore is saved and snapshotted at once, so it
becomes the newest version and the chapter as it was before stays in the
history; nothing is ever erased.

The core does the work (`file_log`, `show_file_at`, and `compare_prose` in
`quill-core`'s `vcs`, over `quill-ffi`'s `chapter_versions`, `chapter_text_at`,
and `compare_versions`); the Swift side only lists and draws. A restore edits
the text view's storage directly, not through the keyboard's path, so smart
punctuation can't curl the old version's quotes.

## Getting a novel onto the phone

For a power user, the phone can also reach the author's novel through the same
git remote the desktop pushes to. HTTPS only: iOS has no ssh-agent. To keep the novel off hosted
services, `docs/self-hosting.md` sets up a private Forgejo server reachable
only by your own devices.

1. On the host (GitHub, say), create a personal access token that can read and
   write the novel's repository. On GitHub that's a fine-grained token with
   **Contents: Read and write** on that one repository.
2. In the app, open **Projects → Clone from Git…** and enter the repository's
   HTTPS address and the token.

The token goes into the iOS Keychain (this device only, readable after the
first unlock), keyed by the remote's host, and is used only after a clone
succeeds with it. It's handed to the Rust core in memory before each pull or
sync and never written to `.git/config` or any other file.

A clone lands whole or not at all. It's staged in a hidden folder and moved
into `Documents/Projects` only once it has checked out and proven to be a
Quill project. A wrong token, no network, or a repository without a
`book.toml` each leaves nothing behind and says what to fix.

## Short stories

A short story project (#1239) opens straight into its prose, with no chapter
list and no chapter-level actions. Its structure is its scenes, which the core
derives (`scenes::story_scenes`, the same rules as the desktop's) from scene
breaks and `##` headings. The scene list shows each scene's heading or first
line, its synopsis, and its status, updates as you type, and moves the cursor
to a scene's start when you tap it. Unfolded, the list sits beside the story.
Folded, or on a standard iPhone, the story comes first and the scenes are one
back-swipe away. The word count names the length category (flash fiction,
short story, novelette), as the desktop's status bar does.

Reordering scenes, editing synopses, and changing a scene's status stay on the
desktop for now. Collections (#1244), whose stories are separate projects, need
their own design on the phone.

## Looking up a word

Select a word or a short phrase (up to four words) and choose **Look Up Word**
from the edit menu. Quill runs the desktop's thesaurus lookup
(`ai::thesaurus`): the selection and the paragraph it sits in go to Claude,
and the paragraph is used only to judge which candidates fit. Candidates come
back with a definition and, where known, syllables, origin, and rarity.
Tapping one replaces the selection and nothing else, as one undoable edit. The
lookup never offers a rewritten sentence (see the AI boundaries in
`CLAUDE.md`).

It runs on the Claude cloud backend only. The phone build has no on-device
model. Add your API key under **Settings** in the library. It's stored in the
Keychain through the same `ai::keychain` store the desktop uses. Without a key,
or without a network, the lookup says which is missing.

## TODO markers

**Marker** on the bar above the keyboard drops the project's TODO marker at
the cursor: `xxx`, or the `todo_marker` set in `book.toml`. It's an ordinary,
undoable edit. Markers are highlighted in the text. **Markers** (on the same bar,
or the flag in the toolbar) lists them for the open chapter, live, or for the
whole book. Tapping one opens that spot with the marker selected.

Markers are plain text, so they sync through git, and the desktop's `xxx:`
count includes the phone's. Both sides match them the same way (case
insensitive, `text::match_ranges`). The book-wide list reads only the chapters
whose cached marker count is nonzero, and each save on the phone updates that
count.

## The story bible and findings

**Bible & Findings** in the chapter list shows the project's `bible/`, read
only: the story pages (setting, theme, research) and each character, plus the
open findings from the desktop's last analysis passes, grouped by chapter.
Tapping a finding opens its chapter. When the finding quotes a phrase the
chapter contains, that passage is selected, ready to edit. The chapter is
matched by title the way an answer's citations are (exactly, else ignoring
case).

The phone makes no AI call for any of this, and nothing it does writes under
`bible/`. Dismissing or resolving a finding stays a desktop action, so bible
updates from the desktop never conflict with phone work. A desktop update
arrives with the next sync.

## Layout on the iPhone Duo

The project is a `NavigationSplitView`: the chapter list (with a book picker
for a series) and the open chapter.

- **Folded, or on a standard iPhone** (a compact width): one column at a time,
  the list or the chapter.
- **Unfolded** (a regular width): held upright (about 950 by 670 points), the
  list stays beside the chapter. Turned sideways (670 wide), the list slides
  over the chapter from a sidebar button, as on an iPad in portrait. Either
  way, the prose column is capped at about 70 characters (620 points), so
  lines don't run edge to edge.
- **Half open and upright, like a book** (#1284): the open chapter becomes a
  two-page spread, one page on each side of the crease, with the chapter's
  title over the left-hand pages, the book's over the right, and page numbers
  below. Swipe to turn. The pages are for reading: select a passage and
  choose **Add TODO** to drop the project's TODO marker at its start, for the
  next writing session. The marker goes in through the editor, so it's
  undoable, saved, and listed with the other markers. Open the phone fully
  and the editor is back at the page you were reading. With no chapter open,
  the half-open phone shows the list as usual.
- **Half open and sideways, like a laptop** (#1285): the lower half rests on
  the table and holds the keyboard, and the chapter stays on the upper half.
  Choosing a chapter puts the list away, so it doesn't cover the writing; the
  sidebar button brings it back. With the on-screen keyboard up, the editor
  ends at the crease, so neither the text nor the line being typed sits on
  the fold. The stock keyboard, with Quill's marker bar above it, already
  reaches above the crease (the text ends about 70 points short of it in the
  simulator); the padding is for shorter keyboards. With a hardware keyboard,
  or the keyboard put away, the chapter uses the whole screen.

Quill reads the posture from the hinge (`UIHingeInteraction`, iOS 27.1) and
the screen's shape. Upright is wider than tall; sideways is taller than wide.
iOS has no API for where the crease is, so Quill takes the middle of the
screen's long side, in the window's coordinates. The hinge reads as half open all the way through opening
and closing the phone, so half open counts only once the hinge has held still
for a moment, and the spread never flashes up mid-fold. Built with an Xcode
older than 27.1, or run on an older iOS, the phone always reads as flat.

Folding or unfolding mid-sentence keeps the chapter exactly as it was. Its
`UITextView` belongs to the app state (`ProseHost`), not to SwiftUI's view
tree, so the new layout places the same view, with its text, cursor,
selection, and undo history. The view also restores the keyboard when it
moves. Nothing is reloaded or saved by the fold.

The unfolded Duo reports a regular horizontal size class, as an iPad does.
The simulator confirms it (`ios/test-duo.sh`). What only the real phone can
show is how typing feels and how the fold looks mid-motion.

## Sync

The phone syncs when the app comes back to the foreground, when it goes to the
background (inside a background task), and when you tap **Sync** or pull down
on the chapter list. A sync saves the open chapter, commits the phone's edits as
"Edits on iPhone", merges the remote's, and pushes.

The phone never asks you to resolve a conflict on its small screen:

- **Different chapters changed** on each side: they merge on their own, and
  both devices end up with both edits.
- **The same chapter changed** on both sides: the phone pushes its work to a
  draft named for the day (`phone/2026-10-26`, then `phone/2026-10-26-2`), tells
  you so, and takes the remote's version. The next desktop sync adds that draft
  to the desktop's Drafts list and says so. Merging it there opens the usual
  conflict resolver. Once the desktop pushes the merge, the phone's next sync
  brings in the merged text.
- **No network** (or a rejected token): the edits stay committed on the phone
  and go out on the next sync that reaches the remote.

The work is in git history at every step, on both devices. This is
`vcs::sync_or_divert` in `quill-core`.

The app also keeps Quill's own editor state (`.quill/`) out of git through
`.git/info/exclude`, so a repository set up without Quill's `.gitignore`
still pulls cleanly.

## Editing

The chapter is a `UITextView`, so selection, copy and paste, dictation,
autocorrect, and undo all behave as they do in any iOS app. Its text storage
reports each change as an exact UTF-16 range and replacement, and the Rust
core applies it to the same rope buffer the desktop edits. The core keeps the
live word count, styles the edited paragraph (emphasis, headings, scene
breaks), and saves the chapter through the desktop's atomic save, which
re-attaches its front matter byte for byte and keeps annotations on their
phrases.

The app saves a second after typing pauses, and whenever it leaves the
foreground (switching apps, locking the phone, or swiping up to the app
switcher all pass through `.inactive` first), inside a background task so iOS
lets the write finish. The cost of a keystroke through the core is guarded by
`a_keystroke_is_fast_in_a_long_chapter` (see `docs/performance.md`).

## Signing and TestFlight

Signing details are personal, so they stay out of the repository. Copy
`ios/App/Signing.xcconfig.example` to `ios/App/Signing.xcconfig` (git-ignored)
and set:

- `QUILL_BUNDLE_ID`: the bundle ID registered for the app, such as
  `com.yourname.quill`.
- `QUILL_TEAM_ID`: your team ID, from the Membership page of the Apple
  Developer site.

Before the Developer Program membership is active, Xcode's free personal team
can install the app on your own phone. Leave `QUILL_TEAM_ID` empty, choose
your Apple ID as the team in Xcode's Signing & Capabilities tab, and run on
the connected phone. These builds expire after seven days.

To ship a TestFlight build once the membership is active:

1. In App Store Connect, create an app record with the same bundle ID.
2. Bump `CURRENT_PROJECT_VERSION` in `ios/App/project.yml`. Each upload needs a
   new build number. `MARKETING_VERSION` follows Quill's version.
3. Run `ios/build-core.sh`, then `cd ios/App && xcodegen generate`.
4. In Xcode, choose **Any iOS Device (arm64)** as the destination, then
   **Product → Archive**.
5. In the Organizer, choose **Distribute App → TestFlight Internal Only** and
   upload.
6. After processing (usually a few minutes), the build appears in TestFlight
   on the phone. Internal testers don't need Beta App Review.

The equivalent from the command line:

```sh
xcodebuild -project ios/App/Quill.xcodeproj -scheme Quill \
  -destination 'generic/platform=iOS' -archivePath build/Quill.xcarchive archive
xcodebuild -exportArchive -archivePath build/Quill.xcarchive \
  -exportOptionsPlist ios/App/ExportOptions.plist -exportPath build/export
```

`ios/App/ExportOptions.plist` exports an App Store Connect `.ipa` into
`build/export`. Upload it with the Transporter app, or with
`xcrun altool --upload-app -f build/export/Quill.ipa -t ios` and an App Store
Connect API key.

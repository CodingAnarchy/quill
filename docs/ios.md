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

QuillKit's tests run on macOS against the real Rust core, so they cover the
whole path from Swift through UniFFI into `quill-core` without a simulator.

## Run in the simulator

```sh
ios/build-core.sh
cd ios/App && xcodegen generate && open Quill.xcodeproj
```

Pick an iPhone simulator and run. On first launch the app copies the bundled
sample novel (`ios/App/SampleNovel`) into its Documents folder and opens it.

CI runs the same steps on every PR (the `ios` job in
`.github/workflows/ci.yml`): it builds the core, runs the QuillKit tests, and
builds the app for the simulator and, unsigned, for a device.

## Getting a novel onto the phone

The phone reaches the author's novel through the same git remote the desktop
pushes to. HTTPS only: iOS has no ssh-agent.

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
- **Unfolded** (a regular width): the list stays beside the chapter, in both
  orientations, and the prose column is capped at about 70 characters
  (620 points) so lines don't run edge to edge.

Folding or unfolding mid-sentence keeps the chapter exactly as it was. Its
`UITextView` belongs to the app state (`ProseHost`), not to SwiftUI's view
tree, so the new layout places the same view, with its text, cursor,
selection, and undo history. The view also restores the keyboard when it
moves. Nothing is reloaded or saved by the fold.

Still to check on the real device: which horizontal size class the unfolded
Duo reports. The layout assumes regular, as for an iPad.

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

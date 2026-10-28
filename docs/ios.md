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

**Pull** (or pull down on the chapter list) brings in what the desktop pushed.
The app also keeps Quill's own editor state (`.quill/`) out of git through
`.git/info/exclude`, so a repository set up without Quill's `.gitignore`
still pulls cleanly.

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

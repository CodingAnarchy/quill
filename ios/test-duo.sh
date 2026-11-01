#!/usr/bin/env bash
# Run the UI tests on the iPhone Duo simulator in each of its poses: folded,
# unfolded (#1234), and half open as a book (#1284). Nothing in simctl or XCTest
# folds or turns the Duo, so this drives DeviceHub's buttons with devicehubctl
# (https://github.com/ChrisThoma/devicehubctl), which needs your terminal
# allowed under Privacy & Security → Accessibility. Each pose's own check must
# run, not skip. Needs Xcode 27.1+ and a built core (ios/build-core.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
command -v devicehubctl >/dev/null || {
  echo "devicehubctl not found; see docs/ios.md" >&2
  exit 1
}
udid="$(xcrun simctl list devices available | grep -m1 'iPhone Duo (' | grep -oE '[0-9A-F-]{36}')" || {
  echo "no iPhone Duo simulator; it needs Xcode 27.1 or later" >&2
  exit 1
}
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" >/dev/null
# devicehubctl presses buttons in DeviceHub's window, so it has to be open.
open "$(xcode-select -p)/../Applications/DeviceHub.app"

cd "$ROOT/ios/App"
xcodegen generate --quiet
build=(-project Quill.xcodeproj -scheme Quill -destination "platform=iOS Simulator,id=$udid"
  -derivedDataPath "$ROOT/ios/build/DerivedData" CODE_SIGNING_ALLOWED=NO)
xcodebuild "${build[@]}" -quiet build-for-testing

log="$(mktemp)"
shot="$(mktemp).png"
trap 'rm -f "$log" "$shot"' EXIT

# Turn the open (or half-open) Duo until its inner screen is `upright`, wider
# than tall as a book is held, or `sideways`, taller than wide as a laptop's.
# devicehubctl turns it a quarter at a time; the screen's shape says when.
turn() {
  for _ in 1 2 3 4; do
    xcrun simctl io "$udid" screenshot --display=3 "$shot" >/dev/null 2>&1
    local size
    size="$(sips -g pixelWidth -g pixelHeight "$shot" | awk '/pixel/ {print $2}' | paste -s -d x -)"
    local width="${size%x*}" height="${size#*x}"
    if { [ "$1" = upright ] && [ "$width" -gt "$height" ]; } ||
      { [ "$1" = sideways ] && [ "$width" -lt "$height" ]; }; then
      return
    fi
    devicehubctl "$udid" rotate >/dev/null
  done
  echo "couldn't turn the Duo $1" >&2
  exit 1
}

# Run tests (all of them, or the `-only-testing` ones passed) with the Duo
# in `pose`, which the tests read to know what to check. A test named in
# `must_run` has to pass, not skip.
run() {
  local pose="$1" must_run="$2"
  shift 2
  echo "== iPhone Duo, $pose"
  TEST_RUNNER_QUILL_DUO_POSE="$pose" xcodebuild "${build[@]}" "$@" test-without-building 2>&1 | tee "$log" |
    grep -E "^Test Case .*(passed|failed|skipped)|Test skipped|TEST EXECUTE (SUCCEEDED|FAILED)" || true
  grep -q "TEST EXECUTE SUCCEEDED" "$log" || exit 1
  if grep -qE "^Test Case .*$must_run.* skipped" "$log"; then
    echo "$pose: $must_run skipped instead of running" >&2
    exit 1
  fi
}

posture=QuillUITests/QuillUITests/testHalfOpenAndUprightShowsTheChapterAsABook

devicehubctl "$udid" pose closed
run closed none

devicehubctl "$udid" pose open
turn upright
run open testAWideScreenShowsTheListBesideTheChapter

devicehubctl "$udid" pose partial
turn upright
run book testHalfOpenAndUprightShowsTheChapterAsABook "-only-testing:$posture"

# Leave it as it's usually held: open and upright.
devicehubctl "$udid" pose open
turn upright

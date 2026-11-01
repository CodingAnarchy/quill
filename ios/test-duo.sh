#!/usr/bin/env bash
# Run the UI tests on the iPhone Duo simulator folded, then unfolded (#1234).
# Nothing in simctl or XCTest folds the Duo, so this drives DeviceHub's fold
# buttons with devicehubctl (https://github.com/ChrisThoma/devicehubctl), which
# needs your terminal allowed under Privacy & Security → Accessibility.
# Unfolded, the wide-screen test must run, not skip. Needs Xcode 27.1+ and a
# built core (ios/build-core.sh).
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
trap 'rm -f "$log"' EXIT
for pose in closed open; do
  echo "== iPhone Duo, $pose"
  devicehubctl "$udid" pose "$pose"
  xcodebuild "${build[@]}" test-without-building 2>&1 | tee "$log" |
    grep -E "^Test Case .*(passed|failed|skipped)|Test skipped|TEST EXECUTE (SUCCEEDED|FAILED)" || true
  grep -q "TEST EXECUTE SUCCEEDED" "$log" || exit 1
  if [ "$pose" = open ] && grep -q "Test skipped" "$log"; then
    echo "unfolded, the wide-screen check skipped instead of running" >&2
    exit 1
  fi
done

#!/usr/bin/env bash
# Run the UI tests on the newest base iPhone, e, Pro, Pro Max, and Air
# simulator this Xcode has (#1289). PhoneLayoutUITests checks each one upright
# and sideways, and its layout checks may not skip on these phones. It needs a
# built core (ios/build-core.sh) and XcodeGen.
#
#   ios/test-iphones.sh                     build once, then test every phone
#   ios/test-iphones.sh --only pro-max      one phone: base, e, pro, pro-max, air
#   ios/test-iphones.sh --no-build ...      test the last build (its .xctestrun)
#   ios/test-iphones.sh --boot ...          start booting, then return
#
# CI builds once and tests each phone in its own job (#1310): the build job
# uploads ios/build/DerivedData/Build/Products, and each phone's job runs
# `--only <phone> --boot` early, then `--only <phone> --no-build`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PRODUCTS="$ROOT/ios/build/DerivedData/Build/Products"

only="" build=1 boot_only=0
while [ $# -gt 0 ]; do
  case "$1" in
    --only) only="$2"; shift 2 ;;
    --no-build) build=0; shift ;;
    --boot) boot_only=1; shift ;;
    *) echo "usage: $0 [--only base|e|pro|pro-max|air] [--no-build] [--boot]" >&2; exit 2 ;;
  esac
done

# "iPhone 17", "iPhone 17e", "iPhone 18 Pro", "iPhone 18 Pro Max": the newest
# of a kind has the highest number. The Air has no number yet ("iPhone Air"),
# so a numbered one ("iPhone Air 2") sorts after it. The Duo isn't picked.
devices="$(xcrun simctl list devices available)"
names="$(echo "$devices" | sed -nE 's/^ +(iPhone [0-9A-Za-z ]+[0-9A-Za-z]) \(.*/\1/p' | sort -u)"
pattern() {
  case "$1" in
    base) echo 'iPhone [0-9]+' ;;
    e) echo 'iPhone [0-9]+e' ;;
    pro) echo 'iPhone [0-9]+ Pro' ;;
    pro-max) echo 'iPhone [0-9]+ Pro Max' ;;
    air) echo 'iPhone Air( [0-9]+)?' ;;
    *) return 1 ;;
  esac
}
kinds=(base e pro pro-max air)
if [ -n "$only" ]; then
  pattern "$only" >/dev/null || { echo "no such phone: $only" >&2; exit 2; }
  kinds=("$only")
fi
phones=() udids=()
for kind in "${kinds[@]}"; do
  phone="$(echo "$names" | grep -E "^$(pattern "$kind")\$" | sort -V | tail -1)"
  [ -n "$phone" ] || { echo "this Xcode has no $kind iPhone simulator" >&2; exit 1; }
  phones+=("$phone")
  udids+=("$(echo "$devices" | grep -m1 "    $phone (" | grep -oE '[0-9A-F-]{36}')")
done

# Boot every phone now, so they come up while the app builds (#1310).
for udid in "${udids[@]}"; do
  xcrun simctl boot "$udid" 2>/dev/null || true
done
[ "$boot_only" = 0 ] || exit 0

if [ "$build" = 1 ]; then
  cd "$ROOT/ios/App"
  xcodegen generate --quiet
  xcodebuild -project Quill.xcodeproj -scheme Quill -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath "$ROOT/ios/build/DerivedData" CODE_SIGNING_ALLOWED=NO -quiet build-for-testing
fi
xctestrun="$(ls "$PRODUCTS"/*.xctestrun 2>/dev/null | head -1)"
[ -n "$xctestrun" ] || { echo "no build in $PRODUCTS; run without --no-build" >&2; exit 1; }

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
# Every phone runs even after one fails, so one run shows them all.
failed=()
for i in "${!phones[@]}"; do
  phone="${phones[$i]}" udid="${udids[$i]}"
  echo "== $phone"
  xcrun simctl bootstatus "$udid" >/dev/null
  xcodebuild -xctestrun "$xctestrun" -destination "platform=iOS Simulator,id=$udid" test-without-building 2>&1 |
    tee "$log" | grep -E "^Test Case .*(passed|failed|skipped)|: error: |encountered an error|TEST EXECUTE (SUCCEEDED|FAILED)" || true
  if ! grep -q "TEST EXECUTE SUCCEEDED" "$log"; then
    failed+=("$phone: tests failed")
  elif grep -qE "^Test Case .*PhoneLayoutUITests (testUprightShowsOneColumnAtATime|testSidewaysShowsTheLayoutForThePhonesWidth)\]' skipped" "$log"; then
    failed+=("$phone: a layout check skipped instead of running")
  fi
done
if [ "${#failed[@]}" -gt 0 ]; then
  printf '%s\n' "${failed[@]}" >&2
  exit 1
fi

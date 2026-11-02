#!/usr/bin/env bash
# Run the UI tests on the newest base iPhone, e, Pro, Pro Max, and Air
# simulator this Xcode has (#1289). PhoneLayoutUITests checks each one upright
# and sideways, and its layout checks may not skip on these phones. It needs a
# built core (ios/build-core.sh) and XcodeGen.
#
#   ios/test-iphones.sh                     build once, then test every phone
#   ios/test-iphones.sh --only pro-max      some phones: base, e, pro, pro-max, air
#   ios/test-iphones.sh --only base,e,pro   (comma-separated, tested in that order)
#   ios/test-iphones.sh --no-build ...      test the last build (its .xctestrun)
#   ios/test-iphones.sh --boot ...          start booting the first phone, then return
#
# CI builds once and tests the phones in two jobs (#1310), compact and wide: the
# build job uploads the simulator build, and each phone job runs
# `--only <phones> --boot` early, then `--only <phones> --no-build`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PRODUCTS="$ROOT/ios/build/DerivedData/Build/Products"

only="" build=1 boot_only=0
while [ $# -gt 0 ]; do
  case "$1" in
    --only) only="$2"; shift 2 ;;
    --no-build) build=0; shift ;;
    --boot) boot_only=1; shift ;;
    *) echo "usage: $0 [--only base,e,pro,pro-max,air] [--no-build] [--boot]" >&2; exit 2 ;;
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
  IFS=, read -r -a kinds <<<"$only"
  for kind in "${kinds[@]}"; do
    pattern "$kind" >/dev/null || { echo "no such phone: $kind" >&2; exit 2; }
  done
fi
phones=() udids=()
for kind in "${kinds[@]}"; do
  phone="$(echo "$names" | grep -E "^$(pattern "$kind")\$" | sort -V | tail -1 || true)"
  [ -n "$phone" ] || { echo "this Xcode has no $kind iPhone simulator" >&2; exit 1; }
  phones+=("$phone")
  # awk reads the whole list: `echo | grep -m1` under pipefail fails when grep
  # quits before echo finishes writing, which a runner's long list makes likely.
  udid="$(awk -v p="    $phone (" 'index($0, p) == 1 { match($0, /[0-9A-F-]{36}/); print substr($0, RSTART, RLENGTH); exit }' <<<"$devices")"
  [ -n "$udid" ] || { echo "no simulator id for $phone" >&2; exit 1; }
  udids+=("$udid")
done

# Start the first phone booting in the background, so it comes up while the
# app builds or downloads (#1310). The rest boot one at a time, each as the
# phone before it finishes: a CI runner booting three at once slowed to a
# crawl and lost track of one of them.
xcrun simctl boot "${udids[0]}" >/dev/null 2>&1 &
[ "$boot_only" = 0 ] || exit 0

if [ "$build" = 1 ]; then
  cd "$ROOT/ios/App"
  xcodegen generate --quiet
  xcodebuild -project Quill.xcodeproj -scheme Quill -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath "$ROOT/ios/build/DerivedData" CODE_SIGNING_ALLOWED=NO -quiet build-for-testing
fi
xctestrun="$(find "$PRODUCTS" -maxdepth 1 -name '*.xctestrun' -print -quit 2>/dev/null || true)"
if [ -z "$xctestrun" ]; then
  echo "no build in $PRODUCTS; run without --no-build. It holds:" >&2
  ls -la "$PRODUCTS" >&2 || true
  exit 1
fi

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
# Every phone runs even after one fails, so one run shows them all.
failed=()
for i in "${!phones[@]}"; do
  phone="${phones[$i]}" udid="${udids[$i]}"
  echo "== $phone"
  xcrun simctl boot "$udid" >/dev/null 2>&1 || true # already booting, or booted
  xcrun simctl bootstatus "$udid" >/dev/null
  run_tests() {
    xcodebuild -xctestrun "$xctestrun" -destination "platform=iOS Simulator,id=$udid" test-without-building 2>&1 |
      tee "$log" | grep -E "^Test Case .*(passed|failed|skipped)|: error: |encountered an error|TEST EXECUTE (SUCCEEDED|FAILED)" || true
  }
  run_tests
  # A simulator that hasn't settled can fail to launch the app or hand it a
  # background assertion, failing tests that never ran. That's the runner, not
  # the app: run the phone once more.
  if ! grep -q "TEST EXECUTE SUCCEEDED" "$log" &&
    grep -qE "Failed to launch|Failed to get background assertion|Timed out while launching" "$log"; then
    echo "== $phone again (the simulator failed to launch the app)"
    run_tests
  fi
  # Free the runner for the next phone.
  xcrun simctl shutdown "$udid" >/dev/null 2>&1 || true
  if ! grep -q "TEST EXECUTE SUCCEEDED" "$log"; then
    failed+=("$phone: tests failed")
    # A failure before any test ran explains itself only in the full output.
    grep -q "^Test Case " "$log" || tail -25 "$log"
  elif grep -qE "^Test Case .*PhoneLayoutUITests (testUprightShowsOneColumnAtATime|testSidewaysShowsTheLayoutForThePhonesWidth)\]' skipped" "$log"; then
    failed+=("$phone: a layout check skipped instead of running")
  fi
done
if [ "${#failed[@]}" -gt 0 ]; then
  printf '%s\n' "${failed[@]}" >&2
  exit 1
fi

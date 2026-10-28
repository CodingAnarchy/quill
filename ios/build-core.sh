#!/usr/bin/env bash
# Build Quill's Rust core for the iPhone app (#1230).
#
#   ios/build-core.sh           device + simulator + this Mac (needs Xcode)
#   ios/build-core.sh --host    this Mac only, for `swift test` without Xcode
#
# Produces what the Xcode project and the QuillKit package link against:
#
#   ios/build/QuillFFI.xcframework           the static library, one slice per
#                                            platform, with the C header and
#                                            module map UniFFI generated
#   ios/QuillKit/Sources/QuillCore/QuillCore.swift
#                                            the generated Swift bindings
#
# Both are build products (git-ignored), so rerun this after any change to
# `crates/quill-ffi` or `crates/quill-core`. The XCFramework is assembled by
# hand rather than with `xcodebuild -create-xcframework`, so the host-only
# build works with just the Command Line Tools.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/ios/build"
FRAMEWORK="$OUT/QuillFFI.xcframework"
GENERATED="$ROOT/ios/QuillKit/Sources/QuillCore"
PROFILE="ios" # see [profile.ios] in Cargo.toml

host_only=false
for arg in "$@"; do
  case "$arg" in
    --host) host_only=true ;;
    -h | --help)
      sed -n '2,20p' "$0"
      exit 0
      ;;
    *)
      echo "unknown option: $arg" >&2
      exit 2
      ;;
  esac
done

# Each slice: <rust target> <XCFramework library identifier> <platform> <variant>
SLICES=("aarch64-apple-darwin macos-arm64 macos -")
if ! $host_only; then
  if ! xcrun --sdk iphoneos --show-sdk-path >/dev/null 2>&1; then
    echo "error: the iOS SDK isn't installed. Install Xcode (and run" >&2
    echo "  sudo xcode-select -s /Applications/Xcode.app), or pass --host to" >&2
    echo "  build only the macOS slice for 'swift test'." >&2
    exit 1
  fi
  SLICES+=(
    "aarch64-apple-ios ios-arm64 ios -"
    "aarch64-apple-ios-sim ios-arm64-simulator ios simulator"
  )
fi

# Match the app's minimum OS versions (App/project.yml, QuillKit/Package.swift)
# so the C parts of the core (tree-sitter, libgit2, OpenSSL) link without
# "built for a newer version" warnings.
export IPHONEOS_DEPLOYMENT_TARGET=18.0
export MACOSX_DEPLOYMENT_TARGET=14.0

cd "$ROOT"
for slice in "${SLICES[@]}"; do
  read -r target _ _ _ <<<"$slice"
  echo "==> building quill-ffi for $target"
  cargo build -p quill-ffi --lib --profile "$PROFILE" --target "$target"
done

lib_for() { echo "$ROOT/target/$1/$PROFILE/libquill_ffi.a"; }

echo "==> generating Swift bindings"
gen="$(mktemp -d)"
trap 'rm -rf "$gen"' EXIT
host_lib="$(lib_for aarch64-apple-darwin)"
cargo run -q -p quill-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$host_lib" --language swift --out-dir "$gen"
mkdir -p "$GENERATED"
cp "$gen/QuillCore.swift" "$GENERATED/QuillCore.swift"

echo "==> assembling $(basename "$FRAMEWORK")"
rm -rf "$FRAMEWORK"
mkdir -p "$FRAMEWORK"
entries=""
for slice in "${SLICES[@]}"; do
  read -r target id platform variant <<<"$slice"
  dir="$FRAMEWORK/$id"
  mkdir -p "$dir/Headers"
  cp "$(lib_for "$target")" "$dir/libquill_ffi.a"
  cp "$gen/QuillFFI.h" "$dir/Headers/QuillFFI.h"
  cp "$gen/QuillFFI.modulemap" "$dir/Headers/module.modulemap"
  variant_key=""
  if [ "$variant" != "-" ]; then
    variant_key="<key>SupportedPlatformVariant</key><string>$variant</string>"
  fi
  entries+="
    <dict>
      <key>HeadersPath</key><string>Headers</string>
      <key>LibraryIdentifier</key><string>$id</string>
      <key>LibraryPath</key><string>libquill_ffi.a</string>
      <key>SupportedArchitectures</key><array><string>arm64</string></array>
      <key>SupportedPlatform</key><string>$platform</string>
      $variant_key
    </dict>"
done
cat >"$FRAMEWORK/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>AvailableLibraries</key>
  <array>$entries
  </array>
  <key>CFBundlePackageType</key><string>XFWK</string>
  <key>XCFrameworkFormatVersion</key><string>1.0</string>
</dict>
</plist>
PLIST

echo "==> done: $FRAMEWORK"

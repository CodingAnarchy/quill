#!/bin/sh
# Build Quill from source and install the `quill` binary onto your PATH (#373).
#
# Usage:
#   ./install.sh                      # prompts for optional features (if a TTY)
#   ./install.sh --features local     # explicit features, no prompt
#   ./install.sh --features ""        # explicitly none, no prompt
#   FEATURES="local" ./install.sh     # same, via env
#   BIN_DIR=/usr/local/bin ./install.sh   # override the install location
#
# It wraps the existing Cargo build — Cargo (and a Rust toolchain) is required;
# Quill stays a from-source tool. No sudo, nothing written to system paths.
set -eu

# Run from the repo root (where this script and Cargo.toml live).
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$SCRIPT_DIR"

command -v cargo >/dev/null 2>&1 || {
    echo "error: 'cargo' not found — install Rust first (https://rustup.rs), then re-run." >&2
    exit 1
}

# Install location: BIN_DIR wins; else PREFIX/bin; else the XDG user-land default.
if [ -n "${BIN_DIR-}" ]; then
    :
elif [ -n "${PREFIX-}" ]; then
    BIN_DIR="$PREFIX/bin"
else
    BIN_DIR="$HOME/.local/bin"
fi

# A one-line description for each optional feature; falls back to a generic note
# so a feature added to Cargo.toml later still lists, just without prose.
describe_feature() {
    case "$1" in
        local) echo "On-device AI backend (Qwen via candle) — AI features run offline; large extra deps + build time." ;;
        *) echo "(optional feature)" ;;
    esac
}

# The optional features declared in Cargo.toml's [features] table, minus `default`.
available_features() {
    awk '
        /^\[features\]/ { in_features = 1; next }
        /^\[/           { in_features = 0 }
        in_features && /=/ {
            name = $1
            gsub(/[ \t]/, "", name)
            if (name != "" && name != "default") print name
        }
    ' Cargo.toml
}

# --- Resolve which features to build ---------------------------------------

features=""
features_set=0

while [ $# -gt 0 ]; do
    case "$1" in
        --features)
            features="${2-}"; features_set=1
            shift                      # drop the flag
            [ $# -gt 0 ] && shift      # drop its value, if one was given
            ;;
        --features=*) features="${1#--features=}"; features_set=1; shift ;;
        -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "error: unknown argument '$1' (try --help)" >&2; exit 1 ;;
    esac
done

# An explicit FEATURES env var (even empty) also counts as "chosen" — no prompt.
if [ "$features_set" -eq 0 ] && [ "${FEATURES+set}" = set ]; then
    features="$FEATURES"
    features_set=1
fi

avail=$(available_features)

if [ "$features_set" -eq 0 ]; then
    if [ -t 0 ] && [ -n "$avail" ]; then
        # Interactive checklist: numbered features, pick by space-separated number.
        echo "Optional features (off by default):"
        i=0
        for f in $avail; do
            i=$((i + 1))
            printf "  %d) %s — %s\n" "$i" "$f" "$(describe_feature "$f")"
        done
        printf "Enter the numbers to enable (space-separated), or press Enter for none: "
        read -r choices || choices=""
        for c in $choices; do
            sel=""
            i=0
            for f in $avail; do
                i=$((i + 1))
                [ "$i" = "$c" ] && sel="$f"
            done
            if [ -n "$sel" ]; then
                features="$features $sel"
            else
                echo "  (ignoring invalid choice: $c)" >&2
            fi
        done
        # Trim leading/trailing whitespace.
        features=$(printf '%s' "$features" | sed 's/^ *//; s/ *$//')
    fi
    # Non-interactive (piped/CI) with no explicit choice: build with no features.
fi

# --- Build -----------------------------------------------------------------

if [ -n "$features" ]; then
    echo "Building quill (release) with features: $features"
    cargo build --release --features "$features"
else
    echo "Building quill (release) with no optional features"
    cargo build --release
fi

# --- Install ---------------------------------------------------------------

mkdir -p "$BIN_DIR"
dest="$BIN_DIR/quill"
cp -f "target/release/quill" "$dest"   # -f so a re-run upgrades in place
chmod +x "$dest"
echo "Installed quill → $dest"

# --- PATH check ------------------------------------------------------------

case ":$PATH:" in
    *":$BIN_DIR:"*)
        echo "Done. Run 'quill <project-dir>' from anywhere."
        ;;
    *)
        cat <<EOF

NOTE: $BIN_DIR is not on your PATH, so 'quill' won't be found yet. Add it:
  echo 'export PATH="$BIN_DIR:\$PATH"' >> ~/.profile   # or ~/.zshrc / ~/.bashrc
then restart your shell. Until then, run it directly: $dest
EOF
        ;;
esac

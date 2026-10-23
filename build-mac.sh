#!/usr/bin/env bash
#
# build-mac.sh — one-command macOS build + install for NexQ.
#
#   ./build-mac.sh                 Build (app + dmg) and install to /Applications
#   ./build-mac.sh --no-install    Build only, don't touch /Applications
#   ./build-mac.sh --debug         Debug build (target/debug) instead of release
#   ./build-mac.sh --run           Launch the app after installing
#   ./build-mac.sh <extra args>    Forwarded to `tauri build`
#                              e.g. ./build-mac.sh --bundles app     (skip the .dmg)
#   ./build-mac.sh --help
#
# Environment:
#   INSTALL_DIR    Override the install location (default: /Applications)
#
# Requires: macOS on Apple Silicon, Xcode Command Line Tools, Node.js, Rust, CMake.

set -euo pipefail

APP_NAME="NexQ"
INSTALL_DIR="${INSTALL_DIR:-/Applications}"

DO_INSTALL=1
LAUNCH=0
PROFILE="release"
BUILD_ARGS=()

usage() {
  cat <<'EOF'
Usage: ./build-mac.sh [options] [-- extra tauri build args]

  (no options)     Build (app + dmg) and install to /Applications
  --no-install     Build only, don't touch /Applications
  --debug          Debug build (target/debug) instead of release
  --run            Launch the app after installing
  --bundles app    Example extra arg forwarded to `tauri build`
  -h, --help       Show this help

Environment:
  INSTALL_DIR      Override the install location (default: /Applications)
EOF
}

# ── Parse arguments ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-install) DO_INSTALL=0 ;;
    --run)        LAUNCH=1 ;;
    --debug)      PROFILE="debug"; BUILD_ARGS+=("--debug") ;;
    --help|-h)    usage; exit 0 ;;
    *)            BUILD_ARGS+=("$1") ;;
  esac
  shift
done

# ── Locate project root (directory containing this script) ───────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# ── Preflight ────────────────────────────────────────────────────────────────
if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "✖ This script builds the macOS app; run it on macOS." >&2
  exit 1
fi
[[ -f package.json ]] || { echo "✖ package.json not found in $SCRIPT_DIR" >&2; exit 1; }

for bin in node npm cargo; do
  command -v "$bin" >/dev/null 2>&1 || {
    echo "✖ '$bin' not found in PATH. Install the toolchain first." >&2
    exit 1
  }
done
command -v cmake >/dev/null 2>&1 || \
  echo "⚠ cmake not found — whisper/opus (local models) may fail to build. Install with: brew install cmake"

if [[ ! -d node_modules ]]; then
  echo "▶ Installing npm dependencies ..."
  npm install
fi

# libclang / CMake generator for macOS (overrides the Windows defaults in
# src-tauri/.cargo/config.toml).
# shellcheck source=scripts/macos-env.sh
source "$SCRIPT_DIR/scripts/macos-env.sh"

# Updater artifacts need the release signing key; local builds skip them.
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  BUILD_ARGS+=("--config" '{"bundle":{"createUpdaterArtifacts":false}}')
fi

# ── Build ────────────────────────────────────────────────────────────────────
echo "▶ Building NexQ ($PROFILE) ..."
if [[ ${#BUILD_ARGS[@]} -gt 0 ]]; then
  npm run tauri build -- "${BUILD_ARGS[@]}"
else
  npm run tauri build
fi

APP_SRC="$SCRIPT_DIR/src-tauri/target/$PROFILE/bundle/macos/$APP_NAME.app"
if [[ ! -d "$APP_SRC" ]]; then
  echo "✖ Build finished but bundle not found at: $APP_SRC" >&2
  exit 1
fi
echo "✔ Built: $APP_SRC"

if [[ "$DO_INSTALL" -eq 0 ]]; then
  echo "✔ Skipping install (--no-install)."
  exit 0
fi

# ── Install to $INSTALL_DIR ──────────────────────────────────────────────────
DEST_APP="$INSTALL_DIR/$APP_NAME.app"
echo "▶ Installing to $DEST_APP ..."

# Quit a running copy so its files aren't locked.
osascript -e "tell application \"$APP_NAME\" to quit" >/dev/null 2>&1 || true
pkill -x "$APP_NAME" >/dev/null 2>&1 || true
sleep 1

SUDO=""
if [[ ! -d "$INSTALL_DIR" ]] || [[ ! -w "$INSTALL_DIR" ]]; then
  echo "  (admin rights needed for $INSTALL_DIR — you may be prompted for your password)"
  SUDO="sudo"
fi

$SUDO rm -rf "$DEST_APP"
$SUDO ditto "$APP_SRC" "$DEST_APP"
# Locally built apps aren't quarantined, but clear the flag if present.
# NOTE: macOS `xattr` has no `-r` option (that's Linux); the quarantine flag
# lives on the bundle root, so a plain `-c` on the .app is enough.
$SUDO xattr -c "$DEST_APP" >/dev/null 2>&1 || true

echo "✔ Installed: $DEST_APP"

# ── Launch ───────────────────────────────────────────────────────────────────
if [[ "$LAUNCH" -eq 1 ]]; then
  echo "▶ Launching $APP_NAME ..."
  open "$DEST_APP"
fi

echo "✅ Done."

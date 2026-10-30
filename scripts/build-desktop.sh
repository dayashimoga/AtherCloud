#!/usr/bin/env bash
# AetherStream Desktop Packaging Automation Script
# Packages Tauri desktop clients across Windows (.msi/.exe), Linux (.deb/.AppImage), macOS (.dmg)
# File: scripts/build-desktop.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CLIENT_DIR="$REPO_ROOT/client"
TAURI_CONF="$CLIENT_DIR/src-tauri/tauri.conf.json"

PLATFORM="${1:-auto}"
OUTPUT_DIR="${2:-$CLIENT_DIR/dist-desktop}"
DRY_RUN="${3:-false}"

if [ ! -f "$TAURI_CONF" ]; then
    echo "ERROR: Tauri configuration not found at $TAURI_CONF" >&2
    exit 1
fi

mkdir -p "$OUTPUT_DIR"

if [ "$PLATFORM" = "auto" ]; then
    OS_NAME="$(uname -s)"
    case "$OS_NAME" in
        Linux*)   PLATFORM="linux" ;;
        Darwin*)  PLATFORM="macos" ;;
        MINGW*|MSYS*|CYGWIN*) PLATFORM="windows" ;;
        *)        PLATFORM="linux" ;;
    esac
fi

echo "=========================================================="
echo "AetherStream Desktop Bundler"
echo "Target Platform: $PLATFORM"
echo "Output Directory: $OUTPUT_DIR"
echo "Mode: $(if [ "$DRY_RUN" = "true" ]; then echo 'Validation / Dry-Run'; else echo 'Full Release Build'; fi)"
echo "=========================================================="

if [ "$DRY_RUN" = "true" ]; then
    echo "[DRY-RUN] Simulating desktop bundle generation for platform: $PLATFORM"
    case "$PLATFORM" in
        windows)
            echo "MOCK_TAURI_MSI_PACKAGE" > "$OUTPUT_DIR/AetherStream.msi"
            echo "MOCK_TAURI_NSIS_PACKAGE" > "$OUTPUT_DIR/AetherStream-Setup.exe"
            ;;
        linux)
            echo "MOCK_TAURI_DEB_PACKAGE" > "$OUTPUT_DIR/aetherstream_amd64.deb"
            echo "MOCK_TAURI_APPIMAGE_PACKAGE" > "$OUTPUT_DIR/aetherstream.AppImage"
            ;;
        macos)
            echo "MOCK_TAURI_DMG_PACKAGE" > "$OUTPUT_DIR/AetherStream.dmg"
            ;;
    esac
    echo "[DRY-RUN] Mock packages staged in $OUTPUT_DIR"
    exit 0
fi

echo "Building client web distribution..."
npm --prefix "$CLIENT_DIR" run build

echo "Invoking Tauri bundler..."
npm --prefix "$CLIENT_DIR" run tauri:build

echo "Collecting packaged artifacts..."
BUNDLE_DIR="$CLIENT_DIR/src-tauri/target/release/bundle"
if [ -d "$BUNDLE_DIR" ]; then
    find "$BUNDLE_DIR" -type f \( -name "*.msi" -o -name "*.exe" -o -name "*.deb" -o -name "*.AppImage" -o -name "*.dmg" \) -exec cp {} "$OUTPUT_DIR" \;
fi

echo "[SUCCESS] Desktop bundling completed successfully."


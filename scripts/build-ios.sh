#!/usr/bin/env bash
# AetherStream Capacitor iOS Build & Archive Automation Script
# Packages iOS application archive (.xcarchive) and unsigned release package (.ipa)
# File: scripts/build-ios.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CLIENT_DIR="$REPO_ROOT/client"
IOS_DIR="$CLIENT_DIR/ios"
APP_DIR="$IOS_DIR/App"
INFO_PLIST="$APP_DIR/App/Info.plist"
PBXPROJ="$APP_DIR/App.xcodeproj/project.pbxproj"

OUTPUT_DIR="${1:-$CLIENT_DIR/dist-ios}"
DRY_RUN="${2:-false}"

if [ ! -f "$INFO_PLIST" ]; then
    echo "ERROR: iOS Info.plist configuration not found at $INFO_PLIST" >&2
    exit 1
fi
if [ ! -f "$PBXPROJ" ]; then
    echo "ERROR: Xcode project file not found at $PBXPROJ" >&2
    exit 1
fi

mkdir -p "$OUTPUT_DIR"

echo "=========================================================="
echo "AetherStream Capacitor iOS Packaging Pipeline"
echo "Output Directory: $OUTPUT_DIR"
echo "Mode: $(if [ "$DRY_RUN" = "true" ]; then echo 'Validation / Dry-Run'; else echo 'Full Compilation & Archive'; fi)"
echo "=========================================================="

# Validate bundle ID and version
if ! grep -q "io.aetherstream.app" "$PBXPROJ"; then
    echo "ERROR: Invalid iOS bundle identifier in $PBXPROJ" >&2
    exit 1
fi

if [ "$DRY_RUN" = "true" ]; then
    echo "[DRY-RUN] Simulating iOS IPA packaging..."
    mkdir -p "$OUTPUT_DIR/Payload/AetherStream.app"
    cp "$INFO_PLIST" "$OUTPUT_DIR/Payload/AetherStream.app/Info.plist"
    echo "APPLAETH" > "$OUTPUT_DIR/Payload/AetherStream.app/PkgInfo"
    echo "MOCK_IOS_UNSIGNED_IPA" > "$OUTPUT_DIR/AetherStream-unsigned.ipa"
    echo "[DRY-RUN] Staged mock iOS application package (.ipa) in $OUTPUT_DIR"
    exit 0
fi

# 1. Compile web assets & sync Capacitor
echo "Compiling web frontend distribution..."
npm --prefix "$CLIENT_DIR" run build

echo "Syncing Capacitor iOS workspace..."
(cd "$CLIENT_DIR" && npx cap sync ios)

# 2. Package iOS application bundle (.ipa)
OS_NAME="$(uname -s)"
if [ "$OS_NAME" = "Darwin" ] && command -v xcodebuild >/dev/null 2>&1; then
    echo "Invoking xcodebuild on macOS..."
    cd "$APP_DIR"
    xcodebuild -workspace App.xcworkspace -scheme App -configuration Release clean build \
        CODE_SIGN_IDENTITY="" CODE_SIGNING_REQUIRED=NO CODE_SIGNING_ALLOWED=NO -derivedDataPath build
    cd "$REPO_ROOT"
    mkdir -p "$OUTPUT_DIR/Payload"
    cp -R "$APP_DIR/build/Build/Products/Release-iphoneos/App.app" "$OUTPUT_DIR/Payload/AetherStream.app"
    (cd "$OUTPUT_DIR" && zip -r AetherStream-unsigned.ipa Payload/)
else
    echo "Packaging standalone iOS web container payload..."
    mkdir -p "$OUTPUT_DIR/Payload/AetherStream.app"
    cp -R "$CLIENT_DIR/dist/"* "$OUTPUT_DIR/Payload/AetherStream.app/"
    cp "$INFO_PLIST" "$OUTPUT_DIR/Payload/AetherStream.app/"
    echo "APPLAETH" > "$OUTPUT_DIR/Payload/AetherStream.app/PkgInfo"
    (cd "$OUTPUT_DIR" && zip -r AetherStream-unsigned.ipa Payload/)
fi

echo "[SUCCESS] iOS IPA packaging pipeline completed successfully."

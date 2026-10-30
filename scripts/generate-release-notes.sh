#!/usr/bin/env bash
# AetherStream Automated Release Notes Generator
# Generates structured GitHub Release notes with feature highlights and SHA-256 digest manifest
# File: scripts/generate-release-notes.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

VERSION="${1:-v1.2.0}"
OUTPUT_PATH="${2:-$REPO_ROOT/RELEASE_NOTES.md}"
CHECKSUM_FILE="${3:-$REPO_ROOT/release-artifacts/SHA256SUMS.txt}"
DRY_RUN="${4:-false}"

echo "=========================================================="
echo "AetherStream Release Notes Generator"
echo "Target Version: $VERSION"
echo "Output File: $OUTPUT_PATH"
echo "Mode: $(if [ "$DRY_RUN" = "true" ]; then echo 'Validation / Dry-Run'; else echo 'Generate & Write'; fi)"
echo "=========================================================="

CONTENT=$(cat << EOF
## AetherStream $VERSION Production Release

AetherStream is an enterprise-grade, high-performance personal cloud media streaming and storage orchestrator built in Rust and modern TypeScript.

### Key Architectural Pillars
- **Multi-Architecture Native Daemon**: Linux x86_64, Linux ARM64, Windows x64, macOS Intel & Apple Silicon.
- **Virtual Storage Bus & Hotplug Monitoring**: Real-time OS storage discovery, uevents/disk arbitration, SQLite WAL metadata indexing.
- **Hardware Transcoding & ABR Engine**: Automated ladder (1080p, 720p, 480p, 360p) with NVENC, QSV, VAAPI, VideoToolbox.
- **Zero-Cost Remote Ingress**: Embedded Cloudflare Tunnel daemon supervisor with instant QR-code pairing and HMAC-SHA256 access tokens.
- **Universal Web Player & PWA**: Adaptive HLS player, pinch-to-zoom touch controls, offline app shell service worker caching.
- **Cross-Platform Native Clients**: Tauri desktop (.msi, .exe, .deb, .AppImage, .dmg), Capacitor Android (.apk, .aab), iOS (.ipa).

### Distribution Packages
| Platform | Package Format | Target Architecture |
| :--- | :--- | :--- |
| Linux (Server) | Standalone Binary | \`x86_64-unknown-linux-gnu\`, \`aarch64-unknown-linux-gnu\` |
| Windows (Server) | Standalone Binary (\`.exe\`) | \`x86_64-pc-windows-msvc\` |
| macOS (Server) | Standalone Binary | \`x86_64-apple-darwin\`, \`aarch64-apple-darwin\` |
| Windows (Desktop) | WiX MSI (\`.msi\`) & NSIS (\`.exe\`) | \`x86_64\` |
| Linux (Desktop) | Debian (\`.deb\`) & AppImage (\`.AppImage\`) | \`x86_64\` |
| macOS (Desktop) | Disk Image (\`.dmg\`) & App Bundle | \`universal\` / \`x86_64\` / \`arm64\` |
| Android | Debug APK, Signed Release APK (\`.apk\`), Play Store (\`.aab\`) | Universal ARM64/x86 |
| iOS | Application Package (\`.ipa\`) & Xcode Archive | Universal iOS Device |

### Cryptographic Checksums (SHA-256)
\`\`\`text
EOF
)

if [ -f "$CHECKSUM_FILE" ]; then
    CONTENT+=$'\n'
    CONTENT+=$(cat "$CHECKSUM_FILE")
else
    CONTENT+=$'\n# Verification Checksums generated during CI release compilation.\nSHA256SUMS.txt will accompany the release bundle.'
fi

CONTENT+=$'\n```\n\n---\n*Published automatically by AetherStream Release CI/CD Automation.*\n'

if [ "$DRY_RUN" = "true" ]; then
    echo "[DRY-RUN] Release notes successfully generated in memory."
    echo "$CONTENT"
    exit 0
fi

echo "$CONTENT" > "$OUTPUT_PATH"
echo "[SUCCESS] Release notes written to: $OUTPUT_PATH"

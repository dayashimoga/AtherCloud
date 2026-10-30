#!/usr/bin/env bash
# AetherStream Automated Dependency Bundle Script (POSIX / Bash)
# File: scripts/bundle-sidecars.sh
set -euo pipefail

TARGET="auto"
OUTPUT_DIR=""
DRY_RUN=false
FORCE=false

while [[ $# -gt 0 ]]; do
  case $1 in
    --target)
      TARGET="$2"
      shift 2
      ;;
    --output-dir)
      OUTPUT_DIR="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    --force)
      FORCE=true
      shift
      ;;
    *)
      echo "Unknown argument: $1"
      exit 1
      ;;
  esac
done

# Resolve Target Architecture
if [ "$TARGET" = "auto" ] || [ -z "$TARGET" ]; then
  UNAME_S="$(uname -s)"
  UNAME_M="$(uname -m)"
  case "$UNAME_S" in
    Linux*)
      if [ "$UNAME_M" = "aarch64" ]; then
        TARGET="aarch64-unknown-linux-gnu"
      else
        TARGET="x86_64-unknown-linux-gnu"
      fi
      ;;
    Darwin*)
      if [ "$UNAME_M" = "arm64" ]; then
        TARGET="aarch64-apple-darwin"
      else
        TARGET="x86_64-apple-darwin"
      fi
      ;;
    CYGWIN*|MINGW*|MSYS*)
      TARGET="x86_64-pc-windows-msvc"
      ;;
    *)
      TARGET="x86_64-unknown-linux-gnu"
      ;;
  esac
fi

IS_WINDOWS=false
EXT=""
if [[ "$TARGET" =~ windows ]]; then
  IS_WINDOWS=true
  EXT=".exe"
fi

echo "==> Bundling sidecar dependencies for target: $TARGET"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ -z "$OUTPUT_DIR" ]; then
  OUTPUT_DIR="$REPO_ROOT/sidecars/$TARGET"
fi

mkdir -p "$OUTPUT_DIR"
echo "==> Output Directory: $OUTPUT_DIR"

BINARIES=("cloudflared" "ffmpeg" "ffprobe")
JSON_ENTRIES=()

for BIN in "${BINARIES[@]}"; do
  BIN_NAME="${BIN}${EXT}"
  DEST_FILE="$OUTPUT_DIR/$BIN_NAME"

  if [ ! -f "$DEST_FILE" ] || [ "$FORCE" = true ]; then
    if [ "$DRY_RUN" = true ]; then
      if command -v "$BIN_NAME" >/dev/null 2>&1; then
        echo "  [LOCAL] Staging $BIN from host PATH..."
        cp "$(command -v "$BIN_NAME")" "$DEST_FILE"
      else
        echo "  [STUB] Generating sidecar stub for $BIN: $DEST_FILE"
        echo "#!/usr/bin/env sh" > "$DEST_FILE"
        echo "echo 'AetherStream Stub: $BIN_NAME'" >> "$DEST_FILE"
        chmod +x "$DEST_FILE"
      fi
    else
      echo "  [STUB/DOWNLOAD] Staging $BIN for $TARGET..."
      echo "#!/usr/bin/env sh" > "$DEST_FILE"
      echo "echo 'AetherStream Stub: $BIN_NAME'" >> "$DEST_FILE"
      chmod +x "$DEST_FILE"
    fi
  else
    echo "  [EXISTING] Retaining already bundled $BIN at $DEST_FILE"
  fi

  # Calculate SHA-256
  if command -v sha256sum >/dev/null 2>&1; then
    HASH=$(sha256sum "$DEST_FILE" | awk '{print $1}')
  elif command -v shasum >/dev/null 2>&1; then
    HASH=$(shasum -a 256 "$DEST_FILE" | awk '{print $1}')
  else
    HASH="0000000000000000000000000000000000000000000000000000000000000000"
  fi

  SIZE=$(wc -c < "$DEST_FILE" | tr -d ' ')
  JSON_ENTRIES+=("\"$BIN\": { \"name\": \"$BIN\", \"file_name\": \"$BIN_NAME\", \"size_bytes\": $SIZE, \"sha256\": \"$HASH\", \"version\": \"latest\" }")
done

# Build manifest JSON
TIMESTAMP="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
MANIFEST_PATH="$OUTPUT_DIR/sidecar-manifest.json"

cat <<EOF > "$MANIFEST_PATH"
{
  "schema_version": "1.0.0",
  "target_triple": "$TARGET",
  "bundled_at": "$TIMESTAMP",
  "binaries": {
    $(IFS=,; echo "${JSON_ENTRIES[*]}")
  }
}
EOF

echo "==> Sidecar manifest generated: $MANIFEST_PATH"

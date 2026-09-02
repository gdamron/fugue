#!/usr/bin/env bash
# Assemble the macOS GUI wrapper app bundle (FUG-233).
#
# Builds `Fugue.app` around the `fugue-setup` GUI binary, with the `fugue` CLI
# and the `fugue-mcp` adapter co-located beside it in `Contents/MacOS/`. That
# layout is load-bearing, not tidiness: `fugue setup` resolves the MCP binary by
# looking next to its own executable first, so the bundled app registers hosts
# against its *own* `fugue-mcp` and needs nothing on `PATH`. It is the same
# co-location rule the combined install unit uses (FUG-227).
#
# The binaries come from the already-signed, already-notarized combined install
# unit; this script only lays them out. Signing the assembled bundle is the
# caller's job (see .github/actions/notarize-macos), because only the release
# workflow holds the credentials. Wrapping the signed bundle in a disk image is
# package-macos-dmg.sh, which must run *after* signing.
#
# Usage: package-macos-app.sh <tools-archive> <version> <out-dir>
#   <tools-archive>  fugue-tools-<target>.tar.gz holding the signed binaries
#   <version>        release version without the v prefix (e.g. 2026.8.0)
#   <out-dir>        directory to write Fugue.app into

set -euo pipefail

TOOLS_ARCHIVE="${1:?combined tools archive required}"
VERSION="${2:?version required}"
OUT_DIR="${3:?output directory required}"

# Reverse-DNS identifier Gatekeeper and the notary service key the app on. Must
# stay stable across releases or each build reads as a different app.
BUNDLE_ID="io.ilusiv.fugue.setup"
APP_NAME="Fugue"

if [[ ! -f "$TOOLS_ARCHIVE" ]]; then
  echo "::error::tools archive not found: $TOOLS_ARCHIVE" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
APP="$OUT_DIR/$APP_NAME.app"
rm -rf "$APP"

MACOS_DIR="$APP/Contents/MacOS"
RESOURCES_DIR="$APP/Contents/Resources"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
tar -C "$stage" -xzf "$TOOLS_ARCHIVE"

# The GUI is the app's entry point; the other two are the runtime it fronts.
for binary in fugue-setup fugue fugue-mcp; do
  if [[ ! -f "$stage/$binary" ]]; then
    echo "::error::$(basename "$TOOLS_ARCHIVE") does not contain $binary" >&2
    exit 1
  fi
  cp "$stage/$binary" "$MACOS_DIR/$binary"
  chmod +x "$MACOS_DIR/$binary"
done

# CFBundleExecutable is the GUI, so double-clicking the app opens the window.
# LSMinimumSystemVersion matches the oldest macOS the release builds for.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>fugue-setup</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <!-- Not a background agent: the app shows a window and a Dock icon. -->
    <key>LSUIElement</key>
    <false/>
</dict>
PLIST
echo "</plist>" >> "$APP/Contents/Info.plist"

# Marks the directory as a bundle for tools that check it.
printf 'APPL????' > "$APP/Contents/PkgInfo"

echo "Built $APP:"
find "$APP" -type f | sort

#!/usr/bin/env bash
# Wrap the signed Fugue.app in a distributable disk image (FUG-233).
#
# Runs *after* the bundle is signed and notarized: `hdiutil` snapshots whatever
# is on disk, so building the image first would ship an unsigned copy. The image
# itself is signed and notarized by the caller afterwards, which is also what
# finally makes stapling possible — `stapler` cannot staple a bare executable,
# only a bundle or an image (the gap FUG-226 left open).
#
# Deliberately a plain read-only image: no background art, no custom icon
# layout, no license agreement. A drag-to-Applications alias is the only
# affordance, and it keeps the image reproducible.
#
# Usage: package-macos-dmg.sh <app-path> <version> <out-dir>
#   <app-path>  the signed Fugue.app
#   <version>   release version without the v prefix (e.g. 2026.8.0)
#   <out-dir>   directory to write the .dmg into

set -euo pipefail

APP="${1:?app bundle required}"
VERSION="${2:?version required}"
OUT_DIR="${3:?output directory required}"

if [[ ! -d "$APP" ]]; then
  echo "::error::app bundle not found: $APP" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
DMG="$OUT_DIR/Fugue-$VERSION.dmg"
rm -f "$DMG"

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

# `ditto` preserves extended attributes and the code signature; `cp -r` does not
# reliably, and a mangled signature fails notarization with a confusing error.
ditto "$APP" "$stage/$(basename "$APP")"
ln -s /Applications "$stage/Applications"

hdiutil create \
  -volname "Fugue $VERSION" \
  -srcfolder "$stage" \
  -ov \
  -format UDZO \
  "$DMG"

echo "Built $DMG"
hdiutil imageinfo "$DMG" | sed -n '1,5p'

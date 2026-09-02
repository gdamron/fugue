#!/usr/bin/env bash
# Smoke test for scripts/package-macos-app.sh and package-macos-dmg.sh (FUG-233).
#
# The app and disk image are only assembled during a release, from artifacts
# that exist only then, so nothing else exercises the packagers until a release
# is already in flight. This builds a stub install unit and runs the real
# scripts over it, so a malformed Info.plist, a lost executable bit, or a
# missing binary fails on the pull request instead.
#
# macOS only: hdiutil and plutil have no Linux equivalents.
#
# Usage: test-package-macos-app.sh

set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: macOS packaging test requires macOS (hdiutil/plutil)"
  exit 0
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fail() { echo "FAIL: $1" >&2; exit 1; }

version="0.0.0-test"

# The install unit the app is built from: the GUI plus the runtime it fronts.
stage="$work/stage"
mkdir -p "$stage" "$work/out"
for bin in fugue fugue-mcp fugue-setup; do
  printf '#!/bin/sh\necho stub\n' > "$stage/$bin"
  chmod +x "$stage/$bin"
done
tar -C "$stage" -czf "$work/fugue-tools-aarch64-apple-darwin.tar.gz" .

bash "$repo_root/scripts/package-macos-app.sh" \
  "$work/fugue-tools-aarch64-apple-darwin.tar.gz" "$version" "$work/out" >/dev/null

app="$work/out/Fugue.app"
[[ -d "$app" ]] || fail "no Fugue.app produced"

# All three binaries are co-located, and still executable. The layout is what
# lets `fugue setup` resolve the MCP binary next to its own executable, so the
# bundled app needs nothing on PATH.
for bin in fugue fugue-mcp fugue-setup; do
  [[ -x "$app/Contents/MacOS/$bin" ]] \
    || fail "Contents/MacOS/$bin is missing or not executable"
done

plist="$app/Contents/Info.plist"
plutil -lint "$plist" >/dev/null || fail "Info.plist is not a valid plist"

check_key() {
  # $1 = plist key, $2 = expected value
  local actual
  actual="$(/usr/libexec/PlistBuddy -c "Print :$1" "$plist" 2>/dev/null)" \
    || fail "Info.plist has no $1"
  [[ "$actual" == "$2" ]] || fail "Info.plist $1 is '$actual', expected '$2'"
}

# CFBundleExecutable must be the GUI, or double-clicking the app runs the wrong
# binary — the failure a user would hit first and understand least.
check_key CFBundleExecutable fugue-setup
check_key CFBundleIdentifier io.ilusiv.fugue.setup
check_key CFBundleShortVersionString "$version"
check_key CFBundleVersion "$version"

# A missing binary must fail loudly at packaging time, not produce a broken app.
incomplete="$work/incomplete"
mkdir -p "$incomplete"
partial="$work/partial"
mkdir -p "$partial"
printf '#!/bin/sh\n' > "$partial/fugue"
chmod +x "$partial/fugue"
tar -C "$partial" -czf "$work/fugue-tools-partial.tar.gz" .
if bash "$repo_root/scripts/package-macos-app.sh" \
    "$work/fugue-tools-partial.tar.gz" "$version" "$incomplete" >/dev/null 2>&1; then
  fail "packaging succeeded despite a missing binary"
fi

# The image must mount and carry the app plus a drag-to-Applications target.
bash "$repo_root/scripts/package-macos-dmg.sh" "$app" "$version" "$work/out" >/dev/null
dmg="$work/out/Fugue-$version.dmg"
[[ -f "$dmg" ]] || fail "no disk image produced"

mount_point="$(hdiutil attach "$dmg" -nobrowse -readonly \
  | sed -n 's|.*\(/Volumes/.*\)$|\1|p' | tail -1)"
[[ -n "$mount_point" ]] || fail "disk image did not mount"
# shellcheck disable=SC2064
trap "hdiutil detach '$mount_point' >/dev/null 2>&1 || true; rm -rf '$work'" EXIT

[[ -d "$mount_point/Fugue.app" ]] || fail "disk image has no Fugue.app"
[[ -x "$mount_point/Fugue.app/Contents/MacOS/fugue-setup" ]] \
  || fail "app in the image lost its executable bit"
[[ -L "$mount_point/Applications" ]] \
  || fail "disk image has no /Applications alias to drag onto"

echo "PASS: macOS app and disk image packaging"

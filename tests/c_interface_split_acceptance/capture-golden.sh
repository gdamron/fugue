#!/usr/bin/env bash
# Capture the golden files check.sh compares against, from main's C libraries.
#
# Run it once, on main, with the libraries main builds:
#
#   cargo build --release --features ffi --target <host>
#   tests/c_interface_split_acceptance/capture-golden.sh <dir holding libfugue.a and libfugue.dylib or .so>
#
# The checked-in golden files came from main at the commit before the C
# interface moved to its own crate, built for aarch64-apple-darwin with rustc
# 1.96.0 exactly as above, and packaged by main's scripts with the stand-in
# cargo-about from lib.sh. Don't rerun it against a later change: the golden
# files are what "the same as main" means.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
source "$HERE/lib.sh"

LIB_DIR="${1:?usage: capture-golden.sh <dir with the main build libfugue.a and shared library>}"
HOST="$(host_triple)"
GOLDEN="$HERE/golden"
mkdir -p "$GOLDEN"
case "$(uname -s)" in Darwin) shared=libfugue.dylib ;; *) shared=libfugue.so ;; esac

dylib_exports "$LIB_DIR/$shared" >"$GOLDEN/c-api.txt"
static_unmangled "$LIB_DIR/libfugue.a" >"$GOLDEN/static-unmangled-$HOST.txt"
{ rustc -V; nm --version 2>/dev/null | grep -i version | head -n 1; } >"$GOLDEN/static-unmangled-$HOST.toolchain"
sha "$ROOT/include/fugue.h" >"$GOLDEN/fugue.h.sha256"
sha "$ROOT/LICENSE" >"$GOLDEN/LICENSE.sha256"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
install_cargo_about_stub "$work/bin"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"

# Native archive for the host, from main's libraries.
mkdir -p "$work/target/$HOST/release" "$work/dist"
cp "$LIB_DIR/libfugue.a" "$LIB_DIR/$shared" "$work/target/$HOST/release/"
FUGUE_TARGET_DIR="$work/target" bash "$ROOT/scripts/package-native.sh" "$HOST" "$VERSION" "$work/dist" >/dev/null
archive_listing "$work/dist/fugue-$VERSION-$HOST.tar.gz" "$VERSION" "$HOST" >"$GOLDEN/native-listing.txt"
mkdir "$work/x" && tar -xzf "$work/dist/fugue-$VERSION-$HOST.tar.gz" -C "$work/x"
normalize "$work/x/fugue-$VERSION-$HOST/README.md" "$VERSION" "$HOST" >"$GOLDEN/native-README.md"
normalize "$work/x/fugue-$VERSION-$HOST/THIRD_PARTY_LICENSES.txt" "$VERSION" "$HOST" >"$GOLDEN/native-notices-stub-body.txt"

# Android archive from stub libraries (no NDK needed for the layout).
for t in aarch64-linux-android x86_64-linux-android; do
  mkdir -p "$work/target/$t/release" && echo stub >"$work/target/$t/release/libfugue.so"
done
FUGUE_TARGET_DIR="$work/target" bash "$ROOT/scripts/package-android.sh" "$VERSION" "$work/dist" >/dev/null
archive_listing "$work/dist/fugue-$VERSION-android.tar.gz" "$VERSION" android >"$GOLDEN/android-listing.txt"
mkdir "$work/a" && tar -xzf "$work/dist/fugue-$VERSION-android.tar.gz" -C "$work/a"
normalize "$work/a/fugue-$VERSION-android/README.md" "$VERSION" android >"$GOLDEN/android-README.md"

echo "captured golden files in $GOLDEN"

#!/usr/bin/env bash
# Assemble the combined "install unit" for each platform from the per-repo client
# archives (FUG-227).
#
# The `fugue` CLI (which includes `fugue serve`) and the `fugue-mcp` adapter are
# built, code-signed, and notarized in their own repos and published as
# `fugue-cli-<target>.tar.gz` / `fugue-mcp-<target>.tar.gz`. This script pairs
# the same-target archives and repackages both binaries into a single
# `fugue-tools-<target>.tar.gz` so users install one thing. Re-taring does not
# touch the Mach-O signatures, so the combined archive ships the identical
# signed, notarized binaries.
#
# The binaries are laid out flat at the archive root so the installer drops them
# into the same directory (`~/.fugue/bin`); co-locating them lets `fugue-mcp`
# find its sibling `fugue` daemon without relying on PATH.
#
# Usage: package-tools.sh <download-dir> <out-dir>
#   <download-dir>  directory holding the per-repo fugue-cli-*/fugue-mcp-* archives
#   <out-dir>       directory to write the combined fugue-tools-* archives into

set -euo pipefail

DOWNLOAD_DIR="${1:?download directory required}"
OUT_DIR="${2:?output directory required}"

mkdir -p "$OUT_DIR"

shopt -s nullglob
found=0
for cli_archive in "$DOWNLOAD_DIR"/fugue-cli-*.tar.gz; do
  base="$(basename "$cli_archive")"
  target="${base#fugue-cli-}"
  target="${target%.tar.gz}"
  mcp_archive="$DOWNLOAD_DIR/fugue-mcp-${target}.tar.gz"
  if [[ ! -f "$mcp_archive" ]]; then
    echo "::error::missing fugue-mcp archive for target ${target} (expected ${mcp_archive})" >&2
    exit 1
  fi

  stage="$(mktemp -d)"
  tar -C "$stage" -xzf "$cli_archive"
  tar -C "$stage" -xzf "$mcp_archive"

  combined="$OUT_DIR/fugue-tools-${target}.tar.gz"
  tar -C "$stage" -czf "$combined" .
  echo "Built ${combined}:"
  tar -tzf "$combined"
  rm -rf "$stage"
  found=1
done

if [[ "$found" -eq 0 ]]; then
  echo "::error::no fugue-cli-*.tar.gz archives found in ${DOWNLOAD_DIR}" >&2
  exit 1
fi

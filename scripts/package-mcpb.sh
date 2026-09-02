#!/usr/bin/env bash
# Build the Claude Desktop `.mcpb` one-click extension from the combined install
# unit (FUG-232).
#
# An `.mcpb` (MCP Bundle) is a zip holding a `manifest.json` plus the server it
# declares. Claude Desktop installs one by double-click and registers the MCP
# server itself, so a user never opens a terminal.
#
# This is a pure wrapper: it repacks the *already signed and notarized* binaries
# from `fugue-tools-<target>.tar.gz` (FUG-226/FUG-227) with a manifest. No new
# runtime, no rebuild — zipping does not touch the Mach-O signatures.
#
# Both binaries go into `server/` so `fugue-mcp` finds its sibling `fugue`
# daemon by co-location (the same arrangement the installer creates in
# `~/.fugue/bin`), and connect-or-spawn (FUG-231) lets the bundled adapter join
# a daemon the CLI already started instead of racing a second one.
#
# The manifest's `mcp_config` mirrors the Claude Desktop registration that
# `fugue setup` writes (FUG-229) — command plus empty args — with the command
# pointed at the extension directory instead of an absolute install path. Keep
# the two in step: `fugue-cli`'s `Host::ClaudeDesktop::desired_server` is the
# other half of this contract.
#
# One bundle is produced per target, because a bundle carries a single
# platform's binaries. Targets whose platform Claude Desktop does not run on are
# skipped.
#
# Usage: package-mcpb.sh <tools-dir> <out-dir> <version> [manifest-template]
#   <tools-dir>          directory holding the fugue-tools-<target>.tar.gz archives
#   <out-dir>            directory to write the .mcpb bundles into
#   <version>            release version without the v prefix (e.g. 2026.8.0)
#   [manifest-template]  manifest with __VERSION__/__MCP_BIN__/__PLATFORM__
#                        placeholders (default: packaging/mcpb/manifest.json)

set -euo pipefail

TOOLS_DIR="${1:?tools directory required}"
OUT_DIR="${2:?output directory required}"
VERSION="${3:?version required}"
TEMPLATE="${4:-$(dirname "$0")/../packaging/mcpb/manifest.json}"

if [[ ! -f "$TEMPLATE" ]]; then
  echo "::error::manifest template not found: ${TEMPLATE}" >&2
  exit 1
fi

# `mcpb pack` validates the manifest against the bundle spec and writes the zip
# with unix permission bits preserved, which the executables depend on.
mcpb() { npx --yes "@anthropic-ai/mcpb@${MCPB_CLI_VERSION:-2.1.2}" "$@"; }

# Claude Desktop ships for macOS and Windows only; a Linux bundle would be dead
# weight on the release. `platforms` values are the manifest spec's, not Rust's.
platform_for_target() {
  case "$1" in
    *-apple-darwin) printf 'darwin' ;;
    *-pc-windows-msvc) printf 'win32' ;;
    *) return 1 ;;
  esac
}

mkdir -p "$OUT_DIR"

shopt -s nullglob
found=0
for tools_archive in "$TOOLS_DIR"/fugue-tools-*.tar.gz; do
  base="$(basename "$tools_archive")"
  target="${base#fugue-tools-}"
  target="${target%.tar.gz}"

  if ! platform="$(platform_for_target "$target")"; then
    echo "Skipping ${target}: Claude Desktop does not run on that platform"
    continue
  fi

  if [[ "$platform" == "win32" ]]; then
    mcp_bin="fugue-mcp.exe"
    cli_bin="fugue.exe"
  else
    mcp_bin="fugue-mcp"
    cli_bin="fugue"
  fi

  stage="$(mktemp -d)"
  mkdir -p "$stage/server"
  tar -C "$stage/server" -xzf "$tools_archive"

  for bin in "$mcp_bin" "$cli_bin"; do
    if [[ ! -f "$stage/server/$bin" ]]; then
      echo "::error::${base} did not contain expected binary '${bin}'" >&2
      exit 1
    fi
    # The archives already carry the bit; assert it rather than trust it, since
    # a bundle whose server is not executable fails only at install time.
    if [[ ! -x "$stage/server/$bin" ]]; then
      echo "::error::${bin} in ${base} is not executable" >&2
      exit 1
    fi
  done

  sed -e "s|__VERSION__|${VERSION}|g" \
      -e "s|__MCP_BIN__|${mcp_bin}|g" \
      -e "s|__PLATFORM__|${platform}|g" \
      "$TEMPLATE" > "$stage/manifest.json"

  bundle="$OUT_DIR/fugue-${VERSION}-${target}.mcpb"
  mcpb pack "$stage" "$bundle"
  echo "Built ${bundle}"
  rm -rf "$stage"
  found=1
done

if [[ "$found" -eq 0 ]]; then
  echo "::error::no fugue-tools-*.tar.gz archives found in ${TOOLS_DIR}" >&2
  exit 1
fi

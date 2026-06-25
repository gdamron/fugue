#!/bin/sh
# Fugue installer — downloads the prebuilt `fugue` and `fugue-mcp` binaries from
# the public Fugue release and installs them into ~/.fugue/bin. No Rust toolchain
# required.
#
#   curl -fsSL https://raw.githubusercontent.com/gdamron/fugue/main/install.sh | sh
#
# Both binaries are published together on the `gdamron/fugue` GitHub release.
#
# Environment overrides:
#   FUGUE_BIN_DIR   install location          (default: $HOME/.fugue/bin)
#   FUGUE_VERSION   release tag to install     (default: latest, e.g. 2026.6.0 or v2026.6.0)
#
# Supported platforms: macOS arm64, Linux x86_64 (glibc), Linux arm64 (glibc).
# Intel Macs and Windows are not yet prebuilt — build from source instead.

set -eu

REPO="gdamron/fugue"

BIN_DIR="${FUGUE_BIN_DIR:-$HOME/.fugue/bin}"
VERSION="${FUGUE_VERSION:-latest}"

info() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$1" >&2; }
err()  { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || err "required command '$1' not found"; }

need curl
need tar
need uname

# --- detect platform → Rust target triple ------------------------------------
detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux)  os_part="unknown-linux-gnu" ;;
    *) err "unsupported OS '$os' (this installer supports macOS and Linux)" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *) err "unsupported architecture '$arch'" ;;
  esac
  printf '%s-%s' "$arch_part" "$os_part"
}

# Only these triples have prebuilt binaries on the release.
is_supported_target() {
  case "$1" in
    aarch64-apple-darwin | x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu) return 0 ;;
    *) return 1 ;;
  esac
}

# Build a release-asset download URL (handles the `latest` alias).
asset_url() {
  # $1 = asset filename
  if [ "$VERSION" = "latest" ]; then
    printf 'https://github.com/%s/releases/latest/download/%s' "$REPO" "$1"
  else
    # accept either "2026.6.0" or "v2026.6.0"
    tag="$VERSION"
    case "$tag" in v*) ;; *) tag="v$tag" ;; esac
    printf 'https://github.com/%s/releases/download/%s/%s' "$REPO" "$tag" "$1"
  fi
}

# --- checksum verification against the release SHA256SUMS.txt -----------------
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 1
  fi
}

verify_asset() {
  # $1 = downloaded file, $2 = asset basename. Uses $SUMS if it was fetched.
  file="$1"; asset="$2"
  [ -f "$SUMS" ] || { warn "no SHA256SUMS.txt available; skipping verification of $asset"; return 0; }
  # SHA256SUMS lines are "<hash>  <path>"; match on the basename of the path.
  expected="$(awk -v want="$asset" '{ n = $2; sub(/.*\//, "", n); if (n == want) { print $1; exit } }' "$SUMS")"
  [ -n "$expected" ] || { warn "no checksum entry for $asset; skipping verification"; return 0; }
  actual="$(sha256_of "$file")" || { warn "no sha256 tool found; skipping verification of $asset"; return 0; }
  [ "$expected" = "$actual" ] || err "checksum mismatch for $asset (expected $expected, got $actual)"
}

# --- download + extract a single binary --------------------------------------
install_binary() {
  # $1 = asset basename, $2 = binary name inside the archive
  asset="$1"; bin="$2"
  url="$(asset_url "$asset")"
  info "Downloading $bin ($asset)"
  curl -fSL --proto '=https' --tlsv1.2 -o "$TMP/$asset" "$url" \
    || err "failed to download $url"
  verify_asset "$TMP/$asset" "$asset"
  tar -C "$TMP" -xzf "$TMP/$asset"
  [ -f "$TMP/$bin" ] || err "archive $asset did not contain expected binary '$bin'"
  install -m 0755 "$TMP/$bin" "$BIN_DIR/$bin" 2>/dev/null \
    || { mkdir -p "$BIN_DIR" && cp "$TMP/$bin" "$BIN_DIR/$bin" && chmod 0755 "$BIN_DIR/$bin"; }
  info "Installed $bin → $BIN_DIR/$bin"
}

TARGET="$(detect_target)"
is_supported_target "$TARGET" \
  || err "no prebuilt binary for $TARGET (Intel Macs and Windows are not yet supported; build from source)"
info "Detected platform: $TARGET"

mkdir -p "$BIN_DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

# Fetch the combined checksum manifest once (best-effort).
SUMS="$TMP/SHA256SUMS.txt"
curl -fSL --proto '=https' --tlsv1.2 -o "$SUMS" "$(asset_url SHA256SUMS.txt)" 2>/dev/null \
  || { warn "could not fetch SHA256SUMS.txt; binaries will not be checksum-verified"; rm -f "$SUMS"; }

install_binary "fugue-cli-$TARGET.tar.gz" "fugue"
install_binary "fugue-mcp-$TARGET.tar.gz" "fugue-mcp"

# --- PATH guidance -----------------------------------------------------------
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    printf '\n'
    info "Add $BIN_DIR to your PATH to use 'fugue' and 'fugue-mcp':"
    printf '\n  export PATH="%s:$PATH"\n\n' "$BIN_DIR"
    printf 'Add that line to your shell profile (e.g. ~/.zshrc or ~/.bashrc).\n'
    ;;
esac

printf '\n'
info "Done. Register the MCP server with:"
printf '\n  claude mcp add fugue %s/fugue-mcp\n\n' "$BIN_DIR"

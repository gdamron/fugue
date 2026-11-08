# Helpers shared by check.sh and capture-golden.sh. Source it; don't run it.
#
# The symbol helpers print one name per line, sorted and unique, with the
# platform's leading underscore removed (Mach-O prefixes C names with "_"), so
# lists compare across macOS and Linux where the names agree.

# host_triple: the triple rustc builds for by default.
host_triple() { rustc -vV | sed -n 's/^host: //p'; }

# dylib_exports <shared-lib>: the C symbols the shared library exports.
dylib_exports() {
  local lib="$1"
  case "$(uname -s)" in
    Darwin) nm -gU "$lib" | awk 'NF >= 3 {print $3}' | sed 's/^_//' ;;
    *) nm -D --defined-only "$lib" | awk 'NF >= 3 {print $3}' ;;
  esac | sort -u
}

# static_unmangled <static-lib>: the defined external symbols of the static
# library that are not Rust-mangled (legacy "_ZN" or v0 "_R"), leaving out
# compiler-generated names that change with every build ("anon.<hash>",
# "<name>.llvm.<hash>") and the toolchain's compiler_builtins members. What
# remains is the C-callable surface: the fugue_* API plus the C symbols of
# bundled dependencies (ring's assembly, compiler-rt helpers), which change
# only with the toolchain or the lockfile. Apple's nm can't read the bitcode
# of the prebuilt std members and skips them (with an error, ignored here), so
# the list depends on the nm in use too; check.sh compares it only when rustc
# and nm match the ones the golden list was captured with.
static_unmangled() {
  local lib="$1"
  case "$(uname -s)" in
    Darwin) { nm -gU -A "$lib" 2>/dev/null || true; } | awk '{print $1, $NF}' ;;
    *) { nm -A --defined-only -g "$lib" 2>/dev/null || true; } | awk '{print $1, $NF}' ;;
  esac \
    | grep -v ':compiler_builtins-' \
    | awk '{print $2}' \
    | sed 's/^_//' \
    | grep -vE '^_?_ZN|^_?_R|^anon\.|\.llvm\.|^ltmp|^l_' \
    | sort -u
}

# c_api <symbol-list-file-or-stdin>: the fugue_* names, the public C API.
c_api() { grep -E '^fugue_' "${1:-/dev/stdin}" || true; }

# archive_listing <tar.gz> <version> <target>: the archive's entries, sorted,
# with the version and target replaced by placeholders and directory entries
# dropped, so listings compare across versions and targets.
archive_listing() {
  local archive="$1" version="$2" target="$3"
  tar -tzf "$archive" | sed 's|^\./||' | grep -v '/$' \
    | sed "s|$version|<VERSION>|g; s|$target|<TARGET>|g" | sort
}

# normalize <file> <version> <target>: the file with the version and target
# replaced by placeholders.
normalize() { sed "s|$2|<VERSION>|g; s|$3|<TARGET>|g" "$1"; }

# sha <file>: the file's SHA-256.
sha() { shasum -a 256 "$1" | awk '{print $1}'; }

# install_cargo_about_stub <dir>: put a stand-in cargo-about 0.9.3 first on
# PATH. It writes notices that pass generate-licenses.sh's guards but list no
# real crates, so archives can be built and their layout checked where the
# pinned cargo-about is not installed. The real notices are checked by
# scripts/check-release-notices.sh.
install_cargo_about_stub() {
  local dir="$1"
  mkdir -p "$dir"
  cat >"$dir/cargo-about" <<'EOF'
#!/usr/bin/env bash
[[ "${1:-}" == "about" ]] && shift
if [[ "${1:-}" == "--version" ]]; then echo "cargo-about 0.9.3"; exit 0; fi
out=""
while [[ $# -gt 0 ]]; do
  if [[ "$1" == "--output-file" ]]; then out="$2"; shift; fi
  shift
done
[[ -n "$out" ]] || { echo "stub cargo-about: no --output-file" >&2; exit 1; }
printf '  fugue 0.0.0 (stub notices)\nPermission is hereby granted, free of charge\nApache License\nVersion 2.0, January 2004\n' >"$out"
EOF
  chmod +x "$dir/cargo-about"
  export PATH="$dir:$PATH"
}

# have_real_cargo_about: the pinned cargo-about is installed.
have_real_cargo_about() {
  [[ "$(cargo about --version 2>/dev/null | awk '{print $2}')" == "0.9.3" ]]
}

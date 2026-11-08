#!/bin/sh
# Retired installer. The Fugue command-line tools and MCP server are installed
# from the Fugue product release now, so this file only points there: it prints
# the install command and exits non-zero. It makes no network request and
# writes no files, so `curl -fsSL <this file's raw URL> | sh` is safe to run.
#
# Keep it at the repository root: users still have the old one-line command
# (https://raw.githubusercontent.com/gdamron/fugue/main/install.sh) in scripts
# and notes.

printf '%s\n' \
  'error: this installer has moved. Install Fugue from the product release instead:' \
  '' \
  '  curl -fsSL https://github.com/Ilusiv-media/fyug/releases/latest/download/install.sh | sh' \
  '' \
  'Nothing was installed.' >&2

exit 1

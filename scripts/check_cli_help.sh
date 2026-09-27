#!/usr/bin/env bash
# Guard for `wit --help` / `wit --version` drift (crates/wit/tests/cli_help.rs):
#   1. every command's --help matches crates/wit/tests/snapshots/help/ (new
#      subcommands need a snapshot, removed ones leave no stale file);
#   2. `wit --version` / `-V` print CARGO_PKG_VERSION (release.yml and
#      publish-npm.yml check the built binaries against the tag and the npm
#      package version);
#   3. the README and AGENTS.md command tables and the README cloud cache claims
#      match the help. The help's cache claims themselves are proved against the
#      code by scripts/check_formal.sh (formal/Wit/CacheSource.lean).
set -euo pipefail
cd "$(dirname "$0")/.."

if ! cargo test -q -p wit --test cli_help; then
  echo "error: wit --help / --version drifted. If the help change is intended, run" >&2
  echo "  WIT_UPDATE_HELP_SNAPSHOTS=1 cargo test -p wit --test cli_help" >&2
  echo "review the snapshot diff, update README.md / AGENTS.md, and run bash scripts/check_formal.sh" >&2
  exit 1
fi
echo "wit --help snapshots, --version, and the README/AGENTS command tables agree"

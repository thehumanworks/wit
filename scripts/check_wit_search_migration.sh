#!/usr/bin/env bash
# Fail if `wit search` regresses back to grep.app.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

status=0

if rg -n 'GrepClient::new|repo_search\(|https://grep\.app|/api/search2' crates/wit/src >/dev/null 2>&1; then
  echo "error: grep.app wiring must not appear anywhere under crates/wit/src." >&2
  rg -n 'GrepClient::new|repo_search\(|https://grep\.app|/api/search2' crates/wit/src >&2 || true
  status=1
fi

exit "$status"

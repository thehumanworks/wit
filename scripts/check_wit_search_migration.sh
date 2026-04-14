#!/usr/bin/env bash
# Fail if `wit search` bypasses the migration guardrails:
# - GrepClient / repo_search must not be wired from cli.rs (use search_run.rs).
# - https://grep.app and search2 literals must not appear outside the allowlisted module.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

ALLOW_FILE="crates/wit/src/search_run.rs"
status=0

if rg -n 'GrepClient::new|repo_search\(' crates/wit/src/cli.rs 2>/dev/null; then
  echo "error: GrepClient::new or repo_search( must not appear in crates/wit/src/cli.rs (route through ${ALLOW_FILE})." >&2
  status=1
fi

while IFS= read -r -d '' f; do
  rel="${f#"${ROOT}/"}"
  if [[ "$rel" == "$ALLOW_FILE" ]]; then
    continue
  fi
  if rg -n 'https://grep\.app|/api/search2' "$f" >/dev/null 2>&1; then
    echo "error: grep.app API URL reference in ${rel} (allowed only in ${ALLOW_FILE})." >&2
    rg -n 'https://grep\.app|/api/search2' "$f" >&2 || true
    status=1
  fi
done < <(find crates/wit/src -name '*.rs' -print0)

exit "$status"

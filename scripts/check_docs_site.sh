#!/usr/bin/env bash
# Formatter/parser unit tests plus a fixture-backed wasm smoke for the Pages try-it.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

echo "==> docs try-it unit tests"
node --test \
  "$root/docs/try/format.test.js" \
  "$root/docs/try/commands.test.js" \
  "$root/docs/try/host.test.js"

echo "==> Pages assemble stamp helper"
bash "$root/scripts/pages_stamp_release.test.sh"

echo "==> no hardcoded v0.x.y in Pages source"
guard_files=(
  ".github/workflows/pages.yml"
  "docs/try/host.js"
  "docs/index.html"
)
for rel in "${guard_files[@]}"; do
  if grep -nE 'v0\.[0-9]+\.[0-9]+' "$root/$rel"; then
    echo "error: $rel must not hardcode a v0.x.y release tag" >&2
    exit 1
  fi
done

# Stage the same module the published host fetches first (same-origin).
# Local cargo output is a build input only — not a live Pages candidate.
same_origin="$root/docs/try/wit_snapshot.wasm"
if [[ ! -f "$same_origin" ]]; then
  release_wasm="$root/target/wasm32-unknown-unknown/release/wit_snapshot.wasm"
  debug_wasm="$root/target/wasm32-unknown-unknown/debug/wit_snapshot.wasm"
  if [[ -f "$release_wasm" ]]; then
    cp "$release_wasm" "$same_origin"
  elif [[ -f "$debug_wasm" ]]; then
    cp "$debug_wasm" "$same_origin"
  else
    if ! rustup target list --installed | grep -qx 'wasm32-unknown-unknown'; then
      rustup target add wasm32-unknown-unknown
    fi
    echo "==> build wit-snapshot wasm for docs smoke"
    cargo build -p wit-snapshot --target wasm32-unknown-unknown --no-default-features
    cp "$debug_wasm" "$same_origin"
  fi
fi

echo "==> docs try-it wasm smoke (demo/repo tree/ls/cat/rg/sed/head/tail)"
node "$root/docs/try/smoke.mjs"

echo "check_docs_site: ok"

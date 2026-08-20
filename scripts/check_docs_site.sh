#!/usr/bin/env bash
# Formatter/parser unit tests plus a fixture-backed wasm smoke for the Pages try-it.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

echo "==> docs try-it unit tests"
node --test "$root/docs/try/format.test.js" "$root/docs/try/commands.test.js"

need_wasm=1
for candidate in \
  "$root/docs/try/wit_snapshot.wasm" \
  "$root/target/wasm32-unknown-unknown/release/wit_snapshot.wasm" \
  "$root/target/wasm32-unknown-unknown/debug/wit_snapshot.wasm"
do
  if [[ -f "$candidate" ]]; then
    need_wasm=0
    break
  fi
done

if [[ "$need_wasm" -eq 1 ]]; then
  if ! rustup target list --installed | grep -qx 'wasm32-unknown-unknown'; then
    rustup target add wasm32-unknown-unknown
  fi
  echo "==> build wit-snapshot wasm for docs smoke"
  cargo build -p wit-snapshot --target wasm32-unknown-unknown --no-default-features
fi

echo "==> docs try-it wasm smoke (demo/repo tree/ls/cat)"
node "$root/docs/try/smoke.mjs"

echo "check_docs_site: ok"

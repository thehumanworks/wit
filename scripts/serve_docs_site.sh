#!/usr/bin/env bash
# Local static serve of the GitHub Pages try-it (docs/ on port 8765).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! rustup target list --installed | grep -qx 'wasm32-unknown-unknown'; then
  rustup target add wasm32-unknown-unknown
fi

echo "==> build wit-snapshot wasm (no reqwest)"
cargo build -p wit-snapshot --release --target wasm32-unknown-unknown --no-default-features
cp "$root/target/wasm32-unknown-unknown/release/wit_snapshot.wasm" "$root/docs/try/wit_snapshot.wasm"

echo "==> http://127.0.0.1:8765/  (ctrl-c to stop)"
echo "    try: wit tree demo/repo"
exec python3 -m http.server 8765 --directory "$root/docs"

#!/usr/bin/env bash
# Build wit-snapshot for wasm32 without reqwest, then run the wasmtime fixture host.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! rustup target list --installed | grep -qx 'wasm32-unknown-unknown'; then
  rustup target add wasm32-unknown-unknown
fi

echo "==> cargo check wit-snapshot (wasm32, no default features / no reqwest)"
cargo check -p wit-snapshot --target wasm32-unknown-unknown --no-default-features

echo "==> cargo build wit-snapshot wasm32 module"
cargo build -p wit-snapshot --target wasm32-unknown-unknown --no-default-features

wasm="$root/target/wasm32-unknown-unknown/debug/wit_snapshot.wasm"
if [[ ! -f "$wasm" ]]; then
  echo "missing $wasm" >&2
  exit 1
fi

# Guard: wasm build must not pull reqwest into the dependency graph for this target.
echo "==> ensure reqwest is not a wasm32 dependency"
if cargo tree -p wit-snapshot --target wasm32-unknown-unknown --no-default-features -i reqwest >/dev/null 2>&1; then
  echo "reqwest must not appear in wit-snapshot wasm32 dependency tree" >&2
  cargo tree -p wit-snapshot --target wasm32-unknown-unknown --no-default-features -i reqwest || true
  exit 1
fi
echo "reqwest: not present (ok)"

echo "==> stage wasm next to browser demo (optional local use)"
cp "$wasm" "$root/crates/wit-snapshot/demo/browser/wit_snapshot.wasm"

echo "==> wasmtime fixture smoke (module runs; not browser-ready)"
cargo run -p wit-snapshot --features wasmtime-fixture --bin wit-snapshot-wasmtime-fixture -- "$wasm"

echo "check_wit_snapshot_wasm: ok"

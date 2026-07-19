#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
fixture="$repo_root/crates/wit/codemode.wit.d.ts"
generated="$(mktemp "${TMPDIR:-/tmp}/codemode.wit.d.ts.XXXXXX")"
trap 'rm -f "$generated"' EXIT

cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -p wit \
  --example generate_codemode_declarations >"$generated"
mv "$generated" "$fixture"

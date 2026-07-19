#!/usr/bin/env bash
set -euo pipefail

workflow=.github/workflows/quickjs-spike.yml
adr=docs/adr/0003-child-process-quickjs-feasibility.md
manifest=crates/wit-quickjs-spike/Cargo.toml

fail() {
  echo "error: $1" >&2
  exit 1
}

for target in \
  x86_64-unknown-linux-musl \
  aarch64-unknown-linux-musl \
  x86_64-apple-darwin \
  aarch64-apple-darwin \
  x86_64-pc-windows-msvc \
  aarch64-pc-windows-msvc
do
  grep -Fq -- "$target" "$workflow" || fail "QuickJS workflow omits $target"
done

grep -Fq 'rquickjs = { version = "0.12.1"' "$manifest" || \
  fail "spike must pin the tested rquickjs minor version"
grep -Fq 'cargo test --locked --release -p wit-quickjs-spike --test containment' "$workflow" || \
  fail "workflow must execute native release-mode containment tests"
grep -Fq 'Decision: NO-GO pending release-matrix evidence' "$adr" || \
  fail "ADR must not silently approve QuickJS before mandatory gates pass"
grep -Fq 'Linux ARM64 musl' "$adr" || fail "ADR lacks Linux ARM64 runtime plan"
grep -Fq 'Intel macOS' "$adr" || fail "ADR lacks Intel macOS runtime plan"
grep -Fq 'Windows ARM64' "$adr" || fail "ADR lacks Windows ARM64 runtime plan"

if grep -Fq 'continue-on-error' "$workflow"; then
  fail "mandatory QuickJS gates must not continue on error"
fi

echo "QuickJS feasibility contract is consistent"

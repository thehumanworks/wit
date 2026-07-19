#!/usr/bin/env bash
set -euo pipefail

if ! command -v tsc >/dev/null 2>&1; then
  echo "TypeScript compiler not found; install it as a development tool to check Code Mode declarations" >&2
  exit 1
fi

tsc \
  --noEmit \
  --strict \
  --target ES2022 \
  --module ESNext \
  --moduleResolution Bundler \
  crates/wit/tests/fixtures/codemode.example.ts

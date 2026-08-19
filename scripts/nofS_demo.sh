#!/usr/bin/env bash
# No-FS demo helpers for the in-memory snapshot backend.
#
# Preferred live recording (native, asserts zero WIT_CACHE_DIR writes):
#   bash scripts/nofS_demo.sh live
#
# Fixture path (no GitHub network; same memory list/read code):
#   bash scripts/nofS_demo.sh fixture
#
# Full wit CLI → wasm32 is not viable in this slice (gix bare clones, fs2 locks,
# tempfile, and process::Command). See docs/nofS-snapshot.md.

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

mode="${1:-live}"

case "$mode" in
  live)
    probe="$(mktemp -d)"
    export WIT_CACHE_DIR="$probe"
    cargo run -p wit-snapshot --features demo --bin wit-nofS-demo -- \
      live -r octocat/Hello-World --op all --cache-probe-dir "$probe"
    echo "probe dir after run:"
    find "$probe" -type f | wc -l
    ;;
  fixture)
    cargo run -p wit-snapshot --features demo --bin wit-nofS-demo -- \
      fixture \
      --tree-json crates/wit-snapshot/tests/cassettes/demo_tree.json \
      --blob-json crates/wit-snapshot/tests/cassettes/demo_blob.json \
      --read-path README.md
    ;;
  cli-memory)
    probe="$(mktemp -d)"
    export WIT_CACHE_DIR="$probe"
    cargo run -p wit --bin wit -- tree -r octocat/Hello-World --backend memory
    cargo run -p wit --bin wit -- ls -r octocat/Hello-World --backend memory
    cargo run -p wit --bin wit -- cat -r octocat/Hello-World README --backend memory
    cargo run -p wit --bin wit -- rg 'Hello' -r octocat/Hello-World --backend memory
    cargo run -p wit --bin wit -- head -n 5 -r octocat/Hello-World README --backend memory
    cargo run -p wit --bin wit -- tail -n 5 -r octocat/Hello-World README --backend memory
    cargo run -p wit --bin wit -- sed -e 's/Hello/Hi/' octocat/Hello-World README --backend memory
    cargo run -p wit --bin wit -- sed -n '1,5p' octocat/Hello-World README --backend memory
    cargo run -p wit --bin wit -- cache -r octocat/Hello-World --backend memory
    cargo run -p wit --bin wit -- branches -r octocat/Hello-World --backend memory
    count="$(find "$probe" -type f | wc -l | tr -d ' ')"
    if [[ "$count" != "0" ]]; then
      echo "ERROR: memory backend wrote $count files under $probe" >&2
      exit 1
    fi
    echo "cli-memory: ok (0 cache files under $probe)"
    ;;
  *)
    echo "usage: $0 [live|fixture|cli-memory]" >&2
    exit 2
    ;;
esac

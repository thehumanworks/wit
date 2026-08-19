# No-filesystem repository snapshots

Status: shipped for the repo-reading CLI parity slice (tree/ls/cat/rg/sed/head/tail +
cache pin + branches API) and MCP open/list/read/search/context on memory.

## What changed

`wit` still defaults to the disk cache (`gix` bare clones under `WIT_CACHE_DIR`,
serialized with `fs2`). That path cannot move onto Cloudflare Workers / WASM as-is.

This slice adds a second backend:

| Backend | Mechanism | Disk cache writes | Surfaces |
|---------|-----------|-------------------|----------|
| `disk` (default) | existing bare-repo cache | yes | all CLI reads; MCP open/list/read/search/context |
| `memory` | GitHub REST trees/blobs into RAM | **none** | CLI `tree`/`ls`/`cat`/`rg`/`sed`/`head`/`tail`; `cache` pin/prefetch; `branches` via API; MCP `wit_open`/`wit_list`/`wit_read`/`wit_search_code`/`wit_context` |

Shared contract: `wit_snapshot::{SnapshotBackend, RepoSnapshot}` with provenance
(`repo`, resolved ref, `commit_sha`, `tree_sha`, backend label, cache state).

## Why full CLI WASM is still impossible

Evidence from the disk path in `crates/wit/src/gitops/ops.rs`:

- `cache_github_repo` writes bare clones under `WIT_CACHE_DIR` / `.wit/cache`
- `fs2::FileExt` advisory locks (`.cache.lock`) require a real filesystem
- `gix` repository handles are filesystem-backed object stores
- Disk MCP snapshots still use `tempfile::TempDir` + `git clone --bare` when `WIT_SNAPSHOT_BACKEND` is unset/`disk`

Lifting that stack into Workers/WASM would fake a filesystem, not remove the dependency.
The memory backend is the honest no-FS path for repo reads; it does not replace disk.

## MCP memory path (SLT ship gate)

Set `WIT_SNAPSHOT_BACKEND=memory` before starting `wit mcp` (or `wit-mcp`). Then:

- `wit_open` loads via `MemoryBackend` (GitHub REST trees/blobs into RAM)
- `wit_list` / `wit_read` / `wit_search_code` / `wit_context` serve that in-memory snapshot
- `cache.state` in the open response is **`memory`** (Architect lock)
- No `PathBuf` / `TempDir` / `gix` / git CLI / `fs2` / `WIT_CACHE_DIR` on that path

MCP request/response types are unchanged. Disk remains the default.

```bash
WIT_SNAPSHOT_BACKEND=memory wit mcp --transport stdio --mode direct
```

## How to run the no-FS demo

```bash
# 1) Dedicated demo binary (asserts zero cache writes)
bash scripts/nofS_demo.sh live

# 2) Fixture-only (no GitHub; same memory list/read logic)
bash scripts/nofS_demo.sh fixture

# 3) Real CLI flags against a public repo, still zero cache files
bash scripts/nofS_demo.sh cli-memory

# Equivalent manual commands:
cargo run -p wit --bin wit -- tree -r octocat/Hello-World --backend memory
cargo run -p wit --bin wit -- ls  -r octocat/Hello-World --backend memory
cargo run -p wit --bin wit -- cat -r octocat/Hello-World README --backend memory
cargo run -p wit --bin wit -- rg 'Hello' -r octocat/Hello-World --backend memory
cargo run -p wit --bin wit -- head -n 5 -r octocat/Hello-World README --backend memory
cargo run -p wit --bin wit -- tail -n 5 -r octocat/Hello-World README --backend memory
cargo run -p wit --bin wit -- sed -e 's/Hello/Hi/' -r octocat/Hello-World README --backend memory
cargo run -p wit --bin wit -- cache -r octocat/Hello-World --backend memory
cargo run -p wit --bin wit -- branches -r octocat/Hello-World --backend memory
# or: WIT_SNAPSHOT_BACKEND=memory wit tree -r octocat/Hello-World
```

A wasmtime **networked** WASI build of the full CLI is not shipped. The memory
backend + `wit-nofS-demo` are the honest no-FS surface; a future Worker/wasi-http
binding can call the same `MemoryBackend` without inventing a disk.

## Failure handling

The memory backend returns typed errors for:

- GitHub rate limits (`403`/`429` with rate-limit body)
- Private / inaccessible repos (`401`/`403`/`404`, or `private: true`)
- Oversized recursive trees (`truncated: true` or entry cap)
- Oversized blobs (default 1 MiB)
- Missing paths / directory-vs-file misuse
- Binary blobs (NUL bytes)
- Memory budget pressure on the in-process blob cache

Unit/integration coverage lives in `crates/wit-snapshot/tests/memory_backend.rs`
(wiremock + in-memory HTTP doubles; no live GitHub flakiness),
`operations::tests` memory MCP paths, and `crates/wit/tests/memory_cli_integration.rs`.

# ADR 0004: WASM Snapshot via Fetch Client, Not a Third Backend

- Status: Accepted
- Date: 2026-08-19

## Context

`wit-snapshot` already serves public-repo `open` / `list` / `read` from RAM through `MemoryBackend<C: GitHubHttpClient>`. Native production uses `ReqwestGitHubClient` (`get_json(path) -> (status, body)`). The disk/gix path stays in the `wit` crate and is not part of this stretch.

Tomas wants Rust compiled to WebAssembly so an agent (or Tomas) can run those three calls in a browser/WASM host with no native binary and no disk. Shipping the full `wit` CLI, `gix`, stdio MCP, or `reqwest` onto wasm32 is the path we already rejected.

GitHub's REST API does not grant browser CORS. A crate-level CORS proxy or a Cloudflare Worker product would be a third backend and is out of this cut.

## Decision

Keep one snapshot impl. Add one HTTP impl.

1. `MemoryBackend<C: GitHubHttpClient>` stays the only no-FS snapshot backend.
2. `GitHubHttpClient::get_json` stays the only HTTP seam. Do not add snapshot methods for WASM.
3. Add `FetchGitHubClient` for wasm32 that implements the same `get_json` contract. The JS or WASI host supplies `fetch` (live GitHub, a same-origin proxy, or a fixture).
4. Native `ReqwestGitHubClient` remains behind the existing `http` feature. wasm32 must not depend on `reqwest`.
5. Compile `wit-snapshot` only to `wasm32-unknown-unknown`. Export `open` / `list` / `read` plus the existing typed errors (`rate_limit`, `oversized`, `not_found`, `binary`, `private_repo`, `oom`). Host/fetch failure maps onto those or `Api` — it is not a third impl.
6. Browser demo is those three exports in-page. wasmtime + fixture is CI evidence that the wasm module runs; it must not be labeled browser-ready.
7. Document this cut in-repo (this ADR plus a short how-to). Do not replace disk as the native default.

## Consequences

Positive:

- The no-FS design is reusable across native, wasmtime, and browser without a `WasmSnapshotStore`.
- CORS and auth stay with the host, where they belong.
- Typed errors stay one set.

Tradeoffs:

- Live `api.github.com` from a bare browser page will often fail CORS; the host must proxy or inject a fixture for a live-looking demo.
- rg / sed / head / tail stay native views; they are not wasm exports in this cut.
- Two HTTP clients to keep in contract (`reqwest` vs `fetch`).

## How-to

See [0004-wasm-fetch-howto.md](./0004-wasm-fetch-howto.md) for build flags, host `http_get`, CORS, the in-page fixture demo, and the wasmtime CI smoke.

## Alternatives Considered

1. WASM the full `wit` CLI:
   - Rejected: tokio, gix, stdio MCP, and clap are not a browser product.
2. `reqwest` on wasm32:
   - Rejected: it is not the host `fetch` Tomas asked for and fights the wasm32 feature cut.
3. A third `SnapshotBackend` for WASM:
   - Rejected: the missing piece is HTTP, not a new snapshot store.
4. Cloudflare Worker / wasi-http as the product:
   - Rejected: stretch users are agents in a browser/WASM host, not a Worker deploy.
5. In-crate CORS proxy:
   - Rejected: that is infrastructure, not `wit-snapshot`.

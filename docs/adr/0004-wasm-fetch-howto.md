# ADR 0004 how-to: wasm32 fetch client for wit-snapshot

Companion to [0004-wasm-fetch-snapshot-client.md](./0004-wasm-fetch-snapshot-client.md).

## What you get

- One snapshot impl: `MemoryBackend<C: GitHubHttpClient>`
- One new HTTP impl for wasm32: `FetchGitHubClient` (`get_json` → host `fetch`)
- Native `ReqwestGitHubClient` stays behind the `http` feature (not linked on wasm32)
- Wasm exports: `wit_snapshot_open`, `wit_snapshot_list`, `wit_snapshot_read`
  plus typed error codes (`rate_limit`, `oversized`, `not_found`, `binary`,
  `private_repo`, `oom`, `api`, `other`)

This cut does **not** ship the full `wit` CLI, disk cache, stdio MCP, or
rg/sed/head/tail as wasm exports.

## Build the module (no reqwest)

```bash
rustup target add wasm32-unknown-unknown
cargo build -p wit-snapshot --target wasm32-unknown-unknown --no-default-features
```

Artifact: `target/wasm32-unknown-unknown/debug/wit_snapshot.wasm`.

CI runs the same check and a **wasmtime + fixture** smoke (module runs; not
browser-ready certification).

## Host-supplied fetch

The guest imports:

| Module | Function | Contract |
|--------|----------|----------|
| `wit_snapshot_host` | `wit_snapshot_host_http_get` | Sync GET. On success: return `0`, write HTTP status (`u16`), allocate body via `wit_snapshot_alloc`, write `body_ptr` / `body_len`. Non-zero → guest maps to `SnapshotError::Api`. |

Relative paths look like `/repos/{owner}/{repo}/...`. Absolute `https://...`
URLs are passed through when the client has a base URL.

The host may implement the import as:

1. **Fixture map** (demo / CI) — path → `(status, json)`
2. **Same-origin proxy** — browser calls your origin; server talks to GitHub
3. **Live GitHub** — fine in wasmtime/WASI or native; usually blocked in a bare browser tab

Host/fetch failures must not invent a third snapshot backend or a new error
family — map them onto the existing typed errors (`Api`, or HTTP status →
`RateLimited` / `PrivateRepo` once a body/status is returned).

## CORS

`api.github.com` does not grant arbitrary browser origins. A page that
`fetch("https://api.github.com/...")` from localhost will typically fail CORS
before your Rust code sees a status code.

Practical options:

- Serve a **same-origin proxy** (e.g. `/github/*` → `api.github.com/*`) and point
  the host import at that proxy
- **Inject fixtures** for demos (what `demo/browser` does)
- Run outside the browser (wasmtime fixture host) where CORS does not apply

Do not treat a Cloudflare Worker or in-crate CORS proxy as a second
`SnapshotBackend` — that infrastructure stays in the host.

## Browser demo (fixtures in-page)

```bash
cargo build -p wit-snapshot --target wasm32-unknown-unknown --no-default-features
cp target/wasm32-unknown-unknown/debug/wit_snapshot.wasm \
  crates/wit-snapshot/demo/browser/
cd crates/wit-snapshot/demo/browser
python3 -m http.server 8765
# open http://127.0.0.1:8765/
```

Buttons call `open` → `list` → `read` against `demo/repo` fixtures. This shows
the three exports wired in-page; it is not a production browser product.

## wasmtime + fixture (CI evidence)

```bash
bash scripts/check_wit_snapshot_wasm.sh
```

Builds the wasm32 module without `http`/reqwest, then runs
`wit-snapshot-wasmtime-fixture` which links fixture `http_get` and asserts
open/list/read. That proves the module runs under wasmtime — it does **not**
mean the browser surface is ready.

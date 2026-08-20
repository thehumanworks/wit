# ADR 0005: wit as a client-side URL API (host adapter, not a third backend)

- Status: Accepted
- Date: 2026-08-20

## Context

Agents and humans already use `wit tree|ls|cat --backend memory` for public
(and PAT-gated) GitHub repos. Tomas wants the same plaintext behind URLs so
`curl https://host/tree/{owner}/{repo}` matches the CLI — without shipping the
full CLI to wasm, without disk / `WIT_CACHE_DIR`, and without inventing a third
`SnapshotBackend`.

ADR 0004 already fixed the HTTP seam: `MemoryBackend<C: GitHubHttpClient>` plus
wasm32 `FetchGitHubClient` (`open` / `list` / `read`). A bare static file host
still cannot run that wasm for `curl`.

## Decision

1. **Reuse only** `MemoryBackend` + existing wasm exports (`open` / `list` /
   `read`). No new Rust snapshot type. No wasm of the `wit` CLI crate.
2. Ship **two host adapters** that sit in front of `get_json` (same role as
   ADR 0004’s host `http_get`):
   - **Static site** (HTML + JS + wasm): path routing renders plaintext in-page.
   - **Tiny Cloudflare Pages Function** (or equivalent single fetch handler):
     loads the same wasm and returns `text/plain` so `curl` works.
3. This Worker/Pages function is **not** a product server and **not** a new
   backend — it is the host adapter.
4. **Route table (locked):**
   - `GET /tree/{owner}/{repo}?path=&branch=&depth=`
   - `GET /ls/{owner}/{repo}?path=&branch=`
   - `GET /cat/{owner}/{repo}?path=` (path required)
   - `?ref=` aliases `branch`. Path is always a query param (never a path
     segment). No `--backend` — this host is memory-only.
5. **Auth:** `Authorization: Bearer <pat>` or `Authorization: token <pat>`
   preferred. `?token=` accepted as fallback but **must be documented as
   leaky** (logs, Referer). Tokens must **never** appear in logs, traces, or
   error bodies.
6. Reuse the existing **24h per-repo@ref** host cache in the browser; the
   worker may keep the same map for the isolate lifetime.
7. Leave out of this cut: `/head` `/tail` `/rg` `/sed` `/branches`, search-repos,
   Code Mode, Node API servers, private-repo guarantees beyond passing the PAT
   through, and deploying to any production domain.

## Honest constraint

A purely static file host cannot execute wasm on behalf of `curl`. The browser
can (GitHub REST allows CORS, including `Authorization`). Therefore both the
static page and the tiny Pages Function ship together.

## Consequences

Positive:

- One snapshot impl stays true; curl and browser share formatting + wasm.
- Auth and caching stay in the host, where they belong.

Tradeoffs:

- Sync wasm `http_get` requires async prefetch into the host cache before
  `open` / `read`.
- Debug wasm payloads are large; release builds are preferred for deploys.

## How-to

See [0005-url-api-howto.md](./0005-url-api-howto.md).

## Alternatives considered

1. WASM the full CLI — rejected (ADR 0004).
2. Third `SnapshotBackend` — rejected.
3. Node/Express product API — rejected (out of scope; Worker is host-only).
4. Path segments for file paths (`/cat/o/r/README`) — rejected; Architect locked
   path as query.

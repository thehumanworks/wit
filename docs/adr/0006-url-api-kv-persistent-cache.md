# ADR 0006: Workers KV Persistent Cache for the URL API Worker

- Status: Accepted
- Date: 2026-08-29

## Context

The ADR 0005 URL API worker keeps its `RepoSnapshotCache` (24h per-repo@ref
map serving the wasm guest's sync `http_get`) at module scope, so it only
lives for the Cloudflare isolate lifetime. A recycled or cold isolate pays
the full GitHub round trips again (repo → commit → recursive tree, then
blobs), and every request re-ran the async prefetch against GitHub even when
the isolate cache was warm.

We want worker-level caching that is not in memory. Durable Objects were the
first candidate.

## Decision

Use **Workers KV**, not a Durable Object, as the persistence layer behind the
existing in-memory `RepoSnapshotCache` — and short-circuit the async prefetch
when a live open entry exists.

1. Keep the sync in-memory Map as the hot path (wasm imports are sync). KV is
   an async hydrate/persist layer around it, the same seam the browser demo
   uses for IndexedDB. Not a third `SnapshotBackend`.
2. New `lib/persistent-cache.js` (`KvRepoCache`, one instance per request):
   - `hydrateOpen`: before prefetch, load the repo@ref entry (metadata + slim
     tree, no blobs) from KV into the sync cache; default-branch requests
     resolve through a small `default:{ownerRepo}` alias key.
   - `hydrateBlob`: before a `cat` blob prefetch, load that one blob from KV.
   - `persistRepo`: after a successful response (via `ctx.waitUntil` when
     available), write rows this request produced; rows hydrated from KV are
     not rewritten.
3. Storage layout, all JSON, all with `expirationTtl` equal to the entry's
   remaining in-memory TTL (min 60s, KV's floor):
   - `v1:repo:{ownerRepo}@{resolvedRef}` → entry without blobs
   - `v1:default:{ownerRepo}` → `{ defaultBranch }`
   - `v1:blob:{ownerRepo}:{sha}` → `{ size, contentBase64 }`
   Blobs are separate keys so `cat` never rewrites the tree row and a tree
   refresh never drops cached blobs.
4. `prefetchOpen` returns early from a live "open entry" (tree + commit
   present, not the synthetic `_blobs` bucket; for default-branch requests
   only an entry whose resolved ref is its own default branch qualifies), so
   both the isolate cache and KV actually absorb GitHub traffic.
5. The `WIT_REPO_CACHE` binding is optional and every KV failure is
   best-effort: hydrate/persist errors are logged (scrubbed) and the request
   falls back to plain GitHub reads. Without the binding the worker behaves
   exactly as before.
6. `npm run dev` passes `--kv=WIT_REPO_CACHE` for local simulated KV;
   production binds a namespace to the Pages project (dashboard, or the
   commented `[[kv_namespaces]]` block in `wrangler.toml`).

## Why not a Durable Object

1. A Pages advanced-mode `_worker.js` cannot export Durable Object classes;
   a DO would require a second deployed Worker service just to host the
   class, against ADR 0005's "tiny host adapter, not a product server".
2. The cached data is immutable content addressed by SHA (trees by tree SHA,
   blobs by blob SHA). KV's eventual consistency is harmless there, and the
   only mutable piece — ref → commit resolution — carries the cache TTL,
   which maps directly onto KV `expirationTtl`.
3. DO key-value storage caps values at 128 KiB (SQLite rows at 2 MB), which
   would force chunking for recursive trees and blobs. KV allows 25 MB
   values; oversized rows are simply not persisted.
4. The cache needs no coordination, transactions, or strong consistency —
   the DO features we would pay latency and moving parts for.

## Consequences

Positive:

- Warm reads survive isolate recycling and are shared across isolates and
  regions; a cold isolate with a warm KV serves `tree`/`ls`/`cat` with zero
  GitHub requests (covered by tests).
- Warm-isolate requests also stop re-fetching repo/commit/tree from GitHub.
- Freshness semantics are unchanged: same 24h default TTL, `?ttlMs=`/`?ttl=`
  still apply, KV expiry tracks the in-memory expiry.

Tradeoffs:

- KV is eventually consistent: a just-written entry may briefly be invisible
  in another region — acceptable for a read cache keyed by immutable SHAs.
- Cached content can be up to TTL stale relative to the branch head (already
  true of the in-memory cache; the CLI's SHA-revalidation from ADR 0002 does
  not apply to the URL API).
- Private-repo content fetched with a PAT lands in the shared namespace keyed
  only by repo@ref, readable by later unauthenticated requests within the
  TTL. This matches the existing in-isolate behavior (the isolate cache
  already had no per-token identity) but persistence widens the window;
  ADR 0005 already scopes the host to best-effort private-repo support.

## Alternatives Considered

1. Durable Object with a companion Worker service: rejected (see above).
2. Cloudflare Cache API: per-datacenter and evictable, not truly persistent;
   also awkward for JSON sub-resources behind one logical entry.
3. R2: no native per-object TTL (lifecycle rules are day-granular) and no
   benefit over KV at these value sizes.

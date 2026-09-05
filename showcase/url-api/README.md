# wit URL API

Read GitHub repositories over plain URLs — the MemoryBackend wasm behind a
Cloudflare Pages Worker (ADR 0005) with agent-grade verbs (ADR 0007).

Live: `https://wit.thehuman.sh/api` · Agent guide: `/api/llms.txt` · OpenAPI:
`/api/openapi.json`. Deploys via `.github/workflows/url-api-deploy.yml`
(Cloudflare Pages project `wit-url-api`), not GitHub Pages.

```bash
B=https://wit.thehuman.sh/api
curl "$B/stats/ratatui/ratatui"                                  # size / tokens / languages, no blob reads
curl "$B/tree/ratatui/ratatui?path=src/widgets&depth=1&l=1"      # listing with bytes + ~tokens
curl "$B/outline/ratatui/ratatui?path=src/widgets/block.rs"      # symbols with line ranges
curl "$B/cat/ratatui/ratatui?path=src/widgets/block.rs&lines=1-60&n=1"
curl "$B/rg/ratatui/ratatui?q=impl%20Widget&glob=*.rs&l=1"       # files only (cheapest locate)
curl "$B/rg/ratatui/ratatui?q=fn%20render&path=src/widgets&C=2&max=20"
curl "$B/refs/ratatui/ratatui"                                   # branches + tags for ?ref=
curl "$B/commits/ratatui/ratatui?path=src/lib.rs&n=5"
curl "$B/search?q=terminal%20ui&lang=rust&limit=5"               # find owner/repo
curl -H "Accept: application/json" "$B/ls/ratatui/ratatui?path=src"
```

## Routes

Repository verbs take `/{verb}/{owner}/{repo}`; the file path is always
`?path=`, never a path segment. A leading `/api` is optional for the verbs
and required for discovery (`/api`, `/api/openapi.json`, `/api/llms.txt`).

| verb | params |
|------|--------|
| `stats` | `path`, `largest` (default 10), `ignore` |
| `tree` | `path`, `depth`, `l=1`, `ignore` |
| `ls` | `path`, `l=1`, `ignore` |
| `outline` | `path` (required), `max_symbols` |
| `cat` | `path` (required), `lines=A-B` / `start` / `end`, `n=1` |
| `head` | `path` (required), `lines` (default 10), `n=1` |
| `tail` | `path` (required), `lines`, `plus=N`, `n=1` |
| `rg` | `q` (required), `path`, `glob`, `i`, `S`, `w`, `v`, `l`, `c`, `C`/`B`/`A`, `max` (≤2000), `max_files` (≤1000), `long`, `ignore` |
| `refs` | — |
| `commits` | `path`, `n` (≤100), `ref` |
| `search` | `q`, `p`, `lang`, `limit` (≤100), `sort=stars\|updated\|forks\|best` |

Common: `ref=` (alias `branch=`; branch, tag, or full commit SHA), `fresh=1`
(re-resolve the ref), `format=json` (or `Accept: application/json`).

Every response carries `x-wit-repo`, `x-wit-ref`, `x-wit-commit`,
`x-wit-cache: hit|miss`, and `x-wit-auth: caller|host|anonymous`. Plaintext
is byte-for-byte the `wit … --backend memory` CLI output; JSON carries the
same provenance fields in the body. Errors are `error: <message>` or
`{"error","code","status"}`; a 429 also sets `retry-after`.

## Auth and quotas

- Callers may send `Authorization: Bearer <token>` (or `token <token>`) for
  private repositories and their own GitHub quota. `?token=` is accepted as a
  fallback but leaks through logs and Referer — avoid it. Tokens never appear
  in logs, traces, or error bodies.
- The host uses its own `GITHUB_TOKEN` secret when the caller sends none.
  Without it anonymous GitHub calls share the egress IP's 60 req/h quota,
  which is permanently exhausted on Cloudflare, so nearly every request fails
  with 429. Configure it once:

  ```bash
  npx wrangler pages secret put GITHUB_TOKEN --project-name wit-url-api
  ```

  Use a fine-grained token with public repository read access only.
- Blob bytes come from `raw.githubusercontent.com` pinned by commit SHA (no
  REST quota) with the blob endpoint as fallback; set `WIT_RAW_BLOBS=0` to
  use the REST endpoint only.

## Persistent worker cache (Workers KV)

The worker's in-memory `RepoSnapshotCache` only lives for the isolate
lifetime. When the optional `WIT_REPO_CACHE` KV binding is present, the
worker hydrates that cache from KV before a request and persists new rows
after it (`lib/persistent-cache.js`), so warm reads survive isolate recycling
and a cold isolate can serve every verb without touching GitHub. Rows already
in KV are never re-written by the same isolate. Keys expire with the entry
TTL (24h default; `?ttl=`/`?ttlMs=` apply per request).

- Local dev: `npm run dev` passes `--kv=WIT_REPO_CACHE` (simulated local KV).
- Production: bind a KV namespace named `WIT_REPO_CACHE` to the Pages project
  (dashboard → Settings → Bindings), or fill in the commented
  `[[kv_namespaces]]` block in `wrangler.toml`.

Why KV rather than a Durable Object, and the storage layout, are documented
in `docs/adr/0006-url-api-kv-persistent-cache.md`.

## Layout

| file | role |
|------|------|
| `lib/routes.js` | route table and per-verb query parsing |
| `lib/handle.js` | request handler: auth resolution, snapshot open, verbs, JSON/text responses |
| `lib/github.js` | GitHub REST prefetch, rate-limit mapping, raw blob fetch |
| `lib/repo-cache.js` | sync host cache in front of the wasm `http_get` import |
| `lib/persistent-cache.js` | Workers KV hydrate/persist |
| `lib/textops.js`, `lib/stats.js`, `lib/outline.js`, `lib/format.js` | pure views: ranges, grep, stats, symbol outline, CLI plaintext |
| `lib/discovery.js` | `/api`, `/api/openapi.json`, `/api/llms.txt` |
| `lib/wasm-host.js` | wasm instantiation and open/list/read calls |
| `public/_worker.js` | Cloudflare Pages Advanced Mode entry (bindings → handler deps) |
| `public/lib/` | committed copy of `lib/` (`npm run sync-lib`) for Pages Git builds |

The Pages project is Git-connected at the **repository root**: Cloudflare's
v2 builder reads root `wrangler.toml` (`pages_build_output_dir =
"showcase/url-api/public"`) and the `public` → `showcase/url-api/public`
symlink covers the dashboard fallback. GitHub Actions deploys from
`showcase/url-api/` after building a fresh wasm.

## Develop

```bash
npm run build:wasm   # cargo build wit-snapshot for wasm32 and stage it
npm test             # fixture-backed tests, no network
npm run dev          # wrangler pages dev on :8787 with simulated KV
curl -sS "http://127.0.0.1:8787/api/stats/octocat/Hello-World"
```

`npm run check` (sync-lib + tests) is what CI runs on pull requests.

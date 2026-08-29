# wit URL API showcase

Client-side MemoryBackend URL surface for `tree` / `ls` / `cat`.

See `docs/adr/0005-url-api-host.md` and `docs/adr/0005-url-api-howto.md`.

Live deploy is Cloudflare Pages (project `wit-url-api`) via
`.github/workflows/url-api-deploy.yml` — not GitHub Pages.

The Pages project is Git-connected at the **repository root**. Cloudflare's
v2 builder reads root `wrangler.toml` (`pages_build_output_dir =
"showcase/url-api/public"`). A `public` → `showcase/url-api/public` symlink
covers the dashboard fallback that looks for output directory `public` when
no Wrangler file is found. GitHub Actions still deploys from
`showcase/url-api/` after building a fresh wasm.

```bash
npm run build:wasm
npm test
npm run dev
curl -sS http://127.0.0.1:8787/tree/octocat/Hello-World
```

Routes (path is a query param):

- `GET /tree/{owner}/{repo}?path=&branch=&depth=`
- `GET /ls/{owner}/{repo}?path=&branch=`
- `GET /cat/{owner}/{repo}?path=` (required)

A leading `/api` is an alias for the same three routes, so
`GET /api/tree/{owner}/{repo}` is identical to `GET /tree/{owner}/{repo}`.

Discovery lives under that prefix: `GET /api` is a plaintext list of the three
curls and `GET /api/openapi.json` is the OpenAPI 3 document for them.

## Persistent worker cache (Workers KV)

The worker's in-memory `RepoSnapshotCache` only lives for the isolate
lifetime. When the optional `WIT_REPO_CACHE` KV binding is present, the
worker hydrates that cache from KV before a request and persists new
entries after it (`lib/persistent-cache.js`), so warm reads survive isolate
recycling and a cold isolate can serve `tree`/`ls`/`cat` without touching
GitHub. Keys expire with the same TTL as the in-memory entries (24h default,
`?ttlMs=`/`?ttl=` still apply). Without the binding, behavior is unchanged.

- Local dev: `npm run dev` passes `--kv=WIT_REPO_CACHE` (simulated local KV).
- Production: bind a KV namespace named `WIT_REPO_CACHE` to the Pages project
  (dashboard → Settings → Bindings), or fill in the commented
  `[[kv_namespaces]]` block in `wrangler.toml`.

Why KV rather than a Durable Object, and the storage layout, are documented
in `docs/adr/0006-url-api-kv-persistent-cache.md`.

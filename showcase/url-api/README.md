# wit URL API showcase

Client-side MemoryBackend URL surface for `tree` / `ls` / `cat`.

See `docs/adr/0005-url-api-host.md` and `docs/adr/0005-url-api-howto.md`.

Live deploy is Cloudflare Pages (project `wit-url-api`) via
`.github/workflows/url-api-deploy.yml` — not GitHub Pages.

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

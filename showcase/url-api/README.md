# wit URL API showcase

Client-side MemoryBackend URL surface for `tree` / `ls` / `cat`.

See `docs/adr/0005-url-api-host.md` and `docs/adr/0005-url-api-howto.md`.

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

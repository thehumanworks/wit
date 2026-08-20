# ADR 0005 how-to: wit URL API showcase

Companion to [0005-url-api-host.md](./0005-url-api-host.md).

## What you get

| Surface | Role |
|---------|------|
| `showcase/url-api/public/` | Static HTML + JS + `wit_snapshot.wasm` |
| `showcase/url-api/functions/` | Cloudflare Pages Function (host `get_json` adapter) |
| `showcase/url-api/lib/` | Shared routing, auth scrubbing, plaintext format, wasm host |

Same `MemoryBackend` as ADR 0004. Exports used: `open` / `list` / `read` only.

## Honest constraint

A purely static file host **cannot** execute wasm for `curl`. Use the Pages
Function (or `wrangler pages dev`) for `text/plain`. The browser page runs the
same handler in-page via wasm.

## Routes (only these three)

```
GET /tree/{owner}/{repo}?path=&branch=&depth=
GET /ls/{owner}/{repo}?path=&branch=
GET /cat/{owner}/{repo}?path=          # path required
```

- `?ref=` aliases `branch`
- Path is a **query** param — never `/tree/{owner}/{repo}/{path}`
- Unknown query keys are ignored
- Memory-only host (no `--backend`, no disk, no `WIT_CACHE_DIR`)

## Auth

Preferred:

```bash
curl -H "Authorization: Bearer $GITHUB_TOKEN" \
  "http://127.0.0.1:8787/tree/octocat/Hello-World"
```

Also accepted: `Authorization: token <pat>`.

Fallback (leaks via access logs and `Referer` — avoid when you can):

```bash
curl "http://127.0.0.1:8787/tree/octocat/Hello-World?token=$GITHUB_TOKEN"
```

Public repos work without a token. Tokens must never appear in logs, traces, or
error bodies (the host scrubs them).

## Build wasm + run locally

```bash
# from repo root
cargo build -p wit-snapshot --target wasm32-unknown-unknown --no-default-features
cp target/wasm32-unknown-unknown/debug/wit_snapshot.wasm \
  showcase/url-api/public/wit_snapshot.wasm

cd showcase/url-api
npm run sync-lib
npm test
npx --yes wrangler@4 pages dev public --functions=functions --port=8787
```

### curl (public repo)

```bash
curl -sS "http://127.0.0.1:8787/tree/octocat/Hello-World"
curl -sS "http://127.0.0.1:8787/ls/octocat/Hello-World"
curl -sS "http://127.0.0.1:8787/cat/octocat/Hello-World?path=README"
```

### curl (header token)

```bash
curl -sS -H "Authorization: Bearer $GITHUB_TOKEN" \
  "http://127.0.0.1:8787/tree/octocat/Hello-World"
```

Plaintext matches `wit tree|ls|cat … --backend memory` stdout (CLI provenance
stays on stderr; HTTP returns stdout only).

### Browser

Open `http://127.0.0.1:8787/` — buttons call the same handler. Visiting an API
path through the Pages Function also returns `text/plain` directly.

## Host cache

Browser: 24h per `(owner/repo, resolved ref)` via `lib/repo-cache.js` (same
contract as ADR 0004 demo). Worker: same map for the isolate lifetime.

## Tests

```bash
cd showcase/url-api && npm test
```

Covers routing, flag/query mapping, header vs `?token=`, secret scrubbing, and
fixture-backed tree/ls/cat plaintext (no live GitHub required).

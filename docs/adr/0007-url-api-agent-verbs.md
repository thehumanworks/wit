# ADR 0007: Agent-grade URL API — reliability, budgeted reads, and SDKs

- Status: Accepted
- Date: 2026-09-04

## Context

`wit`'s north star is to be the first tool an AI agent reaches for when it
needs a GitHub-hosted codebase. The URL API (ADR 0005, hosted at
`wit.thehuman.sh/api`) is the surface most agents can use with zero setup —
one `GET`, no install, no MCP client — so it has to be reliable and it has to
let an agent control what it spends tokens on.

Observed on the live host before this change:

1. **Most requests failed with HTTP 403.** The Worker called GitHub
   anonymously; anonymous quota is 60 requests/hour *per egress IP*, and
   Cloudflare's egress IPs are shared, so the quota is permanently exhausted.
   The error blamed "access" on a public repository, which sends an agent
   down the wrong path.
2. **Three verbs only.** `tree`, `ls`, `cat` (whole file). No way to read a
   line range, find code, or judge a repository's size before reading it.
3. **KV write amplification.** Every warm request re-wrote every blob and
   tree row the isolate held (the per-request `KvRepoCache` only remembered
   what *it* hydrated), burning KV's daily write quota on no-ops.
4. `?ttl=` mutated the shared isolate cache's TTL, leaking one caller's
   setting into concurrent requests; a branch sharing its tree SHA with an
   already cached ref never got its own open entry (the wasm open then 404ed);
   and a branch open could rewrite the repo's recorded default branch.

## Decision

### Reliability

- **Host token fallback.** The Worker reads an optional `GITHUB_TOKEN`
  secret and uses it only when the caller sends no `Authorization` header.
  It must be a fine-grained token with *public repository read access only*;
  the response header `x-wit-auth: caller|host|anonymous` says which
  credential served the request. A caller token always wins.
- **Rate limits are 429, never 403.** Primary (`x-ratelimit-remaining: 0`)
  and secondary (`retry-after`) limits map to HTTP 429 with a `retry-after`
  header and a message that says whose quota is exhausted and how to fix it.
- **Blobs bypass the REST quota.** File bytes come from
  `raw.githubusercontent.com/{owner}/{repo}/{commit_sha}/{path}` first —
  pinned by commit, so as immutable as the blob endpoint — and fall back to
  `GET /repos/{r}/git/blobs/{sha}`. A cold open costs 3 REST calls; reads and
  `rg` cost none. `WIT_RAW_BLOBS=0` disables it.
- **No KV write amplification.** Persisted keys are remembered for the
  isolate lifetime (a `WeakMap` keyed by the sync cache), so a row is written
  once; failed batches clear the memory so a later request retries.
- **Per-request TTL, correct pending state, tree sharing.** `?ttl=` applies
  to the entries a request creates, not to the cache; the repo call always
  restarts the pending open state with the true default branch; a ref whose
  tree is already cached gets its own entry sharing that tree.

### Agent verbs

All take `/{verb}/{owner}/{repo}` with everything else in the query, answer
in CLI-identical plaintext or JSON (`?format=json` / `Accept:
application/json`), and carry provenance headers `x-wit-repo`, `x-wit-ref`,
`x-wit-commit`, `x-wit-cache: hit|miss`.

| verb | what it gives an agent | cost |
|------|------------------------|------|
| `stats` | files, bytes, ~tokens, per-directory and per-language breakdown, largest files, binary count — the "is this worth reading, and where" question | tree only, zero blob reads |
| `outline` | line-numbered symbol index for one file with approximate `end_line`, so the agent picks a `lines=A-B` range instead of the whole file | 1 blob |
| `cat?lines=A-B` | exact one-based inclusive range (`start=`/`end=` too); `n=1` numbers from the real line | 1 blob |
| `head` / `tail` | first/last N lines, `plus=N` from a line to EOF | 1 blob |
| `rg` | bounded ripgrep: `q`, `path`, `glob`, `i/S/w/v`, `l` (files), `c` (counts), `C/B/A` context, `max`, `max_files`; truncation is always reported | ≤ `max_files` blobs |
| `refs` | default branch, branches, tags for `?ref=` | 3 REST |
| `commits` | recent history, optionally for one path | 1 REST |
| `search` | GitHub repository search for "libraries that do X" (`q`, `p`, `lang`, `sort`) | 1 REST |

`tree?l=1` and `ls?l=1` now append `~N tok` (bytes/4) next to sizes, and the
CLI's memory backend prints the same, so budgeting works without blob reads.
`?ref=` accepts a branch, tag, or full commit SHA; `?fresh=1` re-resolves the
ref; `?ignore=GLOB` mirrors the CLI's `--ignore`.

Discovery: `GET /api` (curl list), `GET /api/openapi.json` (every verb), and
`GET /api/llms.txt` — a guide written for agents: which verb to call first,
how truncation is reported, how to pin a read.

### SDKs

`sdk/typescript` (`@nothumanwork/wit-sdk`) and `sdk/python` (`wit-api`) are
zero-dependency clients over the JSON surface. Both expose the same shape —
`client.repo("o/r", ref).stats() | tree() | ls() | cat(path, {lines}) |
head() | tail() | outline() | rg() | rgFiles() | rgCounts() | refs() |
commits()` and `client.search()` — plus two chained helpers that encode the
recommended workflow: `readSymbol(path, name)` (outline → cat range) and
`context(pattern)` (rg → one cat window per file). Errors carry the API's
`code`, `status`, and `retry-after`; retrying 429s is opt-in.

## Non-goals (for now)

- **AST-based search.** `outline` is a regex heuristic on purpose: it runs in
  the Worker with no parser dependencies and covers the languages agents meet
  most. A tree-sitter-backed `symbols`/`query` verb belongs in the Rust
  backend (CLI + MCP + wasm) and is tracked as follow-up work.
- **Cross-repository code search.** GitHub's code-search API only indexes
  default branches, needs auth, and is throttled to 10 req/min; `rg` on a
  pinned snapshot is the honest primitive.
- **Edge (CDN) response caching.** Responses now advertise
  `public, max-age=60, stale-while-revalidate=600` when no caller token was
  used, but the Cache API is not wired in; KV already absorbs GitHub traffic.

## Consequences

Positive:

- With a host token configured, the public host answers anonymous callers
  reliably (5,000 REST calls/hour shared, blobs unmetered).
- An agent can size a repository, locate code, and read exactly the lines it
  needs in three requests, with commit-pinned provenance on each.
- KV writes drop from O(requests × rows) to O(rows).
- Every verb, error path, cache path and both SDKs are covered by
  fixture-backed tests (`showcase/url-api/tests`, `sdk/*/tests`) that run in
  CI on pull requests.

Tradeoffs:

- `rg` on a large, cold repository is bounded by `max_files` (default 200,
  ceiling 1,000) and reports truncation rather than scanning everything;
  agents narrow with `path=`/`glob=`. The CLI and MCP remain the tools for
  exhaustive search.
- Content served under the host token is public-only by policy, not by
  enforcement: the token's scope is the guarantee.
- `outline`'s `end_line` is "until the next symbol at the same indent", not a
  parsed block end; documented as approximate.

## How-to

- Configure the host: `npx wrangler pages secret put GITHUB_TOKEN
  --project-name wit-url-api` (see `wrangler.toml`).
- Local: `cd showcase/url-api && npm run dev`, then
  `curl "http://127.0.0.1:8787/api/stats/octocat/Hello-World"`.
- Agent guide: `curl https://wit.thehuman.sh/api/llms.txt`.

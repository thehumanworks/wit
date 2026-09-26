# ADR 0009: Shared No-Login Cloud Pack Cache (`wit-cache`)

- Status: Accepted
- Date: 2026-09-26

## Context

Every cold `wit` read clones the repository from GitHub (`--bare --depth 1
--single-branch`, gix with `Tags::None`, git CLI fallback). Agent fleets clone
the same public repositories over and over, and large or flaky transfers
(#51) are slow. We want a shared cache that:

- needs no login and no client credentials,
- cannot serve content that differs from GitHub, and cannot admit private
  repositories,
- fills itself lazily on first use,
- is on by default in release builds but easy to disable,
- stays inside the Cloudflare Workers Paid ($5/month) included allowances and
  the R2 free tier on Tomas's account, with no extra bill.

## Decision

### What is cached

One **depth-1 git pack per public `owner/repo` + commit SHA**. These are the
same bytes a depth-1 clone receives, so the client rebuilds exactly today's
bare cache and every disk-backend verb (`tree/ls/cat/rg/sed/head/tail/ast`)
works unchanged. Packs are immutable per SHA, so nothing is ever invalidated.
Key: `v1/github/{owner}/{repo}/{commit}.pack` (owner and repo lowercased and
validated).

### Storage: R2 + one Durable Object + a Queue (not KV, not Containers)

| Store | Used for | Why |
|---|---|---|
| **R2** (`wit-cache-packs`) | Pack bytes | Objects up to 5 TB with streamed multipart writes, streamed reads, **no egress fees**, and a 30-day lifecycle rule for retention. |
| **Durable Object** (`FillCoordinator`, one global instance, SQLite storage) | Fill budget, single-flight, negative cache, pack ledger for eviction | Budgets and single-flight need strong consistency, and only misses and fill completions reach it. |
| **Queue** (`wit-cache-fills`) | Running fills | The consumer gets up to 15 minutes of wall time, independent of the client request. |
| KV | Not used | 25 MiB value cap (codex alone is 19 MB), eventual consistency (budgets and single-flight would race), $0.50/GB-month, and 1k writes/day on the free tier. |

**No Container.** The Worker fills directly over git smart-HTTP protocol v2:

1. `ls-refs` with `ref-prefix refs/heads/{branch}` confirms that the
   requested commit is the **current tip** of the branch the client named.
   The fill never stores an arbitrary SHA, so fork-network smuggling through
   a parent repo's URL is blocked.
2. `fetch` with `want {sha}`, `deepen 1`, `ofs-delta`, `no-progress`, and no
   `include-tag` or `thin-pack`. The Worker demultiplexes side-band 1 and
   streams it into R2: a single PUT under 16 MiB, otherwise a multipart
   upload with equal 16 MiB parts, holding one part in memory. On the way it
   checks the `PACK` header, a non-zero object count, the size cap, and the
   SHA-1 trailer.

A spike confirmed GitHub serves stateless v2 POSTs. The codex pack (18.8 MB)
arrives in about 3 s. A fill costs 9–14 ms of Worker CPU per MB (vscode's
61 MB pack: 558 ms CPU and 10.8 s wall), after switching to workerd
`readAtLeast` reads in chunks of 64 KiB or more (default reads cost about
37 ms/MB). A Container would add
cost (vCPU and memory minutes), cold starts, and an image to maintain, with
no reliability gain.

### Request flow

```
wit (cold local cache or new SHA)
  sha := git ls-remote github                         (unchanged)
  GET {WIT_CACHE_URL}/v1/github/o/r/{sha}.pack?branch=B
    200 -> git init --bare; write shallow=<sha>; index-pack --strict --stdin
           update-ref refs/heads/B <sha>; HEAD; rev-list --objects --missing=error
           gix open + HEAD == sha            -> fill_source "cloud"
    anything else (404/429/5xx/timeout/oversize/bad pack/wrong commit)
        -> existing gix clone, then git CLI    -> fill_source "github"

Worker on a miss: answer 404 right away; afterwards (ctx.waitUntil)
  per-IP fill limiter -> coordinator.requestFill -> queue.send
Queue consumer: fillPack -> coordinator.complete (ledger, eviction, negatives)
```

The miss path costs one R2 lookup (about 180 ms end to end), so a cold run is
no slower than a plain GitHub clone, within network noise.

### Client (`crates/wit/src/gitops/cloud.rs`)

- Hooked in front of both clone chokepoints (`recache_repo` and
  `refresh_repo_with_context`) after `ls-remote` has resolved the SHA. Locks,
  metadata, and stale-while-revalidate are unchanged. `metadata.json`
  records `fill_source`.
- It is a fill source inside the disk backend, not a new `SnapshotBackend`.
  `--refresh-cache` still uses it, because refresh means re-resolving the ref.
- Config: `WIT_CACHE_URL` (release builds default to
  `https://wit-cache.rodat-human-ada.workers.dev`; debug builds default to
  off; empty, `off`, `0`, `false`, `no`, `none`, or `disabled` turn it off;
  `https://` is required except on loopback, and embedded credentials are
  rejected). Also `WIT_CACHE_TIMEOUT_MS` (default 60000, 3 s connect) and
  `WIT_CACHE_MAX_BYTES` (default 512 MiB). Packagers can change the baked-in
  default with `WIT_DEFAULT_CACHE_URL` at build time. Operation contexts (MCP)
  cap the timeout at their remaining deadline.
- The blocking HTTP client runs on its own thread, with no auth and no
  redirects.

### Security properties

- **Integrity is client-side.** The SHA comes from GitHub (`ls-remote`). The
  pack must pass `index-pack --strict` with the client-written `shallow` file
  (object hashes, fsck, connectivity to the shallow boundary), then
  `update-ref` to that SHA, `rev-list --objects --missing=error`, and a HEAD
  check. The client writes its own config, refs, HEAD, and `shallow`; only
  pack bytes come from the server. A compromised cache can deny service but
  cannot change content.
- **Anonymous fills only.** The Worker holds no GitHub credential. GitHub
  answers an anonymous upload-pack for a private or missing repo with 401,
  which becomes a `not_found_or_private` negative entry.
- **Read-only clients.** There is no write route for callers. The caller's
  Authorization header is never read or forwarded (tests assert that no
  upstream request carries one), and the CLI never sends one.
- **Operator takedown:** `DELETE /v1/github/{owner}/{repo}` with
  `X-Wit-Admin-Key` (a `wrangler secret`, not a user login) deletes every
  pack for that repo and blocks refills for 30 days. The route is disabled
  (404) while the secret is unset.
- Error bodies and logs go through `scrubSecrets` / `safeConsole`, copied
  verbatim from `showcase/url-api/lib/auth.js` (a test enforces the copy), and
  are phrased without credential prefixes. Workers observability (persisted
  logs) is off, and client IPs are used only as rate-limit keys.

## Limits and cost reasoning

Account baseline when this shipped (2026-09): R2 held 5.5 GB across 35 other
buckets (the free tier is **account-wide**: 10 GB-month storage, 1M Class A,
10M Class B), about 60k Class A and 90k Class B ops this month, and other
Workers used about 400k requests and 5.5M CPU-ms per month (Workers Paid
includes 10M requests and 30M CPU-ms).

| Limit | Value | Bounds |
|---|---|---|
| Retention | 30 days (R2 lifecycle `expire-30d` on `v1/`; ledger rows pruned at 31 days) | Tomas's decision |
| Storage cap | **3 GB**, oldest filled pack evicted first | Keeps the account at or below about 8.5 GB of the 10 GB free tier, with about 1.5 GB headroom for his other buckets |
| Max pack | **512 MiB** (Worker and client) | Covers torvalds/linux at depth 1; one pack is at most 1/6 of the store. Over-cap repos get `too_large` for 7 days and clone from GitHub. |
| Fills per day | **300** (global, strongly consistent in the coordinator) | Class A and Queue ops |
| Fill bytes per day | **8 GB** upstream | Worker CPU: 8,000 MB × about 14 ms/MB ≈ 112k CPU-ms/day ≈ 3.4M/month, which leaves plenty of the 30M included |
| Fills in flight | 4 (queue `max_concurrency` 4, coordinator cap 4) | Memory (one 16 MiB part per fill) and transient multipart storage |
| Fill timeout | 5 min (queue consumer wall limit is 15 min); `cpu_ms = 60000` | Large packs |
| Per-IP reads | 60/min (Workers rate-limit binding) | Class B ops and Worker requests |
| Per-IP fill requests | 5/min | Coordinator (DO) requests |
| Negative cache | private/missing 1 h (repo); too large 7 d (repo); GitHub rate limit = `Retry-After` (≤ 1 h, global); not a tip, upstream error, or bad pack 10 min (commit); timeout 1 h (commit); takedown 30 d (repo) | Repeat fills |

Worst-case monthly usage at these caps:

- **R2 storage** ≤ 3 GB for wit, so the account stays ≤ 8.5 GB of 10 GB.
  Incomplete multipart parts are aborted on failure, or by the 1-day
  `abort-multipart-1d` rule.
- **R2 Class A** ≤ 300 × (create + complete) + 8 GB / 16 MiB parts ≈ 1.1k/day
  ≈ 35k/month, of the 1M free (other buckets use about 60k).
- **R2 Class B:** one GET per warm pull plus one HEAD per fill, well under the
  10M free at realistic use. This is the only dimension without a global hard
  cap (a global counter would put the DO on the hit path). Per-IP limits bound
  it. If it ever gets close, lower `READ_LIMITER` or serve from a custom
  domain so the edge cache absorbs repeats (the Cache API does nothing on
  `workers.dev`).
- **Workers requests:** one per CLI cold fill or warm pull, plus one queue
  invocation per fill, well within 10M.
- **Workers CPU:** fills ≤ about 3.4M CPU-ms at the byte cap; hits stream R2
  bodies without JS touching the bytes (a few ms each).
- **Durable Object:** only misses (bounded per IP) and at most 300
  completions/day, far below 1M requests and 400k GB-s. SQLite rows are tiny.
- **Queues:** 3 ops per fill ≤ 27k/month of the 1M included.
- The rate-limit bindings are free, and there are no egress fees on R2 or
  Workers.

Abuse can at most exhaust the daily budgets (clients then clone from GitHub)
or churn the 3 GB store. It cannot push storage past the cap.

## Measurements (2026-09-26, cloud VM in the US, release build)

`wit tree -r openai/codex` with a fresh `WIT_CACHE_DIR` each run (the pack is
18.8 MB):

| Scenario | Runs | Wall time |
|---|---|---|
| GitHub clone (`WIT_CACHE_URL=off`) | 5 | 3.69–3.81 s (median 3.73 s) |
| Cloud warm (pack cached) | 5 | 0.95–1.77 s (median 1.04 s) |
| Cloud cold, codex branches not yet cached | 6 | 4.39–4.76 s, versus 4.07–4.75 s for a GitHub clone of the same branch; warm afterwards 0.63–0.86 s |

`microsoft/vscode` (61 MB pack): GitHub 5.26–5.38 s, cloud warm 2.09–2.62 s.
A fill is ready 3–15 s after the first miss (queue latency plus about 3–8 s
of fill). Tree output is byte-identical to the GitHub-filled cache.

## Consequences

- Warm reads of popular repositories are about 3–5× faster and put no clone
  load on GitHub. Cold reads cost the same as before.
- `wit` still needs `git ls-remote` to GitHub for every resolve. There is no
  trust-refs mode.
- Release builds contact the hosted instance by default and reveal which
  public repositories are read (no IPs are stored). `WIT_CACHE_URL=off`
  opts out.
- Shared helpers (`auth.js`) are copies, like `public/lib`. The url-api keeps
  its own KV snapshot store (ADR 0006).

## Operations

- Source: `services/wit-cache/`. Tests: `npm test` there (no network).
  Guard: `scripts/check_cache_worker.sh`.
- Deploy: `.github/workflows/cache-worker-deploy.yml` on pushes to `main` that
  touch `services/wit-cache/**`. It needs the repository secrets
  `CLOUDFLARE_API_TOKEN` (Workers Scripts, Workers R2 Storage, and Queues
  edit on the account) and `CLOUDFLARE_ACCOUNT_ID`, and it creates the bucket,
  lifecycle rules, and queue when they are missing.
- Monitoring: `GET /v1/stats` (stored bytes and packs, fills today, fill bytes
  today, limits).
- Takedown: `npx wrangler@4 secret put ADMIN_KEY`, then
  `curl -X DELETE -H "X-Wit-Admin-Key: …" https://wit-cache.rodat-human-ada.workers.dev/v1/github/{owner}/{repo}`.

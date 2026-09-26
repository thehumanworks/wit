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
  per-IP fill limiter -> coordinator.requestFill (reserve bytes) -> queue.send
Queue consumer: fillPack -> coordinator.complete (settle, ledger, eviction, negatives)
  redelivery: HEAD the pack -> coordinator.complete (never refetches)
```

`requestFill` reserves the most bytes one fill can read (`MAX_PACK_BYTES` plus
one 64 KiB side-band chunk) against the UTC day of the request, and queues
only when the whole reservation fits under `DAILY_FILL_BYTES`. The
reservation's id travels in the queue message. `complete` settles it to the
bytes the fill actually read (capped at the reservation) and refunds the
rest; a failed enqueue releases it in full. Reservations cannot leak or
stall the budget:

- Only the delivery holding the id settles, and a settled row is deleted, so
  a reservation is refunded at most once and a late `complete` never touches
  a newer fill of the same pack.
- A redelivered message (`attempts > 1`, after its `complete` failed) never
  refetches from GitHub: it HEADs the pack and records what the first
  delivery stored, or fails as `upstream_error` and keeps the whole
  reservation. One reservation therefore covers every fetch of a job.
- A pending row that expires unsettled (20 min, e.g. every `complete`
  failed) frees its in-flight slot but stays charged, because nobody knows
  what that fill read. The charge lapses with its UTC day, so the budget is
  at worst held until the next day and never stalls longer.

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
  pack for that repo and blocks refills for 30 days. A fill already in flight
  deletes its pack when it completes, and a later negative never shortens
  the block. The route is disabled (404) while the secret is unset.
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
| Max pack | **512 MiB** (Worker and client) | Covers torvalds/linux at depth 1; one pack is under a fifth of the store (five fit in 3 GB, six do not). Over-cap repos get `too_large` for 7 days and clone from GitHub. |
| Fills per day | **300** (global, strongly consistent in the coordinator) | Class A and Queue ops |
| Fill bytes per day | **8 GB** upstream, with a 512 MiB + 64 KiB reservation per queued fill settled to the bytes read | Worker CPU: at most 32 days' budget is fetched in any 31 days, 256,000 MB × 14 ms/MB = 3.58M CPU-ms/month, even when every `complete` fails |
| Fill retries | queue `max_retries` **1**; the consumer retries `complete` in-process (3 tries, 0.5 s and 2 s apart) before handing the message back | R2 Class A, even when bookkeeping fails |
| Fills in flight | 4 (queue `max_concurrency` 4, coordinator cap 4) | Memory (one 16 MiB part per fill) and transient multipart storage |
| Fill timeout | 5 min (queue consumer wall limit is 15 min); `cpu_ms = 60000` | Large packs |
| Per-IP reads | 60/min (Workers rate-limit binding) | Class B ops and Worker requests |
| Per-IP fill requests | 5/min | Coordinator (DO) requests |
| Negative cache | private/missing 1 h (repo); too large 7 d (repo); GitHub rate limit = `Retry-After` (≤ 1 h, global); not a tip, upstream error, or bad pack 10 min (commit); timeout 1 h (commit); takedown 30 d (repo) | Repeat fills |

Worst-case monthly usage at these caps:

- **R2 storage:** the ledger holds ≤ 3 GB for wit, so the account stays at
  about 8.9 GB of 10 GB (reading the 5.5 GB baseline as GiB). R2 briefly holds
  up to four more packs between a fill's upload and its `complete` (a 5.15 GB
  peak), each for less than one consumer invocation, so the monthly average
  stays at or below about 9.0 GB. Incomplete multipart parts are aborted on
  failure, or by the 1-day `abort-multipart-1d` rule.
- **R2 Class A** ≤ 9,304 fill messages a month (31 × 300, plus four in
  flight across the month boundary) × 2 runs each (the first delivery and
  one queue retry) × 35 ops per run (a 512 MiB upload in 16 MiB parts is
  create + 32 parts + complete; the bound allows a partial last part) + the
  60k baseline = **711,280** of the 1M free. This counts every retry and does not rely on the byte budget,
  because a run whose `complete` failed was never counted against it.
  Realistic use is far lower: 300 × (create + complete) + 8 GB / 16 MiB
  parts ≈ 1.1k/day ≈ 35k/month. A queue retry re-runs the whole fill, so the
  consumer retries `complete` itself before giving up the delivery, and a
  redelivery whose pack is already stored only HEADs it (Class B). With the
  previous `max_retries = 2` the worst case was 1,036,920. One retry is kept
  rather than none so that a coordinator blip longer than a few seconds
  still gets the stored pack into the ledger.
- **R2 Class B:** one GET per warm pull plus one HEAD per fill, well under the
  10M free at realistic use. This is the only dimension without a global hard
  cap (a global counter would put the DO on the hit path). Per-IP limits bound
  it. If it ever gets close, lower `READ_LIMITER` or serve from a custom
  domain so the edge cache absorbs repeats (the Cache API does nothing on
  `workers.dev`).
- **Workers requests:** one per CLI cold fill or warm pull, plus one queue
  invocation per fill, well within 10M.
- **Workers CPU:** fills ≤ **3,584,000** CPU-ms a month (32 × 8 GB fetched
  at 14 ms/MB: a fetch is always covered by a reservation charged to its own
  or the previous UTC day), so with the 5.5M baseline and 20M kept for
  requests the account stays at 29.08M of the 30M included. This holds when
  every `complete` fails, because an unsettled reservation stays charged.
  Hits stream R2 bodies without JS touching the bytes (a few ms each).
- **Durable Object:** only misses (bounded per IP) and at most 300
  completions/day, far below 1M requests and 400k GB-s. SQLite rows are tiny.
- **Queues:** 3 ops per fill ≤ 27k/month of the 1M included.
- The rate-limit bindings are free, and there are no egress fees on R2 or
  Workers.

Abuse can at most exhaust the daily budgets (clients then clone from GitHub)
or churn the 3 GB store. It cannot push storage past the cap.

## Formal proofs

The claims above are proved in Lean 4 under [`formal/`](../../formal), with
the Worker, wrangler, deploy-workflow, and client constants extracted from the
source by `scripts/check_formal.sh` (ADR 0010 covers the approach, the
assumptions, and the limits).

| Claim | Theorem |
|---|---|
| Single-flight per `repo@commit` | [`Coordinator.single_flight`](../../formal/Wit/Coordinator.lean) |
| Fills in flight ≤ 4 | [`Coordinator.inflight_le`](../../formal/Wit/Coordinator.lean) |
| Fills per day ≤ 300; bytes charged per day, reservations included, ≤ 8 GB | [`Coordinator.daily_fills_le`, `daily_bytes_le`](../../formal/Wit/Coordinator.lean), [`Budget.deployed_daily_bytes`](../../formal/Wit/Budget.lean) |
| Settled bytes never exceed the day's charge; bytes fetched in any _n_ days ≤ (_n_ + 1) × 8 GB, whether or not `complete` succeeds | [`Coordinator.spent_le_bytes`, `daily_spent_le`, `fetched_window_le`](../../formal/Wit/Coordinator.lean) |
| The ledger stays ≤ the 3 GB cap; R2 exceeds it only by packs mid-write | [`Coordinator.ledger_le_cap`, `r2_le_cap_plus_inflight`, `r2_le_cap_when_quiescent`](../../formal/Wit/Coordinator.lean) |
| Eviction removes the oldest pack other than the one just stored | [`Coordinator.evict_removes_oldest`](../../formal/Wit/Coordinator.lean) |
| A takedown holds for 30 days, including against fills in flight | [`Coordinator.takedown_holds`, `takedown_no_fill`](../../formal/Wit/Coordinator.lean) |
| Storage, Class A/B, requests, CPU, queue ops, memory, and multipart rules fit the free tiers at the deployed and default limits | [`Budget.deployed_fits`, `defaults_fits`](../../formal/Wit/Budget.lean) |
| Worst-case monthly Class A, queue retries included, is ≤ 1M (711,280) at the deployed and default limits | [`Budget.classA_worst_case_le_free_tier`, `deployed_classA_worst_case`, `defaults_classA_worst_case`](../../formal/Wit/Budget.lean) |
| Worst-case monthly Worker CPU, every `complete` failing included, is ≤ 30M (fills 3,584,000) at the deployed and default limits; the model's reservation is `fillReserveBytes` | [`Budget.cpu_worst_case_le_included`, `deployed_cpu_le_included`, `defaults_cpu_le_included`, `deployed_cpu_worst_case`, `defaults_cpu_worst_case`, `reserve_matches_deployed`, `reserve_matches_defaults`](../../formal/Wit/Budget.lean) |
| One pack is under a fifth of the store | [`Budget.Fits.fivePacksFit`, `six_packs_exceed_cap`](../../formal/Wit/Budget.lean) |
| A cache can deny service but cannot change content; every failure falls back to GitHub | [`Integrity.fill_matches_github`](../../formal/Wit/Integrity.lean) |
| The client sends no credentials; the Worker never reads or forwards `Authorization` | [`ReadOnly.client_sends_no_credentials`, `worker_ignores_authorization`, `worker_upstream_anonymous`, `only_takedown_mutates`](../../formal/Wit/ReadOnly.lean) |
| The verifier sees every byte and rejects packs over the cap; the stored object is the stream, in equal 16 MiB parts | [`Streaming.verifier_state`, `verifier_accepts_le_cap`, `upload_object_eq_stream`](../../formal/Wit/Streaming.lean) |
| Keys are injective, a takedown's prefix covers exactly that repo, and the lifecycle rule covers every pack | [`Keys.packKey_injective`, `repoPrefix_covers_iff`, `lifecycle_covers_packs`](../../formal/Wit/Keys.lean) |

Modeling the coordinator found five bugs, now fixed:

- A fill request that raced a stored pack dropped the pack's ledger row, so
  the cap stopped bounding R2.
- A fill that completed after a takedown restored the pack.
- A fill that completed while a takedown was listing R2 kept its pack.
- A later negative with a shorter TTL shortened a takedown's block.
- Bytes were counted only when `complete` succeeded, so fills whose
  bookkeeping failed fetched outside the byte budget (about 140M CPU-ms a
  month in the worst case, against 30M included). Bytes are now reserved
  when the fill is queued.

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

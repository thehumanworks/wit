# ADR 0010: Formal Proofs of ADR Invariants (`formal/`)

- Status: Accepted
- Date: 2026-09-26

## Context

ADRs explain why a design was chosen. Tests check examples. Neither stops a
later change from quietly breaking a property an ADR relies on. Examples: a
wrangler var raised past the R2 free tier, a coordinator edit that lets the
ledger undercount R2, or a new header read that makes the Worker depend on a
caller's `Authorization`. ADR 0009 in particular rests on invariants
(single-flight, budgets, the storage cap, takedowns, content integrity) that
must hold for every interleaving of requests and fill outcomes. No test suite
covers every interleaving.

## Decision

Keep machine-checked proofs of the invariants that can be stated precisely,
next to the code, and fail CI when the code or config drifts from what was
proved.

- **Lean 4, core only.** `formal/` is a Lake project pinned by
  `formal/lean-toolchain` (`leanprover/lean4:v4.34.1`). It uses no Mathlib:
  the proofs need lists, naturals, `omega`, and `decide`. It builds in
  seconds, with no cache download.
- **Models, tied to the source by extraction.** Each module models one
  decision abstractly. Every number or list the proofs depend on comes from
  `formal/Wit/Generated/Constants.lean`. `scripts/gen_formal_constants.mjs`
  writes that file from:
  - `services/wit-cache/src/config.js` (imported, so the values are the
    ones the Worker runs with);
  - `coordinator.js`, `keys.js`, `pktline.js`, `upload-pack.js`, and
    `index.js`;
  - `wrangler.toml` and the deploy workflow's lifecycle rules;
  - `crates/wit/src/gitops/cloud.rs` and `ops.rs`.

  The generator also fails when a code shape the model depends on changes.
  For example, it requires the negative upsert to keep the later expiry,
  requires `complete` to check `isBlocked`, and requires `takedown` to
  record the block before its first `await`.
- **Hand-written assumptions in one file.** Platform limits, the account
  baseline, and measured costs live in `formal/Wit/Assumptions.lean`. They are
  the only numbers the check cannot verify.
- **The guard is `scripts/check_formal.sh`.** CI runs it in the `formal` job
  of `.github/workflows/ci.yml`. The script:
  1. regenerates the constants and fails with a diff when the committed file
     is stale (`--write` regenerates it instead);
  2. rejects `sorry`, `admit`, `axiom`, `native_decide`, `implemented_by`,
     `extern`, `unsafe`, `opaque`, and `skipKernelTC` outside comments;
  3. runs `lake build`, installing elan and the pinned toolchain if needed;
  4. runs `#print axioms` on every theorem and fails unless each one depends
     only on `propext`, `Classical.choice`, and `Quot.sound`.

  Changing a limit therefore means rerunning with `--write`. If the new
  value breaks a budget, the build fails at the field of `Budget.Fits` that
  no longer holds.

## What is proved

| ADR | Module | Theorems |
|---|---|---|
| 0009 coordinator | [`Coordinator.lean`](../../formal/Wit/Coordinator.lean) | The FillCoordinator, R2, the queue consumer's writes, the lifecycle rule, and the clock are modeled as one state machine, and each property holds in every reachable state: `single_flight`, `inflight_le`, `daily_fills_le`, `daily_bytes_le`, `ledger_le_cap`, `r2_le_ledger_plus_writing`, `r2_le_cap_plus_inflight`, `r2_le_cap_when_quiescent`, `evict_removes_oldest`, `queued_only_within_limits`, `takedown_holds`, `takedown_no_fill` |
| 0009 limits and cost | [`Budget.lean`](../../formal/Wit/Budget.lean) | `deployed_fits` and `defaults_fits` check every field of `Fits`: storage headroom and monthly average, Class A (including retry headroom), Class B, requests, CPU, per-fill CPU, queue ops, part memory, multipart rules, timeouts inside the consumer wall limit and the pending TTL, lifecycle = retention, and the in-flight cap = queue concurrency. Also `params_wf` (the coordinator model's side conditions hold for the real values), `six_packs_exceed_cap`, and `deployed_ledger_le` / `deployed_r2_peak` / `deployed_daily_bytes` / `deployed_daily_fills` |
| 0009 integrity and fallback | [`Integrity.lean`](../../formal/Wit/Integrity.lean) | `valid_complete_agree`, `fill_matches_github`: for every answer the server can give, the filled cache has the client's commit, branch, and remote, and exactly GitHub's objects on everything reachable from the commit |
| 0009 read-only | [`ReadOnly.lean`](../../formal/Wit/ReadOnly.lean) | `client_sends_no_credentials`, `worker_ignores_authorization`, `worker_upstream_anonymous`, `only_takedown_mutates`, `no_persisted_logs` |
| 0009 streaming | [`Streaming.lean`](../../formal/Wit/Streaming.lean) | `verifier_state` (the hash covers every byte but the 20-byte trailer, for any chunking), `verifier_accepts_le_cap`, `upload_object_eq_stream` (the stored object is the stream; parts are exactly `PART_BYTES`; a single PUT happens exactly when the pack is under one part) |
| 0009 keys | [`Keys.lean`](../../formal/Wit/Keys.lean) | `packKey_injective`, `repoPrefix_covers_iff` (a takedown's prefix listing matches exactly that repo's packs), `lifecycle_covers_packs` |
| 0002 | [`Swr.lean`](../../formal/Wit/Swr.lean), [`Keys.lean`](../../formal/Wit/Keys.lean) | `revalidate_sound`, `revalidate_keeps_on_failure`, `revalidate_recaches_only_on_change`, `revalidate_follows_remote`; `encodeBranch_injective`, `encodeBranch_injective_folded`, `encodeBranch_path_safe` |

## Assumptions

In `Wit/Assumptions.lean`:

- R2 free tier: 10 GB-month, 1M Class A, 10M Class B.
- Workers Paid: 10M requests, 30M CPU-ms, 1M queue ops.
- Worker limits: 15 min consumer wall time, 128 MB isolate memory.
- R2 multipart rules.
- R2 applies expiry within one day.
- Queue delivery within 5 minutes.
- The ADR 0009 account baseline, with 5.5 GB read as GiB.
- 14 ms of CPU per MB filled.
- A 31-day month.

In the models:

- **SHA-1 collision resistance** is a hypothesis (`Injective hash`) of the
  integrity theorems, not an axiom.
- **Each coordinator method is one atomic step.** The Durable Object is
  single-threaded, but it can run another request at an `await`.
  - `takedown` is atomic as far as the invariants go, because it records
    its block before its first `await` (the generator enforces this).
  - `complete` awaits R2 deletes inside `evict`, so while two completions
    interleave the ledger can briefly exceed the cap by the packs they are
    recording.
- **Timing.** Stores and `complete` happen while the fill's pending row is
  live. `Fits.runInsidePending` checks the arithmetic behind this: delivery
  plus the consumer wall time is at most `PENDING_TTL_SECONDS`. A queue
  retry after the row expired falls outside the model. The queue's
  `max_concurrency` still bounds such writes.
- **Operation counts per fill** follow `fill.js`/`store.js`: one HEAD, then
  either one PUT, or a create, the parts, and a complete. Abort and delete
  are free.
- **Refreshes deliver the requested SHA.** `Swr.lean` assumes a refresh
  yields the content of the SHA it was asked for. `Integrity.lean` proves
  this for the cloud path. The GitHub clone path clones the branch tip at
  clone time, which can already be a newer commit. The next revalidation
  then sees a different SHA and refreshes again.

## Limits

- The proofs are about models, not about the JavaScript or Rust itself.
  Extraction ties the models' constants and a few code shapes to the source.
  A logic change elsewhere needs a matching model change, and the guard
  cannot notice when one is missing.
- The extraction is syntactic. It finds headers read with
  `request.headers.get("…")` and requests made with `client.get(…)`. It
  would not see a header read through another API, although it does treat
  any use of the auth helpers as reading `Authorization`.
- **Budgets are bounds, not forecasts.** Hits (Class B, Worker requests, DO
  misses) have no global cap, so the proofs only show headroom: 9.5M client
  requests a month fit in requests and Class B, and 20M CPU-ms remain for
  them. Per-IP rate limits are Cloudflare-approximate and are not modeled.
- **Class A with queue retries.** A retried fill whose `complete` failed is
  not counted against the byte budget. `Fits.classA` absorbs 25,000 such runs
  a month. In the pathological worst case, every fill of a month is a
  512 MiB pack whose bookkeeping fails on all three deliveries, and Class A
  would reach about 1.04M. That needs the coordinator to fail
  `complete` while still accepting `requestFill`.
- **Storage is monthly.** The free tier is billed per GB-month. R2 can peak
  at the cap plus four packs mid-write (5.15 GB). The proof bounds the
  monthly average (`Fits.storageMonthlyAverage`) at about 9.0 GB.

## ADRs not formalized

| ADR | Why |
|---|---|
| 0001 global ignore patterns | Ignore monotonicity is immediate, and glob semantics would mean reimplementing `globset` in Lean. |
| 0002 locks | The locks are OS-level concurrency (lock files and a mutex). The subprocess tests in `tests/cache_lock_integration.rs` cover them. A model of the file-lock primitive would only restate its specification. |
| 0003 QuickJS child process | The spike was a NO-GO and never shipped. |
| 0004 wasm fetch snapshot client | Architectural (no-FS, no reqwest on wasm32). `check_wit_snapshot_wasm.sh` already enforces it. |
| 0005 URL API host | Routing and secret scrubbing are regex behavior, covered by the host tests and the deploy guard. |
| 0006 KV persistent cache | The only checkable rule is the KV TTL floor, which is a restatement of one clamp. |
| 0007 URL API agent verbs | Parameter clamps and status mappings; any proof would restate the code. |
| 0008 AST search | Parser behavior of tree-sitter grammars, outside what a model can say. `every_builtin_symbol_query_compiles` guards the queries. |
| 0009 per-IP limiters and the `ls-refs` tip check | The limiters are approximate by design. The tip check is one comparison, and a proof would restate it. |

## Consequences

- A change that breaks a proved property fails CI with the theorem or the
  `Fits` field it breaks. Examples: raising `DAILY_FILL_LIMIT`, shortening
  the lifecycle rule, reading `Authorization`, or reordering `takedown`.
- Modeling found four coordinator bugs before release (listed in ADR 0009).
- Platform prices and account usage change without a code change.
  `Assumptions.lean` must be updated by hand when they do.
- Contributors who change modeled code need elan. `check_formal.sh`
  installs it.

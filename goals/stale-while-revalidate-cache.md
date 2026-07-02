---
goal_id: "stale-while-revalidate-cache"
title: "Add stale-while-revalidate cache refresh"
status: "done"              # active | blocked | exited | done
confidence_floor: 90        # a Task below this CANNOT be ticked done
created: "2026-07-01"
updated: "2026-07-01"
---

# Goal: Cached branch reads serve existing content while SHA-based revalidation refreshes stale entries in the background.

## 1. Invariants · the rules that must not break

This file is the only state for this delivery subgoal — if it isn't written here,
it didn't happen. The full procedure (boot loop, confidence rubric, logging cadence) lives in the
**goal-driven-development** skill; these rules hold even if that skill isn't loaded:

- **Scope is frozen after user confirms DoD + Tasks.** Until then, §3 and §5 may be
  edited freely. After confirm, the only permitted edits are: tick checkboxes (Task
  **and** DoD), update Confidence, append Evidence, append to the live sections
  (§6/§7/§8), and update frontmatter `status`/`updated` — never add, remove, reword,
  split, or merge a DoD item or Task, and never rewrite or delete a live-section entry.
- **Never tick below the floor.** A task is ticked done only at Confidence ≥
  `confidence_floor`. If you cannot reach it, leave it unticked and fire `CONFIDENCE-STALL`.
- **Scope change is an exit, not a decision.** If scope must change, record the
  proposal in §6 and fire `SCOPE-CHANGE` — stop and surface it to the user.
- **Live sections are append-only.** Log each decision (§6) and learning (§7) at
  the moment it happens — before ticking the task it came from. Never delete entries.

---

## 2. References

Everything the agent needs before/while working. Each entry is `path-or-url — why it matters`.

- User feedback, 2026-07-01 — do not expose a max-life TTL; use stale-while-revalidate with optional force invalidation.
- User feedback, 2026-07-01 — know the last SHA for a given repo/branch and invalidate when stale in the background.
- `goals/branch-cache-metadata.md` — prerequisite branch cache identity and per-branch metadata contract.
- `crates/wit/src/gitops/ops.rs` — current cache hit path returns an existing valid bare repo indefinitely.
- `crates/wit/src/cli.rs` — repo-scoped commands need a cache acquisition mode that can either serve stale or force refresh before reading.
- `crates/wit/tests/cache_lock_integration.rs` — existing lock proof should evolve toward per repo+branch revalidation locks.

---

## 3. Definition of Done · INVARIANT

Each item is **atomic** (one verifiable assertion per checkbox), tagged with a
stable id that Tasks reference via **Closes:**, and carries a concrete `verify by:`.

Tick a `DoD-N` box only when its own `verify by:` has been run and passed (not merely
because a closing Task is ticked). Log the command and its outcome as an Evidence bullet
under the Task that **Closes:** it. DONE requires every DoD box ticked.

- [x] **DoD-1** — a repo/branch cache hit returns the current local cache without blocking on remote SHA comparison when not force-invalidated — *verify by:* `cargo test -p wit cache_swr_serves_cached_first --lib`
- [x] **DoD-2** — background revalidation compares the remote branch SHA to metadata `current_sha` and refreshes only when they differ — *verify by:* `cargo test -p wit cache_swr_refreshes_on_sha_change --lib`
- [x] **DoD-3** — force invalidation bypasses stale-while-revalidate and blocks until the repo/branch cache is refreshed before returning content — *verify by:* `cargo test -p wit cache_force_invalidation --lib`
- [x] **DoD-4** — cache locks are scoped to repo+branch so unrelated repos or branches do not serialize behind one global lock, while same repo+branch mutation remains serialized — *verify by:* `cargo test -p wit cache_branch_locks --lib`

---

## 4. Exit Conditions

The goal terminates when **any** condition holds. On exit, state which fired —
explicitly — in the response to the user. Specialize the bracketed values for this goal.

- **`DONE`** — all §3 items ticked and all §5 tasks ≥ confidence floor. *(primary)*
- **`BLOCKED-DEP`** — `goals/branch-cache-metadata.md` is not done, or detached process spawning is unavailable in the target OS after one direct retry. Exit without the blocked step; name it explicitly.
- **`SCOPE-CHANGE`** — work cannot complete without changing scope. Record the
  proposal in §6 and exit to the user.
- **`CONFIDENCE-STALL`** — a task cannot reach the floor after 3 honest attempts. Exit, report the task and the gap.
- **`BUDGET`** — 2 implementation days reached before all DoD items pass. Exit and report progress.

---

## 5. Tasks · INVARIANT

Ordered, dependency-aware units of work that together satisfy the DoD. Tick the
trailing `[ ]` only when the Verification Contract passes and Confidence ≥ floor.

---

### T1 · Define cache acquisition modes · [x]

**Steps**
- [ ] Replace the boolean `refresh` parameter with a named mode such as `ServeStaleAndRevalidate` versus `ForceInvalidate`.
- [ ] Keep first-use behavior blocking when no valid cache exists.
- [ ] Ensure regular repo-scoped commands use stale-while-revalidate mode and explicit force refresh paths use force invalidation.

**Verification Contract**
- *Check:* call sites cannot accidentally pass an ambiguous boolean for cache freshness behavior.
- *Method:* `rg -n "cache_github_repo\\([^\\n]*,\\s*(true|false)\\)|refresh: bool" crates/wit/src`
- *Expected:* no matches for boolean cache freshness calls; named acquisition mode is used instead.
- *BDD scenarios covered:* normal read; first read; force invalidation

**Confidence:** 95 / 90 · **Depends on:** branch-cache-metadata DONE · **Closes:** none

**Evidence (required before tick; append-only)**
- *(none yet — when setting Confidence ≥ floor, append a bullet with all three: date + command/check run + outcome (exit code / test counts / artifact path))*
- 2026-07-01 — `rg -n "cache_github_repo\\([^\\n]*,\\s*(true|false)\\)|refresh: bool" crates/wit/src` — exit 1 with no matches; boolean cache freshness calls and `refresh: bool` parameters were replaced by named `CacheAcquisitionMode`.
- 2026-07-01 — `cargo check -p wit` — exit 0; `wit` compiles after the cache acquisition mode refactor.

---

### T2 · Serve cached content before revalidation completes · [x]

**Steps**
- [ ] On a valid cache hit, return the local branch repo immediately in stale-while-revalidate mode.
- [ ] Trigger a detached background revalidation worker after returning the usable cache handle.
- [ ] Avoid `tokio::spawn` tasks that die with the CLI process; use an internal hidden subcommand or equivalent detached process that can outlive the foreground command.
- [ ] Suppress background refresh noise from normal command stdout/stderr unless debugging is explicitly enabled.

**Verification Contract**
- *Check:* when a cached file differs from the newer remote, the first foreground read returns cached content quickly and the background worker later updates the branch cache.
- *Method:* `cargo test -p wit cache_swr_serves_cached_first --lib`
- *Expected:* exit 0; test fails if the foreground call blocks for remote refresh or returns the new content before revalidation completes.
- *BDD scenarios covered:* valid cache hit; remote branch advanced; foreground command remains responsive

**Confidence:** 92 / 90 · **Depends on:** T1 · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- *(none yet)*
- 2026-07-01 — `cargo test -p wit cache_swr_serves_cached_first --lib` — exit 0; 1 passed, 0 failed; local remote advances after initial cache, foreground stale-while-revalidate hit returns cached `README.md` content first, then the shared revalidation worker refreshes repo content and metadata to the new SHA.

---

### T3 · Revalidate by branch SHA and refresh stale caches · [x]

**Steps**
- [ ] Implement a revalidation worker that reads metadata, asks the remote for the current SHA of that branch, and compares it to `current_sha`.
- [ ] If SHAs match, update `last_checked_at` only.
- [ ] If SHAs differ, refresh the branch cache, update `current_sha`, `last_checked_at`, and `last_updated_at`.
- [ ] Preserve the old cache when background refresh fails, and record `last_error` in metadata without breaking foreground reads.

**Verification Contract**
- *Check:* background revalidation refreshes exactly on SHA mismatch and records success/failure metadata.
- *Method:* `cargo test -p wit cache_swr_refreshes_on_sha_change --lib`
- *Expected:* exit 0; tests fail if refresh is age-based, always reclones, misses SHA changes, or destroys usable cache on worker failure.
- *BDD scenarios covered:* remote SHA same; remote SHA changed; remote unavailable; metadata updated after success

**Confidence:** 94 / 90 · **Depends on:** T2 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- *(none yet)*
- 2026-07-01 — `cargo test -p wit cache_swr_refreshes_on_sha_change --lib` — exit 0; 3 passed, 0 failed; tests cover same-SHA checked-at update, SHA mismatch refresh with metadata `current_sha` update, and remote failure preserving old cache while recording `last_error`.

---

### T4 · Add force invalidation path · [x]

**Steps**
- [ ] Implement force invalidation mode that deletes/rebuilds the repo+branch cache before returning.
- [ ] Keep existing `wit cache -r owner/repo` behavior as a force-refresh path.
- [ ] Provide an internal API that repo-scoped CLI commands can use once the public flag is wired in the follow-up goal.
- [ ] Ensure force invalidation updates metadata with the new SHA before returning.

**Verification Contract**
- *Check:* force invalidation returns content from the refreshed branch cache and does not serve stale content.
- *Method:* `cargo test -p wit cache_force_invalidation --lib`
- *Expected:* exit 0; tests fail if force invalidation uses stale cache or returns before metadata/current SHA is updated.
- *BDD scenarios covered:* valid stale cache; forced refresh; metadata reflects new SHA

**Confidence:** 95 / 90 · **Depends on:** T3 · **Closes:** DoD-3

**Evidence (required before tick; append-only)**
- *(none yet)*
- 2026-07-01 — `cargo test -p wit cache_force_invalidation --lib` — exit 0; 1 passed, 0 failed; local stale cache is force-invalidated after remote branch advances, returned repo contains refreshed content, and metadata `current_sha` matches the new remote SHA before return.

---

### T5 · Replace global mutation lock with repo+branch locks · [x]

**Steps**
- [ ] Scope cache mutation and revalidation locks to the branch cache key.
- [ ] Preserve serialization for the same repo+branch.
- [ ] Allow different branches of the same repo and different repos to cache/revalidate concurrently.
- [ ] Keep lock files out of bare repo internals.

**Verification Contract**
- *Check:* lock behavior is per repo+branch, not global.
- *Method:* `cargo test -p wit cache_branch_locks --lib`
- *Expected:* exit 0; tests fail if unrelated branches block behind one global cache lock or if same-key mutations race.
- *BDD scenarios covered:* same repo same branch serializes; same repo different branch can proceed; different repo can proceed

**Confidence:** 92 / 90 · **Depends on:** T4 · **Closes:** DoD-4

**Evidence (required before tick; append-only)**
- *(none yet)*
- 2026-07-01 — `cargo test -p wit cache_branch_locks --lib` — exit 0; 1 passed, 0 failed; same repo+branch lock blocks a second in-process acquisition while same repo/different branch and different repo locks acquire independently.

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-01 — Adversarial review before scope confirm
- **Context:** User rejected max-life TTL and asked for stale-while-revalidate driven by branch SHA metadata.
- **Decision:** Plan uses SHA mismatch as the only stale trigger and treats max-age/TTL flags as out of scope.
- **Alternatives rejected:** Expose `--max-age`; synchronously check remote SHA before every read; use `tokio::spawn` only for background work inside a short-lived CLI process.
- **Why surface:** A CLI background task must survive command exit, and SHA-based freshness must not degrade into a public TTL policy.
- **Scope impact:** none (pre-confirm authoring edit)

### 2026-07-01 — T1 named cache acquisition modes
- **Context:** The branch metadata prerequisite still exposed cache behavior as `refresh: bool`, making stale-while-revalidate versus force invalidation easy to mix up at call sites.
- **Decision:** Introduce `CacheAcquisitionMode::ServeStaleAndRevalidate` for ordinary reads and `CacheAcquisitionMode::ForceInvalidate` for explicit cache refresh paths.
- **Alternatives rejected:** Keep boolean parameters; add a public TTL/max-age option; let repo-scoped commands choose cache behavior implicitly through raw booleans.
- **Why surface:** Later tasks need mode-specific behavior without guessing what `true` or `false` means.
- **Scope impact:** none

### 2026-07-01 — T2 foreground stale serving
- **Context:** Warm no-branch cache reads can select existing branch metadata without remote access after the prerequisite goal.
- **Decision:** On a valid `ServeStaleAndRevalidate` cache hit, return the local repo immediately and launch a quiet hidden `__cache-revalidate` worker in production; unit tests call the same revalidation worker directly to keep local-remote proof deterministic.
- **Alternatives rejected:** Block the foreground read on SHA comparison; use `tokio::spawn` tied to the short-lived CLI process; print background worker noise in normal commands.
- **Why surface:** This establishes the foreground behavior while leaving detailed SHA/failure assertions to T3.
- **Scope impact:** none

### 2026-07-01 — T3 SHA-only revalidation
- **Context:** Freshness must be driven by branch SHA metadata, not cache age or a public TTL.
- **Decision:** Revalidation reads existing metadata, resolves the remote SHA for that branch, updates only `last_checked_at` when SHAs match, refreshes from a staging clone when SHAs differ, and records `last_error` without deleting the usable cache when remote checks fail.
- **Alternatives rejected:** Always reclone; age-based invalidation; delete the current cache before proving the replacement clone can be fetched.
- **Why surface:** This is the core stale-while-revalidate contract for the follow-up force and lock tasks.
- **Scope impact:** none

### 2026-07-01 — T4 force invalidation semantics
- **Context:** The CLI `wit cache -r` path and future public force flag must bypass stale serving.
- **Decision:** `CacheAcquisitionMode::ForceInvalidate` resolves the current remote default branch/SHA and rebuilds the branch cache before returning the repository handle.
- **Alternatives rejected:** Reuse stale cache in force mode; return before metadata is updated; add a public flag in this goal.
- **Why surface:** This preserves current `wit cache` behavior while keeping public CLI flag wiring for the next goal.
- **Scope impact:** none

### 2026-07-01 — T5 branch-scoped locks
- **Context:** A single root `.cache.lock` serialized unrelated cache operations and conflicted with stale-while-revalidate background work.
- **Decision:** Lock files now live beside branch cache metadata at `branches/<encoded>/.cache.lock`, with an in-process per-path guard so same-key operations serialize while unrelated repo/branch keys can proceed.
- **Alternatives rejected:** Keep the global root lock; put locks inside `repo.git`; rely only on OS file locks within one process.
- **Why surface:** Background revalidation and force invalidation need mutation safety without blocking unrelated cache keys.
- **Scope impact:** none

### 2026-07-01 — Final verification for stale-while-revalidate goal
- **Context:** All DoD-specific stale-while-revalidate tests passed after lock and CLI hidden-command changes.
- **Decision:** Treat the stale-while-revalidate goal as done after format, clippy, full workspace tests, migration guard, and ignored cache-lock integration all passed.
- **Alternatives rejected:** Stop after unit selectors without rerunning the subprocess lock integration; leave the next CLI/docs goal blocked.
- **Why surface:** The root GDD index can now unblock the final CLI/docs/tests goal.
- **Scope impact:** none

---

## 7. Learnings · LIVE (append-only)

Flash cards: trigger → wrong action → revision → correct action, with impact `1–5`.
When an attempt failed and the fix is not yet known, log the **open form** —
trigger → wrong action → *(open: revision/correct not yet found)* → pointer to the raw
failure (log path or commit) — still impact-tagged, so a dead-end is recorded before a
fresh context re-treads it.

*(none yet)*

---

## 8. Skills · LIVE (append-only)

Reusable workflows created via the **skill-creator** skill while working this goal.

*(none yet)*

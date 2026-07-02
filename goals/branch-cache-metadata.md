---
goal_id: "branch-cache-metadata"
title: "Add branch-aware cache metadata"
status: "done"              # active | blocked | exited | done
confidence_floor: 90        # a Task below this CANNOT be ticked done
created: "2026-07-01"
updated: "2026-07-01"
---

# Goal: Each cache entry is addressed by repository and branch, and carries durable branch metadata.

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

- User feedback, 2026-07-01 — cache split per repo is critical; cache entries must become per repo and branch even before public branch reads ship.
- User feedback, 2026-07-01 — cache metadata must store enough branch/SHA state for stale-while-revalidate rather than a public max-life TTL.
- `crates/wit/src/gitops/ops.rs` — owns `wit_cache_dir`, `cache_github_repo`, cache locking, shallow bare clone, fallback clone, and `cache_has_head_commit`.
- `crates/wit/src/cli.rs` — repo-scoped commands currently call `cache_github_repo(&repo, false)` and `wit cache` calls `cache_github_repo(&repo, true)`.
- `crates/wit/tests/cache_lock_integration.rs` — existing process-level cache lock proof to adapt toward repo+branch cache keys.
- `README.md` and `crates/wit/src/skill/SKILL.md` — user-facing cache semantics that must stay aligned with implementation.

---

## 3. Definition of Done · INVARIANT

Each item is **atomic** (one verifiable assertion per checkbox), tagged with a
stable id that Tasks reference via **Closes:**, and carries a concrete `verify by:`.

Tick a `DoD-N` box only when its own `verify by:` has been run and passed (not merely
because a closing Task is ticked). Log the command and its outcome as an Evidence bullet
under the Task that **Closes:** it. DONE requires every DoD box ticked.

- [x] **DoD-1** — cache paths are derived from `(owner/repo, branch)` and two branch names for the same repo cannot map to the same bare repo directory — *verify by:* `cargo test -p wit cache_branch_paths --lib`
- [x] **DoD-2** — a call with no requested branch resolves the remote default branch name and SHA, then stores the default branch under the same branch-keyed layout used for explicit branches — *verify by:* `cargo test -p wit cache_default_branch_resolution --lib`
- [x] **DoD-3** — each branch cache has schema-versioned metadata containing at least `owner_repo`, `branch`, `remote_url`, `current_sha`, `last_checked_at`, `last_updated_at`, and `cache_schema_version` — *verify by:* `cargo test -p wit cache_metadata --lib`
- [x] **DoD-4** — metadata writes are atomic and never leave a valid bare repo paired with unreadable partial metadata after an interrupted write simulation — *verify by:* `cargo test -p wit cache_metadata_atomicity --lib`

---

## 4. Exit Conditions

The goal terminates when **any** condition holds. On exit, state which fired —
explicitly — in the response to the user. Specialize the bracketed values for this goal.

- **`DONE`** — all §3 items ticked and all §5 tasks ≥ confidence floor. *(primary)*
- **`BLOCKED-DEP`** — local `git` CLI or `gix` APIs cannot resolve a branch SHA from a local test remote after one direct retry. Exit without the blocked step; name it explicitly.
- **`SCOPE-CHANGE`** — work cannot complete without changing scope. Record the
  proposal in §6 and exit to the user.
- **`CONFIDENCE-STALL`** — a task cannot reach the floor after 3 honest attempts. Exit, report the task and the gap.
- **`BUDGET`** — 2 implementation days reached before all DoD items pass. Exit and report progress.

---

## 5. Tasks · INVARIANT

Ordered, dependency-aware units of work that together satisfy the DoD. Tick the
trailing `[ ]` only when the Verification Contract passes and Confidence ≥ floor.

---

### T1 · Inventory current cache entry contract · [x]

**Steps**
- [ ] Re-read `cache_github_repo`, `wit_cache_dir`, `recache_repo`, lock acquisition, and CLI call sites.
- [ ] Identify all tests that assume cache path equals `$WIT_CACHE_DIR/owner/repo`.
- [ ] Record the chosen target layout before editing code.

**Verification Contract**
- *Check:* Every cache path caller and test assumption is listed before refactor starts.
- *Method:* `rg -n "cache_github_repo|wit_cache_dir|\\.cache\\.lock|WIT_CACHE_DIR|cache_dir\\.join\\(repo\\)|owner_repo" crates/wit/src crates/wit/tests README.md`
- *Expected:* Search output reviewed and relevant references are reflected in §2 or §6 before code edits.
- *BDD scenarios covered:* orientation prevents preserving the old repo-only path by accident

**Confidence:** 95 / 90 · **Depends on:** none · **Closes:** none

**Evidence (required before tick; append-only)**
- *(none yet — when setting Confidence ≥ floor, append a bullet with all three: date + command/check run + outcome (exit code / test counts / artifact path))*
- 2026-07-01 — `rg -n "cache_github_repo|wit_cache_dir|\\.cache\\.lock|WIT_CACHE_DIR|cache_dir\\.join\\(repo\\)|owner_repo" crates/wit/src crates/wit/tests README.md` — exit 0; reviewed repo-only cache path in `crates/wit/src/gitops/ops.rs`, CLI `cache_github_repo(&repo, ...)` callers, `.cache.lock` tests, and integration tests asserting `$WIT_CACHE_DIR/owner/repo`.

---

### T2 · Introduce branch cache identity and safe paths · [x]

**Steps**
- [ ] Add a `CacheTarget` / `CacheKey` type for `owner_repo` plus resolved branch.
- [ ] Encode branch names safely so `feature/x`, `feature%2Fx`, and other filesystem-sensitive names cannot collide.
- [ ] Move bare repos under a branch-scoped directory such as `$WIT_CACHE_DIR/OWNER/REPO/branches/ENCODED_BRANCH/repo.git`.
- [ ] Replace repo-only cache path tests with branch-key collision tests.

**Verification Contract**
- *Check:* Same repo with `main` and `feature/x` uses distinct repo directories, and encoded names are deterministic.
- *Method:* `cargo test -p wit cache_branch_paths --lib`
- *Expected:* exit 0; tests fail if branch is ignored or two branches collide.
- *BDD scenarios covered:* same repo different branches; branch with slash; branch with percent-like text; same branch same path

**Confidence:** 95 / 90 · **Depends on:** T1 · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- *(none yet)*
- 2026-07-01 — `cargo test -p wit cache_branch_paths --lib` — exit 0; 3 passed, 0 failed; tests cover owner/repo/branch path layout, slash and percent encoding, case-sensitive collision avoidance, dot branch escaping, and invalid identity rejection.
- 2026-07-01 — `cargo test -p wit cache_branch_paths --lib` — exit 0; 3 passed, 0 failed after review hardening; tests now also prove branch cache directories use a safe `b-` prefix so Windows-reserved branch names such as `con` do not become reserved path components.

---

### T3 · Resolve default branch as a branch key · [x]

**Steps**
- [ ] Add a default-branch resolver that obtains remote `HEAD` branch name and SHA before choosing the cache path.
- [ ] Prefer a pure Rust/gix path when practical; allow `git ls-remote --symref` fallback if needed and already acceptable in this crate.
- [ ] Ensure no-branch calls resolve to the actual default branch name, not a synthetic `default` directory.
- [ ] Cover default-branch resolution with local bare-remotes so tests do not require GitHub network.

**Verification Contract**
- *Check:* no-branch cache calls resolve the remote default branch name and SHA before cache path selection.
- *Method:* `cargo test -p wit cache_default_branch_resolution --lib`
- *Expected:* exit 0; tests create a local remote whose default branch is not hard-coded to `main` and assert metadata/path use that branch.
- *BDD scenarios covered:* default branch is `main`; default branch is `trunk`; remote HEAD points to a branch with a slash

**Confidence:** 95 / 90 · **Depends on:** T2 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- *(none yet)*
- 2026-07-01 — `cargo test -p wit cache_default_branch_resolution --lib` — exit 0; 3 passed, 0 failed; local bare-remotes prove no-branch cache target resolves remote HEAD branch and SHA before path selection for `trunk` and `release/v1`, and rejects unresolved HEAD.
- 2026-07-01 — `cargo test -p wit cache_default_branch_resolution --lib` — exit 0; 5 passed, 0 failed after review hardening; added proof that warm reads can select existing branch metadata without remote access and that the real cache write path removes a legacy repo-only bare cache before writing `branches/b-trunk/repo.git` plus adjacent metadata.

---

### T4 · Add schema-versioned branch metadata · [x]

**Steps**
- [ ] Define a metadata struct with `cache_schema_version`, `owner_repo`, `remote_url`, `branch`, `current_sha`, `last_checked_at`, `last_updated_at`, and optional `last_error`.
- [ ] Write metadata next to each branch repo, not in a repo-wide singleton.
- [ ] Read metadata through a typed parser and treat incompatible/missing/corrupt metadata as recache-needed rather than silently trusting it.
- [ ] Write metadata through temp-file plus rename so readers never observe partial JSON.

**Verification Contract**
- *Check:* metadata round-trips, includes required fields, rejects incompatible schema, and survives interrupted-write simulation.
- *Method:* `cargo test -p wit cache_metadata --lib && cargo test -p wit cache_metadata_atomicity --lib`
- *Expected:* exit 0 for both commands; tests fail if metadata is repo-wide, schema-free, missing SHA, or partially readable.
- *BDD scenarios covered:* fresh clone metadata; corrupt metadata; old schema; interrupted metadata write

**Confidence:** 95 / 90 · **Depends on:** T3 · **Closes:** DoD-3, DoD-4

**Evidence (required before tick; append-only)**
- *(none yet)*
- 2026-07-01 — `cargo test -p wit cache_metadata --lib` — exit 0; 3 passed, 0 failed; tests verify required schema-versioned fields, round-trip parsing, usable metadata identity, and missing/corrupt/incompatible metadata rejection.
- 2026-07-01 — `cargo test -p wit cache_metadata_atomicity --lib` — exit 0; 1 passed, 0 failed; interrupted temp-write simulation leaves a valid bare repo paired with the prior readable `metadata.json`, and a later atomic write replaces it cleanly.
- 2026-07-01 — `cargo test -p wit cache_metadata --lib` — exit 0; 5 passed, 0 failed after review hardening; added real cache-path metadata coverage and stale-metadata removal before repo replacement.
- 2026-07-01 — `cargo test -p wit cache_metadata_atomicity --lib` — exit 0; 2 passed, 0 failed after review hardening; temp-write interruption now uses the production temp writer before rename and verifies stale metadata removal makes a valid bare repo unusable until metadata is rewritten.

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-01 — Adversarial review before scope confirm
- **Context:** User asked for a detailed implementation plan for branch-split cache files and metadata as groundwork for stale-while-revalidate.
- **Decision:** Split branch-key/metadata into its own prerequisite goal before any refresh behavior.
- **Alternatives rejected:** Hide default branch under a synthetic `default` key; add a public branch flag in this goal; keep a repo-wide metadata file.
- **Why surface:** The stale detection goal depends on reliable branch identity and per-branch SHA metadata.
- **Scope impact:** none (pre-confirm authoring edit)

### 2026-07-01 — T1 cache identity inventory
- **Context:** Current code derives a bare clone path directly from `$WIT_CACHE_DIR/owner/repo`, while CLI callers pass only `owner/repo` and integration tests assert that repo-only path.
- **Decision:** Target layout for this goal is `$WIT_CACHE_DIR/OWNER/REPO/branches/ENCODED_BRANCH/repo.git`, with branch metadata next to `repo.git`; keep the global `.cache.lock` at `$WIT_CACHE_DIR/.cache.lock` until the stale-while-revalidate goal narrows locking.
- **Alternatives rejected:** Keep repo-only bare repo paths; use a synthetic `default` branch key; store branch metadata in a repo-wide singleton.
- **Why surface:** The branch cache key must be explicit before refactoring path tests and default-branch resolution.
- **Scope impact:** none

### 2026-07-01 — T2 branch path encoding
- **Context:** Branch names can contain slashes, percent-like text, dots, and uppercase letters that collide on case-insensitive filesystems.
- **Decision:** Encode branch directory names byte-by-byte, leaving only lowercase ASCII, digits, `_`, and `-` raw; percent-encode every other byte with uppercase hex.
- **Alternatives rejected:** Raw branch names; ordinary percent encoding that leaves `%` or uppercase letters unescaped; hashing branch names without a readable key.
- **Why surface:** DoD-1 depends on deterministic, non-colliding branch cache paths.
- **Scope impact:** none

### 2026-07-01 — T3 default branch resolver
- **Context:** The public cache API has no branch argument, but the cache path now needs a real branch key before clone/open.
- **Decision:** Resolve `HEAD` with `git ls-remote --symref <remote> HEAD`, parse both `refs/heads/<branch>` and the HEAD SHA, then build the branch-scoped cache target from that resolved branch.
- **Alternatives rejected:** Hard-code `main`; use a synthetic `default` directory; choose the cache path before resolving the remote default branch.
- **Why surface:** Stale-while-revalidate needs both durable branch identity and the remote SHA that selected that identity.
- **Scope impact:** none

### 2026-07-01 — T4 metadata compatibility boundary
- **Context:** Existing branch cache directories are trustworthy only when their metadata is readable, schema-compatible, and matches the resolved owner/repo, branch, and remote URL.
- **Decision:** Store `metadata.json` beside each branch `repo.git`; read through typed serde validation; treat missing, corrupt, schema-incompatible, or identity-mismatched metadata as recache-needed; write via `metadata.json.tmp` plus rename.
- **Alternatives rejected:** Repo-wide metadata; trusting a valid bare repo without metadata; direct in-place metadata writes.
- **Why surface:** The next stale-refresh goal can compare remote SHA state without inheriting repo-only or partially written metadata.
- **Scope impact:** none

### 2026-07-01 — Review hardening for cache safety
- **Context:** Read-only review found that remote HEAD resolution before cache reuse regressed warm-cache/offline behavior, ran outside the cache lock, nested the new layout inside legacy bare caches, and left Windows-reserved branch names possible.
- **Decision:** Acquire the cache lock before default resolution, reuse exactly one existing branch metadata entry for warm no-branch reads, remove legacy repo-only bare caches before creating `branches/...`, prefix every encoded branch directory with `b-`, and remove stale metadata before replacing `repo.git`.
- **Alternatives rejected:** Require remote access for every warm read; keep `main`/raw encoded branch dirs; trust old metadata while replacing the repo.
- **Why surface:** These changes keep this prerequisite aligned with the next stale-while-revalidate goal without implementing background refresh yet.
- **Scope impact:** none

### 2026-07-01 — Final verification for branch metadata goal
- **Context:** All DoD-specific tests were green after review hardening.
- **Decision:** Treat the branch-cache metadata prerequisite as done after broad repo gates and the ignored cache-lock integration test passed.
- **Alternatives rejected:** Stop at targeted helper tests without end-to-end cache-path coverage; skip the networked lock proof after moving default resolution under the lock.
- **Why surface:** The root GDD index can now unblock stale-while-revalidate work with current evidence.
- **Scope impact:** none

---

## 7. Learnings · LIVE (append-only)

Flash cards: trigger → wrong action → revision → correct action, with impact `1–5`.
When an attempt failed and the fix is not yet known, log the **open form** —
trigger → wrong action → *(open: revision/correct not yet found)* → pointer to the raw
failure (log path or commit) — still impact-tagged, so a dead-end is recorded before a
fresh context re-treads it.

- 2026-07-01 — Review flagged warm-cache remote probes → Wrong action: resolved `git ls-remote` before checking existing metadata and before taking the cache lock → Revision: preserve branch-keyed cache identity while serving an existing single branch metadata entry first → Correct action: lock first, clean legacy cache roots, use cached metadata for warm no-branch reads, and reserve remote resolution for cold/refresh/ambiguous cases. impact: 5/5

---

## 8. Skills · LIVE (append-only)

Reusable workflows created via the **skill-creator** skill while working this goal.

*(none yet)*

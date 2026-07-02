---
goal_id: "branch-cache-selection-api"
title: "Add branch cache selection API"
status: "done"              # draft | ready | in-progress | done | exited
confidence_floor: 90        # a Task below this CANNOT be ticked done
created: "2026-07-02"
updated: "2026-07-02"
---

# Goal: Shared cache acquisition can target either the default branch or a named branch without mixing branch caches.

## 1. Invariants · the rules that must not break

This file is the only state for this delivery subgoal — if it isn't written here,
it didn't happen. The full procedure (boot loop, confidence rubric, logging cadence) lives in the
**goal-driven-development** skill; these rules hold even if that skill isn't loaded:

- **Scope is frozen once execution begins** (`status: in-progress`). Until then, §3
  and §5 may be edited freely. Deliver the goal; user comments or adjusts. Pointing
  the agent at this file is approval to execute. After execution begins, the only
  permitted edits are: tick checkboxes (Task **and** DoD), update Confidence, append
  Evidence, append to the live sections (§6/§7/§8), and update frontmatter
  `status`/`updated` — never add, remove, reword, split, or merge a DoD item or Task,
  and never rewrite or delete a live-section entry.
- **Never tick below the floor.** A task is ticked done only at Confidence ≥
  `confidence_floor`. If you cannot reach it, leave it unticked and fire `CONFIDENCE-STALL`.
- **Scope change is an exit, not a decision.** If scope must change, record the
  proposal in §6 and fire `SCOPE-CHANGE` — stop and surface it to the user.
- **Live sections are append-only.** Log each decision (§6) and learning (§7) at
  the moment it happens — before ticking the task it came from. Never delete entries.

---

## 2. References

Everything the agent needs before/while working. Each entry is `path-or-url — why it matters`.

- User request, 2026-07-02 — public `wit` CLI and MCP surfaces need an optional branch parameter for reading branches other than repo defaults.
- `GOAL.md` — root index for this multi-goal branch-parameter plan.
- `goals/branch-cache-metadata.md` — completed prerequisite that created repo+branch cache identity, safe branch keys, and metadata.
- `goals/stale-while-revalidate-cache.md` — completed prerequisite that added SHA-based stale-while-revalidate and branch-scoped locks.
- `docs/adr/0002-branch-keyed-stale-while-revalidate-cache.md` — records the existing branch-keyed cache contract and explicitly deferred public branch reads.
- `crates/wit/src/gitops/ops.rs` — owns `CacheAcquisitionMode`, `CacheTarget`, default branch resolution, branch SHA resolution, cache acquisition, and revalidation worker routing.
- `crates/wit/src/cli.rs` — current CLI consumers call `cache_github_repo(&repo, mode)` and hidden `__cache-revalidate` takes only `--repo`.
- `crates/wit/src/mcp.rs` — current MCP consumers call `cache_github_repo(&args.repo, cache_mode(args.refresh_cache))`.
- `crates/wit/tests/cache_lock_integration.rs` — existing process-level branch cache proof to keep compatible with explicit branches.

---

## 3. Definition of Done · INVARIANT

Each item is **atomic** (one verifiable assertion per checkbox), tagged with a
stable id that Tasks reference via **Closes:**, and carries a concrete `verify by:`.

Tick a `DoD-N` box only when its own `verify by:` has been run and passed (not merely
because a closing Task is ticked). Log the command and its outcome as an Evidence bullet
under the Task that **Closes:** it. DONE requires every DoD box ticked.

- [x] **DoD-1** — the shared cache API accepts an explicit branch selector while no selector preserves existing default-branch resolution — *verify by:* `cargo test -p wit cache_explicit_branch_selection --lib`
- [x] **DoD-2** — explicit branch caches for the same repo are isolated from the default branch and from each other, including slash and percent-like branch names — *verify by:* `cargo test -p wit cache_explicit_branch_isolation --lib`
- [x] **DoD-3** — stale-while-revalidate, force invalidation, and the hidden revalidation worker all revalidate the requested branch, not the repo default — *verify by:* `cargo test -p wit cache_explicit_branch_revalidation --lib`
- [x] **DoD-4** — existing default-branch cache behavior still passes after the public branch-selection API is introduced — *verify by:* `cargo test -p wit cache_ --lib`

---

## 4. Exit Conditions

The goal terminates when **any** condition holds. On exit, state which fired —
explicitly — in the response to the user.

- **`DONE`** — all §3 items ticked and all §5 tasks ≥ confidence floor. *(primary)*
- **`BLOCKED-DEP`** — local `git` CLI or `gix` APIs cannot resolve named branch SHAs from local test remotes after one direct retry. Exit without the blocked step; name it explicitly.
- **`SCOPE-CHANGE`** — implementation requires supporting tags, commit SHAs, pull request refs, or arbitrary refs instead of GitHub branch names under `refs/heads`. Record the proposal in §6 and exit to the user.
- **`CONFIDENCE-STALL`** — a task cannot reach the floor after two honest implementation and verification attempts. Exit, report the task and the gap.
- **`BUDGET`** — six focused implementation cycles or three cache-test red/green loops are reached without satisfying DoD-1 through DoD-4. Exit and report progress.

---

## 5. Tasks · INVARIANT

Ordered, dependency-aware units of work that together satisfy the DoD. Tick the
trailing `[ ]` only when the Verification Contract passes and Confidence ≥ floor.

---

### T1 · Inventory cache target and consumer assumptions · [x]

**Steps**
- [x] Re-read `cache_github_repo`, `default_cache_target_for_cache`, `cached_default_cache_target`, `resolve_branch_sha`, `spawn_cache_revalidation`, `revalidate_github_repo`, and all call sites.
- [x] Identify source tests that currently assert no public branch selection or default-only behavior.
- [x] Record any non-obvious compatibility decision in §6 before touching the shared API.

**Verification Contract**
- *Check:* the execution agent has a complete map of API consumers and stale default-only assertions before refactor starts.
- *Method:* `rg -n "cache_github_repo|revalidate_github_repo|__cache-revalidate|long = \"branch\"|branch-selection|Public branch selection|resolve_branch_sha" crates/wit/src crates/wit/tests README.md docs goals`
- *Expected:* exit 0; relevant call sites and stale assertions are reviewed and reflected in §6 or the later patch.
- *BDD scenarios covered:* default-only API inventory; hidden worker inventory; stale no-branch assertion inventory

**Confidence:** 95 / 90 · **Depends on:** prior cache-freshness goals DONE · **Closes:** none

**Evidence (required before tick; append-only)**
- 2026-07-02 — `rg -n "cache_github_repo|revalidate_github_repo|__cache-revalidate|long = \"branch\"|branch-selection|Public branch selection|resolve_branch_sha" crates/wit/src crates/wit/tests README.md docs goals` — exit 0; reviewed shared cache call sites in `ops.rs`, CLI and MCP callers, hidden revalidation args, README/skill/source-contract assertions that still say public branch selection is absent.

---

### T2 · Introduce an explicit branch selector in the cache API · [x]

**Steps**
- [x] Add a typed branch selector or cache request shape, such as default branch versus named branch, without adding tag/SHA semantics.
- [x] Preserve current no-branch behavior for default branch resolution and warm default cache reads.
- [x] Resolve a cold or forced named branch by asking the remote for `refs/heads/BRANCH`.
- [x] Cover default and named branch selection with local-remotes rather than live GitHub.

**Verification Contract**
- *Check:* explicit branch selection and default selection choose the intended branch target and metadata.
- *Method:* `cargo test -p wit cache_explicit_branch_selection --lib`
- *Expected:* exit 0; tests fail if no selector ignores branch, named branch falls back to default, or default no-selector behavior changes.
- *BDD scenarios covered:* no branch uses remote default; named branch uses `refs/heads/BRANCH`; missing named branch reports the branch name

**Confidence:** 95 / 90 · **Depends on:** T1 · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit cache_explicit_branch_selection --lib` — exit 0; 1 passed, 0 failed; local-remote test proves default selector preserves default branch and named selector reads `refs/heads/feature/api`.

---

### T3 · Keep named branch caches isolated · [x]

**Steps**
- [x] Ensure named branch reads use the existing branch-keyed cache directory and metadata identity.
- [x] Ensure branch names with slash, percent-like text, uppercase letters, or dot components still use collision-safe encoded directories.
- [x] Prevent explicit branch reads from reusing a different branch's metadata or repo.git.

**Verification Contract**
- *Check:* two branch names for one repo cannot collide or return each other's content.
- *Method:* `cargo test -p wit cache_explicit_branch_isolation --lib`
- *Expected:* exit 0; tests fail if explicit branch reads share repo.git with default or another branch, or if branch path encoding regresses.
- *BDD scenarios covered:* default versus feature branch; `release/v1`; percent-like branch text; branch name case

**Confidence:** 95 / 90 · **Depends on:** T2 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit cache_explicit_branch_isolation --lib` — exit 0; 1 passed, 0 failed; local-remote test proves default, slash, percent-like, uppercase, and dotted branch names use distinct cache paths and metadata.

---

### T4 · Thread branch selection through revalidation paths · [x]

**Steps**
- [x] Pass the selected branch to `ServeStaleAndRevalidate` background workers.
- [x] Pass the selected branch to explicit force invalidation.
- [x] Extend hidden `__cache-revalidate` arguments so revalidation cannot drift back to the default branch.
- [x] Preserve quiet background refresh behavior for normal reads.

**Verification Contract**
- *Check:* foreground and background freshness work against the exact selected branch.
- *Method:* `cargo test -p wit cache_explicit_branch_revalidation --lib`
- *Expected:* exit 0; tests fail if worker args omit branch, force invalidation refreshes default, or SHA comparison uses the wrong branch.
- *BDD scenarios covered:* stale named branch; forced named branch refresh; hidden worker branch propagation

**Confidence:** 95 / 90 · **Depends on:** T3 · **Closes:** DoD-3

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit cache_explicit_branch_revalidation --lib` — exit 0; 2 passed, 0 failed; local-remote tests prove named-branch stale revalidation, force invalidation, and hidden worker `--branch` argument propagation.

---

### T5 · Prove cache regression coverage after the API change · [x]

**Steps**
- [x] Run the existing cache unit-test family after named branch support lands.
- [x] Run the ignored cache-lock integration test if the shared cache lock or cache layout is touched.
- [x] Record any skipped integration check explicitly in Evidence with the blocker.

**Verification Contract**
- *Check:* default branch, named branch, SWR, force refresh, and branch locks still pass together.
- *Method:* `cargo test -p wit cache_ --lib && cargo test -p wit --test cache_lock_integration -- --ignored`
- *Expected:* exit 0 for both commands, or a documented `BLOCKED-DEP` if the ignored integration test cannot run because network or GitHub is unavailable.
- *BDD scenarios covered:* default branch cache; named branch cache; stale refresh; force refresh; same branch lock; different branch lock

**Confidence:** 95 / 90 · **Depends on:** T4 · **Closes:** DoD-4

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit cache_ --lib` — exit 0; 26 passed, 0 failed, 2 ignored; existing default-branch cache behavior and new explicit branch tests pass together.
- 2026-07-02 — `cargo test -p wit --test cache_lock_integration -- --ignored` — exit 0; 2 passed, 0 failed; process-level cache lock integration still serializes cache and rg operations.
- 2026-07-02 — Post-review regression check `cargo test -p wit cache_default_branch --lib && cargo test -p wit --test branch_cli_integration` — exit 0; 6 default-branch cache tests and 1 branch CLI integration test passed, including a new no-branch warm-cache read with the remote unavailable.
- 2026-07-02 — Post-marker concurrency check `cargo test -p wit --test cache_lock_integration -- --ignored` — exit 0 after unique marker temp files; 2 passed, 0 failed; parallel cache/rg operations no longer race writing `default_branch.json`.

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-02 — Adversarial review before delivery
- **Context:** User asked for public branch selection across CLI and MCP; prior completed cache plan explicitly deferred public branch reads.
- **Decision:** Split shared cache selection into its own prerequisite goal so CLI and MCP cannot implement divergent branch behavior, and constrain the selector to GitHub branch names rather than tags, SHAs, or arbitrary refs.
- **Alternatives rejected:** Add `--branch` directly in CLI/MCP without changing hidden revalidation; accept arbitrary git refs; fold all work into one broad goal.
- **Why surface:** The main risk is a user asking for a branch while background refresh or force refresh silently reads the default branch.
- **Scope impact:** none (pre-confirm authoring edit)

### 2026-07-02 — Branch selection API shape
- **Context:** Existing callers use `cache_github_repo(owner_repo, mode)` and hidden workers use `revalidate_github_repo(owner_repo)`, while the next goals need a single shared branch selector.
- **Decision:** Add typed `CacheBranchSelection` plus explicit `cache_github_repo_with_branch` and `revalidate_github_repo_with_branch` entry points; keep no-selector wrappers as the default-branch path and make hidden revalidation pass the resolved branch.
- **Alternatives rejected:** Use an untyped `Option<String>` at every call site; reinterpret `owner/repo@branch`; let CLI and MCP build separate branch-target code.
- **Why surface:** Typed selection keeps "default branch" distinct from "named branch" while preserving existing no-branch behavior for commands not yet migrated.
- **Scope impact:** none

### 2026-07-02 — Branch selector implementation shape correction
- **Context:** The prior live decision named wrapper-style entry points, but the implemented shared API directly takes `CacheBranchSelection` so existing callers must make their default-branch intent explicit.
- **Decision:** Use `cache_github_repo(owner_repo, CacheBranchSelection, mode)` and `revalidate_github_repo(owner_repo, CacheBranchSelection)` without keeping legacy wrapper aliases; CLI and MCP public branch flags remain deferred to their own goal files.
- **Alternatives rejected:** Add legacy wrappers only to preserve the old internal signature; update public CLI/MCP branch surfaces inside this prerequisite goal.
- **Why surface:** This keeps the prerequisite focused on one cache selector contract while avoiding unrequested backwards-compatibility aliases.
- **Scope impact:** none

### 2026-07-02 — Default branch cache marker
- **Context:** Review reproduced that no-branch warm reads required live remote HEAD resolution before serving a valid stale default cache, while explicit `--branch` reads served stale content correctly.
- **Decision:** Persist `default_branch.json` under the repo cache root when the default branch is resolved, and trust that marker before remote lookup for non-forced default reads.
- **Alternatives rejected:** Reuse any single cached branch as the default; keep forcing remote default resolution before stale reads; add a public TTL/max-age setting.
- **Why surface:** The marker preserves branch isolation without breaking the stale-while-revalidate contract for default-branch reads while offline.
- **Scope impact:** none

---

## 7. Learnings · LIVE (append-only)

Flash cards: trigger → wrong action → revision → correct action, with impact `1–5`.
When an attempt failed and the fix is not yet known, log the **open form** —
trigger → wrong action → *(open: revision/correct not yet found)* → pointer to the raw
failure (log path or commit) — still impact-tagged, so a dead-end is recorded before a
fresh context re-treads it.

### 2026-07-02 — Case-sensitive branch fixture on macOS
- **Trigger:** Testing branch-name case with both `feature/x` and `Feature/x` in one local remote on a macOS worktree.
- **Wrong action:** Treating those refs as independent local fixture branches.
- **Revision:** macOS case-insensitive filesystem-backed git refs can reject the second branch even though branch-case isolation remains required.
- **Correct action:** Use a distinct uppercase top-level branch such as `Uppercase/x` to cover uppercase encoding without colliding with `feature/x`.
- **impact:** 3/5

### 2026-07-02 — Default stale reads need stored default identity
- **Trigger:** Fixing branch isolation by resolving remote default before consulting cached default metadata.
- **Wrong action:** Making every no-branch stale read depend on `git ls-remote` before serving a warm cache.
- **Revision:** Default selection needs a durable "this branch was the resolved default" marker, not a scan of arbitrary branch caches.
- **Correct action:** Write `default_branch.json` after default resolution and use it to serve a valid default cache before remote lookup on non-forced reads.
- **impact:** 5/5

### 2026-07-02 — Shared default marker temp path races under parallel cache reads
- **Trigger:** Running ignored cache-lock integration after adding `default_branch.json`.
- **Wrong action:** Writing the default marker before the branch lock with a fixed temp filename.
- **Revision:** Parallel processes can all resolve the same default branch and try to rename the same temp file.
- **Correct action:** Use unique marker temp filenames before atomic rename, then keep branch cache mutations under the existing branch lock.
- **impact:** 4/5

---

## 8. Skills · LIVE (append-only)

Reusable workflows created via the **skill-creator** skill while working this goal.

*(none yet)*

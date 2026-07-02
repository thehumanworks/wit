---
goal_id: "branch-cache-selection-api"
title: "Add branch cache selection API"
status: "ready"             # draft | ready | in-progress | done | exited
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

- [ ] **DoD-1** — the shared cache API accepts an explicit branch selector while no selector preserves existing default-branch resolution — *verify by:* `cargo test -p wit cache_explicit_branch_selection --lib`
- [ ] **DoD-2** — explicit branch caches for the same repo are isolated from the default branch and from each other, including slash and percent-like branch names — *verify by:* `cargo test -p wit cache_explicit_branch_isolation --lib`
- [ ] **DoD-3** — stale-while-revalidate, force invalidation, and the hidden revalidation worker all revalidate the requested branch, not the repo default — *verify by:* `cargo test -p wit cache_explicit_branch_revalidation --lib`
- [ ] **DoD-4** — existing default-branch cache behavior still passes after the public branch-selection API is introduced — *verify by:* `cargo test -p wit cache_ --lib`

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

### T1 · Inventory cache target and consumer assumptions · [ ]

**Steps**
- [ ] Re-read `cache_github_repo`, `default_cache_target_for_cache`, `cached_default_cache_target`, `resolve_branch_sha`, `spawn_cache_revalidation`, `revalidate_github_repo`, and all call sites.
- [ ] Identify source tests that currently assert no public branch selection or default-only behavior.
- [ ] Record any non-obvious compatibility decision in §6 before touching the shared API.

**Verification Contract**
- *Check:* the execution agent has a complete map of API consumers and stale default-only assertions before refactor starts.
- *Method:* `rg -n "cache_github_repo|revalidate_github_repo|__cache-revalidate|long = \"branch\"|branch-selection|Public branch selection|resolve_branch_sha" crates/wit/src crates/wit/tests README.md docs goals`
- *Expected:* exit 0; relevant call sites and stale assertions are reviewed and reflected in §6 or the later patch.
- *BDD scenarios covered:* default-only API inventory; hidden worker inventory; stale no-branch assertion inventory

**Confidence:** 0 / 90 · **Depends on:** prior cache-freshness goals DONE · **Closes:** none

**Evidence (required before tick; append-only)**
- *(none yet — when setting Confidence ≥ floor, append a bullet with all three: date + command/check run + outcome (exit code / test counts / artifact path))*

---

### T2 · Introduce an explicit branch selector in the cache API · [ ]

**Steps**
- [ ] Add a typed branch selector or cache request shape, such as default branch versus named branch, without adding tag/SHA semantics.
- [ ] Preserve current no-branch behavior for default branch resolution and warm default cache reads.
- [ ] Resolve a cold or forced named branch by asking the remote for `refs/heads/BRANCH`.
- [ ] Cover default and named branch selection with local-remotes rather than live GitHub.

**Verification Contract**
- *Check:* explicit branch selection and default selection choose the intended branch target and metadata.
- *Method:* `cargo test -p wit cache_explicit_branch_selection --lib`
- *Expected:* exit 0; tests fail if no selector ignores branch, named branch falls back to default, or default no-selector behavior changes.
- *BDD scenarios covered:* no branch uses remote default; named branch uses `refs/heads/BRANCH`; missing named branch reports the branch name

**Confidence:** 0 / 90 · **Depends on:** T1 · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- *(none yet)*

---

### T3 · Keep named branch caches isolated · [ ]

**Steps**
- [ ] Ensure named branch reads use the existing branch-keyed cache directory and metadata identity.
- [ ] Ensure branch names with slash, percent-like text, uppercase letters, or dot components still use collision-safe encoded directories.
- [ ] Prevent explicit branch reads from reusing a different branch's metadata or repo.git.

**Verification Contract**
- *Check:* two branch names for one repo cannot collide or return each other's content.
- *Method:* `cargo test -p wit cache_explicit_branch_isolation --lib`
- *Expected:* exit 0; tests fail if explicit branch reads share repo.git with default or another branch, or if branch path encoding regresses.
- *BDD scenarios covered:* default versus feature branch; `release/v1`; percent-like branch text; branch name case

**Confidence:** 0 / 90 · **Depends on:** T2 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- *(none yet)*

---

### T4 · Thread branch selection through revalidation paths · [ ]

**Steps**
- [ ] Pass the selected branch to `ServeStaleAndRevalidate` background workers.
- [ ] Pass the selected branch to explicit force invalidation.
- [ ] Extend hidden `__cache-revalidate` arguments so revalidation cannot drift back to the default branch.
- [ ] Preserve quiet background refresh behavior for normal reads.

**Verification Contract**
- *Check:* foreground and background freshness work against the exact selected branch.
- *Method:* `cargo test -p wit cache_explicit_branch_revalidation --lib`
- *Expected:* exit 0; tests fail if worker args omit branch, force invalidation refreshes default, or SHA comparison uses the wrong branch.
- *BDD scenarios covered:* stale named branch; forced named branch refresh; hidden worker branch propagation

**Confidence:** 0 / 90 · **Depends on:** T3 · **Closes:** DoD-3

**Evidence (required before tick; append-only)**
- *(none yet)*

---

### T5 · Prove cache regression coverage after the API change · [ ]

**Steps**
- [ ] Run the existing cache unit-test family after named branch support lands.
- [ ] Run the ignored cache-lock integration test if the shared cache lock or cache layout is touched.
- [ ] Record any skipped integration check explicitly in Evidence with the blocker.

**Verification Contract**
- *Check:* default branch, named branch, SWR, force refresh, and branch locks still pass together.
- *Method:* `cargo test -p wit cache_ --lib && cargo test -p wit --test cache_lock_integration -- --ignored`
- *Expected:* exit 0 for both commands, or a documented `BLOCKED-DEP` if the ignored integration test cannot run because network or GitHub is unavailable.
- *BDD scenarios covered:* default branch cache; named branch cache; stale refresh; force refresh; same branch lock; different branch lock

**Confidence:** 0 / 90 · **Depends on:** T4 · **Closes:** DoD-4

**Evidence (required before tick; append-only)**
- *(none yet)*

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-02 — Adversarial review before delivery
- **Context:** User asked for public branch selection across CLI and MCP; prior completed cache plan explicitly deferred public branch reads.
- **Decision:** Split shared cache selection into its own prerequisite goal so CLI and MCP cannot implement divergent branch behavior, and constrain the selector to GitHub branch names rather than tags, SHAs, or arbitrary refs.
- **Alternatives rejected:** Add `--branch` directly in CLI/MCP without changing hidden revalidation; accept arbitrary git refs; fold all work into one broad goal.
- **Why surface:** The main risk is a user asking for a branch while background refresh or force refresh silently reads the default branch.
- **Scope impact:** none (pre-confirm authoring edit)

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

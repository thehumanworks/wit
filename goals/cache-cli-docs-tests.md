---
goal_id: "cache-cli-docs-tests"
title: "Wire cache behavior through CLI docs and tests"
status: "done"              # active | blocked | exited | done
confidence_floor: 90        # a Task below this CANNOT be ticked done
created: "2026-07-01"
updated: "2026-07-01"
---

# Goal: CLI help, README, and tests describe and prove branch-keyed stale-while-revalidate caching without exposing a TTL.

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

- User feedback, 2026-07-01 — expose forceful cache invalidation, not max-life TTL.
- User feedback, 2026-07-01 — branch-specific cache layout must exist now, but public branch reading is a follow-up feature.
- `goals/branch-cache-metadata.md` — prerequisite branch cache identity and metadata.
- `goals/stale-while-revalidate-cache.md` — prerequisite stale-while-revalidate refresh behavior.
- `crates/wit/src/cli.rs` — Clap command definitions and repo-scoped command wiring.
- `README.md` — user-facing install and command docs.
- `crates/wit/src/skill/SKILL.md` — generated/embedded skill docs for `wit` usage.
- `scripts/check_wit_search_migration.sh` — repo-specific final check that must still pass.

---

## 3. Definition of Done · INVARIANT

Each item is **atomic** (one verifiable assertion per checkbox), tagged with a
stable id that Tasks reference via **Closes:**, and carries a concrete `verify by:`.

Tick a `DoD-N` box only when its own `verify by:` has been run and passed (not merely
because a closing Task is ticked). Log the command and its outcome as an Evidence bullet
under the Task that **Closes:** it. DONE requires every DoD box ticked.

- [x] **DoD-1** — repo-scoped commands accept an explicit force-invalidation option and route it to force cache refresh before reading — *verify by:* `cargo test -p wit cli_force_cache_invalidation --lib`
- [x] **DoD-2** — help text and docs document branch-keyed stale-while-revalidate caching, force invalidation, and the absence of any public TTL/max-age option — *verify by:* `cargo test -p wit cli_cache_help_text --lib && rg -n "stale-while-revalidate|refresh-cache|TTL|max-age|branch" README.md crates/wit/src/skill/SKILL.md`
- [x] **DoD-3** — full repo verification passes after cache changes — *verify by:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`

---

## 4. Exit Conditions

The goal terminates when **any** condition holds. On exit, state which fired —
explicitly — in the response to the user. Specialize the bracketed values for this goal.

- **`DONE`** — all §3 items ticked and all §5 tasks ≥ confidence floor. *(primary)*
- **`BLOCKED-DEP`** — either `goals/branch-cache-metadata.md` or `goals/stale-while-revalidate-cache.md` is not done after one direct status check. Exit without the blocked step; name it explicitly.
- **`SCOPE-CHANGE`** — work cannot complete without changing scope. Record the
  proposal in §6 and exit to the user.
- **`CONFIDENCE-STALL`** — a task cannot reach the floor after 3 honest attempts. Exit, report the task and the gap.
- **`BUDGET`** — 1 implementation day reached before all DoD items pass. Exit and report progress.

---

## 5. Tasks · INVARIANT

Ordered, dependency-aware units of work that together satisfy the DoD. Tick the
trailing `[ ]` only when the Verification Contract passes and Confidence ≥ floor.

---

### T1 · Wire force invalidation through repo-scoped CLI commands · [x]

**Steps**
- [x] Add a force-invalidation option, tentatively `--refresh-cache`, to repo-scoped commands that read cached code.
- [x] Ensure `wit cache -r owner/repo` remains a force-refresh command.
- [x] Do not add a public `--branch` flag in this goal; keep branch-targeted cache APIs ready for a later branch-read feature.
- [x] Add CLI parser tests for the new option on representative commands.

**Verification Contract**
- *Check:* CLI parser accepts force invalidation for repo-scoped reads and does not expose a branch-read flag yet.
- *Method:* `cargo test -p wit cli_force_cache_invalidation --lib`
- *Expected:* exit 0; tests fail if force invalidation is not passed to cache acquisition mode or if a public branch flag appears in this goal.
- *BDD scenarios covered:* `tree --refresh-cache`; `cat --refresh-cache`; `rg --refresh-cache`; no branch flag yet

**Confidence:** 95 / 90 · **Depends on:** branch-cache-metadata DONE, stale-while-revalidate-cache DONE · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- 2026-07-01 — `cargo test -p wit cli_force_cache_invalidation --lib` — exit 0; 1 passed, 0 failed, 94 filtered.

---

### T2 · Update user-facing cache documentation · [x]

**Steps**
- [x] Update README cache section to explain default branch cache keying, metadata, stale-while-revalidate, and `--refresh-cache`.
- [x] Update `crates/wit/src/skill/SKILL.md` to match README cache behavior.
- [x] Remove or avoid wording that implies a TTL/max-age cache policy.
- [x] Note that cache storage is per branch internally while public branch selection remains a follow-up feature.

**Verification Contract**
- *Check:* docs mention SWR and force refresh, do not introduce a public TTL option, and do not claim public branch reads exist.
- *Method:* `cargo test -p wit cli_cache_help_text --lib && rg -n "stale-while-revalidate|refresh-cache|TTL|max-age|branch" README.md crates/wit/src/skill/SKILL.md`
- *Expected:* tests exit 0; grep output shows intentional cache wording and no documented public max-age/TTL option.
- *BDD scenarios covered:* user wants fresh content; user wants normal responsive cached read; user asks where branch caches live

**Confidence:** 95 / 90 · **Depends on:** T1 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- 2026-07-01 — `cargo test -p wit cli_cache_help_text --lib` — exit 0; 1 passed, 0 failed, 94 filtered.
- 2026-07-01 — `rg -n "stale-while-revalidate|refresh-cache|TTL|max-age|branch" README.md crates/wit/src/skill/SKILL.md` — exit 0; matched intentional cache contract wording in both docs.

---

### T3 · Run focused and full verification matrix · [x]

**Steps**
- [x] Run focused cache metadata, SWR, force-invalidation, and CLI docs tests.
- [x] Run formatting check.
- [x] Run clippy with warnings denied.
- [x] Run the full workspace test suite.
- [x] Run `scripts/check_wit_search_migration.sh`.
- [x] Inspect `git diff --stat` and ensure no target artifacts or unrelated files are included.

**Verification Contract**
- *Check:* all repo-required verification commands pass after the cache behavior change.
- *Method:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`
- *Expected:* exit 0; any failure is fixed before the goal can be marked done.
- *BDD scenarios covered:* workspace still builds; no grep.app migration regression; all deterministic tests pass

**Confidence:** 95 / 90 · **Depends on:** T2 · **Closes:** DoD-3

**Evidence (required before tick; append-only)**
- 2026-07-01 — `cargo test -p wit cache_metadata --lib` — exit 0; 5 passed, 0 failed, 90 filtered.
- 2026-07-01 — `cargo test -p wit cache_swr --lib` — exit 0; 4 passed, 0 failed, 91 filtered.
- 2026-07-01 — `cargo test -p wit cache_force_invalidation --lib` — exit 0; 1 passed, 0 failed, 94 filtered.
- 2026-07-01 — `cargo test -p wit cli_force_cache_invalidation --lib` — exit 0; 1 passed, 0 failed, 94 filtered.
- 2026-07-01 — `cargo test -p wit cli_cache_help_text --lib` — exit 0; 1 passed, 0 failed, 94 filtered.
- 2026-07-01 — `cargo fmt --all --check` — exit 0.
- 2026-07-01 — `cargo clippy --workspace --all-targets -- -D warnings` — exit 0.
- 2026-07-01 — `cargo test --workspace` — exit 0; deterministic workspace tests passed; ignored live/network tests stayed ignored by default.
- 2026-07-01 — `cargo test -p wit --test cache_lock_integration -- --ignored` — exit 0; 2 passed, 0 failed; observed branch-scoped cache path under `branches/b-master/repo.git`.
- 2026-07-01 — `bash scripts/check_wit_search_migration.sh` — exit 0.
- 2026-07-01 — `git diff --check && git diff --stat && git status --short` — exit 0 for whitespace check; diff limited to intended source/docs/test files and GDD goal docs; `Cargo.lock` churn restored.

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-01 — Adversarial review before scope confirm
- **Context:** User asked for force invalidation and future branch readiness, not necessarily public branch reading now.
- **Decision:** Plan adds `--refresh-cache` for force invalidation and explicitly defers public branch selection.
- **Alternatives rejected:** Add `--max-age`/TTL; ship public branch reads in the same goal; update code without README/skill docs.
- **Why surface:** Documentation and CLI help are how agents will learn the cache freshness contract.
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

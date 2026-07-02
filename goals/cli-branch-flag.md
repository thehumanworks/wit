---
goal_id: "cli-branch-flag"
title: "Add CLI branch flag"
status: "ready"             # draft | ready | in-progress | done | exited
confidence_floor: 90        # a Task below this CANNOT be ticked done
created: "2026-07-02"
updated: "2026-07-02"
---

# Goal: Every repo-reading CLI command can optionally read a named branch with `--branch` while default-branch invocations keep existing behavior.

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

- User request, 2026-07-02 — the `wit` application CLI must accept an optional branch parameter for branches other than repo defaults.
- `goals/branch-cache-selection-api.md` — required prerequisite that makes cache acquisition target a named branch.
- `crates/wit/src/cli.rs` — owns clap command definitions, hidden cache revalidation args, repo cache routing, CLI formatting, and CLI parser tests.
- `crates/wit/src/lib.rs` — contains source-contract tests that currently assert no public `--branch` flag and must be replaced.
- `README.md` — user-facing CLI and cache docs that currently state public branch selection is not exposed.
- `crates/wit/src/skill/SKILL.md` — bundled skill docs that current agents read when using `wit`.
- `docs/adr/0002-branch-keyed-stale-while-revalidate-cache.md` — should gain a follow-up note or remain historically accurate while docs announce public branch selection.
- `crates/wit/tests/cache_lock_integration.rs` — existing binary/integration cache proof to reuse or extend for branch-aware CLI commands.

---

## 3. Definition of Done · INVARIANT

Each item is **atomic** (one verifiable assertion per checkbox), tagged with a
stable id that Tasks reference via **Closes:**, and carries a concrete `verify by:`.

Tick a `DoD-N` box only when its own `verify by:` has been run and passed (not merely
because a closing Task is ticked). Log the command and its outcome as an Evidence bullet
under the Task that **Closes:** it. DONE requires every DoD box ticked.

- [ ] **DoD-1** — `cache`, `tree`, `ls`, `cat`, `rg`, `sed`, `head`, and `tail` parse `--branch BRANCH` and route that value to shared cache acquisition — *verify by:* `cargo test -p wit cli_branch_flag_parses_and_routes --lib`
- [ ] **DoD-2** — branch-selected CLI reads and cache refresh operate on branch-specific content across repo-reading commands — *verify by:* `cargo test -p wit --test branch_cli_integration`
- [ ] **DoD-3** — CLI help, README, bundled skill docs, and source-contract tests describe `--branch`, default branch behavior, `--refresh-cache`, and the continued absence of TTL/max-age controls — *verify by:* `cargo test -p wit cli_branch_help_text --lib && rg -n -- "--branch|branch parameter|default branch|TTL|max-age" README.md crates/wit/src/skill/SKILL.md crates/wit/src/lib.rs`
- [ ] **DoD-4** — full repo verification passes after the CLI branch flag lands — *verify by:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`

---

## 4. Exit Conditions

The goal terminates when **any** condition holds. On exit, state which fired —
explicitly — in the response to the user.

- **`DONE`** — all §3 items ticked and all §5 tasks ≥ confidence floor. *(primary)*
- **`BLOCKED-DEP`** — `goals/branch-cache-selection-api.md` is not done after one direct status check. Exit without starting CLI edits; name it explicitly.
- **`SCOPE-CHANGE`** — completing CLI branch selection requires changing the user-facing command set beyond `--branch BRANCH` on repo-scoped cache/read commands, or supporting non-branch refs. Record the proposal in §6 and exit to the user.
- **`CONFIDENCE-STALL`** — a task cannot reach the floor after two honest implementation and verification attempts. Exit, report the task and the gap.
- **`BUDGET`** — six focused implementation cycles or two full-suite attempts are reached without satisfying DoD-1 through DoD-4. Exit and report progress.

---

## 5. Tasks · INVARIANT

Ordered, dependency-aware units of work that together satisfy the DoD. Tick the
trailing `[ ]` only when the Verification Contract passes and Confidence ≥ floor.

---

### T1 · Inventory CLI command and test surfaces · [ ]

**Steps**
- [ ] Run `gdd_status.py` on `goals/branch-cache-selection-api.md` and stop if it is not done.
- [ ] Re-read CLI command variants, `repo_cache_mode`, hidden `__cache-revalidate`, cache call sites, help tests, and source-contract tests.
- [ ] Identify every stale assertion that says public branch selection is intentionally deferred.

**Verification Contract**
- *Check:* every CLI command that should accept `--branch` and every stale no-branch assertion is identified.
- *Method:* `rg -n "Commands::|refresh_cache|cache_github_repo|__cache-revalidate|long = \"branch\"|branch-selection|Public branch selection|No public TTL" crates/wit/src/cli.rs crates/wit/src/lib.rs README.md crates/wit/src/skill/SKILL.md`
- *Expected:* exit 0; execution notes identify target commands and stale assertions before code edits.
- *BDD scenarios covered:* repo-scoped command inventory; source-contract update inventory; hidden worker flag inventory

**Confidence:** 0 / 90 · **Depends on:** branch-cache-selection-api DONE · **Closes:** none

**Evidence (required before tick; append-only)**
- *(none yet — when setting Confidence ≥ floor, append a bullet with all three: date + command/check run + outcome (exit code / test counts / artifact path))*

---

### T2 · Add and route `--branch` through CLI command handling · [ ]

**Steps**
- [ ] Add a reusable branch option pattern to `cache`, `tree`, `ls`, `cat`, `rg`, `sed`, `head`, and `tail`.
- [ ] Route the parsed value to the shared cache branch selector without changing no-branch defaults.
- [ ] Update parser/source-contract tests so the old no-public-branch assertion is removed, not bypassed.

**Verification Contract**
- *Check:* CLI parser accepts `--branch BRANCH` on the full repo-scoped command set and code routes it to cache acquisition.
- *Method:* `cargo test -p wit cli_branch_flag_parses_and_routes --lib`
- *Expected:* exit 0; tests fail if any target command lacks `--branch` or routes reads without the branch value.
- *BDD scenarios covered:* `cache --branch`; `tree --branch`; `ls --branch`; `cat --branch`; `rg --branch`; `sed --branch`; `head --branch`; `tail --branch`; omitted branch uses default

**Confidence:** 0 / 90 · **Depends on:** T1 · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- *(none yet)*

---

### T3 · Prove CLI branch-specific reads with deterministic fixtures · [ ]

**Steps**
- [ ] Add a deterministic CLI integration test using local seeded branch caches or local-remotes rather than live GitHub where practical.
- [ ] Cover branch-specific content for tree, ls, cat, rg, sed, head, and tail.
- [ ] Cover `wit cache -r owner/repo --branch BRANCH` as a branch-specific force refresh path.

**Verification Contract**
- *Check:* branch-selected CLI commands return content unique to the named branch.
- *Method:* `cargo test -p wit --test branch_cli_integration`
- *Expected:* exit 0; tests fail if commands read default branch content while `--branch` is set.
- *BDD scenarios covered:* named branch file exists; default branch lacks that file; search result differs by branch; cache refresh targets named branch

**Confidence:** 0 / 90 · **Depends on:** T2 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- *(none yet)*

---

### T4 · Update CLI-facing docs and help text · [ ]

**Steps**
- [ ] Update top-level help and per-command help to describe `--branch BRANCH`.
- [ ] Update README cache and command sections so they no longer claim public branch selection is absent.
- [ ] Update bundled `crates/wit/src/skill/SKILL.md` with the same branch-selection contract.
- [ ] Preserve the no public TTL/max-age contract.

**Verification Contract**
- *Check:* docs and help describe branch selection accurately and do not reintroduce TTL or max-age.
- *Method:* `cargo test -p wit cli_branch_help_text --lib && rg -n -- "--branch|branch parameter|default branch|TTL|max-age" README.md crates/wit/src/skill/SKILL.md crates/wit/src/lib.rs`
- *Expected:* exit 0; grep output includes intentional branch/default/TTL wording in all doc surfaces.
- *BDD scenarios covered:* user wants default branch; user wants named branch; user wants fresh named branch; user asks for TTL

**Confidence:** 0 / 90 · **Depends on:** T3 · **Closes:** DoD-3

**Evidence (required before tick; append-only)**
- *(none yet)*

---

### T5 · Run full repo verification for CLI branch selection · [ ]

**Steps**
- [ ] Run rustfmt check after all CLI/docs changes.
- [ ] Run clippy with warnings denied.
- [ ] Run the full workspace test suite and search migration guard.
- [ ] Fix any failure before finalizing, even if it appears outside the immediate CLI diff.

**Verification Contract**
- *Check:* full repo gates pass with the CLI branch flag.
- *Method:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`
- *Expected:* exit 0 for every command.
- *BDD scenarios covered:* formatting; lint; unit and integration replay tests; GitHub-only search migration guard

**Confidence:** 0 / 90 · **Depends on:** T4 · **Closes:** DoD-4

**Evidence (required before tick; append-only)**
- *(none yet)*

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-02 — Adversarial review before delivery
- **Context:** User asked for an optional branch parameter for the CLI and MCP server, while current CLI tests and docs intentionally reject public branch selection.
- **Decision:** Make the CLI surface a single `--branch BRANCH` option on repo-scoped cache/read commands only, with no short alias or legacy alternative until the user requests one.
- **Alternatives rejected:** Add `--ref`; make branch global across `search` or `mcp` startup; keep docs saying branch selection is deferred; skip deterministic branch-read integration proof.
- **Why surface:** The highest-risk shortcut is parsing a flag without proving all read commands actually return branch-specific content.
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

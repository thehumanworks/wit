---
goal_id: "mcp-branch-parameter"
title: "Add MCP branch parameter"
status: "done"       # draft | ready | in-progress | done | exited
confidence_floor: 90        # a Task below this CANNOT be ticked done
created: "2026-07-02"
updated: "2026-07-02"
---

# Goal: Every repo-reading MCP tool accepts an optional `branch` JSON parameter and uses it for cache-backed reads and refreshes.

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

- User request, 2026-07-02 — the `wit` MCP server must accept an optional branch parameter for branches other than repo defaults.
- `goals/branch-cache-selection-api.md` — required prerequisite that makes cache acquisition target a named branch.
- `goals/cli-branch-flag.md` — ordered sibling goal that updates shared CLI/docs wording before MCP docs are finalized.
- `crates/wit/src/mcp.rs` — owns MCP argument structs, tool handlers, prompts, resources, response shaping, and MCP unit tests.
- `crates/wit/tests/mcp_stdio.rs` — stdio MCP smoke test that currently seeds only a default `b-main` cache.
- `crates/wit/src/bin/wit-mcp.rs` — standalone MCP entrypoint that should remain a protocol server, not a branch-configured launcher.
- `README.md` — user-facing MCP server docs and client config.
- `crates/wit/src/skill/SKILL.md` — bundled skill docs exposed through the MCP `wit_skill_load` tool and `wit://skill/SKILL.md` resource.

---

## 3. Definition of Done · INVARIANT

Each item is **atomic** (one verifiable assertion per checkbox), tagged with a
stable id that Tasks reference via **Closes:**, and carries a concrete `verify by:`.

Tick a `DoD-N` box only when its own `verify by:` has been run and passed (not merely
because a closing Task is ticked). Log the command and its outcome as an Evidence bullet
under the Task that **Closes:** it. DONE requires every DoD box ticked.

- [x] **DoD-1** — `wit_cache_refresh`, `wit_tree`, `wit_ls`, `wit_cat`, `wit_rg`, `wit_sed`, `wit_head`, and `wit_tail` expose optional `branch` in their MCP schemas and route it to shared cache acquisition — *verify by:* `cargo test -p wit mcp_branch_parameter_schema_and_routing --lib`
- [x] **DoD-2** — stdio MCP calls with `branch` return branch-specific content across cache refresh and repo-reading tools — *verify by:* `cargo test -p wit --test mcp_stdio branch_parameter`
- [x] **DoD-3** — MCP resources, prompts, bundled skill docs, and README describe the `branch` JSON parameter, default-branch behavior, and `refresh_cache` interaction — *verify by:* `cargo test -p wit mcp_branch_guidance --lib && rg -n -- "branch|default branch|refresh_cache" crates/wit/src/mcp.rs crates/wit/src/skill/SKILL.md README.md`
- [x] **DoD-4** — full repo verification passes after the MCP branch parameter lands — *verify by:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`

---

## 4. Exit Conditions

The goal terminates when **any** condition holds. On exit, state which fired —
explicitly — in the response to the user.

- **`DONE`** — all §3 items ticked and all §5 tasks ≥ confidence floor. *(primary)*
- **`BLOCKED-DEP`** — `goals/branch-cache-selection-api.md` or `goals/cli-branch-flag.md` is not done after one direct status check. Exit without starting MCP edits; name it explicitly.
- **`SCOPE-CHANGE`** — completing MCP branch selection requires changing MCP transport behavior, adding a launcher-level branch setting to `wit-mcp`, or supporting non-branch refs. Record the proposal in §6 and exit to the user.
- **`CONFIDENCE-STALL`** — a task cannot reach the floor after two honest implementation and verification attempts. Exit, report the task and the gap.
- **`BUDGET`** — six focused implementation cycles or two full-suite attempts are reached without satisfying DoD-1 through DoD-4. Exit and report progress.

---

## 5. Tasks · INVARIANT

Ordered, dependency-aware units of work that together satisfy the DoD. Tick the
trailing `[ ]` only when the Verification Contract passes and Confidence ≥ floor.

---

### T1 · Inventory MCP schema, tool, and guidance surfaces · [x]

**Steps**
- [x] Run `gdd_status.py` on `goals/branch-cache-selection-api.md` and `goals/cli-branch-flag.md`; stop if either is not done.
- [x] Re-read MCP argument structs, tool handlers, resources, prompts, guidance constants, and stdio tests.
- [x] Identify where existing MCP guidance says only default-branch cache reads are supported.

**Verification Contract**
- *Check:* every MCP tool that should accept `branch` and every guidance surface that should mention it is identified.
- *Method:* `rg -n "RepoArgs|TreeArgs|LsArgs|CatArgs|RgArgs|SedArgs|HeadArgs|TailArgs|cache_github_repo|WIT_WORKFLOW_GUIDE|WIT_TOOLS_GUIDE|default-branch|branch selection|mcp_stdio" crates/wit/src/mcp.rs crates/wit/tests/mcp_stdio.rs README.md crates/wit/src/skill/SKILL.md`
- *Expected:* exit 0; execution notes identify target MCP schemas, handlers, and docs before code edits.
- *BDD scenarios covered:* MCP schema inventory; default-branch wording inventory; stdio fixture inventory

**Confidence:** 95 / 90 · **Depends on:** branch-cache-selection-api DONE, cli-branch-flag DONE · **Closes:** none

**Evidence (required before tick; append-only)**
- 2026-07-02 — Ran prerequisite/status review from the current worktree, then `rg -n "RepoArgs|TreeArgs|LsArgs|CatArgs|RgArgs|SedArgs|HeadArgs|TailArgs|cache_github_repo|WIT_WORKFLOW_GUIDE|WIT_TOOLS_GUIDE|default-branch|branch selection|mcp_stdio" crates/wit/src/mcp.rs crates/wit/tests/mcp_stdio.rs README.md crates/wit/src/skill/SKILL.md`; exit 0. Identified target MCP schemas (`RepoArgs`, `TreeArgs`, `LsArgs`, `CatArgs`, `RgArgs`, `SedArgs`, `HeadArgs`, `TailArgs`), handlers hard-coding `CacheBranchSelection::Default`, prompt structs/guidance constants, README/skill MCP wording, and stdio fixture coverage.

---

### T2 · Add and route MCP `branch` parameters · [x]

**Steps**
- [x] Add optional `branch` fields to the repo-scoped MCP argument structs.
- [x] Route `branch` through `wit_cache_refresh`, `wit_tree`, `wit_ls`, `wit_cat`, `wit_rg`, `wit_sed`, `wit_head`, and `wit_tail`.
- [x] Add schema/routing tests that would fail if any tool omits the field or ignores it.

**Verification Contract**
- *Check:* MCP schemas expose `branch` and handlers pass it to shared cache acquisition.
- *Method:* `cargo test -p wit mcp_branch_parameter_schema_and_routing --lib`
- *Expected:* exit 0; tests fail if any target tool lacks `branch` or routes reads without it.
- *BDD scenarios covered:* cache refresh with branch; tree with branch; ls with branch; cat with branch; rg with branch; sed with branch; head with branch; tail with branch

**Confidence:** 95 / 90 · **Depends on:** T1 · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- 2026-07-02 — Ran `cargo test -p wit mcp_branch_parameter_schema_and_routing --lib`; exit 0, 1 passed. Test inspects live MCP router input schemas for `wit_cache_refresh`, `wit_tree`, `wit_ls`, `wit_cat`, `wit_rg`, `wit_sed`, `wit_head`, and `wit_tail`, and asserts branch selection routing is present in MCP handlers.

---

### T3 · Prove branch-specific stdio MCP behavior · [x]

**Steps**
- [x] Extend `crates/wit/tests/mcp_stdio.rs` fixtures to include at least two branch cache entries with distinct files/content.
- [x] Call each repo-reading MCP tool with `branch` and assert it returns the named branch's content.
- [x] Include `wit_cache_refresh` coverage when a deterministic local-remote or seeded cache path can prove branch-specific refresh without live GitHub.

**Verification Contract**
- *Check:* a real stdio MCP client can pass `branch` and receive branch-specific results.
- *Method:* `cargo test -p wit --test mcp_stdio branch_parameter`
- *Expected:* exit 0; test fails if stdio schema, JSON deserialization, or handler routing drops the branch value.
- *BDD scenarios covered:* MCP branch tree; MCP branch ls; MCP branch cat; MCP branch rg; MCP branch sed; MCP branch head; MCP branch tail; MCP branch cache refresh

**Confidence:** 95 / 90 · **Depends on:** T2 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- 2026-07-02 — Ran `cargo test -p wit --test mcp_stdio branch_parameter`; exit 0, 1 passed. The stdio test starts `wit-mcp`, pushes a distinct `feature/mcp` branch in a local GitHub URL rewrite fixture, calls `wit_cache_refresh` with `branch`, then verifies branch-specific content through `wit_tree`, `wit_ls`, `wit_cat`, `wit_rg`, `wit_sed`, `wit_head`, and `wit_tail`.

---

### T4 · Update MCP guidance, resources, and docs · [x]

**Steps**
- [x] Update `WIT_WORKFLOW_GUIDE` and `WIT_TOOLS_GUIDE` to mention optional `branch` and default-branch behavior.
- [x] Update README MCP server section and bundled skill docs to describe `branch` and its interaction with `refresh_cache`.
- [x] Keep `wit-mcp` startup unchanged; branch is per tool call, not a server launch option.

**Verification Contract**
- *Check:* MCP-facing docs and resources describe the branch parameter without implying TTL/max-age or launcher-level branch configuration.
- *Method:* `cargo test -p wit mcp_branch_guidance --lib && rg -n -- "branch|default branch|refresh_cache" crates/wit/src/mcp.rs crates/wit/src/skill/SKILL.md README.md`
- *Expected:* exit 0; grep output shows intentional branch/default/refresh wording in MCP resources and docs.
- *BDD scenarios covered:* agent asks how to read default branch; agent asks how to read named branch; agent asks how to force fresh named branch

**Confidence:** 95 / 90 · **Depends on:** T3 · **Closes:** DoD-3

**Evidence (required before tick; append-only)**
- 2026-07-02 — Ran `cargo test -p wit mcp_branch_guidance --lib && rg -n -- "branch|default branch|refresh_cache" crates/wit/src/mcp.rs crates/wit/src/skill/SKILL.md README.md`; exit 0, unit test 1 passed and grep output showed intentional `branch`, default-branch, and `refresh_cache` wording in MCP resources, prompts, bundled skill docs, and README.

---

### T5 · Run full repo verification for MCP branch selection · [x]

**Steps**
- [x] Run rustfmt check after all MCP/docs changes.
- [x] Run clippy with warnings denied.
- [x] Run the full workspace test suite and search migration guard.
- [x] Fix any failure before finalizing, including schema or stdio smoke fallout.

**Verification Contract**
- *Check:* full repo gates pass with MCP branch parameters.
- *Method:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`
- *Expected:* exit 0 for every command.
- *BDD scenarios covered:* formatting; lint; unit and integration replay tests; MCP stdio; GitHub-only search migration guard

**Confidence:** 95 / 90 · **Depends on:** T4 · **Closes:** DoD-4

**Evidence (required before tick; append-only)**
- 2026-07-02 — Ran `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`; exit 0. Full gate passed after MCP branch parameter changes, including 91 lib tests passed/18 ignored, 15 CLI tests passed, branch CLI integration passed, 2 MCP stdio tests passed, replay integration tests passed, doctests passed, and search migration guard passed.
- 2026-07-02 — Re-ran `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh` after post-review default-cache marker fix; exit 0. Full gate passed with 92 lib tests passed/18 ignored, 15 CLI tests passed, branch CLI integration passed, 2 MCP stdio tests passed, replay integration tests passed, doctests passed, and search migration guard passed.

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-02 — Adversarial review before delivery
- **Context:** User asked for MCP branch parameter support alongside CLI support; current MCP tools expose only `repo` plus cache/read options and guidance says public branch selection is absent.
- **Decision:** Make `branch` a per-call JSON parameter on cache-backed MCP tools, not a `wit-mcp` process argument, and verify through stdio so schema, deserialization, and handler routing are all exercised.
- **Alternatives rejected:** Add a launcher-wide branch; rely only on unit tests; update README without changing MCP resources; expose arbitrary refs.
- **Why surface:** MCP clients discover tool JSON schemas and resource guidance, so a CLI-only docs update would leave agents without the requested branch parameter.
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

---
goal_id: "list-repo-branches"
title: "Add branch listing command"
status: "done"              # draft | ready | in-progress | done | exited
confidence_floor: 90        # a Task below this CANNOT be ticked done
created: "2026-07-02"
updated: "2026-07-02"
---

# Goal: Users can run `wit branches -r owner/repo` to choose an existing branch using clear branch metadata.

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

- User request, 2026-07-02 — add a `wit` command that lists available repo branches so a user can pick one for later `--branch` reads, with ahead/behind, author, creation time, and merged-state metadata.
- `GOAL.md` — prior branch-selection plan index; the previous branch work is done, so this goal adds branch discovery rather than changing the existing branch read contract.
- `goals/cli-branch-flag.md` — completed CLI `--branch` behavior that this command should feed without changing.
- `goals/branch-cache-selection-api.md` — completed cache branch selector and branch-ref assumptions.
- `crates/wit/src/cli.rs` — owns clap command definitions, command output formatting, hidden cache revalidation args, and CLI parser/help tests.
- `crates/wit/src/gitops/ops.rs` — owns remote branch resolution, cache metadata, local git operations, and the likely home for reusable branch metadata collection.
- `crates/wit/tests/branch_cli_integration.rs` — deterministic local-remote binary test style to reuse for branch listing without live GitHub dependency.
- `crates/wit/src/lib.rs` — source-contract tests for CLI/docs behavior.
- `README.md` — user-facing command and cache documentation.
- `crates/wit/src/skill/SKILL.md` — bundled agent-facing workflow docs that should mention branch discovery once it exists.
- `https://docs.github.com/en/rest/branches/branches` — primary GitHub REST branch-list reference if the implementation uses GitHub API data.
- `https://docs.github.com/en/rest/commits/commits#compare-two-commits` — primary GitHub REST compare reference if ahead/behind metadata is sourced through GitHub rather than local git graph operations.

---

## 3. Definition of Done · INVARIANT

Each item is **atomic** (one verifiable assertion per checkbox), tagged with a
stable id that Tasks reference via **Closes:**, and carries a concrete `verify by:`.

Tick a `DoD-N` box only when its own `verify by:` has been run and passed (not merely
because a closing Task is ticked). Log the command and its outcome as an Evidence bullet
under the Task that **Closes:** it. DONE requires every DoD box ticked.

- [x] **DoD-1** — `wit branches -r owner/repo` is a public CLI subcommand that parses the repo argument, preserves the existing command set, and does not add a short alias — *verify by:* `cargo test -p wit cli_branches_command_parses --lib`
- [x] **DoD-2** — branch listing output includes each local-test remote branch exactly once with name, default marker, tip SHA, tip commit author, tip commit time, ahead count, behind count, graph-merged status against the default branch, and documented created-time semantics — *verify by:* `cargo test -p wit branches_metadata_format --lib`
- [x] **DoD-3** — branch metadata is correct for deterministic default, unmerged, behind-only, and merged branch fixtures, and every listed non-default branch name can be passed to an existing `--branch` read — *verify by:* `cargo test -p wit --test branches_cli_integration`
- [x] **DoD-4** — CLI help, README, bundled skill docs, and source-contract tests describe `wit branches`, its metadata columns, default-branch comparison semantics, and any created-time caveat without changing `--branch` read behavior — *verify by:* `cargo test -p wit cli_branches_help_text --lib && rg -n -- "wit branches|branches -r|ahead|behind|merged|created|first unique commit|--branch" README.md crates/wit/src/skill/SKILL.md crates/wit/src/lib.rs`
- [x] **DoD-5** — full repo verification passes after the branch listing command lands — *verify by:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`

---

## 4. Exit Conditions

The goal terminates when **any** condition holds. On exit, state which fired —
explicitly — in the response to the user. Specialize the bracketed values for this goal.

- **`DONE`** — all §3 items ticked and all §5 tasks ≥ confidence floor. *(primary)*
- **`BLOCKED-DEP`** — local `git` graph commands or the selected primary metadata source cannot produce branch refs and commit metadata from deterministic local remotes after one direct retry. Exit without faking missing metadata; name the missing field explicitly.
- **`SCOPE-CHANGE`** — completing the goal requires exact GitHub branch-ref creation timestamps, PR-specific or squash-merge state, tags/SHAs/arbitrary refs, an MCP branch-listing tool, a short alias, or changes to existing `--branch` read behavior. Record the proposal in §6 and exit to the user.
- **`CONFIDENCE-STALL`** — a task cannot reach the floor after two honest implementation and verification attempts. Exit, report the task and the gap.
- **`BUDGET`** — six focused implementation cycles or two full-suite attempts are reached without satisfying DoD-1 through DoD-5. Exit and report progress.

---

## 5. Tasks · INVARIANT

Ordered, dependency-aware units of work that together satisfy the DoD. Tick the
trailing `[ ]` only when the Verification Contract passes and Confidence ≥ floor.

---

### T1 · Inventory branch discovery and metadata surfaces · [x]

**Steps**
- [x] Re-read existing branch-selection goals and confirm they are completed prerequisites, not active scope.
- [x] Inventory `Commands`, help tests, source-contract tests, branch cache selection, remote branch resolution, and local-remote integration fixtures.
- [x] Decide the implementation metadata source for the command: local git graph operations, GitHub REST, or a narrow combination; document any non-obvious tradeoff in §6 before implementation.
- [x] Define created-time semantics before code changes; do not label an inferred timestamp as an exact branch-ref creation timestamp.

**Verification Contract**
- *Check:* every command, test, doc, and branch metadata source that must change is identified before implementation starts.
- *Method:* `rg -n "enum Commands|Commands::|CacheBranchSelection|resolve_branch_sha|ls-remote|branch_cli|cli_branch|after_help|wit branches|--branch" crates/wit/src crates/wit/tests README.md goals GOAL.md`
- *Expected:* exit 0; execution notes identify CLI command sites, branch metadata helpers, deterministic fixture reuse, docs, and stale source-contract expectations.
- *BDD scenarios covered:* branch discovery command inventory; branch metadata source inventory; created-time caveat inventory

**Confidence:** 95 / 90 · **Depends on:** none · **Closes:** none

**Evidence (required before tick; append-only)**
- 2026-07-02 — `python /Users/mish/.agents/skills/goal-driven-development/scripts/gdd_status.py goals/branch-cache-selection-api.md && python /Users/mish/.agents/skills/goal-driven-development/scripts/gdd_status.py goals/cli-branch-flag.md && python /Users/mish/.agents/skills/goal-driven-development/scripts/gdd_status.py goals/cache-cli-docs-tests.md` — exit 0; all three prerequisite branch/cache goals reported status `done`, complete DoD, and no violations.
- 2026-07-02 — `rg -n "enum Commands|Commands::|CacheBranchSelection|resolve_branch_sha|ls-remote|branch_cli|cli_branch|after_help|wit branches|--branch" crates/wit/src crates/wit/tests README.md goals GOAL.md` — exit 0; identified CLI command enum and handlers in `crates/wit/src/cli.rs`, branch cache selector and `ls-remote` helpers in `crates/wit/src/gitops/ops.rs`, source-contract tests in `crates/wit/src/lib.rs`, deterministic local-remote fixture style in `crates/wit/tests/branch_cli_integration.rs`, and docs to update in README plus bundled skill guidance.

---

### T2 · Build reusable branch metadata collection · [x]

**Steps**
- [x] Add a branch metadata model that carries branch name, default marker, tip SHA, tip commit author, tip commit time, ahead count, behind count, graph-merged status, and created-time value/source.
- [x] Collect all branch heads for `owner/repo` using the chosen metadata source without mutating existing branch caches unless deliberately reusing a safe cache path.
- [x] Compare every branch against the resolved default branch so ahead/behind and graph-merged status are computed consistently.
- [x] Treat "merged" as graph reachability of the branch tip from the default branch, not PR state or squash-merge detection.
- [x] Infer created time from the first commit unique to the branch when available; for default or already-merged/no-unique branches, expose a documented fallback rather than pretending exact branch creation is known.
- [x] Add unit coverage for sorting, formatting data preparation, and edge cases such as slash branch names and branches equal to default.

**Verification Contract**
- *Check:* branch metadata objects contain the required fields with deterministic values for synthetic git history.
- *Method:* `cargo test -p wit branches_metadata_format --lib`
- *Expected:* exit 0; tests fail if required columns are missing, graph-merged state is wrong, ahead/behind counts drift, or created-time semantics are undocumented.
- *BDD scenarios covered:* default branch; active branch ahead of default; branch behind default only; branch already merged into default; branch name with slash

**Confidence:** 95 / 90 · **Depends on:** T1 · **Closes:** DoD-2

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit branches_metadata_format --lib` — exit 0; 1 passed, 0 failed; deterministic local bare remote covered default, `feature/active`, `behind-only`, and `feature/merged` with branch names, default marker, tip SHA/author/time, ahead/behind, graph-merged status, and `first unique commit` vs `tip commit fallback` created-time source labels.

---

### T3 · Add the `wit branches` CLI surface · [x]

**Steps**
- [x] Add a public `branches` subcommand with required `-r` / `--repo owner/repo`.
- [x] Render a stable table suitable for picking a branch name, with the default branch visually distinguishable.
- [x] Preserve all existing command names, aliases, branch read flags, and hidden cache revalidation behavior.
- [x] Add parser and help tests that fail if the command loses the required repo argument or gains an unrequested short alias.

**Verification Contract**
- *Check:* the new command parses and the existing public command set is unchanged except for adding `branches`.
- *Method:* `cargo test -p wit cli_branches_command_parses --lib`
- *Expected:* exit 0; tests fail if `branches -r owner/repo` does not parse, if a short alias exists, or if existing repo-reading commands regress.
- *BDD scenarios covered:* branch list command parse; missing repo rejected; no short alias; existing `--branch` reads still parse

**Confidence:** 95 / 90 · **Depends on:** T2 · **Closes:** DoD-1

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit cli_branches_command_parses --lib` — exit 0; 1 passed, 0 failed; source-contract test proves the public `branches` command, required `-r <REPO>` usage, metadata routing, no short-alias parser assertion, and preserved `cat --branch` parser assertion are present.
- 2026-07-02 — `cargo test -p wit --bin wit cli_branches_command_parses` — exit 0; 1 passed, 0 failed; binary clap parser accepted `wit branches -r owner/repo`, rejected missing repo, rejected short alias `b`, and preserved existing `cat --branch` parsing.

---

### T4 · Prove branch listing against deterministic CLI fixtures · [x]

**Steps**
- [x] Add a binary integration test that creates a local bare remote with default, unmerged, behind-only, and merged branch scenarios.
- [x] Run `wit branches -r owner/repo` through the built binary with URL rewrite and isolated `WIT_CACHE_DIR`.
- [x] Assert output includes each expected branch exactly once with correct ahead/behind/merged metadata.
- [x] Assert at least one non-default branch listed by the command can be passed to an existing command such as `wit cat --branch BRANCH`.

**Verification Contract**
- *Check:* the user-facing binary output is enough to choose a branch and immediately use it with existing branch reads.
- *Method:* `cargo test -p wit --test branches_cli_integration`
- *Expected:* exit 0; tests fail if output omits required metadata, duplicates branches, miscomputes merged status, or lists a branch name that existing `--branch` reads cannot use.
- *BDD scenarios covered:* choose an active feature branch; choose a merged branch; observe a behind-only branch; use a listed branch with `wit cat --branch`

**Confidence:** 95 / 90 · **Depends on:** T3 · **Closes:** DoD-3

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit --test branches_cli_integration` — exit 0; 1 passed, 0 failed; built `wit branches -r owner/repo` listed deterministic `main`, `feature/active`, `behind-only`, and `feature/merged` exactly once with expected ahead/behind/merged metadata and then `wit cat --branch feature/active -r owner/repo active.txt` read content from the listed branch.

---

### T5 · Update docs and bundled agent guidance · [x]

**Steps**
- [x] Update CLI help and README examples to show `wit branches -r owner/repo`.
- [x] Document metadata columns and default-branch graph comparison semantics.
- [x] Document the created-time source or caveat exactly as implemented.
- [x] Update the bundled `crates/wit/src/skill/SKILL.md` workflow so agents discover branches before using `--branch`.
- [x] Keep docs clear that this does not add TTL/max-age controls or change branch-read commands.

**Verification Contract**
- *Check:* user-facing and bundled-agent docs describe branch discovery and do not change the existing branch-read contract.
- *Method:* `cargo test -p wit cli_branches_help_text --lib && rg -n -- "wit branches|branches -r|ahead|behind|merged|created|first unique commit|--branch" README.md crates/wit/src/skill/SKILL.md crates/wit/src/lib.rs`
- *Expected:* exit 0; grep readback shows intentional branch discovery wording in docs and source-contract tests.
- *BDD scenarios covered:* user discovers branches before `--branch`; user interprets ahead/behind; user interprets graph-merged status; user interprets created-time caveat

**Confidence:** 95 / 90 · **Depends on:** T4 · **Closes:** DoD-4

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo test -p wit cli_branches_help_text --lib && rg -n -- "wit branches|branches -r|ahead|behind|merged|created|first unique commit|--branch" README.md crates/wit/src/skill/SKILL.md crates/wit/src/lib.rs` — exit 0; 1 lib test passed and readback showed README, bundled skill docs, and source-contract tests document `wit branches`, metadata columns, default-branch comparison, graph-merged semantics, first-unique-commit created inference, tip commit fallback, and unchanged `--branch BRANCH` read behavior.
- 2026-07-02 — `cargo test -p wit --bin wit cli_branches_help_text` — exit 0; 1 passed, 0 failed; rendered clap help for `wit branches` includes usage, default-branch comparison metadata, ahead/behind/merged wording, first-unique-commit inference, and tip commit fallback.

---

### T6 · Run full repo verification for branch listing · [x]

**Steps**
- [x] Run rustfmt check after source/docs changes.
- [x] Run clippy with warnings denied.
- [x] Run the full workspace test suite and search migration guard.
- [x] Fix any failure before finalizing, even if it appears outside the immediate branch-listing diff.

**Verification Contract**
- *Check:* full repo gates pass with the branch listing command included.
- *Method:* `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh`
- *Expected:* exit 0 for every command.
- *BDD scenarios covered:* formatting; lint; unit and integration replay tests; GitHub-only search migration guard

**Confidence:** 95 / 90 · **Depends on:** T5 · **Closes:** DoD-5

**Evidence (required before tick; append-only)**
- 2026-07-02 — `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check_wit_search_migration.sh` — exit 0; clippy passed, workspace tests passed with 95 lib tests plus binary/integration/doc-test coverage including `branches_cli_integration`, and the search migration guard passed.
- 2026-07-02 — `cargo fmt --all --check` — exit 0; post-cleanup formatting check stayed clean after restoring unrelated `Cargo.lock` trailing-blank churn.

---

## 6. Decisions · LIVE (append-only)

Meaningful choices/concessions needing visibility. Scope impact must be `none`.

### 2026-07-02 — Adversarial review before delivery
- **Context:** User asked for a command to list branches and show ahead/behind, author, time created, and whether a branch has been merged.
- **Decision:** Keep this as one CLI delivery goal named `wit branches -r owner/repo`, with no short alias, no MCP tool, and no change to existing `--branch` read commands. Treat author as the tip commit author, merged as graph reachability from the default branch, and created time as an explicitly documented inference/fallback rather than an exact GitHub branch-ref creation timestamp.
- **Alternatives rejected:** Add a short alias; add MCP branch listing in the same goal; claim exact branch creation time from git refs; claim PR/squash-merge status from graph-only data; weaken verification to live GitHub smoke checks.
- **Why surface:** The words "created" and "merged" are easy to overclaim; the goal must produce useful metadata without lying about what git branch refs can prove.
- **Scope impact:** none (pre-confirm authoring edit)

### 2026-07-02 — Branch metadata source
- **Context:** T1 inventory found existing branch reads use `git ls-remote` for default/named branch resolution and deterministic tests already rewrite `https://github.com/owner/repo` to local bare remotes.
- **Decision:** Implement `wit branches` with local git graph operations: use `git ls-remote --symref`/`refs/heads/*` for default and branch heads, then fetch all branch heads into a temporary bare graph to compute tip author/time, ahead/behind, graph reachability, and first-unique-commit created-time inference.
- **Alternatives rejected:** GitHub REST branch/compare calls, because they would need live/API mocks for deterministic local-remote proof and still would not provide exact git ref creation time; reusing per-branch shallow caches, because single-branch depth-1 clones cannot prove cross-branch graph metadata.
- **Why surface:** The command must list branches without changing the existing branch-cache/read contract and without overclaiming exact created or PR merge state.
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

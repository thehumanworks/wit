# Branch Parameter Plan

Status: done on 2026-07-02. Branch-aware cache selection, CLI flags, MCP parameters, docs, and tests are implemented and verified.

## Completed Prerequisites

1. `goals/branch-cache-metadata.md` — done. Established repo+branch cache identity, default branch resolution, and per-branch metadata.
2. `goals/stale-while-revalidate-cache.md` — done. Cached branch content is served first, SHA mismatch revalidates in the background, force invalidation refreshes before return, and locks are branch-scoped.
3. `goals/cache-cli-docs-tests.md` — done. Wired force invalidation through CLI/docs/tests and ran the full verification matrix.

## Delivery Order

1. `goals/branch-cache-selection-api.md` — done. Shared cache acquisition targets either the default branch or a named branch without mixing branch caches.
2. `goals/cli-branch-flag.md` — done. CLI repo-scoped cache/read commands accept `--branch BRANCH`.
3. `goals/mcp-branch-parameter.md` — done. MCP cache/read tools accept optional `branch` JSON parameters.

## Kickstart

Original execution entrypoint:

```bash
python /Users/mish/.agents/skills/goal-driven-development/scripts/gdd_status.py goals/branch-cache-selection-api.md
```

Final state: all delivery goals are done; no active next task remains.

## Scope Notes

- Branch selection means GitHub branch names under `refs/heads`, not tags, commit SHAs, pull request refs, or arbitrary git refs.
- No branch parameter keeps the existing default-branch behavior.
- `--refresh-cache` / `refresh_cache` should refresh the selected branch when a branch is supplied.
- No public cache TTL or max-age option.
- Do not edit DoD or Tasks during execution unless a `SCOPE-CHANGE` exit is surfaced and approved.

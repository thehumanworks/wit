# Cache Freshness Plan

Status: done on 2026-07-01.

## Delivery Order

1. `goals/branch-cache-metadata.md` — done. Established repo+branch cache identity, default branch resolution, and per-branch metadata.
2. `goals/stale-while-revalidate-cache.md` — done. Cached branch content is served first, SHA mismatch revalidates in the background, force invalidation refreshes before return, and locks are branch-scoped.
3. `goals/cache-cli-docs-tests.md` — done. Wired force invalidation through CLI/docs/tests and ran the full verification matrix.

## Kickstart

All delivery goals are done. Final verification completed:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/check_wit_search_migration.sh
```

## Scope Notes

- No public cache TTL or max-age option.
- No public branch-reading flag in this plan; cache internals become branch-addressable now so that feature can be added later without a cache migration.
- Do not edit DoD or Tasks during execution unless a `SCOPE-CHANGE` exit is surfaced and approved.

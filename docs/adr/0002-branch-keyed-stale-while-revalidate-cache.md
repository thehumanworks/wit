# ADR 0002: Branch-Keyed Stale-While-Revalidate Repo Cache

- Status: Accepted
- Date: 2026-07-02

## Context

`wit` caches GitHub repositories as shallow bare clones so repo-reading commands can inspect code without cloning into the user's workspace.

The previous repo-only cache identity was too coarse for the next branch-reading features: one `owner/repo` cache path could not safely represent multiple branches, and a valid cache could be reused indefinitely without a durable record of the branch or commit it represented.

We also rejected a public cache age policy. A `--max-age` or TTL would make freshness depend on elapsed time rather than the remote branch's actual state, and it would force users to tune cache policy instead of expressing whether a specific read needs fresh content.

## Decision

Use a branch-keyed stale-while-revalidate cache for repo-scoped reads.

Implementation decisions:

1. Store each cache entry by repository and branch:
   - `$WIT_CACHE_DIR/<owner>/<repo>/branches/<encoded-branch>/repo.git`
   - `<encoded-branch>` is a filesystem-safe branch key with a `b-` prefix.
2. Store schema-versioned `metadata.json` beside each branch cache with:
   - `owner_repo`
   - `remote_url`
   - `branch`
   - `current_sha`
   - `last_checked_at`
   - `last_updated_at`
   - optional `last_error`
3. Resolve the remote default branch when no branch is explicitly requested. Existing valid default-branch metadata may be used for warm reads without first probing the remote.
4. Make ordinary repo-reading commands use `CacheAcquisitionMode::ServeStaleAndRevalidate`:
   - return a valid local branch cache immediately
   - launch quiet background revalidation
   - compare remote branch SHA to `metadata.current_sha`
   - refresh the branch cache only when the SHA changed
   - keep the old cache usable and record `last_error` when revalidation fails
5. Make explicit refresh paths use `CacheAcquisitionMode::ForceInvalidate`:
   - `wit cache -r owner/repo`
   - `--refresh-cache` on repo-reading commands
   - these paths refresh the branch cache before returning content
6. Scope cache locks to the repo+branch cache key, with the lock file beside the branch metadata:
   - `$WIT_CACHE_DIR/<owner>/<repo>/branches/<encoded-branch>/.cache.lock`
7. Do not expose public branch selection in this decision. The cache layout is branch-addressable now so branch-reading can be added later without another cache migration.
8. Do not expose a public TTL or max-age option.

## Consequences

Positive:

- Warm repo reads stay responsive because valid cached content is served before network revalidation completes.
- Freshness is based on remote branch SHA, not cache age.
- Users can still demand freshness for a specific read with `--refresh-cache`.
- Repo+branch locks prevent same-key mutation races without serializing unrelated repositories or branches.
- Branch metadata gives future branch-reading work a durable cache identity to build on.
- Revalidation failures do not destroy an otherwise usable cache.

Tradeoffs:

- A normal cached read can return stale content once, until background revalidation completes.
- Cache layout changed from repo-only paths to branch-keyed paths; legacy repo-only cache directories must be cleaned or ignored by the new path contract.
- Background revalidation requires a hidden worker path and test coverage that separates foreground stale reads from refresh completion.
- Public branch reads still need a follow-up CLI/API design.

## Alternatives Considered

1. Public TTL or `--max-age` option:
   - Rejected because freshness should follow branch SHA changes, not elapsed time or user-tuned cache age.
2. Always refresh before every repo read:
   - Rejected because it removes the main benefit of local caching and makes common reads network-bound.
3. Keep one repo-only cache path:
   - Rejected because it cannot safely support multiple branch identities or branch-specific metadata.
4. Always reclone during revalidation:
   - Rejected because equal remote SHA should update `last_checked_at` without replacing a valid cache.
5. Delete the current cache before proving the replacement clone works:
   - Rejected because a network or fetch failure should not destroy usable cached content.
6. Expose public branch selection with the cache refactor:
   - Rejected to keep this change focused on cache identity and freshness semantics; branch reads remain a separate feature.

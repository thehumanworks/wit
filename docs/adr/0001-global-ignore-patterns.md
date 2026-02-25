# ADR 0001: Global `--ignore` Across Commands

- Status: Accepted
- Date: 2026-02-24

## Context

`wit` traverses repository trees for commands like `tree`, `ls`, and `rg`, and reads explicit files for `cat`, `head`, `tail`, and `sed`.

Users reported cases where accidentally committed binary files or dot-directories add noise and slow down search/exploration. Before this change, there was no shared way to exclude paths.

## Decision

Add a repeatable global CLI flag:

- `--ignore <PATH|GLOB>`

This flag is available on all subcommands and supports:

- file paths (for example `src/generated.rs`)
- directory paths (for example `vendor` or `src/generated`)
- glob patterns (for example `*.png`, `**/fixtures/**`)

Implementation decisions:

1. Centralize pattern handling in `gitops::ops::IgnoreMatcher`.
2. Apply ignore checks at traversal/read boundaries so every repo-reading command uses identical filtering semantics.
3. Keep filtering explicit and additive only (no negated include/unignore rules) to avoid ambiguity.
4. Keep `search` compatibility: it accepts `--ignore`, and applies filtering when snippet paths are available (`--with-snippets`).

## Consequences

Positive:

- Consistent path exclusion behavior across commands.
- Reduced noisy output from accidental binary/dot directories.
- Safer explicit reads (`cat`, `head`, `tail`, `sed`) by rejecting ignored paths.

Tradeoffs:

- `search` cannot fully re-compute upstream grep.app facet counts when snippets are disabled; ignore filtering is therefore snippet-aware.
- Additional pattern compilation step per command invocation.

## Alternatives Considered

1. Per-command ignore implementation:
   - Rejected due to duplicated matching logic and likely drift in semantics.
2. `rg`-only ignore support:
   - Rejected because noise affects traversal/read commands too.
3. Rely only on existing `-g/--glob`:
   - Rejected because it is command-specific and include-oriented, not a shared exclusion contract.

# Direct MCP foundation status

This document reconciles the open foundation issues for the direct MCP server with the behavior present on `main`. It records implementation evidence and remaining work without changing the direct request or result contracts.

## Issue #7: immutable snapshots and provenance

The foundation is implemented. `wit_open` pins a default branch, named branch, tag, or full commit SHA for the server lifetime. Downstream list, search, read, and context results carry the snapshot and commit identity plus path, blob, and applicable line provenance. Integration coverage moves a branch after opening a snapshot and verifies that replay remains byte-identical while a newly refreshed snapshot observes the new commit.

Pull-request head refs remain explicitly unsupported by direct ref resolution. The `wit_open` capability response reports that limitation, and clients can open the head's full commit SHA instead. This satisfies issue #7's accepted follow-up representation; direct pull-request-head convenience should be tracked separately if it becomes required.

Disposition: the snapshot/provenance contract is delivered; close #7 as implemented or narrow it to direct pull-request-head resolution.

## Issue #8: whole-response budgets and cursors

The functional contract is implemented. Collection responses enforce a whole-structured-response byte budget, return deterministic cursors when more results exist, reject cursors replayed with changed normalized arguments or snapshot identity, and avoid rendered-text duplication unless requested. Integration and unit tests cover list, search, read, and refs pagination, cursor replay and mismatch failures, low-budget rejection, UTF-8 content, atomic search context, and high-match bounded output.

The high-match fixture verifies bounded returned items and serialized bytes, but it does not measure peak process memory. The proposed p50/p95 latency, response-size, memory, token, and cursor-rate metrics are also not maintained as a benchmark series.

Disposition: the runtime contract is delivered; update #8 to move peak-memory measurement and longitudinal performance metrics into benchmark follow-up work, then close the functional issue.

## Issue #9: agent-native semantic tools

The direct server exposes exactly eight tools: `wit_find_repositories`, `wit_refs`, `wit_open`, `wit_list`, `wit_search_code`, `wit_read`, `wit_context`, and `wit_ast` (tree-sitter structural search, added by ADR 0008). The default workflow is snapshot-first, structured, bounded, deterministic, and provenance-bearing. The human CLI remains separate and unchanged.

The earlier acceptance language for retaining and benchmarking the removed Unix-shaped server is superseded by the decision to make the semantic server the only direct surface. Tests that required the removed server or its deleted benchmark fixture are therefore not valid current acceptance gates.

Disposition: close #9 as implemented with the compatibility and comparative legacy-benchmark criteria explicitly superseded by the removal decision.

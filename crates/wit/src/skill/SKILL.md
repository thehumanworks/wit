---
name: wit
description: >
  Explore GitHub repositories without cloning using the `wit` CLI. Use when you
  need to discover repositories, browse file trees, read files, search code, or
  extract content from GitHub repos. Supports: finding repos with `wit search`,
  viewing repo structure with `wit tree`/`wit ls`, reading files with `wit cat`/
  `wit head`/`wit tail`/`wit sed`, and searching code with `wit rg`. Install
  with `npm install -g @nothumanwork/wit`.
---

# wit — GitHub Repository Explorer

`wit` caches GitHub repos as shallow bare clones under `$TMPDIR/.wit/cache` (override with `WIT_CACHE_DIR`) and exposes Unix-style commands to explore them without a full clone.

## Typical Workflow

1. Discover repos: `wit search`
2. Orient yourself: `wit tree` or `wit ls`
3. Find code: `wit rg`
4. Read files: `wit cat`, `wit head`, `wit tail`, `wit sed`

All repo-scoped commands take `-r/--repo <owner/repo>` as a required flag.
When the user asks for a non-default branch, run `wit branches -r owner/repo` first to list available branch names and metadata before choosing `--branch BRANCH`.

## MCP Server

When an MCP client is available, use `wit mcp --transport stdio` as the stdio server instead of shelling out. The standalone `wit-mcp` binary exposes the same surface. Both default to direct mode; use `wit mcp --transport stdio --mode code` or `wit-mcp --mode code` only to opt into experimental Code Mode.

Choose direct mode for a simple open, list, search, read, or other one-operation task. It is the default and current recommendation. Choose Code Mode only when bounded JavaScript composition can gather, filter, or compare several pieces of evidence before returning one focused result. Code Mode exposes one normal MCP `code` tool and is not required by MCP.

Use the snapshot-first workflow:

1. Call `wit_find_repositories` only when `owner/repo` is unknown.
2. Call `wit_refs` if branch or tag discovery matters.
3. Call `wit_open`, then reuse its immutable `snapshot_id` for every read in the task. Use `freshness: "require_fresh"` only when a branch must be refreshed before pinning.
4. Call `wit_list` for bounded structural orientation, `wit_search_code` for one or more known regex queries, and `wit_read` for explicit one-based inclusive line ranges.
5. Call `wit_context` when one deterministic operation should rank and merge bounded evidence across files. It does not call an internal model or embeddings.
6. When `has_more` is true, pass `next_cursor` back with otherwise unchanged arguments. Cursors are bound to the tool, snapshot, and normalized query.

Evidence items carry repository, commit SHA, path, blob identity, and line provenance. Collection responses are structured and use a 64 KiB whole-response budget. Fetch `wit://skill/SKILL.md`, `wit://guide/workflow`, or `wit://guide/tools` for reusable guidance.

### Experimental Code Mode workflow

The `code` input is an async JavaScript function body. Use `await` and the generated `codemode.wit.findRepositories`, `codemode.wit.refs`, `codemode.wit.open`, `codemode.wit.list`, `codemode.wit.searchCode`, `codemode.wit.read`, and `codemode.wit.context` methods; no TypeScript syntax or module imports are available. Open once, reuse the parent-server-lifetime `snapshot_id`, and follow explicit `next_cursor` values with otherwise unchanged arguments. Return one focused JSON-serializable value, retaining `repo`, `commit_sha`, `snapshot_id`, `path`, `blob_sha`, and exact line ranges for evidence.

```js
const opened = await codemode.wit.open({ repo: "owner/repo" });
const found = await codemode.wit.searchCode({
  snapshot_id: opened.snapshot_id,
  queries: ["TargetSymbol"],
  max_results: 4,
  max_bytes: 16_384
});
const hit = found.items[0];
if (!hit) return { snapshot_id: opened.snapshot_id, items: [] };
return await codemode.wit.read({
  snapshot_id: opened.snapshot_id,
  path: hit.path,
  start_line: hit.start_line,
  end_line: hit.end_line,
  max_bytes: 16_384
});
```

The fixed defaults bound source, wall time, host calls/concurrency, pages, snapshots, host-result bytes, cumulative bytes, and final JSON. The sandbox has no filesystem, network, environment, process, subprocess, shell, or module-loader capability; credentials, snapshots, cache access, and privileged operations remain in the Rust parent. Host errors are catchable with stable `code`, `operation`, and redacted `message` fields. Cancellation, timeout, resource exhaustion, invalid final JSON, worker exit, and protocol errors fail explicitly. A failed worker is killed and reaped, and the next invocation starts fresh.

Generated source is not persisted or logged. Worker diagnostic content is drained but neither returned nor logged; only capped byte counts and a truncation flag are retained. Never put secrets in source or results. Snapshots do not survive a parent server restart, so call `open` again after reconnecting.

The checked-in external model evaluation is unrun, so Code Mode remains experimental and direct mode remains the fail-closed recommendation. Do not claim token, outer-call, or latency improvements unless a complete checked-in benchmark report passes every predeclared correctness, provenance, efficiency, latency, startup, and invalid-call gate.

## Cache Behavior

Repo-reading commands (`tree`, `ls`, `cat`, `rg`, `sed`, `head`, and `tail`) use a branch-keyed stale-while-revalidate cache by default. When a selected-branch cache exists, `wit` serves it immediately, then checks the remote branch in the background and refreshes the cache if the commit SHA changed. Concurrent checks for the same repository and branch are coalesced, so a remote SHA lookup does not hold up other valid warm-cache reads. Without `--branch`, the selected branch is the repository's default branch.

Use `wit branches -r owner/repo` to list branch names under `refs/heads` before passing one to `--branch`. The table shows the default marker, tip SHA, tip commit author, tip commit time, ahead and behind counts, graph-merged status against the repository default branch, created time, and created source. `merged` means the branch tip is reachable from the default branch, not PR or squash-merge state. Created time is inferred from the first unique commit when one exists; otherwise the created source is `tip commit fallback` and the value is the branch tip commit time.

Use `--branch BRANCH` on `cache`, `tree`, `ls`, `cat`, `rg`, `sed`, `head`, or `tail` to target a GitHub branch under `refs/heads` instead of the repository default branch:

```bash
wit cache -r ratatui/ratatui --branch main
wit cat --branch main -r ratatui/ratatui README.md
wit rg --branch main 'impl Widget' -r ratatui/ratatui
```

Use `--refresh-cache` on a repo-reading command when the read must wait for a fresh cache for the selected branch:

```bash
wit tree --refresh-cache -r ratatui/ratatui src
wit rg --branch main --refresh-cache 'impl Widget' -r ratatui/ratatui
```

`wit cache -r owner/repo` is also a force-refresh command for the default branch; add `--branch BRANCH` to refresh that named branch. Internally, cache entries are stored per repository and branch, with metadata recording the branch name and current SHA. No public `--max-age` or TTL option exists.

## Commands

### search (alias: s)

Find GitHub repositories via the GitHub REST API. Results ordered by stars. Set `GITHUB_TOKEN` for higher rate limits.

```bash
wit search -p 'ratatui' -l 'Rust' --limit 20
wit search -q 'stars:>1000 topic:tui archived:false'
wit search -p 'auth' -q 'user:ory language:Go pushed:>2025-01-01'
```

| Flag | Description |
|------|-------------|
| `-p/--pattern` | Repository name filter (literal, not regex) |
| `-l/--lang` | GitHub `language:` qualifier |
| `-q/--query` | Raw GitHub search qualifiers passed through unchanged |
| `-n/--limit` | Max repositories to return (default 10, max 1000) |

### branches

List branch names and default-branch comparison metadata before choosing `--branch`:

```bash
wit branches -r ratatui/ratatui
```

Columns include branch name, default marker, tip SHA, tip author/time, ahead, behind, merged, created, and created source. Ahead, behind, and merged compare each branch to the repository default branch. Created uses the first unique commit when available; default or no-unique branches use a tip commit fallback.

### cache (alias: c)

Force-refresh a cached repo. Repos are auto-cached on first use by every other command.

```bash
wit cache -r ratatui/ratatui
```

### tree (alias: t)

Show recursive file tree. Use `-l` for line counts and token estimates to decide what to read.

```bash
wit tree -r ratatui/ratatui
wit tree -r ratatui/ratatui src/widgets
wit tree -l -r ratatui/ratatui src
```

### ls

List one directory level (non-recursive). Use `-l` for sizes.

```bash
wit ls -r ratatui/ratatui
wit ls -r ratatui/ratatui src/widgets
wit ls -l -r ratatui/ratatui src
```

### cat

Print file contents. For large files prefer `head`/`tail`/`sed`.

```bash
wit cat -r ratatui/ratatui Cargo.toml
wit cat -n -r ratatui/ratatui src/lib.rs        # numbered lines
wit cat -b -r ratatui/ratatui README.md         # number non-blank only
```

| Flag | Description |
|------|-------------|
| `-r/--repo` | Repository in `owner/repo` format (required) |
| `-n/--number` | Number all output lines |
| `-b/--number-nonblank` | Number non-blank lines only (overrides `-n`) |
| `-s/--squeeze-blank` | Suppress repeated empty lines |
| `-E/--show-ends` | Show `$` at end of each line |
| `-T/--show-tabs` | Show TAB as `^I` |
| `-A/--show-all` | Equivalent to `-ET` |

### rg

Ripgrep-style regex search. Use `-l` to list files containing a pattern (cheaper than full match output).

```bash
wit rg 'impl Widget' -r ratatui/ratatui
wit rg -l 'struct.*Frame' -r ratatui/ratatui        # files only
wit rg -g '*.rs' -i 'todo' -r ratatui/ratatui       # case-insensitive in .rs files
wit rg -C 3 'fn render' -r ratatui/ratatui           # 3 lines of context
wit rg -l --long 'Widget' -r ratatui/ratatui         # files with line counts
```

| Flag | Description |
|------|-------------|
| `-r/--repo` | Repository in `owner/repo` format (required) |
| `-i/--ignore-case` | Case insensitive |
| `-S/--smart-case` | Case-insensitive when pattern is all lowercase |
| `-w/--word-regexp` | Match whole words only |
| `-v/--invert-match` | Show non-matching lines |
| `-m/--max-count` | Max matches to show; omit for unlimited, use `0` for no matches |
| `-C/-B/-A` | Context lines around matches |
| `-g/--glob` | Filter files by glob (e.g. `*.rs`) |
| `-l/--files-with-matches` | Show only file names |
| `-c/--count` | Show match count per file |
| `--long` | Show file sizes alongside names (with `-l`) |

### sed

POSIX-style sed on a single repo file. Regex uses Rust syntax (not POSIX BRE).

```bash
wit sed -n -r modal-labs/modal-client '320,460p' modal/image.py   # line range
wit sed -n -r ratatui/ratatui '/TODO/p' src/lib.rs                # matching lines
wit sed -r ratatui/ratatui 's/Widget/Component/g' src/lib.rs      # substitution
wit sed -n -r ratatui/ratatui '/^pub fn/p' src/lib.rs             # function sigs
```

| Flag | Description |
|------|-------------|
| `-r/--repo` | Repository in `owner/repo` format (required) |
| `-n/--quiet` | Suppress automatic printing of pattern space |
| `-e/--expression` | Add script expression (repeatable) |
| `-f/--file` | Add script from file (repeatable) |

### head

Print first N lines (default 10). Use to preview before deciding whether to read fully.

```bash
wit head -r ratatui/ratatui src/lib.rs
wit head -n 50 -r ratatui/ratatui Cargo.toml
wit head -N -r ratatui/ratatui README.md           # with line numbers
```

### tail

Print last N lines or from line N onward.

```bash
wit tail -r ratatui/ratatui src/lib.rs
wit tail -n 20 -r ratatui/ratatui Cargo.toml
wit tail -p 100 -r ratatui/ratatui src/lib.rs      # from line 100 to EOF
```

## Global Options

`--ignore <PATH|GLOB>` excludes files/dirs from file operations (repeatable; not applied to `search`).

```bash
wit tree -r ratatui/ratatui --ignore '.github' --ignore 'assets/**'
wit rg 'TODO' -r ratatui/ratatui --ignore '*.lock'
```

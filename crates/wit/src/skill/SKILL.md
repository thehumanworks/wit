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

When an MCP client is available, use `wit mcp --transport stdio` as the stdio server instead of shelling out. The standalone `wit-mcp` binary remains available for clients that prefer a dedicated command. The server exposes the CLI command surface as MCP tools:

- `wit_search`
- `wit_cache_refresh`
- `wit_tree`
- `wit_ls`
- `wit_cat`
- `wit_rg`
- `wit_sed`
- `wit_head`
- `wit_tail`
- `wit_skill_load`
- `wit_skill_install`

It also exposes prompts (`wit_explore_repo`, `wit_discover_repos`, `wit_read_precise`) and resources (`wit://skill/SKILL.md`, `wit://guide/workflow`, `wit://guide/tools`) so agents can retrieve the recommended workflow without the user repeating a long preamble. MCP repo-reading tools accept optional JSON `branch` plus `refresh_cache` parameters. Omit `branch` to read the repository default branch; set `refresh_cache: true` when the selected branch must be fetched before reading. MCP `wit_sed` disables local sed file I/O and local script files; `wit_skill_install` writes the bundled skill under a local directory.

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

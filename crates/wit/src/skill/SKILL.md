---
name: wit
description: Explore GitHub repositories without cloning using the `wit` CLI. Use when you need to discover repositories, browse file trees, read files, search code, or extract content from GitHub repos. Supports: finding repos with `wit search`, viewing repo structure with `wit tree`/`wit ls`, reading files with `wit cat`/`wit head`/`wit tail`/`wit sed`, and searching code with `wit rg`. Install with `npm install -g @nothumanwork/wit`.
---

# wit — GitHub Repository Explorer

`wit` caches GitHub repos as shallow bare clones under `$TMPDIR/.wit/cache` (override with `WIT_CACHE_DIR`) and exposes Unix-style commands to explore them without a full clone.

## Typical Workflow

1. Discover repos: `wit search`
2. Orient yourself: `wit tree` or `wit ls`
3. Find code: `wit rg`
4. Read files: `wit cat`, `wit head`, `wit tail`, `wit sed`

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

### cache (alias: c)

Force-refresh a cached repo. Repos are auto-cached on first use by every other command.

```bash
wit cache ratatui/ratatui
```

### tree (alias: t)

Show recursive file tree. Use `-l` for line counts and token estimates to decide what to read.

```bash
wit tree ratatui/ratatui
wit tree ratatui/ratatui src/widgets
wit tree -l ratatui/ratatui src
```

### ls

List one directory level (non-recursive). Use `-l` for sizes.

```bash
wit ls ratatui/ratatui
wit ls ratatui/ratatui src/widgets
wit ls -l ratatui/ratatui src
```

### cat

Print file contents. For large files prefer `head`/`tail`/`sed`.

```bash
wit cat ratatui/ratatui Cargo.toml
wit cat -n ratatui/ratatui src/lib.rs        # numbered lines
wit cat -b ratatui/ratatui README.md         # number non-blank only
```

| Flag | Description |
|------|-------------|
| `-n/--number` | Number all output lines |
| `-b/--number-nonblank` | Number non-blank lines only (overrides `-n`) |
| `-s/--squeeze-blank` | Suppress repeated empty lines |
| `-E/--show-ends` | Show `$` at end of each line |
| `-T/--show-tabs` | Show TAB as `^I` |
| `-A/--show-all` | Equivalent to `-ET` |

### rg

Ripgrep-style regex search. Use `-l` to list files containing a pattern (cheaper than full match output).

```bash
wit rg 'impl Widget' ratatui/ratatui
wit rg -l 'struct.*Frame' ratatui/ratatui        # files only
wit rg -g '*.rs' -i 'todo' ratatui/ratatui       # case-insensitive in .rs files
wit rg -C 3 'fn render' ratatui/ratatui           # 3 lines of context
wit rg -l --long 'Widget' ratatui/ratatui         # files with line counts
```

| Flag | Description |
|------|-------------|
| `-i/--ignore-case` | Case insensitive |
| `-S/--smart-case` | Case-insensitive when pattern is all lowercase |
| `-w/--word-regexp` | Match whole words only |
| `-v/--invert-match` | Show non-matching lines |
| `-m/--max-count` | Max matches to show |
| `-C/-B/-A` | Context lines around matches |
| `-g/--glob` | Filter files by glob (e.g. `*.rs`) |
| `-l/--files-with-matches` | Show only file names |
| `-c/--count` | Show match count per file |
| `--long` | Show file sizes alongside names (with `-l`) |

### sed

POSIX-style sed on a single repo file. Regex uses Rust syntax (not POSIX BRE).

```bash
wit sed -n '320,460p' modal-labs/modal-client modal/image.py   # line range
wit sed -n '/TODO/p' ratatui/ratatui src/lib.rs                # matching lines
wit sed 's/Widget/Component/g' ratatui/ratatui src/lib.rs      # substitution
wit sed -n '/^pub fn/p' ratatui/ratatui src/lib.rs             # function sigs
```

| Flag | Description |
|------|-------------|
| `-n/--quiet` | Suppress automatic printing of pattern space |
| `-e/--expression` | Add script expression (repeatable) |
| `-f/--file` | Add script from file (repeatable) |

### head

Print first N lines (default 10). Use to preview before deciding whether to read fully.

```bash
wit head ratatui/ratatui src/lib.rs
wit head -n 50 ratatui/ratatui Cargo.toml
wit head -N ratatui/ratatui README.md           # with line numbers
```

### tail

Print last N lines or from line N onward.

```bash
wit tail ratatui/ratatui src/lib.rs
wit tail -n 20 ratatui/ratatui Cargo.toml
wit tail -p 100 ratatui/ratatui src/lib.rs      # from line 100 to EOF
```

## Global Options

`--ignore <PATH|GLOB>` excludes files/dirs from file operations (repeatable; not applied to `search`).

```bash
wit tree ratatui/ratatui --ignore '.github' --ignore 'assets/**'
wit rg 'TODO' ratatui/ratatui --ignore '*.lock'
```

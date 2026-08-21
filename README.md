# wit

GitHub for AI Agents -- explore GitHub repositories without cloning. Repos are cached as shallow bare clones under your system temp directory by default (override with `WIT_CACHE_DIR`).

## Status

**v0.1.0** - Early development

## Try without installing

A static GitHub Pages page runs `wit tree`, `wit ls`, and `wit cat` in the
browser through the existing `wit_snapshot.wasm` module (fixture-backed
`demo/repo`; live `api.github.com` is best-effort). The published host
loads same-origin `try/wit_snapshot.wasm`, then the v0.1.33 release
asset — never a cargo `target/` path.

https://thehumanworks.github.io/wit/

```bash
# same page documents this one-liner
mise x github:thehumanworks/wit -- wit tree owner/repo
```

Local preview from this repo:

```bash
bash scripts/serve_docs_site.sh
# open http://127.0.0.1:8765/ and run: wit tree demo/repo
```

## Installation

### Install from npm

```bash
npm install -g @nothumanwork/wit
```

Run without global install:

```bash
npx @nothumanwork/wit --help
```

`@nothumanwork/wit` is a single npm package. During `npm install`, its `postinstall` script detects the host platform, selects the matching bundled release archive, and extracts both `wit` and `wit-mcp` into npm's bin-managed install location, so one package covers:
- `darwin-x64`
- `darwin-arm64`
- `linux-x64`
- `linux-arm64`
- `win32-x64`
- `win32-arm64`

### Install from binary release (`.sh` installer)

```bash
curl -fsSL https://raw.githubusercontent.com/thehumanworks/wit/main/install.sh | sh
```

Install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/thehumanworks/wit/main/install.sh | sh -s -- --version v0.1.0
```

Install to a specific bin directory:

```bash
curl -fsSL https://raw.githubusercontent.com/thehumanworks/wit/main/install.sh | sh -s -- --bin-dir ~/.local/bin
```

The installer auto-detects platform and fetches these release artifacts:
- `wit-linux-x86_64.tar.gz`
- `wit-linux-aarch64.tar.gz`
- `wit-macos-x86_64.tar.gz`
- `wit-macos-aarch64.tar.gz`
- `wit-windows-x86_64.zip`
- `wit-windows-aarch64.zip` (best effort; falls back to x64 in shell environments if unavailable)

### Run with mise (GitHub release)

```bash
mise x github:thehumanworks/wit -- wit tree owner/repo
```

To also fetch the release wasm module (`wit_snapshot.wasm`), set in mise config:

```toml
[tools]
"github:thehumanworks/wit" = { version = "latest", additional_asset_patterns = ["wit_snapshot.wasm"] }
```

CI uploads `wit_snapshot.wasm` on every green main/PR run; semver tags attach it to the GitHub release (and list it in `wit-checksums.txt`).

### Install from source

```bash
cargo install --path crates/wit --bins
```

## Quick Start

```bash
wit search -p 'ratatui' -l 'Rust'                        # Find repos
wit tree -r ratatui/ratatui                                   # See structure
wit ls -l -r ratatui/ratatui src                              # Browse with sizes
wit rg -l 'impl Widget' -r ratatui/ratatui                    # Find files
wit cat -n -r ratatui/ratatui src/lib.rs                      # Read a file
wit head -n 30 -r ratatui/ratatui Cargo.toml                  # Preview a file
wit sed -n -r ratatui/ratatui '100,150p' src/lib.rs           # Extract range
wit branches -r ratatui/ratatui                               # List branches before choosing --branch
wit cat --branch main -r ratatui/ratatui README.md            # Read a named branch
wit tree --refresh-cache -r ratatui/ratatui src               # Force fresh cache before reading
wit rg 'TODO' -r ratatui/ratatui --ignore '.git' --ignore '*.png'  # Exclude paths
```

## MCP Server

`wit mcp --transport stdio` starts the agent-native MCP v2 server. The standalone `wit-mcp` binary provides the same default surface. V2 is snapshot-first: call `wit_open` to pin a repository ref to an immutable commit, then reuse its `snapshot_id` with the semantic exploration tools:

- `wit_find_repositories`: discover `owner/repo` when it is unknown.
- `wit_refs`: discover the default branch, branches, and tags.
- `wit_open`: pin the default branch, a named branch, tag, or full commit SHA.
- `wit_list`: list repository structure with explicit depth.
- `wit_search_code`: run bounded multi-query code search with context and provenance.
- `wit_read`: read explicit one-based inclusive line ranges.
- `wit_context`: rank and merge deterministic multi-file evidence without an internal model.

Every evidence item includes the repository, immutable commit, path, blob identity, and applicable line range. Collection responses are structured, use a 64 KiB default whole-response budget, and return `next_cursor` whenever `has_more` is true. A cursor is bound to the tool, snapshot, and normalized arguments; changing any of them returns an error instead of silently mixing result sets.

Direct MCP is the default and the recommended mode for a simple operation such as one open, list,
search, or read. It exposes seven typed tools directly, so existing client configurations continue
to work unchanged. Code Mode is experimental and is intended for bounded composition where one
model call can open, search, filter, and read before returning a focused result. It exposes one
normal MCP tool named `code`; Code Mode is an optional wit workflow, not an MCP protocol
requirement.

Example MCP client configurations using both supported entrypoints:

```json
{
  "mcpServers": {
    "wit-direct": {
      "command": "wit",
      "args": ["mcp", "--transport", "stdio", "--mode", "direct"],
      "env": {
        "WIT_CACHE_DIR": "/tmp/wit-mcp-cache"
      }
    },
    "wit-code-experimental": {
      "command": "wit",
      "args": ["mcp", "--transport", "stdio", "--mode", "code"]
    }
  }
}
```

Omit `--mode` from either entrypoint to select direct mode. The equivalent explicit commands are
`wit mcp --transport stdio --mode direct|code` and `wit-mcp --mode direct|code`. Both are native
binaries; Code Mode starts a hidden child of the installed executable and requires no Node.js,
npm, Wrangler, Cloudflare, or external JavaScript runtime. Both entrypoints write MCP protocol
frames only to stdout and diagnostics to stderr.

`wit_open` uses the branch-keyed stale-while-revalidate cache by default and reports that state explicitly. Set `freshness: "require_fresh"` to refresh a branch before pinning it. Tags and full commit SHAs are fetched directly into the immutable server-lifetime snapshot store. Pull-request head refs are not yet resolved directly; pass the PR head's full commit SHA.

### Experimental Code Mode

The `code` tool accepts an async JavaScript function body, not TypeScript and not a complete module.
Call `codemode.wit.help()` for the method list, signatures, examples, and result limits, or
`codemode.wit.help("read")` for one method. Use `await`, ordinary JavaScript control flow, and the
generated `codemode.wit` methods `findRepositories`, `refs`, `open`, `list`, `searchCode`, `read`,
and `context`. Unknown method errors include a nearest-name suggestion. The declarations are
generated from the Rust operation registry and checked in at
[`crates/wit/codemode.wit.d.ts`](crates/wit/codemode.wit.d.ts).

When the repository name is fuzzy, discovery stays in Code Mode:

```js
const repositories = await codemode.wit.findRepositories({
  pattern: "ratatuizilla",
  max_items: 5
});
return repositories.items.map(repo => repo.full_name);
```

This example performs open, search, and precise read in one Code Mode invocation and returns only
provenance-bearing evidence:

```js
const opened = await codemode.wit.open({ repo: "thehumanworks/wit" });
const matches = await codemode.wit.searchCode({
  snapshot_id: opened.snapshot_id,
  queries: ["fn code_tool_description"],
  path_prefix: "crates/wit/src",
  glob: "**/*.rs",
  exclude: ["**/tests/**"],
  max_results: 4,
  max_bytes: 16_384
});
const match = matches.items[0];
if (!match) return { snapshot_id: opened.snapshot_id, items: [] };

const read = await codemode.wit.read({
  snapshot_id: opened.snapshot_id,
  path: match.path,
  start_line: match.start_line,
  end_line: match.end_line,
  max_bytes: 16_384
});
return read;
```

In Code Mode, `read` defaults to `format: "text"`, which returns `text` plus one top-level
provenance envelope instead of repeating it for every line. Use `format: "lines"` for
`{ line_number, text }` pairs or `format: "structured"` for the full per-line shape. Use
`list({ ..., format: "paths" })` for a compact paths-only listing. `searchCode` accepts
`path_prefix`, one `glob` or several `globs`, and `exclude` globs so broad matches do not flood
changelogs, examples, or generated paths. Direct MCP keeps its structured list/read defaults.

Snapshots live only for the parent MCP server process; after restart, call `open` again. A snapshot
can be reused by later `code` calls while that parent remains alive. Pagination is never implicit:
when `has_more` is true, pass `next_cursor` as `cursor` with the same method arguments and
`snapshot_id`. Cursors are opaque and bound to the operation, snapshot, and normalized arguments.
Return one focused JSON-serializable value. Page budgets report `serialized_bytes`,
`remaining_bytes`, and a `warning` near `max_bytes`. Final JSON is rejected atomically rather than
truncated; an oversized-result error reports the actual limit and points to the compact read/list
formats. Repository evidence should retain `repo`, `commit_sha`, `snapshot_id`, `path`, `blob_sha`,
and line ranges from the host results.

Code runs with fixed fail-closed limits: 32 KiB of source, 10 seconds wall time, 16 host calls (at
most 4 concurrent), 8 page-producing calls, 2 snapshot opens, 64 KiB per host result, 256 KiB of
cumulative host results, and a 48 KiB final result. The sandbox has no filesystem, network,
environment, process, subprocess, shell, or module-loader capability. GitHub credentials, cache and
snapshot access, and every privileged operation remain in the Rust parent; code cannot raise its
own budgets or invoke arbitrary MCP/host APIs.

Host-operation failures are catchable JavaScript `Error` objects with stable `code`, `operation`,
and redacted `message` fields. MCP-level policy/resource failures return structured `code` and
`message`; cancellation and timeout use `cancelled` and `deadline_exceeded`. A timed-out, cancelled,
wedged, crashed, or malformed worker is killed and reaped; each invocation uses a fresh worker, so
the next call can recover without reusing worker state. Worker stderr is continuously drained, but
only its capped byte count and truncation flag are retained—content is not returned or logged.
Generated source is held only for the invocation and is not persisted or logged. Do not
place secrets in generated source or returned values. Parent-side diagnostics redact privileged
operation failures, and Code Mode clears `OpenResult.cache.last_error` because it can contain paths
or token-shaped backend text.

The checked-in [benchmark status](benchmarks/codemode/results/status.json) says the external model
evaluation has not run, promotion is ineligible, Code Mode is experimental, and direct is the
recommendation. Therefore wit currently makes no token, call-count, or latency improvement claim.
Code Mode can leave experimental status only after a complete report passes every predeclared gate
in the [benchmark contract](benchmarks/codemode/README.md), including correctness and provenance
with no regression from direct mode, the composition-heavy reduction threshold, latency/startup
ceilings, and the invalid-call ceiling. Missing, incomplete, or failed evidence keeps the status
experimental, and the fail-closed recommendation remains direct mode.

See the [Code Mode security boundary](docs/codemode-security.md) for the complete limits and error
contract and the [native release contract](docs/codemode-release.md) for packaging, target, license,
SBOM, and size evidence.

The implementation status and remaining verification work for the snapshot, pagination, and semantic-tool foundations are recorded in [Direct MCP foundation status](docs/direct-mcp-foundation-status.md).

## Global Options

| Flag | Description |
|------|-------------|
| `--ignore <PATH\|GLOB>` | Exclude files/directories/globs from file operations. Repeat the flag to provide multiple patterns. |

Ignore examples:

```bash
wit tree -r ratatui/ratatui --ignore '.github' --ignore 'assets/**'
wit ls -r ratatui/ratatui src --ignore 'generated'
wit rg 'fn main' -r ratatui/ratatui --ignore '.git' --ignore '*.lock'
wit cat -r ratatui/ratatui src/main.rs --ignore 'src/main.rs'   # blocked (explicitly ignored)
```

`search` ignores `--ignore`, because repository discovery is done through GitHub's repository search API rather than cached file traversal.

## Commands

### search (alias: s)

Discover repositories using **GitHub’s repository search API** only. Results are ordered by **stars** and the table shows **stars**. Use `-p/--pattern` for a repository-name filter, `-l/--lang` for a GitHub `language:` qualifier, and `-q/--query` to pass raw GitHub repository-search terms and qualifiers through unchanged.

Set **`GITHUB_TOKEN`** for higher GitHub rate limits. GitHub’s `language:` filter must match [GitHub language names](https://github.com/search?q=language%3ARust&type=repositories). `wit search` fetches only enough GitHub pages to satisfy `--limit` (default: `10`, max: `1000`). GitHub may return at most 1000 repositories per query; if GitHub sets `incomplete_results`, a warning is printed.

```bash
wit search -p 'ratatui' -l 'Rust' --limit 20                 # Rust repos named ratatui (by stars)
wit search -q 'stars:>1000 topic:tui archived:false'         # Raw GitHub qualifiers
wit search -p 'auth' -q 'user:ory language:Go pushed:>2025-01-01'
```

| Flag | Long | Description |
|------|------|-------------|
| `-p` | `--pattern` | Optional repository-name filter (`in:name`) |
| `-l` | `--lang` | Optional GitHub `language:` qualifier |
| `-q` | `--query` | Raw GitHub search terms and qualifiers, passed through as-is |
| `-n` | `--limit` | Maximum repositories to print (default `10`, max `1000`) |

### branches

List GitHub branches under `refs/heads` before choosing a value for `--branch` on cache/read commands.

```bash
wit branches -r ratatui/ratatui
```

The output includes branch name, default marker, tip SHA, tip commit author, tip commit time, ahead and behind counts, graph-merged status, created time, and created source. Ahead, behind, and merged are computed against the repository default branch; merged means the branch tip is reachable from the default branch, not PR or squash-merge state. Created time is inferred from the first unique commit on the branch when one exists. For the default branch and branches with no unique commits, created time falls back to the tip commit time and the source column says `tip commit fallback`.

### cache (alias: c)

Clone a repository into the local cache (or refresh an existing one). Pass the repository with `-r` / `--repo` (`owner/repo`). Repos are auto-cached on first use by other commands.

```bash
wit cache -r ratatui/ratatui                    # Force re-clone of the default branch
wit cache -r ratatui/ratatui --branch main      # Force refresh a named branch
```

### Cache freshness

Repo-reading commands (`tree`, `ls`, `cat`, `rg`, `sed`, `head`, and `tail`) use a branch-keyed stale-while-revalidate cache by default: `wit` serves the cached selected branch immediately when it is present, then quietly checks the remote branch and refreshes the cache when the commit SHA changed. Concurrent freshness checks for the same repository and branch are coalesced, and their remote SHA lookup does not block other valid warm-cache reads. Without `--branch`, the selected branch is the repository's default branch. A cold cache still clones before the read can continue.

Run `wit branches -r owner/repo` to list available branch names with ahead/behind, graph-merged, author, tip, and created-time metadata before passing one to `--branch`.

Use `--branch BRANCH` on `cache`, `tree`, `ls`, `cat`, `rg`, `sed`, `head`, or `tail` to target a GitHub branch under `refs/heads` instead of the repository default branch:

```bash
wit cache -r ratatui/ratatui --branch main
wit cat --branch main -r ratatui/ratatui README.md
wit rg --branch main 'impl Widget' -r ratatui/ratatui
```

Use `--refresh-cache` on a repo-reading command when that specific read must wait for a fresh cache for the selected branch:

```bash
wit tree --refresh-cache -r ratatui/ratatui src
wit rg --branch main --refresh-cache 'impl Widget' -r ratatui/ratatui
```

`wit cache -r owner/repo` is also a force-refresh command for the default branch; add `--branch BRANCH` to refresh that named branch. Internally, cache entries are stored per repository and branch under `WIT_CACHE_DIR`, with metadata recording the branch name and current SHA. No public `--max-age` or TTL option exists.

### Snapshot backends (disk vs memory)

Repo-reading commands default to the **disk** cache backend. Pass `--backend memory` (or set `WIT_SNAPSHOT_BACKEND=memory`) to load a **public** repository over the GitHub REST API into RAM with **zero** `WIT_CACHE_DIR` writes. Provenance (`commit_sha`, `tree_sha`, backend label) is printed on stderr.

Memory covers `tree` / `ls` / `cat` / `rg` / `sed` / `head` / `tail` (including `--branch`, `--ignore`, `-l`, and `-n` where those flags apply). `wit cache --backend memory` pins/opens the in-memory snapshot (prefetch tree; optional root-blob warm) instead of cloning. `wit branches --backend memory` lists branches via the GitHub API. `wit search` always uses the GitHub REST API and never needs the disk cache.

Pass the repository as a positional `owner/repo` or with `-r/--repo` (if both are given they must match):

```bash
wit tree octocat/Hello-World
wit tree octocat/Hello-World --backend memory
wit ls   octocat/Hello-World
wit cat  octocat/Hello-World README
wit rg 'Hello' octocat/Hello-World --backend memory
wit head -n 5 octocat/Hello-World README --backend memory
wit cache octocat/Hello-World --backend memory
wit branches octocat/Hello-World --backend memory
```

See `docs/nofS-snapshot.md` and `bash scripts/nofS_demo.sh` for the no-FS demo.

### tree (alias: t)

Show the file tree of a repository (or subtree). Pass the repository with `-r` / `--repo` (`owner/repo`). Use `-l` for line counts and token estimates.

```bash
wit tree ratatui/ratatui                # Full repo tree
wit tree ratatui/ratatui src/widgets    # Only the widgets subtree
wit tree -l -r ratatui/ratatui src      # With line counts and token estimates
wit tree octocat/Hello-World --backend memory
```

### ls

List directory contents (non-recursive). Unlike `tree`, shows only immediate children. Pass the repository with `-r` / `--repo` (`owner/repo`). Use `-l` for file sizes.

```bash
wit ls -r ratatui/ratatui                    # List repo root
wit ls -r ratatui/ratatui src/widgets        # List a subdirectory
wit ls -l -r ratatui/ratatui src             # With line counts and token estimates
```

### cat

Print a file's contents. Pass the repository with `-r` / `--repo` (`owner/repo`). For large files, prefer `head`/`tail`/`sed` to read specific ranges, or `rg` to search.

```bash
wit cat -r ratatui/ratatui Cargo.toml             # Print file
wit cat -n -r ratatui/ratatui src/lib.rs           # With line numbers
wit cat -b -r ratatui/ratatui README.md            # Number non-blank lines only
```

| Flag | Long | Description |
|------|------|-------------|
| `-r` | `--repo` | GitHub repository (`owner/repo`) |
|      | `--branch` | GitHub branch under `refs/heads` to read instead of the default branch |
| `-n` | `--number` | Number all output lines |
| `-b` | `--number-nonblank` | Number non-blank lines only (overrides -n) |
| `-s` | `--squeeze-blank` | Suppress repeated empty lines |
| `-E` | `--show-ends` | Show `$` at end of each line |
| `-T` | `--show-tabs` | Show TAB as `^I` |
| `-A` | `--show-all` | Equivalent to `-ET` |

### rg

Search file contents (ripgrep-style). Pass the repository with `-r` / `--repo` (`owner/repo`). Use `-l` to find files, `-g` to filter by type.

```bash
wit rg 'impl Widget' -r ratatui/ratatui              # Find implementations
wit rg -l 'struct.*Frame' -r ratatui/ratatui          # List files containing pattern
wit rg -g '*.rs' -i 'todo' -r ratatui/ratatui         # Case-insensitive in .rs files
wit rg -C 3 'fn render' -r ratatui/ratatui             # 3 lines of context
wit rg -l --long 'Widget' -r ratatui/ratatui           # File list with line counts
```

| Flag | Long | Description |
|------|------|-------------|
| `-r` | `--repo` | GitHub repository (`owner/repo`) |
|      | `--branch` | GitHub branch under `refs/heads` to search instead of the default branch |
| `-i` | `--ignore-case` | Case insensitive search |
| `-S` | `--smart-case` | Case-insensitive if pattern is all lowercase |
| `-w` | `--word-regexp` | Match whole words only |
| `-v` | `--invert-match` | Show non-matching lines |
| `-m` | `--max-count` | Maximum matches to show; omit for unlimited, use `0` for no matches |
| `-C` | `--context` | Lines of context before and after matches |
| `-B` | `--before-context` | Lines of context before matches |
| `-A` | `--after-context` | Lines of context after matches |
| `-g` | `--glob` | Glob pattern to filter files (e.g., `*.rs`) |
| `-l` | `--files-with-matches` | Only show file names with matches |
| `-c` | `--count` | Only show count of matches per file |
|      | `--long` | Show file sizes alongside names (useful with `-l`) |

### sed

Extract or transform file content using sed scripts (POSIX-style, Rust regex). Pass the repository with `-r` / `--repo` (`owner/repo`).

```bash
wit sed -n -r modal-labs/modal-client '320,460p' modal/image.py    # Print line range
wit sed -n -r ratatui/ratatui '/TODO/p' src/lib.rs                 # Lines matching pattern
wit sed -r ratatui/ratatui 's/Widget/Component/g' src/lib.rs       # Substitute text
wit sed -n -r ratatui/ratatui '/^pub fn/p' src/lib.rs              # Extract function signatures
```

**Notes:**
- Pass the repository with `-r` / `--repo` (`owner/repo`). Positional arguments are the sed script and the file path, or only the path when the script comes from `-e` / `-f`.
- Regex uses Rust syntax (not POSIX BRE).
- `sed` operates on a single repo file (no stdin or in-place edits).
- Supports addresses, substitution, hold space, branching, and most POSIX commands.

### head

Print the first N lines of a file (default: 10). Pass the repository with `-r` / `--repo` (`owner/repo`). Use to preview a file before deciding whether to read it fully.

```bash
wit head -r ratatui/ratatui src/lib.rs            # First 10 lines
wit head -n 50 -r ratatui/ratatui Cargo.toml      # First 50 lines
wit head -N -r ratatui/ratatui README.md           # With line numbers
```

### tail

Print the last N lines of a file, or from line N onward. Pass the repository with `-r` / `--repo` (`owner/repo`). Use `-p` to read from a specific line to end-of-file.

```bash
wit tail -r ratatui/ratatui src/lib.rs              # Last 10 lines
wit tail -n 20 -r ratatui/ratatui Cargo.toml        # Last 20 lines
wit tail -p 100 -r ratatui/ratatui src/lib.rs       # From line 100 to end
```

## Packaging & Release

- Every push to `main` triggers `.github/workflows/auto-tag.yml`, which increments the patch version in `Cargo.toml`, creates a new `vX.Y.Z` tag, and pushes it.
- Push a semver tag (for example `v0.2.0`) to trigger `.github/workflows/release.yml`.
- `.github/workflows/release.yml` can also be triggered manually with `workflow_dispatch` (select a `vX.Y.Z` tag ref).
- `.github/workflows/release.yml` validates the tag ref, builds Linux/macOS/Windows artifacts on GitHub-hosted runners, uploads `wit-<platform>-<arch>` archives plus `wit-checksums.txt`, and then invokes `.github/workflows/publish-npm.yml`.
- `.github/workflows/publish-npm.yml` publishes `@nothumanwork/wit` from the attached GitHub release assets and can be re-run manually for an existing release tag without rebuilding Rust binaries.
- `install.sh` downloads the matching archive and verifies it against the checksum manifest when available.

## Architecture

```
crates/wit/src/
├── cli.rs           # CLI entry point
├── bin/wit-mcp.rs   # stdio MCP server entry point
├── lib.rs           # Library exports (gitops, MCP, sed, search, search_run)
├── mcp.rs           # Agent-native MCP v2 snapshots, tools, cursors, and budgets
├── search.rs        # GitHub repository search, query assembly, limit-aware pagination
├── search_run.rs    # `wit search`: GitHub-only orchestration
├── sed.rs
└── gitops/          # Bare-repo cache, tree, ls, cat, rg, head, tail

crates/wits/         # grep.app client + `wits` binary; shared result printing
```

## Dependencies

- `clap` - CLI argument parsing
- `gix` (gitoxide) - Bare clone + tree traversal + blob reading
- `octocrab` - GitHub REST API (`wit search`)
- `reqwest` - HTTP client (grep.app in `wits`; other HTTP in wit)
- `scraper` - HTML parsing for grep.app snippets (`wits`)
- `grep-regex` / `grep-searcher` / `grep-matcher` - Ripgrep-style search on blobs
- `ptree` - Tree display
- `colored` - Terminal output formatting
- `tokio` - Async runtime
- `serde` - JSON serialization

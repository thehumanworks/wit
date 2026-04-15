# wit

GitHub for AI Agents -- explore GitHub repositories without cloning. Repos are cached as shallow bare clones under your system temp directory by default (override with `WIT_CACHE_DIR`).

## Status

**v0.1.0** - Early development

## Installation

### Install from npm

```bash
npm install -g @nothumanwork/wit
```

Run without global install:

```bash
npx @nothumanwork/wit --help
```

`@nothumanwork/wit` is a single npm package. During `npm install`, its `postinstall` script detects the host platform, selects the matching bundled release archive, and extracts it into npm's bin-managed install location, so one package covers:
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

### Install from source

```bash
cargo install --path .
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
wit rg 'TODO' -r ratatui/ratatui --ignore '.git' --ignore '*.png'  # Exclude paths
```

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

### cache (alias: c)

Clone a repository into the local cache (or refresh an existing one). Pass the repository with `-r` / `--repo` (`owner/repo`). Repos are auto-cached on first use by other commands.

```bash
wit cache -r ratatui/ratatui          # Force re-clone of ratatui
```

### tree (alias: t)

Show the file tree of a repository (or subtree). Pass the repository with `-r` / `--repo` (`owner/repo`). Use `-l` for line counts and token estimates.

```bash
wit tree -r ratatui/ratatui                # Full repo tree
wit tree -r ratatui/ratatui src/widgets    # Only the widgets subtree
wit tree -l -r ratatui/ratatui src         # With line counts and token estimates
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
| `-i` | `--ignore-case` | Case insensitive search |
| `-S` | `--smart-case` | Case-insensitive if pattern is all lowercase |
| `-w` | `--word-regexp` | Match whole words only |
| `-v` | `--invert-match` | Show non-matching lines |
| `-m` | `--max-count` | Maximum matches to show (0 = unlimited) |
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
├── lib.rs           # Library exports (gitops, sed, search, search_run)
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

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
wit tree ratatui/ratatui                                   # See structure
wit ls -l ratatui/ratatui src                              # Browse with sizes
wit rg -l 'impl Widget' ratatui/ratatui                    # Find files
wit cat -n ratatui/ratatui src/lib.rs                      # Read a file
wit head -n 30 ratatui/ratatui Cargo.toml                  # Preview a file
wit sed -n '100,150p' ratatui/ratatui src/lib.rs           # Extract range
wit rg 'TODO' ratatui/ratatui --ignore '.git' --ignore '*.png'  # Exclude paths
```

## Global Options

| Flag | Description |
|------|-------------|
| `--ignore <PATH\|GLOB>` | Exclude files/directories/globs from file operations. Repeat the flag to provide multiple patterns. |

Ignore examples:

```bash
wit tree ratatui/ratatui --ignore '.github' --ignore 'assets/**'
wit ls ratatui/ratatui src --ignore 'generated'
wit rg 'fn main' ratatui/ratatui --ignore '.git' --ignore '*.lock'
wit cat ratatui/ratatui src/main.rs --ignore 'src/main.rs'   # blocked (explicitly ignored)
```

`search` accepts `--ignore`, but applies it only when `--with-snippets` is enabled.

## Commands

### search (alias: s)

Discover repositories using **GitHub’s repository search API** when `-q` is the default (`.*`, match any code) and `--with-snippets` is off: results are ordered by **stars** and the table shows **stars**. For a non-default `-q`, snippets (`-w`), or code-driven ranking, `wit` uses **grep.app** (same backend as the `wits` crate).

Set **`GITHUB_TOKEN`** for higher GitHub rate limits. GitHub’s `language:` filter must match [GitHub language names](https://github.com/search?q=language%3ARust&type=repositories); with `--regex` true, the GitHub name path only accepts simple tokens (letters, digits, `.`, `_`, `-`); use `--regex false` for literal phrases. GitHub may return at most 1000 repositories per query; if GitHub sets `incomplete_results`, a warning is printed.

```bash
wit search -p 'ratatui' -l 'Rust'                  # GitHub: Rust repos named ratatui (by stars)
wit search -p 'auth' -q 'JWT' -l 'Go' -w           # grep.app: code + snippets
wit search -p 'ratatui' -q 'impl Widget' -w -c      # grep.app: matching lines only
```

| Flag | Long | Description |
|------|------|-------------|
| `-p` | `--pattern` | Repository name pattern (see `--regex` and `wit search --help`) |
| `-l` | `--lang` | Language filter (GitHub label on GitHub path; grep.app pattern on grep path) |
| `-q` | `--query` | Code pattern (default: `.*`; non-default selects grep.app) |
| `-r` | `--regex` | Regex for `-p` on grep path; on GitHub path, true = simple token only |
| `-w` | `--with-snippets` | Code snippets (uses grep.app) |
| `-c` | `--compact` | Matching lines only with `-w` |

### cache (alias: c)

Clone a repository into the local cache (or refresh an existing one). Repos are auto-cached on first use by other commands.

```bash
wit cache ratatui/ratatui          # Force re-clone of ratatui
```

### tree (alias: t)

Show the file tree of a repository (or subtree). Use `-l` for line counts and token estimates.

```bash
wit tree ratatui/ratatui                # Full repo tree
wit tree ratatui/ratatui src/widgets    # Only the widgets subtree
wit tree -l ratatui/ratatui src         # With line counts and token estimates
```

### ls

List directory contents (non-recursive). Unlike `tree`, shows only immediate children. Use `-l` for file sizes.

```bash
wit ls ratatui/ratatui                    # List repo root
wit ls ratatui/ratatui src/widgets        # List a subdirectory
wit ls -l ratatui/ratatui src             # With line counts and token estimates
```

### cat

Print a file's contents. For large files, prefer `head`/`tail`/`sed` to read specific ranges, or `rg` to search.

```bash
wit cat ratatui/ratatui Cargo.toml             # Print file
wit cat -n ratatui/ratatui src/lib.rs           # With line numbers
wit cat -b ratatui/ratatui README.md            # Number non-blank lines only
```

| Flag | Long | Description |
|------|------|-------------|
| `-n` | `--number` | Number all output lines |
| `-b` | `--number-nonblank` | Number non-blank lines only (overrides -n) |
| `-s` | `--squeeze-blank` | Suppress repeated empty lines |
| `-E` | `--show-ends` | Show `$` at end of each line |
| `-T` | `--show-tabs` | Show TAB as `^I` |
| `-A` | `--show-all` | Equivalent to `-ET` |

### rg

Search file contents (ripgrep-style). Use `-l` to find files, `-g` to filter by type.

```bash
wit rg 'impl Widget' ratatui/ratatui              # Find implementations
wit rg -l 'struct.*Frame' ratatui/ratatui          # List files containing pattern
wit rg -g '*.rs' -i 'todo' ratatui/ratatui         # Case-insensitive in .rs files
wit rg -C 3 'fn render' ratatui/ratatui             # 3 lines of context
wit rg -l --long 'Widget' ratatui/ratatui           # File list with line counts
```

| Flag | Long | Description |
|------|------|-------------|
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

Extract or transform file content using sed scripts (POSIX-style, Rust regex).

```bash
wit sed -n '320,460p' modal-labs/modal-client modal/image.py    # Print line range
wit sed -n '/TODO/p' ratatui/ratatui src/lib.rs                 # Lines matching pattern
wit sed 's/Widget/Component/g' ratatui/ratatui src/lib.rs       # Substitute text
wit sed -n '/^pub fn/p' ratatui/ratatui src/lib.rs              # Extract function signatures
```

**Notes:**
- Regex uses Rust syntax (not POSIX BRE).
- `sed` operates on a single repo file (no stdin or in-place edits).
- Supports addresses, substitution, hold space, branching, and most POSIX commands.

### head

Print the first N lines of a file (default: 10). Use to preview a file before deciding whether to read it fully.

```bash
wit head ratatui/ratatui src/lib.rs            # First 10 lines
wit head -n 50 ratatui/ratatui Cargo.toml      # First 50 lines
wit head -N ratatui/ratatui README.md           # With line numbers
```

### tail

Print the last N lines of a file, or from line N onward. Use `-p` to read from a specific line to end-of-file.

```bash
wit tail ratatui/ratatui src/lib.rs              # Last 10 lines
wit tail -n 20 ratatui/ratatui Cargo.toml        # Last 20 lines
wit tail -p 100 ratatui/ratatui src/lib.rs       # From line 100 to end
```

## Packaging & Release

- Every push to `main` triggers `.github/workflows/auto-tag.yml`, which increments the patch version in `Cargo.toml`, creates a new `vX.Y.Z` tag, and pushes it.
- Push a semver tag (for example `v0.2.0`) to trigger `.github/workflows/release.yml`.
- `.github/workflows/release.yml` can also be triggered manually with `workflow_dispatch` (select a `vX.Y.Z` tag ref).
- The workflow builds and uploads `wit-<platform>-<arch>` archives plus `wit-checksums.txt` to the GitHub release.
- `.github/workflows/publish-npm.yml` publishes `@nothumanwork/wit` from the GitHub release assets and can be re-run manually for an existing release tag without rebuilding Rust binaries.
- `install.sh` downloads the matching archive and verifies it against the checksum manifest when available.

## Architecture

```
crates/wit/src/
├── cli.rs           # CLI entry point
├── lib.rs           # Library exports (gitops, sed, search, search_run)
├── search.rs        # GitHub repository search (octocrab)
├── search_run.rs    # `wit search`: GitHub vs grep.app routing
├── sed.rs
└── gitops/          # Bare-repo cache, tree, ls, cat, rg, head, tail

crates/wits/         # grep.app client + `wits` binary; shared result printing
```

## Dependencies

- `clap` - CLI argument parsing
- `gix` (gitoxide) - Bare clone + tree traversal + blob reading
- `octocrab` - GitHub REST API (`wit search` GitHub path)
- `reqwest` - HTTP client (grep.app in `wits`; other HTTP in wit)
- `scraper` - HTML parsing for grep.app snippets (`wits`)
- `grep-regex` / `grep-searcher` / `grep-matcher` - Ripgrep-style search on blobs
- `ptree` - Tree display
- `colored` - Terminal output formatting
- `tokio` - Async runtime
- `serde` - JSON serialization

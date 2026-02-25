# wit

GitHub for AI Agents -- explore GitHub repositories without cloning. Repos are cached as shallow bare clones under your system temp directory by default (override with `WIT_CACHE_DIR`).

## Status

**v0.1.0** - Early development

## Installation

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
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`
- `aarch64-pc-windows-msvc` (best effort; falls back to x64 in shell environments if unavailable)

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

Find GitHub repositories by name and search their code via grep.app.

```bash
wit search -p 'ratatui' -l 'Rust'                  # Find Rust repos named 'ratatui'
wit search -p 'auth' -q 'JWT' -l 'Go' -w           # Find Go auth repos using JWT, show code
wit search -p 'ratatui' -q 'impl Widget' -w -c      # Matching lines only, no context
```

| Flag | Long | Description |
|------|------|-------------|
| `-p` | `--pattern` | Regex pattern to match repository names (required) |
| `-l` | `--lang` | Filter results by language |
| `-q` | `--query` | Code pattern to search within repos (default: `.*`) |
| `-r` | `--regex` | Enable regex search (default: true) |
| `-w` | `--with-snippets` | Show code snippets with context |
| `-c` | `--compact` | Show only matching lines (requires `-w`) |

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

- Push a semver tag (for example `v0.2.0`) to trigger `.github/workflows/release.yml`.
- The workflow builds and uploads platform archives plus `wit-checksums.txt` to the GitHub release.
- `install.sh` downloads the matching archive and verifies it against the checksum manifest when available.

## Architecture

```
src/
├── cli.rs          # CLI entry point and display logic
├── lib.rs          # Library exports
├── sed.rs          # POSIX-style sed engine
├── gitops/
│   ├── mod.rs
│   └── ops.rs      # Bare-repo caching, file access, tree, ls, head/tail, ripgrep
└── grep/
    ├── mod.rs
    ├── client.rs   # grep.app API client
    └── types.rs    # Response types and data structures
```

## Dependencies

- `clap` - CLI argument parsing
- `gix` (gitoxide) - Bare clone + tree traversal + blob reading
- `reqwest` - HTTP client for grep.app
- `scraper` - HTML parsing for code snippets
- `grep-regex` / `grep-searcher` / `grep-matcher` - Ripgrep-style search on blobs
- `ptree` - Tree display
- `colored` - Terminal output formatting
- `tokio` - Async runtime
- `serde` - JSON serialization

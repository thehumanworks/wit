# wit

GitHub repository search CLI powered by [grep.app](https://grep.app).

## Status

**v0.1.0** - Early development

## Installation

```bash
cargo install --path .
```

## Usage

### Caching

Repositories are automatically cached locally on first use by commands like `tree`, `cat`, `rg`, `head`, `tail`, and `search`. Cached repos are reused on subsequent commands to improve performance.

To force a refresh of an existing cache:

```bash
wit cache owner/repo
```

Cache is stored at `/tmp/.wit/cache/`

### repo-search

Search for repositories matching a pattern:

```bash
# Basic search - find repos with "ratatui" in the name
wit repo-search -p "ratatui"

# Filter by language
wit repo-search -p "ratatui" -l "Rust"

# Search for specific code within matching repos
wit repo-search -p "ratatui" -q "Table" -l "Rust"

# Include code snippets with context
wit repo-search -p "ratatui" -q "Table" -w

# Compact mode - only matching lines, no context
wit repo-search -p "ratatui" -q "Table" -w -c
```

### Options

| Flag | Long | Description |
|------|------|-------------|
| `-p` | `--pattern` | Regex pattern to match repository names (required) |
| `-l` | `--lang` | Filter results by language |
| `-q` | `--query` | Code pattern to search within repos (default: `.*`) |
| `-r` | `--regex` | Enable regex search (default: true) |
| `-w` | `--with-snippets` | Show code snippets with context |
| `-c` | `--compact` | Show only matching lines (requires `-w`) |

## Architecture

```
src/
├── cli.rs          # CLI entry point and display logic
├── lib.rs          # Library exports
└── grep/
    ├── mod.rs
    ├── client.rs   # grep.app API client
    └── types.rs    # Response types and data structures
```

## Dependencies

- `clap` - CLI argument parsing
- `reqwest` - HTTP client
- `scraper` - HTML parsing for code snippets
- `colored` - Terminal output formatting
- `tokio` - Async runtime
- `serde` - JSON serialization

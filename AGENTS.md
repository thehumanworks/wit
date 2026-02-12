# Repository Guidelines

## Project Structure

- `src/cli.rs`: Primary CLI binary entrypoint (`wit`). Contains all subcommand definitions (clap derive) and display/output logic.
- `src/lib.rs`: Library crate root; re-exports `gitops`, `grep`, and `sed` modules.
- `src/grep/`: grep.app HTTP client + response types (`client.rs`, `types.rs`, `mod.rs`).
- `src/gitops/`: Git operations module for bare-repo caching, file access, tree display, directory listing, head/tail, and ripgrep-style search (`ops.rs`, `mod.rs`).
- `src/sed.rs`: POSIX-style sed parser and execution engine for `wit sed`. ~1140 lines including 25+ unit tests.
- `tasks/`: Task/planning files (e.g., `sed.txt`).
- `README.md`: User-facing installation and usage examples.
- `target/`: Build artifacts (ignored via `.gitignore`).

## CLI Subcommands

| Subcommand | Alias | Description | Key Flags |
|------------|-------|-------------|-----------|
| `search`   | `s`   | Find GitHub repos by name and search code via grep.app | `-p`, `-q`, `-l`, `-w`, `-c` |
| `cache`    | `c`   | Clone (or refresh) a GitHub repo into local cache | |
| `tree`     | `t`   | Show file tree of a repo (or subtree) | `-l` (line counts + token estimates) |
| `ls`       |       | List directory contents (non-recursive, one level) | `-l` (line counts + token estimates) |
| `cat`      |       | Print file contents (POSIX cat flags: -n, -b, -s, -E, -T, -A) | |
| `rg`       |       | Ripgrep-style search within a cached repo | `-l`, `-g`, `-C`, `--long` (sizes with `-l`) |
| `sed`      |       | POSIX-style sed on a file from a cached repo | `-n`, `-e`, `-f` |
| `head`     |       | First N lines of a file (default: 10) | `-n`, `-N` |
| `tail`     |       | Last N lines / from line N onward | `-n`, `-p`, `-N` |

## Build, Test, and Development Commands

- `cargo build`: Compile the project.
- `cargo run -- search -p "ratatui" -l "Rust"`: Run the CLI from source.
- `cargo install --path .`: Install `wit` locally from the working tree.
- `sh install.sh`: Install from GitHub release artifacts using the repository installer script.
- `cargo fmt`: Format code with rustfmt (standard Rust style).
- `cargo clippy -- -D warnings`: Lint and treat warnings as errors.
- `cargo test`: Run unit tests. `cargo test -- --ignored` for integration tests (require network).

## Coding Style & Naming Conventions

- Rust edition: **2024** (see `Cargo.toml`).
- Formatting: prefer `cargo fmt`; avoid manual alignment.
- Naming: modules/functions `snake_case`, types/traits `CamelCase`, constants `SCREAMING_SNAKE_CASE`.
- CLI: subcommands use `kebab-case`, flags use long `--kebab-case` and short `-x` where helpful.

## Testing Guidelines

- Unit tests are inline with `#[cfg(test)] mod tests { ... }` (36 unit tests in sed.rs and ops.rs).
- Integration tests are marked `#[ignore]` and require network access.
- Prefer tests that validate parsing and output formatting deterministically (use small, embedded fixtures).

## Commit & Pull Request Guidelines

- Commit history uses short, imperative subjects (e.g., "add ls command"); follow the same pattern:
  - Subject <= ~50 chars, present tense; add a body for rationale and edge cases.
- PRs should include:
  - A clear description of behavior changes and any user-visible output changes.
  - Repro commands (e.g., `wit search -p "..." -q "..." -w`).
  - Updates to `README.md` when flags/output change.

## Security & Network Notes

- `wit` queries `https://grep.app` over the network; avoid adding tokens/secrets to logs or CLI output.
- Be mindful of rate limits and handle HTTP failures gracefully in client code (`src/grep/client.rs`).
- Repos are cached as bare git repos in the system temp directory under `.wit/cache` (override with `WIT_CACHE_DIR`); the `cache_github_repo` function handles both initial caching and forced refresh.

## Release Packaging

- `.github/workflows/release.yml` publishes tagged releases (`v*`) with prebuilt artifacts for Linux/macOS/Windows targets.
- Release assets are named `wit-<target>.tar.gz` (Unix) and `wit-<target>.zip` (Windows), plus `wit-checksums.txt`.
- Keep artifact naming in sync with `install.sh`; changes to one should update the other in the same patch.

## Agent-Specific Notes

- Prefer `rg` / `rg --files` for repo search while working on changes.
- Keep patches focused and avoid committing generated artifacts under `target/`.
- Before handing off, run `cargo fmt` and `cargo clippy -- -D warnings`.
- The `sed` subcommand aims for broad POSIX coverage; update tests and docs alongside behavior changes.

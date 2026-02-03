# Repository Guidelines

## Project Structure

- `src/cli.rs`: Primary CLI binary entrypoint (`wit`).
- `src/lib.rs`: Library crate root; re-exports modules for reuse.
- `src/grep/`: grep.app client + response types (`client.rs`, `types.rs`, `mod.rs`).
- `src/gitops/`: Git operations module for repository caching and file access (`ops.rs`, `mod.rs`).
- `README.md`: User-facing installation and usage examples.
- `target/`: Build artifacts (ignored via `.gitignore`).

## Build, Test, and Development Commands

- `cargo build`: Compile the project.
- `cargo run -- repo-search -p "ratatui" -l "Rust"`: Run the CLI from source.
- `cargo install --path .`: Install `wit` locally from the working tree.
- `cargo fmt`: Format code with rustfmt (standard Rust style).
- `cargo clippy -- -D warnings`: Lint and treat warnings as errors.
- `cargo test`: Run tests (add as the project grows).

## Coding Style & Naming Conventions

- Rust edition: **2024** (see `Cargo.toml`).
- Formatting: prefer `cargo fmt`; avoid manual alignment.
- Naming: modules/functions `snake_case`, types/traits `CamelCase`, constants `SCREAMING_SNAKE_CASE`.
- CLI: subcommands use `kebab-case` (e.g., `repo-search`), flags use long `--kebab-case` and short `-x` where helpful.

## Testing Guidelines

- Current state: there is no dedicated `tests/` directory yet.
- Add unit tests next to code with `#[cfg(test)] mod tests { ... }` and integration tests under `tests/` when behavior spans modules.
- Prefer tests that validate parsing and output formatting deterministically (use small, embedded fixtures).

## Commit & Pull Request Guidelines

- Commit history uses short, imperative subjects (e.g., “ssh connection enabled”); follow the same pattern:
  - Subject ≤ ~50 chars, present tense; add a body for rationale and edge cases.
- PRs should include:
  - A clear description of behavior changes and any user-visible output changes.
  - Repro commands (e.g., `wit repo-search -p "…" -q "…" -w`).
  - Updates to `README.md` when flags/output change.

## Security & Network Notes

- `wit` queries `https://grep.app` over the network; avoid adding tokens/secrets to logs or CLI output.
- Be mindful of rate limits and handle HTTP failures gracefully in client code (`src/grep/client.rs`).
- Repos are cached as bare git repos in `/tmp/.wit/cache/`; the `cache_github_repo` function handles both initial caching and forced refresh.

## Agent-Specific Notes

- Prefer `rg` / `rg --files` for repo search while working on changes.
- Keep patches focused and avoid committing generated artifacts under `target/`.
- Before handing off, run `cargo fmt` and `cargo clippy -- -D warnings`.

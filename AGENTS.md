# Repository Guidelines

## Project Structure

This is a Cargo workspace with several crates:

### `crates/wit/` — main CLI
- `src/cli.rs`: Primary CLI binary entrypoint (`wit`). Contains all subcommand definitions (clap derive) and display/output logic.
- `src/lib.rs`: Library crate root; exposes `gitops`, `sed`, `search`, `search_run`, and `snapshot`.
- `src/search.rs`: GitHub repository search (`GitHubSearchClient`, octocrab), raw query assembly, and limit-aware pagination for `wit search`.
- `src/search_run.rs`: `wit search` orchestration for GitHub-only repository discovery and result shaping.
- `src/gitops/`: Git operations module for bare-repo caching, file access, tree display, directory listing, head/tail, and ripgrep-style search (`ops.rs`, `mod.rs`).
- `src/snapshot/`: Disk adapter + memory helpers (`memory_ops.rs`) for the shared `wit-snapshot` open/list/tree/read/search contract; CLI `--backend memory|disk` for tree/ls/cat/rg/sed/head/tail, cache pin, and branches.
- `src/sed.rs`: POSIX-style sed parser and execution engine for `wit sed`. ~1140 lines including 25+ unit tests.

### `crates/wit-snapshot/` — no-FS memory snapshots (+ wasm32 fetch client)
- `src/lib.rs`: `SnapshotBackend` / `RepoSnapshot` traits and shared provenance types.
- `src/memory.rs`: `MemoryBackend<C: GitHubHttpClient>` — sole no-FS snapshot impl (`open` / `list` / `tree` / `read`).
- `src/fetch.rs` + `src/wasm_abi.rs` (wasm32 only): `FetchGitHubClient` via host `http_get`; exports `open` / `list` / `read` + typed errors.
- Native `ReqwestGitHubClient` stays behind the `http` feature; wasm32 builds use `--no-default-features` (no reqwest).
- `src/bin/wit_nofS_demo.rs`: Live + fixture demo harness (`cargo run -p wit-snapshot --features demo --bin wit-nofS-demo`).
- `demo/browser/`: in-page fixture demo of the three exports. `wit-snapshot-wasmtime-fixture`: CI evidence the module runs (not browser-ready).
- `tests/memory_backend.rs`: Wiremock + failure-case coverage (rate limit, private, oversized, binary, OOM budget).
- Docs: `docs/adr/0004-wasm-fetch-snapshot-client.md` + `docs/adr/0004-wasm-fetch-howto.md`.

### `crates/wits/` — grep.app client crate (also standalone CLI `wits`)
- `src/lib.rs`: Library root; re-exports `client`, `types`, `RepoListMetric`, and `print_search_results()`.
- `src/client.rs`: grep.app HTTP client (`GrepClient`) with configurable base URL for testing.
- `src/types.rs`: Serde response types and parsed result structs.
- `src/bin/main.rs`: Standalone `wits` CLI binary (clap `name` may still display as `wit-search` in help text).
- `tests/integration.rs`: VCR integration tests using wiremock with cassette recording/replay.
- `tests/cassettes/`: Recorded grep.app API responses (JSON fixtures).

### Top-level
- `tasks/`: Task/planning files (e.g., `sed.txt`).
- `README.md`: User-facing installation and usage examples.
- `docs/index.html` + `docs/try/`: static GitHub Pages try-it (`wit tree|ls|cat|rg|sed|head|tail` as host JS views over existing `wit_snapshot.wasm` open/list/read).
- `target/`: Build artifacts (ignored via `.gitignore`).

## CLI Subcommands

| Subcommand | Alias | Description | Key Flags |
|------------|-------|-------------|-----------|
| `search`   | `s`   | Repo discovery via the GitHub REST repository search API | `-p`, `-q`, `-l`, `-n` |
| `cache`    | `c`   | Clone (or refresh) a GitHub repo into local cache | `-r` / `--repo` |
| `tree`     | `t`   | Show file tree of a repo (or subtree) | `-r` / `--repo`, `-l` (line counts + token estimates) |
| `ls`       |       | List directory contents (non-recursive, one level) | `-r` / `--repo`, `-l` (line counts + token estimates) |
| `cat`      |       | Print file contents (POSIX cat flags: -n, -b, -s, -E, -T, -A) | `-r` / `--repo` |
| `rg`       |       | Ripgrep-style search within a cached repo | `-r` / `--repo`, `-l`, `-g`, `-C`, `--long` (sizes with `-l`) |
| `sed`      |       | POSIX-style sed on a file from a cached repo | `-r` / `--repo`, `-n`, `-e`, `-f` |
| `head`     |       | First N lines of a file (default: 10) | `-r` / `--repo`, `-n`, `-N` |
| `tail`     |       | Last N lines / from line N onward | `-r` / `--repo`, `-n`, `-p`, `-N` |

## Build, Test, and Development Commands

- `cargo build --workspace`: Compile all crates.
- `cargo run -p wit -- search -p "ratatui" -l "Rust" --limit 20`: Run the wit CLI from source.
- `cargo run -p wits -- -p "ratatui" -l "Rust"`: Run the standalone `wits` search CLI (grep.app only).
- `cargo install --path crates/wit`: Install `wit` locally from the working tree.
- `sh install.sh`: Install from GitHub release artifacts using the repository installer script.
- `cargo fmt --all`: Format code with rustfmt (standard Rust style).
- `cargo clippy --workspace --all-targets -- -D warnings`: Lint and treat warnings as errors.
- `cargo test --workspace`: Run unit tests. `cargo test -- --ignored` for integration tests (require network).
- `bash scripts/check_wit_search_migration.sh`: Enforce `wit search` stays GitHub-only and does not reintroduce grep.app wiring under `crates/wit/src`.
- `bash scripts/check_wit_snapshot_wasm.sh`: Build `wit-snapshot` for `wasm32-unknown-unknown` without reqwest; run wasmtime fixture smoke.
- `bash scripts/check_docs_site.sh`: Pages try-it parser/formatter tests plus fixture wasm smoke (`wit tree demo/repo`).
- `cargo test -p wits --test integration`: Run VCR replay tests for the `wits` crate.
- `cargo test -p wits --test integration -- --ignored`: Re-record VCR cassettes from real API.
- `cargo test -p wit --test search_github_live -- --ignored`: Optional live GitHub smoke test (`GITHUB_TOKEN` recommended).

## Coding Style & Naming Conventions

- Rust edition: **2024** (see `Cargo.toml`).
- Formatting: prefer `cargo fmt`; avoid manual alignment.
- Naming: modules/functions `snake_case`, types/traits `CamelCase`, constants `SCREAMING_SNAKE_CASE`.
- CLI: subcommands use `kebab-case`, flags use long `--kebab-case` and short `-x` where helpful.

## Testing Guidelines

- Unit tests are inline with `#[cfg(test)] mod tests { ... }` (36 unit tests in sed.rs and ops.rs).
- Integration tests are marked `#[ignore]` and require network access.
- Live `wits` grep.app tests can be rate-limited behind a Vercel Security Checkpoint; keep cassette replay coverage strict, but let live-only ignored tests exit cleanly when the service returns checkpoint HTML instead of JSON.
- `wit search` query tests should cover both raw qualifier passthrough and limit-aware pagination; prefer wiremock over live GitHub for this surface.
- Prefer tests that validate parsing and output formatting deterministically (use small, embedded fixtures).
- Cache concurrency has subprocess integration tests at `tests/cache_lock_integration.rs` (including 4x parallel `wit rg`); run them with `cargo test --test cache_lock_integration -- --ignored`.

## Commit & Pull Request Guidelines

- Commit history uses short, imperative subjects (e.g., "add ls command"); follow the same pattern:
  - Subject <= ~50 chars, present tense; add a body for rationale and edge cases.
- PRs should include:
  - A clear description of behavior changes and any user-visible output changes.
  - Repro commands (e.g., `wit search -p "..." -q "stars:>1000 archived:false" --limit 20`).
  - Updates to `README.md` when flags/output change.

## Security & Network Notes

- `wit search` uses the **GitHub REST API** only. `-q/--query` passes raw GitHub repository-search terms and qualifiers through to `q`, and `--limit` should bound paging work as well as output. Avoid adding tokens/secrets to logs or CLI output.
- Be mindful of rate limits and handle HTTP failures gracefully in `wits` client code (`crates/wits/src/client.rs`).
- Repos are cached as bare git repos in the system temp directory under `.wit/cache` (override with `WIT_CACHE_DIR`); the `cache_github_repo` function handles both initial caching and forced refresh.
- A failed `gix` fetch can leave a poisoned cache directory (for example unborn `HEAD`); cache logic should delete partial state before retrying, and can fall back to `git clone --bare --depth 1` when transport timeouts persist.
- Cache operations are serialized by a global cache lock file (`.cache.lock`) plus an in-process mutex; cache reads/writes should continue to route through `cache_github_repo` to preserve this safety.
- `wit rg` max-count semantics now mirror ripgrep: omit `-m` for unlimited matches, use `-m 0` to return no matches.

## Release Packaging

- `.github/workflows/release.yml` must run from a semver tag ref, build Linux/macOS/Windows artifacts on GitHub-hosted runners, publish the GitHub release, and then invoke `.github/workflows/publish-npm.yml` to publish npm from the release assets.
- Release assets are named `wit-<platform>-<arch>.tar.gz` (Unix) and `wit-<platform>-<arch>.zip` (Windows), plus `wit-checksums.txt`.
- Keep artifact naming in sync with `install.sh`; changes to one should update the other in the same patch.

## Agent-Specific Notes

- Prefer `rg` / `rg --files` for repo search while working on changes.
- Keep patches focused and avoid committing generated artifacts under `target/`.
- Before handing off, run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `bash scripts/check_wit_search_migration.sh`, `bash scripts/check_wit_snapshot_wasm.sh`, and `bash scripts/check_docs_site.sh`.
- The `sed` subcommand aims for broad POSIX coverage; update tests and docs alongside behavior changes.

# ADR 0008: AST-backed search with tree-sitter (CLI + MCP)

- Status: Accepted
- Date: 2026-09-04

## Context

`rg` answers "where does this text occur"; it cannot answer "what is defined
in this file and where does each definition end", "every call to `render`
that is not inside a test", or "all `impl` blocks for `Widget`". Agents
approximate those with regexes, read too much, and still get boundaries
wrong. ADR 0007 shipped a regex `outline` in the URL API precisely because
the wasm/Worker host cannot carry parser grammars; it also promised a real
parser in the Rust backend.

## Decision

Add tree-sitter to the `wit` crate (native binaries only) and expose it as:

- **CLI** `wit ast symbols [REPO] [PATH]` and `wit ast query 'QUERY' [REPO] [PATH]`,
  on both the disk and memory backends, with `--kind`, `--name`, `--glob`,
  `--lang`, `--max-files`, and `--json`.
- **MCP** `wit_ast` (Code Mode `codemode.wit.ast`) with `mode: "symbols" |
  "query"`, the usual `path` / `globs` / `exclude` filters, `language`,
  `query`, `kinds`, `name`, `max_files`, cursors, and byte budgets. Items carry
  the same provenance as every other tool (repo, commit, blob, path, lines).

Languages in this cut: Rust, Python, JavaScript, TypeScript, TSX, Go, Java,
C. Each has a built-in definition query (`crates/wit/src/ast.rs`); adding a
language means adding a grammar crate, an extension list, a definition query,
and a kind-label table, all covered by the unit test that compiles every
query.

### Semantics

- `symbols` returns definitions in source order with exact one-based start
  and end lines from the parse tree, `parent`/`depth` nesting, and the first
  line of the definition as `signature`. Rust `impl` blocks are named
  `Trait for Type` / `Type`. A trailing newline that belongs to a node (C
  `#define`) does not extend its end line.
- `query` compiles the query once (fail fast, before walking), then returns
  every capture of every match with `capture`, `pattern_index`,
  `match_index`, node kind, positions, and the first line of text. Predicates
  such as `#eq?` / `#match?` work.
- Files without a grammar, binary blobs, and sources over 4 MiB are skipped,
  not errors. Walks are bounded by `max_files` (CLI default 500; MCP default
  200, ceiling 1,000).
- Query mode needs a language: `--lang` / `language`, or a `PATH` that names a
  single source file.

### Why tree-sitter, and why not in the URL API

tree-sitter grammars are C, compile everywhere `wit` already builds (Linux,
macOS, Windows, musl via `cross`), and give incremental, error-tolerant
parses. The wasm snapshot crate targets `wasm32-unknown-unknown` with no C
toolchain and the Worker has a bundle-size budget; shipping eight grammars
there would multiply the module size by an order of magnitude and require a
Workers-compatible Emscripten runtime that cannot be validated in this
repository's CI. The URL API therefore keeps the regex `outline` and
documents it as a heuristic; agents that need exact structure use the CLI or
MCP.

## Consequences

Positive:

- Reading becomes precise: `symbols` → `read` with the returned range, instead
  of guessing boundaries from regex outlines.
- Structural questions are one call; results are deterministic and
  provenance-bearing like every other tool.
- Both backends and both surfaces share one implementation and one test
  suite (`ast::tests`, `tests/ast_cli_integration.rs`, and the `wit_ast`
  section of `tests/mcp_stdio.rs`).

Tradeoffs:

- Release binaries grow by the size of eight grammars (a few MB).
- Symbol kinds are per-language labels (`fn`, `def`, `method`, …), not a
  cross-language taxonomy; filters use the label as printed.
- Raw queries expose tree-sitter node names, which differ per grammar
  version; the built-in symbol queries pin the grammar crates and are tested.

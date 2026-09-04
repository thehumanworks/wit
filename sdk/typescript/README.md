# @nothumanwork/wit-sdk (TypeScript)

Zero-dependency client for the [wit URL API](../../showcase/url-api/README.md):
read any GitHub repository without cloning, with token budgets you control.
Works anywhere `fetch` exists (Node 18+, Deno, Bun, Workers, browsers).

```ts
import { WitClient } from "@nothumanwork/wit-sdk";

const wit = new WitClient({ token: process.env.GITHUB_TOKEN });
const repo = wit.repo("ratatui/ratatui"); // or wit.repo("o/r", "v0.29.0")

const stats = await repo.stats();                          // size / tokens / languages, no blob reads
const files = await repo.rgFiles("impl Widget for", { glob: "*.rs" });
const outline = await repo.outline(files.files[0].path);   // symbols with line ranges
const impl = outline.symbols.find((s) => s.kind === "impl");
const code = await repo.cat(files.files[0].path, { lines: [impl.line, impl.end_line] });
console.log(code.commit, code.text);                       // provenance travels with every result
```

Higher-level helpers that chain calls for you:

- `repo.readSymbol(path, name, { kind, padding })` → outline + cat range (2 requests).
- `repo.context(pattern, { window, maxSnippets, glob })` → rg + one cat window per file.
- `wit.search({ query: "terminal ui", lang: "rust" })` → find `owner/repo` first.
- `repo.text("tree", { path: "src", l: 1 })` → the CLI-identical plaintext for any verb.

Errors are `WitError` with `status`, `code`, `retryAfter` and `isRateLimited`;
pass `retries: 2` to have 429s retried while honouring `retry-after`.

## Develop

```bash
cd sdk/typescript
npm run check        # tsc --noEmit + node --test (Node 22+ strips types natively)
npm run build        # emits dist/ for publishing
```

The package is marked `private` until the first release; flip it and
`npm publish` from `dist/` when ready.

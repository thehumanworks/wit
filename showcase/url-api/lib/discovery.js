/**
 * Discovery payloads for the URL API host:
 *   GET /api              -> text/plain, the curls a human (or agent) can run
 *   GET /api/openapi.json -> OpenAPI 3 for every verb
 *   GET /api/llms.txt     -> agent guide: when to use which verb, budgets, provenance
 *
 * All are built from the request origin so `wrangler pages dev` advertises
 * localhost while the deployed Worker advertises its own host.
 */

import { outlineLanguages } from "./outline.js";
import {
  DEFAULT_COMMITS,
  DEFAULT_HEAD_LINES,
  DEFAULT_RG_MAX_FILES,
  DEFAULT_RG_MAX_MATCHES,
  DEFAULT_SEARCH_LIMIT,
  DEFAULT_STATS_LARGEST,
  MAX_COMMITS,
  MAX_RG_MAX_FILES,
  MAX_RG_MAX_MATCHES,
  MAX_SEARCH_LIMIT,
  MAX_STATS_LARGEST,
  REPO_VERBS,
} from "./routes.js";

/**
 * @param {string | URL} input
 * @returns {string}
 */
function originOf(input) {
  const url = input instanceof URL ? input : new URL(String(input), "http://local");
  return url.origin;
}

/** One-line summary per verb, in advertised order. */
export const VERB_SUMMARY = {
  tree: "Recursive file listing (files only, dirs implied).",
  ls: "One directory level.",
  cat: "File contents; ?lines=A-B reads a one-based inclusive range.",
  head: "First N lines of a file.",
  tail: "Last N lines, or from line N onward.",
  rg: "Bounded ripgrep-style regex search over text files.",
  stats: "Size, token estimate, language and directory breakdown from the tree alone (no blob reads).",
  outline: "Line-numbered symbol index for one file (regex heuristic, no AST).",
  refs: "Default branch, branches, and tags.",
  commits: "Recent commits, optionally for one path.",
  search: "GitHub repository search (find owner/repo for 'libraries that do X').",
};

/**
 * Plaintext discovery page: the exact commands to paste into a terminal.
 * @param {string | URL} input request URL
 * @returns {string}
 */
export function apiIndexText(input) {
  const origin = originOf(input);
  const line = (verb, query) => `curl "${origin}/api/${verb}/{owner}/{repo}${query}"`;
  return [
    "wit url-api — read GitHub repositories over plain URLs.",
    "The file path is always a query param; the response is plaintext (?format=json for JSON).",
    "",
    "orientation",
    `  ${line("stats", "")}                    # size / tokens / languages, no blob reads`,
    `  ${line("tree", "?path=&depth=")}        # recursive listing (?l=1 adds bytes + ~tokens)`,
    `  ${line("ls", "?path=")}                 # one level`,
    `  ${line("refs", "")}                     # branches + tags for ?ref=`,
    `  ${line("commits", "?path=&n=10")}       # recent history`,
    "reading",
    `  ${line("outline", "?path=")}            # symbols with line ranges -> pick what to read`,
    `  ${line("cat", "?path=&lines=120-180")}  # exact range (?n=1 numbers lines)`,
    `  ${line("head", "?path=&lines=40")}`,
    `  ${line("tail", "?path=&lines=40")}      # or ?plus=N to read from line N`,
    "searching",
    `  ${line("rg", "?q=PATTERN&path=&glob=*.rs&C=2")}   # ?l=1 files only, ?c=1 counts`,
    `  curl "${origin}/api/search?q=terminal+ui+language:rust&limit=10"`,
    "",
    "Every verb accepts ?ref=BRANCH|TAG|SHA and ?fresh=1; responses carry",
    "x-wit-commit / x-wit-ref / x-wit-cache headers. The same verbs also answer",
    "without the /api prefix.",
    `Agent guide: ${origin}/api/llms.txt`,
    `OpenAPI: ${origin}/api/openapi.json`,
    'Private repos / your own quota: -H "Authorization: Bearer $GITHUB_TOKEN"',
    "",
  ].join("\n");
}

/**
 * Agent-facing guide served at /api/llms.txt.
 * @param {string | URL} input
 */
export function llmsText(input) {
  const origin = originOf(input);
  const base = `${origin}/api`;
  return `# wit URL API

> Read any GitHub repository without cloning: plaintext (or JSON) over GET
> requests, pinned to an immutable commit, with token budgets you control.

Base URL: ${base}
OpenAPI: ${base}/openapi.json

## Rules of thumb for agents

1. Start with \`stats\`, not \`tree\`: it tells you how many tokens the repo (or a
   directory) would cost and which directories/languages dominate.
2. Use \`outline\` on a file before \`cat\`: it returns symbols with line ranges, so
   you can \`cat?lines=A-B\` exactly what you need.
3. Prefer \`rg?l=1\` (files only) to locate code, then \`rg?C=2\` on a narrowed
   \`path=\` or \`glob=\` for context. \`rg\` is bounded (max=${DEFAULT_RG_MAX_MATCHES} matches,
   max_files=${DEFAULT_RG_MAX_FILES} files by default; ceilings ${MAX_RG_MAX_MATCHES}/${MAX_RG_MAX_FILES}). Truncation is
   reported in a trailing \`# truncated:\` line (or \`truncated\` in JSON) — narrow
   the query instead of assuming a null result.
4. Pin what you read: every response carries \`x-wit-commit\`, \`x-wit-ref\` and
   \`x-wit-cache: hit|miss\`. Pass \`?ref=<sha>\` to reproduce a read later.
5. Add \`?format=json\` (or \`Accept: application/json\`) for structured output with
   the same provenance fields in the body.

## Verbs

Repository verbs take \`/{verb}/{owner}/{repo}\`; the file path is always \`?path=\`.

| verb | purpose | key params |
|------|---------|------------|
| stats | ${VERB_SUMMARY.stats} | path, largest (default ${DEFAULT_STATS_LARGEST}, max ${MAX_STATS_LARGEST}), ignore |
| tree | ${VERB_SUMMARY.tree} | path, depth, l=1 (bytes + ~tokens), ignore |
| ls | ${VERB_SUMMARY.ls} | path, l=1 |
| outline | ${VERB_SUMMARY.outline} | path (required), max_symbols |
| cat | ${VERB_SUMMARY.cat} | path (required), lines=A-B (or start=&end=), n=1 |
| head | ${VERB_SUMMARY.head} | path (required), lines (default ${DEFAULT_HEAD_LINES}), n=1 |
| tail | ${VERB_SUMMARY.tail} | path (required), lines (default ${DEFAULT_HEAD_LINES}), plus=N, n=1 |
| rg | ${VERB_SUMMARY.rg} | q (required), path, glob, i=1, S=1, w=1, v=1, l=1, c=1, C/B/A, max, max_files, ignore |
| refs | ${VERB_SUMMARY.refs} | — |
| commits | ${VERB_SUMMARY.commits} | path, n (default ${DEFAULT_COMMITS}, max ${MAX_COMMITS}), ref |
| search | ${VERB_SUMMARY.search} | q (raw GitHub query), p (name), lang, limit (default ${DEFAULT_SEARCH_LIMIT}, max ${MAX_SEARCH_LIMIT}), sort=stars\\|updated\\|forks\\|best |

Common params: \`ref=\` (alias \`branch=\`; branch, tag, or full commit SHA),
\`fresh=1\` (re-resolve the ref instead of serving the cached pin),
\`format=json\`, \`ignore=GLOB\` (repeatable).

Outline languages: ${outlineLanguages().join(", ")}.

## Examples

    curl "${base}/stats/ratatui/ratatui"
    curl "${base}/tree/ratatui/ratatui?path=src/widgets&depth=1&l=1"
    curl "${base}/outline/ratatui/ratatui?path=src/widgets/block.rs"
    curl "${base}/cat/ratatui/ratatui?path=src/widgets/block.rs&lines=1-60&n=1"
    curl "${base}/rg/ratatui/ratatui?q=impl%20Widget&glob=*.rs&l=1"
    curl "${base}/rg/ratatui/ratatui?q=fn%20render&path=src/widgets&C=2&max=20"
    curl "${base}/refs/ratatui/ratatui"
    curl "${base}/commits/ratatui/ratatui?path=src/lib.rs&n=5"
    curl "${base}/search?q=terminal%20ui&lang=rust&limit=5"
    curl -H "Accept: application/json" "${base}/ls/ratatui/ratatui?path=src"

## Errors and limits

Errors are \`error: <message>\` (or \`{"error","code","status"}\` in JSON).
HTTP 429 means GitHub's quota is exhausted for the credentials in use; the
body says whether that was the host's token or yours, \`retry-after\` says when
to retry, and sending your own \`Authorization: Bearer <token>\` always uses
your quota instead. Files over 1 MiB, binary files, and repositories whose
recursive tree GitHub truncates are refused with a clear message.

Content is pinned per request but cached for up to 24h per repo@ref; use
\`fresh=1\` when you need the branch head resolved now.

Token estimates are ~4 bytes per token from sizes alone (no blob reads) and
are meant for budgeting, not accounting.
`;
}

/**
 * @param {string} name
 * @param {string} description
 * @param {{ required?: boolean, type?: string, enumValues?: string[] }} [opts]
 */
function queryParam(name, description, opts = {}) {
  /** @type {Record<string, unknown>} */
  const schema = { type: opts.type ?? "string" };
  if (opts.enumValues) schema.enum = opts.enumValues;
  return {
    name,
    in: "query",
    required: Boolean(opts.required),
    description,
    schema,
  };
}

const OWNER_REPO_PARAMS = [
  {
    name: "owner",
    in: "path",
    required: true,
    description: "GitHub owner (user or org).",
    schema: { type: "string" },
  },
  {
    name: "repo",
    in: "path",
    required: true,
    description: "GitHub repository name.",
    schema: { type: "string" },
  },
];

const REF_PARAMS = [
  queryParam("ref", "Branch, tag, or full commit SHA. Defaults to the default branch."),
  queryParam("branch", "Alias for `ref`."),
];

const COMMON_SNAPSHOT_PARAMS = [
  ...REF_PARAMS,
  queryParam("format", "`text` (default, CLI plaintext) or `json`.", { enumValues: ["text", "json"] }),
  queryParam("fresh", "Set to `1` to re-resolve the ref instead of serving the cached pin.", { type: "boolean" }),
];

const IGNORE_PARAM = queryParam(
  "ignore",
  "Glob to exclude (repeatable or comma separated), like the CLI's --ignore.",
);
const NUMBER_PARAM = queryParam("n", "Set to `1` to number output lines.", { type: "boolean" });
const PATH_REQUIRED = queryParam("path", "Repo-relative file path. Required.", { required: true });

/** Per-verb OpenAPI operation shape (query params mirror parseRoute). */
const VERB_SPECS = {
  tree: {
    summary: VERB_SUMMARY.tree,
    params: [
      queryParam("path", "Subtree to start from. Defaults to the repository root."),
      queryParam("depth", "Maximum depth below `path` (non-negative integer).", { type: "integer" }),
      queryParam("l", "Set to `1` for byte sizes and token estimates.", { type: "boolean" }),
      IGNORE_PARAM,
    ],
  },
  ls: {
    summary: VERB_SUMMARY.ls,
    params: [
      queryParam("path", "Directory to list. Defaults to the repository root."),
      queryParam("l", "Set to `1` for byte sizes and token estimates.", { type: "boolean" }),
      IGNORE_PARAM,
    ],
  },
  cat: {
    summary: VERB_SUMMARY.cat,
    params: [
      PATH_REQUIRED,
      queryParam("lines", "One-based inclusive range `A-B` (also `A-`, `-B`)."),
      queryParam("start", "One-based first line (alternative to `lines`).", { type: "integer" }),
      queryParam("end", "One-based last line (alternative to `lines`).", { type: "integer" }),
      NUMBER_PARAM,
    ],
  },
  head: {
    summary: VERB_SUMMARY.head,
    params: [
      PATH_REQUIRED,
      queryParam("lines", `Number of lines (default ${DEFAULT_HEAD_LINES}).`, { type: "integer" }),
      NUMBER_PARAM,
    ],
  },
  tail: {
    summary: VERB_SUMMARY.tail,
    params: [
      PATH_REQUIRED,
      queryParam("lines", `Number of lines (default ${DEFAULT_HEAD_LINES}).`, { type: "integer" }),
      queryParam("plus", "Read from this one-based line to the end (like `tail -n +N`).", { type: "integer" }),
      NUMBER_PARAM,
    ],
  },
  rg: {
    summary: VERB_SUMMARY.rg,
    params: [
      queryParam("q", "Regex pattern (Rust/JavaScript syntax). Required.", { required: true }),
      queryParam("path", "Restrict the search to this file or directory."),
      queryParam("glob", "Git-style glob filter, e.g. `*.rs` or `src/**/*.ts`."),
      queryParam("i", "Case-insensitive.", { type: "boolean" }),
      queryParam("S", "Smart case: case-insensitive when the pattern is all lowercase.", { type: "boolean" }),
      queryParam("w", "Whole-word matches only.", { type: "boolean" }),
      queryParam("v", "Invert: lines that do not match.", { type: "boolean" }),
      queryParam("l", "Only list files with matches.", { type: "boolean" }),
      queryParam("c", "Only count matches per file.", { type: "boolean" }),
      queryParam("C", "Context lines before and after each match (max 50).", { type: "integer" }),
      queryParam("B", "Context lines before each match.", { type: "integer" }),
      queryParam("A", "Context lines after each match.", { type: "integer" }),
      queryParam("max", `Maximum matches (default ${DEFAULT_RG_MAX_MATCHES}, max ${MAX_RG_MAX_MATCHES}).`, { type: "integer" }),
      queryParam("max_files", `Maximum files scanned (default ${DEFAULT_RG_MAX_FILES}, max ${MAX_RG_MAX_FILES}).`, { type: "integer" }),
      queryParam("long", "With `l=1`, append line counts and token estimates.", { type: "boolean" }),
      IGNORE_PARAM,
    ],
  },
  stats: {
    summary: VERB_SUMMARY.stats,
    params: [
      queryParam("path", "Directory to summarize. Defaults to the repository root."),
      queryParam("largest", `How many largest files to list (default ${DEFAULT_STATS_LARGEST}, max ${MAX_STATS_LARGEST}).`, { type: "integer" }),
      IGNORE_PARAM,
    ],
  },
  outline: {
    summary: VERB_SUMMARY.outline,
    params: [
      PATH_REQUIRED,
      queryParam("max_symbols", "Symbol cap (default 2000).", { type: "integer" }),
    ],
  },
  refs: {
    summary: VERB_SUMMARY.refs,
    params: [queryParam("format", "`text` (default) or `json`.", { enumValues: ["text", "json"] })],
    noSnapshot: true,
  },
  commits: {
    summary: VERB_SUMMARY.commits,
    params: [
      queryParam("path", "Only commits touching this path."),
      queryParam("n", `Number of commits (default ${DEFAULT_COMMITS}, max ${MAX_COMMITS}).`, { type: "integer" }),
      ...REF_PARAMS,
      queryParam("format", "`text` (default) or `json`.", { enumValues: ["text", "json"] }),
    ],
    noSnapshot: true,
  },
};

const SEARCH_PARAMS = [
  queryParam("q", "Raw GitHub repository-search terms and qualifiers (e.g. `terminal ui language:rust stars:>100`)."),
  queryParam("p", "Repository name filter (adds `NAME in:name`)."),
  queryParam("lang", "Language qualifier (adds `language:X`)."),
  queryParam("limit", `Results to return (default ${DEFAULT_SEARCH_LIMIT}, max ${MAX_SEARCH_LIMIT}).`, { type: "integer" }),
  queryParam("sort", "Ordering (default `stars`).", { enumValues: ["stars", "updated", "forks", "best"] }),
  queryParam("format", "`text` (default) or `json`.", { enumValues: ["text", "json"] }),
];

const PLAINTEXT_SCHEMA = { "text/plain": { schema: { type: "string" } } };
const JSON_SCHEMA = { "application/json": { schema: { type: "object" } } };

const RESPONSES = {
  200: {
    description:
      "Plaintext output identical to the `wit` CLI (or JSON with `?format=json`). " +
      "Headers: x-wit-repo, x-wit-ref, x-wit-commit, x-wit-cache (hit|miss), x-wit-auth.",
    content: { ...PLAINTEXT_SCHEMA, ...JSON_SCHEMA },
  },
  400: {
    description: "Malformed route, bad owner/repo, bad parameter, or missing required query param.",
    content: PLAINTEXT_SCHEMA,
  },
  404: {
    description: "Repository, ref, or file not found (or not visible to the token).",
    content: PLAINTEXT_SCHEMA,
  },
  429: {
    description:
      "GitHub API quota exhausted for the credentials in use; `retry-after` says when. " +
      "Send `Authorization: Bearer <token>` to use your own quota.",
    content: PLAINTEXT_SCHEMA,
  },
};

/**
 * @param {string} verb
 * @param {boolean} prefixed whether this path carries the `/api` alias prefix
 */
function operation(verb, prefixed) {
  const spec = VERB_SPECS[verb];
  const params = spec.noSnapshot ? spec.params : [...COMMON_SNAPSHOT_PARAMS, ...spec.params];
  return {
    get: {
      operationId: prefixed ? `${verb}ViaApiPrefix` : verb,
      summary: spec.summary,
      description: prefixed
        ? `Identical to \`GET /${verb}/{owner}/{repo}\`; the \`/api\` prefix is an alias.`
        : spec.summary,
      parameters: [...OWNER_REPO_PARAMS, ...params],
      responses: RESPONSES,
    },
  };
}

/**
 * @param {boolean} prefixed
 */
function searchOperation(prefixed) {
  return {
    get: {
      operationId: prefixed ? "searchViaApiPrefix" : "search",
      summary: VERB_SUMMARY.search,
      description: prefixed
        ? "Identical to `GET /search`; the `/api` prefix is an alias."
        : VERB_SUMMARY.search,
      parameters: SEARCH_PARAMS,
      responses: RESPONSES,
    },
  };
}

/**
 * OpenAPI 3 document for every verb, each reachable with and without the
 * `/api` alias prefix.
 * @param {string | URL} input request URL
 */
export function openApiDocument(input) {
  const origin = originOf(input);
  /** @type {Record<string, unknown>} */
  const paths = {};
  for (const prefix of ["", "/api"]) {
    for (const verb of REPO_VERBS) {
      paths[`${prefix}/${verb}/{owner}/{repo}`] = operation(verb, prefix !== "");
    }
    paths[`${prefix}/search`] = searchOperation(prefix !== "");
  }

  return {
    openapi: "3.0.3",
    info: {
      title: "wit url-api",
      version: "2.0.0",
      description:
        "Read GitHub repositories over plain URLs. Responses are text/plain, " +
        "byte-for-byte the `wit` CLI output, or JSON with `?format=json`. " +
        "Every read is pinned to an immutable commit reported in x-wit-commit. " +
        "Authentication is optional and belongs in the `Authorization` header. " +
        `Agent guide: ${origin}/api/llms.txt`,
    },
    servers: [{ url: origin }],
    paths,
    components: {
      securitySchemes: {
        githubToken: {
          type: "http",
          scheme: "bearer",
          description:
            "GitHub token for private repositories and your own rate-limit quota. " +
            "Send it as `Authorization: Bearer <token>`.",
        },
      },
    },
    // `{}` first: every route also answers unauthenticated.
    security: [{}, { githubToken: [] }],
  };
}

/**
 * Discovery payloads for the URL API host:
 *   GET /api              -> text/plain, the three curls a human can run
 *   GET /api/openapi.json -> OpenAPI 3 for the same three verbs
 *
 * Both are built from the request origin so `wrangler pages dev` advertises
 * localhost while the deployed Worker advertises its own host.
 *
 * Only tree / ls / cat live on this host.
 */

/** Verbs in the order they are advertised. */
const VERB_ORDER = ["tree", "ls", "cat"];

/**
 * @param {string | URL} input
 * @returns {string}
 */
function originOf(input) {
  const url = input instanceof URL ? input : new URL(String(input), "http://local");
  return url.origin;
}

/**
 * Plaintext discovery page: the exact commands to paste into a terminal.
 * @param {string | URL} input request URL
 * @returns {string}
 */
export function apiIndexText(input) {
  const origin = originOf(input);
  return [
    "wit url-api — three verbs; the repo path is always a query param.",
    "",
    `curl ${origin}/api/tree/{owner}/{repo}`,
    `curl ${origin}/api/ls/{owner}/{repo}`,
    `curl ${origin}/api/cat/{owner}/{repo}?path=`,
    "",
    "The same three verbs also answer without the /api prefix.",
    `OpenAPI: ${origin}/api/openapi.json`,
    'Private repos: -H "Authorization: Bearer $GITHUB_TOKEN"',
    "",
  ].join("\n");
}

/**
 * @param {string} name
 * @param {string} description
 * @param {boolean} [required]
 */
function queryParam(name, description, required = false) {
  return {
    name,
    in: "query",
    required,
    description,
    schema: { type: "string" },
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

const BRANCH_PARAMS = [
  queryParam("branch", "Branch, tag, or commit-ish. Defaults to the default branch."),
  queryParam("ref", "Alias for `branch`."),
];

/** Per-verb OpenAPI operation shape (query params mirror parseRoute). */
const VERB_SPECS = {
  tree: {
    summary: "Recursive file tree as plaintext.",
    params: [
      queryParam("path", "Subtree to start from. Defaults to the repository root."),
      ...BRANCH_PARAMS,
      queryParam("depth", "Maximum depth below `path` (non-negative integer)."),
      queryParam("l", "Set to `1` for line counts and token estimates."),
      queryParam("long", "Presence-only alias for `l`."),
    ],
  },
  ls: {
    summary: "One directory level as plaintext.",
    params: [
      queryParam("path", "Directory to list. Defaults to the repository root."),
      ...BRANCH_PARAMS,
      queryParam("l", "Set to `1` for sizes."),
      queryParam("long", "Presence-only alias for `l`."),
    ],
  },
  cat: {
    summary: "File contents as plaintext.",
    params: [
      queryParam("path", "Repo-relative file path. Required.", true),
      ...BRANCH_PARAMS,
      queryParam("n", "Set to `1` to number output lines."),
      queryParam("number", "Presence-only alias for `n`."),
    ],
  },
};

const PLAINTEXT_RESPONSES = {
  200: {
    description: "Plaintext output, identical to the `wit` CLI.",
    content: { "text/plain": { schema: { type: "string" } } },
  },
  400: {
    description: "Malformed route, bad owner/repo, or missing required query param.",
    content: { "text/plain": { schema: { type: "string" } } },
  },
  404: {
    description: "Repository, ref, or file not found (or not visible to the token).",
    content: { "text/plain": { schema: { type: "string" } } },
  },
};

/**
 * @param {'tree'|'ls'|'cat'} verb
 * @param {boolean} prefixed whether this path carries the `/api` alias prefix
 */
function operation(verb, prefixed) {
  const spec = VERB_SPECS[verb];
  return {
    get: {
      operationId: prefixed ? `${verb}ViaApiPrefix` : verb,
      summary: spec.summary,
      description: prefixed
        ? `Identical to \`GET /${verb}/{owner}/{repo}\`; the \`/api\` prefix is an alias.`
        : spec.summary,
      parameters: [...OWNER_REPO_PARAMS, ...spec.params],
      responses: PLAINTEXT_RESPONSES,
    },
  };
}

/**
 * OpenAPI 3 document for the three verbs, each reachable with and without
 * the `/api` alias prefix.
 * @param {string | URL} input request URL
 */
export function openApiDocument(input) {
  const origin = originOf(input);
  /** @type {Record<string, unknown>} */
  const paths = {};
  for (const verb of VERB_ORDER) {
    paths[`/${verb}/{owner}/{repo}`] = operation(verb, false);
  }
  for (const verb of VERB_ORDER) {
    paths[`/api/${verb}/{owner}/{repo}`] = operation(verb, true);
  }

  return {
    openapi: "3.0.3",
    info: {
      title: "wit url-api",
      version: "1.0.0",
      description:
        "Read GitHub repositories over plain URLs. Responses are text/plain, " +
        "byte-for-byte the `wit` CLI output. Authentication is optional and " +
        "belongs in the `Authorization` header.",
    },
    servers: [{ url: origin }],
    paths,
    components: {
      securitySchemes: {
        githubToken: {
          type: "http",
          scheme: "bearer",
          description:
            "GitHub token for private repositories and higher rate limits. " +
            "Send it as `Authorization: Bearer <token>`.",
        },
      },
    },
    // `{}` first: every route also answers unauthenticated.
    security: [{}, { githubToken: [] }],
  };
}

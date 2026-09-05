/**
 * URL routing for the wit URL API.
 *
 * Repository verbs (`{owner}/{repo}` in the path, everything else a query
 * param — the file path is never a path segment):
 *
 *   GET /tree/{owner}/{repo}?path=&branch=&depth=&l=
 *   GET /ls/{owner}/{repo}?path=&branch=&l=
 *   GET /cat/{owner}/{repo}?path=&lines=A-B&n=          (path required)
 *   GET /head/{owner}/{repo}?path=&lines=10&n=          (path required)
 *   GET /tail/{owner}/{repo}?path=&lines=10&plus=&n=    (path required)
 *   GET /rg/{owner}/{repo}?q=&path=&glob=&i=&l=&c=&C=   (q required)
 *   GET /stats/{owner}/{repo}?path=&largest=
 *   GET /outline/{owner}/{repo}?path=                   (path required)
 *   GET /refs/{owner}/{repo}
 *   GET /commits/{owner}/{repo}?path=&n=
 *
 * Repository discovery (no owner/repo):
 *
 *   GET /search?q=&p=&lang=&limit=&sort=
 *
 * Every route also answers under a leading `/api` prefix; discovery routes
 * live only under it:
 *
 *   GET /api               -> plaintext curl list
 *   GET /api/openapi.json  -> OpenAPI 3 document
 *   GET /api/llms.txt      -> agent guide
 *
 * `?ref=` aliases `branch` and accepts a branch, tag, or full commit SHA.
 * `?format=json` (or `Accept: application/json`) switches to JSON.
 */

import { SafeError, scrubSecrets } from "./auth.js";
import { parseLineRange } from "./textops.js";

/** Verbs that open a snapshot (`/{verb}/{owner}/{repo}`). */
export const SNAPSHOT_VERBS = ["tree", "ls", "cat", "head", "tail", "rg", "stats", "outline"];
/** Verbs that only need the GitHub REST metadata endpoints. */
export const METADATA_VERBS = ["refs", "commits"];
/** All `/{verb}/{owner}/{repo}` verbs in advertised order. */
export const REPO_VERBS = [...SNAPSHOT_VERBS, ...METADATA_VERBS];
/** Verbs that require `?path=` to name a file. */
export const FILE_VERBS = new Set(["cat", "head", "tail", "outline"]);

const VERBS = new Set(REPO_VERBS);
const API_PREFIX = "api";
const SEARCH_VERB = "search";

export const DEFAULT_HEAD_LINES = 10;
export const DEFAULT_RG_MAX_MATCHES = 200;
export const MAX_RG_MAX_MATCHES = 2000;
export const DEFAULT_RG_MAX_FILES = 200;
export const MAX_RG_MAX_FILES = 1000;
export const DEFAULT_STATS_LARGEST = 10;
export const MAX_STATS_LARGEST = 100;
export const DEFAULT_COMMITS = 10;
export const MAX_COMMITS = 100;
export const DEFAULT_SEARCH_LIMIT = 10;
export const MAX_SEARCH_LIMIT = 100;
export const DEFAULT_OUTLINE_SYMBOLS = 2000;
export const MAX_OUTLINE_SYMBOLS = 10000;
/** Longest accepted rg pattern; bounds regex compile/backtracking cost. */
export const MAX_RG_PATTERN_LENGTH = 512;

/** Discovery routes, keyed by the segment that follows `/api`. */
const META_KINDS = new Map([
  ["", "api-index"],
  ["openapi.json", "openapi"],
  ["llms.txt", "llms"],
]);

/**
 * Split a pathname into route segments, dropping one optional leading `api`
 * alias segment so prefixed and unprefixed URLs share a single parse.
 * @param {string} pathname
 * @returns {{ parts: string[], prefixed: boolean }}
 */
function routeSegments(pathname) {
  const parts = String(pathname).replace(/\/+$/, "").split("/").filter(Boolean);
  if (parts.length > 0 && parts[0].toLowerCase() === API_PREFIX) {
    return { parts: parts.slice(1), prefixed: true };
  }
  return { parts, prefixed: false };
}

/**
 * @param {{ parts: string[], prefixed: boolean }} segments
 * @returns {'api-index' | 'openapi' | 'llms' | null}
 */
function metaKind({ parts, prefixed }) {
  if (!prefixed || parts.length > 1) return null;
  return META_KINDS.get((parts[0] ?? "").toLowerCase()) ?? null;
}

/**
 * @param {string} component
 */
export function isSafeRepoComponent(component) {
  return (
    typeof component === "string" &&
    component.length > 0 &&
    component !== "." &&
    component !== ".." &&
    /^[A-Za-z0-9._-]+$/.test(component)
  );
}

/**
 * Normalize a repo-relative path (query form).
 * @param {string | null | undefined} value
 */
export function normalizePath(value) {
  if (value == null || value === "") return "";
  return String(value)
    .trim()
    .replace(/\\/g, "/")
    .replace(/^\.\//, "")
    .replace(/^\/+/, "")
    .replace(/\/+$/, "");
}

/**
 * Presence-or-truthy query flag: `?l`, `?l=1`, `?l=true` are on; `?l=0`,
 * `?l=false` are off.
 * @param {URLSearchParams} q
 * @param {string[]} keys
 */
function flag(q, ...keys) {
  for (const key of keys) {
    if (!q.has(key)) continue;
    const v = String(q.get(key) ?? "").trim().toLowerCase();
    return !(v === "0" || v === "false" || v === "no");
  }
  return false;
}

/**
 * Bounded non-negative integer query param.
 * @param {URLSearchParams} q
 * @param {string} key
 * @param {{ fallback: number | null, min?: number, max?: number, code: string }} spec
 */
function integer(q, key, spec) {
  if (!q.has(key) || String(q.get(key)).trim() === "") return spec.fallback;
  const n = Number(q.get(key));
  const min = spec.min ?? 0;
  if (!Number.isInteger(n) || n < min) {
    throw new SafeError(`${key} must be an integer >= ${min}`, { status: 400, code: spec.code });
  }
  return spec.max != null ? Math.min(n, spec.max) : n;
}

/**
 * Output format from `?format=` or the Accept header.
 * @param {URLSearchParams} q
 * @param {string | null | undefined} accept
 * @returns {'text' | 'json'}
 */
export function outputFormat(q, accept) {
  if (q.has("format")) {
    const v = String(q.get("format") ?? "").trim().toLowerCase();
    if (v === "json") return "json";
    if (v === "text" || v === "" || v === "plain") return "text";
    throw new SafeError(`format must be text or json, got '${v}'`, {
      status: 400,
      code: "bad_format",
    });
  }
  if (accept && /\bapplication\/json\b/i.test(accept) && !/\btext\/plain\b/i.test(accept)) {
    return "json";
  }
  return "text";
}

/**
 * Parse request URL into a route action.
 * Returns null for non-API paths (static assets, `/`, etc.).
 *
 * @param {string | URL} input
 * @param {{ accept?: string | null }} [opts]
 * @returns {null
 *   | { kind: 'api-index' | 'openapi' | 'llms' }
 *   | { kind: 'search', format: 'text'|'json', query: string | null, pattern: string | null, lang: string | null, limit: number, sort: string }
 *   | RepoRoute}
 */
export function parseRoute(input, opts = {}) {
  const url = input instanceof URL ? input : new URL(String(input), "http://local");
  const segments = routeSegments(url.pathname);
  const meta = metaKind(segments);
  if (meta) return { kind: meta };

  const { parts } = segments;
  if (parts.length === 0) return null;

  const verb = parts[0].toLowerCase();
  const q = url.searchParams;

  if (verb === SEARCH_VERB) {
    if (parts.length !== 1) {
      throw new SafeError("expected /search?q= (repository search takes no owner/repo)", {
        status: 400,
        code: "bad_route",
      });
    }
    const format = outputFormat(q, opts.accept);
    const query = (q.get("q") || q.get("query") || "").trim() || null;
    const pattern = (q.get("p") || q.get("pattern") || "").trim() || null;
    const lang = (q.get("lang") || q.get("language") || "").trim() || null;
    if (!query && !pattern && !lang) {
      throw new SafeError("search requires ?q= (raw GitHub query), ?p= (name), or ?lang=", {
        status: 400,
        code: "query_required",
      });
    }
    const sort = String(q.get("sort") || "stars").trim().toLowerCase();
    if (!["stars", "updated", "forks", "best"].includes(sort)) {
      throw new SafeError("sort must be stars, updated, forks, or best", {
        status: 400,
        code: "bad_sort",
      });
    }
    return {
      kind: "search",
      format,
      query,
      pattern,
      lang,
      limit: integer(q, "limit", { fallback: DEFAULT_SEARCH_LIMIT, min: 1, max: MAX_SEARCH_LIMIT, code: "bad_limit" }),
      sort,
    };
  }

  if (!VERBS.has(verb)) return null;
  if (parts.length !== 3) {
    throw new SafeError(
      `expected /${verb}/{owner}/{repo} (path belongs in ?path=)`,
      { status: 400, code: "bad_route" },
    );
  }

  const owner = parts[1];
  const repo = parts[2];
  if (!isSafeRepoComponent(owner) || !isSafeRepoComponent(repo)) {
    throw new SafeError(`invalid owner/repo: ${owner}/${repo}`, {
      status: 400,
      code: "bad_repo",
    });
  }

  const format = outputFormat(q, opts.accept);
  const path = normalizePath(q.get("path"));
  const branchRaw = q.get("branch") || q.get("ref") || null;
  const branch = branchRaw && branchRaw.trim() ? branchRaw.trim() : null;
  const ignore = q
    .getAll("ignore")
    .flatMap((v) => String(v).split(","))
    .map((v) => v.trim())
    .filter(Boolean);

  if (FILE_VERBS.has(verb) && !path) {
    throw new SafeError(`${verb} requires ?path=`, { status: 400, code: "path_required" });
  }

  /** @type {RepoRoute} */
  const route = {
    kind: "repo",
    verb,
    owner,
    repo,
    ownerRepo: `${owner}/${repo}`,
    path,
    branch,
    format,
    fresh: flag(q, "fresh", "refresh"),
    ignore,
    depth: null,
    long: false,
    number: false,
  };

  switch (verb) {
    case "tree":
      route.depth = integer(q, "depth", { fallback: null, code: "bad_depth" });
      route.long = flag(q, "l", "long");
      break;
    case "ls":
      route.long = flag(q, "l", "long");
      break;
    case "cat": {
      route.number = flag(q, "n", "number");
      let range = null;
      try {
        range = parseLineRange(q.get("lines"));
      } catch (err) {
        throw new SafeError(String(err.message || err), { status: 400, code: "bad_lines" });
      }
      const start = integer(q, "start", { fallback: null, min: 1, code: "bad_lines" });
      const end = integer(q, "end", { fallback: null, min: 1, code: "bad_lines" });
      if (start != null || end != null) {
        range = { start: start ?? range?.start ?? null, end: end ?? range?.end ?? null };
        if (range.start != null && range.end != null && range.end < range.start) {
          throw new SafeError(`lines end (${range.end}) is before start (${range.start})`, {
            status: 400,
            code: "bad_lines",
          });
        }
      }
      route.lines = range;
      break;
    }
    case "head":
      route.number = flag(q, "n", "number", "N");
      route.count = integer(q, "lines", { fallback: DEFAULT_HEAD_LINES, code: "bad_lines" });
      break;
    case "tail":
      route.number = flag(q, "n", "number", "N");
      route.count = integer(q, "lines", { fallback: DEFAULT_HEAD_LINES, code: "bad_lines" });
      route.fromLine = integer(q, "plus", { fallback: null, min: 1, code: "bad_plus" });
      break;
    case "rg": {
      const pattern = q.get("q") ?? q.get("pattern") ?? q.get("query");
      if (pattern == null || pattern === "") {
        throw new SafeError("rg requires ?q=PATTERN (Rust/JS regex)", {
          status: 400,
          code: "pattern_required",
        });
      }
      if (pattern.length > MAX_RG_PATTERN_LENGTH) {
        throw new SafeError(`rg pattern is longer than ${MAX_RG_PATTERN_LENGTH} characters`, {
          status: 400,
          code: "pattern_too_long",
        });
      }
      route.pattern = pattern;
      route.glob = (q.get("glob") || q.get("g") || "").trim() || null;
      route.ignoreCase = flag(q, "i", "ignore_case");
      route.smartCase = flag(q, "S", "smart_case");
      route.wordRegexp = flag(q, "w", "word");
      route.invert = flag(q, "v", "invert");
      route.filesWithMatches = flag(q, "l", "files");
      route.countOnly = flag(q, "c", "count");
      route.long = flag(q, "long");
      const context = integer(q, "C", { fallback: 0, max: 50, code: "bad_context" });
      route.before = integer(q, "B", { fallback: context, max: 50, code: "bad_context" });
      route.after = integer(q, "A", { fallback: context, max: 50, code: "bad_context" });
      route.maxMatches = integer(q, "max", { fallback: DEFAULT_RG_MAX_MATCHES, min: 1, max: MAX_RG_MAX_MATCHES, code: "bad_max" });
      route.maxFiles = integer(q, "max_files", { fallback: DEFAULT_RG_MAX_FILES, min: 1, max: MAX_RG_MAX_FILES, code: "bad_max" });
      break;
    }
    case "stats":
      route.largest = integer(q, "largest", { fallback: DEFAULT_STATS_LARGEST, max: MAX_STATS_LARGEST, code: "bad_largest" });
      break;
    case "outline":
      route.maxSymbols = integer(q, "max_symbols", { fallback: DEFAULT_OUTLINE_SYMBOLS, min: 1, max: MAX_OUTLINE_SYMBOLS, code: "bad_max" });
      break;
    case "commits":
      route.count = integer(q, "n", { fallback: DEFAULT_COMMITS, min: 1, max: MAX_COMMITS, code: "bad_n" });
      break;
    default:
      break;
  }

  return route;
}

/**
 * @typedef {{
 *   kind: 'repo',
 *   verb: string,
 *   owner: string,
 *   repo: string,
 *   ownerRepo: string,
 *   path: string,
 *   branch: string | null,
 *   format: 'text' | 'json',
 *   fresh: boolean,
 *   ignore: string[],
 *   depth: number | null,
 *   long: boolean,
 *   number: boolean,
 *   lines?: { start: number | null, end: number | null } | null,
 *   count?: number,
 *   fromLine?: number | null,
 *   pattern?: string,
 *   glob?: string | null,
 *   ignoreCase?: boolean,
 *   smartCase?: boolean,
 *   wordRegexp?: boolean,
 *   invert?: boolean,
 *   filesWithMatches?: boolean,
 *   countOnly?: boolean,
 *   before?: number,
 *   after?: number,
 *   maxMatches?: number,
 *   maxFiles?: number,
 *   largest?: number,
 *   maxSymbols?: number,
 * }} RepoRoute
 */

/**
 * Whether this pathname belongs to the API host: a known verb (even if
 * malformed) with or without the `/api` prefix, or an `/api` discovery route.
 * @param {string} pathname
 */
export function isApiPath(pathname) {
  const segments = routeSegments(pathname);
  if (metaKind(segments)) return true;
  const first = segments.parts[0];
  if (first == null) return false;
  const verb = first.toLowerCase();
  return VERBS.has(verb) || verb === SEARCH_VERB;
}

/**
 * Build a safe error response body (never includes tokens).
 * @param {unknown} err
 * @param {{ format?: 'text' | 'json' }} [opts]
 */
export function errorBody(err, opts = {}) {
  const status = err && typeof err === "object" && "status" in err ? Number(err.status) || 500 : 500;
  const message = scrubSecrets(
    err instanceof Error ? err.message : String(err ?? "error"),
  );
  const code = err && typeof err === "object" && "code" in err ? String(err.code || "") : "";
  const retryAfter =
    err && typeof err === "object" && "retryAfterSeconds" in err && err.retryAfterSeconds != null
      ? Number(err.retryAfterSeconds)
      : null;
  if (opts.format === "json") {
    return {
      status,
      retryAfter,
      body: `${JSON.stringify({ error: message, code: code || "error", status })}\n`,
    };
  }
  return { status, retryAfter, body: `error: ${message}\n` };
}

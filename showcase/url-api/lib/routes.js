/**
 * URL routing for the three-verb showcase API.
 *
 * ONLY:
 *   GET /tree/{owner}/{repo}?path=&branch=&depth=
 *   GET /ls/{owner}/{repo}?path=&branch=
 *   GET /cat/{owner}/{repo}?path=   (path required)
 *
 * ?ref= aliases branch. Path is always a query param (never a path segment).
 */

import { SafeError, scrubSecrets } from "./auth.js";

const VERBS = new Set(["tree", "ls", "cat"]);

/** Known query keys per verb (unknown keys are ignored). */
const KNOWN_KEYS = {
  tree: new Set(["path", "branch", "ref", "depth", "l", "long", "token", "access_token", "ttl", "ttlMs"]),
  ls: new Set(["path", "branch", "ref", "l", "long", "token", "access_token", "ttl", "ttlMs"]),
  cat: new Set(["path", "branch", "ref", "n", "b", "number", "token", "access_token", "ttl", "ttlMs"]),
};

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
 * Parse request URL into a route action.
 * Returns null for non-API paths (static assets, `/`, etc.).
 *
 * @param {string | URL} input
 * @returns {null | {
 *   verb: 'tree'|'ls'|'cat',
 *   owner: string,
 *   repo: string,
 *   ownerRepo: string,
 *   path: string,
 *   branch: string | null,
 *   depth: number | null,
 *   long: boolean,
 *   number: boolean,
 * }}
 */
export function parseRoute(input) {
  const url = input instanceof URL ? input : new URL(String(input), "http://local");
  const parts = url.pathname.replace(/\/+$/, "").split("/").filter(Boolean);
  if (parts.length === 0) return null;

  const verb = parts[0].toLowerCase();
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

  const q = url.searchParams;
  const path = normalizePath(q.get("path"));
  const branch = q.get("branch") || q.get("ref") || null;
  let depth = null;
  if (q.has("depth")) {
    const n = Number(q.get("depth"));
    if (!Number.isFinite(n) || n < 0 || !Number.isInteger(n)) {
      throw new SafeError("depth must be a non-negative integer", {
        status: 400,
        code: "bad_depth",
      });
    }
    depth = n;
  }

  const long = q.get("l") === "1" || q.get("l") === "true" || q.has("long");
  const number = q.get("n") === "1" || q.get("n") === "true" || q.has("number");

  if (verb === "cat" && !path) {
    throw new SafeError("cat requires ?path=", { status: 400, code: "path_required" });
  }

  // Ignore unknown keys (consistent, curl-friendly). Documented in howto.
  void KNOWN_KEYS[verb];

  return {
    verb,
    owner,
    repo,
    ownerRepo: `${owner}/${repo}`,
    path,
    branch: branch && branch.trim() ? branch.trim() : null,
    depth,
    long,
    number,
  };
}

/**
 * Whether this pathname is one of the three API verbs (even if malformed).
 * @param {string} pathname
 */
export function isApiPath(pathname) {
  const first = String(pathname).replace(/\/+$/, "").split("/").filter(Boolean)[0];
  return first != null && VERBS.has(first.toLowerCase());
}

/**
 * Build a safe error response body (never includes tokens).
 * @param {unknown} err
 */
export function errorBody(err) {
  const status = err && typeof err === "object" && "status" in err ? Number(err.status) || 500 : 500;
  const message = scrubSecrets(
    err instanceof Error ? err.message : String(err ?? "error"),
  );
  return { status, body: `error: ${message}\n` };
}

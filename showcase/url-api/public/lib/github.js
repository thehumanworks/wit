/**
 * Async GitHub REST prefetch for the sync wasm http_get import.
 *
 * Prefetch fills RepoSnapshotCache so wit_snapshot_open / list / read only
 * hit the host cache (imports are synchronous). Same MemoryBackend; host
 * adapter only sits in front of get_json.
 *
 * Rate limits: GitHub answers anonymous callers with HTTP 403 (primary limit,
 * 60 requests/hour per egress IP — always exhausted on shared Worker IPs) or
 * HTTP 429 (secondary limits). Both are surfaced as a 429 SafeError with
 * `retryAfterSeconds`, never as a misleading "access denied".
 */

import { SafeError, githubAuthHeader, scrubSecrets } from "./auth.js";
import { DEFAULT_TTL_MS, RepoSnapshotCache, reconstructBlobJson } from "./repo-cache.js";

const API = "https://api.github.com";
const RAW = "https://raw.githubusercontent.com";
const USER_AGENT = "wit-url-api (+https://github.com/thehumanworks/wit)";
/** Largest blob the wasm MemoryBackend will decode (DEFAULT_MAX_BLOB_BYTES). */
export const MAX_BLOB_BYTES = 1_048_576;

/**
 * @typedef {{
 *   status: number,
 *   body: string,
 *   rateLimitRemaining: number | null,
 *   rateLimitReset: number | null,
 *   retryAfter: number | null,
 * }} GitHubResponse
 */

/**
 * @param {Headers | null | undefined} headers
 * @param {string} name
 */
function numericHeader(headers, name) {
  const raw = headers && typeof headers.get === "function" ? headers.get(name) : null;
  if (raw == null || raw === "") return null;
  const n = Number(raw);
  return Number.isFinite(n) ? n : null;
}

/**
 * Seconds until the primary rate limit resets, or null when unknown.
 * @param {GitHubResponse} res
 * @param {() => number} [now]
 */
export function rateLimitRetryAfterSeconds(res, now = () => Date.now()) {
  if (res.retryAfter != null && res.retryAfter >= 0) return Math.ceil(res.retryAfter);
  if (res.rateLimitReset != null) {
    return Math.max(1, Math.ceil(res.rateLimitReset - now() / 1000));
  }
  return null;
}

/**
 * Whether a GitHub response is a primary or secondary rate-limit rejection
 * (as opposed to a genuine 403 for a private repo).
 * @param {GitHubResponse} res
 */
export function isRateLimited(res) {
  if (res.status === 429) return true;
  if (res.status !== 403) return false;
  if (res.rateLimitRemaining === 0) return true;
  if (res.retryAfter != null) return true;
  return /rate limit/i.test(res.body || "");
}

/**
 * Build the 429 SafeError for a rate-limited response.
 * @param {GitHubResponse} res
 * @param {{ tokenSource?: 'caller' | 'host' | 'anonymous' }} [opts]
 */
export function rateLimitError(res, opts = {}) {
  const retry = rateLimitRetryAfterSeconds(res);
  const source = opts.tokenSource ?? "anonymous";
  // Wording avoids "token <word>" / "Bearer <word>" so scrubSecrets leaves it intact.
  const hint =
    source === "anonymous"
      ? "This host has no GitHub credential configured; send your own in the Authorization header to use your quota"
      : source === "host"
        ? "The host credential's quota is exhausted; send your own in the Authorization header to use your quota"
        : "Your credential's GitHub quota is exhausted";
  const when = retry != null ? ` (resets in ${retry}s)` : "";
  const err = new SafeError(`GitHub API rate limit exceeded${when}. ${hint}.`, {
    status: 429,
    code: "rate_limited",
  });
  err.retryAfterSeconds = retry;
  return err;
}

/**
 * @param {string} path relative `/repos/...` or absolute
 * @param {string | null} token
 * @returns {Promise<GitHubResponse>}
 */
export async function githubGet(path, token) {
  const url = path.startsWith("http://") || path.startsWith("https://")
    ? path
    : `${API}${path.startsWith("/") ? path : `/${path}`}`;
  /** @type {Record<string, string>} */
  const headers = {
    Accept: "application/vnd.github+json",
    "User-Agent": USER_AGENT,
  };
  const auth = githubAuthHeader(token);
  if (auth) headers.Authorization = auth;

  let res;
  try {
    res = await fetch(url, { headers });
  } catch (err) {
    throw new SafeError(`GitHub fetch failed: ${scrubSecrets(String(err))}`, {
      status: 502,
      code: "github_fetch",
    });
  }
  const body = await res.text();
  return {
    status: res.status,
    body,
    rateLimitRemaining: numericHeader(res.headers, "x-ratelimit-remaining"),
    rateLimitReset: numericHeader(res.headers, "x-ratelimit-reset"),
    retryAfter: numericHeader(res.headers, "retry-after"),
  };
}

/**
 * Translate a non-2xx GitHub response into a SafeError. Rate limits win over
 * everything so an exhausted quota is never reported as "private repo".
 *
 * @param {GitHubResponse} res
 * @param {{ notFound: string, label: string, tokenSource?: 'caller'|'host'|'anonymous' }} ctx
 */
export function githubFailure(res, ctx) {
  if (isRateLimited(res)) return rateLimitError(res, { tokenSource: ctx.tokenSource });
  if (res.status === 404) {
    return new SafeError(ctx.notFound, { status: 404, code: "not_found" });
  }
  if (res.status === 401) {
    // "credentials", not "token": the log scrubber redacts `token <word>`.
    return new SafeError("GitHub rejected the supplied credentials (HTTP 401)", {
      status: 401,
      code: "bad_token",
    });
  }
  if (res.status === 403) {
    return new SafeError(
      `GitHub API denied access for ${ctx.label} (HTTP 403); credentials with access may be required`,
      { status: 403, code: "forbidden" },
    );
  }
  return new SafeError(`GitHub API ${ctx.label} returned HTTP ${res.status}`, {
    status: 502,
    code: "github_status",
  });
}

/**
 * Prefetch repo → commit → recursive tree into the host cache.
 *
 * @param {RepoSnapshotCache} cache
 * @param {string} ownerRepo
 * @param {string | null} branch
 * @param {string | null} token
 * @param {{ tokenSource?: 'caller'|'host'|'anonymous', ttlMs?: number | null }} [opts]
 * @returns {Promise<{ ref: string, treeSha: string, commitSha: string, cached: boolean }>}
 */
export async function prefetchOpen(cache, ownerRepo, branch, token, opts = {}) {
  // A live open entry (isolate-warm or hydrated from persistent cache) can
  // serve the whole repo → commit → tree sequence; skip GitHub entirely.
  const cached = cache.findOpenEntry(ownerRepo, branch ?? undefined);
  if (cached) {
    return {
      ref: cached.requestedRef,
      treeSha: cached.treeSha,
      commitSha: cached.commitSha,
      cached: true,
    };
  }
  const tokenSource = opts.tokenSource ?? (token ? "caller" : "anonymous");
  const ttlMs = opts.ttlMs ?? null;

  const repoPath = `/repos/${ownerRepo}`;
  const repoRes = await githubGet(repoPath, token);
  if (repoRes.status < 200 || repoRes.status >= 300) {
    throw githubFailure(repoRes, {
      notFound: `repository '${ownerRepo}' was not found`,
      label: repoPath,
      tokenSource,
    });
  }

  // Seed cache via getOrFetch so pending/open sequence matches browser demo.
  cache.getOrFetch(repoPath, () => repoRes, { ttlMs });
  cache.getOrFetch(`${API}${repoPath}`, () => repoRes, { ttlMs });

  let ref = branch;
  if (!ref) {
    const meta = JSON.parse(repoRes.body);
    ref = String(meta.default_branch || "main");
  }

  const commitPath = `/repos/${ownerRepo}/commits/${encodeURIComponent(ref)}`;
  const commitRes = await githubGet(commitPath, token);
  if (commitRes.status === 422 && !isRateLimited(commitRes)) {
    throw new SafeError(`ref '${ref}' not found in '${ownerRepo}'`, {
      status: 404,
      code: "ref_not_found",
    });
  }
  if (commitRes.status < 200 || commitRes.status >= 300) {
    throw githubFailure(commitRes, {
      notFound: `ref '${ref}' not found in '${ownerRepo}'`,
      label: "commits",
      tokenSource,
    });
  }
  cache.getOrFetch(commitPath, () => commitRes, { ttlMs });
  cache.getOrFetch(`${API}${commitPath}`, () => commitRes, { ttlMs });

  const commit = JSON.parse(commitRes.body);
  const treeSha = commit?.commit?.tree?.sha;
  if (!treeSha) {
    throw new SafeError("commit response missing tree sha", {
      status: 502,
      code: "bad_commit",
    });
  }

  const treePath = `/repos/${ownerRepo}/git/trees/${treeSha}?recursive=1`;
  const treeRes = await githubGet(treePath, token);
  if (treeRes.status < 200 || treeRes.status >= 300) {
    throw githubFailure(treeRes, {
      notFound: `tree ${treeSha} not found in '${ownerRepo}'`,
      label: "tree",
      tokenSource,
    });
  }
  cache.getOrFetch(treePath, () => treeRes, { ttlMs });
  cache.getOrFetch(`${API}${treePath}`, () => treeRes, { ttlMs });

  return { ref, treeSha, commitSha: String(commit.sha || ""), cached: false };
}

/**
 * Base64 for arbitrary bytes without blowing the call stack on large blobs.
 * @param {Uint8Array} bytes
 */
export function bytesToBase64(bytes) {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/**
 * Fetch a blob's bytes from raw.githubusercontent.com. Raw content is pinned
 * by commit SHA, so the result is as immutable as the blob endpoint, but it
 * does not consume the REST API quota. Returns null on any failure so the
 * caller can fall back to the blob endpoint.
 *
 * @param {string} ownerRepo
 * @param {string} commitSha
 * @param {string} filePath
 * @param {string | null} token
 * @returns {Promise<{ size: number, contentBase64: string } | null>}
 */
export async function fetchRawBlob(ownerRepo, commitSha, filePath, token) {
  if (!/^[0-9a-f]{40}$/i.test(commitSha)) return null;
  const encodedPath = filePath.split("/").map(encodeURIComponent).join("/");
  const url = `${RAW}/${ownerRepo}/${commitSha}/${encodedPath}`;
  /** @type {Record<string, string>} */
  const headers = { "User-Agent": USER_AGENT };
  const auth = githubAuthHeader(token);
  if (auth) headers.Authorization = auth;
  let res;
  try {
    res = await fetch(url, { headers });
  } catch {
    return null;
  }
  if (res.status !== 200) return null;
  const declared = numericHeader(res.headers, "content-length");
  if (declared != null && declared > MAX_BLOB_BYTES) return null;
  const bytes = new Uint8Array(await res.arrayBuffer());
  if (bytes.length > MAX_BLOB_BYTES) return null;
  return { size: bytes.length, contentBase64: bytesToBase64(bytes) };
}

/**
 * Prefetch a blob by sha into the host cache (needed before wit_snapshot_read).
 *
 * When `opts.path` and `opts.commitSha` are known and `opts.raw` is not
 * false, the bytes come from raw.githubusercontent.com first (no REST quota
 * spent) and the blob endpoint is only used as a fallback.
 *
 * @param {RepoSnapshotCache} cache
 * @param {string} ownerRepo
 * @param {string} blobSha
 * @param {string | null} token
 * @param {{
 *   path?: string,
 *   commitSha?: string,
 *   raw?: boolean,
 *   tokenSource?: 'caller'|'host'|'anonymous',
 *   ttlMs?: number | null,
 * }} [opts]
 * @returns {Promise<'cache' | 'raw' | 'api'>}
 */
export async function prefetchBlob(cache, ownerRepo, blobSha, token, opts = {}) {
  const blobPath = `/repos/${ownerRepo}/git/blobs/${blobSha}`;
  const hit = cache.findEntryWithBlob(ownerRepo, blobSha);
  if (hit) return "cache";
  const ttlMs = opts.ttlMs ?? null;

  if (opts.raw !== false && opts.path && opts.commitSha) {
    const raw = await fetchRawBlob(ownerRepo, opts.commitSha, opts.path, token);
    if (raw) {
      const body = reconstructBlobJson(blobSha, raw);
      cache.getOrFetch(blobPath, () => ({ status: 200, body }), { ttlMs });
      return "raw";
    }
  }

  const blobRes = await githubGet(blobPath, token);
  if (blobRes.status < 200 || blobRes.status >= 300) {
    throw githubFailure(blobRes, {
      notFound: `blob ${blobSha} not found in '${ownerRepo}'`,
      label: "blob",
      tokenSource: opts.tokenSource ?? (token ? "caller" : "anonymous"),
    });
  }
  cache.getOrFetch(blobPath, () => blobRes, { ttlMs });
  cache.getOrFetch(`${API}${blobPath}`, () => blobRes, { ttlMs });
  return "api";
}

/**
 * Resolve blob sha for a file path from the cached slim tree.
 * @param {RepoSnapshotCache} cache
 * @param {string} ownerRepo
 * @param {string | null} branch
 * @param {string} filePath
 */
export function blobShaForPath(cache, ownerRepo, branch, filePath) {
  const entry = cache.findEntry(ownerRepo, branch ?? undefined);
  if (!entry) return null;
  const want = filePath.replace(/^\/+|\/+$/g, "");
  const row = entry.tree.find((e) => e.type === "blob" && e.path === want);
  return row ? row.sha : null;
}

/**
 * Slim tree rows (blobs and trees) for the open entry, or an empty array.
 * @param {RepoSnapshotCache} cache
 * @param {string} ownerRepo
 * @param {string | null} branch
 */
export function treeRowsFor(cache, ownerRepo, branch) {
  const entry = cache.findOpenEntry(ownerRepo, branch ?? undefined);
  return entry ? entry.tree : [];
}

/**
 * Create a host cache (24h default; worker = isolate lifetime).
 * @param {{ ttlMs?: number }} [opts]
 */
export function createHostCache(opts = {}) {
  return new RepoSnapshotCache({ ttlMs: opts.ttlMs ?? DEFAULT_TTL_MS });
}

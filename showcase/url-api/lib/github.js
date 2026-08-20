/**
 * Async GitHub REST prefetch for the sync wasm http_get import.
 *
 * Prefetch fills RepoSnapshotCache so wit_snapshot_open / list / read only
 * hit the host cache (imports are synchronous). Same MemoryBackend; host
 * adapter only sits in front of get_json.
 */

import { SafeError, githubAuthHeader, scrubSecrets } from "./auth.js";
import { DEFAULT_TTL_MS, RepoSnapshotCache } from "./repo-cache.js";

const API = "https://api.github.com";

/**
 * @param {string} path relative `/repos/...` or absolute
 * @param {string | null} token
 * @returns {Promise<{ status: number, body: string }>}
 */
export async function githubGet(path, token) {
  const url = path.startsWith("http://") || path.startsWith("https://")
    ? path
    : `${API}${path.startsWith("/") ? path : `/${path}`}`;
  /** @type {Record<string, string>} */
  const headers = {
    Accept: "application/vnd.github+json",
    "User-Agent": "wit-url-api-showcase (+https://github.com/thehumanworks/wit)",
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
  return { status: res.status, body };
}

/**
 * Prefetch repo → commit → recursive tree into the host cache.
 * @param {RepoSnapshotCache} cache
 * @param {string} ownerRepo
 * @param {string | null} branch
 * @param {string | null} token
 */
export async function prefetchOpen(cache, ownerRepo, branch, token) {
  const repoPath = `/repos/${ownerRepo}`;
  const repoRes = await githubGet(repoPath, token);
  if (repoRes.status === 404) {
    throw new SafeError(`repository '${ownerRepo}' was not found`, {
      status: 404,
      code: "not_found",
    });
  }
  if (repoRes.status === 401 || repoRes.status === 403) {
    throw new SafeError(
      `GitHub API rejected access to '${ownerRepo}' (HTTP ${repoRes.status})`,
      { status: repoRes.status, code: "forbidden" },
    );
  }
  if (repoRes.status < 200 || repoRes.status >= 300) {
    throw new SafeError(`GitHub API ${repoPath} returned HTTP ${repoRes.status}`, {
      status: 502,
      code: "github_status",
    });
  }

  // Seed cache via getOrFetch so pending/open sequence matches browser demo.
  cache.getOrFetch(repoPath, () => repoRes);
  cache.getOrFetch(`https://api.github.com${repoPath}`, () => repoRes);

  let ref = branch;
  if (!ref) {
    const meta = JSON.parse(repoRes.body);
    ref = String(meta.default_branch || "main");
  }

  const commitPath = `/repos/${ownerRepo}/commits/${encodeURIComponent(ref)}`;
  const commitRes = await githubGet(commitPath, token);
  if (commitRes.status === 404) {
    throw new SafeError(`ref '${ref}' not found in '${ownerRepo}'`, {
      status: 404,
      code: "ref_not_found",
    });
  }
  if (commitRes.status < 200 || commitRes.status >= 300) {
    throw new SafeError(
      `GitHub API commits returned HTTP ${commitRes.status}`,
      { status: 502, code: "github_status" },
    );
  }
  cache.getOrFetch(commitPath, () => commitRes);
  cache.getOrFetch(`https://api.github.com${commitPath}`, () => commitRes);

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
    throw new SafeError(`GitHub API tree returned HTTP ${treeRes.status}`, {
      status: 502,
      code: "github_status",
    });
  }
  cache.getOrFetch(treePath, () => treeRes);
  cache.getOrFetch(`https://api.github.com${treePath}`, () => treeRes);

  return { ref, treeSha, commitSha: String(commit.sha || "") };
}

/**
 * Prefetch a blob by sha into the host cache (needed before wit_snapshot_read).
 * @param {RepoSnapshotCache} cache
 * @param {string} ownerRepo
 * @param {string} blobSha
 * @param {string | null} token
 */
export async function prefetchBlob(cache, ownerRepo, blobSha, token) {
  const blobPath = `/repos/${ownerRepo}/git/blobs/${blobSha}`;
  const hit = cache.findEntryWithBlob(ownerRepo, blobSha);
  if (hit) return;
  const blobRes = await githubGet(blobPath, token);
  if (blobRes.status < 200 || blobRes.status >= 300) {
    throw new SafeError(`GitHub API blob returned HTTP ${blobRes.status}`, {
      status: blobRes.status === 404 ? 404 : 502,
      code: "blob_status",
    });
  }
  cache.getOrFetch(blobPath, () => blobRes);
  cache.getOrFetch(`https://api.github.com${blobPath}`, () => blobRes);
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
 * Create a host cache (24h default; worker = isolate lifetime).
 * @param {{ ttlMs?: number }} [opts]
 */
export function createHostCache(opts = {}) {
  return new RepoSnapshotCache({ ttlMs: opts.ttlMs ?? DEFAULT_TTL_MS });
}

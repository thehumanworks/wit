var __defProp = Object.defineProperty;
var __name = (target, value) => __defProp(target, "name", { value, configurable: true });

// .wrangler/tmp/pages-OOJUIb/bundledWorker-0.6668071131924698.mjs
import wasmModule from "./8badd8744fc02309024c3ea9d2ac8c50e0477598-8badd8744fc02309024c3ea9d2ac8c50e0477598-wit_snapshot.wasm";
var __defProp2 = Object.defineProperty;
var __name2 = /* @__PURE__ */ __name((target, value) => __defProp2(target, "name", { value, configurable: true }), "__name");
var TOKEN_QUERY_KEYS = /* @__PURE__ */ new Set(["token", "access_token"]);
function scrubSecrets(text) {
  if (!text) return text;
  let out = String(text);
  out = out.replace(
    /([?&](?:token|access_token)=)([^&#\s]*)/gi,
    "$1[REDACTED]"
  );
  out = out.replace(
    /(Authorization\s*:\s*(?:Bearer|token)\s+)(\S+)/gi,
    "$1[REDACTED]"
  );
  out = out.replace(/\b(?:github_pat_|ghp_|gho_|ghu_|ghs_|ghr_)[A-Za-z0-9_]+/g, "[REDACTED]");
  return out;
}
__name(scrubSecrets, "scrubSecrets");
__name2(scrubSecrets, "scrubSecrets");
var SafeError = class extends Error {
  static {
    __name(this, "SafeError");
  }
  static {
    __name2(this, "SafeError");
  }
  /**
   * @param {string} message
   * @param {{ status?: number, code?: string }} [opts]
   */
  constructor(message, opts = {}) {
    super(scrubSecrets(message));
    this.name = "SafeError";
    this.status = opts.status ?? 500;
    this.code = opts.code;
  }
};
function extractToken(req) {
  const headers = req.headers;
  let auth = null;
  if (headers && typeof headers.get === "function") {
    auth = headers.get("Authorization") || headers.get("authorization");
  } else if (headers && typeof headers === "object") {
    auth = headers.Authorization || headers.authorization || headers.AUTHORIZATION || null;
  }
  if (auth) {
    const m = String(auth).match(/^\s*(?:Bearer|token)\s+(\S+)\s*$/i);
    if (m) return m[1];
    const raw = String(auth).trim();
    if (raw && !/\s/.test(raw)) return raw;
  }
  const url = req.url ? new URL(String(req.url), "http://local") : null;
  if (url) {
    for (const key of TOKEN_QUERY_KEYS) {
      const v = url.searchParams.get(key);
      if (v) return v;
    }
  }
  return null;
}
__name(extractToken, "extractToken");
__name2(extractToken, "extractToken");
function githubAuthHeader(token) {
  if (!token) return null;
  return `Bearer ${token}`;
}
__name(githubAuthHeader, "githubAuthHeader");
__name2(githubAuthHeader, "githubAuthHeader");
function formatLs(entries, opts = {}) {
  if (!entries || entries.length === 0) {
    return "Directory is empty or does not exist.";
  }
  const long = !!opts.long;
  const lines = [];
  for (const entry of entries) {
    const isDir = entry.kind === "dir";
    if (long) {
      if (isDir) {
        lines.push(`            ${entry.name}/`);
      } else if (entry.size_bytes != null) {
        lines.push(`${String(entry.size_bytes).padStart(8)} B  ${entry.name}`);
      } else {
        lines.push(`            ${entry.name}`);
      }
    } else if (isDir) {
      lines.push(`${entry.name}/`);
    } else {
      lines.push(entry.name);
    }
  }
  return lines.join("\n");
}
__name(formatLs, "formatLs");
__name2(formatLs, "formatLs");
function formatTree(view, opts = {}) {
  const base = (opts.path || "").replace(/\/+$/, "");
  const depth = opts.depth == null ? null : opts.depth;
  const long = !!opts.long;
  const lines = [view.root || (base ? base.split("/").pop() : ".")];
  for (const entry of view.entries) {
    if (entry.kind === "dir") continue;
    let relative;
    if (base) {
      if (entry.path === base) {
        relative = entry.path.split("/").pop() || entry.path;
      } else if (entry.path.startsWith(base + "/")) {
        relative = entry.path.slice(base.length + 1);
      } else {
        relative = entry.path;
      }
    } else {
      relative = entry.path;
    }
    if (!relative) continue;
    if (depth != null) {
      const parts = relative.split("/").filter(Boolean);
      if (parts.length > depth) continue;
    }
    const label = long && entry.size_bytes != null ? `${relative} (${entry.size_bytes} B)` : relative;
    lines.push(`  ${label}`);
  }
  return lines.join("\n");
}
__name(formatTree, "formatTree");
__name2(formatTree, "formatTree");
function formatCat(text, opts = {}) {
  if (!opts.number) {
    const lines2 = text.split("\n");
    if (lines2.length && lines2[lines2.length - 1] === "") lines2.pop();
    return lines2.join("\n");
  }
  const lines = text.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  return lines.map((line, i) => `${String(i + 1).padStart(6)}  ${line}`).join("\n");
}
__name(formatCat, "formatCat");
__name2(formatCat, "formatCat");
var DEFAULT_TTL_MS = 24 * 60 * 60 * 1e3;
function normalizeApiPath(path) {
  if (path.startsWith("https://api.github.com")) {
    return path.slice("https://api.github.com".length) || "/";
  }
  if (path.startsWith("http://api.github.com")) {
    return path.slice("http://api.github.com".length) || "/";
  }
  return path;
}
__name(normalizeApiPath, "normalizeApiPath");
__name2(normalizeApiPath, "normalizeApiPath");
function parseGitHubApiPath(path) {
  const p = normalizeApiPath(path);
  let m = p.match(/^\/repos\/([^/]+\/[^/]+)\/git\/blobs\/([^/?]+)/);
  if (m) {
    return { kind: "blob", ownerRepo: m[1], blobSha: decodeURIComponent(m[2]) };
  }
  m = p.match(/^\/repos\/([^/]+\/[^/]+)\/git\/trees\/([^/?]+)(?:\?(.*))?$/);
  if (m) {
    return { kind: "tree", ownerRepo: m[1], treeSha: decodeURIComponent(m[2]) };
  }
  m = p.match(/^\/repos\/([^/]+\/[^/]+)\/commits\/([^/?]+)/);
  if (m) {
    return { kind: "commit", ownerRepo: m[1], ref: decodeURIComponent(m[2]) };
  }
  m = p.match(/^\/repos\/([^/]+\/[^/]+)\/?$/);
  if (m) {
    return { kind: "repo", ownerRepo: m[1] };
  }
  return { kind: "other", ownerRepo: null };
}
__name(parseGitHubApiPath, "parseGitHubApiPath");
__name2(parseGitHubApiPath, "parseGitHubApiPath");
function repoCacheKey(ownerRepo, resolvedRef) {
  return `${ownerRepo}@${resolvedRef}`;
}
__name(repoCacheKey, "repoCacheKey");
__name2(repoCacheKey, "repoCacheKey");
function resolveRefName(requested) {
  if (/^[0-9a-f]{40}$/i.test(requested) || requested.startsWith("refs/")) {
    return requested;
  }
  return `refs/heads/${requested}`;
}
__name(resolveRefName, "resolveRefName");
__name2(resolveRefName, "resolveRefName");
function slimTreeFromGitHubJson(body) {
  const parsed = JSON.parse(body);
  const tree = Array.isArray(parsed.tree) ? parsed.tree : [];
  return tree.filter((e) => e && (e.type === "blob" || e.type === "tree") && typeof e.path === "string").map((e) => {
    const out = { path: e.path, type: e.type, sha: String(e.sha ?? "") };
    if (typeof e.size === "number") out.size = e.size;
    return out;
  });
}
__name(slimTreeFromGitHubJson, "slimTreeFromGitHubJson");
__name2(slimTreeFromGitHubJson, "slimTreeFromGitHubJson");
function slimBlobFromGitHubJson(body) {
  const parsed = JSON.parse(body);
  const size = Number(parsed.size ?? 0);
  let contentBase64;
  if (parsed.encoding === "base64") {
    contentBase64 = String(parsed.content ?? "").replace(/\s+/g, "");
  } else if (parsed.encoding === "utf-8") {
    contentBase64 = bytesToBase64(new TextEncoder().encode(String(parsed.content ?? "")));
  } else {
    throw new Error(`unsupported blob encoding: ${parsed.encoding}`);
  }
  return { size, contentBase64 };
}
__name(slimBlobFromGitHubJson, "slimBlobFromGitHubJson");
__name2(slimBlobFromGitHubJson, "slimBlobFromGitHubJson");
function bytesToBase64(bytes) {
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
}
__name(bytesToBase64, "bytesToBase64");
__name2(bytesToBase64, "bytesToBase64");
function reconstructRepoJson(entry) {
  return JSON.stringify({
    private: entry.private,
    default_branch: entry.defaultBranch
  });
}
__name(reconstructRepoJson, "reconstructRepoJson");
__name2(reconstructRepoJson, "reconstructRepoJson");
function reconstructCommitJson(entry) {
  return JSON.stringify({
    sha: entry.commitSha,
    commit: { tree: { sha: entry.treeSha } }
  });
}
__name(reconstructCommitJson, "reconstructCommitJson");
__name2(reconstructCommitJson, "reconstructCommitJson");
function reconstructTreeJson(entry) {
  return JSON.stringify({
    sha: entry.treeSha,
    truncated: false,
    tree: entry.tree.map((e) => {
      const row = { path: e.path, type: e.type, sha: e.sha, mode: e.type === "tree" ? "040000" : "100644" };
      if (typeof e.size === "number") row.size = e.size;
      return row;
    })
  });
}
__name(reconstructTreeJson, "reconstructTreeJson");
__name2(reconstructTreeJson, "reconstructTreeJson");
function reconstructBlobJson(sha, blob) {
  return JSON.stringify({
    sha,
    size: blob.size,
    encoding: "base64",
    content: blob.contentBase64
  });
}
__name(reconstructBlobJson, "reconstructBlobJson");
__name2(reconstructBlobJson, "reconstructBlobJson");
var RepoSnapshotCache = class {
  static {
    __name(this, "RepoSnapshotCache");
  }
  static {
    __name2(this, "RepoSnapshotCache");
  }
  /**
   * @param {{
   *   ttlMs?: number,
   *   now?: () => number,
   * }} [opts]
   */
  constructor(opts = {}) {
    this.ttlMs = opts.ttlMs ?? DEFAULT_TTL_MS;
    this.now = opts.now ?? (() => Date.now());
    this.entries = /* @__PURE__ */ new Map();
    this.pending = /* @__PURE__ */ new Map();
    this.lastOutcome = null;
  }
  setTtlMs(ttlMs) {
    this.ttlMs = Math.max(0, Number(ttlMs) || 0);
  }
  /**
   * @param {RepoCacheEntry} entry
   */
  remainingMs(entry) {
    const expiresAt = entry.cachedAt + entry.ttlMs;
    return Math.max(0, expiresAt - this.now());
  }
  /**
   * @param {RepoCacheEntry} entry
   */
  isExpired(entry) {
    return this.remainingMs(entry) <= 0;
  }
  /**
   * Drop expired entries; return whether the given key was removed.
   * @param {string} [key]
   */
  invalidateExpired(key) {
    if (key) {
      const entry = this.entries.get(key);
      if (entry && this.isExpired(entry)) {
        this.entries.delete(key);
        return true;
      }
      return false;
    }
    for (const [k, entry] of [...this.entries]) {
      if (this.isExpired(entry)) this.entries.delete(k);
    }
    return false;
  }
  /**
   * @param {string} ownerRepo
   * @param {string} [requestedRef]
   * @returns {RepoCacheEntry | null}
   */
  findEntry(ownerRepo, requestedRef) {
    this.invalidateExpired();
    for (const entry of this.entries.values()) {
      if (entry.ownerRepo !== ownerRepo) continue;
      if (requestedRef == null) return entry;
      if (entry.requestedRef === requestedRef || entry.resolvedRef === requestedRef || entry.resolvedRef === resolveRefName(requestedRef) || entry.commitSha === requestedRef) {
        return entry;
      }
    }
    return null;
  }
  /**
   * @param {string} ownerRepo
   * @param {string} treeSha
   */
  findEntryByTreeSha(ownerRepo, treeSha) {
    this.invalidateExpired();
    for (const entry of this.entries.values()) {
      if (entry.ownerRepo === ownerRepo && entry.treeSha === treeSha) return entry;
    }
    return null;
  }
  /**
   * @param {string} ownerRepo
   * @param {string} blobSha
   */
  findEntryWithBlob(ownerRepo, blobSha) {
    this.invalidateExpired();
    for (const entry of this.entries.values()) {
      if (entry.ownerRepo === ownerRepo && entry.blobs[blobSha]) return entry;
    }
    return null;
  }
  /**
   * Status rows for the demo UI.
   */
  statusRows() {
    this.invalidateExpired();
    return [...this.entries.values()].map((entry) => ({
      key: repoCacheKey(entry.ownerRepo, entry.resolvedRef),
      ownerRepo: entry.ownerRepo,
      resolvedRef: entry.resolvedRef,
      remainingMs: this.remainingMs(entry),
      ttlMs: entry.ttlMs,
      treeEntries: entry.tree.length,
      blobCount: Object.keys(entry.blobs).length
    }));
  }
  /**
   * Serve from cache or call `fetchFn` on miss/expiry.
   * `fetchFn` must return `{ status: number, body: string }` synchronously.
   *
   * @param {string} path
   * @param {(path: string) => { status: number, body: string } | null} fetchFn
   * @returns {{ status: number, body: string, outcome: 'hit' | 'miss', repoKey: string | null, remainingMs: number | null } | null}
   */
  getOrFetch(path, fetchFn) {
    const parsed = parseGitHubApiPath(path);
    if (parsed.kind === "other" || !parsed.ownerRepo) {
      const raw = fetchFn(path);
      if (!raw) return null;
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return { ...raw, outcome: "miss", repoKey: null, remainingMs: null };
    }
    if (parsed.kind === "blob") {
      return this.#handleBlob(path, parsed.ownerRepo, parsed.blobSha, fetchFn);
    }
    if (parsed.kind === "tree") {
      return this.#handleTree(path, parsed.ownerRepo, parsed.treeSha, fetchFn);
    }
    if (parsed.kind === "commit") {
      return this.#handleCommit(path, parsed.ownerRepo, parsed.ref, fetchFn);
    }
    if (parsed.kind === "repo") {
      return this.#handleRepo(path, parsed.ownerRepo, fetchFn);
    }
    return null;
  }
  /**
   * @param {string} path
   * @param {string} ownerRepo
   * @param {(path: string) => { status: number, body: string } | null} fetchFn
   */
  #handleRepo(path, ownerRepo, fetchFn) {
    const hit = this.findEntry(ownerRepo);
    if (hit) {
      const remainingMs = this.remainingMs(hit);
      const repoKey = repoCacheKey(hit.ownerRepo, hit.resolvedRef);
      this.lastOutcome = { path, outcome: "hit", repoKey, remainingMs };
      return {
        status: 200,
        body: reconstructRepoJson(hit),
        outcome: "hit",
        repoKey,
        remainingMs
      };
    }
    const raw = fetchFn(path);
    if (!raw || raw.status !== 200) {
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return raw ? { ...raw, outcome: "miss", repoKey: null, remainingMs: null } : null;
    }
    const meta = JSON.parse(raw.body);
    const pending = this.pending.get(ownerRepo) ?? { ownerRepo };
    pending.private = !!meta.private;
    pending.defaultBranch = String(meta.default_branch ?? "main");
    this.pending.set(ownerRepo, pending);
    this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
    return { ...raw, outcome: "miss", repoKey: null, remainingMs: null };
  }
  /**
   * @param {string} path
   * @param {string} ownerRepo
   * @param {string} ref
   * @param {(path: string) => { status: number, body: string } | null} fetchFn
   */
  #handleCommit(path, ownerRepo, ref, fetchFn) {
    const hit = this.findEntry(ownerRepo, ref);
    if (hit) {
      const remainingMs = this.remainingMs(hit);
      const repoKey = repoCacheKey(hit.ownerRepo, hit.resolvedRef);
      this.lastOutcome = { path, outcome: "hit", repoKey, remainingMs };
      return {
        status: 200,
        body: reconstructCommitJson(hit),
        outcome: "hit",
        repoKey,
        remainingMs
      };
    }
    const raw = fetchFn(path);
    if (!raw || raw.status !== 200) {
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return raw ? { ...raw, outcome: "miss", repoKey: null, remainingMs: null } : null;
    }
    const commit = JSON.parse(raw.body);
    const pending = this.pending.get(ownerRepo) ?? { ownerRepo };
    pending.requestedRef = ref;
    pending.resolvedRef = resolveRefName(ref);
    pending.commitSha = String(commit.sha ?? "");
    pending.treeSha = String(commit.commit?.tree?.sha ?? "");
    if (pending.defaultBranch == null) pending.defaultBranch = ref;
    if (pending.private == null) pending.private = false;
    this.pending.set(ownerRepo, pending);
    this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
    return { ...raw, outcome: "miss", repoKey: null, remainingMs: null };
  }
  /**
   * @param {string} path
   * @param {string} ownerRepo
   * @param {string} treeSha
   * @param {(path: string) => { status: number, body: string } | null} fetchFn
   */
  #handleTree(path, ownerRepo, treeSha, fetchFn) {
    const hit = this.findEntryByTreeSha(ownerRepo, treeSha);
    if (hit) {
      const remainingMs2 = this.remainingMs(hit);
      const repoKey = repoCacheKey(hit.ownerRepo, hit.resolvedRef);
      this.lastOutcome = { path, outcome: "hit", repoKey, remainingMs: remainingMs2 };
      return {
        status: 200,
        body: reconstructTreeJson(hit),
        outcome: "hit",
        repoKey,
        remainingMs: remainingMs2
      };
    }
    const raw = fetchFn(path);
    if (!raw || raw.status !== 200) {
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return raw ? { ...raw, outcome: "miss", repoKey: null, remainingMs: null } : null;
    }
    const tree = slimTreeFromGitHubJson(raw.body);
    const pending = this.pending.get(ownerRepo) ?? { ownerRepo };
    const requestedRef = pending.requestedRef ?? pending.defaultBranch ?? "main";
    const resolvedRef = pending.resolvedRef ?? resolveRefName(requestedRef);
    const entry = {
      ownerRepo,
      requestedRef,
      resolvedRef,
      commitSha: pending.commitSha ?? "",
      treeSha: pending.treeSha ?? treeSha,
      defaultBranch: pending.defaultBranch ?? requestedRef,
      private: !!pending.private,
      tree,
      blobs: {},
      cachedAt: this.now(),
      ttlMs: this.ttlMs
    };
    const key = repoCacheKey(ownerRepo, resolvedRef);
    const prev = this.entries.get(key);
    if (prev && !this.isExpired(prev)) {
      entry.blobs = { ...prev.blobs };
    }
    this.entries.set(key, entry);
    this.pending.delete(ownerRepo);
    const remainingMs = this.remainingMs(entry);
    this.lastOutcome = { path, outcome: "miss", repoKey: key, remainingMs };
    return { ...raw, outcome: "miss", repoKey: key, remainingMs };
  }
  /**
   * @param {string} path
   * @param {string} ownerRepo
   * @param {string} blobSha
   * @param {(path: string) => { status: number, body: string } | null} fetchFn
   */
  #handleBlob(path, ownerRepo, blobSha, fetchFn) {
    const hit = this.findEntryWithBlob(ownerRepo, blobSha);
    if (hit) {
      const remainingMs2 = this.remainingMs(hit);
      const repoKey2 = repoCacheKey(hit.ownerRepo, hit.resolvedRef);
      this.lastOutcome = { path, outcome: "hit", repoKey: repoKey2, remainingMs: remainingMs2 };
      return {
        status: 200,
        body: reconstructBlobJson(blobSha, hit.blobs[blobSha]),
        outcome: "hit",
        repoKey: repoKey2,
        remainingMs: remainingMs2
      };
    }
    const raw = fetchFn(path);
    if (!raw || raw.status !== 200) {
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return raw ? { ...raw, outcome: "miss", repoKey: null, remainingMs: null } : null;
    }
    const slim = slimBlobFromGitHubJson(raw.body);
    let entry = this.findEntry(ownerRepo);
    if (!entry) {
      const resolvedRef = "refs/heads/_blobs";
      const key = repoCacheKey(ownerRepo, resolvedRef);
      entry = {
        ownerRepo,
        requestedRef: "_blobs",
        resolvedRef,
        commitSha: "",
        treeSha: "",
        defaultBranch: "main",
        private: false,
        tree: [],
        blobs: {},
        cachedAt: this.now(),
        ttlMs: this.ttlMs
      };
      this.entries.set(key, entry);
    }
    entry.blobs[blobSha] = slim;
    const repoKey = repoCacheKey(entry.ownerRepo, entry.resolvedRef);
    const remainingMs = this.remainingMs(entry);
    this.lastOutcome = { path, outcome: "miss", repoKey, remainingMs };
    return { ...raw, outcome: "miss", repoKey, remainingMs };
  }
  /**
   * Replace in-memory entries (e.g. after IndexedDB hydrate).
   * @param {RepoCacheEntry[]} rows
   */
  loadEntries(rows) {
    this.entries.clear();
    for (const row of rows) {
      if (!row?.ownerRepo || !row?.resolvedRef) continue;
      this.entries.set(repoCacheKey(row.ownerRepo, row.resolvedRef), {
        ...row,
        blobs: row.blobs ?? {},
        tree: row.tree ?? []
      });
    }
    this.invalidateExpired();
  }
  /**
   * @returns {RepoCacheEntry[]}
   */
  dumpEntries() {
    this.invalidateExpired();
    return [...this.entries.values()].map((e) => structuredClone(e));
  }
};
function ttlFromSearchParams(search = "") {
  const q = new URLSearchParams(search.startsWith("?") ? search : `?${search}`);
  if (q.has("ttlMs")) {
    const n = Number(q.get("ttlMs"));
    if (Number.isFinite(n) && n >= 0) return n;
  }
  if (q.has("ttl")) {
    const raw = q.get("ttl");
    const n = Number(raw);
    if (Number.isFinite(n) && n >= 0) {
      return n >= 1e3 ? n : n * 1e3;
    }
  }
  return null;
}
__name(ttlFromSearchParams, "ttlFromSearchParams");
__name2(ttlFromSearchParams, "ttlFromSearchParams");
var API = "https://api.github.com";
async function githubGet(path, token) {
  const url = path.startsWith("http://") || path.startsWith("https://") ? path : `${API}${path.startsWith("/") ? path : `/${path}`}`;
  const headers = {
    Accept: "application/vnd.github+json",
    "User-Agent": "wit-url-api-showcase (+https://github.com/thehumanworks/wit)"
  };
  const auth = githubAuthHeader(token);
  if (auth) headers.Authorization = auth;
  let res;
  try {
    res = await fetch(url, { headers });
  } catch (err) {
    throw new SafeError(`GitHub fetch failed: ${scrubSecrets(String(err))}`, {
      status: 502,
      code: "github_fetch"
    });
  }
  const body = await res.text();
  return { status: res.status, body };
}
__name(githubGet, "githubGet");
__name2(githubGet, "githubGet");
async function prefetchOpen(cache, ownerRepo, branch, token) {
  const repoPath = `/repos/${ownerRepo}`;
  const repoRes = await githubGet(repoPath, token);
  if (repoRes.status === 404) {
    throw new SafeError(`repository '${ownerRepo}' was not found`, {
      status: 404,
      code: "not_found"
    });
  }
  if (repoRes.status === 401 || repoRes.status === 403) {
    throw new SafeError(
      `GitHub API rejected access to '${ownerRepo}' (HTTP ${repoRes.status})`,
      { status: repoRes.status, code: "forbidden" }
    );
  }
  if (repoRes.status < 200 || repoRes.status >= 300) {
    throw new SafeError(`GitHub API ${repoPath} returned HTTP ${repoRes.status}`, {
      status: 502,
      code: "github_status"
    });
  }
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
      code: "ref_not_found"
    });
  }
  if (commitRes.status < 200 || commitRes.status >= 300) {
    throw new SafeError(
      `GitHub API commits returned HTTP ${commitRes.status}`,
      { status: 502, code: "github_status" }
    );
  }
  cache.getOrFetch(commitPath, () => commitRes);
  cache.getOrFetch(`https://api.github.com${commitPath}`, () => commitRes);
  const commit = JSON.parse(commitRes.body);
  const treeSha = commit?.commit?.tree?.sha;
  if (!treeSha) {
    throw new SafeError("commit response missing tree sha", {
      status: 502,
      code: "bad_commit"
    });
  }
  const treePath = `/repos/${ownerRepo}/git/trees/${treeSha}?recursive=1`;
  const treeRes = await githubGet(treePath, token);
  if (treeRes.status < 200 || treeRes.status >= 300) {
    throw new SafeError(`GitHub API tree returned HTTP ${treeRes.status}`, {
      status: 502,
      code: "github_status"
    });
  }
  cache.getOrFetch(treePath, () => treeRes);
  cache.getOrFetch(`https://api.github.com${treePath}`, () => treeRes);
  return { ref, treeSha, commitSha: String(commit.sha || "") };
}
__name(prefetchOpen, "prefetchOpen");
__name2(prefetchOpen, "prefetchOpen");
async function prefetchBlob(cache, ownerRepo, blobSha, token) {
  const blobPath = `/repos/${ownerRepo}/git/blobs/${blobSha}`;
  const hit = cache.findEntryWithBlob(ownerRepo, blobSha);
  if (hit) return;
  const blobRes = await githubGet(blobPath, token);
  if (blobRes.status < 200 || blobRes.status >= 300) {
    throw new SafeError(`GitHub API blob returned HTTP ${blobRes.status}`, {
      status: blobRes.status === 404 ? 404 : 502,
      code: "blob_status"
    });
  }
  cache.getOrFetch(blobPath, () => blobRes);
  cache.getOrFetch(`https://api.github.com${blobPath}`, () => blobRes);
}
__name(prefetchBlob, "prefetchBlob");
__name2(prefetchBlob, "prefetchBlob");
function blobShaForPath(cache, ownerRepo, branch, filePath) {
  const entry = cache.findEntry(ownerRepo, branch ?? void 0);
  if (!entry) return null;
  const want = filePath.replace(/^\/+|\/+$/g, "");
  const row = entry.tree.find((e) => e.type === "blob" && e.path === want);
  return row ? row.sha : null;
}
__name(blobShaForPath, "blobShaForPath");
__name2(blobShaForPath, "blobShaForPath");
function createHostCache(opts = {}) {
  return new RepoSnapshotCache({ ttlMs: opts.ttlMs ?? DEFAULT_TTL_MS });
}
__name(createHostCache, "createHostCache");
__name2(createHostCache, "createHostCache");
var VERBS = /* @__PURE__ */ new Set(["tree", "ls", "cat"]);
var KNOWN_KEYS = {
  tree: /* @__PURE__ */ new Set(["path", "branch", "ref", "depth", "l", "long", "token", "access_token", "ttl", "ttlMs"]),
  ls: /* @__PURE__ */ new Set(["path", "branch", "ref", "l", "long", "token", "access_token", "ttl", "ttlMs"]),
  cat: /* @__PURE__ */ new Set(["path", "branch", "ref", "n", "b", "number", "token", "access_token", "ttl", "ttlMs"])
};
function isSafeRepoComponent(component) {
  return typeof component === "string" && component.length > 0 && component !== "." && component !== ".." && /^[A-Za-z0-9._-]+$/.test(component);
}
__name(isSafeRepoComponent, "isSafeRepoComponent");
__name2(isSafeRepoComponent, "isSafeRepoComponent");
function normalizePath(value) {
  if (value == null || value === "") return "";
  return String(value).trim().replace(/\\/g, "/").replace(/^\.\//, "").replace(/^\/+/, "").replace(/\/+$/, "");
}
__name(normalizePath, "normalizePath");
__name2(normalizePath, "normalizePath");
function parseRoute(input) {
  const url = input instanceof URL ? input : new URL(String(input), "http://local");
  const parts = url.pathname.replace(/\/+$/, "").split("/").filter(Boolean);
  if (parts.length === 0) return null;
  const verb = parts[0].toLowerCase();
  if (!VERBS.has(verb)) return null;
  if (parts.length !== 3) {
    throw new SafeError(
      `expected /${verb}/{owner}/{repo} (path belongs in ?path=)`,
      { status: 400, code: "bad_route" }
    );
  }
  const owner = parts[1];
  const repo = parts[2];
  if (!isSafeRepoComponent(owner) || !isSafeRepoComponent(repo)) {
    throw new SafeError(`invalid owner/repo: ${owner}/${repo}`, {
      status: 400,
      code: "bad_repo"
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
        code: "bad_depth"
      });
    }
    depth = n;
  }
  const long = q.get("l") === "1" || q.get("l") === "true" || q.has("long");
  const number = q.get("n") === "1" || q.get("n") === "true" || q.has("number");
  if (verb === "cat" && !path) {
    throw new SafeError("cat requires ?path=", { status: 400, code: "path_required" });
  }
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
    number
  };
}
__name(parseRoute, "parseRoute");
__name2(parseRoute, "parseRoute");
function isApiPath(pathname) {
  const first = String(pathname).replace(/\/+$/, "").split("/").filter(Boolean)[0];
  return first != null && VERBS.has(first.toLowerCase());
}
__name(isApiPath, "isApiPath");
__name2(isApiPath, "isApiPath");
function errorBody(err) {
  const status = err && typeof err === "object" && "status" in err ? Number(err.status) || 500 : 500;
  const message = scrubSecrets(
    err instanceof Error ? err.message : String(err ?? "error")
  );
  return { status, body: `error: ${message}
` };
}
__name(errorBody, "errorBody");
__name2(errorBody, "errorBody");
var ERR_NAMES = {
  0: "ok",
  1: "rate_limit",
  2: "oversized",
  3: "not_found",
  4: "binary",
  5: "private_repo",
  6: "oom",
  7: "api",
  8: "other"
};
function readAscii(api, ptr, len) {
  const bytes = new Uint8Array(api.memory.buffer, ptr, len);
  return new TextDecoder().decode(bytes);
}
__name(readAscii, "readAscii");
__name2(readAscii, "readAscii");
function writeAscii(api, ptr, text) {
  const bytes = new TextEncoder().encode(text);
  new Uint8Array(api.memory.buffer, ptr, bytes.length).set(bytes);
  return bytes.length;
}
__name(writeAscii, "writeAscii");
__name2(writeAscii, "writeAscii");
function makeHostImports(getExports, cache) {
  return {
    wit_snapshot_host: {
      wit_snapshot_host_http_get(pathPtr, pathLen, statusOut, bodyPtrOut, bodyLenOut) {
        const api = getExports();
        const path = readAscii(api, pathPtr, pathLen);
        const result = cache.getOrFetch(path, () => null);
        if (!result) {
          console.error("http_get miss", scrubSecrets(path));
          return 3;
        }
        const bodyBytes = new TextEncoder().encode(result.body);
        const bodyPtr = api.wit_snapshot_alloc(bodyBytes.length || 1);
        if (bodyBytes.length) {
          new Uint8Array(api.memory.buffer, bodyPtr, bodyBytes.length).set(bodyBytes);
        }
        const view = new DataView(api.memory.buffer);
        view.setUint16(statusOut, result.status, true);
        view.setUint32(bodyPtrOut, bodyPtr, true);
        view.setUint32(bodyLenOut, bodyBytes.length, true);
        return 0;
      }
    }
  };
}
__name(makeHostImports, "makeHostImports");
__name2(makeHostImports, "makeHostImports");
async function loadWasm(source, cache) {
  let exports = null;
  const imports = makeHostImports(() => exports, cache);
  let instance;
  if (source instanceof WebAssembly.Module) {
    instance = new WebAssembly.Instance(source, imports);
  } else if (source instanceof Response || source && typeof source.then === "function") {
    const result = await WebAssembly.instantiateStreaming(source, imports);
    instance = result.instance;
  } else {
    const result = await WebAssembly.instantiate(source, imports);
    instance = result.instance;
  }
  exports = instance.exports;
  if (!exports.memory || !exports.wit_snapshot_open) {
    throw new SafeError("wasm exports missing (memory / open / list / read)", {
      status: 500,
      code: "wasm_exports"
    });
  }
  return exports;
}
__name(loadWasm, "loadWasm");
__name2(loadWasm, "loadWasm");
function lastError(api) {
  const buflen = 512;
  const buf = api.wit_snapshot_alloc(buflen);
  const n = api.wit_snapshot_last_error(buf, buflen);
  const msg = readAscii(api, buf, Math.min(n, buflen));
  api.wit_snapshot_dealloc(buf, buflen);
  return scrubSecrets(msg);
}
__name(lastError, "lastError");
__name2(lastError, "lastError");
function check(api, rc, label) {
  if (rc !== 0) {
    const name = ERR_NAMES[rc] || String(rc);
    throw new SafeError(`${label} failed: ${name} \u2014 ${lastError(api)}`, {
      status: statusForCode(rc),
      code: name
    });
  }
}
__name(check, "check");
__name2(check, "check");
function statusForCode(rc) {
  if (rc === 3) return 404;
  if (rc === 5) return 403;
  if (rc === 1) return 429;
  if (rc === 4) return 415;
  return 502;
}
__name(statusForCode, "statusForCode");
__name2(statusForCode, "statusForCode");
function withGuestString(api, text, fn) {
  const bytes = new TextEncoder().encode(text);
  const ptr = api.wit_snapshot_alloc(bytes.length || 1);
  if (bytes.length) writeAscii(api, ptr, text);
  try {
    return fn(ptr, bytes.length);
  } finally {
    api.wit_snapshot_dealloc(ptr, bytes.length || 1);
  }
}
__name(withGuestString, "withGuestString");
__name2(withGuestString, "withGuestString");
function readOutJson(api, outPtrSlot, outLenSlot) {
  const view = new DataView(api.memory.buffer);
  const ptr = view.getUint32(outPtrSlot, true);
  const len = view.getUint32(outLenSlot, true);
  const json = readAscii(api, ptr, len);
  api.wit_snapshot_dealloc(ptr, len || 1);
  return json;
}
__name(readOutJson, "readOutJson");
__name2(readOutJson, "readOutJson");
function wasmOpen(api, ownerRepo, branch) {
  withGuestString(api, ownerRepo, (repoPtr, repoLen) => {
    if (branch) {
      withGuestString(api, branch, (bPtr, bLen) => {
        check(api, api.wit_snapshot_open(repoPtr, repoLen, bPtr, bLen), "open");
      });
    } else {
      check(api, api.wit_snapshot_open(repoPtr, repoLen, 0, 0), "open");
    }
  });
}
__name(wasmOpen, "wasmOpen");
__name2(wasmOpen, "wasmOpen");
function wasmList(api, path) {
  const outPtrSlot = api.wit_snapshot_alloc(4);
  const outLenSlot = api.wit_snapshot_alloc(4);
  try {
    const p = path || "";
    if (p) {
      withGuestString(api, p, (ptr, len) => {
        check(api, api.wit_snapshot_list(ptr, len, outPtrSlot, outLenSlot), "list");
      });
    } else {
      check(api, api.wit_snapshot_list(0, 0, outPtrSlot, outLenSlot), "list");
    }
    const json = readOutJson(api, outPtrSlot, outLenSlot);
    return JSON.parse(json);
  } finally {
    api.wit_snapshot_dealloc(outPtrSlot, 4);
    api.wit_snapshot_dealloc(outLenSlot, 4);
  }
}
__name(wasmList, "wasmList");
__name2(wasmList, "wasmList");
function wasmRead(api, path) {
  const outPtrSlot = api.wit_snapshot_alloc(4);
  const outLenSlot = api.wit_snapshot_alloc(4);
  try {
    withGuestString(api, path, (ptr, len) => {
      check(api, api.wit_snapshot_read(ptr, len, outPtrSlot, outLenSlot), "read");
    });
    const json = readOutJson(api, outPtrSlot, outLenSlot);
    return JSON.parse(json);
  } finally {
    api.wit_snapshot_dealloc(outPtrSlot, 4);
    api.wit_snapshot_dealloc(outLenSlot, 4);
  }
}
__name(wasmRead, "wasmRead");
__name2(wasmRead, "wasmRead");
function collectTreeFiles(api, basePath, depth) {
  const files = [];
  function walk(dir, level) {
    if (depth != null && level > depth) return;
    const entries = wasmList(api, dir);
    for (const e of entries) {
      const kind = e.kind === "dir" ? "dir" : "file";
      const full = e.path || (dir ? `${dir}/${e.name}` : e.name);
      if (kind === "dir") {
        if (depth == null || level < depth) {
          walk(full, level + 1);
        }
      } else {
        const relative = basePath ? full.startsWith(basePath + "/") ? full.slice(basePath.length + 1) : full : full;
        const segs = relative.split("/").filter(Boolean).length;
        if (depth == null || segs <= depth) {
          files.push({
            path: full,
            kind: "file",
            size_bytes: e.size_bytes ?? null
          });
        }
      }
    }
  }
  __name(walk, "walk");
  __name2(walk, "walk");
  walk(basePath || "", 0);
  return files;
}
__name(collectTreeFiles, "collectTreeFiles");
__name2(collectTreeFiles, "collectTreeFiles");
var defaultCache = null;
function getCache(deps) {
  if (deps?.cache) return deps.cache;
  if (!defaultCache) defaultCache = createHostCache();
  return defaultCache;
}
__name(getCache, "getCache");
__name2(getCache, "getCache");
async function handleRequest(request, deps) {
  const url = new URL(request.url);
  if (!isApiPath(url.pathname)) return null;
  if (request.method !== "GET" && request.method !== "HEAD") {
    return textResponse("error: method not allowed\n", 405);
  }
  try {
    const route = parseRoute(url);
    if (!route) return null;
    const token = extractToken({ headers: request.headers, url });
    const ttl = ttlFromSearchParams(url.search);
    const cache = getCache(deps);
    if (ttl != null) cache.setTtlMs(ttl);
    const wasmSource = await deps.loadWasmBytes();
    const api = await loadWasm(wasmSource, cache);
    await prefetchOpen(cache, route.ownerRepo, route.branch, token);
    wasmOpen(api, route.ownerRepo, route.branch);
    let body;
    if (route.verb === "ls") {
      const entries = wasmList(api, route.path);
      const normalized = entries.map((e) => ({
        ...e,
        kind: e.kind === "Dir" || e.kind === "dir" ? "dir" : "file",
        name: e.name,
        path: e.path,
        size_bytes: e.size_bytes ?? null
      }));
      body = formatLs(normalized, { long: route.long });
    } else if (route.verb === "tree") {
      const files = collectTreeFiles(api, route.path, route.depth);
      const root = route.path ? route.path.split("/").filter(Boolean).pop() || route.path : ".";
      body = formatTree(
        { root, entries: files },
        { path: route.path, depth: route.depth, long: route.long }
      );
    } else if (route.verb === "cat") {
      const sha = blobShaForPath(cache, route.ownerRepo, route.branch, route.path);
      if (!sha) {
        throw new SafeError(`File not found: ${route.path}`, {
          status: 404,
          code: "not_found"
        });
      }
      await prefetchBlob(cache, route.ownerRepo, sha, token);
      const file = wasmRead(api, route.path);
      body = formatCat(file.text, { number: route.number });
    } else {
      throw new SafeError("unknown verb", { status: 400, code: "bad_verb" });
    }
    if (request.method === "HEAD") {
      return new Response(null, {
        status: 200,
        headers: plaintextHeaders(body)
      });
    }
    return textResponse(body.endsWith("\n") ? body : `${body}
`, 200);
  } catch (err) {
    const { status, body } = errorBody(err);
    console.error("url-api error", scrubSecrets(String(err?.message || err)));
    return textResponse(body, status);
  }
}
__name(handleRequest, "handleRequest");
__name2(handleRequest, "handleRequest");
function textResponse(body, status) {
  return new Response(body, { status, headers: plaintextHeaders(body) });
}
__name(textResponse, "textResponse");
__name2(textResponse, "textResponse");
function plaintextHeaders(body) {
  return {
    "content-type": "text/plain; charset=utf-8",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    "content-length": String(new TextEncoder().encode(body).length)
  };
}
__name(plaintextHeaders, "plaintextHeaders");
__name2(plaintextHeaders, "plaintextHeaders");
var worker_default = {
  /**
   * @param {Request} request
   * @param {{ ASSETS: { fetch: typeof fetch } }} env
   */
  async fetch(request, env) {
    const apiResponse = await handleRequest(request, {
      loadWasmBytes: /* @__PURE__ */ __name2(async () => wasmModule, "loadWasmBytes")
    });
    if (apiResponse) return apiResponse;
    return env.ASSETS.fetch(request);
  }
};
var drainBody = /* @__PURE__ */ __name2(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } finally {
    try {
      if (request.body !== null && !request.bodyUsed) {
        const reader = request.body.getReader();
        while (!(await reader.read()).done) {
        }
      }
    } catch (e) {
      console.error("Failed to drain the unused request body.", e);
    }
  }
}, "drainBody");
var middleware_ensure_req_body_drained_default = drainBody;
function reduceError(e) {
  return {
    name: e?.name,
    message: e?.message ?? String(e),
    stack: e?.stack,
    cause: e?.cause === void 0 ? void 0 : reduceError(e.cause)
  };
}
__name(reduceError, "reduceError");
__name2(reduceError, "reduceError");
var jsonError = /* @__PURE__ */ __name2(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } catch (e) {
    const error = reduceError(e);
    const body = JSON.stringify(error);
    const headers = {
      "Content-Type": "application/json",
      "MF-Experimental-Error-Stack": "true"
    };
    const encoded = encodeURIComponent(body);
    if (encoded.length <= 8192) {
      headers["MF-Experimental-Error-Stack-Payload"] = encoded;
    }
    return new Response(body, { status: 500, headers });
  }
}, "jsonError");
var middleware_miniflare3_json_error_default = jsonError;
var __INTERNAL_WRANGLER_MIDDLEWARE__ = [
  middleware_ensure_req_body_drained_default,
  middleware_miniflare3_json_error_default
];
var middleware_insertion_facade_default = worker_default;
var __facade_middleware__ = [];
function __facade_register__(...args) {
  __facade_middleware__.push(...args.flat());
}
__name(__facade_register__, "__facade_register__");
__name2(__facade_register__, "__facade_register__");
function __facade_invokeChain__(request, env, ctx, dispatch, middlewareChain) {
  const [head, ...tail] = middlewareChain;
  const middlewareCtx = {
    dispatch,
    next(newRequest, newEnv) {
      return __facade_invokeChain__(newRequest, newEnv, ctx, dispatch, tail);
    }
  };
  return head(request, env, ctx, middlewareCtx);
}
__name(__facade_invokeChain__, "__facade_invokeChain__");
__name2(__facade_invokeChain__, "__facade_invokeChain__");
function __facade_invoke__(request, env, ctx, dispatch, finalMiddleware) {
  return __facade_invokeChain__(request, env, ctx, dispatch, [
    ...__facade_middleware__,
    finalMiddleware
  ]);
}
__name(__facade_invoke__, "__facade_invoke__");
__name2(__facade_invoke__, "__facade_invoke__");
var __Facade_ScheduledController__ = class ___Facade_ScheduledController__ {
  static {
    __name(this, "___Facade_ScheduledController__");
  }
  constructor(scheduledTime, cron, noRetry) {
    this.scheduledTime = scheduledTime;
    this.cron = cron;
    this.#noRetry = noRetry;
  }
  scheduledTime;
  cron;
  static {
    __name2(this, "__Facade_ScheduledController__");
  }
  #noRetry;
  noRetry() {
    if (!(this instanceof ___Facade_ScheduledController__)) {
      throw new TypeError("Illegal invocation");
    }
    this.#noRetry();
  }
};
function wrapExportedHandler(worker) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__ === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__.length === 0) {
    return worker;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__) {
    __facade_register__(middleware);
  }
  const fetchDispatcher = /* @__PURE__ */ __name2(function(request, env, ctx) {
    if (worker.fetch === void 0) {
      throw new Error("Handler does not export a fetch() function.");
    }
    return worker.fetch(request, env, ctx);
  }, "fetchDispatcher");
  return {
    ...worker,
    fetch(request, env, ctx) {
      const dispatcher = /* @__PURE__ */ __name2(function(type, init) {
        if (type === "scheduled" && worker.scheduled !== void 0) {
          const controller = new __Facade_ScheduledController__(
            Date.now(),
            init.cron ?? "",
            () => {
            }
          );
          return worker.scheduled(controller, env, ctx);
        }
      }, "dispatcher");
      return __facade_invoke__(request, env, ctx, dispatcher, fetchDispatcher);
    }
  };
}
__name(wrapExportedHandler, "wrapExportedHandler");
__name2(wrapExportedHandler, "wrapExportedHandler");
function wrapWorkerEntrypoint(klass) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__ === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__.length === 0) {
    return klass;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__) {
    __facade_register__(middleware);
  }
  return class extends klass {
    #fetchDispatcher = /* @__PURE__ */ __name2((request, env, ctx) => {
      this.env = env;
      this.ctx = ctx;
      if (super.fetch === void 0) {
        throw new Error("Entrypoint class does not define a fetch() function.");
      }
      return super.fetch(request);
    }, "#fetchDispatcher");
    #dispatcher = /* @__PURE__ */ __name2((type, init) => {
      if (type === "scheduled" && super.scheduled !== void 0) {
        const controller = new __Facade_ScheduledController__(
          Date.now(),
          init.cron ?? "",
          () => {
          }
        );
        return super.scheduled(controller);
      }
    }, "#dispatcher");
    fetch(request) {
      return __facade_invoke__(
        request,
        this.env,
        this.ctx,
        this.#dispatcher,
        this.#fetchDispatcher
      );
    }
  };
}
__name(wrapWorkerEntrypoint, "wrapWorkerEntrypoint");
__name2(wrapWorkerEntrypoint, "wrapWorkerEntrypoint");
var WRAPPED_ENTRY;
if (typeof middleware_insertion_facade_default === "object") {
  WRAPPED_ENTRY = wrapExportedHandler(middleware_insertion_facade_default);
} else if (typeof middleware_insertion_facade_default === "function") {
  WRAPPED_ENTRY = wrapWorkerEntrypoint(middleware_insertion_facade_default);
}
var middleware_loader_entry_default = WRAPPED_ENTRY;

// ../../../home/ubuntu/.npm/_npx/c943b712072b77c4/node_modules/wrangler/templates/middleware/middleware-ensure-req-body-drained.ts
var drainBody2 = /* @__PURE__ */ __name(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } finally {
    try {
      if (request.body !== null && !request.bodyUsed) {
        const reader = request.body.getReader();
        while (!(await reader.read()).done) {
        }
      }
    } catch (e) {
      console.error("Failed to drain the unused request body.", e);
    }
  }
}, "drainBody");
var middleware_ensure_req_body_drained_default2 = drainBody2;

// ../../../home/ubuntu/.npm/_npx/c943b712072b77c4/node_modules/wrangler/templates/middleware/middleware-miniflare3-json-error.ts
function reduceError2(e) {
  return {
    name: e?.name,
    message: e?.message ?? String(e),
    stack: e?.stack,
    cause: e?.cause === void 0 ? void 0 : reduceError2(e.cause)
  };
}
__name(reduceError2, "reduceError");
var jsonError2 = /* @__PURE__ */ __name(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } catch (e) {
    const error = reduceError2(e);
    const body = JSON.stringify(error);
    const headers = {
      "Content-Type": "application/json",
      "MF-Experimental-Error-Stack": "true"
    };
    const encoded = encodeURIComponent(body);
    if (encoded.length <= 8192) {
      headers["MF-Experimental-Error-Stack-Payload"] = encoded;
    }
    return new Response(body, { status: 500, headers });
  }
}, "jsonError");
var middleware_miniflare3_json_error_default2 = jsonError2;

// .wrangler/tmp/bundle-uPSYQv/middleware-insertion-facade.js
var __INTERNAL_WRANGLER_MIDDLEWARE__2 = [
  middleware_ensure_req_body_drained_default2,
  middleware_miniflare3_json_error_default2
];
var middleware_insertion_facade_default2 = middleware_loader_entry_default;

// ../../../home/ubuntu/.npm/_npx/c943b712072b77c4/node_modules/wrangler/templates/middleware/common.ts
var __facade_middleware__2 = [];
function __facade_register__2(...args) {
  __facade_middleware__2.push(...args.flat());
}
__name(__facade_register__2, "__facade_register__");
function __facade_invokeChain__2(request, env, ctx, dispatch, middlewareChain) {
  const [head, ...tail] = middlewareChain;
  const middlewareCtx = {
    dispatch,
    next(newRequest, newEnv) {
      return __facade_invokeChain__2(newRequest, newEnv, ctx, dispatch, tail);
    }
  };
  return head(request, env, ctx, middlewareCtx);
}
__name(__facade_invokeChain__2, "__facade_invokeChain__");
function __facade_invoke__2(request, env, ctx, dispatch, finalMiddleware) {
  return __facade_invokeChain__2(request, env, ctx, dispatch, [
    ...__facade_middleware__2,
    finalMiddleware
  ]);
}
__name(__facade_invoke__2, "__facade_invoke__");

// .wrangler/tmp/bundle-uPSYQv/middleware-loader.entry.ts
var __Facade_ScheduledController__2 = class ___Facade_ScheduledController__2 {
  constructor(scheduledTime, cron, noRetry) {
    this.scheduledTime = scheduledTime;
    this.cron = cron;
    this.#noRetry = noRetry;
  }
  scheduledTime;
  cron;
  static {
    __name(this, "__Facade_ScheduledController__");
  }
  #noRetry;
  noRetry() {
    if (!(this instanceof ___Facade_ScheduledController__2)) {
      throw new TypeError("Illegal invocation");
    }
    this.#noRetry();
  }
};
function wrapExportedHandler2(worker) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__2 === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__2.length === 0) {
    return worker;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__2) {
    __facade_register__2(middleware);
  }
  const fetchDispatcher = /* @__PURE__ */ __name(function(request, env, ctx) {
    if (worker.fetch === void 0) {
      throw new Error("Handler does not export a fetch() function.");
    }
    return worker.fetch(request, env, ctx);
  }, "fetchDispatcher");
  return {
    ...worker,
    fetch(request, env, ctx) {
      const dispatcher = /* @__PURE__ */ __name(function(type, init) {
        if (type === "scheduled" && worker.scheduled !== void 0) {
          const controller = new __Facade_ScheduledController__2(
            Date.now(),
            init.cron ?? "",
            () => {
            }
          );
          return worker.scheduled(controller, env, ctx);
        }
      }, "dispatcher");
      return __facade_invoke__2(request, env, ctx, dispatcher, fetchDispatcher);
    }
  };
}
__name(wrapExportedHandler2, "wrapExportedHandler");
function wrapWorkerEntrypoint2(klass) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__2 === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__2.length === 0) {
    return klass;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__2) {
    __facade_register__2(middleware);
  }
  return class extends klass {
    #fetchDispatcher = /* @__PURE__ */ __name((request, env, ctx) => {
      this.env = env;
      this.ctx = ctx;
      if (super.fetch === void 0) {
        throw new Error("Entrypoint class does not define a fetch() function.");
      }
      return super.fetch(request);
    }, "#fetchDispatcher");
    #dispatcher = /* @__PURE__ */ __name((type, init) => {
      if (type === "scheduled" && super.scheduled !== void 0) {
        const controller = new __Facade_ScheduledController__2(
          Date.now(),
          init.cron ?? "",
          () => {
          }
        );
        return super.scheduled(controller);
      }
    }, "#dispatcher");
    fetch(request) {
      return __facade_invoke__2(
        request,
        this.env,
        this.ctx,
        this.#dispatcher,
        this.#fetchDispatcher
      );
    }
  };
}
__name(wrapWorkerEntrypoint2, "wrapWorkerEntrypoint");
var WRAPPED_ENTRY2;
if (typeof middleware_insertion_facade_default2 === "object") {
  WRAPPED_ENTRY2 = wrapExportedHandler2(middleware_insertion_facade_default2);
} else if (typeof middleware_insertion_facade_default2 === "function") {
  WRAPPED_ENTRY2 = wrapWorkerEntrypoint2(middleware_insertion_facade_default2);
}
var middleware_loader_entry_default2 = WRAPPED_ENTRY2;
export {
  __INTERNAL_WRANGLER_MIDDLEWARE__2 as __INTERNAL_WRANGLER_MIDDLEWARE__,
  middleware_loader_entry_default2 as default
};
//# sourceMappingURL=bundledWorker-0.6668071131924698.js.map

/**
 * Host-owned per-repo snapshot cache for the wit-snapshot wasm browser path.
 *
 * The wasm guest still calls get_json → host http_get. This module sits inside
 * http_get: serve slim cached tree/blob responses until the per-repo TTL
 * expires, then refetch. Not a third SnapshotBackend.
 *
 * Storage is sync Map for the hot path (wasm imports are sync). IndexedDB is
 * an optional async hydrate/persist layer used by the demo page.
 */

export const DEFAULT_TTL_MS = 24 * 60 * 60 * 1000;

/**
 * @typedef {{ path: string, type: string, sha: string, size?: number }} SlimTreeEntry
 * @typedef {{
 *   ownerRepo: string,
 *   requestedRef: string,
 *   resolvedRef: string,
 *   commitSha: string,
 *   treeSha: string,
 *   defaultBranch: string,
 *   private: boolean,
 *   tree: SlimTreeEntry[],
 *   blobs: Record<string, { size: number, contentBase64: string }>,
 *   cachedAt: number,
 *   ttlMs: number,
 * }} RepoCacheEntry
 */

/**
 * @param {string} path
 */
export function normalizeApiPath(path) {
  if (path.startsWith("https://api.github.com")) {
    return path.slice("https://api.github.com".length) || "/";
  }
  if (path.startsWith("http://api.github.com")) {
    return path.slice("http://api.github.com".length) || "/";
  }
  return path;
}

/**
 * @param {string} path
 * @returns {{
 *   kind: 'repo' | 'commit' | 'tree' | 'blob' | 'other',
 *   ownerRepo: string | null,
 *   ref?: string,
 *   treeSha?: string,
 *   blobSha?: string,
 * }}
 */
export function parseGitHubApiPath(path) {
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

/**
 * @param {string} ownerRepo
 * @param {string} resolvedRef
 */
export function repoCacheKey(ownerRepo, resolvedRef) {
  return `${ownerRepo}@${resolvedRef}`;
}

/**
 * @param {string} requested
 */
export function resolveRefName(requested) {
  if (/^[0-9a-f]{40}$/i.test(requested) || requested.startsWith("refs/")) {
    return requested;
  }
  return `refs/heads/${requested}`;
}

/**
 * Slim recursive tree from a GitHub trees API body.
 * @param {string} body
 * @returns {SlimTreeEntry[]}
 */
export function slimTreeFromGitHubJson(body) {
  const parsed = JSON.parse(body);
  const tree = Array.isArray(parsed.tree) ? parsed.tree : [];
  return tree
    .filter((e) => e && (e.type === "blob" || e.type === "tree") && typeof e.path === "string")
    .map((e) => {
      /** @type {SlimTreeEntry} */
      const out = { path: e.path, type: e.type, sha: String(e.sha ?? "") };
      if (typeof e.size === "number") out.size = e.size;
      return out;
    });
}

/**
 * @param {string} body
 * @returns {{ size: number, contentBase64: string }}
 */
export function slimBlobFromGitHubJson(body) {
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

/**
 * @param {Uint8Array} bytes
 */
function bytesToBase64(bytes) {
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
}

/**
 * @param {RepoCacheEntry} entry
 */
export function reconstructRepoJson(entry) {
  return JSON.stringify({
    private: entry.private,
    default_branch: entry.defaultBranch,
  });
}

/**
 * @param {RepoCacheEntry} entry
 */
export function reconstructCommitJson(entry) {
  return JSON.stringify({
    sha: entry.commitSha,
    commit: { tree: { sha: entry.treeSha } },
  });
}

/**
 * @param {RepoCacheEntry} entry
 */
export function reconstructTreeJson(entry) {
  return JSON.stringify({
    sha: entry.treeSha,
    truncated: false,
    tree: entry.tree.map((e) => {
      const row = { path: e.path, type: e.type, sha: e.sha, mode: e.type === "tree" ? "040000" : "100644" };
      if (typeof e.size === "number") row.size = e.size;
      return row;
    }),
  });
}

/**
 * @param {string} sha
 * @param {{ size: number, contentBase64: string }} blob
 */
export function reconstructBlobJson(sha, blob) {
  return JSON.stringify({
    sha,
    size: blob.size,
    encoding: "base64",
    content: blob.contentBase64,
  });
}

export class RepoSnapshotCache {
  /**
   * @param {{
   *   ttlMs?: number,
   *   now?: () => number,
   * }} [opts]
   */
  constructor(opts = {}) {
    this.ttlMs = opts.ttlMs ?? DEFAULT_TTL_MS;
    this.now = opts.now ?? (() => Date.now());
    /** @type {Map<string, RepoCacheEntry>} */
    this.entries = new Map();
    /** Staging for in-flight open sequence before tree lands. */
    /** @type {Map<string, Partial<RepoCacheEntry> & { ownerRepo: string }>} */
    this.pending = new Map();
    /** @type {{ path: string, outcome: 'hit' | 'miss', repoKey: string | null, remainingMs: number | null } | null} */
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
      if (
        entry.requestedRef === requestedRef ||
        entry.resolvedRef === requestedRef ||
        entry.resolvedRef === resolveRefName(requestedRef) ||
        entry.commitSha === requestedRef
      ) {
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
      blobCount: Object.keys(entry.blobs).length,
    }));
  }

  /**
   * Serve from cache or call `fetchFn` on miss/expiry.
   * `fetchFn` must return `{ status: number, body: string }` synchronously.
   *
   * `opts.ttlMs` overrides the cache-wide TTL for entries this call creates,
   * so one request's `?ttl=` never leaks into concurrent requests.
   *
   * @param {string} path
   * @param {(path: string) => { status: number, body: string } | null} fetchFn
   * @param {{ ttlMs?: number | null }} [opts]
   * @returns {{ status: number, body: string, outcome: 'hit' | 'miss', repoKey: string | null, remainingMs: number | null } | null}
   */
  getOrFetch(path, fetchFn, opts = {}) {
    const ttlMs =
      opts.ttlMs != null && Number.isFinite(Number(opts.ttlMs))
        ? Math.max(0, Number(opts.ttlMs))
        : this.ttlMs;
    const parsed = parseGitHubApiPath(path);
    if (parsed.kind === "other" || !parsed.ownerRepo) {
      const raw = fetchFn(path);
      if (!raw) return null;
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return { ...raw, outcome: "miss", repoKey: null, remainingMs: null };
    }

    if (parsed.kind === "blob") {
      return this.#handleBlob(path, parsed.ownerRepo, parsed.blobSha, fetchFn, ttlMs);
    }
    if (parsed.kind === "tree") {
      return this.#handleTree(path, parsed.ownerRepo, parsed.treeSha, fetchFn, ttlMs);
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
      // The repo call starts an open sequence: seed the pending state from
      // the cached metadata so a later commit/tree miss for another ref
      // still records the true default branch (not the requested ref).
      this.pending.set(ownerRepo, {
        ownerRepo,
        private: hit.private,
        defaultBranch: hit.defaultBranch,
      });
      const remainingMs = this.remainingMs(hit);
      const repoKey = repoCacheKey(hit.ownerRepo, hit.resolvedRef);
      this.lastOutcome = { path, outcome: "hit", repoKey, remainingMs };
      return {
        status: 200,
        body: reconstructRepoJson(hit),
        outcome: "hit",
        repoKey,
        remainingMs,
      };
    }
    const raw = fetchFn(path);
    if (!raw || raw.status !== 200) {
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return raw ? { ...raw, outcome: "miss", repoKey: null, remainingMs: null } : null;
    }
    const meta = JSON.parse(raw.body);
    // A fresh repo response restarts the open sequence for this repo.
    this.pending.set(ownerRepo, {
      ownerRepo,
      private: !!meta.private,
      defaultBranch: String(meta.default_branch ?? "main"),
    });
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
        remainingMs,
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
   * @param {number} ttlMs
   */
  #handleTree(path, ownerRepo, treeSha, fetchFn, ttlMs) {
    const hit = this.findEntryByTreeSha(ownerRepo, treeSha);
    const pending = this.pending.get(ownerRepo) ?? { ownerRepo };
    if (hit) {
      // Another ref (a fresh branch, a tag, or a commit SHA) can point at the
      // same tree. Complete its open sequence as its own entry, sharing the
      // slim tree, instead of leaving it without one.
      if (pending.resolvedRef && pending.resolvedRef !== hit.resolvedRef) {
        return this.#storeTree(path, ownerRepo, treeSha, hit.tree, pending, ttlMs, {
          status: 200,
          body: reconstructTreeJson(hit),
        });
      }
      const remainingMs = this.remainingMs(hit);
      const repoKey = repoCacheKey(hit.ownerRepo, hit.resolvedRef);
      this.lastOutcome = { path, outcome: "hit", repoKey, remainingMs };
      return {
        status: 200,
        body: reconstructTreeJson(hit),
        outcome: "hit",
        repoKey,
        remainingMs,
      };
    }
    const raw = fetchFn(path);
    if (!raw || raw.status !== 200) {
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return raw ? { ...raw, outcome: "miss", repoKey: null, remainingMs: null } : null;
    }
    const tree = slimTreeFromGitHubJson(raw.body);
    return this.#storeTree(path, ownerRepo, treeSha, tree, pending, ttlMs, raw);
  }

  /**
   * Materialize the open entry for the pending ref once its tree is known.
   * @param {string} path
   * @param {string} ownerRepo
   * @param {string} treeSha
   * @param {SlimTreeEntry[]} tree
   * @param {Partial<RepoCacheEntry> & { ownerRepo: string }} pending
   * @param {number} ttlMs
   * @param {{ status: number, body: string }} raw
   */
  #storeTree(path, ownerRepo, treeSha, tree, pending, ttlMs, raw) {
    const requestedRef = pending.requestedRef ?? pending.defaultBranch ?? "main";
    const resolvedRef = pending.resolvedRef ?? resolveRefName(requestedRef);
    /** @type {RepoCacheEntry} */
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
      ttlMs,
    };
    const key = repoCacheKey(ownerRepo, resolvedRef);
    // Preserve blobs if refreshing the same key after expiry.
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
   * @param {number} ttlMs
   */
  #handleBlob(path, ownerRepo, blobSha, fetchFn, ttlMs) {
    const hit = this.findEntryWithBlob(ownerRepo, blobSha);
    if (hit) {
      const remainingMs = this.remainingMs(hit);
      const repoKey = repoCacheKey(hit.ownerRepo, hit.resolvedRef);
      this.lastOutcome = { path, outcome: "hit", repoKey, remainingMs };
      return {
        status: 200,
        body: reconstructBlobJson(blobSha, hit.blobs[blobSha]),
        outcome: "hit",
        repoKey,
        remainingMs,
      };
    }
    const raw = fetchFn(path);
    if (!raw || raw.status !== 200) {
      this.lastOutcome = { path, outcome: "miss", repoKey: null, remainingMs: null };
      return raw ? { ...raw, outcome: "miss", repoKey: null, remainingMs: null } : null;
    }
    const slim = slimBlobFromGitHubJson(raw.body);
    // Attach to any live entry for this owner/repo (prefer matching pending open).
    let entry = this.findEntry(ownerRepo);
    if (!entry) {
      // Blob read before/without a live open entry: keep a synthetic bucket so
      // independent blob TTL still works under the default-branch key.
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
        ttlMs,
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
   * A "complete open" entry for repo@ref: one that can serve the wasm open
   * sequence (repo → commit → tree) without touching GitHub. Excludes the
   * synthetic `_blobs` bucket and blob-only rows. With no requestedRef, only
   * an entry for the repo's own default branch counts — a cached feature
   * branch must not masquerade as the default.
   *
   * @param {string} ownerRepo
   * @param {string} [requestedRef]
   * @returns {RepoCacheEntry | null}
   */
  findOpenEntry(ownerRepo, requestedRef) {
    this.invalidateExpired();
    for (const entry of this.entries.values()) {
      if (entry.ownerRepo !== ownerRepo) continue;
      if (entry.requestedRef === "_blobs" || !entry.treeSha || !entry.commitSha) continue;
      if (requestedRef == null) {
        if (resolveRefName(entry.defaultBranch) === entry.resolvedRef) return entry;
        continue;
      }
      if (
        entry.requestedRef === requestedRef ||
        entry.resolvedRef === requestedRef ||
        entry.resolvedRef === resolveRefName(requestedRef) ||
        entry.commitSha === requestedRef
      ) {
        return entry;
      }
    }
    return null;
  }

  /**
   * Drop the live open entry for repo@ref (a `?fresh=1` request) while
   * keeping its blobs reachable: blobs are content addressed by SHA, so they
   * move to the synthetic `_blobs` bucket instead of being refetched.
   *
   * @param {string} ownerRepo
   * @param {string} [requestedRef]
   * @returns {boolean} whether an entry was evicted
   */
  evictOpenEntry(ownerRepo, requestedRef) {
    const entry = this.findOpenEntry(ownerRepo, requestedRef);
    if (!entry) return false;
    this.entries.delete(repoCacheKey(entry.ownerRepo, entry.resolvedRef));
    const blobShas = Object.keys(entry.blobs ?? {});
    if (blobShas.length > 0) {
      const key = repoCacheKey(ownerRepo, "refs/heads/_blobs");
      const bucket = this.entries.get(key) ?? {
        ownerRepo,
        requestedRef: "_blobs",
        resolvedRef: "refs/heads/_blobs",
        commitSha: "",
        treeSha: "",
        defaultBranch: "main",
        private: false,
        tree: [],
        blobs: {},
        cachedAt: this.now(),
        ttlMs: entry.ttlMs,
      };
      bucket.blobs = { ...bucket.blobs, ...entry.blobs };
      this.entries.set(key, bucket);
    }
    return true;
  }

  /**
   * Insert or replace entries without clearing the rest (persistent-cache
   * hydrate). Existing blobs for the same key are kept.
   * @param {RepoCacheEntry[]} rows
   */
  upsertEntries(rows) {
    for (const row of rows) {
      if (!row?.ownerRepo || !row?.resolvedRef) continue;
      const key = repoCacheKey(row.ownerRepo, row.resolvedRef);
      const prev = this.entries.get(key);
      const entry = { ...row, blobs: row.blobs ?? {}, tree: row.tree ?? [] };
      if (prev && !this.isExpired(prev)) {
        entry.blobs = { ...prev.blobs, ...entry.blobs };
      }
      this.entries.set(key, entry);
    }
    this.invalidateExpired();
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
        tree: row.tree ?? [],
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
}

const IDB_NAME = "wit-snapshot-host-cache";
const IDB_STORE = "repos";
const IDB_VERSION = 1;

/**
 * Persist cache entries to IndexedDB (browser). No-op when IDB is unavailable.
 * @param {RepoSnapshotCache} cache
 */
export async function persistCacheToIdb(cache) {
  if (typeof indexedDB === "undefined") return;
  const rows = cache.dumpEntries();
  const db = await openIdb();
  try {
    await new Promise((resolve, reject) => {
      const tx = db.transaction(IDB_STORE, "readwrite");
      const store = tx.objectStore(IDB_STORE);
      store.clear();
      for (const row of rows) {
        store.put(row, repoCacheKey(row.ownerRepo, row.resolvedRef));
      }
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
  } finally {
    db.close();
  }
}

/**
 * Hydrate an in-memory cache from IndexedDB.
 * @param {RepoSnapshotCache} cache
 */
export async function hydrateCacheFromIdb(cache) {
  if (typeof indexedDB === "undefined") return;
  const db = await openIdb();
  try {
    const rows = await new Promise((resolve, reject) => {
      const tx = db.transaction(IDB_STORE, "readonly");
      const req = tx.objectStore(IDB_STORE).getAll();
      req.onsuccess = () => resolve(req.result ?? []);
      req.onerror = () => reject(req.error);
    });
    cache.loadEntries(rows);
  } finally {
    db.close();
  }
}

function openIdb() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(IDB_NAME, IDB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(IDB_STORE)) {
        db.createObjectStore(IDB_STORE);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

/**
 * Parse demo TTL from `?ttlMs=` / `?ttl=` (seconds) query params.
 * @param {string} [search]
 */
export function ttlFromSearchParams(search = "") {
  const q = new URLSearchParams(search.startsWith("?") ? search : `?${search}`);
  if (q.has("ttlMs")) {
    const n = Number(q.get("ttlMs"));
    if (Number.isFinite(n) && n >= 0) return n;
  }
  if (q.has("ttl")) {
    const raw = q.get("ttl");
    const n = Number(raw);
    if (Number.isFinite(n) && n >= 0) {
      // Values >= 1000 are treated as milliseconds; smaller as seconds for QA.
      return n >= 1000 ? n : n * 1000;
    }
  }
  return null;
}

/**
 * @param {number} ms
 */
export function formatRemaining(ms) {
  if (ms <= 0) return "expired";
  const sec = Math.ceil(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  const rem = sec % 60;
  if (min < 60) return `${min}m ${rem}s`;
  const hr = Math.floor(min / 60);
  const m2 = min % 60;
  return `${hr}h ${m2}m`;
}

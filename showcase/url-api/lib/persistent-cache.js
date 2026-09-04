/**
 * Workers-KV-backed persistence for the worker's RepoSnapshotCache.
 *
 * The isolate-lifetime sync Map stays the hot path (wasm imports are sync);
 * this layer hydrates it from KV before a request and persists new entries
 * after, so a cold isolate can still skip GitHub. Same role as the browser's
 * IndexedDB hydrate/persist — not a third SnapshotBackend.
 *
 * Why KV and not a Durable Object: a Pages advanced-mode `_worker.js` cannot
 * export DO classes (a DO would need a second deployed Worker service), the
 * cached data is immutable content addressed by SHA so KV's eventual
 * consistency is harmless, the only mutable piece (ref → commit) carries the
 * cache TTL which maps directly onto KV `expirationTtl`, and KV's 25 MB value
 * limit fits recursive trees and blobs that would need chunking under a DO's
 * per-value storage limit. See docs/adr/0006-url-api-kv-persistent-cache.md.
 *
 * Storage layout (all values JSON, all with expirationTtl = entry TTL):
 * - `v1:repo:{ownerRepo}@{resolvedRef}` → RepoCacheEntry without blobs
 * - `v1:default:{ownerRepo}`            → { defaultBranch }
 * - `v1:blob:{ownerRepo}:{sha}`         → { size, contentBase64 }
 * Blobs are separate keys so a `cat` never rewrites the tree entry and a
 * tree refresh never drops cached blobs.
 */

import { reconstructBlobJson, repoCacheKey, resolveRefName } from "./repo-cache.js";

/**
 * Isolate-lifetime memory of what already lives in KV, keyed by the sync
 * cache instance. Without it every warm request re-`put`s every blob and
 * tree row the isolate holds (write amplification: N requests → N × rows KV
 * writes, against KV's daily write quota and 1 write/sec/key limit).
 *
 * @type {WeakMap<object, { entries: Map<string, number>, blobs: Set<string> }>}
 */
const PERSISTED = new WeakMap();

/**
 * @param {object} cache
 */
export function persistedState(cache) {
  let state = PERSISTED.get(cache);
  if (!state) {
    state = { entries: new Map(), blobs: new Set() };
    PERSISTED.set(cache, state);
  }
  return state;
}

/** KV rejects expirationTtl below 60 seconds. */
const KV_MIN_TTL_SECONDS = 60;
/** Stay under KV's 25 MB value limit with headroom. */
const KV_MAX_VALUE_BYTES = 24 * 1024 * 1024;
const KEY_PREFIX = "v1";

/**
 * @typedef {{
 *   get(key: string, type: 'json'): Promise<unknown>,
 *   put(key: string, value: string, opts?: { expirationTtl?: number }): Promise<unknown>,
 * }} KvNamespaceLike
 */

export class KvRepoCache {
  /**
   * One instance per request: it tracks what was hydrated from KV so the
   * post-request persist only writes rows this request actually produced.
   *
   * @param {KvNamespaceLike} kv
   * @param {{ prefix?: string }} [opts]
   */
  constructor(kv, opts = {}) {
    this.kv = kv;
    this.prefix = opts.prefix ?? KEY_PREFIX;
    /** @type {Set<string>} blob shas served from KV this request */
    this.hydratedBlobShas = new Set();
    /** @type {Map<string, number>} repoKey → cachedAt as loaded from KV */
    this.hydratedEntries = new Map();
  }

  /** @param {string} ownerRepo @param {string} resolvedRef */
  entryKey(ownerRepo, resolvedRef) {
    return `${this.prefix}:repo:${repoCacheKey(ownerRepo, resolvedRef)}`;
  }

  /** @param {string} ownerRepo */
  defaultBranchKey(ownerRepo) {
    return `${this.prefix}:default:${ownerRepo}`;
  }

  /** @param {string} ownerRepo @param {string} sha */
  blobKey(ownerRepo, sha) {
    return `${this.prefix}:blob:${ownerRepo}:${sha}`;
  }

  /**
   * Load the repo@ref entry (tree + metadata, no blobs) from KV into the
   * sync cache, so prefetchOpen and the wasm open sequence skip GitHub.
   * No-op when the isolate cache already has a live open entry.
   *
   * @param {import('./repo-cache.js').RepoSnapshotCache} cache
   * @param {string} ownerRepo
   * @param {string | null} requestedRef
   */
  async hydrateOpen(cache, ownerRepo, requestedRef) {
    if (cache.findOpenEntry(ownerRepo, requestedRef ?? undefined)) return;

    let ref = requestedRef;
    if (!ref) {
      const def = await this.kv.get(this.defaultBranchKey(ownerRepo), "json");
      if (!def || typeof def.defaultBranch !== "string") return;
      ref = def.defaultBranch;
    }

    const row = await this.kv.get(this.entryKey(ownerRepo, resolveRefName(ref)), "json");
    if (!row || row.ownerRepo !== ownerRepo || !row.treeSha || !row.commitSha) return;
    cache.upsertEntries([row]);
    if (cache.findOpenEntry(ownerRepo, requestedRef ?? undefined)) {
      const key = repoCacheKey(row.ownerRepo, row.resolvedRef);
      this.hydratedEntries.set(key, row.cachedAt);
      persistedState(cache).entries.set(key, row.cachedAt);
    }
  }

  /**
   * Load one blob from KV into the sync cache before wasm read.
   * No-op when any live entry already carries the blob.
   *
   * @param {import('./repo-cache.js').RepoSnapshotCache} cache
   * @param {string} ownerRepo
   * @param {string} blobSha
   */
  async hydrateBlob(cache, ownerRepo, blobSha) {
    if (cache.findEntryWithBlob(ownerRepo, blobSha)) return;
    const blob = await this.kv.get(this.blobKey(ownerRepo, blobSha), "json");
    if (!blob || typeof blob.contentBase64 !== "string") return;
    // Attach through the same path GitHub responses take.
    cache.getOrFetch(`/repos/${ownerRepo}/git/blobs/${blobSha}`, () => ({
      status: 200,
      body: reconstructBlobJson(blobSha, blob),
    }));
    this.hydratedBlobShas.add(blobSha);
    persistedState(cache).blobs.add(this.blobKey(ownerRepo, blobSha));
  }

  /**
   * Persist this repo's live cache entries and blobs to KV. Rows hydrated
   * from KV, or already written by an earlier request in this isolate, are
   * skipped; oversized values are skipped rather than failed. Values expire
   * together with the in-memory TTL.
   *
   * @param {import('./repo-cache.js').RepoSnapshotCache} cache
   * @param {string} ownerRepo
   * @returns {Promise<number>} number of KV writes issued
   */
  async persistRepo(cache, ownerRepo) {
    /** @type {Promise<unknown>[]} */
    const puts = [];
    const persisted = persistedState(cache);
    for (const entry of cache.dumpEntries()) {
      if (entry.ownerRepo !== ownerRepo) continue;
      const expirationTtl = Math.max(
        KV_MIN_TTL_SECONDS,
        Math.ceil(cache.remainingMs(entry) / 1000),
      );

      for (const [sha, blob] of Object.entries(entry.blobs ?? {})) {
        const blobKey = this.blobKey(ownerRepo, sha);
        if (this.hydratedBlobShas.has(sha) || persisted.blobs.has(blobKey)) continue;
        const body = JSON.stringify(blob);
        if (body.length > KV_MAX_VALUE_BYTES) continue;
        persisted.blobs.add(blobKey);
        puts.push(this.kv.put(blobKey, body, { expirationTtl }));
      }

      // The synthetic `_blobs` bucket has no open sequence worth persisting.
      if (entry.requestedRef === "_blobs" || !entry.treeSha || !entry.commitSha) continue;

      const key = repoCacheKey(entry.ownerRepo, entry.resolvedRef);
      if (this.hydratedEntries.get(key) === entry.cachedAt) continue;
      if (persisted.entries.get(key) === entry.cachedAt) continue;
      const body = JSON.stringify({ ...entry, blobs: {} });
      if (body.length > KV_MAX_VALUE_BYTES) continue;
      persisted.entries.set(key, entry.cachedAt);
      puts.push(this.kv.put(this.entryKey(ownerRepo, entry.resolvedRef), body, { expirationTtl }));
      if (
        entry.defaultBranch &&
        resolveRefName(entry.defaultBranch) === entry.resolvedRef
      ) {
        puts.push(
          this.kv.put(
            this.defaultBranchKey(ownerRepo),
            JSON.stringify({ defaultBranch: entry.defaultBranch }),
            { expirationTtl },
          ),
        );
      }
    }
    const results = await Promise.allSettled(puts);
    const failed = results.filter((r) => r.status === "rejected");
    if (failed.length > 0) {
      // Let a later request retry the rows this one could not write.
      persisted.entries.clear();
      persisted.blobs.clear();
      throw failed[0].reason;
    }
    return puts.length;
  }
}

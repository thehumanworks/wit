import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { RepoSnapshotCache, ttlFromSearchParams } from "../lib/repo-cache.js";

const REPO = "o/r";
const repoBody = JSON.stringify({ private: false, default_branch: "main" });
const commitBody = JSON.stringify({ sha: "c".repeat(40), commit: { tree: { sha: "treesha" } } });
const treeBody = JSON.stringify({
  sha: "treesha",
  truncated: false,
  tree: [{ path: "a.txt", type: "blob", sha: "blob-a", size: 2 }],
});
const blobBody = JSON.stringify({ sha: "blob-a", size: 2, encoding: "base64", content: "aGk=" });

function openSequence(cache, opts) {
  cache.getOrFetch(`/repos/${REPO}`, () => ({ status: 200, body: repoBody }), opts);
  cache.getOrFetch(`/repos/${REPO}/commits/main`, () => ({ status: 200, body: commitBody }), opts);
  cache.getOrFetch(`/repos/${REPO}/git/trees/treesha?recursive=1`, () => ({ status: 200, body: treeBody }), opts);
}

describe("RepoSnapshotCache TTL override", () => {
  it("uses the per-call ttlMs for the entry it creates without touching the cache default", () => {
    const cache = new RepoSnapshotCache({ ttlMs: 60_000, now: () => 1000 });
    openSequence(cache, { ttlMs: 5_000 });
    const entry = cache.findOpenEntry(REPO);
    assert.equal(entry.ttlMs, 5000);
    assert.equal(cache.ttlMs, 60_000);
    assert.equal(cache.remainingMs(entry), 5000);
  });

  it("falls back to the cache default when ttlMs is absent or invalid", () => {
    const cache = new RepoSnapshotCache({ ttlMs: 60_000 });
    openSequence(cache, { ttlMs: "nope" });
    assert.equal(cache.findOpenEntry(REPO).ttlMs, 60_000);
  });
});

describe("RepoSnapshotCache.evictOpenEntry", () => {
  it("drops the open entry but keeps its blobs reachable by sha", () => {
    const cache = new RepoSnapshotCache({ ttlMs: 60_000 });
    openSequence(cache);
    cache.getOrFetch(`/repos/${REPO}/git/blobs/blob-a`, () => ({ status: 200, body: blobBody }));
    assert.ok(cache.findEntryWithBlob(REPO, "blob-a"));

    assert.equal(cache.evictOpenEntry(REPO), true);
    assert.equal(cache.findOpenEntry(REPO), null);
    assert.ok(cache.findEntryWithBlob(REPO, "blob-a"), "blob moved to the synthetic bucket");
    const hit = cache.getOrFetch(`/repos/${REPO}/git/blobs/blob-a`, () => null);
    assert.equal(hit.outcome, "hit");
    assert.equal(cache.evictOpenEntry(REPO), false, "nothing left to evict");

    // Re-open lands a fresh entry; the blob is still served from cache.
    openSequence(cache);
    assert.ok(cache.findOpenEntry(REPO));
    assert.equal(cache.getOrFetch(`/repos/${REPO}/git/blobs/blob-a`, () => null).outcome, "hit");
  });

  it("evicts only the requested ref", () => {
    const cache = new RepoSnapshotCache({ ttlMs: 60_000 });
    openSequence(cache);
    cache.getOrFetch(`/repos/${REPO}/commits/dev`, () => ({
      status: 200,
      body: JSON.stringify({ sha: "d".repeat(40), commit: { tree: { sha: "treesha-dev" } } }),
    }));
    cache.getOrFetch(`/repos/${REPO}/git/trees/treesha-dev?recursive=1`, () => ({ status: 200, body: treeBody.replace("treesha", "treesha-dev") }));
    assert.ok(cache.findOpenEntry(REPO, "dev"));
    assert.equal(cache.evictOpenEntry(REPO, "dev"), true);
    assert.equal(cache.findOpenEntry(REPO, "dev"), null);
    assert.ok(cache.findOpenEntry(REPO), "default branch entry survives");
  });
});

describe("refs sharing one tree", () => {
  it("a second ref pointing at an already cached tree still gets its own open entry", () => {
    const cache = new RepoSnapshotCache({ ttlMs: 60_000 });
    openSequence(cache);
    // A freshly created branch has the same tree as main.
    cache.getOrFetch(`/repos/${REPO}/commits/feature`, () => ({ status: 200, body: commitBody }));
    const tree = cache.getOrFetch(`/repos/${REPO}/git/trees/treesha?recursive=1`, () => {
      throw new Error("tree must be served from the cached entry");
    });
    assert.equal(tree.repoKey, `${REPO}@refs/heads/feature`);
    const feature = cache.findOpenEntry(REPO, "feature");
    assert.ok(feature, "feature branch has its own entry");
    assert.equal(feature.treeSha, "treesha");
    assert.ok(cache.findOpenEntry(REPO), "main entry is untouched");
    // The wasm open sequence for the feature ref is now fully served from cache.
    assert.equal(cache.getOrFetch(`/repos/${REPO}/commits/feature`, () => null).outcome, "hit");
  });
});

describe("ttlFromSearchParams", () => {
  it("reads ttlMs and ttl (seconds under 1000)", () => {
    assert.equal(ttlFromSearchParams("?ttlMs=1500"), 1500);
    assert.equal(ttlFromSearchParams("?ttl=5"), 5000);
    assert.equal(ttlFromSearchParams("?ttl=5000"), 5000);
    assert.equal(ttlFromSearchParams("?ttl=-1"), null);
    assert.equal(ttlFromSearchParams(""), null);
  });
});

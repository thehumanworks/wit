/**
 * Node tests for host-owned per-repo snapshot cache.
 * Run: node --test crates/wit-snapshot/demo/browser/repo-cache.test.js
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  DEFAULT_TTL_MS,
  RepoSnapshotCache,
  parseGitHubApiPath,
  slimTreeFromGitHubJson,
  ttlFromSearchParams,
} from "./repo-cache.js";

const TREE_JSON = JSON.stringify({
  sha: "treesha",
  truncated: false,
  tree: [
    { path: "README.md", type: "blob", sha: "blob-readme", size: 14 },
    { path: "src", type: "tree", sha: "tree-src" },
  ],
});

const BLOB_JSON = JSON.stringify({
  sha: "blob-readme",
  size: 14,
  encoding: "base64",
  content: "SGVsbG8sIG1lbW9yeSE=",
});

const REPO_JSON = JSON.stringify({ private: false, default_branch: "main" });
const COMMIT_JSON = JSON.stringify({
  sha: "abc123commit",
  commit: { tree: { sha: "treesha" } },
});

function fixtureMap(ownerRepo = "demo/repo") {
  const map = new Map();
  const put = (path, body) => {
    map.set(path, { status: 200, body });
    map.set(`https://api.github.com${path}`, { status: 200, body });
  };
  put(`/repos/${ownerRepo}`, REPO_JSON);
  put(`/repos/${ownerRepo}/commits/main`, COMMIT_JSON);
  put(`/repos/${ownerRepo}/git/trees/treesha?recursive=1`, TREE_JSON);
  put(`/repos/${ownerRepo}/git/blobs/blob-readme`, BLOB_JSON);
  return map;
}

function countingFetch(map) {
  let calls = 0;
  const paths = [];
  return {
    get calls() {
      return calls;
    },
    get paths() {
      return paths;
    },
    fetch(path) {
      calls += 1;
      paths.push(path);
      return map.get(path) ?? null;
    },
  };
}

function openSequence(cache, fetch, ownerRepo = "demo/repo") {
  const a = cache.getOrFetch(`/repos/${ownerRepo}`, fetch);
  const b = cache.getOrFetch(`/repos/${ownerRepo}/commits/main`, fetch);
  const c = cache.getOrFetch(`/repos/${ownerRepo}/git/trees/treesha?recursive=1`, fetch);
  return [a, b, c];
}

describe("parseGitHubApiPath", () => {
  it("parses repo / commit / tree / blob", () => {
    assert.equal(parseGitHubApiPath("/repos/demo/repo").kind, "repo");
    assert.deepEqual(parseGitHubApiPath("/repos/demo/repo/commits/main"), {
      kind: "commit",
      ownerRepo: "demo/repo",
      ref: "main",
    });
    assert.equal(
      parseGitHubApiPath("/repos/demo/repo/git/trees/treesha?recursive=1").kind,
      "tree",
    );
    assert.equal(parseGitHubApiPath("/repos/demo/repo/git/blobs/blob-readme").blobSha, "blob-readme");
  });
});

describe("slimTreeFromGitHubJson", () => {
  it("keeps path/type/sha/size only", () => {
    const slim = slimTreeFromGitHubJson(TREE_JSON);
    assert.deepEqual(slim, [
      { path: "README.md", type: "blob", sha: "blob-readme", size: 14 },
      { path: "src", type: "tree", sha: "tree-src" },
    ]);
  });
});

describe("ttlFromSearchParams", () => {
  it("reads ttlMs and short ttl seconds", () => {
    assert.equal(ttlFromSearchParams("?ttlMs=1500"), 1500);
    assert.equal(ttlFromSearchParams("?ttl=5"), 5000);
    assert.equal(ttlFromSearchParams("?ttl=5000"), 5000);
    assert.equal(ttlFromSearchParams(""), null);
  });
});

describe("RepoSnapshotCache", () => {
  it("defaults to 24h TTL", () => {
    assert.equal(DEFAULT_TTL_MS, 24 * 60 * 60 * 1000);
    const cache = new RepoSnapshotCache();
    assert.equal(cache.ttlMs, DEFAULT_TTL_MS);
  });

  it("cache hit serves without a second fixture fetch", () => {
    const map = fixtureMap();
    const counter = countingFetch(map);
    let now = 1_000_000;
    const cache = new RepoSnapshotCache({ ttlMs: 60_000, now: () => now });

    const first = openSequence(cache, (p) => counter.fetch(p));
    assert.equal(counter.calls, 3);
    assert.ok(first.every((r) => r.outcome === "miss"));

    const second = openSequence(cache, (p) => counter.fetch(p));
    assert.equal(counter.calls, 3, "second open must not touch fixtures");
    assert.ok(second.every((r) => r.outcome === "hit"));
    assert.match(second[2].body, /README\.md/);

    const blob1 = cache.getOrFetch("/repos/demo/repo/git/blobs/blob-readme", (p) =>
      counter.fetch(p),
    );
    assert.equal(blob1.outcome, "miss");
    assert.equal(counter.calls, 4);

    const blob2 = cache.getOrFetch("/repos/demo/repo/git/blobs/blob-readme", (p) =>
      counter.fetch(p),
    );
    assert.equal(blob2.outcome, "hit");
    assert.equal(counter.calls, 4, "blob hit must not refetch");
    assert.match(blob2.body, /SGVsbG8/);
  });

  it("expiry forces refetch for that repo", () => {
    const map = fixtureMap();
    const counter = countingFetch(map);
    let now = 1_000_000;
    const cache = new RepoSnapshotCache({ ttlMs: 1_000, now: () => now });

    openSequence(cache, (p) => counter.fetch(p));
    assert.equal(counter.calls, 3);

    now += 1_001; // past TTL
    const after = openSequence(cache, (p) => counter.fetch(p));
    assert.equal(counter.calls, 6, "expired open must refetch all three");
    assert.ok(after.every((r) => r.outcome === "miss"));

    // And the new entry is fresh again
    const again = openSequence(cache, (p) => counter.fetch(p));
    assert.equal(counter.calls, 6);
    assert.ok(again.every((r) => r.outcome === "hit"));
  });

  it("two repos have independent TTLs", () => {
    const mapA = fixtureMap("demo/repo");
    const mapB = fixtureMap("other/repo");
    const map = new Map([...mapA, ...mapB]);
    const counter = countingFetch(map);
    let now = 5_000_000;
    const cache = new RepoSnapshotCache({ ttlMs: 10_000, now: () => now });

    openSequence(cache, (p) => counter.fetch(p), "demo/repo");
    now += 2_000; // demo/repo has 8s left
    openSequence(cache, (p) => counter.fetch(p), "other/repo");
    assert.equal(counter.calls, 6);

    now += 9_000; // demo/repo expired (11s total); other/repo still has ~1s
    const demoAfter = openSequence(cache, (p) => counter.fetch(p), "demo/repo");
    assert.ok(demoAfter.every((r) => r.outcome === "miss"));
    assert.equal(counter.calls, 9, "only demo/repo refetched");

    const otherHit = openSequence(cache, (p) => counter.fetch(p), "other/repo");
    assert.ok(otherHit.every((r) => r.outcome === "hit"));
    assert.equal(counter.calls, 9, "other/repo still served from cache");

    const rows = cache.statusRows();
    const demo = rows.find((r) => r.ownerRepo === "demo/repo");
    const other = rows.find((r) => r.ownerRepo === "other/repo");
    assert.ok(demo);
    assert.ok(other);
    assert.ok(demo.remainingMs > 0);
    assert.ok(other.remainingMs > 0);
    assert.notEqual(demo.remainingMs, other.remainingMs);
  });

  it("after expiry, failed refetch passes status/body through and drops entry", () => {
    const map = fixtureMap();
    let now = 2_000_000;
    const cache = new RepoSnapshotCache({ ttlMs: 500, now: () => now });

    openSequence(cache, (p) => map.get(p) ?? null);
    assert.equal(cache.statusRows().length, 1);
    assert.ok(cache.findEntry("demo/repo"));

    now += 501; // expire demo/repo

    const failBody = JSON.stringify({ message: "API rate limit exceeded" });
    const result = cache.getOrFetch("/repos/demo/repo", () => ({
      status: 403,
      body: failBody,
    }));

    // Pass through the typed failure — do not invent a 200 / slim success body.
    assert.equal(result.status, 403);
    assert.equal(result.body, failBody);
    assert.equal(result.outcome, "miss");
    assert.doesNotMatch(result.body, /default_branch/);
    assert.doesNotMatch(result.body, /"private"/);

    // Expired entry must be gone; failure must not re-cache.
    assert.equal(cache.findEntry("demo/repo"), null);
    assert.equal(cache.statusRows().length, 0);
    assert.equal(cache.dumpEntries().length, 0);

    // Same for a mid-open 404 after expiry (commit path).
    openSequence(cache, (p) => map.get(p) ?? null);
    assert.equal(cache.statusRows().length, 1);
    now += 501;

    const notFound = JSON.stringify({ message: "Not Found" });
    const commitFail = cache.getOrFetch("/repos/demo/repo/commits/main", () => ({
      status: 404,
      body: notFound,
    }));
    assert.equal(commitFail.status, 404);
    assert.equal(commitFail.body, notFound);
    assert.equal(commitFail.outcome, "miss");
    assert.doesNotMatch(commitFail.body, /abc123commit/);
    assert.equal(cache.findEntry("demo/repo"), null);
    assert.equal(cache.dumpEntries().length, 0);
  });
});

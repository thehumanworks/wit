/**
 * KV-backed persistent cache tests: unit coverage for KvRepoCache and a
 * cold-isolate round trip through handleRequest (second "isolate" serves
 * tree/cat from KV with zero GitHub fetches).
 */

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { after, before, describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { createHostCache, handleRequest } from "../lib/handle.js";
import { prefetchOpen } from "../lib/github.js";
import { KvRepoCache } from "../lib/persistent-cache.js";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const cassetteDir = join(root, "../../crates/wit-snapshot/tests/cassettes");

const REPO = "demo/repo";

/** In-memory KV namespace double recording expirationTtl per key. */
class FakeKv {
  constructor() {
    /** @type {Map<string, string>} */
    this.store = new Map();
    /** @type {Map<string, number | undefined>} */
    this.ttls = new Map();
    this.puts = 0;
    this.gets = 0;
  }

  async get(key, type) {
    this.gets += 1;
    const raw = this.store.get(key);
    if (raw == null) return null;
    return type === "json" ? JSON.parse(raw) : raw;
  }

  async put(key, value, opts = {}) {
    this.puts += 1;
    this.store.set(key, value);
    this.ttls.set(key, opts.expirationTtl);
  }
}

function makeEntry(overrides = {}) {
  return {
    ownerRepo: REPO,
    requestedRef: "main",
    resolvedRef: "refs/heads/main",
    commitSha: "commitsha",
    treeSha: "treesha",
    defaultBranch: "main",
    private: false,
    tree: [{ path: "README.md", type: "blob", sha: "blob-readme", size: 12 }],
    blobs: { "blob-readme": { size: 12, contentBase64: "aGVsbG8gd29ybGQh" } },
    cachedAt: Date.now(),
    ttlMs: 60_000,
    ...overrides,
  };
}

describe("KvRepoCache unit", () => {
  it("persistRepo writes entry, default alias, and blob keys with TTLs", async () => {
    const kv = new FakeKv();
    const cache = createHostCache({ ttlMs: 60_000 });
    cache.upsertEntries([makeEntry()]);

    await new KvRepoCache(kv).persistRepo(cache, REPO);

    const entryKey = `v1:repo:${REPO}@refs/heads/main`;
    const stored = JSON.parse(kv.store.get(entryKey));
    assert.equal(stored.treeSha, "treesha");
    assert.deepEqual(stored.blobs, {}, "entry row must not embed blobs");
    assert.deepEqual(JSON.parse(kv.store.get(`v1:default:${REPO}`)), {
      defaultBranch: "main",
    });
    const blob = JSON.parse(kv.store.get(`v1:blob:${REPO}:blob-readme`));
    assert.equal(blob.contentBase64, "aGVsbG8gd29ybGQh");
    for (const key of [entryKey, `v1:blob:${REPO}:blob-readme`]) {
      assert.ok(kv.ttls.get(key) >= 60, `expirationTtl for ${key}`);
    }
  });

  it("hydrateOpen restores an entry into a cold cache, including default-branch lookups", async () => {
    const kv = new FakeKv();
    const warm = createHostCache({ ttlMs: 60_000 });
    warm.upsertEntries([makeEntry()]);
    await new KvRepoCache(kv).persistRepo(warm, REPO);

    // Explicit ref.
    const cold = createHostCache({ ttlMs: 60_000 });
    await new KvRepoCache(kv).hydrateOpen(cold, REPO, "main");
    assert.ok(cold.findOpenEntry(REPO, "main"));

    // No ref: resolved via the default-branch alias.
    const cold2 = createHostCache({ ttlMs: 60_000 });
    await new KvRepoCache(kv).hydrateOpen(cold2, REPO, null);
    assert.ok(cold2.findOpenEntry(REPO));
  });

  it("hydrateBlob restores a blob and persistRepo does not rewrite hydrated rows", async () => {
    const kv = new FakeKv();
    const warm = createHostCache({ ttlMs: 60_000 });
    warm.upsertEntries([makeEntry()]);
    await new KvRepoCache(kv).persistRepo(warm, REPO);

    const cold = createHostCache({ ttlMs: 60_000 });
    const persistent = new KvRepoCache(kv);
    await persistent.hydrateOpen(cold, REPO, "main");
    await persistent.hydrateBlob(cold, REPO, "blob-readme");
    assert.ok(cold.findEntryWithBlob(REPO, "blob-readme"));

    const putsBefore = kv.puts;
    await persistent.persistRepo(cold, REPO);
    assert.equal(kv.puts, putsBefore, "hydrated rows must not be re-written");
  });

  it("expired persisted entries are ignored on hydrate", async () => {
    const kv = new FakeKv();
    const warm = createHostCache({ ttlMs: 60_000 });
    warm.upsertEntries([makeEntry({ cachedAt: Date.now() - 120_000 })]);
    // dumpEntries drops expired rows, so persist the row manually.
    await kv.put(
      `v1:repo:${REPO}@refs/heads/main`,
      JSON.stringify({ ...makeEntry({ cachedAt: Date.now() - 120_000 }), blobs: {} }),
    );

    const cold = createHostCache({ ttlMs: 60_000 });
    await new KvRepoCache(kv).hydrateOpen(cold, REPO, "main");
    assert.equal(cold.findOpenEntry(REPO, "main"), null);
  });

  it("a cached feature branch does not answer a default-branch open", async () => {
    const cache = createHostCache({ ttlMs: 60_000 });
    cache.upsertEntries([
      makeEntry({
        requestedRef: "dev",
        resolvedRef: "refs/heads/dev",
        defaultBranch: "main",
      }),
    ]);
    assert.equal(cache.findOpenEntry(REPO), null);
    assert.ok(cache.findOpenEntry(REPO, "dev"));
  });
});

describe("prefetchOpen short-circuit", () => {
  it("returns from a live open entry without calling fetch", async () => {
    const cache = createHostCache({ ttlMs: 60_000 });
    cache.upsertEntries([makeEntry()]);
    const originalFetch = globalThis.fetch;
    let fetches = 0;
    globalThis.fetch = async () => {
      fetches += 1;
      throw new Error("network must not be touched");
    };
    try {
      const out = await prefetchOpen(cache, REPO, null, null);
      assert.equal(out.treeSha, "treesha");
      assert.equal(fetches, 0);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });
});

describe("handleRequest with KV persistence (cold isolate round trip)", () => {
  /** @type {BufferSource} */
  let wasmBytes;
  /** @type {Map<string, {status:number,body:string}>} */
  let fixtures;
  /** @type {typeof fetch} */
  let originalFetch;
  let githubFetches = 0;

  before(async () => {
    wasmBytes = await readFile(join(root, "public/wit_snapshot.wasm"));
    const load = (name) => readFile(join(cassetteDir, name), "utf8");
    fixtures = new Map();
    const put = (path, body) => {
      fixtures.set(path, { status: 200, body });
      fixtures.set(`https://api.github.com${path}`, { status: 200, body });
    };
    put(`/repos/${REPO}`, await load("demo_repo.json"));
    put(`/repos/${REPO}/commits/main`, await load("demo_commit.json"));
    put(`/repos/${REPO}/git/trees/treesha?recursive=1`, await load("demo_tree.json"));
    put(`/repos/${REPO}/git/blobs/blob-readme`, await load("demo_blob.json"));
    put(`/repos/${REPO}/git/blobs/blob-main`, await load("demo_blob_main.json"));

    originalFetch = globalThis.fetch;
    globalThis.fetch = async (input) => {
      githubFetches += 1;
      const url = String(input);
      const path = url.replace(/^https:\/\/api\.github\.com/, "");
      const hit = fixtures.get(url) || fixtures.get(path);
      if (!hit) {
        return new Response(JSON.stringify({ message: "Not Found" }), {
          status: 404,
          headers: { "content-type": "application/json" },
        });
      }
      return new Response(hit.body, {
        status: hit.status,
        headers: { "content-type": "application/json" },
      });
    };
  });

  after(() => {
    globalThis.fetch = originalFetch;
  });

  const routes = [
    "/tree/demo/repo",
    "/cat/demo/repo?path=README.md",
  ];

  it("second cold isolate serves identical bodies with zero GitHub fetches", async () => {
    const kv = new FakeKv();

    // Isolate 1: cold in-memory cache, empty KV.
    const first = {};
    for (const path of routes) {
      const res = await handleRequest(new Request(`https://example.test${path}`), {
        cache: createHostCache({ ttlMs: 60_000 }),
        persistentCache: new KvRepoCache(kv),
        loadWasmBytes: async () => wasmBytes,
      });
      assert.equal(res.status, 200);
      first[path] = await res.text();
    }
    assert.ok(githubFetches > 0, "cold isolate + cold KV must hit GitHub");
    assert.ok(kv.store.has(`v1:repo:${REPO}@refs/heads/main`));
    assert.ok(kv.store.has(`v1:blob:${REPO}:blob-readme`));

    // Isolate 2: fresh in-memory cache, warm KV.
    githubFetches = 0;
    for (const path of routes) {
      const res = await handleRequest(new Request(`https://example.test${path}`), {
        cache: createHostCache({ ttlMs: 60_000 }),
        persistentCache: new KvRepoCache(kv),
        loadWasmBytes: async () => wasmBytes,
      });
      assert.equal(res.status, 200);
      assert.equal(await res.text(), first[path], `${path} diverged from cold read`);
    }
    assert.equal(githubFetches, 0, "warm KV must fully absorb GitHub traffic");
  });

  it("a broken KV binding degrades to normal GitHub reads", async () => {
    const failingKv = {
      async get() {
        throw new Error("kv down");
      },
      async put() {
        throw new Error("kv down");
      },
    };
    githubFetches = 0;
    const res = await handleRequest(new Request("https://example.test/tree/demo/repo"), {
      cache: createHostCache({ ttlMs: 60_000 }),
      persistentCache: new KvRepoCache(failingKv),
      loadWasmBytes: async () => wasmBytes,
    });
    assert.equal(res.status, 200);
    assert.ok(githubFetches > 0);
    assert.match(await res.text(), /README\.md|main\.rs/);
  });
});

/**
 * End-to-end handler tests with fixture-backed GitHub fetch (no live network).
 * Covers routing → plaintext for tree/ls/cat and header vs query token.
 */

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { after, before, describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { createHostCache, handleRequest, scrubSecrets } from "../lib/handle.js";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const cassetteDir = join(
  root,
  "../../crates/wit-snapshot/tests/cassettes",
);

const REPO = "demo/repo";

async function loadCassette(name) {
  return readFile(join(cassetteDir, name), "utf8");
}

describe("handleRequest fixtures", () => {
  /** @type {BufferSource} */
  let wasmBytes;
  /** @type {Map<string, {status:number,body:string}>} */
  let fixtures;
  /** @type {typeof fetch} */
  let originalFetch;
  /** @type {string[]} */
  let seenAuth;

  before(async () => {
    wasmBytes = await readFile(join(root, "public/wit_snapshot.wasm"));
    const repo = await loadCassette("demo_repo.json");
    const commit = await loadCassette("demo_commit.json");
    const tree = await loadCassette("demo_tree.json");
    const blob = await loadCassette("demo_blob.json");
    const blobMain = await loadCassette("demo_blob_main.json");
    fixtures = new Map();
    const put = (path, body) => {
      fixtures.set(path, { status: 200, body });
      fixtures.set(`https://api.github.com${path}`, { status: 200, body });
    };
    put(`/repos/${REPO}`, repo);
    put(`/repos/${REPO}/commits/main`, commit);
    put(`/repos/${REPO}/git/trees/treesha?recursive=1`, tree);
    put(`/repos/${REPO}/git/blobs/blob-readme`, blob);
    put(`/repos/${REPO}/git/blobs/blob-main`, blobMain);

    originalFetch = globalThis.fetch;
    seenAuth = [];
    globalThis.fetch = async (input, init = {}) => {
      const url = String(input);
      const headers = init.headers || {};
      const auth =
        headers.Authorization ||
        headers.authorization ||
        (headers.get && headers.get("Authorization"));
      if (auth) {
        const m = String(auth).match(/^\s*(?:Bearer|token)\s+(\S+)/i);
        if (m) seenAuth.push(m[1]);
      }
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

  it("GET /tree/demo/repo returns plaintext tree", async () => {
    seenAuth.length = 0;
    const cache = createHostCache({ ttlMs: 60_000 });
    const res = await handleRequest(
      new Request("https://example.test/tree/demo/repo"),
      {
        cache,
        loadWasmBytes: async () => wasmBytes,
      },
    );
    assert.ok(res);
    assert.equal(res.status, 200);
    assert.match(res.headers.get("content-type"), /text\/plain/);
    const text = await res.text();
    assert.match(text, /^\./m);
    assert.match(text, /README\.md|main\.rs/);
  });

  it("GET /ls/demo/repo returns plaintext listing", async () => {
    const cache = createHostCache({ ttlMs: 60_000 });
    const res = await handleRequest(
      new Request("https://example.test/ls/demo/repo"),
      {
        cache,
        loadWasmBytes: async () => wasmBytes,
      },
    );
    const text = await res.text();
    assert.equal(res.status, 200);
    assert.match(text, /README\.md|src\/|main\.rs/);
  });

  it("GET /cat/demo/repo?path=README.md returns file text", async () => {
    const cache = createHostCache({ ttlMs: 60_000 });
    const res = await handleRequest(
      new Request("https://example.test/cat/demo/repo?path=README.md"),
      {
        cache,
        loadWasmBytes: async () => wasmBytes,
      },
    );
    const text = await res.text();
    assert.equal(res.status, 200);
    assert.match(text, /Hello|memory|README/i);
  });

  it("/api prefix returns the same status and plaintext as unprefixed", async () => {
    const fetchRoute = async (pathname) => {
      const res = await handleRequest(
        new Request(`https://example.test${pathname}`),
        {
          cache: createHostCache({ ttlMs: 60_000 }),
          loadWasmBytes: async () => wasmBytes,
        },
      );
      assert.ok(res, `expected a response for ${pathname}`);
      return {
        status: res.status,
        contentType: res.headers.get("content-type"),
        text: await res.text(),
      };
    };

    for (const suffix of ["tree/demo/repo", "ls/demo/repo", "cat/demo/repo?path=README.md"]) {
      const plain = await fetchRoute(`/${suffix}`);
      const prefixed = await fetchRoute(`/api/${suffix}`);
      assert.equal(plain.status, 200);
      assert.deepEqual(prefixed, plain, `/api/${suffix} diverged from /${suffix}`);
    }
  });

  it("GET /api/cat/demo/repo without ?path= is 400 path_required", async () => {
    const res = await handleRequest(
      new Request("https://example.test/api/cat/demo/repo"),
      {
        cache: createHostCache({ ttlMs: 60_000 }),
        loadWasmBytes: async () => wasmBytes,
      },
    );
    assert.ok(res);
    assert.equal(res.status, 400);
    assert.match(await res.text(), /cat requires \?path=/);
  });

  it("Authorization header wins over ?token=", async () => {
    seenAuth.length = 0;
    const cache = createHostCache({ ttlMs: 60_000 });
    await handleRequest(
      new Request(
        "https://example.test/tree/demo/repo?token=query-should-lose",
        { headers: { Authorization: "Bearer header-wins" } },
      ),
      {
        cache,
        loadWasmBytes: async () => wasmBytes,
      },
    );
    assert.ok(seenAuth.includes("header-wins"));
    assert.equal(seenAuth.includes("query-should-lose"), false);
  });

  it("token never appears in error bodies", async () => {
    const cache = createHostCache({ ttlMs: 60_000 });
    const errRes = await handleRequest(
      new Request(
        "https://example.test/cat/demo/repo?path=no-such-file&token=ghp_SHOULD_NOT_LEAK",
      ),
      {
        cache,
        loadWasmBytes: async () => wasmBytes,
      },
    );
    const errText = await errRes.text();
    assert.equal(errText.includes("ghp_SHOULD_NOT_LEAK"), false);
    assert.equal(scrubSecrets(errText).includes("ghp_SHOULD_NOT_LEAK"), false);
    assert.match(errText, /^error: /);
  });
});

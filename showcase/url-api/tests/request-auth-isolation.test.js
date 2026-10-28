import assert from "node:assert/strict";
import { it } from "node:test";
import { handleRequest, createHostCache } from "../lib/handle.js";
import { fixtureGitHub, wasmBytes } from "./helpers.js";

it("caller credentials and sensitive headers never enter shared caches or deferred persistence", async () => {
  const github = fixtureGitHub({ files: { "README.md": "fixture content" } });
  github.install();
  try {
    const shared = createHostCache();
    const forbidden = async () => { assert.fail("sensitive request touched persistent cache"); };
    const deps = {
      loadWasmBytes: wasmBytes, cache: shared,
      persistentCache: { hydrateOpen: forbidden, hydrateBlob: forbidden, persistRepo: forbidden },
      waitUntil: () => assert.fail("sensitive request scheduled background persistence"),
    };
    for (const headers of [
      { Authorization: "Bearer synthetic-caller" },
      { Authorization: "Basic c3ludGhldGlj" },
      { Cookie: "session=synthetic-cookie" },
    ]) {
      const response = await handleRequest(new Request("https://example.test/api/cat/demo/repo?path=README.md", { headers }), deps);
      assert.equal(response.status, 200);
      assert.equal(response.headers.get("cache-control"), "private, no-store");
      assert.match(await response.text(), /fixture content/);
      assert.equal(shared.dumpEntries().length, 0);
    }
  } finally {
    github.restore();
  }
});

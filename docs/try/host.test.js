import assert from "node:assert/strict";
import { test } from "node:test";
import { RELEASE_WASM_URL, wasmCandidates } from "./host.js";

test("wasmCandidates is same-origin then v0.1.33 only", () => {
  const urls = wasmCandidates("https://thehumanworks.github.io/try/host.js");
  assert.deepEqual(urls, [
    "https://thehumanworks.github.io/try/wit_snapshot.wasm",
    RELEASE_WASM_URL,
  ]);
  assert.equal(urls.length, 2);
  assert.equal(
    RELEASE_WASM_URL,
    "https://github.com/thehumanworks/wit/releases/download/v0.1.33/wit_snapshot.wasm",
  );
  for (const url of urls) {
    assert.doesNotMatch(url, /\/target\//);
    assert.doesNotMatch(url, /\.\.\/\.\.\/target\//);
  }
});

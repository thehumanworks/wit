/**
 * Node smoke: load the existing wit_snapshot.wasm with the fixture host
 * and assert CLI plaintext for `wit tree demo/repo`.
 */
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildFixtureMap, FIXTURE_FILES, makeImports } from "./host.js";
import { runLine } from "./run.js";

const here = dirname(fileURLToPath(import.meta.url));

function wasmPaths() {
  return [
    resolve(here, "wit_snapshot.wasm"),
    resolve(here, "../../target/wasm32-unknown-unknown/release/wit_snapshot.wasm"),
    resolve(here, "../../target/wasm32-unknown-unknown/debug/wit_snapshot.wasm"),
  ];
}

async function loadLocalWasmBytes() {
  let lastErr = null;
  for (const path of wasmPaths()) {
    try {
      return { bytes: await readFile(path), path };
    } catch (err) {
      lastErr = err;
    }
  }
  throw lastErr || new Error("wit_snapshot.wasm not found (build or copy it)");
}

const texts = {};
for (const name of FIXTURE_FILES) {
  texts[name] = await readFile(resolve(here, "fixtures", name), "utf8");
}
const fixtures = buildFixtureMap(texts);

const { bytes, path: wasmPath } = await loadLocalWasmBytes();
let exports = null;
const imports = makeImports(() => exports, { fixtures });
const { instance } = await WebAssembly.instantiate(bytes, imports);
exports = instance.exports;
if (!exports.memory || !exports.wit_snapshot_open) {
  throw new Error("wasm exports missing");
}

const tree = runLine(exports, "wit tree demo/repo");
assert.equal(tree.kind, "ok", tree.text);
assert.equal(tree.text, ".\n  README.md\n  src/main.rs");

const ls = runLine(exports, "wit ls demo/repo");
assert.equal(ls.kind, "ok", ls.text);
assert.equal(ls.text, "src/\nREADME.md");

const cat = runLine(exports, "wit cat demo/repo README.md");
assert.equal(cat.kind, "ok", cat.text);
assert.equal(cat.text, "Hello, memory!");

const bad = runLine(exports, "wit rg foo demo/repo");
assert.equal(bad.kind, "error");
assert.match(bad.text, /not available/);

const missing = runLine(exports, "wit tree missing/repo");
assert.equal(missing.kind, "error");
assert.match(missing.text, /failed: /);

console.log(`docs site smoke: ok (${wasmPath})`);
console.log(tree.text);

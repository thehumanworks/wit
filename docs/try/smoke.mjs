/**
 * Node smoke: load the existing wit_snapshot.wasm with the fixture host
 * and assert CLI plaintext for demo/repo read verbs.
 */
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildFixtureMap,
  FIXTURE_FILES,
  makeImports,
  prefetchLiveGithub,
  putGithubJson,
  searchRepositoriesPath,
} from "./host.js";
import { headFromText } from "./format.js";
import { runLine } from "./run.js";

const here = dirname(fileURLToPath(import.meta.url));

async function loadLocalWasmBytes() {
  // Same-origin file only. check_docs_site.sh / serve_docs_site.sh copy
  // a built module next to this script; the published host never fetches
  // from a cargo target/ tree.
  const path = resolve(here, "wit_snapshot.wasm");
  try {
    return { bytes: await readFile(path), path };
  } catch {
    throw new Error(
      "docs/try/wit_snapshot.wasm not found (copy the module next to this script)",
    );
  }
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
if (
  !exports.memory ||
  !exports.wit_snapshot_open ||
  typeof exports.wit_snapshot_search_repositories !== "function"
) {
  throw new Error("wasm exports missing (open / list / read / search_repositories)");
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

const rg = runLine(exports, "wit rg Hello demo/repo");
assert.equal(rg.kind, "ok", rg.text);
assert.equal(rg.text, "README.md:1:Hello, memory!");

const head = runLine(exports, "wit head demo/repo README.md");
assert.equal(head.kind, "ok", head.text);
assert.equal(head.text, headFromText("Hello, memory!", 10, false));

const tail = runLine(exports, "wit tail demo/repo README.md");
assert.equal(tail.kind, "ok", tail.text);
assert.equal(tail.text, "Hello, memory!");

const sed = runLine(exports, "wit sed -n '1,2p' demo/repo README.md");
assert.equal(sed.kind, "ok", sed.text);
assert.equal(sed.text, "Hello, memory!\n");

function wrapApi(api) {
  const openCalls = { n: 0 };
  const wrapped = {};
  for (const key of Object.keys(api)) {
    const value = api[key];
    if (key === "wit_snapshot_open") {
      wrapped[key] = (...args) => {
        openCalls.n += 1;
        return value(...args);
      };
    } else if (typeof value === "function") {
      wrapped[key] = (...args) => value(...args);
    } else {
      Object.defineProperty(wrapped, key, {
        get() {
          return api[key];
        },
      });
    }
  }
  return { wrapped, openCalls };
}

const { wrapped: searchApi, openCalls } = wrapApi(exports);
const search = runLine(searchApi, "wit search -p ratatui");
assert.equal(search.kind, "ok", search.text);
assert.match(search.text, /ratatui\/ratatui/);
assert.match(search.text, /stars/);
assert.equal(openCalls.n, 0, "search must not call wasm open");

putGithubJson(
  fixtures,
  searchRepositoriesPath("other in:name"),
  403,
  JSON.stringify({ message: "API rate limit exceeded" }),
);
const limited = runLine(exports, "wit search -p other");
assert.equal(limited.kind, "error", limited.text);
assert.match(limited.text, /rate_limit/);

const unavailable = runLine(exports, "wit skill load");
assert.equal(unavailable.kind, "error");
assert.match(unavailable.text, /not available/);

const missing = runLine(exports, "wit tree missing/repo");
assert.equal(missing.kind, "error");
assert.match(missing.text, /failed: /);

function cassetteFetch(bodies) {
  const routes = {
    "/repos/acme/demo": bodies["demo_repo.json"],
    "/repos/acme/demo/commits/main": bodies["demo_commit.json"],
    "/repos/acme/demo/git/trees/treesha?recursive=1": bodies["demo_tree.json"],
    "/repos/acme/demo/git/blobs/blob-readme": bodies["demo_blob.json"],
    "/repos/acme/demo/git/blobs/blob-main": bodies["demo_blob_main.json"],
  };
  return async (url) => {
    const path = String(url).replace("https://api.github.com", "");
    const body = routes[path];
    if (body == null) {
      throw new Error(`unexpected fetch ${url}`);
    }
    return { status: 200, text: async () => body };
  };
}

await prefetchLiveGithub(
  fixtures,
  { kind: "run", command: "cat", repo: "acme/demo", path: "README.md" },
  cassetteFetch(texts),
);
const liveTree = runLine(exports, "wit tree acme/demo");
assert.equal(liveTree.kind, "ok", liveTree.text);
assert.equal(liveTree.text, ".\n  README.md\n  src/main.rs");
const liveCat = runLine(exports, "wit cat acme/demo README.md");
assert.equal(liveCat.kind, "ok", liveCat.text);
assert.equal(liveCat.text, "Hello, memory!");
const treeAgain = runLine(exports, "wit tree demo/repo");
assert.equal(treeAgain.text, ".\n  README.md\n  src/main.rs");

console.log(`docs site smoke: ok (${wasmPath})`);
console.log(tree.text);

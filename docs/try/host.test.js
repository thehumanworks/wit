import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import {
  GITHUB_API,
  MAX_RG_PREFETCH_FILES,
  RELEASE_TAG,
  RELEASE_WASM_URL,
  githubGetJson,
  isFixtureRepo,
  prefetchLiveGithub,
  releaseWasmUrl,
  wasmCandidates,
} from "./host.js";
import * as host from "./host.js";

test("wasmCandidates is same-origin then stamped release URL only", () => {
  const urls = wasmCandidates("https://thehumanworks.github.io/try/host.js");
  assert.ok(urls.length >= 1 && urls.length <= 2);
  assert.equal(urls[0], "https://thehumanworks.github.io/try/wit_snapshot.wasm");
  assert.equal(RELEASE_TAG, "__WIT_RELEASE_TAG__");
  assert.equal(releaseWasmUrl("__WIT_RELEASE_TAG__"), "");
  assert.equal(releaseWasmUrl("local"), "");
  assert.equal(
    releaseWasmUrl("v9.9.9"),
    "https://github.com/thehumanworks/wit/releases/download/v9.9.9/wit_snapshot.wasm",
  );
  if (urls.length === 2) {
    assert.equal(urls[1], RELEASE_WASM_URL);
    assert.match(
      urls[1],
      /\/releases\/download\/(?:__WIT_RELEASE_TAG__|v\d+\.\d+\.\d+)\/wit_snapshot\.wasm$/,
    );
  } else {
    assert.equal(RELEASE_WASM_URL, "");
  }
  for (const url of urls) {
    assert.doesNotMatch(url, /\/target\//);
    assert.doesNotMatch(url, /\.\.\/\.\.\/target\//);
  }
});

test("live path has no sync XHR helper", async () => {
  assert.equal(host.liveGithubGetSync, undefined);
  const src = await readFile(new URL("./host.js", import.meta.url), "utf8");
  assert.doesNotMatch(src, /new XMLHttpRequest/);
  assert.doesNotMatch(src, /xhr\.open\s*\(/);
  assert.doesNotMatch(src, /open\(\s*["']GET["']\s*,[^,]+,\s*false/);
  assert.doesNotMatch(src, /\/search\/repositories/);
});

test("app.js paints processing and disables input before prefetch", async () => {
  const src = await readFile(new URL("./app.js", import.meta.url), "utf8");
  assert.doesNotMatch(src, /liveGithubGetSync/);
  assert.match(src, /async function runAndRender/);
  assert.match(src, /processing…/);
  assert.match(src, /yieldToPaint/);
  assert.match(src, /prefetchLiveGithub/);
  const processingAt = src.indexOf('appendLine("processing…", "muted")');
  const yieldAt = src.indexOf("await yieldToPaint()");
  const prefetchAt = src.indexOf("await prefetchLiveGithub");
  assert.ok(processingAt > 0 && processingAt < yieldAt && yieldAt < prefetchAt);
  assert.match(src, /input\.disabled = next/);
});

test("demo/repo is the fixture repo and never prefetches", async () => {
  assert.equal(isFixtureRepo("demo/repo"), true);
  assert.equal(isFixtureRepo("openai/codex"), false);
  let fetches = 0;
  const fetchImpl = async () => {
    fetches += 1;
    throw new Error("network should not run for demo/repo");
  };
  const fixtures = new Map();
  await prefetchLiveGithub(
    fixtures,
    { kind: "run", command: "tree", repo: "demo/repo", path: null },
    fetchImpl,
  );
  assert.equal(fetches, 0);
  assert.equal(fixtures.size, 0);
});

function jsonResponse(status, body) {
  return {
    status,
    text: async () => (typeof body === "string" ? body : JSON.stringify(body)),
  };
}

const LIVE_REPO = {
  private: false,
  default_branch: "main",
};
const LIVE_COMMIT = {
  sha: "abc123commit",
  commit: { tree: { sha: "treesha" } },
};
const LIVE_TREE = {
  sha: "treesha",
  truncated: false,
  tree: [
    { path: "README.md", type: "blob", sha: "blob-readme", size: 14 },
    { path: "src/main.rs", type: "blob", sha: "blob-main", size: 20 },
  ],
};
const LIVE_BLOB = {
  sha: "blob-readme",
  size: 14,
  encoding: "base64",
  content: "SGVsbG8sIG1lbW9yeSE=",
};

function mockGithubFetch(routes) {
  const calls = [];
  const fetchImpl = async (url, opts) => {
    calls.push({ url, accept: opts?.headers?.Accept, credentials: opts?.credentials });
    const path = String(url).startsWith(GITHUB_API)
      ? String(url).slice(GITHUB_API.length)
      : String(url);
    if (!routes[path]) {
      throw new Error(`unexpected fetch ${url}`);
    }
    return routes[path];
  };
  return { calls, fetchImpl };
}

test("prefetchLiveGithub fills repo → commit → tree for a live tree", async () => {
  const { calls, fetchImpl } = mockGithubFetch({
    "/repos/acme/demo": jsonResponse(200, LIVE_REPO),
    "/repos/acme/demo/commits/main": jsonResponse(200, LIVE_COMMIT),
    "/repos/acme/demo/git/trees/treesha?recursive=1": jsonResponse(200, LIVE_TREE),
  });
  const fixtures = new Map();
  await prefetchLiveGithub(
    fixtures,
    { kind: "run", command: "tree", repo: "acme/demo", path: null },
    fetchImpl,
  );
  assert.equal(calls.length, 3);
  assert.equal(calls[0].url, `${GITHUB_API}/repos/acme/demo`);
  assert.equal(calls[0].accept, "application/vnd.github+json");
  assert.equal(calls[0].credentials, "omit");
  assert.equal(fixtures.get("/repos/acme/demo")?.status, 200);
  assert.equal(fixtures.get(`${GITHUB_API}/repos/acme/demo`)?.status, 200);
  assert.ok(fixtures.has("/repos/acme/demo/commits/main"));
  assert.ok(fixtures.has("/repos/acme/demo/git/trees/treesha?recursive=1"));
  assert.equal(
    fixtures.has("/repos/acme/demo/git/blobs/blob-readme"),
    false,
    "tree/ls must not fetch blobs",
  );
});

test("prefetchLiveGithub also fetches the blob for cat/head/tail/sed", async () => {
  const { calls, fetchImpl } = mockGithubFetch({
    "/repos/acme/demo": jsonResponse(200, LIVE_REPO),
    "/repos/acme/demo/commits/main": jsonResponse(200, LIVE_COMMIT),
    "/repos/acme/demo/git/trees/treesha?recursive=1": jsonResponse(200, LIVE_TREE),
    "/repos/acme/demo/git/blobs/blob-readme": jsonResponse(200, LIVE_BLOB),
  });
  const fixtures = new Map();
  await prefetchLiveGithub(
    fixtures,
    { kind: "run", command: "cat", repo: "acme/demo", path: "README.md" },
    fetchImpl,
  );
  assert.equal(calls.length, 4);
  assert.equal(calls[3].url, `${GITHUB_API}/repos/acme/demo/git/blobs/blob-readme`);
  assert.equal(fixtures.get("/repos/acme/demo/git/blobs/blob-readme")?.status, 200);

  const headFixtures = new Map();
  const { calls: headCalls, fetchImpl: headFetch } = mockGithubFetch({
    "/repos/acme/demo": jsonResponse(200, LIVE_REPO),
    "/repos/acme/demo/commits/main": jsonResponse(200, LIVE_COMMIT),
    "/repos/acme/demo/git/trees/treesha?recursive=1": jsonResponse(200, LIVE_TREE),
    "/repos/acme/demo/git/blobs/blob-readme": jsonResponse(200, LIVE_BLOB),
  });
  await prefetchLiveGithub(
    headFixtures,
    { kind: "run", command: "head", repo: "acme/demo", path: "README.md" },
    headFetch,
  );
  assert.equal(headCalls.length, 4);
  assert.ok(headFixtures.has("/repos/acme/demo/git/blobs/blob-readme"));
});

test("prefetchLiveGithub fetches rg blobs and errors over the file cap", async () => {
  const { calls, fetchImpl } = mockGithubFetch({
    "/repos/acme/demo": jsonResponse(200, LIVE_REPO),
    "/repos/acme/demo/commits/main": jsonResponse(200, LIVE_COMMIT),
    "/repos/acme/demo/git/trees/treesha?recursive=1": jsonResponse(200, LIVE_TREE),
    "/repos/acme/demo/git/blobs/blob-readme": jsonResponse(200, LIVE_BLOB),
    "/repos/acme/demo/git/blobs/blob-main": jsonResponse(200, {
      sha: "blob-main",
      size: 20,
      encoding: "base64",
      content: "Zm4gbWFpbigpIHt9Cg==",
    }),
  });
  const fixtures = new Map();
  await prefetchLiveGithub(
    fixtures,
    { kind: "run", command: "rg", repo: "acme/demo", path: null, pattern: "Hello" },
    fetchImpl,
  );
  assert.equal(calls.length, 5);
  assert.ok(fixtures.has("/repos/acme/demo/git/blobs/blob-readme"));
  assert.ok(fixtures.has("/repos/acme/demo/git/blobs/blob-main"));

  const hugeTree = {
    sha: "treesha",
    truncated: false,
    tree: Array.from({ length: MAX_RG_PREFETCH_FILES + 1 }, (_, i) => ({
      path: `f${i}.txt`,
      type: "blob",
      sha: `blob-${i}`,
      size: 1,
    })),
  };
  const { fetchImpl: hugeFetch } = mockGithubFetch({
    "/repos/huge/repo": jsonResponse(200, LIVE_REPO),
    "/repos/huge/repo/commits/main": jsonResponse(200, LIVE_COMMIT),
    "/repos/huge/repo/git/trees/treesha?recursive=1": jsonResponse(200, hugeTree),
  });
  await assert.rejects(
    () =>
      prefetchLiveGithub(
        new Map(),
        { kind: "run", command: "rg", repo: "huge/repo", path: null, pattern: "x" },
        hugeFetch,
      ),
    /host error: repo has too many files for rg/,
  );
});

test("prefetch reuses the fixture Map and does not refetch", async () => {
  const { calls, fetchImpl } = mockGithubFetch({
    "/repos/acme/demo": jsonResponse(200, LIVE_REPO),
    "/repos/acme/demo/commits/main": jsonResponse(200, LIVE_COMMIT),
    "/repos/acme/demo/git/trees/treesha?recursive=1": jsonResponse(200, LIVE_TREE),
    "/repos/acme/demo/git/blobs/blob-readme": jsonResponse(200, LIVE_BLOB),
  });
  const fixtures = new Map();
  const parsed = { kind: "run", command: "cat", repo: "acme/demo", path: "README.md" };
  await prefetchLiveGithub(fixtures, parsed, fetchImpl);
  assert.equal(calls.length, 4);
  await prefetchLiveGithub(fixtures, parsed, fetchImpl);
  assert.equal(calls.length, 4);
});

test("CORS / network failures become host errors", async () => {
  const fetchImpl = async () => {
    throw new TypeError("Failed to fetch");
  };
  await assert.rejects(
    () =>
      prefetchLiveGithub(
        new Map(),
        { kind: "run", command: "tree", repo: "openai/codex", path: null },
        fetchImpl,
      ),
    (err) => {
      assert.match(String(err.message), /host error: CORS or network failure/);
      assert.match(String(err.message), /openai\/codex/);
      return true;
    },
  );
  await assert.rejects(
    () => githubGetJson("/repos/openai/codex", fetchImpl),
    /host error: CORS or network failure/,
  );
});

test("HTTP 404 is stored for wasm and does not throw", async () => {
  const { calls, fetchImpl } = mockGithubFetch({
    "/repos/missing/repo": jsonResponse(404, { message: "Not Found" }),
  });
  const fixtures = new Map();
  await prefetchLiveGithub(
    fixtures,
    { kind: "run", command: "tree", repo: "missing/repo", path: null },
    fetchImpl,
  );
  assert.equal(calls.length, 1);
  assert.equal(fixtures.get("/repos/missing/repo")?.status, 404);
});

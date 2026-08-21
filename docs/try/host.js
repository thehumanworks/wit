/**
 * Browser / Node host for the existing wit_snapshot.wasm module.
 *
 * Supplies synchronous `wit_snapshot_host.http_get` the same way
 * crates/wit-snapshot/demo/browser/demo.js does. The import only reads
 * the fixture Map — never XMLHttpRequest. `demo/repo` is cassette-only
 * (no network). Live owner/repo is best-effort: async `fetch` prefetches
 * repo → commit → recursive tree (and a blob for cat/head/tail/sed, or
 * capped blobs for rg) into that map
 * before wasm `open` / `list` / `read`. CORS is the host's problem —
 * not a new backend.
 *
 * Does not import the 641-line IndexedDB repo cache.
 */

export const FIXTURE_REPO = "demo/repo";
export const GITHUB_API = "https://api.github.com";

export const ERR_NAMES = {
  0: "ok",
  1: "rate_limit",
  2: "oversized",
  3: "not_found",
  4: "binary",
  5: "private_repo",
  6: "oom",
  7: "api",
  8: "other",
};

// Assemble replaces this placeholder with the tag of the copied wasm.
export const RELEASE_TAG = "__WIT_RELEASE_TAG__";

const STAMPED_RELEASE_TAG = /^v\d+\.\d+\.\d+$/;

export function isStampedReleaseTag(tag = RELEASE_TAG) {
  return STAMPED_RELEASE_TAG.test(tag);
}

/** Release download URL only when assemble stamped a real vX.Y.Z tag. */
export function releaseWasmUrl(tag = RELEASE_TAG) {
  if (!isStampedReleaseTag(tag)) {
    return "";
  }
  return `https://github.com/thehumanworks/wit/releases/download/${tag}/wit_snapshot.wasm`;
}

export const RELEASE_WASM_URL = releaseWasmUrl();

export const FIXTURE_FILES = [
  "demo_repo.json",
  "demo_commit.json",
  "demo_tree.json",
  "demo_blob.json",
  "demo_blob_main.json",
];

/** Same default as wit-snapshot MemoryBackendLimits::max_blob_bytes. */
export const DEFAULT_MAX_BLOB_BYTES = 1_048_576;
/** Cap live rg prefetch so the page does not freeze. */
export const MAX_RG_PREFETCH_FILES = 200;
export const MAX_RG_PREFETCH_BYTES = 8 * 1024 * 1024;

const BLOB_COMMANDS = new Set(["cat", "head", "tail", "sed"]);

/**
 * @param {Record<string, string>} texts cassette JSON by filename
 * @returns {Map<string, { status: number, body: string }>}
 */
export function buildFixtureMap(texts) {
  const map = new Map();
  putGithubJson(map, "/repos/demo/repo", 200, texts["demo_repo.json"]);
  putGithubJson(map, "/repos/demo/repo/commits/main", 200, texts["demo_commit.json"]);
  putGithubJson(
    map,
    "/repos/demo/repo/git/trees/treesha?recursive=1",
    200,
    texts["demo_tree.json"],
  );
  putGithubJson(map, "/repos/demo/repo/git/blobs/blob-readme", 200, texts["demo_blob.json"]);
  putGithubJson(map, "/repos/demo/repo/git/blobs/blob-main", 200, texts["demo_blob_main.json"]);
  return map;
}

/**
 * Store a GitHub JSON response under the relative path wasm requests and
 * the absolute api.github.com URL (same keys as the cassette map).
 * @param {Map<string, { status: number, body: string }>} map
 * @param {string} path
 * @param {number} status
 * @param {string} body
 */
export function putGithubJson(map, path, status, body) {
  const entry = { status, body };
  map.set(path, entry);
  if (path.startsWith("/")) {
    map.set(`${GITHUB_API}${path}`, entry);
  }
}

export function isFixtureRepo(repo) {
  return repo === FIXTURE_REPO;
}

function githubUrl(path) {
  if (path.startsWith("http://") || path.startsWith("https://")) {
    return path;
  }
  return `${GITHUB_API}${path.startsWith("/") ? path : `/${path}`}`;
}

/**
 * Async GET for prefetch only. Throws a host/CORS error on network failure.
 * @param {string} path
 * @param {typeof fetch} [fetchImpl]
 * @returns {Promise<{ status: number, body: string }>}
 */
export async function githubGetJson(path, fetchImpl = globalThis.fetch) {
  if (typeof fetchImpl !== "function") {
    throw new Error("host error: fetch is not available");
  }
  const url = githubUrl(path);
  let response;
  try {
    response = await fetchImpl(url, {
      method: "GET",
      headers: { Accept: "application/vnd.github+json" },
      credentials: "omit",
    });
  } catch (err) {
    const detail = err && err.message ? err.message : String(err);
    throw new Error(`host error: CORS or network failure fetching ${url}: ${detail}`);
  }
  const body = await response.text();
  return { status: response.status, body };
}

async function ensureGithubJson(fixtures, path, fetchImpl) {
  const hit = fixtures.get(path);
  if (hit) {
    return hit;
  }
  const result = await githubGetJson(path, fetchImpl);
  putGithubJson(fixtures, path, result.status, result.body);
  return result;
}

/**
 * Prefetch live GitHub JSON into the fixture Map before wasm runs.
 * `demo/repo` is a no-op (cassette only). Tree/ls need repo → commit →
 * recursive tree; cat/head/tail/sed also need the blob for `path`.
 * Live rg prefetches tree plus blobs, capped by file count / byte budget.
 * @param {Map<string, { status: number, body: string }>} fixtures
 * @param {{ kind?: string, command?: string, repo?: string, path?: string | null }} parsed
 * @param {typeof fetch} [fetchImpl]
 */
export async function prefetchLiveGithub(fixtures, parsed, fetchImpl = globalThis.fetch) {
  if (!parsed || parsed.kind !== "run" || !parsed.repo || isFixtureRepo(parsed.repo)) {
    return;
  }
  const ownerRepo = parsed.repo;
  const repoPath = `/repos/${ownerRepo}`;
  const repoRes = await ensureGithubJson(fixtures, repoPath, fetchImpl);
  if (repoRes.status !== 200) {
    return;
  }
  let repoMeta;
  try {
    repoMeta = JSON.parse(repoRes.body);
  } catch {
    throw new Error("host error: GitHub repo response was not JSON");
  }
  const ref = String(repoMeta.default_branch || "main");
  const commitPath = `/repos/${ownerRepo}/commits/${ref}`;
  const commitRes = await ensureGithubJson(fixtures, commitPath, fetchImpl);
  if (commitRes.status !== 200) {
    return;
  }
  let commit;
  try {
    commit = JSON.parse(commitRes.body);
  } catch {
    throw new Error("host error: GitHub commit response was not JSON");
  }
  const treeSha = commit?.commit?.tree?.sha;
  if (!treeSha) {
    throw new Error("host error: commit response missing tree sha");
  }
  const treePath = `/repos/${ownerRepo}/git/trees/${treeSha}?recursive=1`;
  const treeRes = await ensureGithubJson(fixtures, treePath, fetchImpl);
  if (treeRes.status !== 200) {
    return;
  }
  if (parsed.command === "rg") {
    await prefetchRgBlobs(fixtures, ownerRepo, treeRes.body, parsed.path, fetchImpl);
    return;
  }
  if (!BLOB_COMMANDS.has(parsed.command) || !parsed.path) {
    return;
  }
  let tree;
  try {
    tree = JSON.parse(treeRes.body);
  } catch {
    throw new Error("host error: GitHub tree response was not JSON");
  }
  const want = String(parsed.path).replace(/^\/+|\/+$/g, "");
  const row = (tree.tree || []).find((entry) => entry.type === "blob" && entry.path === want);
  if (!row?.sha) {
    return;
  }
  await ensureGithubJson(fixtures, `/repos/${ownerRepo}/git/blobs/${row.sha}`, fetchImpl);
}

/**
 * @param {Map<string, { status: number, body: string }>} fixtures
 * @param {string} ownerRepo
 * @param {string} treeBody
 * @param {string | null | undefined} path
 * @param {typeof fetch} fetchImpl
 */
async function prefetchRgBlobs(fixtures, ownerRepo, treeBody, path, fetchImpl) {
  let tree;
  try {
    tree = JSON.parse(treeBody);
  } catch {
    throw new Error("host error: GitHub tree response was not JSON");
  }
  const want = path ? String(path).replace(/^\/+|\/+$/g, "") : "";
  const blobs = (tree.tree || []).filter((entry) => {
    if (entry.type !== "blob" || !entry.path) {
      return false;
    }
    if (!want) {
      return true;
    }
    return entry.path === want || entry.path.startsWith(`${want}/`);
  });
  const totalBytes = blobs.reduce((sum, entry) => sum + (Number(entry.size) || 0), 0);
  if (blobs.length > MAX_RG_PREFETCH_FILES || totalBytes > MAX_RG_PREFETCH_BYTES) {
    throw new Error(
      `host error: repo has too many files for rg in the try-it (${blobs.length} files, ${totalBytes} bytes); use a path or the native CLI`,
    );
  }
  for (const entry of blobs) {
    if (!entry.sha) {
      continue;
    }
    if (Number(entry.size) > DEFAULT_MAX_BLOB_BYTES) {
      continue;
    }
    await ensureGithubJson(fixtures, `/repos/${ownerRepo}/git/blobs/${entry.sha}`, fetchImpl);
  }
}

export function readAscii(memory, ptr, len) {
  const bytes = new Uint8Array(memory.buffer, ptr, len);
  return new TextDecoder().decode(bytes);
}

export function writeAscii(memory, ptr, text) {
  const bytes = new TextEncoder().encode(text);
  new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
  return bytes.length;
}

/**
 * @param {() => WebAssembly.Exports} getExports
 * @param {{ fixtures: Map<string, { status: number, body: string }> }} opts
 */
export function makeImports(getExports, opts) {
  return {
    wit_snapshot_host: {
      wit_snapshot_host_http_get(pathPtr, pathLen, statusOut, bodyPtrOut, bodyLenOut) {
        const { memory, wit_snapshot_alloc } = getExports();
        const path = readAscii(memory, pathPtr, pathLen);
        const result = opts.fixtures.get(path) ?? null;
        if (!result) {
          return 3;
        }
        const bodyBytes = new TextEncoder().encode(result.body);
        const bodyPtr = wit_snapshot_alloc(bodyBytes.length || 1);
        if (bodyBytes.length) {
          new Uint8Array(memory.buffer, bodyPtr, bodyBytes.length).set(bodyBytes);
        }
        const view = new DataView(memory.buffer);
        view.setUint16(statusOut, result.status, true);
        view.setUint32(bodyPtrOut, bodyPtr, true);
        view.setUint32(bodyLenOut, bodyBytes.length, true);
        return 0;
      },
    },
  };
}

/**
 * @param {WebAssembly.Exports} api
 */
export function lastError(api) {
  const buflen = 512;
  const buf = api.wit_snapshot_alloc(buflen);
  const n = api.wit_snapshot_last_error(buf, buflen);
  const msg = readAscii(api.memory, buf, Math.min(n, buflen));
  api.wit_snapshot_dealloc(buf, buflen);
  return msg;
}

export function check(api, rc, label) {
  if (rc !== 0) {
    const name = ERR_NAMES[rc] || String(rc);
    throw new Error(`${label} failed: ${name} — ${lastError(api)}`);
  }
}

export function withGuestString(api, text, fn) {
  const bytes = new TextEncoder().encode(text);
  const ptr = api.wit_snapshot_alloc(bytes.length || 1);
  if (bytes.length) {
    writeAscii(api.memory, ptr, text);
  }
  try {
    return fn(ptr, bytes.length);
  } finally {
    api.wit_snapshot_dealloc(ptr, bytes.length || 1);
  }
}

export function readOutJson(api, outPtrSlot, outLenSlot) {
  const view = new DataView(api.memory.buffer);
  const ptr = view.getUint32(outPtrSlot, true);
  const len = view.getUint32(outLenSlot, true);
  const json = readAscii(api.memory, ptr, len);
  api.wit_snapshot_dealloc(ptr, len || 1);
  return json;
}

export function openRepo(api, ownerRepo) {
  withGuestString(api, ownerRepo, (ptr, len) => {
    check(api, api.wit_snapshot_open(ptr, len, 0, 0), "open");
  });
}

export function listPath(api, path) {
  const outPtrSlot = api.wit_snapshot_alloc(4);
  const outLenSlot = api.wit_snapshot_alloc(4);
  try {
    const rel = path || "";
    if (rel) {
      withGuestString(api, rel, (ptr, len) => {
        check(api, api.wit_snapshot_list(ptr, len, outPtrSlot, outLenSlot), "list");
      });
    } else {
      check(api, api.wit_snapshot_list(0, 0, outPtrSlot, outLenSlot), "list");
    }
    return JSON.parse(readOutJson(api, outPtrSlot, outLenSlot));
  } finally {
    api.wit_snapshot_dealloc(outPtrSlot, 4);
    api.wit_snapshot_dealloc(outLenSlot, 4);
  }
}

export function readFile(api, path) {
  const outPtrSlot = api.wit_snapshot_alloc(4);
  const outLenSlot = api.wit_snapshot_alloc(4);
  try {
    withGuestString(api, path, (ptr, len) => {
      check(api, api.wit_snapshot_read(ptr, len, outPtrSlot, outLenSlot), "read");
    });
    return JSON.parse(readOutJson(api, outPtrSlot, outLenSlot));
  } finally {
    api.wit_snapshot_dealloc(outPtrSlot, 4);
    api.wit_snapshot_dealloc(outLenSlot, 4);
  }
}

/**
 * Recursively list files (wasm has open/list/read, not a tree export).
 * @param {WebAssembly.Exports} api
 * @param {string | null} path
 */
export function listFilesRecursive(api, path) {
  const files = [];
  const walk = (rel) => {
    const entries = listPath(api, rel);
    for (const entry of entries) {
      if (entry.kind === "file") {
        files.push(entry);
      } else if (entry.kind === "dir") {
        walk(entry.path);
      }
    }
  };
  walk(path || "");
  return files;
}

/**
 * Published fetch order only: same-origin `try/wit_snapshot.wasm`, then
 * the stamped GitHub release asset when assemble wrote a real tag.
 * Pages does not own a cargo tree — never list `../../target/` debug
 * or release paths.
 */
export function wasmCandidates(fromMetaUrl = import.meta.url) {
  const urls = [new URL("./wit_snapshot.wasm", fromMetaUrl).href];
  const fallback = releaseWasmUrl();
  if (fallback) {
    urls.push(fallback);
  }
  return urls;
}

/**
 * @param {string[]} urls
 * @param {WebAssembly.Imports} imports
 */
export async function instantiateFirstWasm(urls, imports) {
  let lastErr = null;
  for (const url of urls) {
    try {
      const response = await fetch(url);
      if (!response.ok) {
        lastErr = new Error(`load ${url}: HTTP ${response.status}`);
        continue;
      }
      const bytes = await response.arrayBuffer();
      const result = await WebAssembly.instantiate(bytes, imports);
      return { instance: result.instance, url };
    } catch (err) {
      lastErr = err;
    }
  }
  const detail = lastErr instanceof Error ? lastErr.message : String(lastErr || "");
  const fallback = isStampedReleaseTag(RELEASE_TAG)
    ? `the ${RELEASE_TAG} release`
    : "a GitHub release";
  throw new Error(
    `could not load wit_snapshot.wasm from this page or ${fallback}${
      detail ? `: ${detail}` : ""
    }`,
  );
}

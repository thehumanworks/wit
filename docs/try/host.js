/**
 * Browser / Node host for the existing wit_snapshot.wasm module.
 *
 * Supplies synchronous `wit_snapshot_host.http_get` the same way
 * crates/wit-snapshot/demo/browser/demo.js does. Fixtures first so the
 * page always works with no disk. Live api.github.com is best-effort
 * (sync XHR); CORS is the host's problem — not a new backend.
 *
 * Does not import the 641-line IndexedDB repo cache. Fixture hits are
 * enough for demo/repo.
 */

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

export const RELEASE_WASM_URL =
  "https://github.com/thehumanworks/wit/releases/download/v0.1.33/wit_snapshot.wasm";

export const FIXTURE_FILES = [
  "demo_repo.json",
  "demo_commit.json",
  "demo_tree.json",
  "demo_blob.json",
  "demo_blob_main.json",
];

/**
 * @param {Record<string, string>} texts cassette JSON by filename
 * @returns {Map<string, { status: number, body: string }>}
 */
export function buildFixtureMap(texts) {
  const map = new Map();
  const put = (path, body) => {
    map.set(path, { status: 200, body });
    map.set(`https://api.github.com${path}`, { status: 200, body });
  };
  put("/repos/demo/repo", texts["demo_repo.json"]);
  put("/repos/demo/repo/commits/main", texts["demo_commit.json"]);
  put("/repos/demo/repo/git/trees/treesha?recursive=1", texts["demo_tree.json"]);
  put("/repos/demo/repo/git/blobs/blob-readme", texts["demo_blob.json"]);
  put("/repos/demo/repo/git/blobs/blob-main", texts["demo_blob_main.json"]);
  return map;
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
 * Sync GET used as a best-effort live fallback. Returns null on CORS / network.
 * @param {string} path
 * @returns {{ status: number, body: string } | null}
 */
export function liveGithubGetSync(path) {
  if (typeof XMLHttpRequest === "undefined") {
    return null;
  }
  const url =
    path.startsWith("http://") || path.startsWith("https://")
      ? path
      : `https://api.github.com${path.startsWith("/") ? path : `/${path}`}`;
  const xhr = new XMLHttpRequest();
  try {
    xhr.open("GET", url, false);
    xhr.setRequestHeader("Accept", "application/vnd.github+json");
    xhr.send(null);
  } catch {
    return null;
  }
  if (xhr.status === 0) {
    return null;
  }
  return { status: xhr.status, body: xhr.responseText ?? "" };
}

/**
 * @param {() => WebAssembly.Exports} getExports
 * @param {{
 *   fixtures: Map<string, { status: number, body: string }>,
 *   liveGet?: (path: string) => { status: number, body: string } | null,
 * }} opts
 */
export function makeImports(getExports, opts) {
  const liveGet = opts.liveGet;
  return {
    wit_snapshot_host: {
      wit_snapshot_host_http_get(pathPtr, pathLen, statusOut, bodyPtrOut, bodyLenOut) {
        const { memory, wit_snapshot_alloc } = getExports();
        const path = readAscii(memory, pathPtr, pathLen);
        const result = opts.fixtures.get(path) ?? liveGet?.(path) ?? null;
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
 * the v0.1.33 release asset. Pages does not own a cargo tree — never
 * list `../../target/` debug or release paths.
 */
export function wasmCandidates(fromMetaUrl = import.meta.url) {
  return [new URL("./wit_snapshot.wasm", fromMetaUrl).href, RELEASE_WASM_URL];
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
  throw new Error(
    `could not load wit_snapshot.wasm from this page or the v0.1.33 release${
      detail ? `: ${detail}` : ""
    }`,
  );
}

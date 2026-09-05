/**
 * Wasm host adapter: MemoryBackend via open/list/read exports.
 * Sync http_get is served from RepoSnapshotCache (filled by async prefetch).
 * Not a third SnapshotBackend — host sits in front of get_json.
 */

import { SafeError, safeConsole, scrubSecrets } from "./auth.js";

const ERR_NAMES = {
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

/**
 * @param {WebAssembly.Exports} api
 * @param {number} ptr
 * @param {number} len
 */
function readAscii(api, ptr, len) {
  const bytes = new Uint8Array(api.memory.buffer, ptr, len);
  return new TextDecoder().decode(bytes);
}

/**
 * @param {WebAssembly.Exports} api
 * @param {number} ptr
 * @param {string} text
 */
function writeAscii(api, ptr, text) {
  const bytes = new TextEncoder().encode(text);
  new Uint8Array(api.memory.buffer, ptr, bytes.length).set(bytes);
  return bytes.length;
}

/**
 * Build wit_snapshot_host imports backed by a RepoSnapshotCache.
 * @param {() => WebAssembly.Exports} getExports
 * @param {import('./repo-cache.js').RepoSnapshotCache} cache
 */
export function makeHostImports(getExports, cache) {
  return {
    wit_snapshot_host: {
      wit_snapshot_host_http_get(pathPtr, pathLen, statusOut, bodyPtrOut, bodyLenOut) {
        const api = getExports();
        const path = readAscii(api, pathPtr, pathLen);
        // Sync path: only cache / previously prefetched responses.
        const result = cache.getOrFetch(path, () => null);
        if (!result) {
          // Do not echo secrets (paths are API paths; scrub anyway).
          safeConsole.error("http_get miss", path);
          return 3;
        }
        const bodyBytes = new TextEncoder().encode(result.body);
        const bodyPtr = api.wit_snapshot_alloc(bodyBytes.length || 1);
        if (bodyBytes.length) {
          new Uint8Array(api.memory.buffer, bodyPtr, bodyBytes.length).set(bodyBytes);
        }
        const view = new DataView(api.memory.buffer);
        view.setUint16(statusOut, result.status, true);
        view.setUint32(bodyPtrOut, bodyPtr, true);
        view.setUint32(bodyLenOut, bodyBytes.length, true);
        return 0;
      },
    },
  };
}

/**
 * Instantiate wit_snapshot.wasm with host imports.
 * Accepts bytes/Response (browser) or a precompiled WebAssembly.Module
 * (Cloudflare Workers — dynamic codegen from ArrayBuffer is disallowed).
 *
 * @param {BufferSource | Response | Promise<Response> | WebAssembly.Module} source
 * @param {import('./repo-cache.js').RepoSnapshotCache} cache
 */
export async function loadWasm(source, cache) {
  /** @type {WebAssembly.Exports | null} */
  let exports = null;
  const imports = makeHostImports(() => exports, cache);

  let instance;
  if (source instanceof WebAssembly.Module) {
    instance = new WebAssembly.Instance(source, imports);
  } else if (source instanceof Response || (source && typeof source.then === "function")) {
    const result = await WebAssembly.instantiateStreaming(source, imports);
    instance = result.instance;
  } else {
    const result = await WebAssembly.instantiate(source, imports);
    instance = result.instance;
  }
  exports = instance.exports;
  if (!exports.memory || !exports.wit_snapshot_open) {
    throw new SafeError("wasm exports missing (memory / open / list / read)", {
      status: 500,
      code: "wasm_exports",
    });
  }
  return exports;
}

/**
 * @param {WebAssembly.Exports} api
 */
function lastError(api) {
  const buflen = 512;
  const buf = api.wit_snapshot_alloc(buflen);
  const n = api.wit_snapshot_last_error(buf, buflen);
  const msg = readAscii(api, buf, Math.min(n, buflen));
  api.wit_snapshot_dealloc(buf, buflen);
  return scrubSecrets(msg);
}

/**
 * @param {WebAssembly.Exports} api
 * @param {number} rc
 * @param {string} label
 */
function check(api, rc, label) {
  if (rc !== 0) {
    const name = ERR_NAMES[rc] || String(rc);
    throw new SafeError(`${label} failed: ${name} — ${lastError(api)}`, {
      status: statusForCode(rc),
      code: name,
    });
  }
}

function statusForCode(rc) {
  if (rc === 3) return 404;
  if (rc === 5) return 403;
  if (rc === 1) return 429;
  if (rc === 4) return 415;
  return 502;
}

/**
 * @param {WebAssembly.Exports} api
 * @param {string} text
 * @param {(ptr: number, len: number) => unknown} fn
 */
function withGuestString(api, text, fn) {
  const bytes = new TextEncoder().encode(text);
  const ptr = api.wit_snapshot_alloc(bytes.length || 1);
  if (bytes.length) writeAscii(api, ptr, text);
  try {
    return fn(ptr, bytes.length);
  } finally {
    api.wit_snapshot_dealloc(ptr, bytes.length || 1);
  }
}

/**
 * @param {WebAssembly.Exports} api
 * @param {number} outPtrSlot
 * @param {number} outLenSlot
 */
function readOutJson(api, outPtrSlot, outLenSlot) {
  const view = new DataView(api.memory.buffer);
  const ptr = view.getUint32(outPtrSlot, true);
  const len = view.getUint32(outLenSlot, true);
  const json = readAscii(api, ptr, len);
  api.wit_snapshot_dealloc(ptr, len || 1);
  return json;
}

/**
 * @param {WebAssembly.Exports} api
 * @param {string} ownerRepo
 * @param {string | null} branch
 */
export function wasmOpen(api, ownerRepo, branch) {
  withGuestString(api, ownerRepo, (repoPtr, repoLen) => {
    if (branch) {
      withGuestString(api, branch, (bPtr, bLen) => {
        check(api, api.wit_snapshot_open(repoPtr, repoLen, bPtr, bLen), "open");
      });
    } else {
      check(api, api.wit_snapshot_open(repoPtr, repoLen, 0, 0), "open");
    }
  });
}

/**
 * @param {WebAssembly.Exports} api
 * @param {string} path
 * @returns {Array<{name:string,kind:string,path:string,size_bytes?:number|null}>}
 */
export function wasmList(api, path) {
  const outPtrSlot = api.wit_snapshot_alloc(4);
  const outLenSlot = api.wit_snapshot_alloc(4);
  try {
    const p = path || "";
    if (p) {
      withGuestString(api, p, (ptr, len) => {
        check(api, api.wit_snapshot_list(ptr, len, outPtrSlot, outLenSlot), "list");
      });
    } else {
      check(api, api.wit_snapshot_list(0, 0, outPtrSlot, outLenSlot), "list");
    }
    const json = readOutJson(api, outPtrSlot, outLenSlot);
    return JSON.parse(json);
  } finally {
    api.wit_snapshot_dealloc(outPtrSlot, 4);
    api.wit_snapshot_dealloc(outLenSlot, 4);
  }
}

/**
 * @param {WebAssembly.Exports} api
 * @param {string} path
 * @returns {{ path: string, text: string, size_bytes: number }}
 */
export function wasmRead(api, path) {
  const outPtrSlot = api.wit_snapshot_alloc(4);
  const outLenSlot = api.wit_snapshot_alloc(4);
  try {
    withGuestString(api, path, (ptr, len) => {
      check(api, api.wit_snapshot_read(ptr, len, outPtrSlot, outLenSlot), "read");
    });
    const json = readOutJson(api, outPtrSlot, outLenSlot);
    return JSON.parse(json);
  } finally {
    api.wit_snapshot_dealloc(outPtrSlot, 4);
    api.wit_snapshot_dealloc(outLenSlot, 4);
  }
}

/**
 * Collect file entries under path by recursive list (open/list only — no new export).
 * @param {WebAssembly.Exports} api
 * @param {string} basePath
 * @param {number | null} depth  max relative depth (null = unlimited); depth 0 = files at base only
 */
export function collectTreeFiles(api, basePath, depth) {
  /** @type {Array<{path:string,kind:string,size_bytes?:number|null}>} */
  const files = [];

  /**
   * @param {string} dir
   * @param {number} level
   */
  function walk(dir, level) {
    if (depth != null && level > depth) return;
    const entries = wasmList(api, dir);
    for (const e of entries) {
      const kind = e.kind === "dir" ? "dir" : "file";
      const full = e.path || (dir ? `${dir}/${e.name}` : e.name);
      if (kind === "dir") {
        if (depth == null || level < depth) {
          walk(full, level + 1);
        }
      } else {
        // Files at this level: relative depth = level + 1 from base? 
        // CLI depth filter is on relative path segments; include file if segments <= depth
        const relative = basePath
          ? full.startsWith(basePath + "/")
            ? full.slice(basePath.length + 1)
            : full
          : full;
        const segs = relative.split("/").filter(Boolean).length;
        if (depth == null || segs <= depth) {
          files.push({
            path: full,
            kind: "file",
            size_bytes: e.size_bytes ?? null,
          });
        }
      }
    }
  }

  walk(basePath || "", 0);
  // Byte-wise path order, matching the Rust BTreeMap walk in
  // `MemorySnapshot::tree` (the CLI prints `src/lib.rs` before `src/util/mod.rs`).
  files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return files;
}

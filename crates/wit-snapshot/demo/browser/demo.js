/**
 * Browser host for wit-snapshot wasm32 exports.
 *
 * Supplies synchronous fixture-backed `wit_snapshot_host_http_get` with a
 * host-owned per-repo cache (slim tree + blob bytes, default 24h TTL).
 * Live `api.github.com` from a bare page will fail CORS — use a same-origin
 * proxy or fixtures (see docs/adr/0004-wasm-fetch-howto.md).
 */

import {
  DEFAULT_TTL_MS,
  RepoSnapshotCache,
  formatRemaining,
  hydrateCacheFromIdb,
  persistCacheToIdb,
  ttlFromSearchParams,
} from "./repo-cache.js";

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

const out = document.getElementById("out");
const cacheOut = document.getElementById("cache-out");
const btnLoad = document.getElementById("btn-load");
const btnOpen = document.getElementById("btn-open");
const btnOpenOther = document.getElementById("btn-open-other");
const btnList = document.getElementById("btn-list");
const btnRead = document.getElementById("btn-read");
const btnClearCache = document.getElementById("btn-clear-cache");
const ttlInput = document.getElementById("ttl-ms");

let api = null;
/** @type {Map<string, { status: number, body: string }> | null} */
let fixtures = null;
/** @type {RepoSnapshotCache} */
let repoCache = new RepoSnapshotCache({
  ttlMs: ttlFromSearchParams(window.location.search) ?? DEFAULT_TTL_MS,
});

ttlInput.value = String(repoCache.ttlMs);

function log(msg, isErr = false) {
  out.textContent = msg;
  out.classList.toggle("err", isErr);
}

function renderCacheStatus() {
  const rows = repoCache.statusRows();
  const last = repoCache.lastOutcome;
  const lines = [];
  lines.push(`TTL default: ${repoCache.ttlMs}ms (24h = ${DEFAULT_TTL_MS}ms)`);
  if (last) {
    lines.push(
      `Last http_get: ${last.outcome.toUpperCase()}` +
        (last.repoKey ? ` · ${last.repoKey}` : "") +
        (last.remainingMs != null ? ` · remaining ${formatRemaining(last.remainingMs)}` : "") +
        ` · ${last.path}`,
    );
  } else {
    lines.push("Last http_get: (none yet)");
  }
  if (rows.length === 0) {
    lines.push("Cached repos: (empty)");
  } else {
    lines.push("Cached repos:");
    for (const row of rows) {
      lines.push(
        `  ${row.key} · remaining ${formatRemaining(row.remainingMs)}` +
          ` · tree=${row.treeEntries} blobs=${row.blobCount}`,
      );
    }
  }
  cacheOut.textContent = lines.join("\n");
}

function schedulePersist() {
  persistCacheToIdb(repoCache).catch((err) => console.warn("cache persist failed", err));
  renderCacheStatus();
}

async function loadFixtures() {
  const names = [
    "demo_repo.json",
    "demo_commit.json",
    "demo_tree.json",
    "demo_blob.json",
    "demo_blob_main.json",
  ];
  const texts = {};
  for (const name of names) {
    const res = await fetch(`../../tests/cassettes/${name}`);
    if (!res.ok) throw new Error(`failed to load fixture ${name}: ${res.status}`);
    texts[name] = await res.text();
  }
  const map = new Map();
  const put = (path, body) => {
    map.set(path, { status: 200, body });
    map.set(`https://api.github.com${path}`, { status: 200, body });
  };
  // Primary demo repo
  put("/repos/demo/repo", texts["demo_repo.json"]);
  put("/repos/demo/repo/commits/main", texts["demo_commit.json"]);
  put("/repos/demo/repo/git/trees/treesha?recursive=1", texts["demo_tree.json"]);
  put("/repos/demo/repo/git/blobs/blob-readme", texts["demo_blob.json"]);
  put("/repos/demo/repo/git/blobs/blob-main", texts["demo_blob_main.json"]);
  // Second repo for independent TTL demo (same fixture bodies, different owner/repo)
  put("/repos/other/repo", texts["demo_repo.json"]);
  put("/repos/other/repo/commits/main", texts["demo_commit.json"]);
  put("/repos/other/repo/git/trees/treesha?recursive=1", texts["demo_tree.json"]);
  put("/repos/other/repo/git/blobs/blob-readme", texts["demo_blob.json"]);
  put("/repos/other/repo/git/blobs/blob-main", texts["demo_blob_main.json"]);
  return map;
}

function readAscii(mem, ptr, len) {
  const bytes = new Uint8Array(mem.buffer, ptr, len);
  return new TextDecoder().decode(bytes);
}

function writeAscii(mem, ptr, text) {
  const bytes = new TextEncoder().encode(text);
  new Uint8Array(mem.buffer, ptr, bytes.length).set(bytes);
  return bytes.length;
}

function makeImports(getExports) {
  return {
    wit_snapshot_host: {
      wit_snapshot_host_http_get(pathPtr, pathLen, statusOut, bodyPtrOut, bodyLenOut) {
        const { memory, wit_snapshot_alloc } = getExports();
        const path = readAscii(memory, pathPtr, pathLen);
        const result = repoCache.getOrFetch(path, (p) => fixtures.get(p) ?? null);
        renderCacheStatus();
        if (!result) {
          console.error("fixture miss", path);
          return 3;
        }
        if (result.outcome === "miss") {
          schedulePersist();
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

async function loadWasm() {
  fixtures = await loadFixtures();
  await hydrateCacheFromIdb(repoCache);
  renderCacheStatus();

  let exports = null;
  const imports = makeImports(() => exports);
  const candidates = [
    "./wit_snapshot.wasm",
    "../../../target/wasm32-unknown-unknown/debug/wit_snapshot.wasm",
    "../../../target/wasm32-unknown-unknown/release/wit_snapshot.wasm",
  ];
  let result = null;
  let lastErr = null;
  for (const url of candidates) {
    try {
      result = await WebAssembly.instantiateStreaming(fetch(url), imports);
      log(`Loaded ${url}`);
      break;
    } catch (err) {
      lastErr = err;
    }
  }
  if (!result) {
    throw lastErr || new Error("could not load wit_snapshot.wasm");
  }
  exports = result.instance.exports;
  if (!exports.memory || !exports.wit_snapshot_open) {
    throw new Error("wasm exports missing (memory / open / list / read)");
  }
  api = exports;
  btnOpen.disabled = false;
  btnOpenOther.disabled = false;
}

function lastError() {
  const buflen = 512;
  const buf = api.wit_snapshot_alloc(buflen);
  const n = api.wit_snapshot_last_error(buf, buflen);
  const msg = readAscii(api.memory, buf, Math.min(n, buflen));
  api.wit_snapshot_dealloc(buf, buflen);
  return msg;
}

function check(rc, label) {
  if (rc !== 0) {
    throw new Error(`${label} failed: ${ERR_NAMES[rc] || rc} — ${lastError()}`);
  }
}

function withGuestString(text, fn) {
  const bytes = new TextEncoder().encode(text);
  const ptr = api.wit_snapshot_alloc(bytes.length || 1);
  if (bytes.length) writeAscii(api.memory, ptr, text);
  try {
    return fn(ptr, bytes.length);
  } finally {
    api.wit_snapshot_dealloc(ptr, bytes.length || 1);
  }
}

function readOutJson(outPtrSlot, outLenSlot) {
  const view = new DataView(api.memory.buffer);
  const ptr = view.getUint32(outPtrSlot, true);
  const len = view.getUint32(outLenSlot, true);
  const json = readAscii(api.memory, ptr, len);
  api.wit_snapshot_dealloc(ptr, len || 1);
  return json;
}

function applyTtlFromInput() {
  const n = Number(ttlInput.value);
  if (Number.isFinite(n) && n >= 0) {
    repoCache.setTtlMs(n);
  }
}

function openRepo(ownerRepo) {
  applyTtlFromInput();
  withGuestString(ownerRepo, (ptr, len) => {
    check(api.wit_snapshot_open(ptr, len, 0, 0), "open");
  });
  const last = repoCache.lastOutcome;
  const tag = last ? last.outcome.toUpperCase() : "?";
  log(`open ${ownerRepo}: ok (${tag})`);
  btnList.disabled = false;
  btnRead.disabled = false;
  renderCacheStatus();
}

btnLoad.addEventListener("click", async () => {
  btnLoad.disabled = true;
  try {
    applyTtlFromInput();
    await loadWasm();
    btnList.disabled = true;
    btnRead.disabled = true;
    renderCacheStatus();
  } catch (err) {
    log(String(err), true);
    btnLoad.disabled = false;
  }
});

btnOpen.addEventListener("click", () => {
  try {
    openRepo("demo/repo");
  } catch (err) {
    log(String(err), true);
  }
});

btnOpenOther.addEventListener("click", () => {
  try {
    openRepo("other/repo");
  } catch (err) {
    log(String(err), true);
  }
});

btnList.addEventListener("click", () => {
  try {
    const outPtrSlot = api.wit_snapshot_alloc(4);
    const outLenSlot = api.wit_snapshot_alloc(4);
    try {
      check(api.wit_snapshot_list(0, 0, outPtrSlot, outLenSlot), "list");
      const json = readOutJson(outPtrSlot, outLenSlot);
      log(`list:\n${JSON.stringify(JSON.parse(json), null, 2)}`);
    } finally {
      api.wit_snapshot_dealloc(outPtrSlot, 4);
      api.wit_snapshot_dealloc(outLenSlot, 4);
    }
  } catch (err) {
    log(String(err), true);
  }
});

btnRead.addEventListener("click", () => {
  try {
    const outPtrSlot = api.wit_snapshot_alloc(4);
    const outLenSlot = api.wit_snapshot_alloc(4);
    try {
      withGuestString("README.md", (ptr, len) => {
        check(api.wit_snapshot_read(ptr, len, outPtrSlot, outLenSlot), "read");
      });
      const json = readOutJson(outPtrSlot, outLenSlot);
      const last = repoCache.lastOutcome;
      const tag = last ? last.outcome.toUpperCase() : "?";
      log(`read (${tag}):\n${JSON.stringify(JSON.parse(json), null, 2)}`);
      renderCacheStatus();
    } finally {
      api.wit_snapshot_dealloc(outPtrSlot, 4);
      api.wit_snapshot_dealloc(outLenSlot, 4);
    }
  } catch (err) {
    log(String(err), true);
  }
});

btnClearCache.addEventListener("click", async () => {
  repoCache = new RepoSnapshotCache({ ttlMs: Number(ttlInput.value) || DEFAULT_TTL_MS });
  try {
    await persistCacheToIdb(repoCache);
  } catch (err) {
    console.warn(err);
  }
  renderCacheStatus();
  log("host cache cleared");
});

ttlInput.addEventListener("change", () => {
  applyTtlFromInput();
  renderCacheStatus();
});

renderCacheStatus();

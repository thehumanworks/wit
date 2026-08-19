/**
 * Browser host for wit-snapshot wasm32 exports.
 *
 * Supplies synchronous fixture-backed `wit_snapshot_host_http_get`.
 * Live `api.github.com` from a bare page will fail CORS — use a same-origin
 * proxy or fixtures (see docs/adr/0004-wasm-fetch-howto.md).
 */

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
const btnLoad = document.getElementById("btn-load");
const btnOpen = document.getElementById("btn-open");
const btnList = document.getElementById("btn-list");
const btnRead = document.getElementById("btn-read");

let api = null;

function log(msg, isErr = false) {
  out.textContent = msg;
  out.classList.toggle("err", isErr);
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
  put("/repos/demo/repo", texts["demo_repo.json"]);
  put("/repos/demo/repo/commits/main", texts["demo_commit.json"]);
  put("/repos/demo/repo/git/trees/treesha?recursive=1", texts["demo_tree.json"]);
  put("/repos/demo/repo/git/blobs/blob-readme", texts["demo_blob.json"]);
  put("/repos/demo/repo/git/blobs/blob-main", texts["demo_blob_main.json"]);
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

function makeImports(fixtures, getExports) {
  return {
    wit_snapshot_host: {
      wit_snapshot_host_http_get(pathPtr, pathLen, statusOut, bodyPtrOut, bodyLenOut) {
        const { memory, wit_snapshot_alloc } = getExports();
        const path = readAscii(memory, pathPtr, pathLen);
        const hit = fixtures.get(path);
        if (!hit) {
          console.error("fixture miss", path);
          return 3;
        }
        const bodyBytes = new TextEncoder().encode(hit.body);
        const bodyPtr = wit_snapshot_alloc(bodyBytes.length || 1);
        if (bodyBytes.length) {
          new Uint8Array(memory.buffer, bodyPtr, bodyBytes.length).set(bodyBytes);
        }
        const view = new DataView(memory.buffer);
        view.setUint16(statusOut, hit.status, true);
        view.setUint32(bodyPtrOut, bodyPtr, true);
        view.setUint32(bodyLenOut, bodyBytes.length, true);
        return 0;
      },
    },
  };
}

async function loadWasm() {
  const fixtures = await loadFixtures();
  let exports = null;
  const imports = makeImports(fixtures, () => exports);
  // Prefer a copied artifact next to this page; fall back to cargo target path
  // when the check script stages it.
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

btnLoad.addEventListener("click", async () => {
  btnLoad.disabled = true;
  try {
    await loadWasm();
    btnList.disabled = true;
    btnRead.disabled = true;
  } catch (err) {
    log(String(err), true);
    btnLoad.disabled = false;
  }
});

btnOpen.addEventListener("click", () => {
  try {
    withGuestString("demo/repo", (ptr, len) => {
      check(api.wit_snapshot_open(ptr, len, 0, 0), "open");
    });
    log("open demo/repo: ok");
    btnList.disabled = false;
    btnRead.disabled = false;
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
      log(`read:\n${JSON.stringify(JSON.parse(json), null, 2)}`);
    } finally {
      api.wit_snapshot_dealloc(outPtrSlot, 4);
      api.wit_snapshot_dealloc(outLenSlot, 4);
    }
  } catch (err) {
    log(String(err), true);
  }
});

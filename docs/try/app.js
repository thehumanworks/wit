import { USAGE } from "./commands.js";
import {
  FIXTURE_FILES,
  RELEASE_WASM_URL,
  buildFixtureMap,
  instantiateFirstWasm,
  liveGithubGetSync,
  makeImports,
  wasmCandidates,
} from "./host.js";
import { runLine } from "./run.js";

const out = document.getElementById("term-out");
const input = document.getElementById("term-in");
const statusEl = document.getElementById("wasm-status");
const form = document.getElementById("term-form");

/** @type {WebAssembly.Exports | null} */
let api = null;
const history = [];
let historyIdx = -1;

function appendLine(text, className) {
  const row = document.createElement("div");
  if (className) {
    row.className = className;
  }
  row.textContent = text;
  out.appendChild(row);
  out.scrollTop = out.scrollHeight;
}

function appendBlock(text, className) {
  if (text == null || text === "") {
    return;
  }
  const row = document.createElement("div");
  if (className) {
    row.className = className;
  }
  row.textContent = text;
  out.appendChild(row);
  out.scrollTop = out.scrollHeight;
}

function setStatus(text, isErr = false) {
  statusEl.textContent = text;
  statusEl.classList.toggle("err", isErr);
}

async function loadFixtures() {
  const texts = {};
  for (const name of FIXTURE_FILES) {
    const response = await fetch(new URL(`./fixtures/${name}`, import.meta.url));
    if (!response.ok) {
      throw new Error(`failed to load fixture ${name}: ${response.status}`);
    }
    texts[name] = await response.text();
  }
  return buildFixtureMap(texts);
}

async function boot() {
  setStatus("Loading wit_snapshot.wasm…");
  const fixtures = await loadFixtures();
  let exports = null;
  const imports = makeImports(() => exports, {
    fixtures,
    liveGet: liveGithubGetSync,
  });
  const { instance, url } = await instantiateFirstWasm(
    wasmCandidates(import.meta.url),
    imports,
  );
  exports = instance.exports;
  if (!exports.memory || !exports.wit_snapshot_open) {
    throw new Error("wasm exports missing (memory / open / list / read)");
  }
  api = exports;
  const source =
    url === RELEASE_WASM_URL
      ? "v0.1.33 release asset"
      : url.endsWith("wit_snapshot.wasm")
        ? url
        : url;
  setStatus(`Ready · ${source}`);
  appendBlock("wit try-it — fixture repo demo/repo always works (no disk).");
  appendBlock("Live api.github.com is best-effort; CORS errors print here.");
  appendBlock(USAGE, "muted");
  input.disabled = false;
  input.focus();
  runAndRender("wit tree demo/repo");
}

function runAndRender(line) {
  appendLine(`$ ${line}`, "cmd");
  if (!api) {
    appendBlock("wasm not loaded", "err");
    return;
  }
  const result = runLine(api, line);
  if (result.kind === "clear") {
    out.replaceChildren();
    return;
  }
  if (result.kind === "empty") {
    return;
  }
  appendBlock(result.text, result.error ? "err" : "");
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  const line = input.value;
  if (line.trim()) {
    history.push(line);
    historyIdx = history.length;
  }
  input.value = "";
  runAndRender(line);
});

input.addEventListener("keydown", (event) => {
  if (event.key === "ArrowUp") {
    if (!history.length) {
      return;
    }
    event.preventDefault();
    historyIdx = Math.max(0, historyIdx - 1);
    input.value = history[historyIdx] ?? "";
    input.setSelectionRange(input.value.length, input.value.length);
  } else if (event.key === "ArrowDown") {
    event.preventDefault();
    historyIdx = Math.min(history.length, historyIdx + 1);
    input.value = history[historyIdx] ?? "";
  }
});

document.querySelectorAll("[data-fill]").forEach((el) => {
  el.addEventListener("click", (event) => {
    event.preventDefault();
    input.value = el.getAttribute("data-fill") ?? "";
    input.focus();
  });
});

boot().catch((err) => {
  setStatus(String(err), true);
  appendBlock(String(err), "err");
  appendBlock(
    `Need the wasm module. Place wit_snapshot.wasm next to this script, or fetch ${RELEASE_WASM_URL}`,
    "err",
  );
});

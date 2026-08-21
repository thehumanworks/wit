import { USAGE, parseCommand } from "./commands.js";
import {
  FIXTURE_FILES,
  RELEASE_TAG,
  RELEASE_WASM_URL,
  buildFixtureMap,
  instantiateFirstWasm,
  makeImports,
  prefetchLiveGithub,
  wasmCandidates,
} from "./host.js";
import { runLine } from "./run.js";

const out = document.getElementById("term-out");
const input = document.getElementById("term-in");
const statusEl = document.getElementById("wasm-status");
const form = document.getElementById("term-form");

/** @type {WebAssembly.Exports | null} */
let api = null;
/** @type {Map<string, { status: number, body: string }> | null} */
let fixtures = null;
const history = [];
let historyIdx = -1;
let busy = false;

function appendLine(text, className) {
  const row = document.createElement("div");
  if (className) {
    row.className = className;
  }
  row.textContent = text;
  out.appendChild(row);
  out.scrollTop = out.scrollHeight;
  return row;
}

function appendBlock(text, className) {
  if (text == null || text === "") {
    return;
  }
  return appendLine(text, className);
}

function setStatus(text, isErr = false) {
  statusEl.textContent = text;
  statusEl.classList.toggle("err", isErr);
}

function setBusy(next) {
  busy = next;
  input.disabled = next;
  input.setAttribute("aria-busy", next ? "true" : "false");
}

/**
 * Yield so `$ <cmd>` and `processing…` paint before any host fetch / wasm.
 * Double rAF waits until after the next frame; setTimeout(0) is the Node /
 * no-rAF fallback.
 */
function yieldToPaint() {
  if (typeof requestAnimationFrame === "function") {
    return new Promise((resolve) => {
      requestAnimationFrame(() => {
        requestAnimationFrame(resolve);
      });
    });
  }
  return new Promise((resolve) => setTimeout(resolve, 0));
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
  fixtures = await loadFixtures();
  let exports = null;
  const imports = makeImports(() => exports, { fixtures });
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
    RELEASE_WASM_URL && url === RELEASE_WASM_URL
      ? `${RELEASE_TAG} release asset`
      : url;
  setStatus(`Ready · ${source}`);
  appendBlock("wit try-it — fixture repo demo/repo always works (no disk).");
  appendBlock("Live api.github.com is best-effort; CORS errors print here.");
  appendBlock(USAGE, "muted");
  await runAndRender("wit tree demo/repo");
  if (!busy) {
    input.disabled = false;
    input.focus();
  }
}

async function runAndRender(line) {
  if (busy) {
    return;
  }
  appendLine(`$ ${line}`, "cmd");
  const processing = appendLine("processing…", "muted");
  setBusy(true);
  try {
    await yieldToPaint();
    if (!api || !fixtures) {
      processing.remove();
      appendBlock("wasm not loaded", "err");
      return;
    }
    const parsed = parseCommand(line);
    if (parsed.kind === "run") {
      await prefetchLiveGithub(fixtures, parsed);
    }
    const result = runLine(api, line);
    processing.remove();
    if (result.kind === "clear") {
      out.replaceChildren();
      return;
    }
    if (result.kind === "empty") {
      return;
    }
    appendBlock(result.text, result.error ? "err" : "");
  } catch (err) {
    processing.remove();
    appendBlock(String(err.message || err), "err");
  } finally {
    setBusy(false);
    input.focus();
  }
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  if (busy || input.disabled) {
    return;
  }
  const line = input.value;
  if (line.trim()) {
    history.push(line);
    historyIdx = history.length;
  }
  input.value = "";
  void runAndRender(line);
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
    RELEASE_WASM_URL
      ? `Could not load wit_snapshot.wasm. Tried this page's copy (try/wit_snapshot.wasm), then ${RELEASE_WASM_URL}.`
      : "Could not load wit_snapshot.wasm. Tried this page's copy (try/wit_snapshot.wasm).",
    "err",
  );
});

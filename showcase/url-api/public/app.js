/**
 * In-browser path routing for /tree|/ls|/cat — same handler as the worker.
 * Renders plaintext in the page (and history URL when possible).
 */

import { createHostCache, handleRequest } from "./lib/handle.js";

const out = document.getElementById("out");
const ownerEl = document.getElementById("owner");
const repoEl = document.getElementById("repo");
const pathEl = document.getElementById("path");
const branchEl = document.getElementById("branch");
const tokenEl = document.getElementById("token");

const cache = createHostCache();

function show(text, isErr = false) {
  out.textContent = text;
  out.classList.toggle("err", isErr);
}

/**
 * Build API URL for the three verbs (path stays in query).
 * @param {'tree'|'ls'|'cat'} verb
 */
function buildApiUrl(verb) {
  const owner = ownerEl.value.trim();
  const repo = repoEl.value.trim();
  const url = new URL(`/${verb}/${owner}/${repo}`, window.location.origin);
  const path = pathEl.value.trim();
  if (path) url.searchParams.set("path", path);
  const branch = branchEl.value.trim();
  if (branch) url.searchParams.set("branch", branch);
  return url;
}

async function run(verb) {
  const url = buildApiUrl(verb);
  /** @type {Record<string, string>} */
  const headers = {};
  const token = tokenEl.value.trim();
  if (token) headers.Authorization = `Bearer ${token}`;

  // Update address bar without putting token in the query.
  history.replaceState(null, "", url.pathname + url.search);

  show("Loading…");
  try {
    const request = new Request(url, { method: "GET", headers });
    const response = await handleRequest(request, {
      cache,
      loadWasmBytes: () => fetch(new URL("./wit_snapshot.wasm", window.location.href)),
    });
    if (!response) {
      show("not an API route", true);
      return;
    }
    const text = await response.text();
    show(text, !response.ok);
  } catch (err) {
    show(String(err?.message || err), true);
  }
}

for (const btn of document.querySelectorAll("button[data-verb]")) {
  btn.addEventListener("click", () => run(btn.getAttribute("data-verb")));
}

// If the page was opened on an API path, run it in-browser.
(async () => {
  const path = window.location.pathname;
  if (!/^\/(tree|ls|cat)\//i.test(path)) return;
  const parts = path.replace(/\/+$/, "").split("/").filter(Boolean);
  if (parts.length >= 3) {
    ownerEl.value = parts[1];
    repoEl.value = parts[2];
  }
  const q = new URLSearchParams(window.location.search);
  if (q.has("path")) pathEl.value = q.get("path") || "";
  if (q.has("branch") || q.has("ref")) {
    branchEl.value = q.get("branch") || q.get("ref") || "";
  }
  await run(parts[0].toLowerCase());
})();

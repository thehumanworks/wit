/**
 * In-browser path routing for every URL API verb — same handler as the
 * worker, running the wasm in the page. Renders plaintext (and the history
 * URL when possible).
 */

import { createHostCache, handleRequest } from "./lib/handle.js";

const out = document.getElementById("out");
const ownerEl = document.getElementById("owner");
const repoEl = document.getElementById("repo");
const pathEl = document.getElementById("path");
const branchEl = document.getElementById("branch");
const extraEl = document.getElementById("extra");
const tokenEl = document.getElementById("token");

const cache = createHostCache();

/** Verbs that take `/{verb}/{owner}/{repo}`. */
const REPO_VERBS = new Set([
  "tree", "ls", "cat", "head", "tail", "rg", "stats", "outline", "refs", "commits",
]);

function show(text, isErr = false) {
  out.textContent = text;
  out.classList.toggle("err", isErr);
}

/**
 * Build the API URL for a verb (path and every other option stay in the query).
 * The "extra" field takes raw query pairs such as `q=fn main&glob=*.rs`.
 * @param {string} verb
 */
function buildApiUrl(verb) {
  const owner = ownerEl.value.trim();
  const repo = repoEl.value.trim();
  const url = verb === "search"
    ? new URL("/search", window.location.origin)
    : new URL(`/${verb}/${owner}/${repo}`, window.location.origin);
  const path = pathEl.value.trim();
  if (path && verb !== "search") url.searchParams.set("path", path);
  const branch = branchEl.value.trim();
  if (branch && verb !== "search") url.searchParams.set("ref", branch);
  const extra = extraEl.value.trim();
  if (extra) {
    for (const [key, value] of new URLSearchParams(extra)) url.searchParams.set(key, value);
  }
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
    const commit = response.headers.get("x-wit-commit");
    const provenance = commit
      ? `# ${response.headers.get("x-wit-repo")} @ ${commit.slice(0, 7)} (${response.headers.get("x-wit-ref")}, cache ${response.headers.get("x-wit-cache")})\n`
      : "";
    show(provenance + text, !response.ok);
  } catch (err) {
    show(String(err?.message || err), true);
  }
}

for (const btn of document.querySelectorAll("button[data-verb]")) {
  btn.addEventListener("click", () => run(btn.getAttribute("data-verb")));
}

// If the page was opened on an API path, run it in-browser.
(async () => {
  const path = window.location.pathname.replace(/^\/api(?=\/)/i, "");
  const parts = path.replace(/\/+$/, "").split("/").filter(Boolean);
  const verb = (parts[0] || "").toLowerCase();
  if (!REPO_VERBS.has(verb) && verb !== "search") return;
  if (parts.length >= 3) {
    ownerEl.value = parts[1];
    repoEl.value = parts[2];
  }
  const q = new URLSearchParams(window.location.search);
  if (q.has("path")) pathEl.value = q.get("path") || "";
  if (q.has("branch") || q.has("ref")) {
    branchEl.value = q.get("branch") || q.get("ref") || "";
  }
  const rest = new URLSearchParams();
  for (const [key, value] of q) {
    if (!["path", "branch", "ref", "token", "access_token"].includes(key)) rest.set(key, value);
  }
  extraEl.value = rest.toString().replace(/\+/g, "%20");
  await run(verb);
})();

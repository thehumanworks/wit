/**
 * Shared fixture harness: an in-memory GitHub (REST + raw.githubusercontent)
 * behind `globalThis.fetch`, built from a plain `{ path: text }` map so tests
 * can describe a repository in a few lines and assert on every outgoing call.
 */

import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const root = join(here, "..");

/** @type {Promise<Buffer> | null} */
let wasmPromise = null;
/** The checked-in wasm module bytes (built by `npm run build:wasm`). */
export function wasmBytes() {
  if (!wasmPromise) wasmPromise = readFile(join(root, "public/wit_snapshot.wasm"));
  return wasmPromise;
}

export const COMMIT_SHA = "0123456789abcdef0123456789abcdef01234567";
export const TREE_SHA = "treesha0000000000000000000000000000000000";

/**
 * @param {string} text
 */
function base64(text) {
  return Buffer.from(text, "utf8").toString("base64");
}

/**
 * @param {string} path
 */
export function blobShaFor(path) {
  return `blob-${path.replace(/[^A-Za-z0-9]+/g, "-")}`;
}

/**
 * Build a fixture GitHub for one repository.
 *
 * @param {{
 *   repo?: string,
 *   defaultBranch?: string,
 *   files: Record<string, string | Uint8Array>,
 *   isPrivate?: boolean,
 *   branches?: string[],
 *   tags?: string[],
 *   commits?: Array<{ sha: string, author: string, date: string, message: string }>,
 *   searchItems?: unknown[],
 *   raw?: boolean,
 * }} spec
 */
export function fixtureGitHub(spec) {
  const repo = spec.repo ?? "demo/repo";
  const defaultBranch = spec.defaultBranch ?? "main";
  const raw = spec.raw ?? true;
  /** @type {Array<{ url: string, auth: string | null }>} */
  const calls = [];
  /** @type {{ status: number, body: string, headers?: Record<string, string> } | null} */
  let forcedResponse = null;
  /** @type {((url: string) => boolean) | null} */
  let forcedMatch = null;

  const tree = [];
  const blobs = new Map();
  const dirs = new Set();
  for (const [path, content] of Object.entries(spec.files)) {
    const bytes = typeof content === "string" ? Buffer.from(content, "utf8") : Buffer.from(content);
    const sha = blobShaFor(path);
    tree.push({ path, mode: "100644", type: "blob", sha, size: bytes.length });
    blobs.set(sha, { bytes, path });
    const parts = path.split("/");
    for (let i = 1; i < parts.length; i += 1) dirs.add(parts.slice(0, i).join("/"));
  }
  for (const dir of dirs) tree.push({ path: dir, mode: "040000", type: "tree", sha: `tree-${dir}` });
  tree.sort((a, b) => a.path.localeCompare(b.path));

  /** @type {Map<string, () => { status: number, body: string }>} */
  const routes = new Map();
  const json = (value, status = 200) => ({ status, body: JSON.stringify(value) });
  routes.set(`/repos/${repo}`, () =>
    json({ private: Boolean(spec.isPrivate), default_branch: defaultBranch, full_name: repo }),
  );
  const commitBody = () => json({ sha: COMMIT_SHA, commit: { tree: { sha: TREE_SHA } } });
  routes.set(`/repos/${repo}/commits/${defaultBranch}`, commitBody);
  routes.set(`/repos/${repo}/commits/${COMMIT_SHA}`, commitBody);
  for (const b of spec.branches ?? []) routes.set(`/repos/${repo}/commits/${b}`, commitBody);
  for (const t of spec.tags ?? []) routes.set(`/repos/${repo}/commits/${t}`, commitBody);
  routes.set(`/repos/${repo}/git/trees/${TREE_SHA}?recursive=1`, () =>
    json({ sha: TREE_SHA, truncated: false, tree }),
  );
  for (const [sha, blob] of blobs) {
    routes.set(`/repos/${repo}/git/blobs/${sha}`, () =>
      json({ sha, size: blob.bytes.length, encoding: "base64", content: base64(blob.bytes.toString("latin1")) }),
    );
  }
  routes.set(`/repos/${repo}/branches?per_page=100`, () =>
    json([defaultBranch, ...(spec.branches ?? [])].map((name) => ({ name, commit: { sha: COMMIT_SHA } }))),
  );
  routes.set(`/repos/${repo}/tags?per_page=100`, () =>
    json((spec.tags ?? []).map((name) => ({ name, commit: { sha: COMMIT_SHA } }))),
  );

  const fetchImpl = async (input, init = {}) => {
    const url = String(input);
    const headers = init.headers || {};
    const auth = headers.Authorization || headers.authorization || null;
    const token = auth ? String(auth).replace(/^\s*(?:Bearer|token)\s+/i, "") : null;
    calls.push({ url, auth: token });

    if (forcedResponse && (!forcedMatch || forcedMatch(url))) {
      const forced = forcedResponse;
      return new Response(forced.body, {
        status: forced.status,
        headers: { "content-type": "application/json", ...(forced.headers ?? {}) },
      });
    }

    if (url.startsWith("https://raw.githubusercontent.com/")) {
      if (!raw) return new Response("disabled", { status: 404 });
      const rest = url.slice("https://raw.githubusercontent.com/".length);
      const m = rest.match(/^([^/]+\/[^/]+)\/([0-9a-f]{40})\/(.+)$/);
      if (!m || m[1] !== repo || m[2] !== COMMIT_SHA) return new Response("nope", { status: 404 });
      const path = decodeURIComponent(m[3]);
      const blob = blobs.get(blobShaFor(path));
      if (!blob) return new Response("nope", { status: 404 });
      return new Response(blob.bytes, { status: 200, headers: { "content-length": String(blob.bytes.length) } });
    }

    const apiPath = url.replace(/^https:\/\/api\.github\.com/, "");
    if (apiPath.startsWith("/search/repositories")) {
      const items = spec.searchItems ?? [];
      return new Response(JSON.stringify({ total_count: items.length, items }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }
    if (apiPath.startsWith(`/repos/${repo}/commits?`)) {
      const params = new URL(url).searchParams;
      const n = Number(params.get("per_page") || 10);
      const path = params.get("path");
      const all = (spec.commits ?? []).filter((c) => !path || !c.paths || c.paths.includes(path));
      const body = all.slice(0, n).map((c) => ({
        sha: c.sha,
        commit: { author: { name: c.author, date: c.date }, message: c.message },
      }));
      return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
    }
    const hit = routes.get(apiPath);
    if (!hit) {
      return new Response(JSON.stringify({ message: "Not Found" }), {
        status: 404,
        headers: { "content-type": "application/json" },
      });
    }
    const res = hit();
    return new Response(res.body, { status: res.status, headers: { "content-type": "application/json" } });
  };

  let original = null;
  return {
    repo,
    calls,
    tree,
    install() {
      original = globalThis.fetch;
      globalThis.fetch = fetchImpl;
    },
    restore() {
      if (original) globalThis.fetch = original;
    },
    reset() {
      calls.length = 0;
      forcedResponse = null;
      forcedMatch = null;
    },
    /**
     * Force every (or matching) fetch to return this response until reset().
     * @param {{ status: number, body: string, headers?: Record<string, string> }} response
     * @param {(url: string) => boolean} [match]
     */
    force(response, match) {
      forcedResponse = response;
      forcedMatch = match ?? null;
    },
    apiCalls() {
      return calls.filter((c) => c.url.startsWith("https://api.github.com"));
    },
    rawCalls() {
      return calls.filter((c) => c.url.startsWith("https://raw.githubusercontent.com"));
    },
  };
}

/**
 * A representative small repository for the verb tests.
 */
export const DEMO_FILES = {
  "README.md": "# demo\n\nHello world from the memory backend.\n\n## Usage\n\nRun `wit tree demo/repo`.\n\n## License\n\nMIT\n",
  "Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\n\n[dependencies]\nserde = "1"\n',
  "src/main.rs":
    "use std::fmt;\n\npub struct Widget {\n    name: String,\n}\n\nimpl Widget {\n    pub fn new(name: &str) -> Self {\n        Self { name: name.to_string() }\n    }\n\n    fn render(&self) -> String {\n        format!(\"<{}>\", self.name)\n    }\n}\n\npub fn main() {\n    let w = Widget::new(\"hello\");\n    println!(\"{}\", w.render());\n}\n",
  "src/lib.rs": "//! demo lib\n\npub mod util;\n\npub fn answer() -> u8 {\n    42\n}\n",
  "src/util/mod.rs": "pub fn helper() -> &'static str {\n    \"TODO: implement\"\n}\n",
  "scripts/run.py": "import sys\n\n\nclass Runner:\n    def __init__(self, name):\n        self.name = name\n\n    async def run(self):\n        return self.name\n\n\ndef main():\n    print(Runner('x').run())  # TODO\n",
  "assets/logo.png": new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d]),
};

export const DEMO_COMMITS = [
  { sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", author: "Ada", date: "2026-08-01T10:00:00Z", message: "add util\n\nbody", paths: ["src/util/mod.rs", "src/lib.rs"] },
  { sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", author: "Bob", date: "2026-07-01T10:00:00Z", message: "initial import", paths: ["README.md", "src/main.rs", "src/lib.rs"] },
];

/**
 * @param {import('../lib/handle.js').HandlerDeps} extra
 */
export async function deps(extra = {}) {
  const { createHostCache } = await import("../lib/handle.js");
  const bytes = await wasmBytes();
  return {
    cache: createHostCache({ ttlMs: 60_000 }),
    loadWasmBytes: async () => bytes,
    ...extra,
  };
}

/**
 * Run one request through the handler and unpack it.
 * @param {string} url
 * @param {import('../lib/handle.js').HandlerDeps} handlerDeps
 * @param {RequestInit} [init]
 */
export async function call(url, handlerDeps, init = {}) {
  const { handleRequest } = await import("../lib/handle.js");
  const res = await handleRequest(new Request(url, init), handlerDeps);
  if (!res) return { res: null, status: null, text: "", json: null, headers: null };
  const text = await res.text();
  let json = null;
  if (/application\/json/.test(res.headers.get("content-type") || "")) {
    try {
      json = JSON.parse(text);
    } catch {
      json = null;
    }
  }
  return { res, status: res.status, text, json, headers: res.headers };
}

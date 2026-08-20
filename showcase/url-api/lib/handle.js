/**
 * Shared request handler for browser page + Cloudflare Pages Function.
 * Three verbs only; MemoryBackend via wasm open/list/read; text/plain out.
 */

import { extractToken, SafeError, scrubSecrets } from "./auth.js";
import { formatCat, formatLs, formatTree } from "./format.js";
import {
  blobShaForPath,
  createHostCache,
  prefetchBlob,
  prefetchOpen,
} from "./github.js";
import { errorBody, isApiPath, parseRoute } from "./routes.js";
import {
  collectTreeFiles,
  loadWasm,
  wasmList,
  wasmOpen,
  wasmRead,
} from "./wasm-host.js";
import { ttlFromSearchParams } from "./repo-cache.js";

/**
 * @typedef {{
 *   loadWasmBytes: () => Promise<BufferSource | Response>,
 *   cache?: import('./repo-cache.js').RepoSnapshotCache,
 * }} HandlerDeps
 */

/** Isolate-lifetime cache for the worker (browser passes its own). */
let defaultCache = null;

/**
 * @param {HandlerDeps} [deps]
 */
function getCache(deps) {
  if (deps?.cache) return deps.cache;
  if (!defaultCache) defaultCache = createHostCache();
  return defaultCache;
}

/**
 * Handle an API or static-miss request.
 * Returns null when the path is not an API route (caller serves static).
 *
 * @param {Request} request
 * @param {HandlerDeps} deps
 * @returns {Promise<Response | null>}
 */
export async function handleRequest(request, deps) {
  const url = new URL(request.url);
  if (!isApiPath(url.pathname)) return null;

  if (request.method !== "GET" && request.method !== "HEAD") {
    return textResponse("error: method not allowed\n", 405);
  }

  try {
    const route = parseRoute(url);
    if (!route) return null;

    const token = extractToken({ headers: request.headers, url });
    const ttl = ttlFromSearchParams(url.search);
    const cache = getCache(deps);
    if (ttl != null) cache.setTtlMs(ttl);

    const wasmSource = await deps.loadWasmBytes();
    const api = await loadWasm(wasmSource, cache);

    await prefetchOpen(cache, route.ownerRepo, route.branch, token);
    wasmOpen(api, route.ownerRepo, route.branch);

    let body;
    if (route.verb === "ls") {
      const entries = wasmList(api, route.path);
      // Normalize kind to lowercase strings used by formatLs
      const normalized = entries.map((e) => ({
        ...e,
        kind: e.kind === "Dir" || e.kind === "dir" ? "dir" : "file",
        name: e.name,
        path: e.path,
        size_bytes: e.size_bytes ?? null,
      }));
      body = formatLs(normalized, { long: route.long });
    } else if (route.verb === "tree") {
      const files = collectTreeFiles(api, route.path, route.depth);
      const root = route.path
        ? route.path.split("/").filter(Boolean).pop() || route.path
        : ".";
      body = formatTree(
        { root, entries: files },
        { path: route.path, depth: route.depth, long: route.long },
      );
    } else if (route.verb === "cat") {
      const sha = blobShaForPath(cache, route.ownerRepo, route.branch, route.path);
      if (!sha) {
        throw new SafeError(`File not found: ${route.path}`, {
          status: 404,
          code: "not_found",
        });
      }
      await prefetchBlob(cache, route.ownerRepo, sha, token);
      const file = wasmRead(api, route.path);
      body = formatCat(file.text, { number: route.number });
    } else {
      throw new SafeError("unknown verb", { status: 400, code: "bad_verb" });
    }

    if (request.method === "HEAD") {
      return new Response(null, {
        status: 200,
        headers: plaintextHeaders(body),
      });
    }
    return textResponse(body.endsWith("\n") ? body : `${body}\n`, 200);
  } catch (err) {
    const { status, body } = errorBody(err);
    // Never log raw token-bearing URLs
    console.error("url-api error", scrubSecrets(String(err?.message || err)));
    return textResponse(body, status);
  }
}

/**
 * @param {string} body
 * @param {number} status
 */
function textResponse(body, status) {
  return new Response(body, { status, headers: plaintextHeaders(body) });
}

/**
 * @param {string} body
 */
function plaintextHeaders(body) {
  return {
    "content-type": "text/plain; charset=utf-8",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    "content-length": String(new TextEncoder().encode(body).length),
  };
}

export { createHostCache, extractToken, parseRoute, scrubSecrets };

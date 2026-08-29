/**
 * Shared request handler for browser page + Cloudflare Pages Function.
 * Three verbs only; MemoryBackend via wasm open/list/read; text/plain out.
 */

import {
  extractToken,
  SafeError,
  safeConsole,
  scrubSecrets,
  withActiveSecrets,
} from "./auth.js";
import { apiIndexText, openApiDocument } from "./discovery.js";
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
 *   persistentCache?: import('./persistent-cache.js').KvRepoCache,
 *   waitUntil?: (promise: Promise<unknown>) => void,
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

  const token = extractToken({ headers: request.headers, url });
  // Scrub both the winning Authorization PAT and any ?token= fallback so a
  // mistaken log of request.url cannot leak either value.
  const queryToken = url.searchParams.get("token") || url.searchParams.get("access_token");
  const secrets = [token, queryToken].filter(Boolean);

  return withActiveSecrets(secrets, () => handleRequestInner(request, deps, url, token));
}

/**
 * @param {Request} request
 * @param {HandlerDeps} deps
 * @param {URL} url
 * @param {string | null} token
 */
async function handleRequestInner(request, deps, url, token) {
  try {
    const route = parseRoute(url);
    if (!route) return null;

    if (route.kind === "api-index") {
      return bodyResponse(request, apiIndexText(url));
    }
    if (route.kind === "openapi") {
      return bodyResponse(
        request,
        `${JSON.stringify(openApiDocument(url), null, 2)}\n`,
        "application/json; charset=utf-8",
      );
    }

    const ttl = ttlFromSearchParams(url.search);
    const cache = getCache(deps);
    if (ttl != null) cache.setTtlMs(ttl);

    const wasmSource = await deps.loadWasmBytes();
    const api = await loadWasm(wasmSource, cache);

    // Persistence is best-effort: a KV failure must never fail the read.
    const persistent = deps.persistentCache ?? null;
    if (persistent) {
      try {
        await persistent.hydrateOpen(cache, route.ownerRepo, route.branch);
      } catch (err) {
        safeConsole.error("persistent cache hydrate failed", err?.message || err);
      }
    }

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
      if (persistent) {
        try {
          await persistent.hydrateBlob(cache, route.ownerRepo, sha);
        } catch (err) {
          safeConsole.error("persistent blob hydrate failed", err?.message || err);
        }
      }
      await prefetchBlob(cache, route.ownerRepo, sha, token);
      const file = wasmRead(api, route.path);
      body = formatCat(file.text, { number: route.number });
    } else {
      throw new SafeError("unknown verb", { status: 400, code: "bad_verb" });
    }

    if (persistent) {
      const persisted = persistent
        .persistRepo(cache, route.ownerRepo)
        .catch((err) => {
          safeConsole.error("persistent cache write failed", err?.message || err);
        });
      if (deps.waitUntil) deps.waitUntil(persisted);
      else await persisted;
    }

    return bodyResponse(request, body.endsWith("\n") ? body : `${body}\n`);
  } catch (err) {
    const { status, body } = errorBody(err);
    // Never log raw token-bearing URLs / PATs (safeConsole scrubs every arg).
    safeConsole.error("url-api error", err?.message || err, String(url));
    return textResponse(body, status);
  }
}

/**
 * 200 response for a successful body; HEAD gets the same headers, no body.
 * @param {Request} request
 * @param {string} body
 * @param {string} [contentType]
 */
function bodyResponse(request, body, contentType) {
  const headers = responseHeaders(body, contentType);
  if (request.method === "HEAD") return new Response(null, { status: 200, headers });
  return new Response(body, { status: 200, headers });
}

/**
 * @param {string} body
 * @param {number} status
 */
function textResponse(body, status) {
  return new Response(body, { status, headers: responseHeaders(body) });
}

/**
 * @param {string} body
 * @param {string} [contentType]
 */
function responseHeaders(body, contentType = "text/plain; charset=utf-8") {
  return {
    "content-type": contentType,
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    "content-length": String(new TextEncoder().encode(body).length),
  };
}

export { createHostCache, extractToken, parseRoute, scrubSecrets };

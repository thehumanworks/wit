/**
 * Cloudflare Pages Advanced Mode worker — host adapter for get_json.
 * Same MemoryBackend wasm path as the static page; curl gets text/plain.
 *
 * Wasm is imported as a CompiledWasm module (Workers disallow codegen from
 * arbitrary ArrayBuffers).
 *
 * Bindings (all optional; see wrangler.toml):
 * - WIT_REPO_CACHE  KV namespace: persistent snapshot cache (ADR 0006).
 * - GITHUB_TOKEN    secret: host fallback token used when the caller sends
 *                   none. Without it anonymous GitHub calls share the egress
 *                   IP's 60 req/h quota, which is permanently exhausted on
 *                   Cloudflare, so most requests fail with 429 (ADR 0007).
 * - WIT_RAW_BLOBS   set to "0" to fetch blobs from the REST blob endpoint
 *                   only (default: raw.githubusercontent.com first).
 */

import { safeConsole } from "../lib/auth.js";
import { handleRequest } from "../lib/handle.js";
import { KvRepoCache } from "../lib/persistent-cache.js";
// wrangler / workerd compiles this to a WebAssembly.Module
import wasmModule from "./wit_snapshot.wasm";

export default {
  /**
   * @param {Request} request
   * @param {{
   *   ASSETS: { fetch: typeof fetch },
   *   WIT_REPO_CACHE?: unknown,
   *   GITHUB_TOKEN?: string,
   *   WIT_RAW_BLOBS?: string,
   * }} env
   * @param {{ waitUntil?: (p: Promise<unknown>) => void }} [ctx]
   */
  async fetch(request, env, ctx) {
    try {
      const apiResponse = await handleRequest(request, {
        loadWasmBytes: async () => wasmModule,
        // KV-backed persistence survives isolate recycling; optional so the
        // worker still runs when the binding is not configured.
        persistentCache: env.WIT_REPO_CACHE
          ? new KvRepoCache(env.WIT_REPO_CACHE)
          : undefined,
        waitUntil: ctx?.waitUntil ? ctx.waitUntil.bind(ctx) : undefined,
        serverToken: typeof env.GITHUB_TOKEN === "string" ? env.GITHUB_TOKEN : null,
        rawBlobs: env.WIT_RAW_BLOBS !== "0",
      });
      if (apiResponse) return apiResponse;
      return env.ASSETS.fetch(request);
    } catch (err) {
      // Last-resort host log — never echo raw PATs from URL/headers.
      safeConsole.error("worker fetch failed", err?.message || err, String(request.url));
      return new Response("error: internal\n", {
        status: 500,
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }
  },
};

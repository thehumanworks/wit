/**
 * Cloudflare Pages Advanced Mode worker — host adapter for get_json.
 * Same MemoryBackend wasm path as the static page; curl gets text/plain.
 *
 * Wasm is imported as a CompiledWasm module (Workers disallow codegen from
 * arbitrary ArrayBuffers).
 */

import { safeConsole } from "../lib/auth.js";
import { handleRequest } from "../lib/handle.js";
// wrangler / workerd compiles this to a WebAssembly.Module
import wasmModule from "./wit_snapshot.wasm";

export default {
  /**
   * @param {Request} request
   * @param {{ ASSETS: { fetch: typeof fetch } }} env
   */
  async fetch(request, env) {
    try {
      const apiResponse = await handleRequest(request, {
        loadWasmBytes: async () => wasmModule,
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

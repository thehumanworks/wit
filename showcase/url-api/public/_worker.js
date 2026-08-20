/**
 * Cloudflare Pages Advanced Mode worker — host adapter for get_json.
 * Same MemoryBackend wasm path as the static page; curl gets text/plain.
 *
 * Wasm is imported as a CompiledWasm module (Workers disallow codegen from
 * arbitrary ArrayBuffers).
 */

import { handleRequest } from "../lib/handle.js";
// wrangler / workerd compiles this to a WebAssembly.Module
import wasmModule from "./wit_snapshot.wasm";

export default {
  /**
   * @param {Request} request
   * @param {{ ASSETS: { fetch: typeof fetch } }} env
   */
  async fetch(request, env) {
    const apiResponse = await handleRequest(request, {
      loadWasmBytes: async () => wasmModule,
    });
    if (apiResponse) return apiResponse;
    return env.ASSETS.fetch(request);
  },
};

/**
 * Cloudflare Pages Function — host adapter in front of get_json.
 * Same MemoryBackend wasm exports as the static page; returns text/plain for curl.
 */

import { handleRequest } from "../lib/handle.js";

/**
 * @param {EventContext} context
 */
export async function onRequest(context) {
  const { request, env, next } = context;
  const apiResponse = await handleRequest(request, {
    loadWasmBytes: async () => {
      const wasmUrl = new URL("/wit_snapshot.wasm", request.url);
      const res = await env.ASSETS.fetch(wasmUrl);
      if (!res.ok) {
        throw new Error(`failed to load wit_snapshot.wasm (${res.status})`);
      }
      return res;
    },
  });
  if (apiResponse) return apiResponse;
  // Static assets / index
  if (typeof next === "function") return next();
  return env.ASSETS.fetch(request);
}

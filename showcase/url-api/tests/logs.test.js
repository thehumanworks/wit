/**
 * Log-line token scrub: console.error/log/warn must never echo raw PATs.
 */

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { after, before, describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { createHostCache, handleRequest } from "../lib/handle.js";
import { scrubSecrets, withActiveSecrets } from "../lib/auth.js";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");

describe("console log scrubbing", () => {
  /** @type {BufferSource} */
  let wasmBytes;
  /** @type {typeof fetch} */
  let originalFetch;
  /** @type {{ log: unknown[][], info: unknown[][], warn: unknown[][], error: unknown[][], debug: unknown[][] }} */
  let captured;
  /** @type {Partial<Record<'log'|'info'|'warn'|'error'|'debug', (...args: unknown[]) => void>>} */
  let originals;

  before(async () => {
    wasmBytes = await readFile(join(root, "public/wit_snapshot.wasm"));
    originalFetch = globalThis.fetch;
    // Force GitHub prefetch failure so handleRequest hits the error log path.
    globalThis.fetch = async () => {
      throw new Error("network down for scrub test");
    };
  });

  after(() => {
    globalThis.fetch = originalFetch;
  });

  function installConsoleSpies() {
    captured = { log: [], info: [], warn: [], error: [], debug: [] };
    originals = {};
    for (const level of /** @type {const} */ (["log", "info", "warn", "error", "debug"])) {
      originals[level] = console[level].bind(console);
      console[level] = (...args) => {
        captured[level].push(args);
        // Still forward to original so test runners see failures if needed —
        // but suppress noise for this suite by not calling through.
      };
    }
  }

  function restoreConsole() {
    for (const level of Object.keys(originals)) {
      console[level] = originals[level];
    }
  }

  function flattenCaptured() {
    /** @type {string[]} */
    const parts = [];
    for (const level of Object.keys(captured)) {
      for (const args of captured[level]) {
        for (const arg of args) {
          parts.push(typeof arg === "string" ? arg : String(arg));
        }
      }
    }
    return parts.join("\n");
  }

  for (const prefix of ["", "/api"]) {
    it(`failing GET ${prefix}/cat with ?token= and Authorization never logs raw PATs`, async () => {
      const queryPat = "ghp_SHOULD_NOT_LEAK";
      const headerPat = "ghp_HEADER_MUST_STAY_SECRET";
      installConsoleSpies();
      try {
        const cache = createHostCache({ ttlMs: 60_000 });
        const res = await handleRequest(
          new Request(
            `https://example.test${prefix}/cat/demo/repo?path=README.md&token=${queryPat}`,
            { headers: { Authorization: `Bearer ${headerPat}` } },
          ),
          {
            cache,
            loadWasmBytes: async () => wasmBytes,
          },
        );
        assert.ok(res);
        assert.ok(res.status >= 400);

        const body = await res.text();
        assert.equal(body.includes(queryPat), false);
        assert.equal(body.includes(headerPat), false);

        // At least one console.error from the catch path (and it must be scrubbed).
        assert.ok(captured.error.length > 0, "expected console.error during failure");

        const joined = flattenCaptured();
        assert.equal(
          joined.includes(queryPat),
          false,
          `query token leaked into logs:\n${joined}`,
        );
        assert.equal(
          joined.includes(headerPat),
          false,
          `header token leaked into logs:\n${joined}`,
        );
        // Prefer explicit redaction markers when a token-bearing URL was logged.
        if (joined.includes("token=")) {
          assert.match(joined, /token=\[REDACTED\]/);
        }
      } finally {
        restoreConsole();
      }
    });
  }

  it("safeConsole redacts active secrets across log/warn/error", async () => {
    installConsoleSpies();
    try {
      await withActiveSecrets(["ghp_ACTIVE_SECRET"], async () => {
        const { safeConsole } = await import("../lib/auth.js");
        safeConsole.log("see", "ghp_ACTIVE_SECRET", { token: "ghp_ACTIVE_SECRET" });
        safeConsole.warn("Bearer ghp_ACTIVE_SECRET");
        safeConsole.error("url?token=ghp_ACTIVE_SECRET");
      });
      const joined = flattenCaptured();
      assert.equal(joined.includes("ghp_ACTIVE_SECRET"), false);
      assert.match(joined, /\[REDACTED\]/);
    } finally {
      restoreConsole();
    }
  });

  it("scrubSecrets alone still redacts query and ghp_ forms", () => {
    const s = scrubSecrets(
      "https://h/tree/o/r?token=ghp_LEAK&x=1 Authorization: Bearer ghp_LEAK",
    );
    assert.equal(s.includes("ghp_LEAK"), false);
    assert.match(s, /\[REDACTED\]/);
  });
});

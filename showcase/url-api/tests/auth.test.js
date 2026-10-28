/**
 * Auth + secret scrubbing tests.
 * Run: node --test tests/auth.test.js
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  SafeError,
  extractToken,
  scrubSecrets,
} from "../lib/auth.js";

describe("extractToken", () => {
  it("prefers Authorization Bearer over ?token=", () => {
    const token = extractToken({
      headers: { Authorization: "Bearer header-pat" },
      url: "https://h/tree/o/r?token=query-pat",
    });
    assert.equal(token, "header-pat");
  });

  it("accepts Authorization token scheme", () => {
    assert.equal(
      extractToken({
        headers: { authorization: "token ghp_abc" },
        url: "https://h/ls/o/r",
      }),
      "ghp_abc",
    );
  });

  it("falls back to ?token=", () => {
    assert.equal(
      extractToken({
        headers: {},
        url: "https://h/cat/o/r?path=README&token=query-only",
      }),
      "query-only",
    );
  });

  it("returns null when absent", () => {
    assert.equal(extractToken({ headers: {}, url: "https://h/tree/o/r" }), null);
  });
});

describe("scrubSecrets", () => {
  it("redacts query token and bearer headers", () => {
    const s = scrubSecrets(
      "fail https://h/tree/o/r?token=ghp_SECRET123&path=x Authorization: Bearer ghp_SECRET123",
    );
    assert.equal(s.includes("ghp_SECRET123"), false);
    assert.match(s, /\[REDACTED\]/);
  });

  it("SafeError message is scrubbed", () => {
    const err = new SafeError("boom ?token=ghp_LEAK", { status: 400 });
    assert.equal(err.message.includes("ghp_LEAK"), false);
    assert.match(err.message, /REDACTED/);
  });
});

describe("safeConsole", () => {
  it("scrubs every console.error argument", async () => {
    const { withActiveSecrets } = await import("../lib/auth.js");
    const seen = [];
    const orig = console.error;
    console.error = (...args) => {
      seen.push(args);
    };
    try {
      await withActiveSecrets(["ghp_LOG_SECRET"], async ({ console: safeConsole }) => {
        safeConsole.error("hit", "ghp_LOG_SECRET", "url?token=ghp_LOG_SECRET");
      });
      const flat = seen.flat().join(" ");
      assert.equal(flat.includes("ghp_LOG_SECRET"), false);
      assert.match(flat, /REDACTED/);
    } finally {
      console.error = orig;
    }
  });
});

it("request contexts isolate overlapping secrets and clear on success and failure", async () => {
  const { withActiveSecrets, scrubSecrets } = await import("../lib/auth.js");
  let endA, endB, contextA, contextB;
  const a = withActiveSecrets(["synthetic-first"], async (scope) => {
    contextA = scope;
    await new Promise((resolve) => { endA = resolve; });
    assert.equal(scope.scrub("synthetic-first"), "[REDACTED]");
  });
  const b = withActiveSecrets(["synthetic-second"], async (scope) => {
    contextB = scope;
    await new Promise((resolve) => { endB = resolve; });
    assert.equal(scope.scrub("synthetic-second"), "[REDACTED]");
    throw new Error("controlled failure");
  });
  endA(); await a;
  assert.equal(contextB.scrub("synthetic-second"), "[REDACTED]");
  endB(); await assert.rejects(b, /controlled failure/);
  assert.equal(scrubSecrets("synthetic-first synthetic-second"), "synthetic-first synthetic-second", "no module-level secret registration survives");
  assert.equal(contextA.scrub("late callback"), "[REDACTED]");
  assert.equal(contextB.scrub("late callback"), "[REDACTED]");
});

it("scrubs Cookie and arbitrary Authorization schemes without forwarding them", async () => {
  const { requestSecrets, withActiveSecrets, extractToken, scrubSecrets } = await import("../lib/auth.js");
  const request = new Request("https://example.test/api/tree/o/r?token=synthetic-query&access_token=synthetic-other", {
    headers: { Authorization: "Basic c3ludGhldGlj", Cookie: 'session="synthetic-cookie"; other=synthetic-extra' },
  });
  assert.equal(extractToken({ headers: request.headers }), null);
  await withActiveSecrets(requestSecrets(request), async ({ scrub }) => {
    for (const value of ["c3ludGhldGlj", "synthetic-cookie", "synthetic-extra", "synthetic-query", "synthetic-other"]) {
      assert.equal(scrub(value), "[REDACTED]");
    }
  });
  for (const value of [JSON.stringify({ Cookie: 'session="synthetic-cookie"' }), "Cookie: session=synthetic-cookie", '"Authorization":"Basic c3ludGhldGlj"', "Proxy-Authorization: Custom opaque-value", "Set-Cookie: session=synthetic-cookie"]) {
    assert.match(scrubSecrets(value), /\[REDACTED\]/);
    assert.doesNotMatch(scrubSecrets(value), /synthetic-cookie|c3ludGhldGlj|opaque-value/);
  }
});

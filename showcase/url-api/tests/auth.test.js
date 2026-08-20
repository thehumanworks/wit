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

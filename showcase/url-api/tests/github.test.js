/**
 * GitHub client unit tests: rate-limit detection, failure mapping, and the
 * raw.githubusercontent.com blob path.
 */

import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { SafeError } from "../lib/auth.js";
import {
  bytesToBase64,
  fetchRawBlob,
  githubFailure,
  githubGet,
  isRateLimited,
  rateLimitError,
  rateLimitRetryAfterSeconds,
} from "../lib/github.js";

const base = { rateLimitRemaining: null, rateLimitReset: null, retryAfter: null };

describe("isRateLimited", () => {
  it("recognises primary and secondary limits but not ordinary 403s", () => {
    assert.equal(isRateLimited({ ...base, status: 429, body: "" }), true);
    assert.equal(isRateLimited({ ...base, status: 403, body: "", rateLimitRemaining: 0 }), true);
    assert.equal(isRateLimited({ ...base, status: 403, body: "", retryAfter: 30 }), true);
    assert.equal(isRateLimited({ ...base, status: 403, body: '{"message":"API rate limit exceeded"}' }), true);
    assert.equal(isRateLimited({ ...base, status: 403, body: '{"message":"Resource not accessible"}', rateLimitRemaining: 55 }), false);
    assert.equal(isRateLimited({ ...base, status: 404, body: "rate limit" }), false);
  });
});

describe("rateLimitRetryAfterSeconds / rateLimitError", () => {
  it("prefers retry-after, then the reset epoch", () => {
    assert.equal(rateLimitRetryAfterSeconds({ ...base, status: 429, body: "", retryAfter: 12 }), 12);
    const now = () => 1_000_000_000;
    assert.equal(rateLimitRetryAfterSeconds({ ...base, status: 403, body: "", rateLimitReset: 1_000_045 }, now), 45);
    assert.equal(rateLimitRetryAfterSeconds({ ...base, status: 403, body: "", rateLimitReset: 999_000 }, now), 1);
    assert.equal(rateLimitRetryAfterSeconds({ ...base, status: 403, body: "" }), null);
  });

  it("builds a 429 SafeError whose hint depends on the token source", () => {
    const anon = rateLimitError({ ...base, status: 403, body: "", retryAfter: 5 });
    assert.ok(anon instanceof SafeError);
    assert.equal(anon.status, 429);
    assert.equal(anon.code, "rate_limited");
    assert.equal(anon.retryAfterSeconds, 5);
    assert.match(anon.message, /resets in 5s.*no GitHub credential configured/);
    assert.match(rateLimitError({ ...base, status: 429, body: "" }, { tokenSource: "host" }).message, /host credential's quota/);
    assert.match(rateLimitError({ ...base, status: 429, body: "" }, { tokenSource: "caller" }).message, /Your credential's/);
    // The generic secret scrubber must leave the guidance readable.
    assert.equal(anon.message.includes("[REDACTED]"), false);
  });
});

describe("githubFailure", () => {
  const ctx = { notFound: "repo missing", label: "/repos/o/r" };
  it("maps statuses to stable codes", () => {
    assert.equal(githubFailure({ ...base, status: 404, body: "" }, ctx).code, "not_found");
    assert.equal(githubFailure({ ...base, status: 401, body: "" }, ctx).code, "bad_token");
    assert.equal(githubFailure({ ...base, status: 403, body: "{}" }, ctx).code, "forbidden");
    assert.equal(githubFailure({ ...base, status: 403, body: "rate limit" }, ctx).code, "rate_limited");
    const upstream = githubFailure({ ...base, status: 503, body: "" }, ctx);
    assert.equal(upstream.status, 502);
    assert.equal(upstream.code, "github_status");
    assert.match(upstream.message, /HTTP 503/);
  });
});

describe("githubGet", () => {
  let original;
  before(() => {
    original = globalThis.fetch;
  });
  after(() => {
    globalThis.fetch = original;
  });

  it("reads rate-limit headers and sends the bearer token", async () => {
    let seen;
    globalThis.fetch = async (url, init) => {
      seen = { url: String(url), auth: init.headers.Authorization };
      return new Response("{}", {
        status: 200,
        headers: { "x-ratelimit-remaining": "41", "x-ratelimit-reset": "1700000000" },
      });
    };
    const res = await githubGet("/repos/o/r", "tok");
    assert.equal(seen.url, "https://api.github.com/repos/o/r");
    assert.equal(seen.auth, "Bearer tok");
    assert.equal(res.rateLimitRemaining, 41);
    assert.equal(res.rateLimitReset, 1700000000);
    assert.equal(res.retryAfter, null);
  });

  it("wraps transport failures as 502 without leaking tokens", async () => {
    globalThis.fetch = async () => {
      throw new Error("ECONNRESET token ghp_LEAK");
    };
    await assert.rejects(githubGet("/repos/o/r", "ghp_LEAK"), (err) => {
      assert.equal(err.status, 502);
      assert.equal(err.message.includes("ghp_LEAK"), false);
      return true;
    });
  });
});

describe("fetchRawBlob", () => {
  let original;
  before(() => {
    original = globalThis.fetch;
  });
  after(() => {
    globalThis.fetch = original;
  });

  it("only fires for full commit SHAs and encodes the path", async () => {
    let seen = null;
    globalThis.fetch = async (url) => {
      seen = String(url);
      return new Response(new Uint8Array([104, 105]), { status: 200 });
    };
    assert.equal(await fetchRawBlob("o/r", "main", "a.txt", null), null);
    assert.equal(seen, null);
    const sha = "a".repeat(40);
    const blob = await fetchRawBlob("o/r", sha, "dir with space/f#1.txt", null);
    assert.equal(seen, `https://raw.githubusercontent.com/o/r/${sha}/dir%20with%20space/f%231.txt`);
    assert.deepEqual(blob, { size: 2, contentBase64: "aGk=" });
  });

  it("returns null on non-200, oversized, or thrown fetches", async () => {
    const sha = "b".repeat(40);
    globalThis.fetch = async () => new Response("no", { status: 404 });
    assert.equal(await fetchRawBlob("o/r", sha, "x", null), null);
    globalThis.fetch = async () =>
      new Response("x", { status: 200, headers: { "content-length": String(2 * 1024 * 1024) } });
    assert.equal(await fetchRawBlob("o/r", sha, "x", null), null);
    globalThis.fetch = async () => {
      throw new Error("down");
    };
    assert.equal(await fetchRawBlob("o/r", sha, "x", null), null);
  });

  it("base64-encodes arbitrary bytes in chunks", () => {
    const bytes = new Uint8Array(70_000);
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = i % 251;
    assert.equal(bytesToBase64(bytes), Buffer.from(bytes).toString("base64"));
  });
});

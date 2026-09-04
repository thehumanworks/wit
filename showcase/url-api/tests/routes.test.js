/**
 * Route table tests — three verbs, path as query.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { errorBody, isApiPath, parseRoute } from "../lib/routes.js";
import { SafeError } from "../lib/auth.js";

describe("parseRoute", () => {
  it("parses /tree/{owner}/{repo} with path/branch/depth query", () => {
    const r = parseRoute(
      "https://h/tree/octocat/Hello-World?path=src&branch=main&depth=2",
    );
    assert.deepEqual(
      {
        verb: r.verb,
        ownerRepo: r.ownerRepo,
        path: r.path,
        branch: r.branch,
        depth: r.depth,
      },
      {
        verb: "tree",
        ownerRepo: "octocat/Hello-World",
        path: "src",
        branch: "main",
        depth: 2,
      },
    );
  });

  it("aliases ?ref= to branch", () => {
    const r = parseRoute("https://h/ls/octocat/Hello-World?ref=master");
    assert.equal(r.branch, "master");
    assert.equal(r.verb, "ls");
  });

  it("requires ?path= for cat", () => {
    assert.throws(
      () => parseRoute("https://h/cat/octocat/Hello-World"),
      (err) => err instanceof SafeError && err.status === 400,
    );
  });

  it("rejects path segments after owner/repo", () => {
    assert.throws(
      () => parseRoute("https://h/tree/octocat/Hello-World/README"),
      (err) =>
        err instanceof SafeError &&
        /path belongs in \?path=/.test(err.message),
    );
  });

  it("returns null for non-API paths", () => {
    assert.equal(parseRoute("https://h/"), null);
    assert.equal(parseRoute("https://h/index.html"), null);
    assert.equal(isApiPath("/wit_snapshot.wasm"), false);
    assert.equal(isApiPath("/tree/o/r"), true);
  });

  it("ignores unknown query keys", () => {
    const r = parseRoute(
      "https://h/tree/octocat/Hello-World?backend=disk&path=&weird=1",
    );
    assert.equal(r.verb, "tree");
    assert.equal(r.path, "");
  });
});

describe("/api prefix alias", () => {
  const suffixes = [
    "tree/octocat/Hello-World?path=src&branch=main&depth=2",
    "ls/octocat/Hello-World?ref=master",
    "cat/octocat/Hello-World?path=README.md",
  ];

  for (const suffix of suffixes) {
    it(`parses /api/${suffix} exactly like /${suffix}`, () => {
      assert.deepEqual(
        parseRoute(`https://h/api/${suffix}`),
        parseRoute(`https://h/${suffix}`),
      );
    });
  }

  it("requires ?path= for /api/cat", () => {
    assert.throws(
      () => parseRoute("https://h/api/cat/octocat/Hello-World"),
      (err) =>
        err instanceof SafeError &&
        err.status === 400 &&
        err.code === "path_required",
    );
  });

  it("rejects a missing owner/repo the same way as unprefixed", () => {
    assert.throws(
      () => parseRoute("https://h/api/tree"),
      (err) => err instanceof SafeError && err.code === "bad_route",
    );
  });

  it("treats unknown /api verbs as non-API", () => {
    assert.equal(isApiPath("/api/foo"), false);
    assert.equal(parseRoute("https://h/api/foo"), null);
    assert.equal(isApiPath("/api/clone/o/r"), false);
    assert.equal(parseRoute("https://h/api/clone/o/r"), null);
    // Only one prefix level is stripped.
    assert.equal(isApiPath("/api/api/tree/o/r"), false);
  });

  it("marks every repo verb and search as API paths", () => {
    for (const verb of ["rg", "head", "tail", "stats", "outline", "refs", "commits"]) {
      assert.equal(isApiPath(`/api/${verb}/o/r`), true, verb);
      assert.equal(isApiPath(`/${verb}/o/r`), true, verb);
    }
    assert.equal(isApiPath("/search"), true);
    assert.equal(isApiPath("/api/search"), true);
  });

  it("marks prefixed verb paths as API paths", () => {
    assert.equal(isApiPath("/api/tree/x/y"), true);
    assert.equal(isApiPath("/api/ls/x/y"), true);
    assert.equal(isApiPath("/api/cat/x/y"), true);
  });
});

describe("/api discovery routes", () => {
  it("routes /api (with or without a trailing slash) to the index", () => {
    assert.equal(isApiPath("/api"), true);
    assert.equal(isApiPath("/api/"), true);
    assert.deepEqual(parseRoute("https://h/api"), { kind: "api-index" });
    assert.deepEqual(parseRoute("https://h/api/"), { kind: "api-index" });
  });

  it("routes /api/openapi.json to the OpenAPI document", () => {
    assert.equal(isApiPath("/api/openapi.json"), true);
    assert.deepEqual(parseRoute("https://h/api/openapi.json"), { kind: "openapi" });
  });

  it("keeps discovery behind the /api prefix", () => {
    assert.equal(isApiPath("/"), false);
    assert.equal(parseRoute("https://h/"), null);
    assert.equal(isApiPath("/openapi.json"), false);
    assert.equal(parseRoute("https://h/openapi.json"), null);
    assert.equal(isApiPath("/api/api"), false);
    assert.equal(isApiPath("/api/openapi.json/extra"), false);
  });
});

describe("errorBody", () => {
  it("never echoes tokens", () => {
    const { body } = errorBody(new SafeError("nope ?token=ghp_ZZZ"));
    assert.equal(body.includes("ghp_ZZZ"), false);
    assert.match(body, /^error: /);
  });

  it("renders JSON errors with code, status, and retry-after", () => {
    const err = new SafeError("slow", { status: 429, code: "rate_limited" });
    err.retryAfterSeconds = 30;
    const out = errorBody(err, { format: "json" });
    assert.equal(out.status, 429);
    assert.equal(out.retryAfter, 30);
    assert.deepEqual(JSON.parse(out.body), { error: "slow", code: "rate_limited", status: 429 });
    assert.deepEqual(errorBody(new Error("plain"), { format: "json" }).status, 500);
  });
});

describe("verb-specific query parsing", () => {
  it("cat: lines / start / end / n", () => {
    const r = parseRoute("https://h/cat/o/r?path=a.rs&lines=10-20&n=1");
    assert.deepEqual([r.verb, r.path, r.lines, r.number], ["cat", "a.rs", { start: 10, end: 20 }, true]);
    const se = parseRoute("https://h/cat/o/r?path=a.rs&start=5&end=9");
    assert.deepEqual(se.lines, { start: 5, end: 9 });
    const mixed = parseRoute("https://h/cat/o/r?path=a.rs&lines=5-&end=9");
    assert.deepEqual(mixed.lines, { start: 5, end: 9 });
    assert.throws(() => parseRoute("https://h/cat/o/r?path=a.rs&lines=x"), (e) => e.code === "bad_lines");
    assert.throws(() => parseRoute("https://h/cat/o/r?path=a.rs&start=9&end=2"), (e) => e.code === "bad_lines");
    assert.throws(() => parseRoute("https://h/cat/o/r?path=a.rs&start=0"), (e) => e.code === "bad_lines");
  });

  it("head/tail: lines count, plus, and N/n numbering", () => {
    const h = parseRoute("https://h/head/o/r?path=a&lines=25&N=1");
    assert.deepEqual([h.count, h.number], [25, true]);
    assert.equal(parseRoute("https://h/head/o/r?path=a").count, 10);
    const t = parseRoute("https://h/tail/o/r?path=a&plus=40&n=true");
    assert.deepEqual([t.count, t.fromLine, t.number], [10, 40, true]);
    assert.throws(() => parseRoute("https://h/tail/o/r?path=a&plus=0"), (e) => e.code === "bad_plus");
    assert.throws(() => parseRoute("https://h/head/o/r"), (e) => e.code === "path_required");
  });

  it("rg: pattern, flags, context, and bounded limits", () => {
    const r = parseRoute("https://h/rg/o/r?q=fn%20main&path=src&glob=*.rs&i=1&l=1&C=3&max=5000&max_files=9999&ignore=a,b&ignore=c");
    assert.equal(r.pattern, "fn main");
    assert.equal(r.path, "src");
    assert.equal(r.glob, "*.rs");
    assert.equal(r.ignoreCase, true);
    assert.equal(r.filesWithMatches, true);
    assert.deepEqual([r.before, r.after], [3, 3]);
    assert.equal(r.maxMatches, 2000, "capped at the ceiling");
    assert.equal(r.maxFiles, 1000, "capped at the ceiling");
    assert.deepEqual(r.ignore, ["a", "b", "c"]);
    const ba = parseRoute("https://h/rg/o/r?q=x&B=1&A=4&C=2&c=1&w=1&v=1&S=1&long=1");
    assert.deepEqual([ba.before, ba.after, ba.countOnly, ba.wordRegexp, ba.invert, ba.smartCase, ba.long], [1, 4, true, true, true, true, true]);
    assert.throws(() => parseRoute("https://h/rg/o/r"), (e) => e.code === "pattern_required");
    assert.throws(() => parseRoute(`https://h/rg/o/r?q=${"a".repeat(513)}`), (e) => e.code === "pattern_too_long");
    assert.equal(parseRoute(`https://h/rg/o/r?q=${"a".repeat(512)}`).pattern.length, 512);
    assert.throws(() => parseRoute("https://h/rg/o/r?q=x&C=-1"), (e) => e.code === "bad_context");
    assert.throws(() => parseRoute("https://h/rg/o/r?q=x&max=0"), (e) => e.code === "bad_max");
  });

  it("stats / outline / commits limits", () => {
    assert.equal(parseRoute("https://h/stats/o/r").largest, 10);
    assert.equal(parseRoute("https://h/stats/o/r?largest=500").largest, 100);
    assert.equal(parseRoute("https://h/outline/o/r?path=a").maxSymbols, 2000);
    assert.equal(parseRoute("https://h/commits/o/r?n=500").count, 100);
    assert.equal(parseRoute("https://h/commits/o/r").count, 10);
  });

  it("search: query composition inputs, limit, sort, and format", () => {
    const s = parseRoute("https://h/api/search?q=terminal%20ui&p=rata&lang=rust&limit=500&sort=updated&format=json");
    assert.deepEqual(s, { kind: "search", format: "json", query: "terminal ui", pattern: "rata", lang: "rust", limit: 100, sort: "updated" });
    assert.equal(parseRoute("https://h/search?q=x").sort, "stars");
    assert.throws(() => parseRoute("https://h/search"), (e) => e.code === "query_required");
    assert.throws(() => parseRoute("https://h/search/o/r?q=x"), (e) => e.code === "bad_route");
    assert.throws(() => parseRoute("https://h/search?q=x&sort=nope"), (e) => e.code === "bad_sort");
  });

  it("format and fresh flags, Accept negotiation, flag negation", () => {
    assert.equal(parseRoute("https://h/tree/o/r").format, "text");
    assert.equal(parseRoute("https://h/tree/o/r?format=json").format, "json");
    assert.equal(parseRoute("https://h/tree/o/r", { accept: "application/json" }).format, "json");
    assert.equal(parseRoute("https://h/tree/o/r", { accept: "text/plain, application/json" }).format, "text");
    assert.equal(parseRoute("https://h/tree/o/r?format=text", { accept: "application/json" }).format, "text");
    assert.throws(() => parseRoute("https://h/tree/o/r?format=xml"), (e) => e.code === "bad_format");
    assert.equal(parseRoute("https://h/tree/o/r?fresh").fresh, true);
    assert.equal(parseRoute("https://h/tree/o/r?fresh=0").fresh, false);
    assert.equal(parseRoute("https://h/tree/o/r?l=false").long, false);
    assert.equal(parseRoute("https://h/tree/o/r?long").long, true);
  });
});

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

  it("treats /api and unknown /api verbs as non-API", () => {
    assert.equal(isApiPath("/api"), false);
    assert.equal(isApiPath("/api/foo"), false);
    assert.equal(parseRoute("https://h/api"), null);
    assert.equal(parseRoute("https://h/api/foo"), null);
    // Only one prefix level is stripped.
    assert.equal(isApiPath("/api/api/tree/o/r"), false);
  });

  it("marks prefixed verb paths as API paths", () => {
    assert.equal(isApiPath("/api/tree/x/y"), true);
    assert.equal(isApiPath("/api/ls/x/y"), true);
    assert.equal(isApiPath("/api/cat/x/y"), true);
  });
});

describe("errorBody", () => {
  it("never echoes tokens", () => {
    const { body } = errorBody(new SafeError("nope ?token=ghp_ZZZ"));
    assert.equal(body.includes("ghp_ZZZ"), false);
    assert.match(body, /^error: /);
  });
});

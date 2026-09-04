import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { WitClient, WitError, buildQuery } from "../src/index.ts";

interface Recorded {
  url: string;
  headers: Record<string, string>;
}

/** A fetch double that records requests and answers from a route table. */
function fakeFetch(routes: Record<string, (url: URL) => Response | Promise<Response>>) {
  const calls: Recorded[] = [];
  const fetch = async (input: string, init?: RequestInit) => {
    const url = new URL(input);
    calls.push({ url: input, headers: { ...((init?.headers as Record<string, string>) ?? {}) } });
    const key = url.pathname;
    const handler = routes[key] ?? routes["*"];
    if (!handler) return new Response("error: not found\n", { status: 404 });
    return handler(url);
  };
  return { fetch, calls };
}

const json = (body: unknown, init: ResponseInit = {}) =>
  new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
    ...init,
  });

const PROV = {
  api_version: "2",
  repo: "o/r",
  requested_ref: "main",
  ref: "refs/heads/main",
  commit: "c".repeat(40),
  cache: "miss",
};

describe("buildQuery", () => {
  it("drops empty values, expands arrays, and maps booleans to 1", () => {
    assert.equal(
      buildQuery({ path: "src", l: true, v: false, n: undefined, x: null, ignore: ["a", "b"], max: 5 }),
      "path=src&l=1&ignore=a&ignore=b&max=5",
    );
  });
});

describe("WitClient", () => {
  it("targets the public host by default and sends Accept + Authorization", async () => {
    const { fetch, calls } = fakeFetch({ "*": () => json({ ...PROV, verb: "ls", path: ".", entries: [] }) });
    const client = new WitClient({ fetch, token: "ghp_x", headers: { "User-Agent": "test" } });
    await client.repo("o/r").ls();
    assert.equal(client.baseUrl, "https://wit.thehuman.sh/api");
    assert.equal(calls[0].url, "https://wit.thehuman.sh/api/ls/o/r?l=1&format=json");
    assert.equal(calls[0].headers.Authorization, "Bearer ghp_x");
    assert.equal(calls[0].headers.Accept, "application/json");
    assert.equal(calls[0].headers["User-Agent"], "test");
  });

  it("builds every verb URL with ref, fresh, ignore and verb-specific params", async () => {
    const { fetch, calls } = fakeFetch({ "*": () => json({ ...PROV }) });
    const repo = new WitClient({ fetch, baseUrl: "http://h/api/" }).repo("o/r", "dev");
    await repo.stats({ path: "src", largest: 3, fresh: true, ignore: ["*.md"] });
    await repo.tree({ path: "src", depth: 1 });
    await repo.cat("a.rs", { lines: [10, 20] });
    await repo.cat("a.rs", { lines: [null, 5] });
    await repo.cat("a.rs", { lines: "7-" });
    await repo.head("a.rs", 3);
    await repo.tail("a.rs", { plus: 40 });
    await repo.outline("a.rs", { maxSymbols: 10 });
    await repo.rg("fn main", { glob: "*.rs", ignoreCase: true, context: 2, max: 9, maxFiles: 4 });
    await repo.rgFiles("todo", { path: "src", long: true });
    await repo.rgCounts("x", { before: 1, after: 2 });
    await repo.refs();
    await repo.commits({ path: "a.rs", n: 3 });
    await repo.at(null).ls("src");
    const paths = calls.map((c) => c.url.replace("http://h/api", ""));
    assert.deepEqual(paths, [
      "/stats/o/r?ref=dev&fresh=1&ignore=*.md&path=src&largest=3&format=json",
      "/tree/o/r?ref=dev&path=src&depth=1&l=1&format=json",
      "/cat/o/r?ref=dev&path=a.rs&lines=10-20&format=json",
      "/cat/o/r?ref=dev&path=a.rs&lines=-5&format=json",
      "/cat/o/r?ref=dev&path=a.rs&lines=7-&format=json",
      "/head/o/r?ref=dev&path=a.rs&lines=3&format=json",
      "/tail/o/r?ref=dev&path=a.rs&plus=40&format=json",
      "/outline/o/r?ref=dev&path=a.rs&max_symbols=10&format=json",
      "/rg/o/r?ref=dev&q=fn+main&glob=*.rs&i=1&C=2&max=9&max_files=4&format=json",
      "/rg/o/r?ref=dev&q=todo&path=src&long=1&l=1&format=json",
      "/rg/o/r?ref=dev&q=x&B=1&A=2&c=1&format=json",
      "/refs/o/r?format=json",
      "/commits/o/r?ref=dev&path=a.rs&n=3&format=json",
      "/ls/o/r?path=src&l=1&format=json",
    ]);
  });

  it("search composes query parameters and text() returns CLI plaintext", async () => {
    const { fetch, calls } = fakeFetch({
      "/api/search": () => json({ api_version: "2", verb: "search", query: "x", sort: "stars", total_count: 0, items: [] }),
      "/api/tree/o/r": () => new Response("src\n  lib.rs\n", { status: 200 }),
    });
    const client = new WitClient({ fetch, baseUrl: "http://h/api" });
    const result = await client.search({ query: "terminal ui", lang: "rust", limit: 5, sort: "best" });
    assert.equal(result.total_count, 0);
    assert.equal(calls[0].url, "http://h/api/search?q=terminal+ui&lang=rust&limit=5&sort=best&format=json");
    const text = await client.repo("o/r").text("tree", { path: "src" });
    assert.equal(text, "src\n  lib.rs\n");
    assert.equal(calls[1].url, "http://h/api/tree/o/r?path=src");
    assert.equal(calls[1].headers.Accept, "text/plain");
  });

  it("throws WitError with the API code, status and retry-after", async () => {
    const { fetch } = fakeFetch({
      "/api/cat/o/r": () => json({ error: "File not found: x", code: "not_found", status: 404 }, { status: 404 }),
      "/api/tree/o/r": () =>
        new Response("error: GitHub API rate limit exceeded (resets in 12s). hint\n", {
          status: 429,
          headers: { "retry-after": "12" },
        }),
    });
    const client = new WitClient({ fetch, baseUrl: "http://h/api" });
    await assert.rejects(client.repo("o/r").cat("x"), (err: unknown) => {
      assert.ok(err instanceof WitError);
      assert.equal(err.status, 404);
      assert.equal(err.code, "not_found");
      assert.equal(err.message, "File not found: x");
      assert.equal(err.isRateLimited, false);
      return true;
    });
    await assert.rejects(client.repo("o/r").text("tree"), (err: unknown) => {
      assert.ok(err instanceof WitError);
      assert.equal(err.status, 429);
      assert.equal(err.retryAfter, 12);
      assert.equal(err.isRateLimited, true);
      assert.match(err.message, /^GitHub API rate limit exceeded/);
      return true;
    });
  });

  it("retries 429s when asked, honouring retry-after up to the cap", async () => {
    let attempts = 0;
    const { fetch } = fakeFetch({
      "/api/ls/o/r": () => {
        attempts += 1;
        if (attempts < 3) {
          return new Response("error: slow\n", { status: 429, headers: { "retry-after": "60" } });
        }
        return json({ ...PROV, verb: "ls", path: ".", entries: [] });
      },
    });
    const client = new WitClient({ fetch, baseUrl: "http://h/api", retries: 2, maxRetryDelayMs: 1 });
    const result = await client.repo("o/r").ls();
    assert.equal(result.verb, "ls");
    assert.equal(attempts, 3);

    attempts = 0;
    const strict = new WitClient({ fetch, baseUrl: "http://h/api" });
    await assert.rejects(strict.repo("o/r").ls(), (err: unknown) => err instanceof WitError && err.status === 429);
    assert.equal(attempts, 1);
  });

  it("readSymbol chains outline and cat", async () => {
    const { fetch, calls } = fakeFetch({
      "/api/outline/o/r": () =>
        json({
          ...PROV,
          verb: "outline",
          path: "a.rs",
          blob_sha: "b",
          language: "Rust",
          supported: true,
          total_lines: 40,
          truncated: false,
          symbols: [
            { line: 3, end_line: 9, kind: "struct", name: "Widget", signature: "pub struct Widget {" },
            { line: 12, end_line: 30, kind: "impl", name: "Widget", signature: "impl Widget {" },
          ],
        }),
      "/api/cat/o/r": (url) =>
        json({
          ...PROV,
          verb: "cat",
          path: "a.rs",
          blob_sha: "b",
          total_lines: 40,
          start_line: Number(url.searchParams.get("lines")!.split("-")[0]),
          end_line: Number(url.searchParams.get("lines")!.split("-")[1]),
          text: "code",
        }),
    });
    const repo = new WitClient({ fetch, baseUrl: "http://h/api" }).repo("o/r");
    const hit = await repo.readSymbol("a.rs", "Widget", { kind: "impl", padding: 2 });
    assert.ok(hit);
    assert.equal(hit.symbol.kind, "impl");
    assert.deepEqual([hit.start_line, hit.end_line], [10, 32]);
    assert.match(calls[1].url, /lines=10-32/);
    assert.equal(await repo.readSymbol("a.rs", "nope"), null);
  });

  it("context locates matches then reads a window per file", async () => {
    const { fetch, calls } = fakeFetch({
      "/api/rg/o/r": () =>
        json({
          ...PROV,
          verb: "rg",
          pattern: "x",
          path: ".",
          glob: null,
          files_scanned: 2,
          files_candidate: 2,
          files_skipped_binary: 0,
          match_count: 3,
          truncated: false,
          matches: [
            { path: "a.rs", line: 5, text: "x", is_context: false },
            { path: "a.rs", line: 9, text: "x", is_context: false },
            { path: "b.rs", line: 100, text: "x", is_context: false },
          ],
        }),
      "/api/cat/o/r": (url) =>
        json({
          ...PROV,
          verb: "cat",
          path: url.searchParams.get("path"),
          blob_sha: "b",
          total_lines: 200,
          start_line: 1,
          end_line: 2,
          text: "snippet",
        }),
    });
    const repo = new WitClient({ fetch, baseUrl: "http://h/api" }).repo("o/r");
    const snippets = await repo.context("x", { window: 3, maxSnippets: 5 });
    assert.deepEqual(snippets.map((s) => s.path), ["a.rs", "b.rs"]);
    assert.equal(snippets[0].commit, PROV.commit);
    assert.match(calls[1].url, /path=a\.rs&lines=2-8/);
    assert.match(calls[2].url, /path=b\.rs&lines=97-103/);
  });

  it("rejects malformed owner/repo before any request", () => {
    const client = new WitClient({ fetch: async () => new Response("") });
    assert.throws(() => client.repo("nope"), /owner\/repo/);
    assert.throws(() => client.repo("a/b/c"), /owner\/repo/);
  });
});

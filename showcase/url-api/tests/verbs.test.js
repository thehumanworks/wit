/**
 * Agent verbs end to end through handleRequest against the in-memory
 * fixture GitHub: cat ranges, head, tail, outline, stats, rg, refs, commits,
 * search, JSON output, provenance headers, fresh, ignore, host token and
 * rate-limit mapping, raw blob fetches.
 */

import assert from "node:assert/strict";
import { after, before, beforeEach, describe, it } from "node:test";
import {
  COMMIT_SHA,
  DEMO_COMMITS,
  DEMO_FILES,
  call,
  deps,
  fixtureGitHub,
} from "./helpers.js";

const BASE = "https://example.test";

describe("agent verbs", () => {
  const gh = fixtureGitHub({
    files: DEMO_FILES,
    branches: ["dev"],
    tags: ["v0.1.0"],
    commits: DEMO_COMMITS,
    searchItems: [
      {
        full_name: "ratatui/ratatui",
        description: "A Rust crate for cooking up terminal user interfaces",
        language: "Rust",
        stargazers_count: 12000,
        forks_count: 300,
        html_url: "https://github.com/ratatui/ratatui",
        default_branch: "main",
        pushed_at: "2026-08-30T00:00:00Z",
        archived: false,
        topics: ["tui"],
      },
      { full_name: "o/tiny", stargazers_count: 3 },
    ],
  });

  before(() => gh.install());
  after(() => gh.restore());
  beforeEach(() => gh.reset());

  it("cat?lines=A-B returns the inclusive range and numbers from the real line", async () => {
    const d = await deps();
    const { status, text, headers } = await call(`${BASE}/cat/demo/repo?path=src/main.rs&lines=3-5&n=1`, d);
    assert.equal(status, 200);
    assert.equal(text, "     3  pub struct Widget {\n     4      name: String,\n     5  }\n");
    assert.equal(headers.get("x-wit-commit"), COMMIT_SHA);
    assert.equal(headers.get("x-wit-ref"), "refs/heads/main");
    assert.equal(headers.get("x-wit-cache"), "miss");
    assert.equal(headers.get("x-wit-auth"), "anonymous");
  });

  it("cat accepts start=/end=, open-ended ranges, and rejects out-of-bounds with 416", async () => {
    const d = await deps();
    const a = await call(`${BASE}/cat/demo/repo?path=src/lib.rs&start=5`, d);
    assert.equal(a.text, "pub fn answer() -> u8 {\n    42\n}\n");
    const b = await call(`${BASE}/cat/demo/repo?path=src/lib.rs&lines=-2`, d);
    assert.equal(b.text, "//! demo lib\n\n", "a blank last line is printed like the CLI does");
    const c = await call(`${BASE}/cat/demo/repo?path=src/lib.rs&lines=900-950`, d);
    assert.equal(c.status, 416);
    assert.match(c.text, /outside src\/lib\.rs \(7 lines\)/);
    const bad = await call(`${BASE}/cat/demo/repo?path=src/lib.rs&lines=9-2`, d);
    assert.equal(bad.status, 400);
    assert.match(bad.text, /before start/);
  });

  it("cat as JSON carries provenance, blob sha, and line bounds", async () => {
    const d = await deps();
    const { status, json } = await call(`${BASE}/cat/demo/repo?path=src/lib.rs&lines=5-6&format=json`, d);
    assert.equal(status, 200);
    assert.equal(json.verb, "cat");
    assert.equal(json.repo, "demo/repo");
    assert.equal(json.commit, COMMIT_SHA);
    assert.equal(json.ref, "refs/heads/main");
    assert.equal(json.requested_ref, "main");
    assert.equal(json.blob_sha, "blob-src-lib-rs");
    assert.deepEqual(
      { start: json.start_line, end: json.end_line, total: json.total_lines },
      { start: 5, end: 6, total: 7 },
    );
    assert.equal(json.text, "pub fn answer() -> u8 {\n    42");
  });

  it("Accept: application/json selects JSON like ?format=json", async () => {
    const d = await deps();
    const viaHeader = await call(`${BASE}/ls/demo/repo?path=src`, d, {
      headers: { Accept: "application/json" },
    });
    const viaQuery = await call(`${BASE}/ls/demo/repo?path=src&format=json`, d);
    assert.equal(viaHeader.status, 200);
    assert.equal(viaHeader.json.cache, "miss");
    assert.equal(viaQuery.json.cache, "hit", "same cache, second open is warm");
    delete viaHeader.json.cache;
    delete viaQuery.json.cache;
    assert.deepEqual(viaHeader.json, viaQuery.json);
    assert.deepEqual(
      viaQuery.json.entries.map((e) => [e.name, e.kind]),
      [["util", "dir"], ["lib.rs", "file"], ["main.rs", "file"]],
    );
    assert.equal(viaQuery.json.entries[1].tokens_est, Math.ceil(viaQuery.json.entries[1].size_bytes / 4));
  });

  it("head and tail mirror the CLI helpers, with ?plus= for tail", async () => {
    const d = await deps();
    const h = await call(`${BASE}/head/demo/repo?path=src/lib.rs&lines=2&n=1`, d);
    assert.equal(h.text, "     1  //! demo lib\n     2  \n");
    const t = await call(`${BASE}/tail/demo/repo?path=src/lib.rs&lines=2`, d);
    assert.equal(t.text, "    42\n}\n");
    const p = await call(`${BASE}/tail/demo/repo?path=src/lib.rs&plus=6&n=1`, d);
    assert.equal(p.text, "     6      42\n     7  }\n");
    const j = await call(`${BASE}/tail/demo/repo?path=src/lib.rs&plus=6&format=json`, d);
    assert.deepEqual([j.json.start_line, j.json.end_line, j.json.total_lines], [6, 7, 7]);
    const def = await call(`${BASE}/head/demo/repo?path=src/lib.rs`, d);
    assert.equal(def.text.split("\n").length - 1, 7, "default head is 10 lines, file has 7");
  });

  it("file verbs require ?path= and distinguish directories from missing files", async () => {
    const d = await deps();
    for (const verb of ["cat", "head", "tail", "outline"]) {
      const r = await call(`${BASE}/${verb}/demo/repo`, d);
      assert.equal(r.status, 400, verb);
      assert.match(r.text, new RegExp(`${verb} requires \\?path=`));
    }
    const dir = await call(`${BASE}/cat/demo/repo?path=src`, d);
    assert.equal(dir.status, 400);
    assert.match(dir.text, /Not a file: src/);
    const missing = await call(`${BASE}/cat/demo/repo?path=nope.txt`, d);
    assert.equal(missing.status, 404);
    assert.match(missing.text, /File not found: nope\.txt/);
  });

  it("outline lists symbols with approximate line ranges", async () => {
    const d = await deps();
    const { status, text, json } = await call(`${BASE}/outline/demo/repo?path=src/main.rs&format=json`, d);
    assert.equal(status, 200);
    assert.equal(json.language, "Rust");
    assert.deepEqual(
      json.symbols.map((s) => [s.kind, s.name, s.line, s.end_line]),
      [
        ["struct", "Widget", 3, 6],
        ["impl", "Widget", 7, 16],
        ["fn", "new", 8, 11],
        ["fn", "render", 12, 16],
        ["fn", "main", 17, 20],
      ],
    );
    assert.equal(json.total_lines, 20);
    assert.ok(text.startsWith("{"));
    const plain = await call(`${BASE}/outline/demo/repo?path=src/main.rs`, d);
    assert.match(plain.text, /^src\/main\.rs \(Rust, 20 lines\)\n/);
    assert.match(plain.text, /\n   3-6 {3}struct Widget\n/);
    assert.match(plain.text, /\n   8-11 {6}fn new\n/);
  });

  it("outline handles python, markdown, and unsupported files honestly", async () => {
    const d = await deps();
    const py = await call(`${BASE}/outline/demo/repo?path=scripts/run.py&format=json`, d);
    assert.deepEqual(
      py.json.symbols.map((s) => `${s.kind} ${s.name}@${s.line}`),
      ["class Runner@4", "def __init__@5", "def run@8", "def main@12"],
    );
    const md = await call(`${BASE}/outline/demo/repo?path=README.md&format=json`, d);
    assert.deepEqual(
      md.json.symbols.map((s) => [s.name, s.line, s.end_line]),
      [["demo", 1, 11], ["Usage", 5, 8], ["License", 9, 11]],
    );
    const toml = await call(`${BASE}/outline/demo/repo?path=Cargo.toml`, d);
    assert.match(toml.text, /section package/);
    assert.match(toml.text, /section dependencies/);
    const png = await call(`${BASE}/outline/demo/repo?path=assets/logo.png`, d);
    assert.equal(png.status, 415, "binary file is refused by the wasm read");
  });

  it("stats summarises the tree with zero blob fetches", async () => {
    const d = await deps();
    const { status, text, json } = await call(`${BASE}/stats/demo/repo?format=json`, d);
    assert.equal(status, 200);
    assert.equal(gh.rawCalls().length, 0);
    assert.equal(gh.apiCalls().filter((c) => c.url.includes("/git/blobs/")).length, 0);
    assert.equal(json.files, 7);
    assert.equal(json.binary_files, 1);
    assert.equal(json.tokens_est, Math.ceil(json.bytes / 4));
    assert.deepEqual(json.directories.map((x) => x.name).sort(), [".", "assets/", "scripts/", "src/"]);
    const rust = json.languages.find((l) => l.language === "Rust");
    assert.equal(rust.files, 3);
    assert.equal(json.largest_files[0].path, "src/main.rs");
    assert.equal(json.max_depth, 2);
    assert.ok(text.startsWith("{"));

    const plain = await call(`${BASE}/stats/demo/repo?path=src&largest=2`, d);
    assert.match(plain.text, /^demo\/repo @ 0123456 \(refs\/heads\/main\)\npath: src\nfiles: 3 /);
    assert.match(plain.text, /by directory:\n {2}\. +2 files .*\n {2}util\/ +1 files /);
    assert.match(plain.text, /largest files:\n {2}src\/main\.rs/);
    assert.equal((plain.text.match(/~\d+ tok/g) || []).length > 3, true);
    const missing = await call(`${BASE}/stats/demo/repo?path=nope`, d);
    assert.equal(missing.status, 404);
  });

  it("tree/ls ?l=1 append token estimates and ?ignore= filters paths", async () => {
    const d = await deps();
    const tree = await call(`${BASE}/tree/demo/repo?path=src&l=1`, d);
    assert.equal(tree.text, "src\n  lib.rs (62 B, ~16 tok)\n  main.rs (315 B, ~79 tok)\n  util/mod.rs (58 B, ~15 tok)\n");
    const ls = await call(`${BASE}/ls/demo/repo?l=1&ignore=assets,scripts/**`, d);
    assert.match(ls.text, /Cargo\.toml {2}\(~\d+ tok\)/);
    assert.equal(ls.text.includes("assets/"), false);
    assert.equal(ls.text.includes("scripts/"), false);
    const ignoredCat = await call(`${BASE}/cat/demo/repo?path=README.md&ignore=*.md`, d);
    assert.equal(ignoredCat.status, 404);
    assert.match(ignoredCat.text, /excluded by \?ignore=/);
  });

  it("rg returns path:line:text, honours i/l/c/glob/path, and skips binaries", async () => {
    const d = await deps();
    const todo = await call(`${BASE}/rg/demo/repo?q=todo&i=1`, d);
    assert.equal(todo.status, 200);
    assert.equal(
      todo.text,
      "scripts/run.py:13:    print(Runner('x').run())  # TODO\nsrc/util/mod.rs:2:    \"TODO: implement\"\n",
    );
    const files = await call(`${BASE}/rg/demo/repo?q=TODO&l=1`, d);
    assert.equal(files.text, "scripts/run.py\nsrc/util/mod.rs\n");
    const long = await call(`${BASE}/rg/demo/repo?q=TODO&l=1&long=1`, d);
    assert.equal(long.text, "scripts/run.py (13 ln, ~65 tok)\nsrc/util/mod.rs (3 ln, ~15 tok)\n");
    const counts = await call(`${BASE}/rg/demo/repo?q=fn&c=1&glob=*.rs`, d);
    assert.equal(counts.text, "src/lib.rs:1\nsrc/main.rs:3\nsrc/util/mod.rs:1\n");
    const scoped = await call(`${BASE}/rg/demo/repo?q=fn&path=src/util&format=json`, d);
    assert.deepEqual(scoped.json.matches.map((m) => m.path), ["src/util/mod.rs"]);
    assert.equal(scoped.json.files_candidate, 1);
    assert.equal(scoped.json.truncated, false);
    const all = await call(`${BASE}/rg/demo/repo?q=.&format=json`, d);
    assert.equal(all.json.files_skipped_binary, 1, "png is skipped, not an error");
    assert.equal(all.json.files_scanned, 6);
  });

  it("rg context output matches the CLI shape (path-line-text, --, blank between files)", async () => {
    const d = await deps();
    const { text } = await call(`${BASE}/rg/demo/repo?q=fn%20(new|main)&glob=*.rs&C=1`, d);
    assert.equal(
      text,
      [
        "src/main.rs-7-impl Widget {",
        "src/main.rs:8:    pub fn new(name: &str) -> Self {",
        "src/main.rs-9-        Self { name: name.to_string() }",
        "--",
        "src/main.rs-16-",
        "src/main.rs:17:pub fn main() {",
        "src/main.rs-18-    let w = Widget::new(\"hello\");",
        "",
      ].join("\n"),
    );
  });

  it("rg reports truncation by max= and max_files= and validates patterns", async () => {
    const d = await deps();
    const capped = await call(`${BASE}/rg/demo/repo?q=fn&max=2&format=json`, d);
    assert.equal(capped.json.match_count, 2);
    assert.equal(capped.json.truncated, true);
    assert.equal(capped.json.truncated_reason, "max_matches");
    const cappedText = await call(`${BASE}/rg/demo/repo?q=fn&max=2`, d);
    assert.match(cappedText.text, /\n# truncated: reached max=2 matches\n$/);
    const fewFiles = await call(`${BASE}/rg/demo/repo?q=zzz-no-match&max_files=2`, d);
    assert.match(fewFiles.text, /^# truncated: scanned 2 of 7 candidate files/);
    const bad = await call(`${BASE}/rg/demo/repo?q=(unclosed`, d);
    assert.equal(bad.status, 400);
    assert.match(bad.text, /invalid rg pattern/);
    const none = await call(`${BASE}/rg/demo/repo`, d);
    assert.equal(none.status, 400);
    assert.match(none.text, /rg requires \?q=/);
  });

  it("rg -w and -v behave like ripgrep", async () => {
    const d = await deps();
    const word = await call(`${BASE}/rg/demo/repo?q=name&w=1&glob=main.rs&c=1`, d);
    assert.equal(word.text, "src/main.rs:4\n");
    const invert = await call(`${BASE}/rg/demo/repo?q=.&v=1&glob=src/lib.rs`, d);
    assert.equal(invert.text, "src/lib.rs:2:\nsrc/lib.rs:4:\n");
  });

  it("blobs come from raw.githubusercontent.com first and the REST blob endpoint as fallback", async () => {
    const withRaw = await deps();
    await call(`${BASE}/cat/demo/repo?path=README.md`, withRaw);
    assert.equal(gh.rawCalls().length, 1);
    assert.equal(gh.apiCalls().filter((c) => c.url.includes("/git/blobs/")).length, 0);

    gh.reset();
    const noRaw = await deps({ rawBlobs: false });
    const r = await call(`${BASE}/cat/demo/repo?path=README.md`, noRaw);
    assert.equal(r.status, 200);
    assert.equal(gh.rawCalls().length, 0);
    assert.equal(gh.apiCalls().filter((c) => c.url.includes("/git/blobs/")).length, 1);

    gh.reset();
    const rawDown = fixtureGitHub({ files: DEMO_FILES, raw: false });
    rawDown.install();
    try {
      const fallback = await call(`${BASE}/cat/demo/repo?path=README.md`, await deps());
      assert.equal(fallback.status, 200);
      assert.equal(rawDown.rawCalls().length, 1);
      assert.equal(rawDown.apiCalls().filter((c) => c.url.includes("/git/blobs/")).length, 1);
    } finally {
      rawDown.restore();
      gh.install();
    }
  });

  it("refs lists the default branch, branches, and tags", async () => {
    const d = await deps();
    const { status, text, json } = await call(`${BASE}/refs/demo/repo?format=json`, d);
    assert.equal(status, 200);
    assert.equal(json.default_branch, "main");
    assert.deepEqual(json.branches.map((b) => b.name), ["main", "dev"]);
    assert.deepEqual(json.tags.map((t) => t.name), ["v0.1.0"]);
    assert.ok(text.startsWith("{"));
    const plain = await call(`${BASE}/refs/demo/repo`, d);
    assert.equal(
      plain.text,
      `* branch main    ${COMMIT_SHA.slice(0, 12)}\n  branch dev     ${COMMIT_SHA.slice(0, 12)}\n  tag    v0.1.0  ${COMMIT_SHA.slice(0, 12)}\n`,
    );
  });

  it("commits lists recent history, optionally for one path, one REST call", async () => {
    const d = await deps();
    const all = await call(`${BASE}/commits/demo/repo?n=5`, d);
    assert.equal(all.status, 200);
    assert.equal(all.text, "aaaaaaa  2026-08-01  Ada  add util\nbbbbbbb  2026-07-01  Bob  initial import\n");
    assert.equal(gh.apiCalls().length, 1);
    const scoped = await call(`${BASE}/commits/demo/repo?path=README.md&format=json`, d);
    assert.deepEqual(scoped.json.commits.map((c) => c.author), ["Bob"]);
    assert.equal(scoped.json.path, "README.md");
  });

  it("search composes the query like the CLI and renders stars + descriptions", async () => {
    const d = await deps();
    const { status, text, json } = await call(`${BASE}/search?p=ratatui&lang=rust&limit=5&format=json`, d);
    assert.equal(status, 200);
    assert.equal(json.query, "ratatui in:name language:rust");
    assert.equal(json.items[0].full_name, "ratatui/ratatui");
    assert.equal(json.items[0].stars, 12000);
    const url = new URL(gh.apiCalls()[0].url);
    assert.equal(url.searchParams.get("sort"), "stars");
    assert.equal(url.searchParams.get("per_page"), "5");
    assert.ok(text.startsWith("{"));

    const plain = await call(`${BASE}/api/search?q=terminal+ui&sort=best`, d);
    assert.match(plain.text, /Found 2 repositories:/);
    assert.match(plain.text, /1\. ratatui\/ratatui {2}12000 stars {2}\[Rust\]\n {7}A Rust crate/);
    assert.equal(new URL(gh.apiCalls()[1].url).searchParams.has("sort"), false);

    const empty = await call(`${BASE}/search`, d);
    assert.equal(empty.status, 400);
    assert.match(empty.text, /search requires \?q=/);
    const badSort = await call(`${BASE}/search?q=x&sort=oldest`, d);
    assert.equal(badSort.status, 400);
  });

  it("?ref= accepts a full commit SHA and a branch; ?fresh=1 re-resolves the pin", async () => {
    const d = await deps();
    const first = await call(`${BASE}/ls/demo/repo`, d);
    assert.equal(first.headers.get("x-wit-cache"), "miss");
    const bySha = await call(`${BASE}/ls/demo/repo?ref=${COMMIT_SHA}`, d);
    assert.equal(bySha.status, 200);
    assert.equal(bySha.headers.get("x-wit-ref"), COMMIT_SHA);
    const dev = await call(`${BASE}/ls/demo/repo?branch=dev&format=json`, d);
    assert.equal(dev.status, 200);
    assert.equal(dev.json.ref, "refs/heads/dev");
    assert.equal(dev.json.requested_ref, "dev");
    assert.equal(d.cache.findOpenEntry("demo/repo", "dev").defaultBranch, "main", "a branch open never rewrites the default branch");

    gh.reset();
    const warm = await call(`${BASE}/ls/demo/repo`, d);
    assert.equal(warm.headers.get("x-wit-cache"), "hit", "second open on the same cache is served from memory");
    assert.equal(gh.apiCalls().length, 0);

    gh.reset();
    const fresh = await call(`${BASE}/ls/demo/repo?fresh=1`, d);
    assert.equal(fresh.status, 200);
    assert.equal(fresh.headers.get("x-wit-cache"), "miss");
    assert.equal(gh.apiCalls().length, 3, "repo + commit + tree re-fetched");
    await call(`${BASE}/cat/demo/repo?path=README.md`, d);
    gh.reset();
    const again = await call(`${BASE}/cat/demo/repo?path=README.md&fresh=1`, d);
    assert.equal(again.status, 200);
    assert.equal(gh.rawCalls().length, 0, "blobs survive a fresh re-resolve (content addressed)");
    assert.equal(gh.apiCalls().length, 3, "only repo + commit + tree were re-fetched");
  });

  it("host token is used when the caller sends none, and never overrides a caller token", async () => {
    const d = await deps({ serverToken: "ghp_HOSTSECRET" });
    const anon = await call(`${BASE}/ls/demo/repo`, d);
    assert.equal(anon.headers.get("x-wit-auth"), "host");
    assert.ok(gh.apiCalls().every((c) => c.auth === "ghp_HOSTSECRET"));
    assert.equal(anon.headers.get("cache-control"), "public, max-age=60, stale-while-revalidate=600");

    gh.reset();
    const caller = await call(`${BASE}/ls/demo/repo?fresh=1`, d, {
      headers: { Authorization: "Bearer ghp_CALLER" },
    });
    assert.equal(caller.headers.get("x-wit-auth"), "caller");
    assert.ok(gh.apiCalls().every((c) => c.auth === "ghp_CALLER"));
    assert.equal(caller.headers.get("cache-control"), "private, no-store");

    gh.reset();
    gh.force({ status: 500, body: "boom ghp_HOSTSECRET leaked?" });
    const err = await call(`${BASE}/ls/demo/repo?fresh=1`, d);
    assert.equal(err.status, 502);
    assert.equal(err.text.includes("ghp_HOSTSECRET"), false, "host token is scrubbed from errors");
  });

  it("GitHub rate limits become 429 with retry-after and an actionable message", async () => {
    const d = await deps();
    const reset = Math.floor(Date.now() / 1000) + 90;
    gh.force(
      {
        status: 403,
        body: JSON.stringify({ message: "API rate limit exceeded for 1.2.3.4." }),
        headers: { "x-ratelimit-remaining": "0", "x-ratelimit-reset": String(reset) },
      },
      (url) => url.includes("/repos/"),
    );
    const anon = await call(`${BASE}/tree/demo/repo`, d);
    assert.equal(anon.status, 429);
    assert.match(anon.text, /^error: GitHub API rate limit exceeded \(resets in \d+s\)\. This host has no GitHub credential configured; send your own/);
    assert.equal(anon.text.includes("[REDACTED]"), false);
    const retry = Number(anon.headers.get("retry-after"));
    assert.ok(retry >= 85 && retry <= 91, `retry-after ${retry}`);

    const withHost = await deps({ serverToken: "ghp_HOST" });
    const host = await call(`${BASE}/tree/demo/repo?format=json`, withHost);
    assert.equal(host.status, 429);
    assert.equal(host.json.code, "rate_limited");
    assert.match(host.json.error, /host credential's quota is exhausted; send your own/);

    gh.force({ status: 429, body: "slow down", headers: { "retry-after": "7" } });
    const secondary = await call(`${BASE}/tree/demo/repo`, d, { headers: { Authorization: "Bearer ghp_ME" } });
    assert.equal(secondary.status, 429);
    assert.equal(secondary.headers.get("retry-after"), "7");
    assert.match(secondary.text, /Your credential's GitHub quota is exhausted/);
  });

  it("a genuine 403 is still reported as access denied, and 401 as a bad token", async () => {
    const d = await deps();
    gh.force({ status: 403, body: JSON.stringify({ message: "Resource not accessible" }) });
    const denied = await call(`${BASE}/tree/demo/repo`, d);
    assert.equal(denied.status, 403);
    assert.match(denied.text, /denied access .*credentials with access may be required/);
    assert.equal(denied.text.includes("[REDACTED]"), false);
    gh.force({ status: 401, body: JSON.stringify({ message: "Bad credentials" }) });
    const bad = await call(`${BASE}/tree/demo/repo`, d, { headers: { Authorization: "Bearer nope" } });
    assert.equal(bad.status, 401);
    assert.match(bad.text, /rejected the supplied credentials \(HTTP 401\)/);
    assert.equal(bad.text.includes("[REDACTED]"), false);
  });

  it("rg stops on a mid-scan rate limit and reports partial results", async () => {
    const d = await deps({ blobConcurrency: 1 });
    let blobCalls = 0;
    gh.force(
      { status: 403, body: "rate limit", headers: { "x-ratelimit-remaining": "0" } },
      (url) => {
        if (!url.includes("raw.githubusercontent.com") && !url.includes("/git/blobs/")) return false;
        blobCalls += 1;
        return blobCalls > 4; // first two files (raw + api fallback each) succeed
      },
    );
    const { status, text, json } = await call(`${BASE}/rg/demo/repo?q=.&format=json`, d);
    assert.equal(status, 200);
    assert.equal(json.truncated, true);
    assert.equal(json.truncated_reason, "rate_limited");
    assert.ok(json.files_scanned >= 1 && json.files_scanned < 6);
    assert.ok(text.startsWith("{"));
    const plain = await call(`${BASE}/rg/demo/repo?q=.&fresh=1`, await deps({ blobConcurrency: 1 }));
    assert.match(plain.text, /# truncated: GitHub rate limit reached/);
  });

  it("per-request ?ttl= never leaks into the shared cache", async () => {
    const d = await deps();
    const before = d.cache.ttlMs;
    await call(`${BASE}/ls/demo/repo?ttl=5`, d);
    assert.equal(d.cache.ttlMs, before, "cache-wide TTL is untouched");
    const entry = d.cache.findOpenEntry("demo/repo");
    assert.equal(entry.ttlMs, 5000, "the entry this request created carries the override");
  });

  it("errors are JSON when JSON was requested", async () => {
    const d = await deps();
    const { status, json, headers } = await call(`${BASE}/cat/demo/repo?path=missing.rs&format=json`, d);
    assert.equal(status, 404);
    assert.match(headers.get("content-type"), /application\/json/);
    assert.deepEqual(json, { error: "File not found: missing.rs", code: "not_found", status: 404 });
  });

  it("CORS headers expose provenance to browser agents", async () => {
    const d = await deps();
    const { headers } = await call(`${BASE}/ls/demo/repo`, d);
    assert.equal(headers.get("access-control-allow-origin"), "*");
    assert.match(headers.get("access-control-expose-headers"), /x-wit-commit/);
  });
});

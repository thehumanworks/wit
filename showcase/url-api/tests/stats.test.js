import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { classifyPath, computeStats, formatStats } from "../lib/stats.js";

const ROWS = [
  { path: "README.md", type: "blob", size: 400 },
  { path: "Makefile", type: "blob", size: 100 },
  { path: "src", type: "tree" },
  { path: "src/main.rs", type: "blob", size: 4000 },
  { path: "src/lib.rs", type: "blob", size: 1000 },
  { path: "src/deep", type: "tree" },
  { path: "src/deep/x.rs", type: "blob", size: 10 },
  { path: "assets/logo.png", type: "blob", size: 90000 },
  { path: "scripts/run.py", type: "blob", size: 250 },
];

describe("classifyPath", () => {
  it("maps extensions and special names to languages and flags binaries", () => {
    assert.deepEqual(classifyPath("src/main.rs"), { name: "main.rs", ext: "rs", language: "Rust", binary: false });
    assert.equal(classifyPath("Makefile").language, "Makefile");
    assert.equal(classifyPath("Cargo.lock").language, "Lockfile");
    assert.equal(classifyPath("a/b.PNG").binary, true);
    assert.equal(classifyPath("weird.xyz").language, "xyz");
    assert.equal(classifyPath("LICENSE").language, "Text");
    assert.equal(classifyPath("noext").language, "(no extension)");
  });
});

describe("computeStats", () => {
  it("aggregates the whole tree", () => {
    const s = computeStats(ROWS, "", { largest: 3 });
    assert.equal(s.path, ".");
    assert.equal(s.files, 7);
    assert.equal(s.bytes, 95760);
    assert.equal(s.tokens_est, Math.ceil(95760 / 4));
    assert.equal(s.binary_files, 1);
    assert.equal(s.max_depth, 2);
    assert.deepEqual(s.directories.map((d) => d.name), ["assets/", "src/", ".", "scripts/"]);
    assert.deepEqual(s.directories[2], { name: ".", files: 2, bytes: 500, tokens_est: 125 });
    assert.equal(s.languages[0].language, "Markdown" === s.languages[0].language ? "Markdown" : s.languages[0].language);
    assert.deepEqual(s.languages.find((l) => l.language === "Rust"), { language: "Rust", files: 3, bytes: 5010, tokens_est: 1253 });
    assert.deepEqual(s.largest_files.map((f) => f.path), ["assets/logo.png", "src/main.rs", "src/lib.rs"]);
    assert.equal(s.largest_files[0].binary, true);
  });

  it("scopes to a subtree and honours ignore", () => {
    const s = computeStats(ROWS, "src", { isIgnored: (p) => p.endsWith("x.rs") });
    assert.equal(s.path, "src");
    assert.equal(s.files, 2);
    assert.deepEqual(s.directories.map((d) => d.name), ["."]);
    assert.equal(s.max_depth, 1);
    const deep = computeStats(ROWS, "src");
    assert.deepEqual(deep.directories.map((d) => d.name), [".", "deep/"]);
  });

  it("handles an empty subtree", () => {
    const s = computeStats(ROWS, "nothing");
    assert.equal(s.files, 0);
    assert.deepEqual(s.largest_files, []);
    assert.match(formatStats(s, { repo: "o/r", ref: "refs/heads/main", commit: "abcdef0123" }), /files: 0 /);
  });
});

describe("formatStats", () => {
  it("renders aligned plaintext with tokens everywhere", () => {
    const text = formatStats(computeStats(ROWS, "", { largest: 2 }), {
      repo: "o/r",
      ref: "refs/heads/main",
      commit: "abcdef0123456789",
    });
    const lines = text.split("\n");
    assert.equal(lines[0], "o/r @ abcdef0 (refs/heads/main)");
    assert.equal(lines[1], "path: .");
    assert.match(lines[2], /^files: 7 {2}bytes: 93\.5 KB {2}~24k tok {2}binary: 1 {2}max depth: 2$/);
    assert.ok(lines.includes("by directory:"));
    assert.ok(lines.includes("by language:"));
    assert.ok(lines.includes("largest files:"));
    assert.match(text, /assets\/logo\.png .*\[bin\]$/m);
    assert.equal(lines.filter((l) => l.startsWith("  ")).length, 4 + 5 + 2);
  });
});

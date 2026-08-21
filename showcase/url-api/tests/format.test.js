/**
 * Plaintext formatters matching CLI memory stdout.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { formatCat, formatLs, formatTree } from "../lib/format.js";

describe("formatLs", () => {
  it("prints dir/ and file names", () => {
    const text = formatLs([
      { name: "src", kind: "dir", path: "src" },
      { name: "README", kind: "file", path: "README", size_bytes: 12 },
    ]);
    assert.equal(text, "src/\nREADME");
  });

  it("long mode shows sizes", () => {
    const text = formatLs(
      [{ name: "README", kind: "file", path: "README", size_bytes: 12 }],
      { long: true },
    );
    assert.match(text, /12 B\s+README/);
  });
});

describe("formatTree", () => {
  it("matches CLI memory tree shape", () => {
    const text = formatTree({
      root: ".",
      entries: [
        { path: "README", kind: "file", size_bytes: 13 },
        { path: "src/main.rs", kind: "file", size_bytes: 40 },
      ],
    });
    assert.equal(text, ".\n  README\n  src/main.rs");
  });

  it("honors depth", () => {
    const text = formatTree(
      {
        root: ".",
        entries: [
          { path: "README", kind: "file" },
          { path: "src/main.rs", kind: "file" },
        ],
      },
      { depth: 1 },
    );
    assert.equal(text, ".\n  README");
  });
});

describe("formatCat", () => {
  it("prints file text", () => {
    assert.equal(formatCat("hello\nworld\n"), "hello\nworld");
  });

  it("numbers lines with ?n=", () => {
    assert.equal(formatCat("a\nb\n", { number: true }), "     1  a\n     2  b");
  });
});

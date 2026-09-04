import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  compileSearchRegex,
  estimateTokens,
  globToRegExp,
  grepText,
  headFromText,
  humanBytes,
  humanTokens,
  parseLineRange,
  rustLines,
  sliceLines,
  tailFromText,
} from "../lib/textops.js";

describe("rustLines", () => {
  it("matches Rust str::lines()", () => {
    assert.deepEqual(rustLines(""), []);
    assert.deepEqual(rustLines("a\nb\n"), ["a", "b"]);
    assert.deepEqual(rustLines("a\r\nb"), ["a", "b"]);
    assert.deepEqual(rustLines("\n"), [""]);
  });
});

describe("parseLineRange", () => {
  it("parses A-B, A-, -B, A, and A:B", () => {
    assert.equal(parseLineRange(null), null);
    assert.equal(parseLineRange(""), null);
    assert.deepEqual(parseLineRange("10-20"), { start: 10, end: 20 });
    assert.deepEqual(parseLineRange("10-"), { start: 10, end: null });
    assert.deepEqual(parseLineRange("-20"), { start: null, end: 20 });
    assert.deepEqual(parseLineRange("7"), { start: 7, end: 7 });
    assert.deepEqual(parseLineRange("3:9"), { start: 3, end: 9 });
  });

  it("rejects malformed, zero, and reversed ranges", () => {
    assert.throws(() => parseLineRange("abc"), /START-END/);
    assert.throws(() => parseLineRange("-"), /START-END/);
    assert.throws(() => parseLineRange("0-5"), /one-based/);
    assert.throws(() => parseLineRange("9-2"), /before start/);
  });
});

describe("sliceLines", () => {
  const text = "l1\nl2\nl3\nl4\n";
  it("clamps to the file and reports bounds", () => {
    assert.deepEqual(sliceLines(text, null), { lines: ["l1", "l2", "l3", "l4"], start: 1, end: 4, total: 4 });
    assert.deepEqual(sliceLines(text, { start: 2, end: 3 }), { lines: ["l2", "l3"], start: 2, end: 3, total: 4 });
    assert.deepEqual(sliceLines(text, { start: 3, end: 99 }), { lines: ["l3", "l4"], start: 3, end: 4, total: 4 });
    assert.deepEqual(sliceLines(text, { start: 9, end: 12 }).lines, []);
    assert.deepEqual(sliceLines("", { start: 1, end: 2 }), { lines: [], start: 1, end: 0, total: 0 });
  });
});

describe("head/tail", () => {
  const text = "a\nb\nc\nd\n";
  it("mirror memory_ops.rs", () => {
    assert.equal(headFromText(text, 2, false), "a\nb");
    assert.equal(headFromText(text, 2, true), "     1  a\n     2  b");
    assert.equal(tailFromText(text, 2, null, false), "c\nd");
    assert.equal(tailFromText(text, 2, null, true), "     3  c\n     4  d");
    assert.equal(tailFromText(text, 10, 2, false), "b\nc\nd");
    assert.equal(tailFromText(text, 10, 99, false), "");
  });
});

describe("compileSearchRegex", () => {
  it("applies i/S/w and rejects empty or invalid patterns", () => {
    assert.equal(compileSearchRegex("abc", { ignoreCase: true }).flags, "i");
    assert.equal(compileSearchRegex("abc", { smartCase: true }).flags, "i");
    assert.equal(compileSearchRegex("Abc", { smartCase: true }).flags, "");
    assert.equal(compileSearchRegex("foo|bar", { wordRegexp: true }).source, "\\b(?:foo|bar)\\b");
    assert.throws(() => compileSearchRegex(""), /non-empty/);
    assert.throws(() => compileSearchRegex("("), /invalid rg pattern/);
  });
});

describe("grepText", () => {
  const text = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
  it("returns matches without context", () => {
    const out = grepText("f", text, /e/);
    assert.deepEqual(out.lines.map((l) => l.line), [1, 3, 5, 7]);
    assert.equal(out.matchCount, 4);
    assert.ok(out.lines.every((l) => l.is_context === false));
  });

  it("emits context lines and -- separators without duplicates", () => {
    const out = grepText("f", text, /^(one|seven)$/, { before: 1, after: 1 });
    assert.deepEqual(
      out.lines.map((l) => (l.line === 0 ? l.text : `${l.line}:${l.is_context ? "c" : "m"}:${l.text}`)),
      ["1:m:one", "2:c:two", "--", "6:c:six", "7:m:seven"],
    );
    const overlap = grepText("f", text, /^(two|three)$/, { before: 1, after: 1 });
    assert.deepEqual(overlap.lines.map((l) => l.line), [1, 2, 3, 4]);
  });

  it("honours invert and maxMatches", () => {
    assert.equal(grepText("f", text, /e/, { invert: true }).matchCount, 3);
    assert.equal(grepText("f", text, /e/, { maxMatches: 2 }).matchCount, 2);
  });
});

describe("globToRegExp", () => {
  it("matches basenames for slash-less globs and full paths otherwise", () => {
    const rs = globToRegExp("*.rs");
    assert.ok(rs.test("src/main.rs"));
    assert.ok(rs.test("main.rs"));
    assert.equal(rs.test("src/main.rsx"), false);
    const nested = globToRegExp("src/**/*.ts");
    assert.ok(nested.test("src/a/b/c.ts"));
    assert.ok(nested.test("src/c.ts"));
    assert.equal(nested.test("lib/c.ts"), false);
    const alt = globToRegExp("*.{js,mjs}");
    assert.ok(alt.test("x/y.mjs"));
    assert.equal(alt.test("x/y.cjs"), false);
    assert.ok(globToRegExp("docs/**").test("docs/adr/0001.md"));
    assert.ok(globToRegExp("?.txt").test("a.txt"));
    assert.equal(globToRegExp("?.txt").test("ab.txt"), false);
    assert.equal(globToRegExp(""), null);
  });
});

describe("human labels", () => {
  it("format bytes and tokens compactly", () => {
    assert.equal(estimateTokens(0), 0);
    assert.equal(estimateTokens(10), 3);
    assert.equal(humanBytes(512), "512 B");
    assert.equal(humanBytes(2048), "2.0 KB");
    assert.equal(humanBytes(3 * 1024 * 1024), "3.0 MB");
    assert.equal(humanTokens(999), "~999 tok");
    assert.equal(humanTokens(1500), "~1.5k tok");
    assert.equal(humanTokens(35000), "~35k tok");
    assert.equal(humanTokens(2_100_000), "~2.1M tok");
  });
});

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  formatCat,
  formatLs,
  formatRg,
  formatSearch,
  formatSed,
  formatTree,
  headFromText,
  tailFromText,
} from "./format.js";

test("formatTree matches memory CLI plaintext for demo/repo", () => {
  const text = formatTree(null, [
    { path: "src/main.rs", size_bytes: 20 },
    { path: "README.md", size_bytes: 14 },
  ]);
  assert.equal(text, ".\n  README.md\n  src/main.rs");
});

test("formatTree strips a subtree prefix like print_snapshot_tree", () => {
  const text = formatTree("src", [{ path: "src/main.rs", size_bytes: 20 }]);
  assert.equal(text, "src\n  main.rs");
});

test("formatLs uses trailing slash for dirs", () => {
  const text = formatLs([
    { name: "src", kind: "dir" },
    { name: "README.md", kind: "file" },
  ]);
  assert.equal(text, "src/\nREADME.md");
});

test("formatLs empty directory uses the CLI sentence", () => {
  assert.equal(formatLs([]), "Directory is empty or does not exist.");
});

test("formatCat prints file text only", () => {
  assert.equal(formatCat({ text: "Hello, memory!" }), "Hello, memory!");
  assert.equal(formatCat("fn main() {}\n"), "fn main() {}\n");
});

test("headFromText and tailFromText match memory_ops.rs", () => {
  const text = "a\nb\nc\nd\n";
  assert.equal(headFromText(text, 2, false), "a\nb");
  assert.equal(headFromText(text, 2, true), "     1  a\n     2  b");
  assert.equal(tailFromText(text, 2, null, false), "c\nd");
  assert.equal(tailFromText(text, 2, null, true), "     3  c\n     4  d");
  assert.equal(tailFromText(text, 10, 2, false), "b\nc\nd");
  assert.equal(headFromText("Hello, memory!", 10, false), "Hello, memory!");
});

test("formatSed print-range and /re/p match native plaintext", () => {
  const input = "alpha\nbeta\ngamma\ndelta\n";
  assert.equal(formatSed(input, "2,3p", { quiet: true }), "beta\ngamma\n");
  assert.equal(formatSed(input, "/beta/p", { quiet: true }), "beta\n");
  assert.equal(formatSed("Hello, memory!", "1,2p", { quiet: true }), "Hello, memory!\n");
  assert.equal(formatSed("Hello, memory!", "s/Hello/Hi/"), "Hi, memory!\n");
});

test("formatRg prints path:line:text or files-with-matches", () => {
  const matches = [{ path: "README.md", line: 1, text: "Hello, memory!" }];
  assert.equal(formatRg(matches), "README.md:1:Hello, memory!");
  assert.equal(formatRg(matches, { filesWithMatches: true }), "README.md");
});

test("formatSearch prints stars and repo names", () => {
  const text = formatSearch({
    items: [
      { full_name: "ratatui/ratatui", stargazers_count: 15000 },
      { full_name: "ratatui/ratatui-website", stargazers_count: 80 },
    ],
  });
  assert.match(text, /Found 2 repositories:/);
  assert.match(text, /ratatui\/ratatui/);
  assert.match(text, /15000 stars/);
});

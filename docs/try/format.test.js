import assert from "node:assert/strict";
import { test } from "node:test";
import { formatCat, formatLs, formatTree } from "./format.js";

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

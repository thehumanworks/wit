import assert from "node:assert/strict";
import { test } from "node:test";
import { parseCommand } from "./commands.js";

test("parses wit tree owner/repo", () => {
  assert.deepEqual(parseCommand("wit tree demo/repo"), {
    kind: "run",
    command: "tree",
    repo: "demo/repo",
    path: null,
  });
});

test("parses wit cat owner/repo PATH and -- / -r shapes", () => {
  assert.deepEqual(parseCommand("wit cat demo/repo README.md"), {
    kind: "run",
    command: "cat",
    repo: "demo/repo",
    path: "README.md",
  });
  assert.deepEqual(parseCommand("wit cat -r demo/repo README.md"), {
    kind: "run",
    command: "cat",
    repo: "demo/repo",
    path: "README.md",
  });
  assert.deepEqual(parseCommand("wit tree -- demo/repo src"), {
    kind: "run",
    command: "tree",
    repo: "demo/repo",
    path: "src",
  });
});

test("rejects rg/sed and unknown flags in the terminal", () => {
  const rg = parseCommand("wit rg TODO demo/repo");
  assert.equal(rg.kind, "error");
  assert.match(rg.message, /not available/);
  const flag = parseCommand("wit tree -l demo/repo");
  assert.equal(flag.kind, "error");
  assert.match(flag.message, /unknown flag/);
  const bare = parseCommand("tree demo/repo");
  assert.equal(bare.kind, "error");
  assert.match(bare.message, /bad command/);
});

test("missing repo and cat path print CLI-shaped errors", () => {
  const missing = parseCommand("wit tree");
  assert.equal(missing.kind, "error");
  assert.match(missing.message, /missing repository/);
  const cat = parseCommand("wit cat demo/repo");
  assert.equal(cat.kind, "error");
  assert.match(cat.message, /missing arguments/);
});

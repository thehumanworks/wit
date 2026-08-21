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

test("parses rg / sed / head / tail with the small native flag set", () => {
  assert.deepEqual(parseCommand("wit rg Hello demo/repo"), {
    kind: "run",
    command: "rg",
    repo: "demo/repo",
    path: null,
    pattern: "Hello",
    ignoreCase: false,
    filesWithMatches: false,
  });
  assert.deepEqual(parseCommand("wit rg -i -l Hello -r demo/repo src"), {
    kind: "run",
    command: "rg",
    repo: "demo/repo",
    path: "src",
    pattern: "Hello",
    ignoreCase: true,
    filesWithMatches: true,
  });
  assert.deepEqual(parseCommand("wit sed -n '1,2p' demo/repo README.md"), {
    kind: "run",
    command: "sed",
    repo: "demo/repo",
    path: "README.md",
    script: "1,2p",
    quiet: true,
  });
  assert.deepEqual(parseCommand("wit head -n 2 demo/repo README.md"), {
    kind: "run",
    command: "head",
    repo: "demo/repo",
    path: "README.md",
    lines: 2,
    number: false,
  });
  assert.deepEqual(parseCommand("wit tail -n 2 -p 1 demo/repo README.md"), {
    kind: "run",
    command: "tail",
    repo: "demo/repo",
    path: "README.md",
    lines: 2,
    number: false,
    fromLine: 1,
  });
});

test("parses wit search -p / -l", () => {
  assert.deepEqual(parseCommand("wit search -p ratatui"), {
    kind: "run",
    command: "search",
    pattern: "ratatui",
    lang: null,
  });
  assert.deepEqual(parseCommand("wit search -p ratatui -l Rust"), {
    kind: "run",
    command: "search",
    pattern: "ratatui",
    lang: "Rust",
  });
});

test("rejects skill/mcp/cache/branches and unknown flags", () => {
  for (const line of [
    "wit skill load",
    "wit mcp",
    "wit cache demo/repo",
    "wit branches demo/repo",
  ]) {
    const parsed = parseCommand(line);
    assert.equal(parsed.kind, "error", line);
    assert.match(parsed.message, /not available/, line);
  }
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

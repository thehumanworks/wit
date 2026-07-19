#!/usr/bin/env node

import { spawn } from "node:child_process";
import readline from "node:readline";

const binary = process.argv[2];
const prefixArgs = process.argv.slice(3);
if (!binary) {
  console.error("usage: node scripts/smoke_mcp_modes.mjs <binary> [mcp-prefix-args...]");
  process.exit(2);
}

const DIRECT_TOOLS = [
  "wit_context",
  "wit_find_repositories",
  "wit_list",
  "wit_open",
  "wit_read",
  "wit_refs",
  "wit_search_code",
];

function fail(message) {
  throw new Error(message);
}

async function listTools(mode) {
  const child = spawn(binary, [...prefixArgs, "--mode", mode], {
    shell: process.platform === "win32",
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });

  const responses = new Map();
  const waiters = new Map();
  const lines = readline.createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const message = JSON.parse(line);
    if (message.id === undefined) return;
    const waiter = waiters.get(String(message.id));
    if (waiter) {
      waiters.delete(String(message.id));
      waiter(message);
    } else {
      responses.set(String(message.id), message);
    }
  });

  const timeout = setTimeout(() => child.kill(), 15_000);
  const send = (message) => child.stdin.write(`${JSON.stringify(message)}\n`);
  const response = (id) =>
    new Promise((resolve, reject) => {
      const key = String(id);
      if (responses.has(key)) {
        const message = responses.get(key);
        responses.delete(key);
        resolve(message);
        return;
      }
      waiters.set(key, resolve);
      child.once("error", reject);
      child.once("exit", (code) =>
        reject(new Error(`${mode} server exited early (${code}): ${stderr}`)),
      );
    });

  try {
    send({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "wit-release-smoke", version: "1" },
      },
    });
    const initialized = await response(1);
    if (initialized.error) fail(`${mode} initialize failed: ${JSON.stringify(initialized.error)}`);
    send({ jsonrpc: "2.0", method: "notifications/initialized" });
    send({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} });
    const listed = await response(2);
    if (listed.error) fail(`${mode} tools/list failed: ${JSON.stringify(listed.error)}`);
    return listed.result.tools.map((tool) => tool.name).sort();
  } finally {
    clearTimeout(timeout);
    child.stdin.end();
    child.kill();
    lines.close();
  }
}

for (const [mode, expected] of [
  ["direct", DIRECT_TOOLS],
  ["code", ["code"]],
]) {
  const actual = await listTools(mode);
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    fail(`${mode} tools/list mismatch: expected ${expected.join(", ")}; got ${actual.join(", ")}`);
  }
}

console.log("direct and Code Mode tools/list smoke passed");

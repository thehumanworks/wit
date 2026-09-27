#!/usr/bin/env node

import { spawn } from "node:child_process";
import path from "node:path";
import readline from "node:readline";

const TIMEOUT_MS = 15_000;
// A request that trails `notifications/initialized` by a few tens of milliseconds
// is the pattern that stalled optimized servers writing via `tokio::io::Stdout`.
const POST_INITIALIZED_PAUSE_MS = 50;

const binary = process.argv[2];
const prefixArgs = process.argv.slice(3);
if (!binary) {
  console.error("usage: node scripts/smoke_mcp_modes.mjs <binary> [mcp-prefix-args...]");
  process.exit(2);
}

const DIRECT_TOOLS = [
  "wit_ast",
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

// npm installs bare commands as `.cmd` shims on Windows, which only a shell can
// launch. cmd.exe parses `/` in a relative path as a switch, so explicit paths
// are resolved and spawned directly instead.
function spawnTarget() {
  const bare = !/[\\/]/.test(binary);
  return {
    command: bare ? binary : path.resolve(binary),
    shell: process.platform === "win32" && bare,
  };
}

async function listTools(mode) {
  const { command, shell } = spawnTarget();
  const child = spawn(command, [...prefixArgs, "--mode", mode], {
    shell,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });
  // Writes after an early exit raise EPIPE; the exit handler reports the cause.
  child.stdin.on("error", () => {});

  const responses = new Map();
  const waiters = new Map();
  let exitError;
  const failWaiters = (error) => {
    exitError ??= error;
    for (const { reject } of waiters.values()) reject(exitError);
    waiters.clear();
  };
  child.once("error", failWaiters);
  child.once("exit", (code, signal) =>
    failWaiters(
      new Error(`${mode} server exited early (code ${code}, signal ${signal}): ${stderr}`),
    ),
  );

  const lines = readline.createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const message = JSON.parse(line);
    if (message.id === undefined) return;
    const waiter = waiters.get(String(message.id));
    if (waiter) {
      waiters.delete(String(message.id));
      waiter.resolve(message);
    } else {
      responses.set(String(message.id), message);
    }
  });

  const send = (message) => child.stdin.write(`${JSON.stringify(message)}\n`);
  const response = (id, method) =>
    new Promise((resolve, reject) => {
      const key = String(id);
      if (responses.has(key)) {
        const message = responses.get(key);
        responses.delete(key);
        resolve(message);
        return;
      }
      if (exitError) {
        reject(exitError);
        return;
      }
      const timer = setTimeout(() => {
        waiters.delete(key);
        reject(
          new Error(`${mode} server did not answer ${method} within ${TIMEOUT_MS} ms: ${stderr}`),
        );
      }, TIMEOUT_MS);
      waiters.set(key, {
        resolve: (message) => {
          clearTimeout(timer);
          resolve(message);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
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
    const initialized = await response(1, "initialize");
    if (initialized.error) fail(`${mode} initialize failed: ${JSON.stringify(initialized.error)}`);
    send({ jsonrpc: "2.0", method: "notifications/initialized" });
    await new Promise((resolve) => setTimeout(resolve, POST_INITIALIZED_PAUSE_MS));
    send({ jsonrpc: "2.0", id: 2, method: "tools/list", params: {} });
    const listed = await response(2, "tools/list");
    if (listed.error) fail(`${mode} tools/list failed: ${JSON.stringify(listed.error)}`);
    return listed.result.tools.map((tool) => tool.name).sort();
  } finally {
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

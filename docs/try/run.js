import { parseCommand } from "./commands.js";
import { formatCat, formatLs, formatTree } from "./format.js";
import { listFilesRecursive, listPath, openRepo, readFile } from "./host.js";

/**
 * @param {WebAssembly.Exports} api
 * @param {string} line
 * @returns {{ kind: string, text?: string, error?: boolean }}
 */
export function runLine(api, line) {
  const parsed = parseCommand(line);
  if (parsed.kind === "empty") {
    return { kind: "empty" };
  }
  if (parsed.kind === "help") {
    return { kind: "help", text: parsed.message };
  }
  if (parsed.kind === "clear") {
    return { kind: "clear" };
  }
  if (parsed.kind === "error") {
    return { kind: "error", text: parsed.message, error: true };
  }
  try {
    const text = execute(api, parsed);
    return { kind: "ok", text };
  } catch (err) {
    return { kind: "error", text: String(err.message || err), error: true };
  }
}

/**
 * @param {WebAssembly.Exports} api
 * @param {{ command: string, repo: string, path?: string | null }} parsed
 */
export function execute(api, parsed) {
  openRepo(api, parsed.repo);
  if (parsed.command === "tree") {
    const files = listFilesRecursive(api, parsed.path);
    return formatTree(parsed.path, files);
  }
  if (parsed.command === "ls") {
    const entries = listPath(api, parsed.path);
    return formatLs(entries);
  }
  if (parsed.command === "cat") {
    const file = readFile(api, parsed.path);
    return formatCat(file);
  }
  throw new Error(`bad command: '${parsed.command}' is not available here`);
}

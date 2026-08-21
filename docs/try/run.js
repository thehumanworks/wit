import { parseCommand } from "./commands.js";
import {
  fileText,
  formatCat,
  formatLs,
  formatRg,
  formatSearch,
  formatSed,
  formatTree,
  headFromText,
  rustLines,
  tailFromText,
} from "./format.js";
import {
  buildSearchQuery,
  listFilesRecursive,
  listPath,
  openRepo,
  readFile,
  searchRepositories,
} from "./host.js";

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
  if (parsed.command === "search") {
    const query = buildSearchQuery(parsed.pattern, parsed.lang);
    const body = searchRepositories(api, query);
    return formatSearch(body);
  }
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
  if (parsed.command === "head") {
    const content = fileText(readFile(api, parsed.path));
    return headFromText(content, parsed.lines ?? 10, Boolean(parsed.number));
  }
  if (parsed.command === "tail") {
    const content = fileText(readFile(api, parsed.path));
    return tailFromText(
      content,
      parsed.lines ?? 10,
      parsed.fromLine ?? null,
      Boolean(parsed.number),
    );
  }
  if (parsed.command === "sed") {
    const content = fileText(readFile(api, parsed.path));
    return formatSed(content, parsed.script, { quiet: parsed.quiet });
  }
  if (parsed.command === "rg") {
    return executeRg(api, parsed);
  }
  throw new Error(`bad command: '${parsed.command}' is not available here`);
}

function executeRg(api, parsed) {
  let regex;
  try {
    regex = new RegExp(parsed.pattern, parsed.ignoreCase ? "i" : "");
  } catch (err) {
    throw new Error(`invalid rg pattern: ${err.message || err}`);
  }
  const files = listFilesRecursive(api, parsed.path);
  const matches = [];
  for (const file of files) {
    let content;
    try {
      content = fileText(readFile(api, file.path));
    } catch {
      continue;
    }
    if (content.includes("\0")) {
      continue;
    }
    const lines = rustLines(content);
    for (let i = 0; i < lines.length; i += 1) {
      regex.lastIndex = 0;
      if (!regex.test(lines[i])) {
        continue;
      }
      matches.push({ path: file.path, line: i + 1, text: lines[i] });
      if (parsed.filesWithMatches) {
        break;
      }
    }
  }
  return formatRg(matches, { filesWithMatches: parsed.filesWithMatches });
}

/**
 * Plaintext formatters matching `wit … --backend memory` stdout
 * (provenance stays on stderr in the CLI; HTTP returns stdout only).
 */

import { estimateTokens } from "./textops.js";

/**
 * @typedef {{ name: string, kind: 'file'|'dir', path: string, size_bytes?: number|null }} DirEntry
 * @typedef {{ path: string, kind: 'file'|'dir', size_bytes?: number|null }} TreeEntry
 */

/**
 * Format `wit ls` memory stdout.
 * @param {DirEntry[]} entries
 * @param {{ long?: boolean }} [opts]
 */
export function formatLs(entries, opts = {}) {
  if (!entries || entries.length === 0) {
    return "Directory is empty or does not exist.";
  }
  const long = !!opts.long;
  const lines = [];
  for (const entry of entries) {
    const isDir = entry.kind === "dir";
    if (long) {
      if (isDir) {
        lines.push(`            ${entry.name}/`);
      } else if (entry.size_bytes != null) {
        lines.push(
          `${String(entry.size_bytes).padStart(8)} B  ${entry.name}  (~${estimateTokens(entry.size_bytes)} tok)`,
        );
      } else {
        lines.push(`            ${entry.name}`);
      }
    } else if (isDir) {
      lines.push(`${entry.name}/`);
    } else {
      lines.push(entry.name);
    }
  }
  return lines.join("\n");
}

/**
 * Format `wit tree` memory stdout.
 * CLI prints root then `  {relative}` for each file entry (dirs implied).
 *
 * @param {{ root: string, entries: TreeEntry[] }} view
 * @param {{ path?: string, depth?: number|null, long?: boolean }} [opts]
 */
export function formatTree(view, opts = {}) {
  const base = (opts.path || "").replace(/\/+$/, "");
  const depth = opts.depth == null ? null : opts.depth;
  const long = !!opts.long;
  const lines = [view.root || (base ? base.split("/").pop() : ".")];

  for (const entry of view.entries) {
    if (entry.kind === "dir") continue;
    let relative;
    if (base) {
      if (entry.path === base) {
        relative = entry.path.split("/").pop() || entry.path;
      } else if (entry.path.startsWith(base + "/")) {
        relative = entry.path.slice(base.length + 1);
      } else {
        relative = entry.path;
      }
    } else {
      relative = entry.path;
    }
    if (!relative) continue;

    if (depth != null) {
      const parts = relative.split("/").filter(Boolean);
      if (parts.length > depth) continue;
    }

    const label =
      long && entry.size_bytes != null
        ? `${relative} (${entry.size_bytes} B, ~${estimateTokens(entry.size_bytes)} tok)`
        : relative;
    lines.push(`  ${label}`);
  }
  return lines.join("\n");
}

/**
 * Format `wit cat` stdout (optional line numbers via ?n=). When a line range
 * was applied, numbering starts at the first selected line.
 * @param {string | string[]} text full text or already-selected lines
 * @param {{ number?: boolean, startLine?: number }} [opts]
 */
export function formatCat(text, opts = {}) {
  let lines;
  if (Array.isArray(text)) {
    lines = text;
  } else {
    lines = text.split("\n");
    // If text ended with \n, split yields trailing ""; CLI's .lines() drops it.
    if (lines.length && lines[lines.length - 1] === "") lines.pop();
  }
  if (!opts.number) return lines.join("\n");
  const start = opts.startLine ?? 1;
  return lines
    .map((line, i) => `${String(start + i).padStart(6)}  ${line}`)
    .join("\n");
}

/**
 * Format `wit rg` stdout: `path:line:text` for matches, `path-line-text` for
 * context, `--` between non-adjacent groups, and a blank line between files
 * when context is on.
 *
 * @param {import('./textops.js').GrepLine[]} lines
 * @param {{ hasContext?: boolean }} [opts]
 */
export function formatRgMatches(lines, opts = {}) {
  if (lines.length === 0) return "";
  const out = [];
  let currentFile = "";
  for (const m of lines) {
    if (m.path !== currentFile) {
      if (currentFile !== "" && opts.hasContext) out.push("");
      currentFile = m.path;
    }
    if (m.line === 0 && m.text === "--") {
      out.push("--");
      continue;
    }
    out.push(m.is_context ? `${m.path}-${m.line}-${m.text}` : `${m.path}:${m.line}:${m.text}`);
  }
  return out.join("\n");
}

/**
 * `wit rg -l --long`: `path (N ln, ~T tok)`; plain `-l`: one path per line.
 * @param {Array<{ path: string, lines?: number | null }>} files
 * @param {{ long?: boolean }} [opts]
 */
export function formatRgFiles(files, opts = {}) {
  return files
    .map((f) =>
      opts.long && f.lines != null ? `${f.path} (${f.lines} ln, ~${f.lines * 5} tok)` : f.path,
    )
    .join("\n");
}

/**
 * `wit rg -c`: `path:count`.
 * @param {Array<{ path: string, count: number }>} counts
 */
export function formatRgCounts(counts) {
  return counts.map((c) => `${c.path}:${c.count}`).join("\n");
}

/**
 * Stars + repo names, same columns as `wits::print_search_results`, plus the
 * description on a second line when present so "libraries that do X"
 * queries are answerable from the plaintext alone.
 *
 * @param {Array<{ full_name: string, stars: number, description?: string | null, language?: string | null }>} items
 */
export function formatSearch(items) {
  if (!items.length) return "No repositories found.";
  const maxName = items.reduce((max, r) => Math.max(max, r.full_name.length), 0);
  const lines = ["", `Found ${items.length} repositories:`, ""];
  for (let i = 0; i < items.length; i += 1) {
    const item = items[i];
    const rank = `${String(i + 1).padStart(3, " ")}.`;
    const name = item.full_name.padEnd(maxName, " ");
    const stars = String(item.stars).padStart(6, " ");
    const lang = item.language ? `  [${item.language}]` : "";
    lines.push(`  ${rank} ${name} ${stars} stars${lang}`);
    if (item.description) {
      lines.push(`       ${String(item.description).replace(/\s+/g, " ").slice(0, 160)}`);
    }
  }
  lines.push("");
  return lines.join("\n");
}

/**
 * `wit refs`-style listing: `* ` marks the default branch.
 * @param {{ default_branch: string, branches: Array<{ name: string, sha: string }>, tags: Array<{ name: string, sha: string }> }} refs
 */
export function formatRefs(refs) {
  const lines = [];
  const width = Math.max(
    0,
    ...refs.branches.map((b) => b.name.length),
    ...refs.tags.map((t) => t.name.length),
  );
  for (const b of refs.branches) {
    const marker = b.name === refs.default_branch ? "*" : " ";
    lines.push(`${marker} branch ${b.name.padEnd(width)}  ${b.sha.slice(0, 12)}`);
  }
  for (const t of refs.tags) {
    lines.push(`  tag    ${t.name.padEnd(width)}  ${t.sha.slice(0, 12)}`);
  }
  return lines.length ? lines.join("\n") : "No refs found.";
}

/**
 * `git log --oneline`-style: `sha  date  author  subject`.
 * @param {Array<{ sha: string, date: string, author: string, message: string }>} commits
 */
export function formatCommits(commits) {
  if (!commits.length) return "No commits found.";
  return commits
    .map((c) => {
      const subject = String(c.message).split("\n")[0];
      return `${c.sha.slice(0, 7)}  ${c.date.slice(0, 10)}  ${c.author}  ${subject}`;
    })
    .join("\n");
}

/**
 * Build a TreeView-like object from recursive list results or slim cache tree.
 * Prefer wasm list recursion; this helper also accepts slim host-cache entries.
 *
 * @param {string} rootLabel
 * @param {Array<{ path: string, type?: string, kind?: string, size?: number, size_bytes?: number }>} files
 */
export function treeViewFromFiles(rootLabel, files) {
  const entries = files
    .filter((e) => (e.type || e.kind) === "blob" || (e.type || e.kind) === "file")
    .map((e) => ({
      path: e.path,
      kind: "file",
      size_bytes: e.size_bytes ?? e.size ?? null,
    }));
  return { root: rootLabel, entries, truncated: false };
}

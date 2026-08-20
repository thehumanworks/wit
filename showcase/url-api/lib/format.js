/**
 * Plaintext formatters matching `wit … --backend memory` stdout
 * (provenance stays on stderr in the CLI; HTTP returns stdout only).
 */

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
        lines.push(`${String(entry.size_bytes).padStart(8)} B  ${entry.name}`);
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
        ? `${relative} (${entry.size_bytes} B)`
        : relative;
    lines.push(`  ${label}`);
  }
  return lines.join("\n");
}

/**
 * Format `wit cat` stdout (optional line numbers via ?n=).
 * @param {string} text
 * @param {{ number?: boolean }} [opts]
 */
export function formatCat(text, opts = {}) {
  if (!opts.number) {
    // Match CLI: print lines without forcing a trailing newline beyond content lines.
    const lines = text.split("\n");
    // If text ended with \n, split yields trailing ""; CLI's .lines() drops it.
    if (lines.length && lines[lines.length - 1] === "") lines.pop();
    return lines.join("\n");
  }
  const lines = text.split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  return lines
    .map((line, i) => `${String(i + 1).padStart(6)}  ${line}`)
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

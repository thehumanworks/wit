/**
 * CLI plaintext for tree / ls / cat.
 *
 * Matches the memory-backend printers in crates/wit/src/cli.rs
 * (`print_snapshot_tree`, `print_snapshot_ls`, and cat stdout).
 * There is no showcase/url-api/lib/format.js in this tree — do not invent
 * a second box-drawing / brochure style.
 */

/**
 * @param {string | null | undefined} path
 * @param {{ path: string, size_bytes?: number | null }[]} files
 * @param {{ long?: boolean }} [opts]
 */
export function formatTree(path, files, opts = {}) {
  const base = normalizePath(path);
  const root = base ? basename(base) : ".";
  const lines = [root];
  const sorted = [...files].sort((a, b) => a.path.localeCompare(b.path));
  for (const entry of sorted) {
    let relative = entry.path;
    if (base) {
      if (relative === base) {
        continue;
      }
      const prefix = `${base}/`;
      if (relative.startsWith(prefix)) {
        relative = relative.slice(prefix.length);
      }
    }
    if (!relative) {
      continue;
    }
    let label = relative;
    if (opts.long && entry.size_bytes != null) {
      label = `${relative} (${entry.size_bytes} B)`;
    }
    lines.push(`  ${label}`);
  }
  return lines.join("\n");
}

/**
 * @param {{ name: string, kind: string }[]} entries
 * @param {{ long?: boolean }} [opts]
 */
export function formatLs(entries, opts = {}) {
  if (!entries.length) {
    return "Directory is empty or does not exist.";
  }
  const lines = [];
  if (opts.long) {
    for (const entry of entries) {
      if (entry.kind === "dir") {
        lines.push(`            ${entry.name}/`);
      } else if (entry.size_bytes != null) {
        const size = String(entry.size_bytes).padStart(8, " ");
        lines.push(`${size} B  ${entry.name}`);
      } else {
        lines.push(`            ${entry.name}`);
      }
    }
  } else {
    for (const entry of entries) {
      lines.push(entry.kind === "dir" ? `${entry.name}/` : entry.name);
    }
  }
  return lines.join("\n");
}

/**
 * @param {{ text?: string } | string} file
 */
export function formatCat(file) {
  if (typeof file === "string") {
    return file;
  }
  return file?.text ?? "";
}

export function normalizePath(value) {
  if (!value) {
    return "";
  }
  return String(value)
    .trim()
    .replaceAll("\\", "/")
    .replace(/^\.\//, "")
    .replace(/^\/+/, "")
    .replace(/\/+$/, "");
}

function basename(path) {
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] || path;
}

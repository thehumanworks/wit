/**
 * CLI plaintext for tree / ls / cat / rg / sed / head / tail.
 *
 * Matches the memory-backend printers in crates/wit/src/cli.rs
 * (`print_snapshot_tree`, `print_snapshot_ls`, cat, rg, sed, head, tail).
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
  return fileText(file);
}

/**
 * `head_from_text` in crates/wit/src/snapshot/memory_ops.rs
 * @param {string} content
 * @param {number} count
 * @param {boolean} [number]
 */
export function headFromText(content, count, number = false) {
  return formatNumbered(rustLines(content).slice(0, count), 1, number);
}

/**
 * `tail_from_text` in crates/wit/src/snapshot/memory_ops.rs
 * @param {string} content
 * @param {number} count
 * @param {number | null} [fromLine]
 * @param {boolean} [number]
 */
export function tailFromText(content, count, fromLine = null, number = false) {
  const all = rustLines(content);
  const total = all.length;
  let selected;
  let startLineNum;
  if (fromLine != null) {
    const skip = Math.max(0, fromLine - 1);
    selected = all.slice(skip);
    startLineNum = fromLine;
  } else {
    const skip = Math.max(0, total - count);
    selected = all.slice(skip);
    startLineNum = skip + 1;
  }
  return formatNumbered(selected, startLineNum, number);
}

/**
 * Documented sed subset: print-range, /re/p, and simple s/a/b/[g].
 * Matches native plaintext for those scripts only — not full POSIX.
 * @param {string} content
 * @param {string} script
 * @param {{ quiet?: boolean }} [opts]
 */
export function formatSed(content, script, opts = {}) {
  const quiet = Boolean(opts.quiet);
  const text = String(script ?? "").trim();
  const lines = rustLines(content);

  const range = text.match(/^(\d+),(\d+)p$/);
  if (range) {
    if (!quiet) {
      throw unsupportedSed(text);
    }
    const start = Number(range[1]);
    const end = Number(range[2]);
    return printSedLines(lines, (index) => index >= start && index <= end);
  }

  const one = text.match(/^(\d+)p$/);
  if (one) {
    if (!quiet) {
      throw unsupportedSed(text);
    }
    const target = Number(one[1]);
    return printSedLines(lines, (index) => index === target);
  }

  const rePrint = text.match(/^\/(.+)\/p$/);
  if (rePrint) {
    if (!quiet) {
      throw unsupportedSed(text);
    }
    const re = compileSedRegex(rePrint[1]);
    return printSedLines(lines, (_index, line) => re.test(line));
  }

  const subst = text.match(/^s\/((?:\\\/|[^/])*)\/((?:\\\/|[^/])*)\/(g)?$/);
  if (subst) {
    const re = compileSedRegex(unescapeSedDelim(subst[1]), Boolean(subst[3]));
    const replacement = unescapeSedDelim(subst[2]);
    const out = lines.map((line) => line.replace(re, replacement));
    if (quiet) {
      return "";
    }
    return printSedLines(out, () => true);
  }

  throw unsupportedSed(text);
}

/**
 * @param {{ path: string, line: number, text: string }[]} matches
 * @param {{ filesWithMatches?: boolean }} [opts]
 */
export function formatRg(matches, opts = {}) {
  if (!matches.length) {
    return "";
  }
  if (opts.filesWithMatches) {
    const seen = [];
    for (const match of matches) {
      if (!seen.includes(match.path)) {
        seen.push(match.path);
      }
    }
    return seen.join("\n");
  }
  return matches.map((match) => `${match.path}:${match.line}:${match.text}`).join("\n");
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

export function fileText(file) {
  if (typeof file === "string") {
    return file;
  }
  return file?.text ?? "";
}

/** Rust `str::lines()` — split on `\n`/`\r\n`, drop a trailing empty line. */
export function rustLines(content) {
  const text = String(content ?? "");
  if (text === "") {
    return [];
  }
  const lines = text.split(/\r?\n/);
  if (lines.length && lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines;
}

function formatNumbered(lines, startLineNum, number) {
  if (!number) {
    return lines.join("\n");
  }
  return lines
    .map((line, i) => `${String(startLineNum + i).padStart(6, " ")}  ${line}`)
    .join("\n");
}

function printSedLines(lines, keep) {
  let out = "";
  for (let i = 0; i < lines.length; i += 1) {
    if (keep(i + 1, lines[i])) {
      out += `${lines[i]}\n`;
    }
  }
  return out;
}

function compileSedRegex(source, global = false) {
  try {
    return new RegExp(source, global ? "g" : "");
  } catch (err) {
    throw new Error(`unsupported sed regex: ${err.message || err}`);
  }
}

function unescapeSedDelim(value) {
  return String(value ?? "").replace(/\\([/\\])/g, "$1");
}

function unsupportedSed(script) {
  return new Error(
    `unsupported sed script ${JSON.stringify(script)} (this page prints ranges, /re/p, and s/a/b/[g] only)`,
  );
}
